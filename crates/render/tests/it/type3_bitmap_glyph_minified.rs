//! A Type 3 bitmap glyph (ISO 32000-2 §9.6.5) drawn smaller than its image mask is area-averaged
//! (hayro patch (77)), so a stroke thinner than the reduction ratio still shows. TeX PK fonts
//! emit every glyph as a 300-600 dpi mask, shrunk 3-6:1 on screen. Drawn larger, the glyph stays
//! a hard-edged stencil.

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RenderedPage};
use std::sync::Arc;

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut out = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\nendstream");
    out
}

/// A 40 x 40 page showing one `d1` glyph at 8 pt at (10, 10). The glyph is a 40 x 40 1-bit
/// image mask painting only rows 19 and 20: a horizontal bar two samples high.
fn pk_glyph_pdf() -> Vec<u8> {
    let mut mask = vec![0xFFu8; 5 * 40];
    for byte in mask.iter_mut().skip(19 * 5).take(2 * 5) {
        *byte = 0;
    }
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(),
        stream("", b"BT /F1 8 Tf 10 10 Td (A) Tj ET"),
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 40 40] /FontMatrix [0.025 0 0 0.025 0 0] /CharProcs << /A 6 0 R >> /Encoding << /Type /Encoding /Differences [65 /A] >> /FirstChar 65 /LastChar 65 /Widths [40] /Resources << /XObject << /Im0 7 0 R >> >> >>".to_vec(),
        stream("", b"40 0 0 0 40 40 d1 40 0 0 40 0 0 cm /Im0 Do"),
        stream("/Type /XObject /Subtype /Image /Width 40 /Height 40 /ImageMask true /BitsPerComponent 1", &mask),
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
    bytes
}

fn render(scale: f32) -> RenderedPage {
    let mut renderer = PageRenderer::new(Arc::new(pk_glyph_pdf()), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, scale, ..Default::default() });
    assert!(page.error.is_none(), "{:?}", page.error);
    page
}

fn red(page: &RenderedPage, x: u32, y: u32) -> u8 {
    page.rgba[usize::try_from((y * page.width + x) * 4).unwrap()]
}

/// The device pixel box from PDF x0..x1, y0..y1 (y up) at `scale`.
fn device(scale: f32, x: (f32, f32), y: (f32, f32)) -> (std::ops::Range<u32>, std::ops::Range<u32>) {
    let d = |v: f32| (v * scale) as u32;
    (d(x.0)..d(x.1), d(40.0 - y.1)..d(40.0 - y.0))
}

/// At scale 1 the glyph is 8 device pixels high (5:1), at 2 it is 16 (2.5:1). Point sampling
/// reads one sample row in five and misses the bar outright. Some device row across the
/// glyph's interior must be inked at every column.
#[test]
fn a_shrunk_bitmap_glyph_keeps_a_stroke_thinner_than_the_ratio() {
    for scale in [1.0f32, 2.0] {
        let page = render(scale);
        let (xs, ys) = device(scale, (11.0, 17.0), (10.0, 18.0));
        let inked = ys.clone().any(|y| xs.clone().all(|x| red(&page, x, y) < 250));
        assert!(inked, "at scale {scale} the two-sample bar was dropped");
    }
}

/// Drawn at 16x (3.2 device pixels per sample), the same glyph is still two colours.
#[test]
fn a_magnified_bitmap_glyph_stays_two_colours() {
    let scale = 16.0;
    let page = render(scale);
    let (xs, ys) = device(scale, (11.0, 17.0), (11.0, 17.0));
    let mut seen = Vec::new();
    for y in ys {
        for x in xs.clone() {
            let i = usize::try_from((y * page.width + x) * 4).unwrap();
            let px = [page.rgba[i], page.rgba[i + 1], page.rgba[i + 2]];
            if !seen.contains(&px) {
                seen.push(px);
            }
        }
    }
    assert_eq!(seen.len(), 2, "magnified stencil colours: {seen:?}");
}
