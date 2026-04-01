# Media Capability Design

**Date:** 2026-03-26
**Status:** Approved
**Scope:** Add MediaCapability trait (6th capability) for camera capture and audio device management.

## Scope

- `camera_snap` — capture JPEG photo from default camera
- `camera_clip` — record MP4 video from default camera
- `list_audio_devices` — enumerate audio input devices via CoreAudio
- New `media` tool exposing all three actions to LLM

## Types (`media_types.rs`)

```rust
pub struct CameraSnapConfig { pub quality: f32 }  // 0.0-1.0, default 0.9
pub struct CameraSnapResult { pub image_base64: String, pub width: u32, pub height: u32 }
pub struct CameraClipConfig { pub duration_secs: f64, pub with_audio: bool }  // 0.25-60.0
pub struct CameraClipResult { pub file_path: String, pub duration_secs: f64, pub has_audio: bool }
pub struct AudioDeviceInfo { pub uid: String, pub name: String, pub is_input: bool, pub is_default: bool }
```

## Trait (`traits/media.rs`)

```rust
#[async_trait]
pub trait MediaCapability: Send + Sync {
    async fn camera_snap(&self, config: CameraSnapConfig) -> Result<CameraSnapResult>;
    async fn camera_clip(&self, config: CameraClipConfig) -> Result<CameraClipResult>;
    async fn list_audio_devices(&self) -> Result<Vec<AudioDeviceInfo>>;
}
```

Default impls return NotImplemented. DesktopPlatform gets `fn media()`.

## macOS Implementation

- **camera_snap**: AVCaptureSession + AVCaptureDeviceInput (default video device) + AVCapturePhotoOutput. Capture via delegate (define_class! implementing AVCapturePhotoCaptureDelegate), extract JPEG data, base64 encode.
- **camera_clip**: AVCaptureSession + AVCaptureMovieFileOutput. Record to temp MP4, stop after duration. Delegate (define_class! implementing AVCaptureFileOutputRecordingDelegate) signals completion.
- **list_audio_devices**: CoreAudio C FFI — `AudioObjectGetPropertyData` with `kAudioHardwarePropertyDevices` to enumerate, then query each device's name/UID/input channels.

## Dependencies

```toml
# desktop-macos already has objc2-av-foundation for permission check
# Need additional features: AVCaptureSession, AVCaptureDeviceInput,
# AVCapturePhotoOutput, AVCaptureMovieFileOutput
```

## File Changes

| Action | File |
|--------|------|
| Create | `crates/desktop/src/media_types.rs` |
| Create | `crates/desktop/src/traits/media.rs` |
| Create | `crates/desktop-macos/src/media.rs` |
| Create | `src/builtin_tools/media_tool.rs` |
| Modify | `crates/desktop/src/traits/mod.rs`, `lib.rs`, `platform.rs` |
| Modify | `crates/desktop-macos/src/lib.rs`, `Cargo.toml` |
| Modify | `crates/desktop-linux/src/lib.rs`, `crates/desktop-windows/src/lib.rs` |
| Modify | `src/builtin_tools/mod.rs`, `src/executor/builtin_registry/builder.rs`, `registry.rs` |
