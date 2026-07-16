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

/// Map a 0.05–1.0 quality knob to an ffmpeg mjpeg `-q:v` value (2 = best,
/// 31 = worst).
fn quality_to_qv(quality: f32) -> u32 {
    let q = quality.clamp(0.05, 1.0);
    // q=1.0 -> 2, q=0.05 -> ~30
    (1.0 - q).mul_add(29.0, 2.0).round() as u32
}

/// Run an ffmpeg invocation, mapping a missing binary / non-zero exit to a
/// friendly [`DesktopError`].
async fn run_ffmpeg(args: &[String]) -> Result<()> {
    let output = tokio::process::Command::new("ffmpeg")
        .args(args)
        .output()
        .await
        .map_err(|e| {
            DesktopError::PlatformError(format!("Failed to run ffmpeg (install ffmpeg): {e}"))
        })?;

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
    let output = tokio::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-list_devices",
            "true",
            "-f",
            "dshow",
            "-i",
            "dummy",
        ])
        .output()
        .await
        .map_err(|e| {
            DesktopError::PlatformError(format!("Failed to run ffmpeg (install ffmpeg): {e}"))
        })?;

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

        let args: Vec<String> = vec![
            "-hide_banner".into(),
            "-loglevel".into(),
            "error".into(),
            "-f".into(),
            "dshow".into(),
            "-i".into(),
            format!("video={device}"),
            "-frames:v".into(),
            "1".into(),
            "-q:v".into(),
            qv.to_string(),
            "-y".into(),
            out_str.clone(),
        ];

        run_ffmpeg(&args).await.map_err(|e| {
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

        run_ffmpeg(&args).await.map_err(|e| {
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

        run_ffmpeg(&args)
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
