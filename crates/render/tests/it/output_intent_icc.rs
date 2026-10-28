//! On a page whose document has a CMYK output intent (ISO 32000-2 §14.11.5), an `ICCBased`
//! colour is drawn as the output device prints it: through the output intent's profile under
//! the rendering intent (§8.6.5.8), then shown as that CMYK is shown (hayro-interpret patch
//! (73)). Paints and images both take that route.
//!
//! Each page has a reference square on the left, painted in `DeviceCMYK` with the ink the
//! output intent gives sRGB blue (computed here with the same colour engine), and sRGB blue on
//! the right: drawn right, the two match. Without the output intent, sRGB blue is pure blue.

use moxcms::{ColorProfile, Layout, LutDataType, LutStore, LutType, LutWarehouse, Matrix3d, RenderingIntent, TransformOptions};
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RenderedPage};
use std::sync::Arc;

/// The renderer's bundled CMYK profile; it converts from CMYK only.
const CMYK_PROFILE: &[u8] = include_bytes!("../../../../vendor/hayro-interpret/assets/CGATS001Compat-v2-micro.icc");

/// The document's output profile: the bundled profile with a two-point `lut16` from Lab into
/// CMYK (C = 1 - L, M = a, Y = 1 - b, K = 0, all normalised), so colours can convert into it.
/// The renderer and [`ink`] read the same bytes, so the table need only be valid.
fn output_profile() -> Vec<u8> {
    let mut clut = Vec::new();
    for l in [0u16, 65535] {
        for a in [0u16, 65535] {
            for b in [0u16, 65535] {
                clut.extend_from_slice(&[65535 - l, a, 65535 - b, 0]);
            }
        }
    }
    profile_with(clut)
}

/// An output profile that prints every colour as one ink, `GREEN`: a green sRGB can't show,
/// so the screen colour can't be turned back into the same ink.
fn green_profile() -> Vec<u8> {
    let green = GREEN.map(|t| (t * 65535.0) as u16);
    profile_with(green.repeat(8))
}

const GREEN: [f32; 4] = [0.85, 0.0, 1.0, 0.0];

/// The bundled profile with a two-point `lut16` from Lab into CMYK whose grid is `clut`.
fn profile_with(clut: Vec<u16>) -> Vec<u8> {
    let mut profile = ColorProfile::new_from_slice(CMYK_PROFILE).unwrap();
    let lut = LutWarehouse::Lut(LutDataType {
        num_input_channels: 3,
        num_output_channels: 4,
        num_clut_grid_points: 2,
        matrix: Matrix3d::IDENTITY,
        num_input_table_entries: 2,
        num_output_table_entries: 2,
        input_table: LutStore::Store16([0, 65535].repeat(3)),
        clut_table: LutStore::Store16(clut),
        output_table: LutStore::Store16([0, 65535].repeat(4)),
        lut_type: LutType::Lut16,
    });
    profile.lut_b_to_a_perceptual = Some(lut.clone());
    profile.lut_b_to_a_colorimetric = Some(lut.clone());
    profile.lut_b_to_a_saturation = Some(lut);
    profile.encode().unwrap()
}

fn srgb_profile() -> Vec<u8> {
    ColorProfile::new_srgb().encode().unwrap()
}

/// The ink the output profile gives sRGB `rgb` under relative colorimetric, the default intent.
fn ink(rgb: [f32; 3]) -> [f32; 4] {
    let dest = ColorProfile::new_from_slice(&output_profile()).unwrap();
    let options = TransformOptions { rendering_intent: RenderingIntent::RelativeColorimetric, ..TransformOptions::default() };
    let t = ColorProfile::new_srgb().create_transform_f32(Layout::Rgb, &dest, Layout::Rgba, options).unwrap();
    let mut out = [0.0f32; 4];
    t.transform(&rgb, &mut out).unwrap();
    out
}

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut out = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\nendstream");
    out
}

/// A 200 x 100 page: the reference square (10..90) in the ink for sRGB blue, then `content`
/// with `/CS0` an sRGB `ICCBased` space and `/Im0` a 1 x 1 sRGB-blue image in it. The catalog
/// names the CMYK output intent when `intent`.
fn render(intent: bool, content: &str) -> RenderedPage {
    let [c, m, y, k] = ink([0.0, 0.0, 1.0]);
    let reference = format!("{c:.4} {m:.4} {y:.4} {k:.4} k 10 10 80 80 re f");
    render_in(if intent { Some(output_profile()) } else { None }, "", "", &reference, content)
}

/// The page [`render`] draws, with `profile` the output intent's, `group` the page group,
/// `states` the `ExtGState` resources and `reference` the reference square.
fn render_in(profile: Option<Vec<u8>>, group: &str, states: &str, reference: &str, content: &str) -> RenderedPage {
    let body = format!("{reference} {content}");
    let intent = profile.is_some();
    let intents = if intent {
        "/OutputIntents [<< /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier (Synthetic) /DestOutputProfile 7 0 R >>]"
    } else {
        ""
    };
    let objects: Vec<Vec<u8>> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {intents} >>").into_bytes(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R {group} /Resources << /ExtGState << {states} >> /ColorSpace << /CS0 [/ICCBased 5 0 R] /L [/Lab << /WhitePoint [0.9642 1 0.8249] >>] >> /XObject << /Im0 6 0 R /Im1 8 0 R >> >> >>").into_bytes(),
        stream("", body.as_bytes()),
        stream("/N 3", &srgb_profile()),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace [/ICCBased 5 0 R]", &[0, 0, 255]),
        stream("/N 4", &profile.unwrap_or_default()),
        stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace [/Indexed [/ICCBased 5 0 R] 0 <0000FF>]", &[0]),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        bytes.extend_from_slice(object);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    let mut renderer = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(page.error.is_none(), "{content}: {:?}", page.error);
    page
}

/// The colour at PDF point (x, y).
fn at(page: &RenderedPage, x: u32, y: u32) -> [u8; 3] {
    let offset = usize::try_from(((100 - y) * page.width + x) * 4).unwrap();
    [page.rgba[offset], page.rgba[offset + 1], page.rgba[offset + 2]]
}

fn close(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= 4)
}

const PAINT: &str = "/CS0 cs 0 0 1 sc 110 10 80 80 re f";
const IMAGE: &str = "q 80 0 0 80 110 10 cm /Im0 Do Q";
const INDEXED_IMAGE: &str = "q 80 0 0 80 110 10 cm /Im1 Do Q";

#[test]
fn an_icc_colour_is_drawn_through_the_output_intent() {
    let page = render(true, PAINT);
    let (reference, drawn) = (at(&page, 50, 50), at(&page, 150, 50));
    assert!(close(drawn, reference), "drawn {drawn:?}, reference {reference:?}");
}

#[test]
fn an_icc_image_is_drawn_through_the_output_intent() {
    let page = render(true, IMAGE);
    let (reference, drawn) = (at(&page, 50, 50), at(&page, 150, 50));
    assert!(close(drawn, reference), "drawn {drawn:?}, reference {reference:?}");
}

/// Control: with no output intent, sRGB blue stays pure blue, which the print ink can't reach.
#[test]
fn without_an_output_intent_an_icc_colour_is_drawn_as_its_profile_says() {
    for content in [PAINT, IMAGE] {
        let page = render(false, content);
        let (reference, drawn) = (at(&page, 50, 50), at(&page, 150, 50));
        assert!(close(drawn, [0, 0, 255]), "{content}: {drawn:?}");
        assert!(!close(drawn, reference), "{content}: the print ink {reference:?} is not pure blue");
    }
}

const CMYK_GROUP: &str = "/Group << /S /Transparency /CS /DeviceCMYK >>";

/// In a `DeviceCMYK` page group an ICC image deposits the ink the output intent gives it
/// (patch (75)), even one sRGB can't show: drawn as the output intent's colour and turned back
/// into ink, the green came back as another ink and showed as a different green.
fn assert_prints_green(states: &str) {
    let [c, m, y, k] = GREEN;
    let reference = format!("{c} {m} {y} {k} k 10 10 80 80 re f");
    for content in [IMAGE, INDEXED_IMAGE] {
        let page = render_in(Some(green_profile()), CMYK_GROUP, states, &reference, content);
        let (reference, drawn) = (at(&page, 50, 50), at(&page, 150, 50));
        assert!(close(drawn, reference), "{states} {content}: drawn {drawn:?}, reference {reference:?}");
    }
}

/// Drawn in two passes, cyan-magenta-yellow and black, as a group with blend modes is.
#[test]
fn in_a_blending_cmyk_page_group_an_icc_image_prints_the_output_intents_ink() {
    assert_prints_green("/BM << /BM /Multiply >>");
}

/// Drawn one ink at a time, as a group with overprint is.
#[test]
fn in_an_overprinting_cmyk_page_group_an_icc_image_prints_the_output_intents_ink() {
    assert_prints_green("/OP << /OP true /op true >>");
}

/// `Lab` neutrals (L\*) with sRGB's grey of each: the same colour, so the same ink.
const LAB_GREYS: [(u8, f32); 2] = [(20, 0.1902), (60, 0.5647)];

/// A `Lab` colour painted into a `DeviceCMYK` page group deposits the ink the output intent
/// gives it (patch (79)): converted through moxcms's Lab profile, darker neutrals came out as
/// a lighter ink.
fn assert_lab_prints_its_ink(states: &str) {
    for (l, grey) in LAB_GREYS {
        let [c, m, y, k] = ink([grey; 3]);
        let reference = format!("{c:.4} {m:.4} {y:.4} {k:.4} k 10 10 80 80 re f");
        let content = format!("/L cs {l} 0 0 sc 110 10 80 80 re f");
        let page = render_in(Some(output_profile()), CMYK_GROUP, states, &reference, &content);
        let (reference, drawn) = (at(&page, 50, 50), at(&page, 150, 50));
        assert!(close(drawn, reference), "{states} L* {l}: drawn {drawn:?}, reference {reference:?}");
    }
}

#[test]
fn in_a_blending_cmyk_page_group_a_lab_colour_prints_the_output_intents_ink() {
    assert_lab_prints_its_ink("/BM << /BM /Multiply >>");
}

#[test]
fn in_an_overprinting_cmyk_page_group_a_lab_colour_prints_the_output_intents_ink() {
    assert_lab_prints_its_ink("/OP << /OP true /op true >>");
}
