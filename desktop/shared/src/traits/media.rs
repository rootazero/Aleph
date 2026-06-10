//! Media capture capability (camera, audio devices).

use async_trait::async_trait;

use crate::media_types::{CameraSnapConfig, CameraSnapResult, CameraClipConfig, CameraClipResult, AudioDeviceInfo, AudioRecordConfig, AudioRecordResult, SpeechToTextConfig, SpeechToTextResult};
use crate::Result;

/// Camera capture and audio device management.
#[async_trait]
pub trait MediaCapability: Send + Sync {
    /// Capture a photo from the default camera as JPEG.
    async fn camera_snap(&self, config: CameraSnapConfig) -> Result<CameraSnapResult> {
        let _ = config;
        Err(crate::DesktopError::NotImplemented(
            "camera snap not available on this platform".into(),
        ))
    }

    /// Record video from the default camera as MP4.
    async fn camera_clip(&self, config: CameraClipConfig) -> Result<CameraClipResult> {
        let _ = config;
        Err(crate::DesktopError::NotImplemented(
            "camera clip not available on this platform".into(),
        ))
    }

    /// List audio input devices.
    async fn list_audio_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        Err(crate::DesktopError::NotImplemented(
            "audio device listing not available on this platform".into(),
        ))
    }

    /// Record audio from the default microphone.
    async fn record_audio(&self, config: AudioRecordConfig) -> Result<AudioRecordResult> {
        let _ = config;
        Err(crate::DesktopError::NotImplemented(
            "audio recording not available on this platform".into(),
        ))
    }

    /// Begin an open-ended push-to-talk recording (stop via [`Self::record_audio_stop`]).
    ///
    /// Powers the Panel mic button on platforms whose webview cannot reach the
    /// microphone via `getUserMedia` (unsigned macOS `WKWebView`). The default
    /// `NotImplemented` is the signal callers use to fall back to browser
    /// capture on platforms without a native helper (Windows/Linux).
    async fn record_audio_start(&self) -> Result<()> {
        Err(crate::DesktopError::NotImplemented(
            "native audio recording not available on this platform".into(),
        ))
    }

    /// Stop the active push-to-talk recording and return the captured file.
    async fn record_audio_stop(&self) -> Result<AudioRecordResult> {
        Err(crate::DesktopError::NotImplemented(
            "native audio recording not available on this platform".into(),
        ))
    }

    /// Transcribe an audio file to text using on-device speech recognition.
    async fn speech_to_text(
        &self,
        audio_path: &str,
        config: SpeechToTextConfig,
    ) -> Result<SpeechToTextResult> {
        let _ = (audio_path, config);
        Err(crate::DesktopError::NotImplemented(
            "speech to text not available on this platform".into(),
        ))
    }

    /// Poll the default microphone for its current EMA-smoothed RMS level.
    ///
    /// Returns [`MicMeterSample`] which carries the level (when the underlying
    /// session is running) and a textual reason when it can't be started
    /// (missing permission, no input device, etc.). Designed for use by a
    /// long-running polling daemon (`tasks::mic_level`) — the implementation is
    /// expected to keep an audio tap warm across calls, not to spin one up per
    /// poll.
    ///
    /// Default impl returns `NotImplemented` for non-macOS platforms.
    async fn mic_level(&self) -> Result<MicMeterSample> {
        Err(crate::DesktopError::NotImplemented(
            "mic_level not available on this platform".into(),
        ))
    }
}

/// One mic-meter sample as seen by the core (mirror of `MicMeterResult` on
/// the wire — kept separate so the trait does not leak `aleph_protocol` types
/// into platforms that never speak the bridge).
#[derive(Debug, Clone, PartialEq)]
pub struct MicMeterSample {
    /// EMA-smoothed RMS level, range 0.0..=1.0. `None` if the meter session
    /// could not start.
    pub level: Option<f32>,
    /// `true` iff the meter session is currently running.
    pub active: bool,
    /// Human-readable reason the session is not active, when `active` is
    /// false (e.g. "permission denied", "no input device").
    pub reason: Option<String>,
}

impl MicMeterSample {
    pub fn inactive(reason: impl Into<String>) -> Self {
        Self {
            level: None,
            active: false,
            reason: Some(reason.into()),
        }
    }
}
