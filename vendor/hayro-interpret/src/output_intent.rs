//! PdfCraft patch (71): the document's output intent (ISO 32000-2 §14.11.5), the CMYK
//! profile an ICC-based colour is converted through, under its rendering intent (§8.6.5.8),
//! when it is painted into a `DeviceCMYK` blending space.

use crate::cache::Cache;
use crate::color::{ICCProfile, RenderingIntent};
use crate::util::hash128;
use hayro_syntax::object::dict::keys::{DEST_OUTPUT_PROFILE, OUTPUT_INTENTS};
use hayro_syntax::object::{Array, Dict, Stream};
use hayro_syntax::xref::XRef;
use moxcms::{ColorProfile, DataColorSpace, Layout, TransformF32Executor, TransformOptions};
use std::sync::{Arc, Mutex};

/// Transforms kept per output intent; a page rarely uses more than a few source profiles.
const MAX_TRANSFORMS: usize = 16;

/// What a transform converts from.
#[derive(Clone)]
enum Source {
    Icc(ICCProfile),
    /// CIE L\*a\*b\*, given as encoded D50 XYZ (see [`pcs_xyz`]).
    Lab,
}

impl Source {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Icc(a), Self::Icc(b)) => a.same(b),
            (Self::Lab, Self::Lab) => true,
            _ => false,
        }
    }
}

/// The first output intent whose destination profile is a CMYK one, with the transforms
/// built into it so far.
pub struct OutputIntent {
    dest: ColorProfile,
    transforms: Mutex<Vec<(Source, RenderingIntent, Arc<TransformF32Executor>)>>,
}

impl OutputIntent {
    /// The output intent of the document `xref` reads, through `cache`; `None` when it has
    /// none with a usable CMYK destination profile.
    pub(crate) fn of(xref: &XRef, cache: &Cache) -> Option<Arc<Self>> {
        let key = hash128(&("pdfcraft-output-intent", xref.root_id()));
        cache.get_or_insert_with(key, || Self::read(xref).map(Arc::new))
    }

    fn read(xref: &XRef) -> Option<Self> {
        let catalog = xref.get::<Dict<'_>>(xref.root_id())?;
        let intents = catalog.get::<Array<'_>>(OUTPUT_INTENTS)?;
        intents.iter::<Dict<'_>>().find_map(|intent| {
            let stream = intent.get::<Stream<'_>>(DEST_OUTPUT_PROFILE)?;
            let data = stream.decoded().ok()?;
            let dest = ColorProfile::new_from_slice(&data).ok()?;
            (dest.color_space == DataColorSpace::Cmyk).then(|| Self {
                dest,
                transforms: Mutex::new(Vec::new()),
            })
        })
    }

    /// `rgb`, in the three-component space `src`, as ink tints `[C, M, Y, K]` (0 = no ink) of
    /// the destination profile under `intent`. `None` when no transform can be built.
    pub(crate) fn rgb_to_ink(&self, src: &ICCProfile, intent: RenderingIntent, rgb: [f32; 3]) -> Option<[f32; 4]> {
        self.to_ink(&Source::Icc(src.clone()), intent, rgb)
    }

    /// `lab`, CIE L\*a\*b\* (L\* 0..=100), as ink tints of the destination profile under
    /// `intent`, for a `Lab` colour painted into a `DeviceCMYK` blending space.
    pub(crate) fn lab_to_ink(&self, intent: RenderingIntent, lab: [f32; 3]) -> Option<[f32; 4]> {
        self.to_ink(&Source::Lab, intent, pcs_xyz(lab))
    }

    fn to_ink(&self, src: &Source, intent: RenderingIntent, rgb: [f32; 3]) -> Option<[f32; 4]> {
        let transform = self.transform(src, intent)?;
        let input = rgb.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 });
        let mut out = [0.0f32; 4];
        transform.transform(&input, &mut out).ok()?;
        Some(out.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }))
    }

    fn transform(&self, src: &Source, intent: RenderingIntent) -> Option<Arc<TransformF32Executor>> {
        // A poisoned lock only means another thread panicked mid-push; the list is still whole.
        let mut transforms = self.transforms.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, _, t)) = transforms.iter().find(|(p, i, _)| p.same(src) && *i == intent) {
            return Some(t.clone());
        }
        let options = TransformOptions {
            rendering_intent: intent.to_moxcms(),
            ..TransformOptions::default()
        };
        let lab;
        let profile = match src {
            Source::Icc(icc) => icc.src_profile(),
            Source::Lab => {
                lab = ColorProfile::new_lab();
                &lab
            }
        };
        let transform = profile
            .create_transform_f32(Layout::Rgb, &self.dest, Layout::Rgba, options)
            .ok()?;
        if transforms.len() >= MAX_TRANSFORMS {
            transforms.remove(0);
        }
        transforms.push((src.clone(), intent, transform.clone()));
        Some(transform)
    }
}

/// D50 white (ISO 32000-2 §8.6.5.4 converts `Lab` through the space's white point; the ICC
/// connection space is D50).
const D50: [f32; 3] = [0.9642, 1.0, 0.8249];

/// `lab` as D50 XYZ in the ICC connection space's encoding (1.0 = 65535/32768), which is what
/// moxcms's generic Lab profile, an identity into an XYZ connection space, takes as input.
fn pcs_xyz([l, a, b]: [f32; 3]) -> [f32; 3] {
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let g = |f: f32| {
        if f > 6.0 / 29.0 {
            f * f * f
        } else {
            3.0 * (6.0f32 / 29.0).powi(2) * (f - 4.0 / 29.0)
        }
    };
    let [wx, wy, wz] = D50;
    [g(fx) * wx, g(fy) * wy, g(fz) * wz].map(|v| v * (32768.0 / 65535.0))
}
