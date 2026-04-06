//! macOS media capture (camera photo/video, audio device listing).
//!
//! Camera operations use native AVFoundation APIs:
//! - Photos: AVCaptureSession + AVCapturePhotoOutput — captures JPEG from default camera
//! - Video: AVCaptureSession + AVCaptureMovieFileOutput — records MOV from default camera
//!
//! Audio recording uses ffmpeg CLI (audio-only capture via AVFoundation is complex).
//! Audio device listing uses CoreAudio C FFI directly.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use aleph_desktop::media_types::*;
use aleph_desktop::traits::MediaCapability;
use aleph_desktop::Result;
use async_trait::async_trait;
use base64::Engine;
use tracing::{debug, warn};

type PhotoDataSlot = Arc<std::sync::Mutex<Option<std::result::Result<Vec<u8>, String>>>>;

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

// ---------------------------------------------------------------------------
// AVFoundation capture delegates — MUST be at module scope to avoid
// ObjC class re-registration panic on repeated calls.
// ---------------------------------------------------------------------------

mod av_capture_delegates {
    use objc2::DefinedClass;
    use objc2_av_foundation::AVCaptureConnection;
    use objc2_av_foundation::{
        AVCaptureFileOutput, AVCaptureFileOutputRecordingDelegate, AVCapturePhoto,
        AVCapturePhotoCaptureDelegate, AVCapturePhotoOutput,
    };
    use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol, NSURL};
    use std::sync::{Arc, Condvar, Mutex};

    type PhotoDelegateData = Arc<Mutex<Option<Result<Vec<u8>, String>>>>;

    // ── Photo capture delegate ──────────────────────────────────

    pub struct PhotoDelegateIvars {
        pub data: PhotoDelegateData,
        pub signal: Arc<(Mutex<bool>, Condvar)>,
    }

    objc2::define_class!(
        #[unsafe(super(NSObject))]
        #[ivars = PhotoDelegateIvars]
        #[name = "AlephPhotoCaptureDelegate"]
        pub struct PhotoCaptureDelegate;

        unsafe impl AVCapturePhotoCaptureDelegate for PhotoCaptureDelegate {
            #[unsafe(method(captureOutput:didFinishProcessingPhoto:error:))]
            fn _did_finish_processing(
                &self,
                _output: &AVCapturePhotoOutput,
                photo: &AVCapturePhoto,
                error: Option<&NSError>,
            ) {
                let ivars = self.ivars();

                let result = if let Some(err) = error {
                    Err(err.to_string())
                } else {
                    match unsafe { photo.fileDataRepresentation() } {
                        Some(ns_data) => Ok(ns_data.to_vec()),
                        None => Err("fileDataRepresentation returned nil".into()),
                    }
                };

                {
                    let mut guard = ivars.data.lock().unwrap_or_else(|e| e.into_inner());
                    *guard = Some(result);
                }
                let (ref lock, ref cvar) = *ivars.signal;
                let mut done = lock.lock().unwrap_or_else(|e| e.into_inner());
                *done = true;
                cvar.notify_all();
            }
        }

        unsafe impl NSObjectProtocol for PhotoCaptureDelegate {}
    );

    // ── Movie recording delegate ────────────────────────────────

    pub struct MovieDelegateIvars {
        pub error: Arc<Mutex<Option<String>>>,
        pub signal: Arc<(Mutex<bool>, Condvar)>,
    }

    objc2::define_class!(
        #[unsafe(super(NSObject))]
        #[ivars = MovieDelegateIvars]
        #[name = "AlephMovieRecordingDelegate"]
        pub struct MovieRecordingDelegate;

        unsafe impl AVCaptureFileOutputRecordingDelegate for MovieRecordingDelegate {
            #[unsafe(method(captureOutput:didFinishRecordingToOutputFileAtURL:fromConnections:error:))]
            fn _did_finish_recording(
                &self,
                _output: &AVCaptureFileOutput,
                _url: &NSURL,
                _connections: &NSArray<AVCaptureConnection>,
                error: Option<&NSError>,
            ) {
                let ivars = self.ivars();
                if let Some(err) = error {
                    let mut guard = ivars.error.lock().unwrap_or_else(|e| e.into_inner());
                    *guard = Some(err.to_string());
                }
                let (ref lock, ref cvar) = *ivars.signal;
                let mut done = lock.lock().unwrap_or_else(|e| e.into_inner());
                *done = true;
                cvar.notify_all();
            }
        }

        unsafe impl NSObjectProtocol for MovieRecordingDelegate {}
    );
}

use av_capture_delegates::*;

/// macOS media capability using native AVFoundation + CoreAudio FFI.
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
// Camera snap via native AVFoundation (AVCapturePhotoOutput)
// ---------------------------------------------------------------------------

fn snap_native_blocking() -> Result<CameraSnapResult> {
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::AllocAnyThread;
    use objc2_av_foundation::{
        AVCaptureDevice, AVCaptureDeviceInput, AVCapturePhotoCaptureDelegate, AVCapturePhotoOutput,
        AVCapturePhotoSettings, AVCaptureSession, AVMediaTypeVideo,
    };
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    // 1. Get default video device
    let media_type = unsafe { AVMediaTypeVideo }
        .ok_or_else(|| bridge_err("AVMediaTypeVideo is not available"))?;
    let device = unsafe { AVCaptureDevice::defaultDeviceWithMediaType(media_type) }
        .ok_or_else(|| bridge_err("No camera device found"))?;

    // 2. Create input from device
    let input = unsafe { AVCaptureDeviceInput::deviceInputWithDevice_error(&device) }
        .map_err(|e| bridge_err(&format!("Failed to create device input: {e}")))?;

    // 3. Create session and add input
    let session = unsafe { AVCaptureSession::new() };
    let input_ref: &objc2_av_foundation::AVCaptureInput = &input;
    if !unsafe { session.canAddInput(input_ref) } {
        return Err(bridge_err("Cannot add camera input to session"));
    }
    unsafe { session.addInput(input_ref) };

    // 4. Create photo output and add to session
    let photo_output = unsafe { AVCapturePhotoOutput::new() };
    let output_ref: &objc2_av_foundation::AVCaptureOutput = &photo_output;
    if !unsafe { session.canAddOutput(output_ref) } {
        return Err(bridge_err("Cannot add photo output to session"));
    }
    unsafe { session.addOutput(output_ref) };

    // 5. Start session (blocks until running)
    unsafe { session.startRunning() };

    // 6. Wait for auto-exposure stabilization (~0.5s)
    std::thread::sleep(Duration::from_millis(500));

    // 7. Create delegate with signal channel
    let data_slot: PhotoDataSlot = Arc::new(Mutex::new(None));
    let signal = Arc::new((Mutex::new(false), Condvar::new()));

    let delegate_ivars = PhotoDelegateIvars {
        data: data_slot.clone(),
        signal: signal.clone(),
    };
    let delegate: Retained<PhotoCaptureDelegate> = {
        let alloc = PhotoCaptureDelegate::alloc().set_ivars(delegate_ivars);
        unsafe { objc2::msg_send![super(alloc), init] }
    };

    // 8. Capture photo with default JPEG settings
    let settings = unsafe { AVCapturePhotoSettings::photoSettings() };
    let delegate_proto: &ProtocolObject<dyn AVCapturePhotoCaptureDelegate> =
        ProtocolObject::from_ref(&*delegate);
    unsafe {
        photo_output.capturePhotoWithSettings_delegate(&settings, delegate_proto);
    }

    // 9. Wait for delegate callback (timeout 10s)
    let (ref lock, ref cvar) = *signal;
    let mut done = lock.lock().unwrap_or_else(|e| e.into_inner());
    let timeout = Duration::from_secs(10);
    while !*done {
        let (guard, wait_result) = cvar
            .wait_timeout(done, timeout)
            .unwrap_or_else(|e| e.into_inner());
        done = guard;
        if wait_result.timed_out() && !*done {
            unsafe { session.stopRunning() };
            return Err(bridge_err("Photo capture timed out after 10 seconds"));
        }
    }

    // 10. Stop session
    unsafe { session.stopRunning() };

    // 11. Extract JPEG data
    let result = {
        let guard = data_slot.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .as_ref()
            .cloned()
            .unwrap_or_else(|| Err("No photo data received".into()))
    };

    let jpeg_bytes = result.map_err(|e| bridge_err(&format!("Photo capture failed: {e}")))?;

    // 12. Get dimensions and encode to base64
    let (width, height) = image_dimensions_from_bytes(&jpeg_bytes);
    let image_base64 = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);

    Ok(CameraSnapResult {
        image_base64,
        width,
        height,
    })
}

/// Extract image dimensions from in-memory JPEG/HEIC bytes.
fn image_dimensions_from_bytes(data: &[u8]) -> (u32, u32) {
    // Write to a temp file for image crate to read dimensions
    let tmp = media_dir().join(format!("_tmp_dim_{}.jpg", timestamp_suffix()));
    if std::fs::write(&tmp, data).is_ok() {
        let dims = image::image_dimensions(&tmp).unwrap_or((0, 0));
        let _ = std::fs::remove_file(&tmp);
        dims
    } else {
        (0, 0)
    }
}

// ---------------------------------------------------------------------------
// Camera clip via native AVFoundation (AVCaptureMovieFileOutput)
// ---------------------------------------------------------------------------

fn clip_native_blocking(config: &CameraClipConfig) -> Result<CameraClipResult> {
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::AllocAnyThread;
    use objc2_av_foundation::{
        AVCaptureDevice, AVCaptureDeviceInput, AVCaptureFileOutputRecordingDelegate,
        AVCaptureMovieFileOutput, AVCaptureSession, AVMediaTypeAudio, AVMediaTypeVideo,
    };
    use objc2_foundation::NSURL;
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    let out_path = media_dir().join(format!("camera_clip_{}.mov", timestamp_suffix()));

    // 1. Get default video device
    let video_type = unsafe { AVMediaTypeVideo }
        .ok_or_else(|| bridge_err("AVMediaTypeVideo is not available"))?;
    let video_device = unsafe { AVCaptureDevice::defaultDeviceWithMediaType(video_type) }
        .ok_or_else(|| bridge_err("No camera device found"))?;

    // 2. Create session and add video input
    let session = unsafe { AVCaptureSession::new() };

    let video_input = unsafe { AVCaptureDeviceInput::deviceInputWithDevice_error(&video_device) }
        .map_err(|e| bridge_err(&format!("Failed to create video input: {e}")))?;

    let video_input_ref: &objc2_av_foundation::AVCaptureInput = &video_input;
    if !unsafe { session.canAddInput(video_input_ref) } {
        return Err(bridge_err("Cannot add video input to session"));
    }
    unsafe { session.addInput(video_input_ref) };

    // 3. Optionally add audio input
    let has_audio = if config.with_audio {
        let audio_type = unsafe { AVMediaTypeAudio };
        if let Some(audio_type) = audio_type {
            if let Some(audio_device) =
                unsafe { AVCaptureDevice::defaultDeviceWithMediaType(audio_type) }
            {
                match unsafe { AVCaptureDeviceInput::deviceInputWithDevice_error(&audio_device) } {
                    Ok(audio_input) => {
                        let audio_input_ref: &objc2_av_foundation::AVCaptureInput = &audio_input;
                        if unsafe { session.canAddInput(audio_input_ref) } {
                            unsafe { session.addInput(audio_input_ref) };
                            true
                        } else {
                            warn!("Cannot add audio input to session, recording video only");
                            false
                        }
                    }
                    Err(e) => {
                        warn!("Failed to create audio input: {e}, recording video only");
                        false
                    }
                }
            } else {
                warn!("No audio device found, recording video only");
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    // 4. Create movie file output and add to session
    let movie_output = unsafe { AVCaptureMovieFileOutput::new() };
    let output_ref: &objc2_av_foundation::AVCaptureOutput = &movie_output;
    if !unsafe { session.canAddOutput(output_ref) } {
        return Err(bridge_err("Cannot add movie output to session"));
    }
    unsafe { session.addOutput(output_ref) };

    // 5. Start session
    unsafe { session.startRunning() };

    // Brief warmup for auto-exposure
    std::thread::sleep(Duration::from_millis(300));

    // 6. Create delegate
    let error_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let signal = Arc::new((Mutex::new(false), Condvar::new()));

    let delegate_ivars = MovieDelegateIvars {
        error: error_slot.clone(),
        signal: signal.clone(),
    };
    let delegate: Retained<MovieRecordingDelegate> = {
        let alloc = MovieRecordingDelegate::alloc().set_ivars(delegate_ivars);
        unsafe { objc2::msg_send![super(alloc), init] }
    };

    // 7. Start recording
    let out_path_str = out_path.to_string_lossy();
    let ns_str = objc2_foundation::NSString::from_str(&out_path_str);
    let file_url = NSURL::fileURLWithPath(&ns_str);

    let delegate_proto: &ProtocolObject<dyn AVCaptureFileOutputRecordingDelegate> =
        ProtocolObject::from_ref(&*delegate);
    let file_output: &objc2_av_foundation::AVCaptureFileOutput = &movie_output;
    unsafe {
        file_output.startRecordingToOutputFileURL_recordingDelegate(&file_url, delegate_proto);
    }

    // 8. Record for the specified duration
    let record_duration = Duration::from_secs_f64(config.duration_secs);
    std::thread::sleep(record_duration);

    // 9. Stop recording
    unsafe { file_output.stopRecording() };

    // 10. Wait for delegate callback (timeout: duration + 10s buffer)
    let timeout = Duration::from_secs(config.duration_secs as u64 + 10);
    let (ref lock, ref cvar) = *signal;
    let mut done = lock.lock().unwrap_or_else(|e| e.into_inner());
    while !*done {
        let (guard, wait_result) = cvar
            .wait_timeout(done, timeout)
            .unwrap_or_else(|e| e.into_inner());
        done = guard;
        if wait_result.timed_out() && !*done {
            unsafe { session.stopRunning() };
            return Err(bridge_err("Movie recording delegate timed out"));
        }
    }

    // 11. Stop session
    unsafe { session.stopRunning() };

    // 12. Check for errors
    {
        let guard = error_slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref err_msg) = *guard {
            // AVFoundation reports "no error" with error code -11806 when
            // recording is stopped normally; only fail on real errors
            if !err_msg.contains("-11806") {
                return Err(bridge_err(&format!("Movie recording failed: {err_msg}")));
            }
        }
    }

    // 13. Verify file exists
    if !out_path.exists() {
        return Err(bridge_err("Movie recording produced no output file"));
    }

    Ok(CameraClipResult {
        file_path: out_path.to_string_lossy().into_owned(),
        duration_secs: config.duration_secs,
        has_audio,
    })
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
        let _ = config; // AVFoundation handles quality internally
        debug!("Taking camera photo via native AVFoundation");
        tokio::task::spawn_blocking(snap_native_blocking)
            .await
            .map_err(|e| bridge_err(&format!("Failed to spawn camera snap task: {e}")))?
    }

    async fn camera_clip(&self, config: CameraClipConfig) -> Result<CameraClipResult> {
        let config = config.clamped();
        debug!(
            duration = config.duration_secs,
            audio = config.with_audio,
            "Recording camera clip via native AVFoundation"
        );
        tokio::task::spawn_blocking(move || clip_native_blocking(&config))
            .await
            .map_err(|e| bridge_err(&format!("Failed to spawn camera clip task: {e}")))?
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
