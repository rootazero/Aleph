//! Windows `WinRT` OCR implementation.

#[allow(unused_imports)]
use crate::error::{DesktopError, Result};
#[allow(unused_imports)]
use crate::OcrResult;

/// Perform OCR using the Windows `WinRT` `OcrEngine` API.
///
/// Steps:
/// 1. Decode PNG bytes into a `SoftwareBitmap` via `BitmapDecoder`.
/// 2. Create an `OcrEngine` (prefer zh-Hans, fallback to en, then user default).
/// 3. Call `RecognizeAsync` to extract text and line bounding boxes.
#[cfg(target_os = "windows")]
pub(super) fn windows_ocr(png_bytes: &[u8]) -> Result<OcrResult> {
    let bitmap = decode_png_to_bitmap(png_bytes)?;
    let engine = create_ocr_engine()?;

    // 4. Recognize text.
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| DesktopError::OcrFailed(format!("RecognizeAsync failed: {e}")))?
        .get()
        .map_err(|e| DesktopError::OcrFailed(format!("OCR result async get failed: {e}")))?;

    let full_text = result
        .Text()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();

    let lines = extract_lines(&result)?;

    Ok(OcrResult { full_text, lines })
}

/// Steps 1–2: write the PNG bytes into an in-memory stream and decode them into
/// a `SoftwareBitmap` the OCR engine can consume.
#[cfg(target_os = "windows")]
fn decode_png_to_bitmap(
    png_bytes: &[u8],
) -> Result<windows::Graphics::Imaging::SoftwareBitmap> {
    use windows::core::Interface;
    use windows::Graphics::Imaging::BitmapDecoder;
    use windows::Storage::Streams::{DataWriter, IRandomAccessStream, InMemoryRandomAccessStream};

    // 1. Write PNG bytes into an IRandomAccessStream via DataWriter.
    let stream = InMemoryRandomAccessStream::new()
        .map_err(|e| DesktopError::OcrFailed(format!("Failed to create memory stream: {e}")))?;

    let writer = DataWriter::CreateDataWriter(
        &stream
            .cast::<windows::Storage::Streams::IOutputStream>()
            .map_err(|e| DesktopError::OcrFailed(format!("Stream cast failed: {e}")))?,
    )
    .map_err(|e| DesktopError::OcrFailed(format!("Failed to create DataWriter: {e}")))?;

    writer
        .WriteBytes(png_bytes)
        .map_err(|e| DesktopError::OcrFailed(format!("WriteBytes failed: {e}")))?;
    writer
        .StoreAsync()
        .map_err(|e| DesktopError::OcrFailed(format!("StoreAsync failed: {e}")))?
        .get()
        .map_err(|e| DesktopError::OcrFailed(format!("StoreAsync.get failed: {e}")))?;
    writer
        .FlushAsync()
        .map_err(|e| DesktopError::OcrFailed(format!("FlushAsync failed: {e}")))?
        .get()
        .map_err(|e| DesktopError::OcrFailed(format!("FlushAsync.get failed: {e}")))?;

    // Seek to beginning before decoding.
    stream
        .Seek(0)
        .map_err(|e| DesktopError::OcrFailed(format!("Seek failed: {e}")))?;

    // 2. Decode the PNG into a SoftwareBitmap.
    let decoder =
        BitmapDecoder::CreateAsync(&stream.cast::<IRandomAccessStream>().map_err(|e| {
            DesktopError::OcrFailed(format!("Stream cast to IRandomAccessStream failed: {e}"))
        })?)
        .map_err(|e| DesktopError::OcrFailed(format!("BitmapDecoder::CreateAsync failed: {e}")))?
        .get()
        .map_err(|e| DesktopError::OcrFailed(format!("BitmapDecoder async get failed: {e}")))?;

    let bitmap = decoder
        .GetSoftwareBitmapAsync()
        .map_err(|e| DesktopError::OcrFailed(format!("GetSoftwareBitmapAsync failed: {e}")))?
        .get()
        .map_err(|e| DesktopError::OcrFailed(format!("SoftwareBitmap async get failed: {e}")))?;

    Ok(bitmap)
}

/// Step 3: create an `OcrEngine`, preferring zh-Hans, then en-US, then the
/// user's default profile languages.
#[cfg(target_os = "windows")]
fn create_ocr_engine() -> Result<windows::Media::Ocr::OcrEngine> {
    use windows::Globalization::Language;
    use windows::Media::Ocr as WinOcr;

    let zh = Language::CreateLanguage(&windows::core::HSTRING::from("zh-Hans")).ok();
    let en = Language::CreateLanguage(&windows::core::HSTRING::from("en-US")).ok();

    let try_create = |lang: &Language| -> Option<WinOcr::OcrEngine> {
        if WinOcr::OcrEngine::IsLanguageSupported(lang).unwrap_or(false) {
            WinOcr::OcrEngine::TryCreateFromLanguage(lang).ok()
        } else {
            None
        }
    };

    zh.as_ref()
        .and_then(try_create)
        .or_else(|| en.as_ref().and_then(try_create))
        .or_else(|| WinOcr::OcrEngine::TryCreateFromUserProfileLanguages().ok())
        .ok_or_else(|| DesktopError::OcrFailed("No OCR language available on this system".into()))
}

/// Step 5: walk the recognized lines, attaching each line's merged word
/// bounding box.
#[cfg(target_os = "windows")]
fn extract_lines(
    result: &windows::Media::Ocr::OcrResult,
) -> Result<Vec<crate::OcrLine>> {
    use crate::OcrLine;
    use windows::Media::Ocr as WinOcr;

    let ocr_lines: windows::Foundation::Collections::IVectorView<WinOcr::OcrLine> = result
        .Lines()
        .map_err(|e| DesktopError::OcrFailed(format!("Failed to get OCR lines: {e}")))?;

    let mut lines: Vec<OcrLine> = Vec::new();
    for line in &ocr_lines {
        let line: WinOcr::OcrLine = line;
        let text = line.Text().map(|s| s.to_string_lossy()).unwrap_or_default();

        let bounding_box = merge_word_bounds(&line)?;

        lines.push(OcrLine {
            text,
            bounding_box,
            confidence: None,
        });
    }

    Ok(lines)
}

/// Merge the bounding boxes of every word in `line` into one enclosing box, or
/// `None` when no word reports bounds.
#[cfg(target_os = "windows")]
fn merge_word_bounds(
    line: &windows::Media::Ocr::OcrLine,
) -> Result<Option<crate::BoundingBox>> {
    use crate::BoundingBox;
    use windows::Media::Ocr as WinOcr;

    let words: windows::Foundation::Collections::IVectorView<WinOcr::OcrWord> = line
        .Words()
        .map_err(|e| DesktopError::OcrFailed(format!("Failed to get words: {e}")))?;

    let mut min_x: f64 = f64::MAX;
    let mut min_y: f64 = f64::MAX;
    let mut max_x: f64 = f64::MIN;
    let mut max_y: f64 = f64::MIN;
    let mut has_bounds = false;

    for word in &words {
        let word: WinOcr::OcrWord = word;
        if let Ok(rect) = word.BoundingRect() {
            has_bounds = true;
            min_x = min_x.min(f64::from(rect.X));
            min_y = min_y.min(f64::from(rect.Y));
            max_x = max_x.max(f64::from(rect.X + rect.Width));
            max_y = max_y.max(f64::from(rect.Y + rect.Height));
        }
    }

    Ok(if has_bounds {
        Some(BoundingBox {
            x: min_x,
            y: min_y,
            w: max_x - min_x,
            h: max_y - min_y,
        })
    } else {
        None
    })
}
