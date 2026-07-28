//! Windows `MediaCapability` implementation.
//!
//! Camera capture and audio recording via `ffmpeg`'s `DirectShow` (`dshow`)
//! input, mirroring the shell-out approach used by the Linux platform (which
//! targets V4L2 / `PulseAudio`). This keeps both platforms on one battle-tested
//! subprocess pattern instead of introducing native Win32/WinRT capture
//! bindings (R3 — core stays light):
//!
//! - **Camera** (`camera_snap` / `camera_clip`): `ffmpeg -f dshow -i
//!   video="<name>"`. The device name comes from `ALEPH_CAMERA_DEVICE` or, when
//!   unset, the first enumerated `DirectShow` video device.
//! - **Audio recording** (`record_audio`): `ffmpeg -f dshow -i audio="<name>"`,
//!   the name coming from `ALEPH_AUDIO_DEVICE` or the first enumerated audio
//!   device.
//! - **Audio device listing** (`list_audio_devices`): parsed from `ffmpeg
//!   -list_devices true -f dshow -i dummy` (ffmpeg prints the device table to
//!   stderr and exits non-zero because `dummy` is unopenable — that exit code is
//!   expected and ignored).
//!
//! `speech_to_text` falls back to the trait default (`NotImplemented`, as on
//! Linux); `mic_level` reports inactive (Ok) so the opt-in mic-level reporter
//! degrades quietly rather than log-spamming.

use std::time::{SystemTime, UNIX_EPOCH};

use aleph_desktop::media_types::{
    AudioDeviceInfo, AudioRecordConfig, AudioRecordResult, CameraClipConfig, CameraClipResult,
    CameraSnapConfig, CameraSnapResult,
};
use aleph_desktop::traits::media::{MediaCapability, MicMeterSample};
use aleph_desktop::{DesktopError, Result};
use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};

pub struct WindowsMedia;

impl WindowsMedia {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for WindowsMedia {
    fn default() -> Self {
        Self::new()
    }
}

/// Kind of a `DirectShow` capture device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DshowKind {
    Video,
    Audio,
}

/// A `DirectShow` device discovered via `ffmpeg -list_devices`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DshowDevice {
    name: String,
    kind: DshowKind,
}

/// Build a unique temp-file path under the system temp dir.
///
/// Uses pid + nanosecond clock to avoid collisions between concurrent captures
/// without pulling in the `tempfile` crate as a runtime dependency.
fn temp_path(ext: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!("aleph-media-{}-{nanos}.{ext}", std::process::id()))
}

/// A capture's requested length as a [`Duration`](std::time::Duration).
///
/// The config values are already clamped to a sane range by `clamped()`;
/// `from_secs_f64` panics on a negative or non-finite input, so the guard here
/// is about never letting a malformed value reach it (P7).
fn duration_of(secs: f64) -> std::time::Duration {
    if secs.is_finite() && secs > 0.0 {
        std::time::Duration::from_secs_f64(secs)
    } else {
        std::time::Duration::ZERO
    }
}

/// Map a 0.05–1.0 quality knob to an ffmpeg mjpeg `-q:v` value (2 = best,
/// 31 = worst).
fn quality_to_qv(quality: f32) -> u32 {
    let q = quality.clamp(0.05, 1.0);
    // q=1.0 -> 2, q=0.05 -> ~30
    (1.0 - q).mul_add(29.0, 2.0).round() as u32
}

/// How many frames to let a webcam throw away before the one that is kept.
///
/// A DirectShow camera streams from the instant it opens, but its auto-exposure,
/// auto-white-balance and (on most laptop panels) the physical shutter need a
/// few frames to settle. `-frames:v 1` took the very first one, so
/// `camera_snap` habitually returned a black or washed-out image — a picture
/// technically taken and practically useless, and one a model has no way to
/// recognize as a device artefact rather than a dark room.
///
/// Five frames is well under a fifth of a second on any 30 fps device, so the
/// call is not measurably slower; the cost is entirely in the device-open that
/// was already being paid.
const CAMERA_WARMUP_FRAMES: u32 = 5;

/// Build the argument vector for a single-frame camera capture.
///
/// Pure (no I/O) so the frame-selection filter can be pinned by a unit test
/// without a webcam.
fn camera_snap_args(device: &str, qv: u32, output: &str) -> Vec<String> {
    vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-f".into(),
        "dshow".into(),
        "-i".into(),
        format!("video={device}"),
        // Keep frame N, discard everything before it. `-ss` cannot do this: a
        // live capture device has no seekable timeline.
        "-vf".into(),
        format!("select=gte(n\\,{CAMERA_WARMUP_FRAMES})"),
        "-frames:v".into(),
        "1".into(),
        "-q:v".into(),
        qv.to_string(),
        "-y".into(),
        output.to_string(),
    ]
}

/// Headroom over a capture's own duration before it is considered wedged.
///
/// `ffmpeg -f dshow -i video=…` blocks in the device-open call when another
/// application (a video call, say) already holds the camera or microphone, and
/// it blocks *indefinitely* — there is no DirectShow open timeout. Without a cap
/// the tool call hangs until the harness's per-turn ceiling and leaks the ffmpeg
/// child behind it. The margin covers device negotiation and the final mux,
/// which are slow on some webcams but not minutes-slow.
const FFMPEG_MARGIN: std::time::Duration = std::time::Duration::from_secs(45);

/// Run an ffmpeg invocation under a deadline, mapping a missing binary, a
/// timeout, or a non-zero exit to a friendly [`DesktopError`].
///
/// `expected` is how long the capture itself should take (zero for a single
/// frame or a device probe); the deadline is that plus [`FFMPEG_MARGIN`].
async fn run_ffmpeg(args: &[String], expected: std::time::Duration) -> Result<()> {
    let mut cmd = aleph_desktop::script_exec::hidden_command("ffmpeg");
    cmd.args(args);
    // Without `kill_on_drop` the child outlives the timed-out future as an
    // orphan still holding the capture device.
    cmd.kill_on_drop(true);

    let deadline = expected + FFMPEG_MARGIN;
    let output = match tokio::time::timeout(deadline, cmd.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            return Err(DesktopError::PlatformError(format!(
                "Failed to run ffmpeg (install ffmpeg): {e}"
            )))
        }
        Err(_elapsed) => {
            return Err(DesktopError::PlatformError(format!(
                "ffmpeg did not finish within {}s and was terminated. The capture device is \
                 usually held by another application (a video call, or a previous capture that \
                 has not released it) — close it and retry.",
                deadline.as_secs()
            )))
        }
    };

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(DesktopError::PlatformError(format!(
            "ffmpeg failed: {}",
            stderr.trim().lines().last().unwrap_or("unknown error")
        )))
    }
}

/// Enumerate `DirectShow` capture devices via ffmpeg.
///
/// `ffmpeg -list_devices true -f dshow -i dummy` prints the device table to
/// stderr and exits non-zero (the `dummy` pseudo-device can't be opened); that
/// non-zero exit is expected, so the stderr is parsed regardless of status.
async fn list_dshow_devices() -> Result<Vec<DshowDevice>> {
    let mut cmd = aleph_desktop::script_exec::hidden_command("ffmpeg");
    cmd.args([
        "-hide_banner",
        "-list_devices",
        "true",
        "-f",
        "dshow",
        "-i",
        "dummy",
    ]);
    cmd.kill_on_drop(true);

    // Enumeration walks every registered DirectShow filter, and a wedged driver
    // can stall that walk; the same margin the capture paths use bounds it.
    let output = match tokio::time::timeout(FFMPEG_MARGIN, cmd.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            return Err(DesktopError::PlatformError(format!(
                "Failed to run ffmpeg (install ffmpeg): {e}"
            )))
        }
        Err(_elapsed) => {
            return Err(DesktopError::PlatformError(format!(
                "listing DirectShow devices did not finish within {}s; a capture driver is not \
                 responding.",
                FFMPEG_MARGIN.as_secs()
            )))
        }
    };

    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(parse_dshow_devices(&stderr))
}

/// Extract the first double-quoted substring from a line, if any.
fn extract_quoted(s: &str) -> Option<String> {
    let start = s.find('"')?;
    let rest = &s[start + 1..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Parse `ffmpeg -list_devices` stderr into [`DshowDevice`] entries.
///
/// Handles both ffmpeg output dialects:
/// - **Modern** (ffmpeg 5+): each device line carries an explicit `(video)` or
///   `(audio)` suffix, e.g. `[dshow @ ..] "Integrated Camera" (video)`.
/// - **Legacy**: devices are grouped under `DirectShow video devices` /
///   `DirectShow audio devices` section headers with no per-line suffix.
///
/// `Alternative name "@device_..."` lines are skipped.
fn parse_dshow_devices(stderr: &str) -> Vec<DshowDevice> {
    let mut devices = Vec::new();
    let mut section: Option<DshowKind> = None;

    for line in stderr.lines() {
        let l = line.trim();

        // Legacy section headers.
        if l.contains("DirectShow video devices") {
            section = Some(DshowKind::Video);
            continue;
        }
        if l.contains("DirectShow audio devices") {
            section = Some(DshowKind::Audio);
            continue;
        }

        // Skip the per-device "Alternative name" lines (also quoted).
        if l.contains("Alternative name") {
            continue;
        }

        let name = match extract_quoted(l) {
            Some(n) if !n.is_empty() => n,
            _ => continue,
        };

        // Prefer the explicit modern suffix; fall back to the legacy section.
        let kind = if l.ends_with("(video)") {
            Some(DshowKind::Video)
        } else if l.ends_with("(audio)") {
            Some(DshowKind::Audio)
        } else {
            section
        };

        if let Some(kind) = kind {
            devices.push(DshowDevice { name, kind });
        }
    }

    devices
}

/// Project enumerated devices into the audio-input view used by the trait.
///
/// `DirectShow` exposes no "default device" flag, so the first audio device is
/// marked default as a best-effort heuristic (matching the order ffmpeg
/// reports, which tracks the system's preferred device on most setups).
fn audio_input_devices(parsed: &[DshowDevice]) -> Vec<AudioDeviceInfo> {
    parsed
        .iter()
        .filter(|d| d.kind == DshowKind::Audio)
        .enumerate()
        .map(|(i, d)| AudioDeviceInfo {
            uid: d.name.clone(),
            name: d.name.clone(),
            is_input: true,
            is_default: i == 0,
        })
        .collect()
}

/// First video device name, if any.
fn first_video_device(parsed: &[DshowDevice]) -> Option<String> {
    parsed
        .iter()
        .find(|d| d.kind == DshowKind::Video)
        .map(|d| d.name.clone())
}

/// First audio device name, if any.
fn first_audio_device(parsed: &[DshowDevice]) -> Option<String> {
    parsed
        .iter()
        .find(|d| d.kind == DshowKind::Audio)
        .map(|d| d.name.clone())
}

/// Resolve the camera device name: `ALEPH_CAMERA_DEVICE` or the first
/// enumerated `DirectShow` video device.
async fn resolve_camera_device() -> Result<String> {
    if let Ok(name) = std::env::var("ALEPH_CAMERA_DEVICE") {
        if !name.trim().is_empty() {
            return Ok(name);
        }
    }
    let devices = list_dshow_devices().await?;
    first_video_device(&devices).ok_or_else(|| {
        DesktopError::PlatformError(
            "no DirectShow video device found; set ALEPH_CAMERA_DEVICE".into(),
        )
    })
}

/// Resolve the microphone device name: `ALEPH_AUDIO_DEVICE` or the first
/// enumerated `DirectShow` audio device.
async fn resolve_audio_device() -> Result<String> {
    if let Ok(name) = std::env::var("ALEPH_AUDIO_DEVICE") {
        if !name.trim().is_empty() {
            return Ok(name);
        }
    }
    let devices = list_dshow_devices().await?;
    first_audio_device(&devices).ok_or_else(|| {
        DesktopError::PlatformError(
            "no DirectShow audio device found; set ALEPH_AUDIO_DEVICE".into(),
        )
    })
}

#[async_trait]
impl MediaCapability for WindowsMedia {
    async fn camera_snap(&self, config: CameraSnapConfig) -> Result<CameraSnapResult> {
        let config = config.clamped();
        let device = resolve_camera_device().await?;
        let out = temp_path("jpg");
        let out_str = out.to_string_lossy().to_string();
        let qv = quality_to_qv(config.quality);

        let args: Vec<String> = camera_snap_args(&device, qv, &out_str);

        // A single frame: the whole cost is opening the device.
        run_ffmpeg(&args, std::time::Duration::ZERO)
            .await
            .map_err(|e| {
                DesktopError::PlatformError(format!("camera_snap from {device} failed: {e}"))
            })?;

        let bytes = tokio::task::spawn_blocking(move || std::fs::read(&out))
            .await
            .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))?
            .map_err(|e| {
                DesktopError::PlatformError(format!("Failed to read captured frame: {e}"))
            })?;

        let (width, height) = image::load_from_memory(&bytes).map_or((0, 0), |img| {
            use image::GenericImageView as _;
            img.dimensions()
        });

        let _ = tokio::fs::remove_file(&out_str).await;

        Ok(CameraSnapResult {
            image_base64: general_purpose::STANDARD.encode(&bytes),
            width,
            height,
        })
    }

    async fn camera_clip(&self, config: CameraClipConfig) -> Result<CameraClipResult> {
        let config = config.clamped();
        let device = resolve_camera_device().await?;
        let out = temp_path("mp4");
        let out_str = out.to_string_lossy().to_string();

        let mut args: Vec<String> = vec![
            "-hide_banner".into(),
            "-loglevel".into(),
            "error".into(),
            "-f".into(),
            "dshow".into(),
            "-i".into(),
            format!("video={device}"),
        ];
        if config.with_audio {
            let mic = resolve_audio_device().await?;
            args.extend([
                "-f".into(),
                "dshow".into(),
                "-i".into(),
                format!("audio={mic}"),
            ]);
        }
        args.extend([
            "-t".into(),
            format!("{:.3}", config.duration_secs),
            "-y".into(),
            out_str.clone(),
        ]);

        run_ffmpeg(&args, duration_of(config.duration_secs))
            .await
            .map_err(|e| {
                DesktopError::PlatformError(format!("camera_clip from {device} failed: {e}"))
            })?;

        Ok(CameraClipResult {
            file_path: out_str,
            duration_secs: config.duration_secs,
            has_audio: config.with_audio,
        })
    }

    async fn list_audio_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        let devices = list_dshow_devices().await?;
        Ok(audio_input_devices(&devices))
    }

    async fn record_audio(&self, config: AudioRecordConfig) -> Result<AudioRecordResult> {
        let config = config.clamped();
        let device = resolve_audio_device().await?;
        let out = temp_path("m4a");
        let out_str = out.to_string_lossy().to_string();

        let args: Vec<String> = vec![
            "-hide_banner".into(),
            "-loglevel".into(),
            "error".into(),
            "-f".into(),
            "dshow".into(),
            "-i".into(),
            format!("audio={device}"),
            "-t".into(),
            format!("{:.3}", config.duration_secs),
            "-y".into(),
            out_str.clone(),
        ];

        run_ffmpeg(&args, duration_of(config.duration_secs))
            .await
            .map_err(|e| DesktopError::PlatformError(format!("record_audio failed: {e}")))?;

        Ok(AudioRecordResult {
            file_path: out_str,
            duration_secs: config.duration_secs,
            format: "m4a".to_string(),
        })
    }

    async fn mic_level(&self) -> Result<MicMeterSample> {
        // No warm-tap meter on Windows here; report inactive (Ok, not Err) so
        // the opt-in mic-level reporter stays quiet. Use `record_audio` for
        // actual capture.
        Ok(MicMeterSample::inactive(
            "mic level metering is not implemented on Windows; use record_audio",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let _ = WindowsMedia::default();
    }

    #[test]
    fn quality_maps_to_qv_range() {
        assert_eq!(quality_to_qv(1.0), 2); // best
        assert!(quality_to_qv(0.05) >= 29); // worst end
        assert!(quality_to_qv(0.3) > quality_to_qv(0.9)); // monotonic
        assert_eq!(quality_to_qv(5.0), 2); // clamped, never panics
        assert!(quality_to_qv(-1.0) >= 29);
    }

    #[test]
    fn temp_path_is_unique_and_under_tmp() {
        let a = temp_path("jpg");
        let b = temp_path("jpg");
        assert_ne!(a, b);
        assert!(a.starts_with(std::env::temp_dir()));
        assert_eq!(a.extension().and_then(|e| e.to_str()), Some("jpg"));
    }

    #[test]
    fn extract_quoted_basic() {
        assert_eq!(extract_quoted(r#"x "hello" y"#).as_deref(), Some("hello"));
        assert_eq!(extract_quoted("no quotes"), None);
        assert_eq!(extract_quoted(r#"unterminated "open"#).as_deref(), None);
    }

    #[test]
    fn parses_modern_suffix_format() {
        let out = "\
[dshow @ 0001] \"Integrated Camera\" (video)\n\
[dshow @ 0001]   Alternative name \"@device_pnp_cam\"\n\
[dshow @ 0001] \"Microphone (Realtek Audio)\" (audio)\n\
[dshow @ 0001]   Alternative name \"@device_cm_mic\"\n";
        let devices = parse_dshow_devices(out);
        assert_eq!(devices.len(), 2, "alternative-name lines must be skipped");
        assert_eq!(devices[0].name, "Integrated Camera");
        assert_eq!(devices[0].kind, DshowKind::Video);
        assert_eq!(devices[1].name, "Microphone (Realtek Audio)");
        assert_eq!(devices[1].kind, DshowKind::Audio);
    }

    #[test]
    fn parses_legacy_section_format() {
        let out = "\
[dshow @ 0001] DirectShow video devices (some options omitted)\n\
[dshow @ 0001]  \"Integrated Camera\"\n\
[dshow @ 0001]  Alternative name \"@device_pnp_cam\"\n\
[dshow @ 0001] DirectShow audio devices\n\
[dshow @ 0001]  \"Microphone (Realtek Audio)\"\n\
[dshow @ 0001]  Alternative name \"@device_cm_mic\"\n";
        let devices = parse_dshow_devices(out);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].name, "Integrated Camera");
        assert_eq!(devices[0].kind, DshowKind::Video);
        assert_eq!(devices[1].name, "Microphone (Realtek Audio)");
        assert_eq!(devices[1].kind, DshowKind::Audio);
    }

    #[test]
    fn parses_empty_and_garbage() {
        assert!(parse_dshow_devices("").is_empty());
        assert!(parse_dshow_devices("garbage without quotes or sections").is_empty());
    }

    #[test]
    fn audio_view_filters_video_and_marks_first_default() {
        let parsed = vec![
            DshowDevice {
                name: "Cam".into(),
                kind: DshowKind::Video,
            },
            DshowDevice {
                name: "Mic A".into(),
                kind: DshowKind::Audio,
            },
            DshowDevice {
                name: "Mic B".into(),
                kind: DshowKind::Audio,
            },
        ];
        let audio = audio_input_devices(&parsed);
        assert_eq!(audio.len(), 2, "video device must be excluded");
        assert_eq!(audio[0].name, "Mic A");
        assert!(audio[0].is_input);
        assert!(
            audio[0].is_default,
            "first audio device is best-effort default"
        );
        assert!(!audio[1].is_default);
    }

    #[test]
    fn camera_snap_discards_the_warmup_frames() {
        let args = camera_snap_args("Integrated Camera", 4, r"C:\tmp\shot.jpg");
        let joined = args.join(" ");
        assert!(
            joined.contains(&format!("select=gte(n\\,{CAMERA_WARMUP_FRAMES})")),
            "the frame-selection filter must survive assembly: {joined}"
        );
        // Still exactly one frame out, at the requested quality, to the given
        // path — the warm-up must not change what the caller asked for.
        assert!(joined.contains("-frames:v 1"));
        assert!(joined.contains("-q:v 4"));
        assert_eq!(args.last().unwrap(), r"C:\tmp\shot.jpg");
        assert!(joined.contains("video=Integrated Camera"));
    }

    #[test]
    fn first_device_helpers() {
        let parsed = vec![
            DshowDevice {
                name: "Mic A".into(),
                kind: DshowKind::Audio,
            },
            DshowDevice {
                name: "Cam".into(),
                kind: DshowKind::Video,
            },
        ];
        assert_eq!(first_video_device(&parsed).as_deref(), Some("Cam"));
        assert_eq!(first_audio_device(&parsed).as_deref(), Some("Mic A"));
        assert_eq!(first_video_device(&[]), None);
    }
}
