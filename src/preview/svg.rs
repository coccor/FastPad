//! SVG images, rasterized on the image worker thread with Direct2D's SVG renderer into the same
//! premultiplied BGRA pixels the WIC decoders produce. Direct2D implements SVG 1.1 shapes, paths,
//! gradients, `<use>`, transforms, opacity, and clipping, and never fetches external resources; text,
//! filters, masks, and CSS stylesheets do not render.

use crate::preview::dwrite::{create_d2d_factory, hresult_error, load_system_library};
use crate::preview::html::{Token, attr, tokenize};
use crate::preview::images::{DecodedImage, MAX_IMAGE_PIXELS};
use crate::{FastPadError, Result};
use std::path::Path;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_SIZE_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_FACTORY_TYPE_MULTI_THREADED, D2D1_RENDER_TARGET_PROPERTIES, ID2D1DeviceContext5,
    ID2D1Factory,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapCacheOnLoad,
};
use windows::core::Interface;
use windows_numerics::Matrix3x2;

pub const MAX_SVG_BYTES: u64 = 8 * 1024 * 1024;
/// A browser's size for an SVG with neither dimensions nor a viewBox.
const DEFAULT_SIZE: (f32, f32) = (300.0, 150.0);

pub fn decode_svg(wic: &IWICImagingFactory, path: &Path, max_width: u32) -> Result<DecodedImage> {
    if std::fs::metadata(path).map_err(FastPadError::Io)?.len() > MAX_SVG_BYTES {
        return Err(FastPadError::Invariant("SVG file is larger than 8 MB"));
    }
    let bytes = std::fs::read(path).map_err(FastPadError::Io)?;
    let source = String::from_utf8_lossy(&bytes);
    let natural = natural_size(&source);
    let (natural_width, natural_height) = (natural.0.ceil() as u32, natural.1.ceil() as u32);
    if natural_width == 0
        || natural_height == 0
        || u64::from(natural_width) * u64::from(natural_height) > MAX_IMAGE_PIXELS
    {
        return Err(FastPadError::Invariant(
            "image is empty or larger than 64 megapixels",
        ));
    }
    let (width, height) = if max_width > 0 && natural_width > max_width {
        let scaled = (u64::from(natural_height) * u64::from(max_width)) / u64::from(natural_width);
        (max_width, scaled.max(1) as u32)
    } else {
        (natural_width, natural_height)
    };
    let document = rewrite_for_direct2d(&source);
    let module = load_system_library("d2d1.dll")?;
    let pixels = {
        let factory = create_d2d_factory(&module, D2D1_FACTORY_TYPE_MULTI_THREADED)?;
        rasterize(wic, &factory, document.as_bytes(), natural, (width, height))?
    };
    drop(module);
    Ok(DecodedImage {
        width,
        height,
        natural_width,
        natural_height,
        pixels,
    })
}

fn rasterize(
    wic: &IWICImagingFactory,
    factory: &ID2D1Factory,
    document: &[u8],
    natural: (f32, f32),
    (width, height): (u32, u32),
) -> Result<Vec<u8>> {
    unsafe {
        let bitmap = wic
            .CreateBitmap(
                width,
                height,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapCacheOnLoad,
            )
            .map_err(hresult_error)?;
        let properties = D2D1_RENDER_TARGET_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        let target = factory
            .CreateWicBitmapRenderTarget(&bitmap, &properties)
            .map_err(hresult_error)?;
        // SVG support arrived with ID2D1DeviceContext5 in Windows 10 1703; older systems fail here
        // and the image shows its placeholder.
        let context: ID2D1DeviceContext5 = target.cast().map_err(hresult_error)?;
        let stream = wic.CreateStream().map_err(hresult_error)?;
        stream
            .InitializeFromMemory(document)
            .map_err(hresult_error)?;
        let svg = context
            .CreateSvgDocument(
                &stream,
                D2D_SIZE_F {
                    width: natural.0,
                    height: natural.1,
                },
            )
            .map_err(hresult_error)?;
        context.BeginDraw();
        context.Clear(Some(&D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));
        context.SetTransform(&Matrix3x2::scale(
            width as f32 / natural.0,
            height as f32 / natural.1,
        ));
        context.DrawSvgDocument(&svg);
        context.EndDraw(None, None).map_err(hresult_error)?;
        drop(svg);
        drop(context);
        drop(target);
        let mut pixels = vec![0_u8; width as usize * height as usize * 4];
        bitmap
            .CopyPixels(std::ptr::null(), width * 4, &mut pixels)
            .map_err(hresult_error)?;
        Ok(pixels)
    }
}

/// The root `<svg>` element's size: `width`/`height` in plain numbers or `px`, completed from the
/// `viewBox` aspect ratio when only one is given; otherwise the `viewBox` size; otherwise 300×150.
pub fn natural_size(svg: &str) -> (f32, f32) {
    let Some(start) = svg
        .match_indices("<svg")
        .map(|(index, _)| index)
        .find(|index| element_is(&svg[*index..], "svg"))
    else {
        return DEFAULT_SIZE;
    };
    let end = tag_end(&svg[start..]).map_or(svg.len(), |end| start + end);
    let Some(Token::Start { attrs, .. }) = tokenize(&svg[start..end]).into_iter().next() else {
        return DEFAULT_SIZE;
    };
    let width = attr(&attrs, "width").and_then(parse_length);
    let height = attr(&attrs, "height").and_then(parse_length);
    let view_box = attr(&attrs, "viewbox").and_then(parse_view_box);
    match (width, height, view_box) {
        (Some(width), Some(height), _) => (width, height),
        (Some(width), None, Some((box_width, box_height))) => {
            (width, width * box_height / box_width)
        }
        (None, Some(height), Some((box_width, box_height))) => {
            (height * box_width / box_height, height)
        }
        (None, None, Some(size)) => size,
        (Some(width), None, None) => (width, DEFAULT_SIZE.1),
        (None, Some(height), None) => (DEFAULT_SIZE.0, height),
        (None, None, None) => DEFAULT_SIZE,
    }
}

fn parse_length(value: &str) -> Option<f32> {
    let value = value.trim();
    let number = value.strip_suffix("px").unwrap_or(value).trim();
    number
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
}

fn parse_view_box(value: &str) -> Option<(f32, f32)> {
    let numbers = value
        .split(|character: char| character.is_ascii_whitespace() || character == ',')
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<f32>().ok())
        .collect::<Option<Vec<_>>>()?;
    match numbers.as_slice() {
        [_, _, width, height]
            if width.is_finite() && height.is_finite() && *width > 0.0 && *height > 0.0 =>
        {
            Some((*width, *height))
        }
        _ => None,
    }
}

/// Direct2D implements SVG 1.1 linking: `<use>` needs `xlink:href`, and the root must declare the
/// `xlink` namespace. SVG 2 files written with plain `href` are rewritten to that form.
pub fn rewrite_for_direct2d(svg: &str) -> String {
    let mut output = String::with_capacity(svg.len() + 64);
    let mut rest = svg;
    let mut root_seen = false;
    while let Some(open) = rest.find('<') {
        output.push_str(&rest[..open]);
        rest = &rest[open..];
        let Some(end) = tag_end(rest) else {
            break;
        };
        let tag = &rest[..end];
        if !root_seen && element_is(tag, "svg") {
            root_seen = true;
            output.push_str("<svg");
            if !tag.contains("xmlns:xlink") {
                output.push_str(r#" xmlns:xlink="http://www.w3.org/1999/xlink""#);
            }
            output.push_str(&tag[4..]);
        } else if element_is(tag, "use") {
            output.push_str(&xlink_hrefs(tag));
        } else {
            output.push_str(tag);
        }
        rest = &rest[end..];
    }
    output.push_str(rest);
    output
}

/// Whether `tag` (starting at `<`) opens element `name`.
fn element_is(tag: &str, name: &str) -> bool {
    tag.strip_prefix('<')
        .and_then(|tag| tag.strip_prefix(name))
        .is_some_and(|rest| {
            rest.starts_with(|character: char| {
                character.is_ascii_whitespace() || character == '>' || character == '/'
            })
        })
}

/// The byte length of the tag at the start of `tag`, through its `>`, skipping quoted values.
fn tag_end(tag: &str) -> Option<usize> {
    let mut quote = None;
    for (index, byte) in tag.bytes().enumerate().skip(1) {
        match quote {
            Some(open) if byte == open => quote = None,
            Some(_) => {}
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            None if byte == b'>' => return Some(index + 1),
            None => {}
        }
    }
    None
}

fn xlink_hrefs(tag: &str) -> String {
    let bytes = tag.as_bytes();
    let mut output = String::with_capacity(tag.len() + 6);
    let mut copied = 0;
    let mut search = 1;
    while let Some(found) = tag[search..].find("href=") {
        let at = search + found;
        if bytes[at - 1].is_ascii_whitespace() {
            output.push_str(&tag[copied..at]);
            output.push_str("xlink:");
            copied = at;
        }
        search = at + 5;
    }
    output.push_str(&tag[copied..]);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::images::decode_image;
    use std::path::PathBuf;

    fn icon() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("fastpad-icon.svg")
    }

    fn scratch(name: &str, contents: &[u8]) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("fastpad-svg-{name}-{}.svg", std::process::id()));
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn pixel(image: &DecodedImage, x: u32, y: u32) -> [u8; 4] {
        let offset = ((y * image.width + x) * 4) as usize;
        image.pixels[offset..offset + 4].try_into().unwrap()
    }

    /// BGRA within a small tolerance for antialiasing.
    fn near(actual: [u8; 4], expected: [u8; 4]) -> bool {
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.abs_diff(expected) <= 8)
    }

    const HEADER_ORANGE: [u8; 4] = [0x20, 0xB0, 0xFF, 0xFF];

    /// The spec §7.3 spike: Direct2D must rasterize SVG into a WIC bitmap off the UI thread.
    #[test]
    fn the_fastpad_icon_rasterizes_on_a_worker_thread() {
        let image = std::thread::spawn(|| decode_image(&icon(), 96))
            .join()
            .unwrap()
            .unwrap();
        assert_eq!((image.natural_width, image.natural_height), (256, 256));
        assert_eq!((image.width, image.height), (96, 96));
        // The header band (#FFB020) at (150, 72) of 256 px, below the binder rings.
        let header = pixel(&image, 56, 27);
        assert!(near(header, HEADER_ORANGE), "{header:?}");
        // Outside the notepad the icon is transparent.
        assert_eq!(pixel(&image, 2, 2), [0, 0, 0, 0]);
    }

    #[test]
    fn use_elements_render_after_the_xlink_rewrite() {
        let image = decode_image(&icon(), 0).unwrap();
        // A binder ring drawn through `<use href="#rings">` covers the header at (95, 50).
        let ring = pixel(&image, 95, 50);
        assert!(!near(ring, HEADER_ORANGE), "{ring:?}");
    }

    #[test]
    fn malformed_huge_and_oversized_svgs_fail() {
        let malformed = scratch("malformed", b"<svg width='10' height='10'><path d=");
        assert!(decode_image(&malformed, 0).is_err());
        let huge = scratch(
            "huge",
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="100000" height="100000"></svg>"#,
        );
        assert!(decode_image(&huge, 0).is_err());
        let mut padded = b"<svg xmlns='http://www.w3.org/2000/svg' width='1' height='1'>".to_vec();
        padded.resize(MAX_SVG_BYTES as usize + 1, b' ');
        let oversized = scratch("oversized", &padded);
        assert!(decode_image(&oversized, 0).is_err());
        for path in [malformed, huge, oversized] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn natural_size_follows_dimensions_then_view_box_then_the_browser_default() {
        assert_eq!(
            natural_size(r#"<svg width="64" height="32px">"#),
            (64.0, 32.0)
        );
        assert_eq!(
            natural_size(r#"<?xml version="1.0"?><!-- c --><svg viewBox="0 0 256 128">"#),
            (256.0, 128.0)
        );
        assert_eq!(
            natural_size(r#"<svg width="100%" viewBox="0,0,10,20">"#),
            (10.0, 20.0)
        );
        assert_eq!(
            natural_size(r#"<svg width="50" viewBox="0 0 10 20">"#),
            (50.0, 100.0)
        );
        assert_eq!(natural_size("<svg>"), (300.0, 150.0));
        assert_eq!(
            natural_size("<svgx width='9'><svg height='40'>"),
            (300.0, 40.0)
        );
        assert_eq!(natural_size("not svg"), (300.0, 150.0));
    }

    #[test]
    fn use_elements_get_xlink_hrefs_and_the_root_declares_the_namespace() {
        assert_eq!(
            rewrite_for_direct2d(
                r##"<svg viewBox="0 0 1 1"><use href="#a"/><use xlink:href="#b"/><a href="x"/></svg>"##
            ),
            r##"<svg xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 1 1"><use xlink:href="#a"/><use xlink:href="#b"/><a href="x"/></svg>"##
        );
        let declared =
            r##"<svg xmlns:xlink="http://www.w3.org/1999/xlink"><use href="#a"/></svg>"##;
        assert_eq!(
            rewrite_for_direct2d(declared),
            declared.replace(" href=", " xlink:href=")
        );
    }

    #[test]
    #[ignore = "performance measurement: cargo test --release --lib preview::svg -- --ignored --nocapture"]
    fn decoding_the_icon_at_256_px_is_recorded() {
        let mut samples = (0..21)
            .map(|_| {
                let started = std::time::Instant::now();
                decode_image(&icon(), 256).unwrap();
                started.elapsed().as_micros() as u64
            })
            .collect::<Vec<_>>();
        samples.sort_unstable();
        println!(
            "svg decode at 256 px: median {} us, max {} us",
            samples[10], samples[20]
        );
    }
}
