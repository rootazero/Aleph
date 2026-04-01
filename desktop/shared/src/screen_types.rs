//! Types for screen recording.

use serde::{Deserialize, Serialize};

use crate::ScreenRegion;

/// Screen recording configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenRecordConfig {
    /// Recording duration in seconds. Clamped to [0.25, 60.0].
    pub duration_secs: f64,
    /// Frames per second. Clamped to [1, 60]. Default: 30.
    pub fps: u32,
    /// Whether to include system audio.
    pub with_audio: bool,
    /// Optional region to record. None = full primary display.
    pub region: Option<ScreenRegion>,
}

impl ScreenRecordConfig {
    /// Apply parameter constraints.
    pub fn clamped(mut self) -> Self {
        self.duration_secs = self.duration_secs.clamp(0.25, 60.0);
        self.fps = self.fps.clamp(1, 60);
        self
    }
}

impl Default for ScreenRecordConfig {
    fn default() -> Self {
        Self {
            duration_secs: 5.0,
            fps: 30,
            with_audio: false,
            region: None,
        }
    }
}

/// Screen recording result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenRecordResult {
    /// Absolute path to the recorded MP4 file.
    pub file_path: String,
    /// Actual recording duration in seconds.
    pub duration_secs: f64,
    /// Whether audio was captured.
    pub has_audio: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_clamp() {
        let config = ScreenRecordConfig {
            duration_secs: 100.0,
            fps: 120,
            with_audio: false,
            region: None,
        }
        .clamped();
        assert_eq!(config.duration_secs, 60.0);
        assert_eq!(config.fps, 60);
    }

    #[test]
    fn test_config_clamp_min() {
        let config = ScreenRecordConfig {
            duration_secs: 0.01,
            fps: 0,
            with_audio: false,
            region: None,
        }
        .clamped();
        assert_eq!(config.duration_secs, 0.25);
        assert_eq!(config.fps, 1);
    }

    #[test]
    fn test_config_default() {
        let config = ScreenRecordConfig::default();
        assert_eq!(config.duration_secs, 5.0);
        assert_eq!(config.fps, 30);
        assert!(!config.with_audio);
        assert!(config.region.is_none());
    }
}
