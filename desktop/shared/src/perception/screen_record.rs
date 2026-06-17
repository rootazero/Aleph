//! Screen recording implementations.
//!
//! - **macOS**: `SCRecordingOutput` (macOS 15+) with a `screencapture -V` CLI
//!   fallback (macOS 13–14).
//! - **Linux**: `ffmpeg -f x11grab` for X11 / `XWayland` sessions, with graceful
//!   `NotImplemented` degradation on pure Wayland.
//! - **Windows**: `ffmpeg -f gdigrab -i desktop` — the GDI screen grabber, the
//!   direct analog of Linux's x11grab, reusing the same `ffmpeg` binary
//!   `media.rs` already shells out to (no new crate dependency, R3).

// Consumed by every implemented recording path (macOS/Linux/Windows); only the
// stub fallback for other OSes leaves them unused.
#[cfg_attr(
    not(any(target_os = "macos", target_os = "linux", target_os = "windows")),
    allow(unused_imports)
)]
use crate::error::{DesktopError, Result};
#[cfg_attr(
    not(any(target_os = "macos", target_os = "linux", target_os = "windows")),
    allow(unused_imports)
)]
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
                    let mut guard = ivars
                        .error
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    *guard = Some(msg);
                }
                let (ref lock, ref cvar) = *ivars.finished;
                let mut finished = lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *finished = true;
                cvar.notify_all();
            }

            #[unsafe(method(recordingOutputDidFinishRecording:))]
            fn _did_finish(&self, _recording_output: &SCRecordingOutput) {
                tracing::debug!("SCRecordingOutput: recording finished");
                let ivars = self.ivars();
                let (ref lock, ref cvar) = *ivars.finished;
                let mut finished = lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
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
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
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

/// Check if we can use `SCRecordingOutput` (macOS 15.0+).
#[cfg(target_os = "macos")]
fn can_use_sc_recording_output() -> bool {
    let info = objc2_foundation::NSProcessInfo::processInfo();
    let version = info.operatingSystemVersion();
    version.majorVersion >= 15
}

/// Record using `SCRecordingOutput` (macOS 15+).
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
    let guard = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _result = cvar
        .wait_timeout_while(guard, Duration::from_secs(15), |finished| !*finished)
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // Check for recording errors
    if let Some(err_msg) = error_slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
    {
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

/// Build the `ffmpeg` argument vector for an x11grab screen recording.
///
/// Pure function (no I/O) so the argument assembly can be unit-tested without a
/// display server. `display` is the X11 `DISPLAY` value (e.g. ":0.0").
#[cfg(any(target_os = "linux", test))]
fn build_x11grab_args(
    display: &str,
    config: &crate::screen_types::ScreenRecordConfig,
    output: &str,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-f".into(),
        "x11grab".into(),
        "-framerate".into(),
        config.fps.to_string(),
    ];

    // Region capture uses an explicit size plus a +X,Y input offset; full-screen
    // omits the size and lets x11grab capture the whole root window.
    let input = match &config.region {
        Some(r) => {
            args.push("-video_size".into());
            args.push(format!("{}x{}", r.width, r.height));
            format!("{display}+{},{}", r.x, r.y)
        }
        None => display.to_string(),
    };
    args.push("-i".into());
    args.push(input);

    // Optional system audio from the default PulseAudio source (mirrors
    // media.rs::record_audio).
    if config.with_audio {
        args.push("-f".into());
        args.push("pulse".into());
        args.push("-i".into());
        args.push("default".into());
    }

    args.push("-t".into());
    args.push(format!("{:.3}", config.duration_secs));
    // H.264 + yuv420p for broad MP4 player compatibility.
    args.push("-c:v".into());
    args.push("libx264".into());
    args.push("-pix_fmt".into());
    args.push("yuv420p".into());
    args.push(output.to_string());
    args
}

/// Record the primary display (or a region) to MP4 via `ffmpeg -f x11grab`.
///
/// Linux desktop capture is fragmented by display server:
/// - **X11 / XWayland** (`DISPLAY` set): handled here via ffmpeg x11grab — the
///   single most broadly-available mechanism, reusing the same `ffmpeg` binary
///   `media.rs` already shells out to (no new crate dependency, R3).
/// - **pure Wayland** (no X server): returns [`DesktopError::NotImplemented`]
///   with a hint, since x11grab cannot read native Wayland surfaces. Mirrors the
///   graceful X11/Wayland degradation in `LinuxSystem::user_idle_seconds`.
#[cfg(target_os = "linux")]
pub fn screen_record(
    config: &crate::screen_types::ScreenRecordConfig,
) -> Result<crate::screen_types::ScreenRecordResult> {
    use std::process::Command;

    let config = config.clone().clamped();

    let display = match std::env::var("DISPLAY") {
        Ok(d) if !d.is_empty() => d,
        _ => {
            return Err(DesktopError::NotImplemented(
                "Screen recording needs an X server (X11 or XWayland; DISPLAY is unset). \
                 On pure Wayland use a compositor-native recorder such as wf-recorder \
                 (wlroots) or the xdg-desktop-portal ScreenCast API."
                    .into(),
            ));
        }
    };

    let output_path = screen_record_output_path()?;
    let output_str = output_path.to_string_lossy().into_owned();
    let args = build_x11grab_args(&display, &config, &output_str);

    let output = Command::new("ffmpeg").args(&args).output().map_err(|e| {
        DesktopError::ScreenCapture(format!("Failed to run ffmpeg (install ffmpeg): {e}"))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DesktopError::ScreenCapture(format!(
            "ffmpeg x11grab recording failed: {}",
            stderr.trim()
        )));
    }
    if !output_path.exists() {
        return Err(DesktopError::ScreenCapture(
            "ffmpeg completed but the output file was not created".into(),
        ));
    }

    debug!(
        "Screen recording (ffmpeg x11grab) complete: {}",
        output_path.display()
    );

    Ok(crate::screen_types::ScreenRecordResult {
        file_path: output_str,
        duration_secs: config.duration_secs,
        has_audio: config.with_audio,
    })
}

/// Build the `ffmpeg` argument vector for a `gdigrab` screen recording (Windows).
///
/// Pure function (no I/O) so the argument assembly can be unit-tested without a
/// desktop session. `audio_device` is an optional `DirectShow` audio input name;
/// when `Some`, a second `-f dshow -i audio=<name>` input is appended.
///
/// `gdigrab` is the GDI screen grabber — the direct Windows analog of x11grab.
/// Capture geometry (`-offset_x` / `-offset_y` / `-video_size`) is supplied as
/// *input* options preceding `-i desktop`; full-screen omits them.
#[cfg(any(target_os = "windows", test))]
fn build_gdigrab_args(
    config: &crate::screen_types::ScreenRecordConfig,
    audio_device: Option<&str>,
    output: &str,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-f".into(),
        "gdigrab".into(),
        "-framerate".into(),
        config.fps.to_string(),
    ];

    // Region capture: gdigrab wants the offset + size as input options *before*
    // `-i desktop`. Full-screen leaves them off and grabs the whole desktop.
    if let Some(r) = &config.region {
        args.push("-offset_x".into());
        args.push(r.x.to_string());
        args.push("-offset_y".into());
        args.push(r.y.to_string());
        args.push("-video_size".into());
        args.push(format!("{}x{}", r.width, r.height));
    }
    args.push("-i".into());
    args.push("desktop".into());

    // Optional audio via DirectShow (mirrors media.rs). DirectShow has no
    // "default" pseudo-source like PulseAudio, so an explicit device name is
    // required; the caller resolves it and passes `None` to skip audio.
    if let Some(dev) = audio_device {
        args.push("-f".into());
        args.push("dshow".into());
        args.push("-i".into());
        args.push(format!("audio={dev}"));
    }

    args.push("-t".into());
    args.push(format!("{:.3}", config.duration_secs));
    // H.264 + yuv420p for broad MP4 player compatibility (matches x11grab path).
    args.push("-c:v".into());
    args.push("libx264".into());
    args.push("-pix_fmt".into());
    args.push("yuv420p".into());
    args.push(output.to_string());
    args
}

/// Record the primary desktop (or a region) to MP4 via `ffmpeg -f gdigrab`.
///
/// Reuses the same `ffmpeg` binary `media.rs` already depends on (R3 — no new
/// native capture crate). System audio is opt-in and requires an explicit
/// `DirectShow` device named via `ALEPH_AUDIO_DEVICE` (the same env the Windows
/// `MediaCapability` honours); when audio is requested without a named device,
/// the capture gracefully degrades to video-only (P7) rather than failing.
#[cfg(target_os = "windows")]
pub fn screen_record(
    config: &crate::screen_types::ScreenRecordConfig,
) -> Result<crate::screen_types::ScreenRecordResult> {
    use std::process::Command;

    let config = config.clone().clamped();

    let audio_device = if config.with_audio {
        match std::env::var("ALEPH_AUDIO_DEVICE") {
            Ok(d) if !d.trim().is_empty() => Some(d),
            _ => {
                debug!(
                    "screen_record: with_audio requested but ALEPH_AUDIO_DEVICE is unset; \
                     recording video only (DirectShow has no default-source pseudo-device)"
                );
                None
            }
        }
    } else {
        None
    };
    let has_audio = audio_device.is_some();

    let output_path = screen_record_output_path()?;
    let output_str = output_path.to_string_lossy().into_owned();
    let args = build_gdigrab_args(&config, audio_device.as_deref(), &output_str);

    let output = Command::new("ffmpeg").args(&args).output().map_err(|e| {
        DesktopError::ScreenCapture(format!("Failed to run ffmpeg (install ffmpeg): {e}"))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DesktopError::ScreenCapture(format!(
            "ffmpeg gdigrab recording failed: {}",
            stderr.trim()
        )));
    }
    if !output_path.exists() {
        return Err(DesktopError::ScreenCapture(
            "ffmpeg completed but the output file was not created".into(),
        ));
    }

    debug!(
        "Screen recording (ffmpeg gdigrab) complete: {}",
        output_path.display()
    );

    Ok(crate::screen_types::ScreenRecordResult {
        file_path: output_str,
        duration_secs: config.duration_secs,
        has_audio,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen_types::ScreenRecordConfig;
    use crate::ScreenRegion;

    #[test]
    fn x11grab_fullscreen_omits_video_size() {
        let cfg = ScreenRecordConfig {
            duration_secs: 5.0,
            fps: 30,
            with_audio: false,
            region: None,
        };
        let args = build_x11grab_args(":0.0", &cfg, "/tmp/out.mp4");
        assert!(args.iter().any(|a| a == "x11grab"));
        assert!(!args.iter().any(|a| a == "-video_size"));
        let i = args.iter().position(|a| a == "-i").unwrap();
        assert_eq!(args[i + 1], ":0.0");
        assert_eq!(args.last().unwrap(), "/tmp/out.mp4");
        assert!(!args.iter().any(|a| a == "pulse"));
    }

    #[test]
    fn x11grab_region_sets_size_and_offset() {
        let cfg = ScreenRecordConfig {
            duration_secs: 3.0,
            fps: 24,
            with_audio: false,
            region: Some(ScreenRegion {
                x: 100,
                y: 50,
                width: 640,
                height: 480,
            }),
        };
        let args = build_x11grab_args(":1", &cfg, "/tmp/r.mp4");
        let vs = args.iter().position(|a| a == "-video_size").unwrap();
        assert_eq!(args[vs + 1], "640x480");
        let i = args.iter().position(|a| a == "-i").unwrap();
        assert_eq!(args[i + 1], ":1+100,50");
        let fr = args.iter().position(|a| a == "-framerate").unwrap();
        assert_eq!(args[fr + 1], "24");
    }

    #[test]
    fn x11grab_audio_adds_pulse_input() {
        let cfg = ScreenRecordConfig {
            duration_secs: 2.0,
            fps: 30,
            with_audio: true,
            region: None,
        };
        let args = build_x11grab_args(":0", &cfg, "/tmp/a.mp4");
        assert!(args.iter().any(|a| a == "pulse"));
        assert!(args.iter().any(|a| a == "default"));
    }

    #[test]
    fn gdigrab_fullscreen_omits_geometry() {
        let cfg = ScreenRecordConfig {
            duration_secs: 5.0,
            fps: 30,
            with_audio: false,
            region: None,
        };
        let args = build_gdigrab_args(&cfg, None, "C:/tmp/out.mp4");
        assert!(args.iter().any(|a| a == "gdigrab"));
        assert!(!args.iter().any(|a| a == "-video_size"));
        assert!(!args.iter().any(|a| a == "-offset_x"));
        let i = args.iter().position(|a| a == "-i").unwrap();
        assert_eq!(args[i + 1], "desktop");
        assert_eq!(args.last().unwrap(), "C:/tmp/out.mp4");
        // No audio input when no device is supplied.
        assert!(!args.iter().any(|a| a == "dshow"));
    }

    #[test]
    fn gdigrab_region_sets_offset_and_size() {
        let cfg = ScreenRecordConfig {
            duration_secs: 3.0,
            fps: 24,
            with_audio: false,
            region: Some(ScreenRegion {
                x: 100,
                y: 50,
                width: 640,
                height: 480,
            }),
        };
        let args = build_gdigrab_args(&cfg, None, "C:/tmp/r.mp4");
        let ox = args.iter().position(|a| a == "-offset_x").unwrap();
        assert_eq!(args[ox + 1], "100");
        let oy = args.iter().position(|a| a == "-offset_y").unwrap();
        assert_eq!(args[oy + 1], "50");
        let vs = args.iter().position(|a| a == "-video_size").unwrap();
        assert_eq!(args[vs + 1], "640x480");
        let fr = args.iter().position(|a| a == "-framerate").unwrap();
        assert_eq!(args[fr + 1], "24");
        // Geometry must precede `-i desktop`.
        let i = args.iter().position(|a| a == "-i").unwrap();
        assert!(vs < i, "video_size must come before -i desktop");
    }

    #[test]
    fn gdigrab_audio_adds_dshow_input() {
        let cfg = ScreenRecordConfig {
            duration_secs: 2.0,
            fps: 30,
            with_audio: true,
            region: None,
        };
        let args = build_gdigrab_args(&cfg, Some("Microphone (Realtek Audio)"), "C:/tmp/a.mp4");
        assert!(args.iter().any(|a| a == "dshow"));
        assert!(args.iter().any(|a| a == "audio=Microphone (Realtek Audio)"));
    }
}
