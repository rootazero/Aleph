//! macOS Vision framework OCR implementation.

use crate::error::{DesktopError, Result};
use crate::OcrResult;

/// Perform OCR using the macOS Vision framework.
#[cfg(target_os = "macos")]
pub(super) fn macos_ocr(png_bytes: &[u8]) -> Result<OcrResult> {
    use crate::{BoundingBox, OcrLine};
    use objc2::AnyThread;
    use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
    use objc2_vision::{
        VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
    };

    // 1. Create NSData from PNG bytes
    let ns_data = NSData::with_bytes(png_bytes);

    // Decode image dimensions from PNG header for bounding box conversion
    let (img_width, img_height) = png_dimensions(png_bytes).unwrap_or((1.0, 1.0));

    // 2. Create and configure text recognition request
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setUsesLanguageCorrection(true);

    let languages =
        NSArray::from_retained_slice(&[NSString::from_str("zh-Hans"), NSString::from_str("en-US")]);
    request.setRecognitionLanguages(&languages);

    // 3. Create image handler from data and perform request
    let empty_opts: objc2::rc::Retained<
        NSDictionary<objc2_vision::VNImageOption, objc2::runtime::AnyObject>,
    > = NSDictionary::new();
    let handler = VNImageRequestHandler::initWithData_options(
        VNImageRequestHandler::alloc(),
        &ns_data,
        &empty_opts,
    );

    // SAFETY: VNRecognizeTextRequest inherits from VNRequest in the Objective-C class
    // hierarchy, so the pointer cast from *mut VNRecognizeTextRequest to *mut VNRequest
    // is valid and preserves the object's retain count and memory layout.
    let requests: objc2::rc::Retained<NSArray<VNRequest>> = unsafe {
        let ptr = objc2::rc::Retained::into_raw(objc2::rc::Retained::clone(&request));
        let vn_req = objc2::rc::Retained::from_raw(ptr as *mut VNRequest).ok_or_else(|| {
            DesktopError::OcrFailed("VNRequest cast produced null pointer".into())
        })?;
        NSArray::from_retained_slice(&[vn_req])
    };

    handler
        .performRequests_error(&requests)
        .map_err(|e| DesktopError::OcrFailed(format!("Vision performRequests failed: {e}")))?;

    // 4. Extract results
    let mut lines = Vec::new();
    let mut full_text = String::new();

    if let Some(observations) = request.results() {
        for obs in observations.iter() {
            let candidates = obs.topCandidates(1);
            if candidates.count() == 0 {
                continue;
            }
            let candidate = candidates.objectAtIndex(0);

            let text = candidate.string().to_string();
            let confidence = candidate.confidence() as f64;

            // Get bounding box (normalized 0-1, origin bottom-left)
            let bbox = unsafe { obs.boundingBox() };

            // Convert from Vision coordinates (bottom-left origin) to
            // screen coordinates (top-left origin)
            let bounding_box = BoundingBox {
                x: bbox.origin.x * img_width,
                y: (1.0 - bbox.origin.y - bbox.size.height) * img_height,
                w: bbox.size.width * img_width,
                h: bbox.size.height * img_height,
            };

            if !full_text.is_empty() {
                full_text.push('\n');
            }
            full_text.push_str(&text);

            lines.push(OcrLine {
                text,
                bounding_box: Some(bounding_box),
                confidence: Some(confidence),
            });
        }
    }

    Ok(OcrResult { full_text, lines })
}

/// Extract width/height from PNG header (IHDR chunk).
#[cfg(target_os = "macos")]
pub(super) fn png_dimensions(png_bytes: &[u8]) -> Option<(f64, f64)> {
    // PNG: 8 bytes signature, then IHDR chunk: 4 len + 4 "IHDR" + 4 width + 4 height
    if png_bytes.len() < 24 {
        return None;
    }
    // Verify PNG signature
    if &png_bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let width = u32::from_be_bytes([png_bytes[16], png_bytes[17], png_bytes[18], png_bytes[19]]);
    let height = u32::from_be_bytes([png_bytes[20], png_bytes[21], png_bytes[22], png_bytes[23]]);
    Some((width as f64, height as f64))
}
