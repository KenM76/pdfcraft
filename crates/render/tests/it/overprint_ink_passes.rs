//! Overprint (ISO 32000-2 §8.6.7, §11.7.4.5) on a page whose blending space is `DeviceCMYK`:
//! the page is drawn one ink at a time, and a paint with overprint on deposits only the inks
//! it names (hayro-interpret patch (72)).
//!
//! Each page puts a cyan box on the left and an overprinting box over its right half, plus
//! reference squares on the right. Drawn right, the overlap shows both inks: it matches the
//! reference painted with both, or, for a spot ink, the cyan and the spot multiplied. Drawn
//! without overprint, the overlap shows the top box alone.

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RenderedPage};
use std::sync::Arc;

const SPOT: &str = "[/Separation /Spot /DeviceCMYK << /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 1 0 0] /N 1 >>]";

fn stream(dict: &str, data: &str) -> Vec<u8> {
    format!("<< {dict} /Length {} >>\nstream\n{data}\nendstream", data.len()).into_bytes()
}

/// A 300 x 100 page with a `DeviceCMYK` page group: a cyan box (10..110), then `top` drawn
/// under `/GS` (overprint on, mode `opm`), then `refs`. `/S` is a magenta spot ink, `/I` an
/// Indexed space over it, `/Sh` an axial shading in it at full tint.
fn render(opm: u8, top: &str, refs: &str) -> RenderedPage {
    let body = format!("1 0 0 0 k 10 10 100 80 re f q /GS gs {top} Q {refs}");
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 100] /Contents 4 0 R \
             /Group << /S /Transparency /CS /DeviceCMYK /I true >> \
             /Resources << /ExtGState << /GS << /OP true /op true /OPM {opm} >> >> \
             /ColorSpace << /S {SPOT} /I [/Indexed {SPOT} 1 <00FF>] >> \
             /Shading << /Sh << /ShadingType 2 /ColorSpace {SPOT} /Coords [0 0 300 0] \
             /Function << /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [1] /N 1 >> >> >> >> >>"
        )
        .into_bytes(),
        stream("", &body),
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
    assert!(page.error.is_none(), "{top}: {:?}", page.error);
    page
}

/// The colour at PDF point (x, y).
fn at(page: &RenderedPage, x: u32, y: u32) -> [u8; 3] {
    let offset = usize::try_from(((100 - y) * page.width + x) * 4).unwrap();
    [page.rgba[offset], page.rgba[offset + 1], page.rgba[offset + 2]]
}

fn close(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= 6)
}

/// Two colours multiplied, as overlapping inks print.
fn multiply(a: [u8; 3], b: [u8; 3]) -> [u8; 3] {
    [0, 1, 2].map(|i| u8::try_from(u16::from(a[i]) * u16::from(b[i]) / 255).unwrap())
}

/// The top box (60..160) in process magenta; a reference square (200..240) in cyan plus magenta.
const PROCESS: (&str, &str) = ("0 1 0 0 k 60 10 100 80 re f", "1 1 0 0 k 200 30 40 40 re f");
/// Reference squares: the spot alone at 200..240.
const SPOT_REF: &str = "/S cs 1 scn 200 30 40 40 re f";

/// The overlap (85, 50) must show cyan and the spot ink multiplied.
fn assert_spot_overprints(top: &str) {
    let page = render(0, top, SPOT_REF);
    let (cyan, spot, overlap) = (at(&page, 35, 50), at(&page, 220, 50), at(&page, 85, 50));
    let want = multiply(cyan, spot);
    assert!(close(overlap, want), "{top}: overlap {overlap:?}, cyan {cyan:?} x spot {spot:?} = {want:?}");
}

/// OPM 1: a `DeviceCMYK` paint's zero components leave those inks alone.
#[test]
fn overprint_mode_one_keeps_the_process_inks_a_paint_leaves_at_zero() {
    let page = render(1, PROCESS.0, PROCESS.1);
    let (overlap, both) = (at(&page, 85, 50), at(&page, 220, 50));
    assert!(close(overlap, both), "overlap {overlap:?}, cyan plus magenta {both:?}");
}

/// Control: OPM 0 knocks out every process ink a `DeviceCMYK` paint covers, zero or not.
#[test]
fn overprint_mode_zero_knocks_out_the_process_inks() {
    let page = render(0, PROCESS.0, "");
    let (overlap, magenta) = (at(&page, 85, 50), at(&page, 135, 50));
    assert!(close(overlap, magenta), "overlap {overlap:?}, magenta {magenta:?}");
}

/// A spot ink keeps every process ink, whatever the mode.
#[test]
fn a_spot_paint_keeps_the_process_inks() {
    assert_spot_overprints("/S cs 1 scn 60 10 100 80 re f");
}

/// An Indexed colour over a spot base paints the spot ink, so it keeps the process inks too.
#[test]
fn an_indexed_colour_over_a_spot_keeps_the_process_inks() {
    assert_spot_overprints("/I cs 1 sc 60 10 100 80 re f");
}

/// A shading in a spot space deposits its own ink per sample and keeps the rest.
#[test]
fn a_spot_shading_keeps_the_process_inks() {
    assert_spot_overprints("60 10 100 80 re W n /Sh sh");
}
