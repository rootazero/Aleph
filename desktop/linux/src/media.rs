//! Linux `MediaCapability` implementation.
//!
//! Camera capture and audio recording via standard freedesktop CLI tools,
//! mirroring the shell-out approach already used by [`crate::system`]:
//!
//! - **Camera** (`camera_snap` / `camera_clip`): `ffmpeg` reading a V4L2
//!   device (`/dev/video0` by default).
//! - **Audio recording** (`record_audio`): `ffmpeg` capturing from `PulseAudio` /
//!   `PipeWire` (`-f pulse`), falling back to ALSA (`-f alsa`).
//! - **Audio device listing** (`list_audio_devices`): `pactl list short
//!   sources` (works on both `PulseAudio` and `PipeWire`'s `pipewire-pulse`).
//!
//! No heavy native bindings are introduced — this keeps the crate light (R3)
//! and works across X11 and Wayland sessions alike. `mic_level` is reported as
//! inactive rather than erroring so the opt-in mic-level reporter degrades
//! quietly on Linux.

use std::time::{SystemTime, UNIX_EPOCH};

use aleph_desktop::media_types::{
    AudioDeviceInfo, AudioRecordConfig, AudioRecordResult, CameraClipConfig, CameraClipResult,
    CameraSnapConfig, CameraSnapResult,
};
use aleph_desktop::traits::media::{MediaCapability, MicMeterSample};
use aleph_desktop::{DesktopError, Result};
use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};

/// Default V4L2 camera device. Overridable via the `ALEPH_CAMERA_DEVICE` env
/// var for hosts whose webcam is not enumerated first.
const DEFAULT_CAMERA_DEVICE: &str = "/dev/video0";

pub struct LinuxMedia;

impl LinuxMedia {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for LinuxMedia {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve the V4L2 camera device path (env override or default).
fn camera_device() -> String {
    std::env::var("ALEPH_CAMERA_DEVICE").unwrap_or_else(|_| DEFAULT_CAMERA_DEVICE.to_string())
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

#[async_trait]
impl MediaCapability for LinuxMedia {
    async fn camera_snap(&self, config: CameraSnapConfig) -> Result<CameraSnapResult> {
        let config = config.clamped();
        let device = camera_device();
        let out = temp_path("jpg");
        let out_str = out.to_string_lossy().to_string();
        let qv = quality_to_qv(config.quality);

        let args: Vec<String> = vec![
            "-hide_banner".into(),
            "-loglevel".into(),
            "error".into(),
            "-f".into(),
            "v4l2".into(),
            "-i".into(),
            device.clone(),
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

        // Read the JPEG, derive dimensions, base64-encode, then clean up.
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
        let device = camera_device();
        let out = temp_path("mp4");
        let out_str = out.to_string_lossy().to_string();

        let mut args: Vec<String> = vec![
            "-hide_banner".into(),
            "-loglevel".into(),
            "error".into(),
            "-f".into(),
            "v4l2".into(),
            "-i".into(),
            device.clone(),
        ];
        if config.with_audio {
            args.extend(["-f".into(), "pulse".into(), "-i".into(), "default".into()]);
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
        // `pactl` ships with both PulseAudio and PipeWire's pulse shim, so a
        // single tool covers the vast majority of modern Linux desktops.
        let output = tokio::process::Command::new("pactl")
            .args(["list", "short", "sources"])
            .output()
            .await
            .map_err(|e| {
                DesktopError::PlatformError(format!(
                    "Failed to list audio devices (install pulseaudio-utils / pipewire-pulse): {e}"
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::PlatformError(format!(
                "pactl failed: {}",
                stderr.trim()
            )));
        }

        // The default source name, used to flag `is_default`. Best-effort:
        // a failure here just means nothing is marked default.
        let default_name = tokio::process::Command::new("pactl")
            .args(["get-default-source"])
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(parse_pactl_sources(&stdout, default_name.as_deref()))
    }

    async fn record_audio(&self, config: AudioRecordConfig) -> Result<AudioRecordResult> {
        let config = config.clamped();
        let out = temp_path("m4a");
        let out_str = out.to_string_lossy().to_string();
        let dur = format!("{:.3}", config.duration_secs);

        // Prefer PulseAudio / PipeWire; fall back to ALSA for bare setups.
        let pulse_args: Vec<String> = vec![
            "-hide_banner".into(),
            "-loglevel".into(),
            "error".into(),
            "-f".into(),
            "pulse".into(),
            "-i".into(),
            "default".into(),
            "-t".into(),
            dur.clone(),
            "-y".into(),
            out_str.clone(),
        ];

        if run_ffmpeg(&pulse_args).await.is_err() {
            let alsa_args: Vec<String> = vec![
                "-hide_banner".into(),
                "-loglevel".into(),
                "error".into(),
                "-f".into(),
                "alsa".into(),
                "-i".into(),
                "default".into(),
                "-t".into(),
                dur,
                "-y".into(),
                out_str.clone(),
            ];
            run_ffmpeg(&alsa_args)
                .await
                .map_err(|e| DesktopError::PlatformError(format!("record_audio failed: {e}")))?;
        }

        Ok(AudioRecordResult {
            file_path: out_str,
            duration_secs: config.duration_secs,
            format: "m4a".to_string(),
        })
    }

    async fn mic_level(&self) -> Result<MicMeterSample> {
        // Linux has no warm-tap meter equivalent to macOS's AVAudioEngine here;
        // report inactive (Ok, not Err) so the opt-in mic-level reporter stays
        // quiet instead of log-spamming. Use `record_audio` for actual capture.
        Ok(MicMeterSample::inactive(
            "mic level metering is not implemented on Linux; use record_audio",
        ))
    }
}

/// Parse `pactl list short sources` output into [`AudioDeviceInfo`] entries.
///
/// Each line is tab-separated: `index  name  driver  sample_spec  state`.
/// Monitor sources (`*.monitor`) are loopbacks of output sinks, not real
/// capture devices, so they are excluded from the input-device list.
fn parse_pactl_sources(stdout: &str, default_name: Option<&str>) -> Vec<AudioDeviceInfo> {
    let mut devices = Vec::new();
    for line in stdout.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 2 {
            continue;
        }
        let name = fields[1].trim();
        if name.is_empty() || name.ends_with(".monitor") {
            continue;
        }
        devices.push(AudioDeviceInfo {
            uid: name.to_string(),
            name: name.to_string(),
            is_input: true,
            is_default: default_name == Some(name),
        });
    }
    devices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_default() {
        let _ = LinuxMedia;
    }

    #[test]
    fn quality_maps_to_qv_range() {
        assert_eq!(quality_to_qv(1.0), 2); // best
        assert!(quality_to_qv(0.05) >= 29); // worst end
                                            // monotonic: lower quality -> higher (worse) qv
        assert!(quality_to_qv(0.3) > quality_to_qv(0.9));
        // out-of-range inputs are clamped, never panic
        assert_eq!(quality_to_qv(5.0), 2);
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
    fn camera_device_honors_env_override() {
        // Default when unset.
        std::env::remove_var("ALEPH_CAMERA_DEVICE");
        assert_eq!(camera_device(), DEFAULT_CAMERA_DEVICE);
        std::env::set_var("ALEPH_CAMERA_DEVICE", "/dev/video9");
        assert_eq!(camera_device(), "/dev/video9");
        std::env::remove_var("ALEPH_CAMERA_DEVICE");
    }

    #[test]
    fn parses_pactl_sources_excluding_monitors() {
        let out = "0\talsa_input.pci-0000_00_1f.3.analog-stereo\tmodule.c\ts16le 2ch 44100Hz\tSUSPENDED\n\
                   1\talsa_output.pci-0000_00_1f.3.analog-stereo.monitor\tmodule.c\ts16le 2ch 44100Hz\tIDLE\n\
                   2\tbluez_source.AA_BB.headset\tmodule.c\ts16le 1ch 16000Hz\tRUNNING";
        let devices = parse_pactl_sources(out, Some("bluez_source.AA_BB.headset"));
        assert_eq!(devices.len(), 2, "monitor source must be excluded");
        assert_eq!(devices[0].name, "alsa_input.pci-0000_00_1f.3.analog-stereo");
        assert!(devices[0].is_input);
        assert!(!devices[0].is_default);
        assert!(devices[1].is_default, "default source must be flagged");
    }

    #[test]
    fn parses_pactl_sources_handles_empty_and_garbage() {
        assert!(parse_pactl_sources("", None).is_empty());
        assert!(parse_pactl_sources("garbage-without-tabs", None).is_empty());
    }
}
