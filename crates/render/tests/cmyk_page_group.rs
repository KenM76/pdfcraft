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
/// square from 10 to 90, then `content`. Resources: `/D` (`/BM /Difference`) when `content`
/// uses it, `/F`, a form drawing [`BLEND`] with its own `/D`, and `/G`, a form drawing `/F`.
fn render(cs: Option<&str>, content: &str) -> RenderedPage {
    render_with(cs, content, false)
}

/// [`render`], with the page's resources on the `/Pages` node (inherited, §7.7.3.4) when
/// `inherited`.
fn render_with(cs: Option<&str>, content: &str, inherited: bool) -> RenderedPage {
    let gs = "/ExtGState << /D << /BM /Difference >> >>";
    // The page names `/D` only when it uses it, so a blend inside a form is found in the form.
    let page_gs = if content.contains("/D gs") { gs } else { "" };
    let resources = format!("/Resources << {page_gs} /XObject << /F 5 0 R /G 6 0 R >> >>");
    let (on_pages, on_page) = if inherited { (resources.as_str(), "") } else { ("", resources.as_str()) };
    let group = cs.map(|cs| format!("/Group << /S /Transparency /CS {cs} >>")).unwrap_or_default();
    let form = "/Type /XObject /Subtype /Form /BBox [0 0 200 100]";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [3 0 R] /Count 1 {on_pages} >>"),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] {group} /Contents 4 0 R {on_page} >>"),
        stream("", &format!("{GREEN_INK} 10 10 80 80 re f {content}")),
        stream(&format!("{form} /Resources << {gs} >>"), BLEND),
        stream(&format!("{form} /Resources << /XObject << /F 5 0 R >> >>"), "/F Do"),
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

/// The blend found through every way a page reaches a graphics state: directly, in a form, in
/// a form inside a form, and through resources inherited from the page tree.
#[test]
fn a_blend_mode_anywhere_on_a_cmyk_page_blends_in_cmyk() {
    for (content, inherited) in [(BLEND, true), ("/F Do", false), ("/G Do", false), ("/G Do", true)] {
        let page = render_with(Some("/DeviceCMYK"), content, inherited);
        let (reference, blended) = (at(&page, 50, 50), at(&page, 150, 50));
        assert!(close(blended, reference), "{content}, inherited {inherited}: blended {blended:?}, reference {reference:?}");
    }
}

/// Controls: an RGB page group, and no page group, blend in RGB as before, so the blend is not
/// the green ink of the reference square; a CMYK page with an image, which the passes don't
/// cover yet, is drawn as before, the same as the RGB page, and so is a knockout page group.
#[test]
fn pages_that_dont_blend_in_cmyk_are_drawn_as_before() {
    for cs in [Some("/DeviceRGB"), None] {
        let page = render(cs, BLEND);
        let (reference, blended) = (at(&page, 50, 50), at(&page, 150, 50));
        assert!(!close(blended, reference), "{cs:?}: blended in RGB, yet green ink {blended:?}");
    }

    // Content the CMYK passes don't cover yet: an image.
    let content = format!("{BLEND} q 5 0 0 5 0 0 cm BI /W 1 /H 1 /CS /DeviceRGB /BPC 8 /F /AHx ID ff0000> EI Q");
    assert_eq!(render(Some("/DeviceCMYK"), &content).rgba, render(Some("/DeviceRGB"), &content).rgba);

    // A knockout page group is not drawn in the CMYK passes.
    let knockout = render(Some("/DeviceCMYK /K true"), BLEND);
    assert!(!close(at(&knockout, 150, 50), at(&knockout, 50, 50)), "knockout page blended in CMYK");
}

/// An RGB colour on a CMYK page is the ink that draws back to it through the CMYK profile
/// (hayro-interpret patch (69)), compared with what Acrobat draws for the same page.
///
/// Painted without a blend, a colour inside the profile's gamut draws as itself, black stays
/// black and pure red is clipped to the profile's red. Under `/BM /Difference` with black ink,
/// a colour's cyan, magenta and yellow are complemented and its black kept, so 50% grey, mostly
/// cyan, magenta and yellow ink, comes out a warm grey: neither the neutral grey of RGB blending
/// nor the near-black of grey made from black ink alone.
#[test]
fn an_rgb_colour_on_a_cmyk_page_is_the_ink_that_draws_as_it() {
    let grey = "q 0.5 0.5 0.5 rg 110 10 80 80 re f /D gs 0 0 0 1 k 110 10 80 80 re f Q";
    // (colour, expected pixel, how far from it): the colour itself inside the gamut; Acrobat's
    // pixel for 10% grey, black and red.
    let cases = [
        ("0.8 0.4 0.2", [204, 102, 51], 3),
        ("0.3 0.7 0.4", [77, 179, 102], 3),
        ("0.9 0.9 0.85", [230, 230, 217], 3),
        ("0.1 0.1 0.1", [27, 27, 27], 5),
        ("0 0 0", [5, 6, 7], 8),
        ("1 0 0", [238, 32, 37], 8),
    ];
    for (rgb, expected, within) in cases {
        let drawn = at(&render(Some("/DeviceCMYK"), &format!("{rgb} rg 10 10 80 80 re f {grey}")), 50, 50);
        assert!(drawn.iter().zip(expected).all(|(a, b)| a.abs_diff(b) <= within), "{rgb} drew as {drawn:?}, expected {expected:?}");
    }
    let blended = at(&render(Some("/DeviceCMYK"), grey), 150, 50);
    let acrobat = [137, 112, 106];
    assert!(blended.iter().zip(acrobat).all(|(a, b)| a.abs_diff(b) <= 10), "grey under Difference with black: {blended:?}, Acrobat {acrobat:?}");
}
