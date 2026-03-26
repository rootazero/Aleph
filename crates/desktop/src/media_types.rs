//! Types for media capture (camera, audio devices).

use serde::{Deserialize, Serialize};

/// Camera photo capture configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraSnapConfig {
    /// JPEG quality (0.0–1.0). Default: 0.9.
    pub quality: f32,
}

impl CameraSnapConfig {
    pub fn clamped(mut self) -> Self {
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
    pub fn clamped(mut self) -> Self {
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
