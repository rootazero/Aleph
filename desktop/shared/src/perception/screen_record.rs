//! macOS screen recording implementation (SCRecordingOutput + screencapture CLI fallback).

use crate::error::{DesktopError, Result};
use tracing::debug;

// SCRecordingOutput delegate — defined at module scope to avoid ObjC
// class re-registration panic on repeated calls.
#[cfg(target_os = "macos")]
pub(super) mod sc_recording_delegate {
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
pub fn screen_record(
    config: &crate::screen_types::ScreenRecordConfig,
) -> Result<crate::screen_types::ScreenRecordResult> {
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
                    Some(r) => {
                        let _ = content_tx.send(Ok(r));
                    }
                    None => {
                        let _ =
                            content_tx.send(Err("SCShareableContent retain returned None".into()));
                    }
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
        return Err(DesktopError::ScreenCapture(format!(
            "Recording failed: {err_msg}"
        )));
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
