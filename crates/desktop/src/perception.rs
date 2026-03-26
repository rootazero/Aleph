//! Perception capabilities — screen capture, OCR, accessibility tree.
//!
//! This module provides platform-specific implementations for:
//! - Screenshot capture via `xcap`
//! - OCR via platform APIs (WinRT on Windows; not available on macOS/Linux)
//! - Raw PNG capture for use as OCR input
//!
//! All functions are synchronous and should be called via
//! `tokio::task::spawn_blocking` from async contexts.

use base64::{engine::general_purpose, Engine as _};
use std::io::Cursor;
use tracing::debug;

use crate::error::{DesktopError, Result};
use crate::{OcrResult, ScreenRegion, Screenshot};

/// Capture a screenshot of the primary monitor, optionally cropped to a region.
///
/// Uses `xcap::Monitor` to enumerate displays and capture the primary one.
/// The image is encoded as PNG and returned as a base64-encoded [`Screenshot`].
///
/// # Errors
///
/// - [`DesktopError::ScreenCapture`] if no monitors are found, no primary
///   monitor exists, or the capture/encoding fails.
pub fn take_screenshot(region: Option<&ScreenRegion>) -> Result<Screenshot> {
    debug!("Taking screenshot, region: {:?}", region);

    let monitors = xcap::Monitor::all()
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to enumerate monitors: {e}")))?;

    let monitor = monitors
        .into_iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .ok_or_else(|| DesktopError::ScreenCapture("No primary monitor found".into()))?;

    let image = match region {
        Some(r) => monitor.capture_region(r.x, r.y, r.width, r.height),
        None => monitor.capture_image(),
    }
    .map_err(|e| DesktopError::ScreenCapture(format!("Screen capture failed: {e}")))?;

    let (width, height) = (image.width(), image.height());

    let mut buf = Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| DesktopError::ScreenCapture(format!("PNG encoding failed: {e}")))?;

    let image_base64 = general_purpose::STANDARD.encode(buf.into_inner());

    debug!("Screenshot captured: {}x{}", width, height);

    Ok(Screenshot {
        image_base64,
        width,
        height,
        format: "png".to_string(),
    })
}

/// Capture the primary monitor as raw PNG bytes.
///
/// This is a convenience function for OCR input — it captures the full
/// primary monitor and returns the PNG-encoded bytes without base64 encoding.
///
/// # Errors
///
/// Same as [`take_screenshot`].
pub fn capture_screen_png() -> Result<Vec<u8>> {
    debug!("Capturing screen as raw PNG bytes");

    let monitors = xcap::Monitor::all()
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to enumerate monitors: {e}")))?;

    let monitor = monitors
        .into_iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .ok_or_else(|| DesktopError::ScreenCapture("No primary monitor found".into()))?;

    let image = monitor
        .capture_image()
        .map_err(|e| DesktopError::ScreenCapture(format!("Screen capture failed: {e}")))?;

    let mut buf = Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| DesktopError::ScreenCapture(format!("PNG encoding failed: {e}")))?;

    Ok(buf.into_inner())
}

/// Perform OCR on raw PNG image bytes.
///
/// # Platform support
///
/// - **Windows**: Uses WinRT `OcrEngine` API (prefers zh-Hans, fallback to en-US).
/// - **macOS/Linux**: Returns [`DesktopError::NotImplemented`] — macOS OCR is
///   handled by the native Swift app.
///
/// # Errors
///
/// - [`DesktopError::NotImplemented`] on non-Windows platforms.
/// - [`DesktopError::OcrFailed`] if the Windows OCR engine fails.
pub fn perform_ocr(png_bytes: &[u8]) -> Result<OcrResult> {
    #[cfg(target_os = "windows")]
    {
        windows_ocr(png_bytes)
    }

    #[cfg(target_os = "macos")]
    {
        macos_ocr(png_bytes)
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = png_bytes;
        Err(DesktopError::NotImplemented(
            "OCR not implemented on this platform".into(),
        ))
    }
}

// ── macOS Vision OCR ────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn macos_ocr(png_bytes: &[u8]) -> Result<OcrResult> {
    use crate::{BoundingBox, OcrLine};
    use objc2::AnyThread;
    use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
    use objc2_vision::{
        VNImageRequestHandler, VNRecognizeTextRequest, VNRequest,
        VNRequestTextRecognitionLevel,
    };

    // 1. Create NSData from PNG bytes
    let ns_data = NSData::with_bytes(png_bytes);

    // Decode image dimensions from PNG header for bounding box conversion
    let (img_width, img_height) = png_dimensions(png_bytes).unwrap_or((1.0, 1.0));

    // 2. Create and configure text recognition request
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setUsesLanguageCorrection(true);

    let languages = NSArray::from_retained_slice(&[
        NSString::from_str("zh-Hans"),
        NSString::from_str("en-US"),
    ]);
    request.setRecognitionLanguages(&languages);

    // 3. Create image handler from data and perform request
    let empty_opts: objc2::rc::Retained<NSDictionary<objc2_vision::VNImageOption, objc2::runtime::AnyObject>> =
        NSDictionary::new();
    let handler = VNImageRequestHandler::initWithData_options(
        VNImageRequestHandler::alloc(),
        &ns_data,
        &empty_opts,
    );

    // VNRecognizeTextRequest inherits from VNRequest — use ProtocolObject or direct cast
    let requests: objc2::rc::Retained<NSArray<VNRequest>> = unsafe {
        let ptr = objc2::rc::Retained::into_raw(objc2::rc::Retained::clone(&request));
        let vn_req = objc2::rc::Retained::from_raw(ptr as *mut VNRequest)
            .ok_or_else(|| DesktopError::OcrFailed("VNRequest cast produced null pointer".into()))?;
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
fn png_dimensions(png_bytes: &[u8]) -> Option<(f64, f64)> {
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

// ── macOS Screen Recording ─────────────────────────────────────

// SCRecordingOutput delegate — defined at module scope to avoid ObjC
// class re-registration panic on repeated calls.
#[cfg(target_os = "macos")]
mod sc_recording_delegate {
    use objc2::DefinedClass;
    use objc2_foundation::{NSError, NSObject, NSObjectProtocol};
    use objc2_screen_capture_kit::{SCRecordingOutput, SCRecordingOutputDelegate};
    use std::sync::{Arc, Condvar, Mutex};

    pub struct SCRecordingDelegateIvars {
        pub finished: Arc<(Mutex<bool>, Condvar)>,
        pub error: Arc<Mutex<Option<String>>>,
    }

    objc2::define_class!(
        #[unsafe(super(NSObject))]
        #[ivars = SCRecordingDelegateIvars]
        #[name = "AlephSCRecordingDelegate"]
        pub struct SCRecordingDelegate;

        unsafe impl SCRecordingOutputDelegate for SCRecordingDelegate {
            #[unsafe(method(recordingOutputDidStartRecording:))]
            fn _did_start(&self, _recording_output: &SCRecordingOutput) {
                tracing::debug!("SCRecordingOutput: recording started");
            }

            #[unsafe(method(recordingOutput:didFailWithError:))]
            fn _did_fail(&self, _recording_output: &SCRecordingOutput, error: &NSError) {
                let msg = error.to_string();
                tracing::error!("SCRecordingOutput: recording failed: {}", msg);
                let ivars = self.ivars();
                {
                    let mut guard = ivars.error.lock().unwrap_or_else(|e| e.into_inner());
                    *guard = Some(msg);
                }
                let (ref lock, ref cvar) = *ivars.finished;
                let mut finished = lock.lock().unwrap_or_else(|e| e.into_inner());
                *finished = true;
                cvar.notify_all();
            }

            #[unsafe(method(recordingOutputDidFinishRecording:))]
            fn _did_finish(&self, _recording_output: &SCRecordingOutput) {
                tracing::debug!("SCRecordingOutput: recording finished");
                let ivars = self.ivars();
                let (ref lock, ref cvar) = *ivars.finished;
                let mut finished = lock.lock().unwrap_or_else(|e| e.into_inner());
                *finished = true;
                cvar.notify_all();
            }
        }

        unsafe impl NSObjectProtocol for SCRecordingDelegate {}
    );
}

#[cfg(target_os = "macos")]
use sc_recording_delegate::{SCRecordingDelegate, SCRecordingDelegateIvars};

/// Record the primary display to an MP4 file.
///
/// Uses a two-tier approach:
/// - **macOS 15+**: Native `SCRecordingOutput` API for high-quality recording.
/// - **macOS 13–14**: Fallback to the built-in `screencapture -V` CLI.
///
/// The output file is written to `~/.aleph/data/_media/screen_record_{timestamp}.mp4`.
///
/// # Errors
///
/// - [`DesktopError::ScreenCapture`] on any recording failure.
#[cfg(target_os = "macos")]
pub fn screen_record(config: &crate::screen_types::ScreenRecordConfig) -> Result<crate::screen_types::ScreenRecordResult> {
    let config = config.clone().clamped();
    let output_path = screen_record_output_path()?;

    if can_use_sc_recording_output() {
        debug!("Using SCRecordingOutput (macOS 15+) for screen recording");
        sc_recording_output_record(&config, &output_path)
    } else {
        debug!("Using screencapture CLI fallback (macOS 13-14)");
        screencapture_cli_record(&config, &output_path)
    }
}

/// Generate the output file path: `~/.aleph/data/_media/screen_record_{timestamp}.mp4`
#[cfg(target_os = "macos")]
fn screen_record_output_path() -> Result<std::path::PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| DesktopError::ScreenCapture("Cannot determine home directory".into()))?;
    let media_dir = home.join(".aleph/data/_media");
    std::fs::create_dir_all(&media_dir)
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to create _media dir: {e}")))?;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    Ok(media_dir.join(format!("screen_record_{ts}.mp4")))
}

/// Check if we can use SCRecordingOutput (macOS 15.0+).
#[cfg(target_os = "macos")]
fn can_use_sc_recording_output() -> bool {
    let info = objc2_foundation::NSProcessInfo::processInfo();
    let version = info.operatingSystemVersion();
    version.majorVersion >= 15
}

/// Record using SCRecordingOutput (macOS 15+).
#[cfg(target_os = "macos")]
fn sc_recording_output_record(
    config: &crate::screen_types::ScreenRecordConfig,
    output_path: &std::path::Path,
) -> Result<crate::screen_types::ScreenRecordResult> {
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::AnyThread;
    use objc2_core_media::{CMTime, CMTimeFlags};
    use objc2_foundation::{NSArray, NSError, NSURL};
    use objc2_screen_capture_kit::{
        SCContentFilter, SCRecordingOutput, SCRecordingOutputConfiguration,
        SCRecordingOutputDelegate, SCShareableContent, SCStream, SCStreamConfiguration,
    };
    use std::sync::mpsc;
    use std::time::Duration;

    // 1. Get shareable content
    let (content_tx, content_rx) =
        mpsc::channel::<std::result::Result<Retained<SCShareableContent>, String>>();
    let content_block = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            if content.is_null() {
                let msg = if error.is_null() {
                    "Unknown error getting shareable content".to_string()
                } else {
                    unsafe { &*error }.to_string()
                };
                let _ = content_tx.send(Err(msg));
            } else {
                match unsafe { Retained::retain(content) } {
                    Some(r) => { let _ = content_tx.send(Ok(r)); }
                    None => { let _ = content_tx.send(Err("SCShareableContent retain returned None".into())); }
                }
            }
        },
    );
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, true, &content_block,
        );
    }
    let content = content_rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|e| {
            DesktopError::ScreenCapture(format!("Timeout getting shareable content: {e}"))
        })?
        .map_err(|e| {
            DesktopError::ScreenCapture(format!("Failed to get shareable content: {e}"))
        })?;

    // 2. Pick primary display (first in the list)
    let displays = unsafe { content.displays() };
    if displays.count() == 0 {
        return Err(DesktopError::ScreenCapture("No displays found".into()));
    }
    let display = displays.objectAtIndex(0);

    let display_width = unsafe { display.width() } as usize;
    let display_height = unsafe { display.height() } as usize;

    // 3. Create content filter (capture entire display, no excluded windows)
    let empty_windows: Retained<NSArray<objc2_screen_capture_kit::SCWindow>> = NSArray::new();
    let filter = unsafe {
        SCContentFilter::initWithDisplay_excludingWindows(
            SCContentFilter::alloc(),
            &display,
            &empty_windows,
        )
    };

    // 4. Create stream configuration
    let stream_config = unsafe { SCStreamConfiguration::new() };

    // Use 2x scale for retina displays
    let scale: usize = 2;
    unsafe {
        stream_config.setWidth(display_width * scale);
        stream_config.setHeight(display_height * scale);
        stream_config.setMinimumFrameInterval(CMTime {
            value: 1,
            timescale: config.fps as i32,
            flags: CMTimeFlags::Valid,
            epoch: 0,
        });
        stream_config.setShowsCursor(true);
        stream_config.setCapturesAudio(config.with_audio);
    }

    // 5. Create recording output configuration
    let recording_config = unsafe { SCRecordingOutputConfiguration::new() };
    let file_url = {
        let path_str = output_path.to_string_lossy();
        let ns_str = objc2_foundation::NSString::from_str(&path_str);
        NSURL::fileURLWithPath(&ns_str)
    };
    unsafe {
        recording_config.setOutputURL(&file_url);
    }

    // 6. Construct delegate for recording lifecycle events
    use std::sync::{Arc, Condvar, Mutex};

    let finished_signal = Arc::new((Mutex::new(false), Condvar::new()));
    let error_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    // Construct delegate (class defined at module scope to avoid ObjC re-registration panic)
    let delegate_ivars = SCRecordingDelegateIvars {
        finished: finished_signal.clone(),
        error: error_slot.clone(),
    };
    let delegate: Retained<SCRecordingDelegate> = {
        let alloc = SCRecordingDelegate::alloc().set_ivars(delegate_ivars);
        unsafe { objc2::msg_send![super(alloc), init] }
    };

    // 7. Create SCRecordingOutput
    let delegate_proto: &ProtocolObject<dyn SCRecordingOutputDelegate> =
        ProtocolObject::from_ref(&*delegate);
    let recording_output = unsafe {
        SCRecordingOutput::initWithConfiguration_delegate(
            SCRecordingOutput::alloc(),
            &recording_config,
            delegate_proto,
        )
    };

    // 8. Create SCStream and add recording output
    let stream = unsafe {
        SCStream::initWithFilter_configuration_delegate(
            SCStream::alloc(),
            &filter,
            &stream_config,
            None, // no stream delegate needed for recording
        )
    };

    unsafe {
        stream
            .addRecordingOutput_error(&recording_output)
            .map_err(|e| {
                DesktopError::ScreenCapture(format!("Failed to add recording output: {e}"))
            })?;
    }

    // 9. Start capture
    let (start_tx, start_rx) = mpsc::channel::<std::result::Result<(), String>>();
    let start_block = RcBlock::new(move |error: *mut NSError| {
        if error.is_null() {
            let _ = start_tx.send(Ok(()));
        } else {
            let _ = start_tx.send(Err(unsafe { &*error }.to_string()));
        }
    });
    unsafe {
        stream.startCaptureWithCompletionHandler(Some(&start_block));
    }
    start_rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|e| DesktopError::ScreenCapture(format!("Timeout starting capture: {e}")))?
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to start capture: {e}")))?;

    // 10. Sleep for the recording duration
    std::thread::sleep(Duration::from_secs_f64(config.duration_secs));

    // 11. Stop capture
    let (stop_tx, stop_rx) = mpsc::channel::<std::result::Result<(), String>>();
    let stop_block = RcBlock::new(move |error: *mut NSError| {
        if error.is_null() {
            let _ = stop_tx.send(Ok(()));
        } else {
            let _ = stop_tx.send(Err(unsafe { &*error }.to_string()));
        }
    });
    unsafe {
        stream.stopCaptureWithCompletionHandler(Some(&stop_block));
    }
    stop_rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|e| DesktopError::ScreenCapture(format!("Timeout stopping capture: {e}")))?
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to stop capture: {e}")))?;

    // 12. Wait for delegate's didFinishRecording callback
    let (lock, cvar) = &*finished_signal;
    let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let _result = cvar
        .wait_timeout_while(guard, Duration::from_secs(15), |finished| !*finished)
        .unwrap_or_else(|e| e.into_inner());

    // Check for recording errors
    if let Some(err_msg) = error_slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
        return Err(DesktopError::ScreenCapture(format!("Recording failed: {err_msg}")));
    }

    debug!("Screen recording complete: {}", output_path.display());

    Ok(crate::screen_types::ScreenRecordResult {
        file_path: output_path.to_string_lossy().into_owned(),
        duration_secs: config.duration_secs,
        has_audio: config.with_audio,
    })
}

/// Fallback: record using `screencapture -V` CLI (macOS 13–14).
#[cfg(target_os = "macos")]
fn screencapture_cli_record(
    config: &crate::screen_types::ScreenRecordConfig,
    output_path: &std::path::Path,
) -> Result<crate::screen_types::ScreenRecordResult> {
    use std::process::Command;

    let duration = config.duration_secs.ceil() as u64;
    let output_str = output_path.to_string_lossy();

    let status = Command::new("screencapture")
        .args(["-V", &duration.to_string(), &*output_str])
        .status()
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to run screencapture: {e}")))?;

    if !status.success() {
        return Err(DesktopError::ScreenCapture(format!(
            "screencapture exited with status: {status}"
        )));
    }

    if !output_path.exists() {
        return Err(DesktopError::ScreenCapture(
            "screencapture completed but output file not found".into(),
        ));
    }

    debug!("Screen recording (CLI) complete: {}", output_path.display());

    Ok(crate::screen_types::ScreenRecordResult {
        file_path: output_path.to_string_lossy().into_owned(),
        duration_secs: config.duration_secs,
        has_audio: config.with_audio,
    })
}

// ── Windows WinRT OCR ───────────────────────────────────────────

/// Perform OCR using the Windows WinRT `OcrEngine` API.
///
/// Steps:
/// 1. Decode PNG bytes into a `SoftwareBitmap` via `BitmapDecoder`.
/// 2. Create an `OcrEngine` (prefer zh-Hans, fallback to en, then user default).
/// 3. Call `RecognizeAsync` to extract text and line bounding boxes.
#[cfg(target_os = "windows")]
fn windows_ocr(png_bytes: &[u8]) -> Result<OcrResult> {
    use crate::{BoundingBox, OcrLine};
    use windows::core::Interface;
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::BitmapDecoder;
    use windows::Media::Ocr as WinOcr;
    use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream, IRandomAccessStream};

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
    let decoder = BitmapDecoder::CreateAsync(
        &stream
            .cast::<IRandomAccessStream>()
            .map_err(|e| {
                DesktopError::OcrFailed(format!(
                    "Stream cast to IRandomAccessStream failed: {e}"
                ))
            })?,
    )
    .map_err(|e| DesktopError::OcrFailed(format!("BitmapDecoder::CreateAsync failed: {e}")))?
    .get()
    .map_err(|e| DesktopError::OcrFailed(format!("BitmapDecoder async get failed: {e}")))?;

    let bitmap = decoder
        .GetSoftwareBitmapAsync()
        .map_err(|e| DesktopError::OcrFailed(format!("GetSoftwareBitmapAsync failed: {e}")))?
        .get()
        .map_err(|e| DesktopError::OcrFailed(format!("SoftwareBitmap async get failed: {e}")))?;

    // 3. Create OcrEngine — prefer zh-Hans, fallback to en-US, then user default.
    let engine = {
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
            .ok_or_else(|| {
                DesktopError::OcrFailed("No OCR language available on this system".into())
            })?
    };

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

    // 5. Build lines array with bounding boxes.
    let ocr_lines: windows::Foundation::Collections::IVectorView<WinOcr::OcrLine> = result
        .Lines()
        .map_err(|e| DesktopError::OcrFailed(format!("Failed to get OCR lines: {e}")))?;

    let mut lines: Vec<OcrLine> = Vec::new();
    for line in &ocr_lines {
        let line: WinOcr::OcrLine = line;
        let text = line
            .Text()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();

        // Merge bounding boxes of all words in this line.
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
                min_x = min_x.min(rect.X as f64);
                min_y = min_y.min(rect.Y as f64);
                max_x = max_x.max((rect.X + rect.Width) as f64);
                max_y = max_y.max((rect.Y + rect.Height) as f64);
            }
        }

        let bounding_box = if has_bounds {
            Some(BoundingBox {
                x: min_x,
                y: min_y,
                w: max_x - min_x,
                h: max_y - min_y,
            })
        } else {
            None
        };

        lines.push(OcrLine {
            text,
            bounding_box,
            confidence: None,
        });
    }

    Ok(OcrResult { full_text, lines })
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// On non-Windows/macOS platforms, `perform_ocr` should return `NotImplemented`.
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    #[test]
    fn test_ocr_not_implemented_on_non_windows() {
        let dummy_png = b"fake png data";
        let result = perform_ocr(dummy_png);
        assert!(result.is_err());
        match result.unwrap_err() {
            DesktopError::NotImplemented(msg) => {
                assert!(
                    msg.contains("OCR not implemented"),
                    "Expected NotImplemented message about OCR, got: {msg}"
                );
            }
            other => panic!("Expected NotImplemented, got: {other:?}"),
        }
    }

    /// Verify that `take_screenshot` returns a proper error type when
    /// it fails (e.g., no display in CI).
    ///
    /// This test doesn't require a display — it just validates that the
    /// function exists and returns a meaningful `DesktopError` variant
    /// (either `ScreenCapture` on failure, or `Ok` if a display is present).
    #[test]
    fn test_take_screenshot_returns_correct_types() {
        let result = take_screenshot(None);
        match result {
            Ok(screenshot) => {
                // If we have a display, verify the screenshot is well-formed.
                assert!(!screenshot.image_base64.is_empty());
                assert!(screenshot.width > 0);
                assert!(screenshot.height > 0);
                assert_eq!(screenshot.format, "png");
            }
            Err(DesktopError::ScreenCapture(msg)) => {
                // No display available (CI) — that's fine, just verify
                // we got the correct error variant.
                assert!(
                    !msg.is_empty(),
                    "ScreenCapture error should have a message"
                );
            }
            Err(other) => {
                panic!("Expected ScreenCapture error or Ok, got: {other:?}");
            }
        }
    }

    /// Verify that `capture_screen_png` returns raw PNG bytes or a proper error.
    #[test]
    fn test_capture_screen_png_returns_correct_types() {
        let result = capture_screen_png();
        match result {
            Ok(bytes) => {
                // PNG files start with the magic bytes: 0x89 P N G
                assert!(bytes.len() > 8, "PNG should be more than 8 bytes");
                assert_eq!(&bytes[..4], b"\x89PNG", "Should start with PNG magic");
            }
            Err(DesktopError::ScreenCapture(_)) => {
                // No display available (CI) — acceptable.
            }
            Err(other) => {
                panic!("Expected ScreenCapture error or Ok, got: {other:?}");
            }
        }
    }
}
