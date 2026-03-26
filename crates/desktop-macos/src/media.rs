//! macOS media capture (camera photo/video, audio device listing).
//!
//! Camera operations use CLI tools for reliability:
//! - Photos: `imagesnap` (brew install imagesnap) — captures JPEG from default camera
//! - Video: `ffmpeg` — records MP4 from default camera with optional audio
//!
//! Audio device listing uses CoreAudio C FFI directly.

use std::path::PathBuf;
use std::time::SystemTime;

use aleph_desktop::media_types::*;
use aleph_desktop::traits::MediaCapability;
use aleph_desktop::Result;
use async_trait::async_trait;
use base64::Engine;
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

/// macOS media capability using CLI tools + CoreAudio FFI.
pub struct MacOSMedia {
    _private: (),
}

impl MacOSMedia {
    pub fn new() -> Self {
        Self { _private: () }
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

// ---------------------------------------------------------------------------
// Camera snap via imagesnap CLI
// ---------------------------------------------------------------------------

async fn snap_with_imagesnap() -> Result<CameraSnapResult> {
    let out_path = media_dir().join(format!("camera_snap_{}.jpg", timestamp_suffix()));

    // imagesnap -w 1.0 captures with a 1-second warmup for auto-exposure
    let output = tokio::process::Command::new("imagesnap")
        .args(["-w", "1.0", out_path.to_string_lossy().as_ref()])
        .output()
        .await
        .map_err(|e| {
            aleph_desktop::DesktopError::BridgeFailed(format!(
                "Failed to run imagesnap (install: brew install imagesnap): {e}"
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(aleph_desktop::DesktopError::BridgeFailed(format!(
            "imagesnap failed: {stderr}"
        )));
    }

    // Read file and encode to base64
    let data = tokio::fs::read(&out_path).await.map_err(|e| {
        aleph_desktop::DesktopError::BridgeFailed(format!(
            "Failed to read captured photo: {e}"
        ))
    })?;

    // Get image dimensions
    let (width, height) = match image::image_dimensions(&out_path) {
        Ok((w, h)) => (w, h),
        Err(_) => (0, 0),
    };

    let image_base64 = base64::engine::general_purpose::STANDARD.encode(&data);

    // Clean up temp file
    let _ = tokio::fs::remove_file(&out_path).await;

    Ok(CameraSnapResult {
        image_base64,
        width,
        height,
    })
}

// ---------------------------------------------------------------------------
// Camera clip via ffmpeg CLI
// ---------------------------------------------------------------------------

async fn clip_with_ffmpeg(config: &CameraClipConfig) -> Result<CameraClipResult> {
    let out_path = media_dir().join(format!("camera_clip_{}.mp4", timestamp_suffix()));
    let duration_str = format!("{:.1}", config.duration_secs);

    let mut args: Vec<String> = vec![
        "-f".into(),
        "avfoundation".into(),
    ];

    // Input device: "0:0" = default video + default audio, "0:none" = video only
    let input_device = if config.with_audio {
        "0:0".to_string()
    } else {
        "0:none".to_string()
    };
    args.extend(["-i".into(), input_device]);

    // Duration and output settings
    args.extend([
        "-t".into(),
        duration_str.clone(),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "ultrafast".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
    ]);

    if config.with_audio {
        args.extend(["-c:a".into(), "aac".into()]);
    }

    // Overwrite output
    args.extend(["-y".into(), out_path.to_string_lossy().into_owned()]);

    debug!(args = ?args, "Running ffmpeg for camera clip");

    let output = tokio::process::Command::new("ffmpeg")
        .args(&args)
        .output()
        .await
        .map_err(|e| {
            aleph_desktop::DesktopError::BridgeFailed(format!(
                "Failed to run ffmpeg (install: brew install ffmpeg): {e}"
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // ffmpeg writes progress info to stderr; only treat as error if file doesn't exist
        if !out_path.exists() {
            return Err(aleph_desktop::DesktopError::BridgeFailed(format!(
                "ffmpeg recording failed: {stderr}"
            )));
        }
    }

    Ok(CameraClipResult {
        file_path: out_path.to_string_lossy().into_owned(),
        duration_secs: config.duration_secs,
        has_audio: config.with_audio,
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

fn get_device_string_property(device_id: core_audio_ffi::AudioDeviceID, selector: u32) -> Option<String> {
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
        core_foundation::string::CFString::wrap_under_create_rule(cf_string as core_foundation::string::CFStringRef)
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
        AudioObjectGetPropertyDataSize(
            device_id,
            &address,
            0,
            std::ptr::null(),
            &mut data_size,
        )
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

    let recognizer = unsafe {
        SFSpeechRecognizer::initWithLocale(SFSpeechRecognizer::alloc(), &locale)
    };
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
        SFSpeechURLRecognitionRequest::initWithURL(
            SFSpeechURLRecognitionRequest::alloc(),
            &url,
        )
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

    // Start recognition task (the returned task keeps recognition alive)
    let _task = unsafe {
        recognizer.recognitionTaskWithRequest_resultHandler(&request, &handler)
    };

    // Wait for result with a 60-second timeout (Apple's speech recognition limit)
    match rx.recv_timeout(Duration::from_secs(60)) {
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
    }
}

// ---------------------------------------------------------------------------
// MediaCapability implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl MediaCapability for MacOSMedia {
    async fn camera_snap(&self, config: CameraSnapConfig) -> Result<CameraSnapResult> {
        let _ = config; // imagesnap handles quality internally
        debug!("Taking camera photo via imagesnap");
        snap_with_imagesnap().await
    }

    async fn camera_clip(&self, config: CameraClipConfig) -> Result<CameraClipResult> {
        let config = config.clamped();
        debug!(
            duration = config.duration_secs,
            audio = config.with_audio,
            "Recording camera clip via ffmpeg"
        );
        clip_with_ffmpeg(&config).await
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
