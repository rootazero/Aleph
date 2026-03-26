//! Media capture capability (camera, audio devices).

use async_trait::async_trait;

use crate::media_types::*;
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
}
