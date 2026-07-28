//! Media camera + audio capture + speech recognition over JSON-RPC.
//!
//! All three media sub-domains proxy to the Swift `AlephBridge` helper over
//! the shared long-lived `SwiftBridge`. Rust-side native media code is empty
//! as of Stage 1c.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const METHOD_CAMERA_SNAP: &str = "media.camera.snap";
pub const METHOD_CAMERA_CLIP: &str = "media.camera.clip";

pub const METHOD_AUDIO_LIST_DEVICES: &str = "media.audio.list_devices";
pub const METHOD_AUDIO_RECORD: &str = "media.audio.record";
/// Open-ended push-to-talk: start recording now, stop on a later call. Used by
/// the Panel mic button (the `WKWebView` `getUserMedia` path is blocked on
/// unsigned macOS builds, so capture happens natively in the helper instead).
pub const METHOD_AUDIO_RECORD_START: &str = "media.audio.record_start";
pub const METHOD_AUDIO_RECORD_STOP: &str = "media.audio.record_stop";
pub const METHOD_AUDIO_MIC_METER: &str = "media.audio.mic_meter";

pub const METHOD_SPEECH_TRANSCRIBE_FILE: &str = "media.speech.transcribe_file";

// ── Deadlines ────────────────────────────────────────────────────────────────

/// Deadline for the media calls whose length this table can predict.
///
/// The three that it cannot — `media.camera.clip`, `media.audio.record` and
/// `media.speech.transcribe_file` — run for as long as the *caller* asked them
/// to, so their limbs pass an explicitly computed deadline (requested duration
/// plus warm-up headroom) rather than reading anything from here. This default
/// only ever applies to them if that computation is ever dropped, and one minute
/// is the same catch-all they would have fallen back to before.
pub const DEFAULT_TIMEOUT_MS: u64 = 60_000;

/// Snap is essentially synchronous (<1s on a warm camera) but the `AVCapture`
/// pipeline takes time to initialise on first use.
pub const TIMEOUT_MS_SNAP: u64 = 10_000;

/// Device enumeration is fast but `CoreAudio` can stall.
pub const TIMEOUT_MS_LIST_DEVICES: u64 = 5_000;

/// Mic-meter poll is a single atomic read of the running session's EMA — fast,
/// but on first call the helper has to install an `AVAudioEngine` tap which
/// takes a few hundred ms.
pub const TIMEOUT_MS_MIC_METER: u64 = 2_000;

/// Stopping an open-ended recording finalises the encoder before replying.
pub const TIMEOUT_MS_RECORD_STOP: u64 = 15_000;

pub const TIMEOUT_OVERRIDES_MS: &[(&str, u64)] = &[
    (METHOD_CAMERA_SNAP, TIMEOUT_MS_SNAP),
    (METHOD_AUDIO_LIST_DEVICES, TIMEOUT_MS_LIST_DEVICES),
    (METHOD_AUDIO_MIC_METER, TIMEOUT_MS_MIC_METER),
    (METHOD_AUDIO_RECORD_STOP, TIMEOUT_MS_RECORD_STOP),
];

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnapParams {
    /// JPEG quality, 0.0 – 1.0. Caller is expected to clamp; helper rejects
    /// values outside [0.05, 1.0] with `ERR_INVALID_PARAMS`.
    pub quality: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnapResult {
    /// Base64-encoded JPEG.
    pub image_base64: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ClipParams {
    /// Duration in seconds. Caller clamps to [0.25, 60.0].
    pub duration_secs: f64,
    /// Mix microphone audio into the clip.
    pub with_audio: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ClipResult {
    /// Absolute path to the recorded MP4/MOV.
    pub file_path: String,
    pub duration_secs: f64,
    pub has_audio: bool,
}

/// List audio devices request. No parameters; sent as an empty struct for
/// schema consistency with the rest of the RPC surface.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListAudioDevicesParams {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AudioDeviceInfo {
    /// Unique device identifier (`CoreAudio` UID).
    pub uid: String,
    /// Human-readable device name.
    pub name: String,
    /// Whether this is an input device.
    pub is_input: bool,
    /// Whether this is the current default input device.
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListAudioDevicesResult {
    /// List of available audio devices.
    pub devices: Vec<AudioDeviceInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RecordAudioParams {
    /// Duration in seconds. Caller clamps to [0.25, 300.0].
    pub duration_secs: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RecordAudioResult {
    /// Absolute path to the recorded audio file.
    pub file_path: String,
    /// Actual recording duration in seconds.
    pub duration_secs: f64,
    /// Audio format (e.g., "m4a").
    pub format: String,
}

/// Start a push-to-talk recording. No parameters; the helper holds the active
/// recorder until [`METHOD_AUDIO_RECORD_STOP`]. Sent as an empty object.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct RecordStartParams {}

/// Acknowledgement that recording began. No payload.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct RecordStartResult {}

/// Stop the active push-to-talk recording. No parameters. The result mirrors
/// [`RecordAudioResult`] (file path + actual elapsed duration + format).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct RecordStopParams {}

/// Mic meter poll request. No parameters; first call lazily starts a long-lived
/// `AVAudioEngine` tap on the default input device. The helper auto-shuts the
/// tap down after [`MIC_METER_IDLE_TIMEOUT_SECS`] of no polls.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct MicMeterParams {}

/// Idle timeout after which the helper tears down the mic meter session.
pub const MIC_METER_IDLE_TIMEOUT_SECS: u64 = 60;

/// One mic-meter sample.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MicMeterResult {
    /// EMA-smoothed RMS level, range 0.0..=1.0. `None` when the session
    /// couldn't start (no input device or permission denied).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<f32>,
    /// `true` once the underlying `AVAudioEngine` tap is running and
    /// producing samples; `false` if the helper had to refuse (no perm /
    /// no device).
    pub active: bool,
    /// Reason the helper couldn't start, when `active` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TranscribeFileParams {
    /// Absolute path to the audio file (m4a / wav / mp3 supported by `SFSpeechRecognizer`).
    pub audio_path: String,
    /// BCP-47 language tag (e.g. "en-US", "zh-Hans"). Helper rejects unknown
    /// locales with `ERR_INVALID_PARAMS`.
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TranscribeFileResult {
    /// Best transcription, formatted (punctuation + capitalization).
    pub text: String,
    /// Echo of the requested language.
    pub language: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_roundtrip() {
        let p = SnapParams { quality: 0.85 };
        let j = serde_json::to_string(&p).unwrap();
        let back: SnapParams = serde_json::from_str(&j).unwrap();
        assert!((back.quality - 0.85).abs() < 1e-6);
    }

    #[test]
    fn snap_result_roundtrip() {
        let r = SnapResult {
            image_base64: "abc==".into(),
            width: 1920,
            height: 1080,
        };
        let j = serde_json::to_string(&r).unwrap();
        let back: SnapResult = serde_json::from_str(&j).unwrap();
        assert_eq!(back.width, 1920);
        assert_eq!(back.height, 1080);
        assert_eq!(back.image_base64, "abc==");
    }

    #[test]
    fn clip_roundtrip() {
        let p = ClipParams {
            duration_secs: 3.5,
            with_audio: true,
        };
        let j = serde_json::to_string(&p).unwrap();
        let back: ClipParams = serde_json::from_str(&j).unwrap();
        assert!((back.duration_secs - 3.5).abs() < 1e-6);
        assert!(back.with_audio);
    }

    #[test]
    fn clip_result_roundtrip() {
        let r = ClipResult {
            file_path: "/tmp/x.mp4".into(),
            duration_secs: 3.5,
            has_audio: true,
        };
        let j = serde_json::to_string(&r).unwrap();
        let back: ClipResult = serde_json::from_str(&j).unwrap();
        assert_eq!(back.file_path, "/tmp/x.mp4");
        assert!(back.has_audio);
    }

    #[test]
    fn list_audio_devices_params_roundtrip() {
        let p = ListAudioDevicesParams {};
        let j = serde_json::to_string(&p).unwrap();
        assert_eq!(j, "{}");
        let back: ListAudioDevicesParams = serde_json::from_str(&j).unwrap();
        let _verify = back; // Just verify it deserializes
    }

    #[test]
    fn audio_device_info_roundtrip() {
        let d = AudioDeviceInfo {
            uid: "xyz".into(),
            name: "MacBook Pro Microphone".into(),
            is_input: true,
            is_default: true,
        };
        let j = serde_json::to_string(&d).unwrap();
        let back: AudioDeviceInfo = serde_json::from_str(&j).unwrap();
        assert_eq!(back.uid, "xyz");
        assert_eq!(back.name, "MacBook Pro Microphone");
        assert!(back.is_input);
        assert!(back.is_default);
    }

    #[test]
    fn list_audio_devices_result_roundtrip() {
        let device = AudioDeviceInfo {
            uid: "device-123".into(),
            name: "Built-in Microphone".into(),
            is_input: true,
            is_default: true,
        };
        let r = ListAudioDevicesResult {
            devices: vec![device],
        };
        let j = serde_json::to_string(&r).unwrap();
        let back: ListAudioDevicesResult = serde_json::from_str(&j).unwrap();
        assert_eq!(back.devices.len(), 1);
        assert_eq!(back.devices[0].uid, "device-123");
    }

    #[test]
    fn record_audio_params_roundtrip() {
        let p = RecordAudioParams { duration_secs: 5.0 };
        let j = serde_json::to_string(&p).unwrap();
        let back: RecordAudioParams = serde_json::from_str(&j).unwrap();
        assert!((back.duration_secs - 5.0).abs() < 1e-6);
    }

    #[test]
    fn record_audio_result_roundtrip() {
        let r = RecordAudioResult {
            file_path: "/tmp/a.m4a".into(),
            duration_secs: 5.0,
            format: "m4a".into(),
        };
        let j = serde_json::to_string(&r).unwrap();
        let back: RecordAudioResult = serde_json::from_str(&j).unwrap();
        assert_eq!(back.file_path, "/tmp/a.m4a");
        assert!((back.duration_secs - 5.0).abs() < 1e-6);
        assert_eq!(back.format, "m4a");
    }

    #[test]
    fn transcribe_params_roundtrip() {
        let p = TranscribeFileParams {
            audio_path: "/tmp/a.m4a".into(),
            language: "en-US".into(),
        };
        let j = serde_json::to_string(&p).unwrap();
        let back: TranscribeFileParams = serde_json::from_str(&j).unwrap();
        assert_eq!(back.audio_path, "/tmp/a.m4a");
        assert_eq!(back.language, "en-US");
    }

    #[test]
    fn transcribe_result_roundtrip() {
        let r = TranscribeFileResult {
            text: "hello world".into(),
            language: "en-US".into(),
        };
        let j = serde_json::to_string(&r).unwrap();
        let back: TranscribeFileResult = serde_json::from_str(&j).unwrap();
        assert_eq!(back.text, "hello world");
        assert_eq!(back.language, "en-US");
    }

    #[test]
    fn method_names_stable() {
        assert_eq!(METHOD_CAMERA_SNAP, "media.camera.snap");
        assert_eq!(METHOD_CAMERA_CLIP, "media.camera.clip");
        assert_eq!(METHOD_AUDIO_LIST_DEVICES, "media.audio.list_devices");
        assert_eq!(METHOD_AUDIO_RECORD, "media.audio.record");
        assert_eq!(METHOD_AUDIO_MIC_METER, "media.audio.mic_meter");
        assert_eq!(
            METHOD_SPEECH_TRANSCRIBE_FILE,
            "media.speech.transcribe_file"
        );
    }

    #[test]
    fn mic_meter_result_with_level_roundtrip() {
        let r = MicMeterResult {
            level: Some(0.42),
            active: true,
            reason: None,
        };
        let j = serde_json::to_string(&r).unwrap();
        let back: MicMeterResult = serde_json::from_str(&j).unwrap();
        assert_eq!(back.level, Some(0.42));
        assert!(back.active);
        assert!(back.reason.is_none());
    }

    #[test]
    fn mic_meter_result_without_level_skips_field() {
        let r = MicMeterResult {
            level: None,
            active: false,
            reason: Some("permission denied".into()),
        };
        let j = serde_json::to_string(&r).unwrap();
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        // level is `None` → skipped from the wire envelope
        assert!(v.get("level").is_none());
        assert_eq!(v["active"], false);
        assert_eq!(v["reason"], "permission denied");
    }

    #[test]
    fn mic_meter_params_serializes_as_empty_object() {
        let p = MicMeterParams {};
        let j = serde_json::to_string(&p).unwrap();
        assert_eq!(j, "{}");
    }
}
