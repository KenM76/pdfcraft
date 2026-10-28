//! In a `DeviceCMYK` page group drawn as cyan-magenta-yellow and black passes (ISO 32000-2
//! §11.3.4), a `DeviceCMYK`, `Separation` or `DeviceN` image deposits the inks its colours
//! name, as a paint in its space does (hayro-interpret patch (76)), not the inks its screen
//! colour converts back to.
//!
//! The page fills a square with black ink and draws an image of nearly the same black over
//! it, clipped to an X. Drawn right, the X's anti-aliased edge mixes two nearly equal blacks
//! and doesn't show. With the image turned into RGB and back, it came out as a mix of all four
//! inks, and the edge pixels, part black ink and part that mix, printed as a light seam.

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest, RenderedPage};
use std::sync::Arc;

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut out = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\nendstream");
    out
}

/// A 100 x 100 page: a K 0.75 square (10..90) and, clipped to an X inside it, a one-pixel
/// image in `space` that `pixel` makes K 190/255. `/M` names a blend mode, so the page
/// group is drawn in passes.
fn render(space: &str, pixel: &[u8]) -> RenderedPage {
    let content = "0 0 0 0.75 k 10 10 80 80 re f \
        q 20 30 m 30 20 l 50 40 l 70 20 l 80 30 l 60 50 l 80 70 l 70 80 l 50 60 l 30 80 l 20 70 l 40 50 l h W n \
        80 0 0 80 10 10 cm /Im0 Do Q";
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R \
           /Group << /S /Transparency /CS /DeviceCMYK >> \
           /Resources << /XObject << /Im0 5 0 R >> /ExtGState << /M << /BM /Multiply >> >> >> >>"
            .to_vec(),
        stream("", content.as_bytes()),
        stream(&format!("/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace {space}"), pixel),
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
    let page = renderer.render(RenderRequest { page: 0, scale: 4.0, ..Default::default() });
    assert!(page.error.is_none(), "{space}: {:?}", page.error);
    page
}

/// The pixels inside the square (device 48..352) that differ from its middle-left colour by
/// more than 3 levels.
fn seams(page: &RenderedPage) -> Vec<(u32, u32, [u8; 3])> {
    let px = |x: u32, y: u32| {
        let o = usize::try_from((y * page.width + x) * 4).unwrap();
        [page.rgba[o], page.rgba[o + 1], page.rgba[o + 2]]
    };
    let base = px(50, 200);
    (48..352)
        .flat_map(|y| (48..352).map(move |x| (x, y)))
        .map(|(x, y)| (x, y, px(x, y)))
        .filter(|(_, _, c)| c.iter().zip(base).any(|(a, b)| a.abs_diff(b) > 3))
        .collect()
}

#[test]
fn a_device_cmyk_image_over_the_same_ink_leaves_no_seam() {
    let page = render("/DeviceCMYK", &[0, 0, 0, 190]);
    let seams = seams(&page);
    assert!(seams.is_empty(), "{} seam pixels, e.g. {:?}", seams.len(), seams.first());
}

#[test]
fn an_indexed_device_cmyk_image_over_the_same_ink_leaves_no_seam() {
    let page = render("[/Indexed /DeviceCMYK 0 <000000BE>]", &[0]);
    let seams = seams(&page);
    assert!(seams.is_empty(), "{} seam pixels, e.g. {:?}", seams.len(), seams.first());
}

/// A black ink whose alternate is process black.
const BLACK: &str = "/DeviceCMYK << /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 0 0 1] /N 1 >>";

#[test]
fn a_separation_image_over_the_same_ink_leaves_no_seam() {
    let page = render(&format!("[/Separation /Black {BLACK}]"), &[190]);
    let seams = seams(&page);
    assert!(seams.is_empty(), "{} seam pixels, e.g. {:?}", seams.len(), seams.first());
}

#[test]
fn a_device_n_image_over_the_same_ink_leaves_no_seam() {
    let page = render(&format!("[/DeviceN [/Black] {BLACK}]"), &[190]);
    let seams = seams(&page);
    assert!(seams.is_empty(), "{} seam pixels, e.g. {:?}", seams.len(), seams.first());
}
