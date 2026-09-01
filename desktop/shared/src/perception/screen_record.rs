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

/// SCK crop rect: `x`/`y`/`w`/`h` are display **points** (fed to
/// `setSourceRect`); `out_w`/`out_h` are **pixels** (fed to `setWidth`/`setHeight`).
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, PartialEq)]
struct SckRegionRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    out_w: usize,
    out_h: usize,
}

/// Map a requested `region` to the SCK crop rect.
///
/// `region` is in **physical pixels** (`ScreenRegion`'s documented unit — see
/// `lib.rs`; the Linux/Windows sibling recorders treat it the same). `display_w_pts`
/// ×`display_h_pts` are the display size in **points** (`SCDisplay.width/height`),
/// and `scale` is the pixel-per-point factor. The region is clamped to the display
/// (compared in pixels = points × `scale`), then split into the `sourceRect`
/// (points = pixels ÷ `scale`) and the output pixel dimensions (the clamped region
/// pixels, used as-is). `None` when the region's origin is off-display or the
/// clamped size is zero — the caller maps that to a `ScreenCapture` error. Pure so
/// it is unit-testable without a display (mirrors `build_x11grab_args`).
///
/// A region equal to the whole display reproduces the whole-display path exactly:
/// `sourceRect` = full display in points, output = display points × `scale`.
#[cfg(any(target_os = "macos", test))]
fn sck_region_rect(
    region: &crate::ScreenRegion,
    display_w_pts: u32,
    display_h_pts: u32,
    scale: u32,
) -> Option<SckRegionRect> {
    // Clamp against the display in the region's own unit (pixels = points × scale).
    let display_w_px = display_w_pts * scale;
    let display_h_px = display_h_pts * scale;
    if region.x >= display_w_px || region.y >= display_h_px {
        return None; // origin past the display — no intersection
    }
    let w_px = region.width.min(display_w_px - region.x);
    let h_px = region.height.min(display_h_px - region.y);
    if w_px == 0 || h_px == 0 {
        return None;
    }
    let scale_f = f64::from(scale);
    Some(SckRegionRect {
        // sourceRect is in points: pixels ÷ scale.
        x: f64::from(region.x) / scale_f,
        y: f64::from(region.y) / scale_f,
        w: f64::from(w_px) / scale_f,
        h: f64::from(h_px) / scale_f,
        // Output buffer is in pixels: the clamped region, used as-is.
        out_w: w_px as usize,
        out_h: h_px as usize,
    })
}

/// Clamp `region` to a `display_width`×`display_height` display, in the
/// region's own unit (physical pixels — see [`crate::ScreenRegion`]).
///
/// The x11grab / wf-recorder / gdigrab backends splice the region straight
/// into the recorder's argv, so an absurd region (e.g. `u32::MAX` from a
/// model hallucinating geometry) would have ffmpeg try to allocate a frame
/// buffer that size. `x`/`y` must land inside the display; `width`/`height`
/// shrink to the space remaining past the origin. `None` when nothing
/// intersects — the caller treats that as "no usable region" and records
/// full-screen instead (the same fallback `resolve_region_target` in
/// `screenshot.rs` uses for an off-display region). Pure so it is
/// unit-testable without a display (mirrors `sck_region_rect`).
#[cfg(any(target_os = "linux", target_os = "windows", test))]
fn clamp_region_to_display(
    region: &crate::ScreenRegion,
    display_width: u32,
    display_height: u32,
) -> Option<crate::ScreenRegion> {
    if region.x >= display_width || region.y >= display_height {
        return None; // origin past the display — no intersection
    }
    let width = region.width.min(display_width - region.x);
    let height = region.height.min(display_height - region.y);
    if width == 0 || height == 0 {
        return None;
    }
    Some(crate::ScreenRegion {
        x: region.x,
        y: region.y,
        width,
        height,
    })
}

/// Primary display size in physical pixels, via the same `xcap` enumeration
/// `screenshot.rs` already uses (no new dependency, R3).
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn primary_display_size() -> Option<(u32, u32)> {
    let monitors = xcap::Monitor::all().ok()?;
    let monitor = monitors
        .iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .or(monitors.first())?;
    Some((monitor.width().ok()?, monitor.height().ok()?))
}

/// Clamp `config.region` against the primary display, in place.
///
/// A region that does not intersect the display at all is dropped (`None` →
/// full-screen) rather than turned into an error: recording the whole screen
/// when handed a nonsense rectangle beats recording nothing, and matches the
/// screenshot path's fall-back-to-primary convention. If the display size
/// cannot be determined the region is left untouched — no worse than the
/// pre-clamp behaviour.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn clamp_config_region(config: &mut crate::screen_types::ScreenRecordConfig) {
    let Some(region) = config.region else {
        return;
    };
    let Some((w, h)) = primary_display_size() else {
        debug!("screen_record: display size unknown; region left unclamped");
        return;
    };
    let clamped = clamp_region_to_display(&region, w, h);
    if clamped != Some(region) {
        debug!(
            "screen_record: region {}x{}+{},{} clamped to {clamped:?} against a {w}x{h} display",
            region.width, region.height, region.x, region.y
        );
    }
    config.region = clamped;
}

/// Confirm a recording actually produced a non-empty file. `timed_out` is the
/// delegate-wait timeout flag. A timeout, a missing file, or a zero-byte file
/// is a failure — the `SCRecordingOutput` path previously returned `Ok` in all
/// three cases (false success). Pure over the filesystem so it is unit-testable.
#[cfg(any(target_os = "macos", test))]
fn verify_recording_output(path: &std::path::Path, timed_out: bool) -> Result<()> {
    if timed_out {
        return Err(DesktopError::ScreenCapture(
            "recording did not signal completion within 15s".into(),
        ));
    }
    match std::fs::metadata(path) {
        Ok(m) if m.len() > 0 => Ok(()),
        Ok(_) => Err(DesktopError::ScreenCapture(
            "recording finished but the output file is empty".into(),
        )),
        Err(e) => Err(DesktopError::ScreenCapture(format!(
            "recording finished but the output file is missing: {e}"
        ))),
    }
}

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

        // SAFETY: the class implements every `SCRecordingOutputDelegate` method
        // below with the exact selector and C ABI ScreenCaptureKit invokes, so
        // declaring conformance to the protocol is sound.
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

        // SAFETY: `SCRecordingDelegate` derives from `NSObject` (see the
        // `#[unsafe(super(NSObject))]` above), which supplies every
        // `NSObjectProtocol` method, so declaring conformance is sound.
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

/// Generate the output file path: `~/.aleph/data/_media/screen_record_{timestamp}_{counter}.mp4`
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn screen_record_output_path() -> Result<std::path::PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};

    let home = dirs::home_dir()
        .ok_or_else(|| DesktopError::ScreenCapture("Cannot determine home directory".into()))?;
    let media_dir = home.join(".aleph/data/_media");
    std::fs::create_dir_all(&media_dir)
        .map_err(|e| DesktopError::ScreenCapture(format!("Failed to create _media dir: {e}")))?;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    // Two recordings started in the same millisecond would otherwise collide
    // and overwrite each other.
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
    Ok(media_dir.join(format!("screen_record_{ts}_{counter}.mp4")))
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
                    // SAFETY: `error` is a non-null `NSError` pointer passed by
                    // the framework completion handler.
                    unsafe { &*error }.to_string()
                };
                let _ = content_tx.send(Err(msg));
            } else {
                // SAFETY: `content` is a non-null `SCShareableContent` pointer
                // owned by the framework callback; `Retained::retain` transfers
                // it into a safe retained reference.
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
    // SAFETY: `content_block` outlives this synchronous call; the framework
    // invokes the handler before returning.
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
    // SAFETY: `content` is a retained `SCShareableContent` from the framework;
    // accessing its displays is the documented API pattern.
    let displays = unsafe { content.displays() };
    if displays.count() == 0 {
        return Err(DesktopError::ScreenCapture("No displays found".into()));
    }
    let display = displays.objectAtIndex(0);

    // SAFETY: `display` is a valid object retrieved from `displays` above.
    let display_width = unsafe { display.width() } as usize;
    // SAFETY: `display` is a valid object retrieved from `displays` above.
    let display_height = unsafe { display.height() } as usize;

    // 3. Create content filter (capture entire display, no excluded windows)
    let empty_windows: Retained<NSArray<objc2_screen_capture_kit::SCWindow>> = NSArray::new();
    // SAFETY: `display` is a valid `SCDisplay`; `empty_windows` is a freshly
    // allocated empty array. This is the documented initializer pattern.
    let filter = unsafe {
        SCContentFilter::initWithDisplay_excludingWindows(
            SCContentFilter::alloc(),
            &display,
            &empty_windows,
        )
    };

    // 4. Create stream configuration
    // SAFETY: `SCStreamConfiguration::new()` is the documented allocator for
    // this mutable configuration object.
    let stream_config = unsafe { SCStreamConfiguration::new() };

    // Use 2x scale for retina displays
    let scale: u32 = 2;
    // Honor a requested sub-region: crop via `setSourceRect` (display points =
    // region pixels ÷ scale) and size the output to the region's pixels. No
    // region → whole display, exactly as before.
    let (out_w, out_h) = match config.region.as_ref() {
        None => (
            display_width * scale as usize,
            display_height * scale as usize,
        ),
        Some(region) => {
            let rect = sck_region_rect(region, display_width as u32, display_height as u32, scale)
                .ok_or_else(|| {
                    DesktopError::ScreenCapture(format!(
                        "region {}x{}+{},{} does not intersect the {display_width}x{display_height} display",
                        region.width, region.height, region.x, region.y
                    ))
                })?;
            use objc2_core_foundation::{CGPoint, CGRect, CGSize};
            // SAFETY: `stream_config` is a freshly allocated mutable configuration;
            // `setSourceRect` takes a by-value `CGRect` in display points.
            unsafe {
                stream_config.setSourceRect(CGRect::new(
                    CGPoint::new(rect.x, rect.y),
                    CGSize::new(rect.w, rect.h),
                ));
            }
            (rect.out_w, rect.out_h)
        }
    };

    // SAFETY: `stream_config` is a freshly allocated mutable configuration;
    // these setters are the documented way to populate it.
    unsafe {
        stream_config.setWidth(out_w);
        stream_config.setHeight(out_h);
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
    // SAFETY: `SCRecordingOutputConfiguration::new()` is the documented
    // allocator for this mutable configuration object.
    let recording_config = unsafe { SCRecordingOutputConfiguration::new() };
    let file_url = {
        let path_str = output_path.to_string_lossy();
        let ns_str = objc2_foundation::NSString::from_str(&path_str);
        NSURL::fileURLWithPath(&ns_str)
    };
    // SAFETY: `recording_config` is a freshly allocated mutable configuration
    // and `file_url` is a valid `NSURL` for the output path.
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
        // SAFETY: `alloc` is a freshly allocated instance with ivars set;
        // `init` is the designated initializer inherited from `NSObject`.
        unsafe { objc2::msg_send![super(alloc), init] }
    };

    // 7. Create SCRecordingOutput
    let delegate_proto: &ProtocolObject<dyn SCRecordingOutputDelegate> =
        ProtocolObject::from_ref(&*delegate);
    // SAFETY: `recording_config` and `delegate_proto` are valid objects; this
    // is the documented initializer pattern.
    let recording_output = unsafe {
        SCRecordingOutput::initWithConfiguration_delegate(
            SCRecordingOutput::alloc(),
            &recording_config,
            delegate_proto,
        )
    };

    // 8. Create SCStream and add recording output
    // SAFETY: `filter` and `stream_config` are valid objects built above; no
    // stream delegate is required for recording.
    let stream = unsafe {
        SCStream::initWithFilter_configuration_delegate(
            SCStream::alloc(),
            &filter,
            &stream_config,
            None, // no stream delegate needed for recording
        )
    };

    // SAFETY: `recording_output` is a valid `SCRecordingOutput` created above.
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
            // SAFETY: `error` is a non-null `NSError` pointer passed by the
            // framework completion handler.
            let _ = start_tx.send(Err(unsafe { &*error }.to_string()));
        }
    });
    // SAFETY: `start_block` outlives this synchronous call; the framework
    // invokes the handler before returning.
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
            // SAFETY: `error` is a non-null `NSError` pointer passed by the
            // framework completion handler.
            let _ = stop_tx.send(Err(unsafe { &*error }.to_string()));
        }
    });
    // SAFETY: `stop_block` outlives this synchronous call; the framework
    // invokes the handler before returning.
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
    let (_guard, wait_res) = cvar
        .wait_timeout_while(guard, Duration::from_secs(15), |finished| !*finished)
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let timed_out = wait_res.timed_out();

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

    // The delegate never signalling completion, or an absent/empty file, means
    // no usable recording — do not report success (matches the CLI/ffmpeg paths).
    verify_recording_output(output_path, timed_out)?;

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
    let input = config.region.as_ref().map_or(display.to_string(), |r| {
        args.push("-video_size".into());
        args.push(format!("{}x{}", r.width, r.height));
        format!("{display}+{},{}", r.x, r.y)
    });
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

/// Build the `wf-recorder` argument vector for a wlroots Wayland recording.
///
/// Pure (no I/O) so the argv can be unit-tested on any host — which is the only
/// coverage this backend can honestly claim, since the development machine is
/// X11 (the same caveat the `sway` / `hyprland` window backends carry).
///
/// `wf-recorder` has no `-t/--duration`: it records until it is interrupted, so
/// the duration lives in the caller's stop logic, not here.
#[cfg(any(target_os = "linux", test))]
fn build_wf_recorder_args(
    config: &crate::screen_types::ScreenRecordConfig,
    output: &str,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-f".into(),
        output.to_string(),
        // Same codec/pixel-format pair as the x11grab and gdigrab paths, so a
        // recording is playable in the same set of players regardless of which
        // backend produced it.
        "-c".into(),
        "libx264".into(),
        "-x".into(),
        "yuv420p".into(),
        "-r".into(),
        config.fps.to_string(),
    ];
    if let Some(r) = &config.region {
        // wf-recorder geometry is one token: "x,y WxH".
        args.push("-g".into());
        args.push(format!("{},{} {}x{}", r.x, r.y, r.width, r.height));
    }
    if config.with_audio {
        // Bare `--audio` takes the default PulseAudio/PipeWire source, matching
        // the `-f pulse -i default` the ffmpeg paths use.
        args.push("--audio".into());
    }
    args
}

/// Which recorder can serve this Linux session.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordBackend {
    /// `ffmpeg -f x11grab` against a real X server.
    X11Grab,
    /// `wf-recorder`, which speaks the `wlr-screencopy` protocol.
    WfRecorder,
}

/// Choose a recording backend, or explain why this session has none.
///
/// Pure, and the reason this is a function rather than a chain of `if`s inline:
/// the matrix is four-way and every wrong branch is silent. In particular, a
/// **Wayland session that also exports `DISPLAY`** (i.e. one running XWayland,
/// which is nearly all of them) used to satisfy the old `DISPLAY`-is-set test
/// and go straight to x11grab — which sees only the XWayland root window, so the
/// recording came back black or showing a single legacy app, reported as a
/// success. The session type, not the presence of `DISPLAY`, is what decides.
#[cfg(any(target_os = "linux", test))]
fn pick_record_backend(
    session: crate::linux::LinuxSession,
    tb: &crate::linux::ToolBox,
    has_display: bool,
) -> Result<RecordBackend> {
    use crate::linux::Compositor;

    if session.kind.is_wayland() {
        // wf-recorder needs `wlr-screencopy`, which is a wlroots protocol: sway
        // and Hyprland have it, GNOME's Mutter and KDE's KWin do not.
        let wlroots = matches!(session.compositor, Compositor::Sway | Compositor::Hyprland);
        return match (wlroots, tb.has("wf-recorder")) {
            (true, true) => Ok(RecordBackend::WfRecorder),
            (true, false) => Err(DesktopError::NotAvailable(
                "Screen recording on this Wayland compositor goes through `wf-recorder`, \
                 which is not installed. Install it (`sudo apt install wf-recorder`) and \
                 retry. Still working meanwhile: screenshots, OCR and the accessibility tree."
                    .into(),
            )),
            (false, _) => Err(DesktopError::NotImplemented(
                "This Wayland compositor exposes no screen-recording interface Aleph can \
                 drive: `wf-recorder` needs the wlroots `wlr-screencopy` protocol (sway, \
                 Hyprland), and GNOME/KDE route capture through xdg-desktop-portal's \
                 ScreenCast API, which requires an interactive picker per session. \
                 Take periodic screenshots instead — those go through the portal and work here."
                    .into(),
            )),
        };
    }

    // X11, or a session we could not classify but which still has an X server.
    if has_display {
        return Ok(RecordBackend::X11Grab);
    }
    Err(DesktopError::NotImplemented(
        "Screen recording needs a display server. No X server is reachable (DISPLAY is \
         unset) and this is not a Wayland session — a daemon started outside a desktop \
         session sees exactly this."
            .into(),
    ))
}

/// Grace period for a recorder to finalise its container after being asked to
/// stop. A truncated MP4 is unplayable, so this is not optional.
#[cfg(target_os = "linux")]
const RECORDER_FINALISE_GRACE: std::time::Duration = std::time::Duration::from_secs(20);

/// Headroom over the requested duration before `ffmpeg` is considered hung.
///
/// `ffmpeg -t` stops itself, so exceeding this means it never got going —
/// typically an X server that stopped answering, or a display gone with a
/// suspended machine. Without the cap that hung the agent turn until the
/// harness's own ceiling and leaked the child; `media.rs` learned this first,
/// and this was the last capture path still running uncapped.
#[cfg(target_os = "linux")]
const FFMPEG_RECORD_OVERHEAD: std::time::Duration = std::time::Duration::from_secs(30);

/// Record the primary display (or a region) to MP4.
///
/// Linux desktop capture is fragmented by display server, so the backend is
/// chosen by [`pick_record_backend`]:
/// - **X11 / XWayland-as-X11**: `ffmpeg -f x11grab` — the single most broadly
///   available mechanism, reusing the same `ffmpeg` binary `media.rs` already
///   shells out to (no new crate dependency, R3).
/// - **wlroots Wayland** (sway / Hyprland): `wf-recorder`, mirroring the way
///   window management already picks a per-compositor backend.
/// - **GNOME / KDE Wayland**: an honest [`DesktopError::NotImplemented`] that
///   names the working alternative, rather than an x11grab recording of an empty
///   XWayland root.
#[cfg(target_os = "linux")]
pub fn screen_record(
    config: &crate::screen_types::ScreenRecordConfig,
) -> Result<crate::screen_types::ScreenRecordResult> {
    let mut config = config.clone().clamped();
    clamp_config_region(&mut config);

    let display = std::env::var("DISPLAY").ok().filter(|d| !d.is_empty());
    let backend = pick_record_backend(
        crate::linux::session(),
        crate::linux::tools(),
        display.is_some(),
    )?;

    let output_path = screen_record_output_path()?;
    let output_str = output_path.to_string_lossy().into_owned();

    match backend {
        RecordBackend::X11Grab => {
            // `display.is_some()` is what got us this backend.
            let display = display.unwrap_or_default();
            let args = build_x11grab_args(&display, &config, &output_str);
            let deadline =
                std::time::Duration::from_secs_f64(config.duration_secs) + FFMPEG_RECORD_OVERHEAD;

            let mut cmd = crate::script_exec::hidden_std_command("ffmpeg");
            cmd.args(&args);
            let output = crate::script_exec::output_capped_blocking(
                cmd,
                deadline,
                "Screen recording (ffmpeg x11grab)",
            )
            .map_err(|e| {
                if crate::script_exec::is_spawn_failure(&e) {
                    DesktopError::ScreenCapture(format!(
                        "Failed to run ffmpeg (install ffmpeg): {e}"
                    ))
                } else {
                    DesktopError::ScreenCapture(e.to_string())
                }
            })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(DesktopError::ScreenCapture(format!(
                    "ffmpeg x11grab recording failed: {}",
                    stderr.trim()
                )));
            }
        }
        RecordBackend::WfRecorder => wf_recorder_record(&config, &output_str)?,
    }

    if !output_path.exists() {
        return Err(DesktopError::ScreenCapture(
            "the recorder completed but the output file was not created".into(),
        ));
    }

    debug!(
        "Screen recording ({backend:?}) complete: {}",
        output_path.display()
    );

    Ok(crate::screen_types::ScreenRecordResult {
        file_path: output_str,
        duration_secs: config.duration_secs,
        has_audio: config.with_audio,
    })
}

/// Headroom over a recording's own length before it counts as wedged.
///
/// Mirrors `media.rs`'s `FFMPEG_MARGIN`, for the same reason: opening a capture
/// device and muxing the final file are slow on some hardware but not
/// minutes-slow, whereas a device held by another application never opens at all.
#[cfg(target_os = "windows")]
const RECORD_STARTUP_MARGIN: std::time::Duration = std::time::Duration::from_secs(45);

/// Run `wf-recorder` for `config.duration_secs`, then stop it cleanly.
///
/// `wf-recorder` has no duration flag — it records until interrupted — so the
/// stop is ours to perform, and it has to be `SIGINT`: that is the signal
/// wf-recorder handles to flush and close the container. `SIGKILL` leaves a
/// header-less file that no player will open, which is why the escalation below
/// only reaches for it after the finalise grace has elapsed (at which point
/// there is nothing to salvage anyway).
#[cfg(target_os = "linux")]
fn wf_recorder_record(
    config: &crate::screen_types::ScreenRecordConfig,
    output_str: &str,
) -> Result<()> {
    use std::process::Stdio;
    use std::time::Instant;

    let args = build_wf_recorder_args(config, output_str);
    let mut child = crate::script_exec::hidden_std_command("wf-recorder")
        .args(&args)
        // stdin null: wf-recorder asks "file exists, overwrite?" on a terminal,
        // and a recorder that blocks forever on a prompt nobody can see is the
        // worst of both worlds. (The path is freshly generated, so the prompt
        // should never appear — this makes sure it cannot hang if it does.)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            DesktopError::ScreenCapture(format!(
                "Failed to run wf-recorder (install wf-recorder): {e}"
            ))
        })?;

    // Drain stderr for the whole life of the recorder, not just after it exits.
    // wf-recorder runs for up to a minute here, and a child that fills the
    // 64 KiB pipe buffer blocks on the write — at which point it never gets to
    // handle our SIGINT, the finalise grace expires, and a recording that was
    // working gets killed and discarded. Same trap `output_capped_blocking`
    // documents; this path cannot use that helper because the stop is timed
    // rather than waited for.
    let stderr_reader = crate::script_exec::drain_on_thread(child.stderr.take());
    let read_stderr = move || {
        let bytes = stderr_reader
            .map(|h| h.join().unwrap_or_default())
            .unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
    };

    // `clamped()` has already coerced the duration into [0.25, 60.0], so this
    // cannot panic on a non-finite value.
    std::thread::sleep(std::time::Duration::from_secs_f64(config.duration_secs));

    // SAFETY: `child.id()` is the pid of a child of this very process, still
    // owned by the `Child` handle (so it cannot have been reaped and its pid
    // reused). `SIGINT` to it is wf-recorder's documented stop path.
    #[allow(clippy::cast_possible_wrap, reason = "`Child::id()` returns `u32` on unix and `pid_t` is `i32`; the cast can wrap only for pid values > i32::MAX, which the kernel never assigns on Linux/macOS")]
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGINT);
    }

    let deadline = Instant::now() + RECORDER_FINALISE_GRACE;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return Ok(());
                }
                // The child is gone, so the pipe is closed and the reader thread
                // has finished — joining here cannot block.
                let stderr = read_stderr();
                return Err(DesktopError::ScreenCapture(format!(
                    "wf-recorder exited with {status}: {}",
                    stderr.trim().lines().last().unwrap_or("no detail")
                )));
            }
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(DesktopError::ScreenCapture(format!(
                    "wf-recorder: failed to wait for the recorder: {e}"
                )));
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(DesktopError::ScreenCapture(format!(
                "wf-recorder did not finish writing the file within {}s of being asked to \
                 stop, so the recording is incomplete and was discarded.",
                RECORDER_FINALISE_GRACE.as_secs()
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
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
    use crate::script_exec::hidden_std_command;

    let mut config = config.clone().clamped();
    clamp_config_region(&mut config);

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

    // `hidden_std_command`: a console child spawned from the windowless daemon
    // would flash a black console over whatever the recording is capturing.
    let mut cmd = hidden_std_command("ffmpeg");
    cmd.args(&args);

    // `-t` is not a guarantee. When `with_audio` names a DirectShow microphone
    // that another application already holds, ffmpeg blocks in the device-open
    // call *before* the duration ever starts counting — the same infinite wait
    // `media.rs` caps, on the one capture path that had no cap. The margin
    // covers device negotiation and the final mux.
    let deadline =
        std::time::Duration::from_secs_f64(config.duration_secs.max(0.0)) + RECORD_STARTUP_MARGIN;
    let output = crate::script_exec::output_capped_blocking(
        cmd,
        deadline,
        "Screen recording (ffmpeg gdigrab)",
    )
    .map_err(|e| {
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
    fn wf_recorder_argv_carries_codec_fps_and_output() {
        let cfg = ScreenRecordConfig {
            duration_secs: 5.0,
            fps: 24,
            with_audio: false,
            region: None,
        };
        let args = build_wf_recorder_args(&cfg, "/tmp/out.mp4");
        assert!(args.windows(2).any(|w| w == ["-f", "/tmp/out.mp4"]));
        assert!(args.windows(2).any(|w| w == ["-c", "libx264"]));
        assert!(args.windows(2).any(|w| w == ["-r", "24"]));
        // No duration flag exists; the caller owns the stop.
        assert!(!args.iter().any(|a| a == "-t"), "{args:?}");
        assert!(!args.iter().any(|a| a == "--audio"));
    }

    #[test]
    fn wf_recorder_region_is_one_geometry_token() {
        let cfg = ScreenRecordConfig {
            duration_secs: 5.0,
            fps: 30,
            with_audio: true,
            region: Some(ScreenRegion {
                x: 10,
                y: 20,
                width: 640,
                height: 480,
            }),
        };
        let args = build_wf_recorder_args(&cfg, "/tmp/out.mp4");
        let g = args.iter().position(|a| a == "-g").expect("geometry flag");
        assert_eq!(args[g + 1], "10,20 640x480");
        assert!(args.iter().any(|a| a == "--audio"));
    }

    // ── Backend selection ────────────────────────────────────────────────

    fn session(
        kind: crate::linux::SessionKind,
        c: crate::linux::Compositor,
    ) -> crate::linux::LinuxSession {
        crate::linux::LinuxSession {
            kind,
            compositor: c,
        }
    }

    #[test]
    fn x11_records_through_x11grab() {
        use crate::linux::{Compositor, SessionKind, ToolBox};
        let tb = ToolBox::from_names(&[]);
        assert_eq!(
            pick_record_backend(session(SessionKind::X11, Compositor::Other), &tb, true).unwrap(),
            RecordBackend::X11Grab
        );
    }

    #[test]
    fn a_wayland_session_never_falls_back_to_x11grab_via_xwayland() {
        // The regression this guards: nearly every Wayland session also exports
        // DISPLAY for XWayland, so the old `if DISPLAY is set` test recorded the
        // XWayland root — a black or single-app video, reported as a success.
        use crate::linux::{Compositor, SessionKind, ToolBox};
        let tb = ToolBox::from_names(&["wf-recorder"]);
        let picked = pick_record_backend(
            session(SessionKind::Wayland, Compositor::Sway),
            &tb,
            /* has_display */ true,
        )
        .unwrap();
        assert_eq!(picked, RecordBackend::WfRecorder);
    }

    #[test]
    fn wlroots_without_wf_recorder_names_the_binary_to_install() {
        use crate::linux::{Compositor, SessionKind, ToolBox};
        let tb = ToolBox::from_names(&[]);
        for compositor in [Compositor::Sway, Compositor::Hyprland] {
            let err = pick_record_backend(session(SessionKind::Wayland, compositor), &tb, false)
                .unwrap_err();
            assert!(
                err.to_string().contains("wf-recorder"),
                "{compositor:?}: {err}"
            );
        }
    }

    #[test]
    fn gnome_and_kde_wayland_explain_themselves_and_name_a_working_route() {
        use crate::linux::{Compositor, SessionKind, ToolBox};
        // Even with wf-recorder installed: it needs wlr-screencopy, which these
        // compositors do not implement, so having the binary changes nothing.
        let tb = ToolBox::from_names(&["wf-recorder"]);
        for compositor in [Compositor::Gnome, Compositor::Kde, Compositor::Other] {
            let err = pick_record_backend(session(SessionKind::Wayland, compositor), &tb, true)
                .unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("screenshot"), "{compositor:?}: {msg}");
        }
    }

    #[test]
    fn no_display_server_at_all_is_an_honest_refusal() {
        use crate::linux::{Compositor, SessionKind, ToolBox};
        let tb = ToolBox::from_names(&["wf-recorder"]);
        let err = pick_record_backend(session(SessionKind::Unknown, Compositor::Other), &tb, false)
            .unwrap_err();
        assert!(err.to_string().contains("DISPLAY"), "{err}");
    }

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

    // ── Region clamping ─────────────────────────────────────────────────

    #[test]
    fn clamp_region_inside_display_is_unchanged() {
        let r = ScreenRegion {
            x: 10,
            y: 20,
            width: 640,
            height: 480,
        };
        assert_eq!(super::clamp_region_to_display(&r, 1920, 1080), Some(r));
    }

    #[test]
    fn clamp_region_overflow_shrinks_to_remaining_space() {
        let r = ScreenRegion {
            x: 1900,
            y: 1000,
            width: 300,
            height: 300,
        };
        assert_eq!(
            super::clamp_region_to_display(&r, 1920, 1080),
            Some(ScreenRegion {
                x: 1900,
                y: 1000,
                width: 20,
                height: 80,
            })
        );
    }

    #[test]
    fn clamp_region_absurd_size_is_bounded_to_the_display() {
        // The defect this guards: u32::MAX width/height spliced into ffmpeg's
        // -video_size made it try to allocate a gigantic frame buffer.
        let r = ScreenRegion {
            x: 0,
            y: 0,
            width: u32::MAX,
            height: u32::MAX,
        };
        assert_eq!(
            super::clamp_region_to_display(&r, 1920, 1080),
            Some(ScreenRegion {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            })
        );
    }

    #[test]
    fn clamp_region_origin_off_display_is_none() {
        let r = ScreenRegion {
            x: 1920,
            y: 0,
            width: 10,
            height: 10,
        };
        assert_eq!(super::clamp_region_to_display(&r, 1920, 1080), None);
    }

    #[test]
    fn clamp_region_zero_result_is_none() {
        // Zero-width request, and a zero-sized display, both leave nothing to
        // record — the caller falls back to full-screen.
        let r = ScreenRegion {
            x: 10,
            y: 10,
            width: 0,
            height: 50,
        };
        assert_eq!(super::clamp_region_to_display(&r, 1920, 1080), None);
        let r = ScreenRegion {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        assert_eq!(super::clamp_region_to_display(&r, 0, 0), None);
    }

    // `region` is in PHYSICAL PIXELS; the display args are POINTS; scale=2 → the
    // display spans 2000×1600 px. sourceRect = pixels ÷ scale (points); output =
    // clamped region pixels as-is.
    #[test]
    fn sck_region_rect_converts_pixels_to_source_points() {
        let r = crate::ScreenRegion {
            x: 10,
            y: 20,
            width: 100,
            height: 50,
        };
        assert_eq!(
            super::sck_region_rect(&r, 1000, 800, 2),
            Some(super::SckRegionRect {
                x: 5.0,
                y: 10.0,
                w: 50.0,
                h: 25.0,
                out_w: 100,
                out_h: 50
            })
        );
    }

    #[test]
    fn sck_region_rect_clamps_overflow_to_display() {
        // Far edge exceeds the display's pixel extent (2000×1600): width/height
        // clamp to what remains, in pixels, before the ÷scale conversion.
        let r = crate::ScreenRegion {
            x: 1900,
            y: 1500,
            width: 300,
            height: 300,
        };
        assert_eq!(
            super::sck_region_rect(&r, 1000, 800, 2),
            Some(super::SckRegionRect {
                x: 950.0,
                y: 750.0,
                w: 50.0,
                h: 50.0,
                out_w: 100,
                out_h: 100
            })
        );
    }

    #[test]
    fn sck_region_rect_origin_outside_is_none() {
        // x == display pixel width (1000 pts × 2) → no intersection.
        let r = crate::ScreenRegion {
            x: 2000,
            y: 0,
            width: 10,
            height: 10,
        };
        assert_eq!(super::sck_region_rect(&r, 1000, 800, 2), None);
    }

    #[test]
    fn sck_region_rect_zero_size_is_none() {
        let r = crate::ScreenRegion {
            x: 10,
            y: 10,
            width: 0,
            height: 50,
        };
        assert_eq!(super::sck_region_rect(&r, 1000, 800, 2), None);
    }

    // Retina case with odd pixel values: proves region is pixels (output == region
    // pixels as-is) and sourceRect = pixels ÷ scale, including fractional points.
    #[test]
    fn sck_region_rect_retina_preserves_fractional_points() {
        let r = crate::ScreenRegion {
            x: 101,
            y: 201,
            width: 641,
            height: 481,
        };
        assert_eq!(
            super::sck_region_rect(&r, 1440, 900, 2),
            Some(super::SckRegionRect {
                x: 50.5,
                y: 100.5,
                w: 320.5,
                h: 240.5,
                out_w: 641,
                out_h: 481
            })
        );
    }

    // Correctness anchor: a region equal to the whole display (in pixels) must
    // reproduce the whole-display path — sourceRect = full display points,
    // output = display points × scale.
    #[test]
    fn sck_region_rect_full_display_matches_whole_display_path() {
        let r = crate::ScreenRegion {
            x: 0,
            y: 0,
            width: 2000,  // 1000 pts × 2
            height: 1600, // 800 pts × 2
        };
        assert_eq!(
            super::sck_region_rect(&r, 1000, 800, 2),
            Some(super::SckRegionRect {
                x: 0.0,
                y: 0.0,
                w: 1000.0,
                h: 800.0,
                out_w: 2000,
                out_h: 1600
            })
        );
    }

    #[test]
    fn verify_recording_output_ok_for_nonempty_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("ok.mp4");
        std::fs::write(&f, b"data").unwrap();
        assert!(super::verify_recording_output(&f, false).is_ok());
    }

    #[test]
    fn verify_recording_output_err_on_timeout() {
        let f = std::path::Path::new("/nonexistent/whatever.mp4");
        assert!(super::verify_recording_output(f, true).is_err());
    }

    #[test]
    fn verify_recording_output_err_on_missing_or_empty() {
        assert!(
            super::verify_recording_output(std::path::Path::new("/no/such.mp4"), false).is_err()
        );
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("empty.mp4");
        std::fs::write(&f, b"").unwrap();
        assert!(super::verify_recording_output(&f, false).is_err());
    }
}
