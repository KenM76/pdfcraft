//! PdfCraft patch (70): an isolated transparency group that blends in `DeviceCMYK`.
//!
//! ISO 32000-2 §11.3.4: a group's elements blend in the group's colour space, and the
//! components of a subtractive space are complemented before the blend function and
//! complemented back after it. The renderer composites in RGB, which gives other colours
//! wherever a blend mode meets CMYK artwork: `0 1 0 0 k` (magenta) under `0 0 0 1 k` (black)
//! with `/BM /Difference` is green when blended in CMYK (`|(1,0,1,1) − (1,1,1,0)|`
//! complemented back is `1 0 1 0`) and magenta when blended in RGB.
//!
//! An *isolated* group starts from a transparent backdrop of its own (§11.4.7), so its
//! interior can be computed apart from the page and handed back as finished pixels. A device
//! that can blend in CMYK takes such a group through [`Device::draw_isolated_cmyk_group`] and
//! interprets it with [`CmykGroup::interpret`] as often as it needs; every other device, and a
//! device that declines, gets the group's contents as before.
//!
//! PdfCraft patch (68): a page whose group blends in `DeviceCMYK` takes the same path. The page
//! group is always isolated (§11.4.7), so its contents are such a group too, composited onto
//! the paper after they are converted out of CMYK.

use crate::InterpreterSettings;
use crate::context::{Context, InterpreterCache};
use crate::device::Device;
use crate::interpret::interpret_page_contents;
use crate::interpret::state::{ClipType, State};
use crate::x_object::{FormXObject, interpret_form_contents};
use hayro_syntax::object::dict::keys::{BM, CS, EXT_G_STATE, FONT, GROUP, K, OP, OP_NS, PATTERN, RESOURCES, XOBJECT};
use hayro_syntax::object::{Array, Dict, Name, ObjectIdentifier, Stream};
use hayro_syntax::page::{Page, Resources};
use hayro_syntax::xref::XRef;
use kurbo::{Affine, Rect, Shape};
use std::cell::Cell;
use std::ops::Deref;

/// An isolated, non-knockout transparency group whose blending colour space is `DeviceCMYK`,
/// captured where it is painted (PdfCraft patch (70); see the module documentation).
pub struct CmykGroup<'a, 'b> {
    contents: Contents<'a, 'b>,
    resources: &'b Resources<'a>,
    /// The graphics state the group's contents start with: the state at `Do`, after the
    /// form's `/Matrix`, with the group's own opacity, soft mask and blend mode already taken
    /// by the layer the group is composited through.
    state: State<'a>,
    root_transform: Affine,
    bbox: Rect,
    interpreter_cache: InterpreterCache<'a>,
    settings: InterpreterSettings,
    xref: &'a XRef,
    nesting_depth: u32,
    /// Whether a page's contents were charged to its content budget yet (patch (68)).
    charged: Cell<bool>,
    /// PdfCraft patch (72): whether the contents may set overprint.
    overprint: bool,
    /// PdfCraft patch (72): whether the contents blend in `DeviceCMYK` without overprint, so
    /// the two-pass drawing suits them when the per-ink one can't be done.
    blends: bool,
}

/// What a [`CmykGroup`] draws: a form's contents, or a page's (patch (68)).
enum Contents<'a, 'b> {
    Form(&'b FormXObject<'a>),
    Page(&'b Page<'a>),
}

impl<'a, 'b> CmykGroup<'a, 'b> {
    pub(crate) fn new(form: &'b FormXObject<'a>, resources: &'b Resources<'a>, context: &Context<'a>) -> Self {
        Self::with(Contents::Form(form), resources, context)
    }

    /// PdfCraft patch (68): the contents of `page`, whose group blends in `DeviceCMYK`.
    pub(crate) fn page(page: &'b Page<'a>, context: &Context<'a>, overprint: bool, blends: bool) -> Self {
        Self { overprint, blends, ..Self::with(Contents::Page(page), page.resources(), context) }
    }

    /// PdfCraft patch (72): whether the group may be drawn ignoring overprint, as two passes,
    /// when [`Self::overprints`] and the per-ink drawing fails.
    pub fn blends(&self) -> bool {
        self.blends
    }

    /// PdfCraft patch (72): whether the contents may set overprint, so each ink needs a pass
    /// of its own.
    pub fn overprints(&self) -> bool {
        self.overprint
    }

    fn with(contents: Contents<'a, 'b>, resources: &'b Resources<'a>, context: &Context<'a>) -> Self {
        let mut state = context.get().clone();
        // The clips in force belong to the device the group is composited onto; contents
        // interpreted on their own push and pop only the clips they set themselves.
        state.clips = Vec::<ClipType>::new();
        Self {
            contents,
            resources,
            state,
            root_transform: context.root_transform(),
            bbox: context.bbox(),
            interpreter_cache: context.interpreter_cache.clone(),
            settings: context.settings.clone(),
            xref: context.xref,
            nesting_depth: context.nesting_depth(),
            charged: Cell::new(false),
            overprint: false,
            blends: true,
        }
    }

    /// The device-space box the group can paint: its `/BBox` under the current transform,
    /// within the area being drawn (for a page, that area).
    pub fn device_bbox(&self) -> Rect {
        let Contents::Form(form) = self.contents else {
            return self.bbox;
        };
        let [x0, y0, x1, y1] = form.bbox;
        let user = Rect::new(x0 as f64, y0 as f64, x1 as f64, y1 as f64);
        let device = (self.state.ctm * user.to_path(0.1)).bounding_box();
        device.intersect(self.bbox)
    }

    /// PdfCraft patch (71): the document's output intent, if it has a CMYK one.
    pub fn output_intent(&self) -> Option<std::sync::Arc<crate::OutputIntent>> {
        crate::OutputIntent::of(self.xref, &self.interpreter_cache.object_cache)
    }

    /// Whether a page's contents were charged to its content budget by [`Self::interpret`].
    pub(crate) fn charged(&self) -> bool {
        self.charged.get()
    }

    /// Interpret the group's contents into `device`, with device space moved by `offset`
    /// (so a device covering only [`Self::device_bbox`] can start at its corner).
    pub fn interpret(&self, device: &mut impl Device<'a>, offset: (f64, f64)) {
        let shift = Affine::translate((-offset.0, -offset.1));
        let mut state = self.state.clone();
        state.ctm = shift * state.ctm;
        let mut context = Context::new_with(
            shift * self.root_transform,
            shift.transform_rect_bbox(self.bbox),
            &self.interpreter_cache,
            self.xref,
            self.settings.clone(),
            state,
            self.nesting_depth,
        );
        match self.contents {
            Contents::Form(form) => interpret_form_contents(self.resources, form, &mut context, device),
            Contents::Page(page) => {
                let charge = !self.charged.replace(true);
                interpret_page_contents(page, self.resources, &mut context, device, charge);
            }
        }
    }
}

/// Whether `form`'s transparency group blends in `DeviceCMYK`: it says so in its `/CS`.
pub(crate) fn blends_in_device_cmyk(form: &Dict<'_>) -> bool {
    form.get::<Dict<'_>>(GROUP).and_then(|g| g.get::<Name<'_>>(CS)).is_some_and(|cs| matches!(cs.deref(), b"DeviceCMYK" | b"CMYK"))
}

/// Whether `form`'s transparency group names a blending colour space of its own (`/CS`).
pub(crate) fn names_a_blending_space(form: &Dict<'_>) -> bool {
    form.get::<Dict<'_>>(GROUP).is_some_and(|g| g.contains_key(CS))
}

/// PdfCraft patch (68): whether `page`'s group is a knockout group (`/K true`), which the CMYK
/// passes don't draw, as for a form.
pub(crate) fn page_knocks_out(page: &Page<'_>) -> bool {
    page.raw().get::<Dict<'_>>(GROUP).and_then(|g| g.get::<bool>(K)).unwrap_or(false)
}

/// PdfCraft patch (68): the most resource dictionaries [`page_blends`] reads (the page's, then
/// one per form, tiling pattern or Type 3 font it reaches).
const MAX_SCANNED_RESOURCES: usize = 512;
/// PdfCraft patch (68): how deep [`page_blends`] follows forms, patterns and fonts into each other.
const MAX_SCAN_DEPTH: u32 = 16;
/// PdfCraft patch (68): the most graphics states and named resources [`page_blends`] looks at.
const MAX_SCANNED_ENTRIES: usize = 16_384;

/// PdfCraft patch (68): whether anything `page` draws may use a blend mode other than
/// `Normal`: some graphics state in its resources (inherited ones too), or in those of a form,
/// tiling pattern or Type 3 font it reaches, names one. Bounded; a resource tree too large to
/// read counts as blending.
pub(crate) fn page_blends(page: &Page<'_>) -> bool {
    page_scan(page, names_a_blend_mode)
}

/// PdfCraft patch (72): whether anything `page` draws may set overprint (`/OP` or `/op` true),
/// read as [`page_blends`] reads blend modes.
pub(crate) fn page_overprints(page: &Page<'_>) -> bool {
    page_scan(page, sets_overprint)
}

fn page_scan(page: &Page<'_>, wanted: fn(&Dict<'_>) -> bool) -> bool {
    let mut scan = BlendScan { wanted: Some(wanted), ..BlendScan::default() };
    let mut level = Some(page.resources());
    while let Some(resources) = level {
        let found = scan.level(&resources.ext_g_states, [&resources.x_objects, &resources.patterns, &resources.fonts], 0);
        if found != Some(false) {
            return true;
        }
        level = resources.parent();
    }
    while let Some((resources, depth)) = scan.pending.pop() {
        let lists = [XOBJECT, PATTERN, FONT].map(|key| resources.get::<Dict<'_>>(key).unwrap_or_default());
        let states = resources.get::<Dict<'_>>(EXT_G_STATE).unwrap_or_default();
        if scan.level(&states, [&lists[0], &lists[1], &lists[2]], depth) != Some(false) {
            return true;
        }
    }
    false
}

/// [`page_blends`]'s walk: resource dictionaries still to read, and what it has read.
#[derive(Default)]
struct BlendScan<'a> {
    pending: Vec<(Dict<'a>, u32)>,
    seen: std::collections::HashSet<ObjectIdentifier>,
    resources: usize,
    entries: usize,
    /// What a graphics state is looked for.
    wanted: Option<fn(&Dict<'_>) -> bool>,
}

impl<'a> BlendScan<'a> {
    /// Reads one resource dictionary's graphics states and queues the resources of the forms,
    /// patterns and fonts it names. `Some(true)`: a blend mode; `None`: over a limit.
    fn level(&mut self, states: &Dict<'a>, lists: [&Dict<'a>; 3], depth: u32) -> Option<bool> {
        self.resources += 1;
        if self.resources > MAX_SCANNED_RESOURCES || depth > MAX_SCAN_DEPTH {
            return None;
        }
        for key in states.keys() {
            self.count()?;
            let wanted = self.wanted.unwrap_or(names_a_blend_mode);
            if states.get::<Dict<'_>>(key.deref()).is_some_and(|gs| wanted(&gs)) {
                return Some(true);
            }
        }
        for list in lists {
            for key in list.keys() {
                self.count()?;
                // Only what has resources of its own can reach more graphics states: an image
                // has none, and neither does a shading pattern or another font.
                let child = list.get::<Stream<'_>>(key.deref()).map(|s| (Some(s.obj_id()), s.dict().clone()));
                let Some((id, dict)) = child.or_else(|| list.get::<Dict<'_>>(key.deref()).map(|d| (d.obj_id(), d))) else {
                    continue;
                };
                let Some(inner) = dict.get::<Dict<'_>>(RESOURCES) else {
                    continue;
                };
                if id.is_none_or(|id| self.seen.insert(id)) {
                    self.pending.push((inner, depth + 1));
                }
            }
        }
        Some(false)
    }

    fn count(&mut self) -> Option<()> {
        self.entries += 1;
        (self.entries <= MAX_SCANNED_ENTRIES).then_some(())
    }
}

/// PdfCraft patch (72): whether graphics state `gs` turns overprint on.
fn sets_overprint(gs: &Dict<'_>) -> bool {
    [OP, OP_NS].into_iter().any(|key| gs.get::<bool>(key).unwrap_or(false))
}

/// Whether graphics state `gs` sets `/BM` to anything but `Normal` (or its old name).
fn names_a_blend_mode(gs: &Dict<'_>) -> bool {
    let plain = |n: &Name<'_>| matches!(n.deref(), b"Normal" | b"Compatible");
    if let Some(name) = gs.get::<Name<'_>>(BM) {
        return !plain(&name);
    }
    gs.get::<Array<'_>>(BM).is_some_and(|modes| modes.iter::<Name<'_>>().any(|n| !plain(&n)))
}
