//! A page whose group blends in `DeviceCMYK` (ISO 32000-2 §11.3.4) blends its contents in
//! CMYK: the page group is always isolated (§11.4.7), so it takes the isolated CMYK group's
//! passes (hayro-interpret patch (68)). Magenta `0 1 0 0 k` under black `0 0 0 1 k` with
//! `/BM /Difference` is green `1 0 1 0 k` in CMYK, and stays magenta when blended in RGB.
//!
//! Each page has a reference square on the left, painted directly in the CMYK the blend should
//! give, and the blend on the right: drawn right, the two match.

use std::sync::Arc;

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RenderedPage, RequestKind};

const GREEN_INK: &str = "1 0 1 0 k";
const BLEND: &str = "0 1 0 0 k 110 10 80 80 re f /D gs 0 0 0 1 k 110 10 80 80 re f";

fn pdf(objects: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

fn stream(dict: &str, content: &str) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{content}\nendstream", content.len())
}

/// Renders a 200 x 100 page with group colour space `cs` (`None`: no `/Group`), the reference
/// square from 10 to 90, then `content`. Resources: `/D` (`/BM /Difference`), and `/F`, a form
/// drawing [`BLEND`] with its own `/D`.
fn render(cs: Option<&str>, content: &str) -> RenderedPage {
    let gs = "/ExtGState << /D << /BM /Difference >> >>";
    let group = cs.map(|cs| format!("/Group << /S /Transparency /CS {cs} >>")).unwrap_or_default();
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] {group} /Contents 4 0 R /Resources << {gs} /XObject << /F 5 0 R >> >> >>"),
        stream("", &format!("{GREEN_INK} 10 10 80 80 re f {content}")),
        stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 200 100] /Resources << {gs} >>"), BLEND),
    ];
    let mut renderer = PageRenderer::new(Arc::new(pdf(&objects)), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.0, tag: 0, ..Default::default() });
    assert!(page.error.is_none(), "{content}: {:?}", page.error);
    page
}

/// The colour at PDF point (x, y).
fn at(page: &RenderedPage, x: u32, y: u32) -> [u8; 3] {
    let offset = usize::try_from(((100 - y) * page.width + x) * 4).unwrap();
    [page.rgba[offset], page.rgba[offset + 1], page.rgba[offset + 2]]
}

fn close(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= 3)
}

#[test]
fn a_cmyk_page_group_blends_its_contents_in_cmyk() {
    let page = render(Some("/DeviceCMYK"), BLEND);
    let (reference, blended) = (at(&page, 50, 50), at(&page, 150, 50));
    assert!(close(blended, reference), "blended {blended:?}, reference green ink {reference:?}");
}

#[test]
fn a_blend_mode_inside_a_form_on_a_cmyk_page_blends_in_cmyk() {
    let page = render(Some("/DeviceCMYK"), "/F Do");
    let (reference, blended) = (at(&page, 50, 50), at(&page, 150, 50));
    assert!(close(blended, reference), "blended {blended:?}, reference green ink {reference:?}");
}

/// Controls: an RGB page group, and no page group, blend in RGB as before, so the blend is not
/// the green ink of the reference square; a CMYK page with content that has no ink tints (an
/// RGB colour) is drawn as before, the same as the RGB page.
#[test]
fn pages_that_dont_blend_in_cmyk_are_drawn_as_before() {
    for cs in [Some("/DeviceRGB"), None] {
        let page = render(cs, BLEND);
        let (reference, blended) = (at(&page, 50, 50), at(&page, 150, 50));
        assert!(!close(blended, reference), "{cs:?}: blended in RGB, yet green ink {blended:?}");
    }

    let with_rgb = format!("{BLEND} 1 0 0 rg 0 0 5 5 re f");
    assert_eq!(render(Some("/DeviceCMYK"), &with_rgb).rgba, render(Some("/DeviceRGB"), &with_rgb).rgba);
}
