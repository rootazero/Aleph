//! Media capture capability (camera, audio devices).

use async_trait::async_trait;

use crate::media_types::{
    AudioDeviceInfo, AudioRecordConfig, AudioRecordResult, CameraClipConfig, CameraClipResult,
    CameraSnapConfig, CameraSnapResult, SpeechToTextConfig, SpeechToTextResult,
    VoiceCaptureObservation,
};
use crate::{CancellationToken, Result};

/// Owns one native microphone capture. Keep it in a task that survives outer
/// cancellation so it can await source teardown; dropping is not release proof.
#[async_trait]
pub trait VoiceCapture: Send {
    /// Read the next contiguous frame levels after the absolute sample cursor.
    async fn poll(
        &mut self,
        after_sample: u32,
        cancel: &CancellationToken,
    ) -> Result<Vec<VoiceCaptureObservation>>;

    /// Stop capture and return only the requested half-open range as WAV.
    async fn seal(
        self: Box<Self>,
        first_sample: u32,
        end_sample: u32,
        cancel: &CancellationToken,
    ) -> Result<Vec<u8>>;

    /// Stop capture and discard its audio, independent of caller cancellation.
    async fn cancel(self: Box<Self>) -> Result<()>;
}

/// Camera capture and audio device management.
#[async_trait]
pub trait MediaCapability: Send + Sync {
    /// Static platform support, not microphone permission or device readiness.
    fn supports_voice_capture(&self) -> bool {
        false
    }

    async fn voice_capture_begin(
        &self,
        cancel: CancellationToken,
    ) -> Result<Box<dyn VoiceCapture>> {
        let _ = cancel;
        Err(crate::DesktopError::NotImplemented(
            "native voice capture not available on this platform".into(),
        ))
    }

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

    /// Recognize with the installed on-device engine only; never fall back to a
    /// network recognizer. `language` absent means the OS locale. The caller
    /// owns `audio_path` and deletes it once this returns, so implementations
    /// must stop the engine and release the file before returning, including
    /// after `cancel` fires. `NotImplemented` means no offline engine.
    async fn speech_to_text_offline(
        &self,
        audio_path: &str,
        language: Option<&str>,
        cancel: crate::CancellationToken,
    ) -> Result<SpeechToTextResult> {
        let _ = (audio_path, language, cancel);
        Err(crate::DesktopError::NotImplemented(
            "offline speech recognition not available on this platform".into(),
        ))
    }

    // `mic_level()` and its `MicMeterSample` were removed on 2026-08-09 along
    // with their only caller, the `tasks::mic_level` reporter. The macOS limb
    // behind it kept a long-lived `AVAudioEngine` tap warm in the helper — a
    // capability with a running cost and no consumer, which is the shape R10
    // says to retract rather than keep warm for a future one. Audio capture is
    // unaffected: `record_audio` / `record_audio_start` / `record_audio_stop`
    // are the live paths.
}
