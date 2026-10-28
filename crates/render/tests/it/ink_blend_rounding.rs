//! In a `DeviceCMYK` page group drawn ink by ink (ISO 32000-2 §11.3.4 blending), a blended
//! ink lands on the same level as that ink painted flat (vendored hayro patch (81)): the passes
//! blend in floating point and round once.

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RenderedPage};
use std::sync::Arc;

fn stream(data: &str) -> String {
    format!("<< /Length {} >>\nstream\n{data}\nendstream", data.len())
}

/// A 100 x 100 page in a `DeviceCMYK` group: a 10 % black backdrop, `form` (a form drawing
/// its left half) painted under `/B`, and `right` painted normally on the right half.
fn render(blend: &str, form: &str, right: &str) -> RenderedPage {
    let content = format!("0 0 0 0.1 k 10 10 80 80 re f /B gs /Fm0 Do /N gs {right}");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R \
             /Group << /S /Transparency /CS /DeviceCMYK >> /Resources << /ExtGState << \
             /B << /BM /{blend} >> /N << /BM /Normal >> >> /XObject << /Fm0 5 0 R >> >> >>"
        ),
        stream(&content),
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Length {} >>\nstream\n{form}\nendstream", form.len()),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    let mut renderer = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(page.error.is_none(), "{blend}: {:?}", page.error);
    page
}

/// The colour at PDF point (x, 50).
fn at(page: &RenderedPage, x: u32) -> [u8; 3] {
    let offset = usize::try_from((50 * page.width + x) * 4).unwrap();
    [page.rgba[offset], page.rgba[offset + 1], page.rgba[offset + 2]]
}

/// HardLight of black .66 over .1 is 1 − (1 − .1)(1 − (2 × .66 − 1)) = .388 black.
#[test]
fn a_hard_light_black_matches_the_same_black_painted_flat() {
    let page = render("HardLight", "0 0 0 0.66 k 10 10 40 80 re f", "0 0 0 0.388 k 50 10 40 80 re f");
    let (blended, flat) = (at(&page, 30), at(&page, 70));
    assert_eq!(blended, flat, "blended {blended:?}, flat {flat:?}");
}
