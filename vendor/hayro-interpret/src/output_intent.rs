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
    /// CIE L\*a\*b\*, given as ProPhoto RGB (see [`pro_photo`]).
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
    transforms: Mutex<Vec<(Source, RenderingIntent, Option<Xform>)>>,
}

/// A transform into the destination profile: direct, or through a wide-gamut RGB space when
/// moxcms can't join the two profiles (a CMYK source into a CMYK destination).
#[derive(Clone)]
enum Xform {
    Direct(Arc<TransformF32Executor>),
    /// Source to ProPhoto RGB, then on into the destination.
    ViaPcs(Arc<TransformF32Executor>, Arc<TransformF32Executor>),
}

impl Xform {
    fn run(&self, input: &[f32], out: &mut [f32]) -> Option<()> {
        match self {
            Self::Direct(t) => t.transform(input, out).ok(),
            Self::ViaPcs(to_pcs, from_pcs) => {
                let n = out.len() / 4;
                let mut pcs = vec![0.0f32; n.checked_mul(3)?];
                to_pcs.transform(input, &mut pcs).ok()?;
                from_pcs.transform(&pcs, out).ok()
            }
        }
    }
}

impl std::fmt::Debug for OutputIntent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OutputIntent {..}")
    }
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
        self.to_ink(&Source::Lab, intent, pro_photo(lab))
    }

    /// PdfCraft patch (73): `input`, colours of `src`'s components each, as ink tints of the
    /// destination profile under `intent`, four a colour. `None` when no transform can be
    /// built or `input` isn't whole colours.
    pub(crate) fn icc_to_ink(&self, src: &ICCProfile, intent: RenderingIntent, input: &[f32]) -> Option<Vec<f32>> {
        let n = src.number_components();
        if n == 0 || input.len() % n != 0 {
            return None;
        }
        let transform = self.transform(&Source::Icc(src.clone()), intent)?;
        let clean: Vec<f32> = input.iter().map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }).collect();
        let mut out = vec![0.0f32; (input.len() / n).checked_mul(4)?];
        transform.run(&clean, &mut out)?;
        Some(out.into_iter().map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }).collect())
    }

    fn to_ink(&self, src: &Source, intent: RenderingIntent, rgb: [f32; 3]) -> Option<[f32; 4]> {
        let transform = self.transform(src, intent)?;
        let input = rgb.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 });
        let mut out = [0.0f32; 4];
        transform.run(&input, &mut out)?;
        Some(out.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }))
    }

    /// The transform from `src` under `intent`, built once; a failure is remembered too.
    fn transform(&self, src: &Source, intent: RenderingIntent) -> Option<Xform> {
        // A poisoned lock only means another thread panicked mid-push; the list is still whole.
        let mut transforms = self.transforms.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, _, t)) = transforms.iter().find(|(p, i, _)| p.same(src) && *i == intent) {
            return t.clone();
        }
        let options = TransformOptions {
            rendering_intent: intent.to_moxcms(),
            ..TransformOptions::default()
        };
        let lab;
        let (profile, layout) = match src {
            Source::Icc(icc) => (icc.src_profile(), icc.layout()),
            Source::Lab => {
                lab = ColorProfile::new_pro_photo_rgb();
                (&lab, Layout::Rgb)
            }
        };
        let transform = match profile.create_transform_f32(layout, &self.dest, Layout::Rgba, options) {
            Ok(t) => Some(Xform::Direct(t)),
            Err(_) => {
                // moxcms joins a CMYK source to an RGB destination only; ProPhoto's gamut holds
                // every print gamut, and its matrix has no intent of its own.
                let pcs = ColorProfile::new_pro_photo_rgb();
                let to_pcs = profile.create_transform_f32(layout, &pcs, Layout::Rgb, options).ok();
                let from_pcs = pcs.create_transform_f32(Layout::Rgb, &self.dest, Layout::Rgba, options).ok();
                to_pcs.zip(from_pcs).map(|(a, b)| Xform::ViaPcs(a, b))
            }
        };
        if transforms.len() >= MAX_TRANSFORMS {
            transforms.remove(0);
        }
        transforms.push((src.clone(), intent, transform.clone()));
        transform
    }
}

/// D50 white (ISO 32000-2 §8.6.5.4 converts `Lab` through the space's white point; the ICC
/// connection space is D50).
const D50: [f32; 3] = [0.9642, 1.0, 0.8249];

/// PdfCraft patch (79): `lab` as ProPhoto RGB, whose D50 white and gamut hold nearly every
/// colour `Lab` can name; the rare one outside is clipped. moxcms's own Lab source profile
/// gives a different ink from the destination's tables (a neutral `60 0 0` loses a sixth of
/// its black), while a matrix RGB source converts as other colour engines do.
fn pro_photo([l, a, b]: [f32; 3]) -> [f32; 3] {
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
    let xyz = [g(fx) * wx, g(fy) * wy, g(fz) * wz];
    // D50 XYZ to linear ProPhoto (ROMM) RGB, then its 1.8 gamma.
    const TO_RGB: [[f32; 3]; 3] = [
        [1.345_943_3, -0.255_607_5, -0.051_111_8],
        [-0.544_598_9, 1.508_167_3, 0.020_535_1],
        [0.0, 0.0, 1.211_812_8],
    ];
    TO_RGB.map(|row| {
        let v = row[0] * xyz[0] + row[1] * xyz[1] + row[2] * xyz[2];
        if v.is_finite() {
            v.clamp(0.0, 1.0).powf(1.0 / 1.8)
        } else {
            0.0
        }
    })
}
