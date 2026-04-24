//! Media camera capture over JSON-RPC.
//!
//! Audio and speech recognition live alongside camera in `MediaCapability`
//! on the Rust side but are migrated in Stages 1b and 1c separately.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const METHOD_CAMERA_SNAP: &str = "media.camera.snap";
pub const METHOD_CAMERA_CLIP: &str = "media.camera.clip";

/// Snap is essentially synchronous (<1s on a warm camera) but the AVCapture
/// pipeline takes time to initialise on first use.
pub const SUGGESTED_TIMEOUT_MS_SNAP: u64 = 10_000;

/// Clip timeout is duration + warm-up headroom. Callers should compute
/// `duration_secs * 1000 + CLIP_OVERHEAD_MS` rather than relying on a fixed
/// constant, so we expose the overhead directly.
pub const CLIP_OVERHEAD_MS: u64 = 5_000;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnapParams {
    /// JPEG quality, 0.0 – 1.0. Caller is expected to clamp; helper rejects
    /// values outside [0.05, 1.0] with ERR_INVALID_PARAMS.
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
    fn method_names_stable() {
        assert_eq!(METHOD_CAMERA_SNAP, "media.camera.snap");
        assert_eq!(METHOD_CAMERA_CLIP, "media.camera.clip");
    }
}
