//! macOS media capability: Rust-side proxy to the Swift helper over JSON-RPC.
//!
//! Camera + audio operations delegate to the Swift `AlephBridge` via the
//! shared long-lived `SwiftBridge`. Speech-to-text still uses SFSpeechRecognizer
//! via `objc2-speech` and migrates to the Swift helper in Stage 1c.

use std::sync::Arc;

use aleph_desktop::media_types::*;
use aleph_desktop::traits::MediaCapability;
use aleph_desktop::Result;
use aleph_desktop::SwiftBridge;
use async_trait::async_trait;
use tracing::debug;

/// macOS media capability. Camera + audio calls are proxied to the Swift
/// helper via the shared long-lived `SwiftBridge`; speech remains native
/// until Stage 1c.
pub struct MacOSMedia {
    bridge: Arc<SwiftBridge>,
}

impl MacOSMedia {
    pub fn new(bridge: Arc<SwiftBridge>) -> Self {
        Self { bridge }
    }
}

/// Shorthand for creating a BridgeFailed error.
fn bridge_err(msg: &str) -> aleph_desktop::DesktopError {
    aleph_desktop::DesktopError::BridgeFailed(msg.to_string())
}

// ---------------------------------------------------------------------------
// Speech-to-text via SFSpeechRecognizer
// ---------------------------------------------------------------------------

fn speech_to_text_blocking(
    audio_path: &str,
    config: &SpeechToTextConfig,
) -> Result<SpeechToTextResult> {
    use objc2::AllocAnyThread;
    use objc2_foundation::{NSLocale, NSString, NSURL};
    use objc2_speech::{
        SFSpeechRecognitionResult, SFSpeechRecognizer, SFSpeechURLRecognitionRequest,
    };
    use std::sync::mpsc;
    use std::time::Duration;

    // Verify the audio file exists
    if !std::path::Path::new(audio_path).exists() {
        return Err(aleph_desktop::DesktopError::BridgeFailed(format!(
            "Audio file not found: {audio_path}"
        )));
    }

    // Create locale and recognizer
    let locale_str = NSString::from_str(&config.language);
    let locale = NSLocale::initWithLocaleIdentifier(NSLocale::alloc(), &locale_str);

    let recognizer =
        unsafe { SFSpeechRecognizer::initWithLocale(SFSpeechRecognizer::alloc(), &locale) };
    let recognizer = recognizer.ok_or_else(|| {
        aleph_desktop::DesktopError::BridgeFailed(format!(
            "Failed to create speech recognizer for locale: {}",
            config.language
        ))
    })?;

    if !unsafe { recognizer.isAvailable() } {
        return Err(aleph_desktop::DesktopError::BridgeFailed(
            "Speech recognizer is not available".into(),
        ));
    }

    // Create URL recognition request
    let path_str = NSString::from_str(audio_path);
    let url = NSURL::fileURLWithPath(&path_str);
    let request = unsafe {
        SFSpeechURLRecognitionRequest::initWithURL(SFSpeechURLRecognitionRequest::alloc(), &url)
    };

    // Bridge the async callback with a channel
    let (tx, rx) = mpsc::channel::<std::result::Result<String, String>>();

    let handler = block2::RcBlock::new(
        move |result: *mut SFSpeechRecognitionResult, error: *mut objc2_foundation::NSError| {
            if !error.is_null() {
                let err = unsafe { &*error };
                let desc = err.localizedDescription();
                let _ = tx.send(Err(desc.to_string()));
                return;
            }
            if result.is_null() {
                return;
            }
            let result = unsafe { &*result };
            if unsafe { result.isFinal() } {
                let transcription = unsafe { result.bestTranscription() };
                let text = unsafe { transcription.formattedString() };
                let _ = tx.send(Ok(text.to_string()));
            }
        },
    );

    // Start recognition task — must stay alive until result received.
    // DO NOT use `_task` (leading underscore) — it drops immediately!
    let task = unsafe { recognizer.recognitionTaskWithRequest_resultHandler(&request, &handler) };

    // Wait for result with a 60-second timeout (Apple's speech recognition limit)
    let result = match rx.recv_timeout(Duration::from_secs(60)) {
        Ok(Ok(text)) => Ok(SpeechToTextResult {
            text,
            language: config.language.clone(),
        }),
        Ok(Err(err_msg)) => Err(aleph_desktop::DesktopError::BridgeFailed(format!(
            "Speech recognition error: {err_msg}"
        ))),
        Err(_) => Err(aleph_desktop::DesktopError::BridgeFailed(
            "Speech recognition timed out after 60 seconds".into(),
        )),
    };

    // Explicitly drop task after result is received
    drop(task);
    result
}

// ---------------------------------------------------------------------------
// MediaCapability implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl MediaCapability for MacOSMedia {
    async fn camera_snap(&self, config: CameraSnapConfig) -> Result<CameraSnapResult> {
        use aleph_protocol::desktop_bridge::methods::media::{
            SnapParams, SnapResult, METHOD_CAMERA_SNAP,
        };
        let config = config.clamped();
        debug!(quality = config.quality, "Proxying camera snap to Swift helper");
        let rpc: SnapResult = self
            .bridge
            .call(
                METHOD_CAMERA_SNAP,
                SnapParams {
                    quality: config.quality,
                },
            )
            .await
            .map_err(|e| bridge_err(&format!("media.camera.snap RPC: {e}")))?;
        Ok(CameraSnapResult {
            image_base64: rpc.image_base64,
            width: rpc.width,
            height: rpc.height,
        })
    }

    async fn camera_clip(&self, config: CameraClipConfig) -> Result<CameraClipResult> {
        use aleph_protocol::desktop_bridge::methods::media::{
            ClipParams, ClipResult, METHOD_CAMERA_CLIP,
        };
        let config = config.clamped();
        debug!(
            duration = config.duration_secs,
            audio = config.with_audio,
            "Proxying camera clip to Swift helper"
        );
        let rpc: ClipResult = self
            .bridge
            .call(
                METHOD_CAMERA_CLIP,
                ClipParams {
                    duration_secs: config.duration_secs,
                    with_audio: config.with_audio,
                },
            )
            .await
            .map_err(|e| bridge_err(&format!("media.camera.clip RPC: {e}")))?;
        Ok(CameraClipResult {
            file_path: rpc.file_path,
            duration_secs: rpc.duration_secs,
            has_audio: rpc.has_audio,
        })
    }

    async fn record_audio(&self, config: AudioRecordConfig) -> Result<AudioRecordResult> {
        use aleph_protocol::desktop_bridge::methods::media::{
            RecordAudioParams, RecordAudioResult, METHOD_AUDIO_RECORD,
        };
        let config = config.clamped();
        debug!(
            duration = config.duration_secs,
            "Proxying audio record to Swift helper"
        );
        let rpc: RecordAudioResult = self
            .bridge
            .call(
                METHOD_AUDIO_RECORD,
                RecordAudioParams {
                    duration_secs: config.duration_secs,
                },
            )
            .await
            .map_err(|e| bridge_err(&format!("media.audio.record RPC: {e}")))?;
        Ok(AudioRecordResult {
            file_path: rpc.file_path,
            duration_secs: rpc.duration_secs,
            format: rpc.format,
        })
    }

    async fn list_audio_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        use aleph_protocol::desktop_bridge::methods::media::{
            ListAudioDevicesParams, ListAudioDevicesResult, METHOD_AUDIO_LIST_DEVICES,
        };
        debug!("Proxying audio list_devices to Swift helper");
        let rpc: ListAudioDevicesResult = self
            .bridge
            .call(METHOD_AUDIO_LIST_DEVICES, ListAudioDevicesParams {})
            .await
            .map_err(|e| bridge_err(&format!("media.audio.list_devices RPC: {e}")))?;
        Ok(rpc
            .devices
            .into_iter()
            .map(|d| AudioDeviceInfo {
                uid: d.uid,
                name: d.name,
                is_input: d.is_input,
                is_default: d.is_default,
            })
            .collect())
    }

    async fn speech_to_text(
        &self,
        audio_path: &str,
        config: SpeechToTextConfig,
    ) -> Result<SpeechToTextResult> {
        debug!(path = audio_path, lang = %config.language, "Transcribing audio via SFSpeechRecognizer");
        let path = audio_path.to_string();
        tokio::task::spawn_blocking(move || speech_to_text_blocking(&path, &config))
            .await
            .map_err(|e| {
                aleph_desktop::DesktopError::BridgeFailed(format!(
                    "Failed to spawn speech-to-text task: {e}"
                ))
            })?
    }
}
