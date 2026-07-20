//! Types for media capture (camera, audio devices).

use serde::{Deserialize, Serialize};

/// Camera photo capture configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraSnapConfig {
    /// JPEG quality (0.0–1.0). Default: 0.9.
    pub quality: f32,
}

impl CameraSnapConfig {
    #[must_use]
    pub const fn clamped(mut self) -> Self {
        self.quality = self.quality.clamp(0.05, 1.0);
        self
    }
}

impl Default for CameraSnapConfig {
    fn default() -> Self {
        Self { quality: 0.9 }
    }
}

/// Camera photo capture result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraSnapResult {
    /// Base64-encoded JPEG image.
    pub image_base64: String,
    pub width: u32,
    pub height: u32,
}

/// Camera video recording configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraClipConfig {
    /// Duration in seconds. Clamped to [0.25, 60.0]. Default: 3.0.
    pub duration_secs: f64,
    /// Include audio from microphone. Default: false.
    pub with_audio: bool,
}

impl CameraClipConfig {
    #[must_use]
    pub const fn clamped(mut self) -> Self {
        // `f64::clamp` panics on NaN; the on-macOS path passes this
        // through to `Duration::from_secs_f64`, so guard non-finite
        // values first to avoid a runtime panic on bad JSON.
        if !self.duration_secs.is_finite() {
            self.duration_secs = 3.0;
        }
        self.duration_secs = self.duration_secs.clamp(0.25, 60.0);
        self
    }
}

impl Default for CameraClipConfig {
    fn default() -> Self {
        Self {
            duration_secs: 3.0,
            with_audio: false,
        }
    }
}

/// Camera video recording result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraClipResult {
    /// Absolute path to the recorded MP4 file.
    pub file_path: String,
    pub duration_secs: f64,
    pub has_audio: bool,
}

/// Speech-to-text configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeechToTextConfig {
    /// Recognition language (e.g., "zh-Hans", "en-US"). Default: "en-US".
    pub language: String,
}

impl Default for SpeechToTextConfig {
    fn default() -> Self {
        Self {
            language: "en-US".to_string(),
        }
    }
}

/// Speech-to-text result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeechToTextResult {
    /// Transcribed text.
    pub text: String,
    /// Language used for recognition.
    pub language: String,
}

/// Audio recording configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioRecordConfig {
    /// Recording duration in seconds. Clamped to [0.25, 300.0]. Default: 5.0.
    pub duration_secs: f64,
}

impl AudioRecordConfig {
    #[must_use]
    pub const fn clamped(mut self) -> Self {
        // `f64::clamp` panics on NaN; the on-macOS path passes this
        // through to `Duration::from_secs_f64`, so guard non-finite
        // values first to avoid a runtime panic on bad JSON.
        if !self.duration_secs.is_finite() {
            self.duration_secs = 5.0;
        }
        self.duration_secs = self.duration_secs.clamp(0.25, 300.0);
        self
    }
}

impl Default for AudioRecordConfig {
    fn default() -> Self {
        Self { duration_secs: 5.0 }
    }
}

/// Audio recording result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioRecordResult {
    /// Absolute path to the recorded audio file.
    pub file_path: String,
    /// Actual recording duration in seconds.
    pub duration_secs: f64,
    /// Audio format (e.g., "m4a").
    pub format: String,
}

/// Information about an audio input device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDeviceInfo {
    /// Unique device identifier.
    pub uid: String,
    /// Human-readable device name.
    pub name: String,
    /// Whether this is an input device.
    pub is_input: bool,
    /// Whether this is the current default input device.
    pub is_default: bool,
}
