//! In a `DeviceCMYK` page group drawn ink by ink (ISO 32000-2 §11.7.4.5 overprint, or
//! §11.3.4 blending), a `DeviceGray` image deposits black ink alone (§10.4.2.2), whatever its
//! bit depth or decode array (hayro-interpret patch (78)). An 8-bit gray image already did;
//! a 16-bit, 1-bit or decode-inverted one decoded to RGB and deposited the four inks that RGB
//! converts to, so the same black printed darker beside it.

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RenderedPage};
use std::sync::Arc;

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut out = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\nendstream");
    out
}

/// A 100 x 50 page in a `DeviceCMYK` group whose resources name `state`: on the left a
/// one-pixel 8-bit black `DeviceGray` image, on the right the black of `image` (the rest of an
/// image dictionary, and its data).
fn render(state: &str, image: (&str, &[u8])) -> RenderedPage {
    let content = "q 50 0 0 50 0 0 cm /Im0 Do Q q 50 0 0 50 50 0 cm /Im1 Do Q";
    let gray = "/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray";
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 50] /Contents 4 0 R \
             /Group << /S /Transparency /CS /DeviceCMYK >> \
             /Resources << /XObject << /Im0 5 0 R /Im1 6 0 R >> /ExtGState << /S << {state} >> >> >> >>"
        )
        .into_bytes(),
        stream("", content.as_bytes()),
        stream(&format!("{gray} /BitsPerComponent 8"), &[0]),
        stream(&format!("{gray} {}", image.0), image.1),
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
    assert!(page.error.is_none(), "{state} {}: {:?}", image.0, page.error);
    page
}

/// The colour at PDF point (x, 25).
fn at(page: &RenderedPage, x: u32) -> [u8; 3] {
    let offset = usize::try_from((25 * page.width + x) * 4).unwrap();
    [page.rgba[offset], page.rgba[offset + 1], page.rgba[offset + 2]]
}

/// Black in each depth and decode the 8-bit image's fast path doesn't take.
const BLACKS: [(&str, &[u8]); 4] =
    [("/BitsPerComponent 16", &[0, 0]), ("/BitsPerComponent 1", &[0]), ("/BitsPerComponent 4", &[0]), ("/BitsPerComponent 8 /Decode [1 0]", &[255])];

fn assert_blacks_match(state: &str) {
    for image in BLACKS {
        let page = render(state, image);
        let (eight, other) = (at(&page, 25), at(&page, 75));
        assert!(eight.iter().zip(other).all(|(a, b)| a.abs_diff(b) <= 2), "{state} {}: 8-bit {eight:?}, other {other:?}", image.0);
    }
}

/// Overprint anywhere on the page draws the group one ink at a time.
#[test]
fn a_gray_image_of_any_depth_prints_black_ink_alone_in_an_overprinting_group() {
    assert_blacks_match("/OP true /op true");
}

/// A blend mode draws the group as cyan-magenta-yellow and black passes.
#[test]
fn a_gray_image_of_any_depth_prints_black_ink_alone_in_a_blending_group() {
    assert_blacks_match("/BM /Multiply");
}
