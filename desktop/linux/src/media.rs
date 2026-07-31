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

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aleph_desktop::media_types::{
    AudioDeviceInfo, AudioRecordConfig, AudioRecordResult, CameraClipConfig, CameraClipResult,
    CameraSnapConfig, CameraSnapResult,
};
use aleph_desktop::script_exec::{is_deadline_failure, is_spawn_failure, output_capped};
use aleph_desktop::traits::media::{MediaCapability, MicMeterSample};
use aleph_desktop::{DesktopError, Result};
use async_trait::async_trait;
use base64::{engine::general_purpose, Engine as _};

/// Fallback V4L2 camera device when nothing can be enumerated.
///
/// Kept as a last resort rather than as *the* answer: see [`camera_device`].
const DEFAULT_CAMERA_DEVICE: &str = "/dev/video0";

/// Frames discarded before the one that is kept.
///
/// A UVC webcam's first frames come out before auto-exposure and auto-white-
/// balance have converged — typically black or a wash of green. Handing one of
/// those to the model is worse than a failure, because the model cannot tell
/// "the camera is broken" from "the room is dark" and will reason about the
/// wrong one. Windows discards the same way for the same reason (dshow, 2026-07).
const CAMERA_WARMUP_FRAMES: u32 = 5;

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

/// Resolve the V4L2 camera device path: the env override, else the first node
/// that looks like a capture device, else [`DEFAULT_CAMERA_DEVICE`].
fn camera_device() -> String {
    if let Ok(explicit) = std::env::var("ALEPH_CAMERA_DEVICE") {
        return explicit;
    }
    pick_capture_node(&enumerate_video_nodes()).unwrap_or_else(|| DEFAULT_CAMERA_DEVICE.to_string())
}

/// One `/dev/videoN` node as the selection needs to see it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VideoNode {
    /// The `N` in `/dev/videoN`, which is also the enumeration order.
    number: u32,
    /// The V4L2 device index within its parent hardware, from
    /// `/sys/class/video4linux/videoN/index`, or `None` when sysfs does not say.
    index: Option<u32>,
}

/// Read every `/dev/videoN` and its sysfs index.
fn enumerate_video_nodes() -> Vec<VideoNode> {
    let Ok(entries) = std::fs::read_dir("/dev") else {
        return Vec::new();
    };
    let mut nodes: Vec<VideoNode> = entries
        .filter_map(|e| {
            let name = e.ok()?.file_name().into_string().ok()?;
            let number: u32 = name.strip_prefix("video")?.parse().ok()?;
            let index = std::fs::read_to_string(format!("/sys/class/video4linux/{name}/index"))
                .ok()
                .and_then(|s| s.trim().parse().ok());
            Some(VideoNode { number, index })
        })
        .collect();
    nodes.sort_by_key(|n| n.number);
    nodes
}

/// Choose the node most likely to be a capture device.
///
/// `/dev/video0` is not reliably the webcam. A UVC camera registers **two**
/// nodes — the capture node and a metadata node — and on a laptop with an
/// infrared sensor for face unlock there are four, so the lowest number is
/// regularly a device that yields no picture at all. ffmpeg then fails with
/// "Not a video capture device" and the user is told their camera is broken.
///
/// The discriminator is the V4L2 device index sysfs exposes: within one piece of
/// hardware the capture node is index 0 and the ancillary nodes count upward. So
/// prefer the lowest-numbered node whose index is 0.
///
/// When sysfs says nothing (a container without `/sys`, an out-of-tree driver)
/// this falls back to the lowest node — which is exactly the previous
/// behaviour, so the change can only improve the answer, never replace a working
/// one with a guess.
///
/// Pure, so the whole ordering is testable on a host with no camera.
fn pick_capture_node(nodes: &[VideoNode]) -> Option<String> {
    let chosen = nodes
        .iter()
        .find(|n| n.index == Some(0))
        .or_else(|| nodes.first())?;
    Some(format!("/dev/video{}", chosen.number))
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

/// Headroom over a capture's own duration before it is considered hung.
///
/// Covers device open, format negotiation and container finalisation, all of
/// which happen outside the requested capture window.
const FFMPEG_OVERHEAD: Duration = Duration::from_secs(20);

/// Deadline for a `pactl` query.
///
/// Device enumeration is a sub-second round-trip to the sound server; the cap is
/// for the case where that server is wedged, which is why it is far tighter than
/// [`FFMPEG_OVERHEAD`] (that one has to cover an actual capture).
const PACTL_TIMEOUT: Duration = Duration::from_secs(5);

/// Deadline for a capture that should take `capture_secs` of wall clock.
fn ffmpeg_deadline(capture_secs: f64) -> Duration {
    let secs = if capture_secs.is_finite() && capture_secs > 0.0 {
        capture_secs
    } else {
        0.0
    };
    Duration::from_secs_f64(secs) + FFMPEG_OVERHEAD
}

/// Argv for a single still frame, discarding the sensor's warm-up frames.
///
/// `select=gte(n,K)` drops the first `K` frames inside the filter graph and
/// `-frames:v 1` then takes the first that survives, so exactly one frame is
/// encoded — the capture still costs only the handful of frame intervals the
/// sensor needs to settle. `-vsync 0` keeps ffmpeg from duplicating or dropping
/// around the gap the filter leaves in the timestamps.
///
/// Pure, so the flag order is pinned by a test rather than by a live camera.
fn snap_args(device: &str, qv: u32, out_path: &str) -> Vec<String> {
    vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-f".into(),
        "v4l2".into(),
        "-i".into(),
        device.to_string(),
        "-vf".into(),
        format!("select=gte(n\\,{CAMERA_WARMUP_FRAMES})"),
        "-vsync".into(),
        "0".into(),
        "-frames:v".into(),
        "1".into(),
        "-q:v".into(),
        qv.to_string(),
        "-y".into(),
        out_path.to_string(),
    ]
}

/// Run an ffmpeg invocation under a deadline, mapping a missing binary,
/// a timeout, or a non-zero exit to a friendly [`DesktopError`].
///
/// The deadline is the point: a V4L2 device that is already open by another
/// process, or a PulseAudio server that never answers, leaves `ffmpeg` blocked
/// forever. Without a cap that became the agent's problem — the turn hung until
/// the harness's own 300s ceiling and the child was leaked. `output_capped`
/// kills the child on elapse and is the same helper `automation.rs` already
/// uses for scripts; this was the one capture path still running uncapped.
async fn run_ffmpeg(args: &[String], deadline: Duration) -> Result<()> {
    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.args(args);

    let output = output_capped(cmd, deadline).await.map_err(|e| {
        if is_spawn_failure(&e) {
            DesktopError::PlatformError(format!("Failed to run ffmpeg (install ffmpeg): {e}"))
        } else if is_deadline_failure(&e) {
            // Passed through unchanged, variant included: callers classify on it
            // to decide whether a second candidate is worth trying at all, and
            // rewrapping would erase the only signal that says "it hung" rather
            // than "it refused".
            e
        } else {
            DesktopError::PlatformError(format!("ffmpeg: {e}"))
        }
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

        let args = snap_args(&device, qv, &out_str);

        // A single frame: no capture window of its own, just the overhead budget.
        // A failed ffmpeg can still have written a partial output file, so the
        // temp path is cleaned up on this path too.
        if let Err(e) = run_ffmpeg(&args, ffmpeg_deadline(0.0)).await {
            let _ = tokio::fs::remove_file(&out_str).await;
            return Err(DesktopError::PlatformError(format!(
                "camera_snap from {device} failed: {e}"
            )));
        }

        // Read the JPEG, derive dimensions, base64-encode, then clean up. The
        // temp file is removed unconditionally: a read failure must not leave
        // `aleph-media-*.jpg` behind in the temp dir.
        let read_result = tokio::task::spawn_blocking(move || std::fs::read(&out))
            .await
            .map_err(|e| DesktopError::PlatformError(format!("task join error: {e}")))
            .and_then(|r| {
                r.map_err(|e| {
                    DesktopError::PlatformError(format!("Failed to read captured frame: {e}"))
                })
            });
        let _ = tokio::fs::remove_file(&out_str).await;
        let bytes = read_result?;

        let (width, height) = image::load_from_memory(&bytes).map_or((0, 0), |img| {
            use image::GenericImageView as _;
            img.dimensions()
        });

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

        run_ffmpeg(&args, ffmpeg_deadline(config.duration_secs))
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
        // `pactl` ships with both PulseAudio and PipeWire's pulse shim, so a
        // single tool covers the vast majority of modern Linux desktops.
        // Capped: `pactl` waits on the sound server, and a wedged
        // PulseAudio/PipeWire daemon leaves it blocked with no timeout of its
        // own — the same failure mode the clipboard and compositor probes have.
        let mut cmd = tokio::process::Command::new("pactl");
        cmd.args(["list", "short", "sources"]);
        let output = output_capped(cmd, PACTL_TIMEOUT).await.map_err(|e| {
            if is_spawn_failure(&e) {
                DesktopError::PlatformError(format!(
                    "Failed to list audio devices (install pulseaudio-utils / pipewire-pulse): {e}"
                ))
            } else {
                DesktopError::PlatformError(e.to_string())
            }
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
        let mut cmd = tokio::process::Command::new("pactl");
        cmd.args(["get-default-source"]);
        let default_name = output_capped(cmd, PACTL_TIMEOUT)
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

        let deadline = ffmpeg_deadline(config.duration_secs);
        // ALSA is tried only when PulseAudio failed *fast*. A deadline that
        // elapsed means the sound server never answered — and the ALSA path
        // talks to the same wedged stack, so re-running it just spends a second
        // full budget: a 60s clip could hold the turn for ~160s before failing.
        // Everywhere else in this tree a fallback is gated on the reason for the
        // failure rather than on its mere existence (`run_script`'s pwsh →
        // powershell chain, the clipboard candidates); this is the last capture
        // path that was not.
        if let Err(e) = run_ffmpeg(&pulse_args, deadline).await {
            if is_deadline_failure(&e) {
                return Err(DesktopError::PlatformError(format!(
                    "record_audio failed: {e}. The PulseAudio/PipeWire server did not respond \
                     within the capture deadline — ALSA was not tried because it addresses the \
                     same stack. Check that the sound server is running and that the microphone \
                     is not held by another application."
                )));
            }
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
            run_ffmpeg(&alsa_args, deadline)
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

    // ── Camera device selection ─────────────────────────────────────────────

    fn node(number: u32, index: Option<u32>) -> VideoNode {
        VideoNode { number, index }
    }

    #[test]
    fn the_capture_node_wins_over_a_lower_numbered_metadata_node() {
        // The real shape on a UVC laptop: video0 is the camera, video1 its
        // metadata node. On a machine with an IR sensor for face unlock the
        // order flips and video0 yields no picture at all — which is the case
        // that used to report "your camera is broken".
        let nodes = [node(0, Some(1)), node(1, Some(0)), node(2, Some(1))];
        assert_eq!(pick_capture_node(&nodes).as_deref(), Some("/dev/video1"));
    }

    #[test]
    fn the_lowest_capture_node_wins_when_several_qualify() {
        let nodes = [node(0, Some(0)), node(2, Some(0))];
        assert_eq!(pick_capture_node(&nodes).as_deref(), Some("/dev/video0"));
    }

    #[test]
    fn without_sysfs_the_answer_is_the_previous_behaviour() {
        // A container without /sys, or an out-of-tree driver: fall back to the
        // lowest node, which is what the hard-coded default already did. The
        // selection can improve the answer, never replace a working one.
        let nodes = [node(0, None), node(1, None)];
        assert_eq!(pick_capture_node(&nodes).as_deref(), Some("/dev/video0"));
        assert_eq!(pick_capture_node(&[]), None);
    }

    #[test]
    fn a_snapshot_discards_the_sensors_warm_up_frames() {
        // Without this the returned frame is whatever the sensor produced
        // before auto-exposure converged — usually black, and the model cannot
        // tell that from a dark room.
        let args = snap_args("/dev/video0", 4, "/tmp/out.jpg");
        let joined = args.join(" ");
        assert!(
            joined.contains(&format!("select=gte(n\\,{CAMERA_WARMUP_FRAMES})")),
            "{joined}"
        );
        // Exactly one frame is still encoded, so the capture stays a snapshot.
        let frames = args
            .iter()
            .position(|a| a == "-frames:v")
            .expect("-frames:v");
        assert_eq!(args[frames + 1], "1");
        assert_eq!(args.last().unwrap(), "/tmp/out.jpg");
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
    fn a_captures_deadline_covers_its_duration_plus_overhead() {
        // A 10s clip must not be killed at 10s: device open and container
        // finalisation happen outside the capture window.
        assert!(ffmpeg_deadline(10.0) > Duration::from_secs(10));
        assert_eq!(ffmpeg_deadline(10.0), Duration::from_secs(30));
        // A single frame still gets the overhead budget.
        assert_eq!(ffmpeg_deadline(0.0), FFMPEG_OVERHEAD);
    }

    #[test]
    fn a_nonsense_duration_still_yields_a_finite_deadline() {
        // Duration::from_secs_f64 panics on NaN/inf/negative; the guard is what
        // keeps a malformed request from taking the process down.
        assert_eq!(ffmpeg_deadline(f64::NAN), FFMPEG_OVERHEAD);
        assert_eq!(ffmpeg_deadline(f64::INFINITY), FFMPEG_OVERHEAD);
        assert_eq!(ffmpeg_deadline(-5.0), FFMPEG_OVERHEAD);
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
