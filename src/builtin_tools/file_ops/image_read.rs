//! Image-aware reading for `file_read`.
//!
//! `file_read` historically degraded *every* binary file to a "not
//! displayable" stub — images included. Yet Aleph already carries the full
//! multimodal tool-result pipeline (`hoist_inline_images` → `ToolImage` →
//! `ContentBlock::Image`), which the desktop screenshot tool uses to put pixels
//! in front of a vision model. This module closes the one missing wire: when
//! `file_read` lands on a decodable raster image, it base64-encodes a
//! model-ready copy and the caller surfaces it as `{ image_base64, format }`,
//! which the existing result pipeline promotes to a viewable image block. This
//! is pi `read.ts`'s image-read capability, with a sharper, token-optimal
//! downscale cap.
//!
//! Self-contained: only the already-vendored `image` (png/jpeg/gif/tiff) and
//! `base64` crates — no new dependency (R3), no platform API (R1), no harness
//! change (R10). The promotion to an `Image` content block is done entirely by
//! infrastructure that already exists for screenshots.

use std::io::Cursor;

use base64::Engine;
use image::{DynamicImage, ImageFormat, ImageReader};

/// Anthropic's recommended maximum image edge. Above ~1568px Claude downscales
/// server-side anyway, so uploading larger pixels only burns request tokens for
/// no added legibility. Capping here keeps a `file_read` of a multi-megapixel
/// image from bloating the request. (pi caps at 2000²; 1568 is the
/// token-optimal edge for current Claude vision.)
const MAX_IMAGE_EDGE: u32 = 1568;

/// Hard ceiling on the *decoded* pixel dimensions, well above `MAX_IMAGE_EDGE`
/// so the legitimate path always succeeds and the cap only fires on a malicious
/// header that claims a 100k × 100k image. Without this, `image::load_from_memory`
/// will allocate the full buffer before any size guard can fire — the
/// decompression-bomb vector a crafted multi-MB PNG/GIF/TIFF can exploit.
const MAX_DECODED_DIMENSION: u32 = 16_384;

/// Hard ceiling on the *decoded* byte budget. 512 MiB stops a 16k×16k RGBA
/// image (256 MiB) AND a 32k×8k RGBA image (256 MiB) from succeeding — both
/// pass the dimension check but neither is a real-world photograph.
const MAX_DECODED_BYTES: u64 = 512 * 1024 * 1024;

/// A model-ready image lifted out of a `file_read` of a raster image file.
pub(super) struct EncodedImage {
    /// Base64 (no data-URI prefix) of the re-encoded image.
    pub base64: String,
    /// Canonical format string the result pipeline maps to a MIME type:
    /// `"jpeg"` or `"png"`.
    pub format: &'static str,
    /// Width of the encoded image in pixels (post-downscale).
    pub width: u32,
    /// Height of the encoded image in pixels (post-downscale).
    pub height: u32,
}

/// Try to turn raw file bytes into a model-viewable image.
///
/// Returns `None` when the bytes are not a raster image the vendored `image`
/// features can decode (png/jpeg/gif/tiff) — the caller then falls back to the
/// generic "binary, not displayable" stub. JPEG sources stay JPEG (cheap for
/// photos); every other source is normalized to lossless PNG so the downstream
/// `format → MIME` map (`jpeg`/`jpg` → image/jpeg, else image/png) is always
/// correct. Formats compiled out (webp/bmp) fail to decode and degrade to the
/// binary stub rather than emit a broken image block.
pub(super) fn encode_for_model(bytes: &[u8]) -> Option<EncodedImage> {
    // Magic-byte sniff first: a cheap reject for the ~all non-image binaries an
    // agent reads, before paying for a full decode.
    let sniffed = image::guess_format(bytes).ok()?;
    // Decode with a strict `Limits` so a crafted header (a 100k × 100k PNG,
    // a multi-GB TIFF) cannot materialize a giant allocation before the
    // caller has a chance to refuse. `ImageReader::decode` returns
    // `ImageError::Limits` on a breach, which we fold into `None` so the
    // outer flow degrades to the binary-not-displayable stub — never a
    // multi-minute decode that wedges the executor.
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODED_DIMENSION);
    limits.max_image_height = Some(MAX_DECODED_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    let decoded = ImageReader::from_memory(bytes)
        .with_guessed_format()
        .ok()?
        .with_limits(limits)
        .decode()
        .ok()?;

    let (w, h) = (decoded.width(), decoded.height());
    let fitted = if w.max(h) > MAX_IMAGE_EDGE {
        decoded.resize(
            MAX_IMAGE_EDGE,
            MAX_IMAGE_EDGE,
            image::imageops::FilterType::Triangle,
        )
    } else {
        decoded
    };

    let (out_fmt, format) = if sniffed == ImageFormat::Jpeg {
        (ImageFormat::Jpeg, "jpeg")
    } else {
        (ImageFormat::Png, "png")
    };

    // JPEG cannot encode an alpha channel; flatten to RGB8 on the JPEG path so a
    // PNG-with-alpha source re-encoded as JPEG never errors. The PNG path keeps
    // the source channels (lossless).
    let encodable = if out_fmt == ImageFormat::Jpeg {
        DynamicImage::ImageRgb8(fitted.to_rgb8())
    } else {
        fitted
    };

    let mut buf = Vec::new();
    encodable
        .write_to(&mut Cursor::new(&mut buf), out_fmt)
        .ok()?;

    Some(EncodedImage {
        base64: base64::engine::general_purpose::STANDARD.encode(&buf),
        format,
        width: encodable.width(),
        height: encodable.height(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, RgbaImage};
    use std::io::Cursor;

    /// Encode a solid-colour test image to the given format's bytes.
    fn make_image(w: u32, h: u32, fmt: ImageFormat) -> Vec<u8> {
        let img =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255])));
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), fmt).unwrap();
        buf
    }

    #[test]
    fn png_is_encoded_as_png() {
        let png = make_image(64, 48, ImageFormat::Png);
        let out = encode_for_model(&png).expect("png decodes");
        assert_eq!(out.format, "png");
        assert_eq!((out.width, out.height), (64, 48));
        assert!(!out.base64.is_empty());
    }

    #[test]
    fn jpeg_stays_jpeg() {
        let jpeg = make_image(40, 40, ImageFormat::Jpeg);
        let out = encode_for_model(&jpeg).expect("jpeg decodes");
        assert_eq!(out.format, "jpeg");
    }

    #[test]
    fn oversized_image_is_downscaled_within_cap() {
        let big = make_image(4000, 2000, ImageFormat::Png);
        let out = encode_for_model(&big).expect("png decodes");
        // The longest edge is clamped to the cap; aspect ratio is preserved.
        assert!(out.width.max(out.height) <= MAX_IMAGE_EDGE);
        assert_eq!(out.width, MAX_IMAGE_EDGE);
        assert_eq!(out.height, MAX_IMAGE_EDGE / 2);
    }

    #[test]
    fn small_image_is_not_upscaled() {
        let small = make_image(100, 100, ImageFormat::Png);
        let out = encode_for_model(&small).expect("png decodes");
        assert_eq!((out.width, out.height), (100, 100));
    }

    #[test]
    fn non_image_bytes_return_none() {
        // A NUL-laden blob with no image magic number is not an image.
        assert!(encode_for_model(b"\x00\x01\x02not an image at all").is_none());
        assert!(encode_for_model(&[0u8; 32]).is_none());
    }

    #[test]
    fn base64_roundtrips_to_a_valid_image() {
        let png = make_image(20, 20, ImageFormat::Png);
        let out = encode_for_model(&png).expect("png decodes");
        let raw = base64::engine::general_purpose::STANDARD
            .decode(out.base64.as_bytes())
            .expect("valid base64");
        // The emitted bytes must themselves decode as an image.
        assert!(image::load_from_memory(&raw).is_ok());
    }
}
