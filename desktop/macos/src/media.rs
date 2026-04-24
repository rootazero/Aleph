//! macOS media capabilities.
//!
//! Camera operations proxy to the Swift helper over JSON-RPC
//! (`media.camera.snap` / `media.camera.clip`). Audio recording uses the
//! `ffmpeg` CLI (audio-only capture via AVFoundation is complex and will
//! migrate in Stage 1b). Audio device listing uses CoreAudio C FFI directly.
//! Speech-to-text uses SFSpeechRecognizer via `objc2-speech` and will migrate
//! to the Swift helper in Stage 1c.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use aleph_desktop::media_types::*;
use aleph_desktop::traits::MediaCapability;
use aleph_desktop::Result;
use aleph_desktop::SwiftBridge;
use async_trait::async_trait;
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// CoreAudio C FFI for audio device listing
// ---------------------------------------------------------------------------

#[allow(non_upper_case_globals)]
mod core_audio_ffi {
    use std::os::raw::c_void;

    pub type AudioObjectID = u32;
    pub type AudioDeviceID = AudioObjectID;
    pub type OSStatus = i32;

    #[repr(C)]
    pub struct AudioObjectPropertyAddress {
        pub selector: u32,
        pub scope: u32,
        pub element: u32,
    }

    // Property selectors
    pub const kAudioHardwarePropertyDevices: u32 = u32::from_be_bytes(*b"dev#");
    pub const kAudioHardwarePropertyDefaultInputDevice: u32 = u32::from_be_bytes(*b"dIn ");
    pub const kAudioObjectPropertyName: u32 = u32::from_be_bytes(*b"lnam");
    pub const kAudioDevicePropertyDeviceUID: u32 = u32::from_be_bytes(*b"uid ");
    pub const kAudioDevicePropertyStreamConfiguration: u32 = u32::from_be_bytes(*b"slay");

    // Scopes
    pub const kAudioObjectPropertyScopeGlobal: u32 = u32::from_be_bytes(*b"glob");
    pub const kAudioDevicePropertyScopeInput: u32 = u32::from_be_bytes(*b"inpt");

    // Elements
    pub const kAudioObjectPropertyElementMain: u32 = 0;

    // System object
    pub const kAudioObjectSystemObject: AudioObjectID = 1;

    // AudioBufferList for stream configuration
    #[repr(C)]
    pub struct AudioBuffer {
        pub number_channels: u32,
        pub data_byte_size: u32,
        pub data: *mut c_void,
    }

    #[repr(C)]
    pub struct AudioBufferList {
        pub number_buffers: u32,
        // Followed by variable-length array of AudioBuffer
        pub buffers: [AudioBuffer; 1],
    }

    extern "C" {
        pub fn AudioObjectGetPropertyDataSize(
            object_id: AudioObjectID,
            address: *const AudioObjectPropertyAddress,
            qualifier_data_size: u32,
            qualifier_data: *const c_void,
            out_data_size: *mut u32,
        ) -> OSStatus;

        pub fn AudioObjectGetPropertyData(
            object_id: AudioObjectID,
            address: *const AudioObjectPropertyAddress,
            qualifier_data_size: u32,
            qualifier_data: *const c_void,
            io_data_size: *mut u32,
            out_data: *mut c_void,
        ) -> OSStatus;
    }
}

/// macOS media capability. Camera calls are proxied to the Swift helper via
/// the shared long-lived `SwiftBridge`; audio + speech remain native for now.
pub struct MacOSMedia {
    bridge: Arc<SwiftBridge>,
}

impl MacOSMedia {
    pub fn new(bridge: Arc<SwiftBridge>) -> Self {
        Self { bridge }
    }
}

// ---------------------------------------------------------------------------
// Helper: media output directory
// ---------------------------------------------------------------------------

fn media_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    let dir = home.join(".aleph/data/_media");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn timestamp_suffix() -> String {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

/// Shorthand for creating a BridgeFailed error.
fn bridge_err(msg: &str) -> aleph_desktop::DesktopError {
    aleph_desktop::DesktopError::BridgeFailed(msg.to_string())
}

// ---------------------------------------------------------------------------
// Helper: check if a CLI tool is available
// ---------------------------------------------------------------------------

async fn check_tool_available(tool: &str) -> bool {
    tokio::process::Command::new("which")
        .arg(tool)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Audio recording via ffmpeg CLI
// ---------------------------------------------------------------------------

async fn record_audio_with_ffmpeg(config: &AudioRecordConfig) -> Result<AudioRecordResult> {
    if !check_tool_available("ffmpeg").await {
        return Err(aleph_desktop::DesktopError::BridgeFailed(
            "ffmpeg is not installed. Install it with: brew install ffmpeg".into(),
        ));
    }

    let out_path = media_dir().join(format!("audio_record_{}.m4a", timestamp_suffix()));
    let duration_str = format!("{:.1}", config.duration_secs);

    // Record from default microphone using avfoundation
    // :0 = default audio input device (no video)
    let out_path_str = out_path.to_string_lossy().into_owned();
    let args = [
        "-f",
        "avfoundation",
        "-i",
        ":0",
        "-t",
        &duration_str,
        "-y",
        &out_path_str,
    ];

    debug!(
        duration = config.duration_secs,
        "Recording audio via ffmpeg"
    );

    let output = tokio::process::Command::new("ffmpeg")
        .args(args)
        .output()
        .await
        .map_err(|e| {
            aleph_desktop::DesktopError::BridgeFailed(format!("Failed to run ffmpeg: {e}"))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // ffmpeg writes progress info to stderr; only treat as error if file doesn't exist
        if !out_path.exists() {
            return Err(aleph_desktop::DesktopError::BridgeFailed(format!(
                "ffmpeg audio recording failed: {stderr}"
            )));
        }
    }

    Ok(AudioRecordResult {
        file_path: out_path.to_string_lossy().into_owned(),
        duration_secs: config.duration_secs,
        format: "m4a".to_string(),
    })
}

// ---------------------------------------------------------------------------
// Audio device listing via CoreAudio FFI
// ---------------------------------------------------------------------------

fn list_audio_devices_ffi() -> Result<Vec<AudioDeviceInfo>> {
    use core_audio_ffi::*;
    use std::os::raw::c_void;

    // Get default input device
    let default_input: AudioDeviceID = {
        let address = AudioObjectPropertyAddress {
            selector: kAudioHardwarePropertyDefaultInputDevice,
            scope: kAudioObjectPropertyScopeGlobal,
            element: kAudioObjectPropertyElementMain,
        };
        let mut device_id: AudioDeviceID = 0;
        let mut size = std::mem::size_of::<AudioDeviceID>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                kAudioObjectSystemObject,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                &mut device_id as *mut _ as *mut c_void,
            )
        };
        if status != 0 {
            warn!(status, "Failed to get default input device");
            0
        } else {
            device_id
        }
    };

    // Get all audio devices
    let address = AudioObjectPropertyAddress {
        selector: kAudioHardwarePropertyDevices,
        scope: kAudioObjectPropertyScopeGlobal,
        element: kAudioObjectPropertyElementMain,
    };

    let mut data_size: u32 = 0;
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            kAudioObjectSystemObject,
            &address,
            0,
            std::ptr::null(),
            &mut data_size,
        )
    };
    if status != 0 {
        return Err(aleph_desktop::DesktopError::BridgeFailed(format!(
            "AudioObjectGetPropertyDataSize failed: {status}"
        )));
    }

    let device_count = data_size as usize / std::mem::size_of::<AudioDeviceID>();
    let mut device_ids = vec![0u32; device_count];
    let status = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject,
            &address,
            0,
            std::ptr::null(),
            &mut data_size,
            device_ids.as_mut_ptr() as *mut c_void,
        )
    };
    if status != 0 {
        return Err(aleph_desktop::DesktopError::BridgeFailed(format!(
            "AudioObjectGetPropertyData (devices) failed: {status}"
        )));
    }

    let mut devices = Vec::new();

    for &device_id in &device_ids {
        // Check if device has input channels
        let input_channels = get_input_channel_count(device_id);
        if input_channels == 0 {
            continue; // Skip non-input devices
        }

        let name = get_device_string_property(device_id, kAudioObjectPropertyName)
            .unwrap_or_else(|| format!("Device {device_id}"));
        let uid = get_device_string_property(device_id, kAudioDevicePropertyDeviceUID)
            .unwrap_or_else(|| format!("{device_id}"));

        devices.push(AudioDeviceInfo {
            uid,
            name,
            is_input: true,
            is_default: device_id == default_input,
        });
    }

    Ok(devices)
}

fn get_device_string_property(
    device_id: core_audio_ffi::AudioDeviceID,
    selector: u32,
) -> Option<String> {
    use core_audio_ffi::*;

    let address = AudioObjectPropertyAddress {
        selector,
        scope: kAudioObjectPropertyScopeGlobal,
        element: kAudioObjectPropertyElementMain,
    };

    // CFStringRef is a pointer-sized value
    let mut cf_string: core_foundation::base::CFTypeRef = std::ptr::null();
    let mut size = std::mem::size_of::<core_foundation::base::CFTypeRef>() as u32;

    let status = unsafe {
        AudioObjectGetPropertyData(
            device_id,
            &address,
            0,
            std::ptr::null(),
            &mut size,
            &mut cf_string as *mut _ as *mut std::os::raw::c_void,
        )
    };

    if status != 0 || cf_string.is_null() {
        return None;
    }

    // Convert CFStringRef to Rust String
    let cf_str = unsafe {
        use core_foundation::base::TCFType;
        core_foundation::string::CFString::wrap_under_create_rule(
            cf_string as core_foundation::string::CFStringRef,
        )
    };
    Some(cf_str.to_string())
}

fn get_input_channel_count(device_id: core_audio_ffi::AudioDeviceID) -> u32 {
    use core_audio_ffi::*;

    let address = AudioObjectPropertyAddress {
        selector: kAudioDevicePropertyStreamConfiguration,
        scope: kAudioDevicePropertyScopeInput,
        element: kAudioObjectPropertyElementMain,
    };

    let mut data_size: u32 = 0;
    let status = unsafe {
        AudioObjectGetPropertyDataSize(device_id, &address, 0, std::ptr::null(), &mut data_size)
    };
    if status != 0 || data_size == 0 {
        return 0;
    }

    // Allocate buffer for AudioBufferList
    let mut buffer = vec![0u8; data_size as usize];
    let status = unsafe {
        AudioObjectGetPropertyData(
            device_id,
            &address,
            0,
            std::ptr::null(),
            &mut data_size,
            buffer.as_mut_ptr() as *mut std::os::raw::c_void,
        )
    };
    if status != 0 {
        return 0;
    }

    // Parse AudioBufferList to count input channels
    let buffer_list = unsafe { &*(buffer.as_ptr() as *const AudioBufferList) };
    let mut total_channels = 0u32;

    if buffer_list.number_buffers > 0 {
        let buffers = unsafe {
            std::slice::from_raw_parts(
                &buffer_list.buffers[0] as *const AudioBuffer,
                buffer_list.number_buffers as usize,
            )
        };
        for buf in buffers {
            total_channels += buf.number_channels;
        }
    }

    total_channels
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
        let config = config.clamped();
        debug!(
            duration = config.duration_secs,
            "Recording audio via ffmpeg"
        );
        record_audio_with_ffmpeg(&config).await
    }

    async fn list_audio_devices(&self) -> Result<Vec<AudioDeviceInfo>> {
        debug!("Listing audio input devices via CoreAudio FFI");
        // Run FFI on a blocking thread to avoid potential issues
        tokio::task::spawn_blocking(list_audio_devices_ffi)
            .await
            .map_err(|e| {
                aleph_desktop::DesktopError::BridgeFailed(format!(
                    "Failed to spawn blocking task: {e}"
                ))
            })?
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
