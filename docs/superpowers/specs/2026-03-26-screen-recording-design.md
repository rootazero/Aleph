# Screen Recording Design

**Date:** 2026-03-26
**Status:** Approved
**Scope:** Add screen recording to ScreenCapability via ScreenCaptureKit, output MP4 with optional audio.

## Background

Aleph can capture screenshots (xcap) and perform OCR (Vision), but cannot record screen video. Screen recording is a core AI assistant perception capability — enabling the LLM to observe workflows, capture demonstrations, and provide contextual help based on what's happening on screen.

OpenClaw implements this via `ScreenRecordService.swift` using ScreenCaptureKit (SCStream + AVAssetWriter). Aleph will achieve the same using `objc2-screen-capture-kit` from Rust.

## Scope

### In Scope
- Extend ScreenCapability trait with `screen_record()` method
- macOS implementation using ScreenCaptureKit (SCStream)
- H.264 video + optional AAC audio encoding to MP4 via AVAssetWriter
- Configurable duration, FPS, audio toggle, region
- Desktop tool `screen_record` action

### Out of Scope
- Real-time screen streaming (no consumer)
- Window-specific recording (display-level only in v1)
- Linux/Windows screen recording
- Screenshot improvements (xcap already works)

## Types

**File:** `crates/desktop/src/screen_types.rs`

```rust
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
```

## Trait Extension

**File:** `crates/desktop/src/traits/screen.rs`

```rust
// Add to existing ScreenCapability trait:

    /// Record the screen for a specified duration, output as MP4.
    async fn screen_record(&self, config: ScreenRecordConfig) -> Result<ScreenRecordResult> {
        let _ = config;
        Err(DesktopError::NotImplemented("screen recording not available".into()))
    }
```

Default implementation returns NotImplemented. Linux/Windows/NativeScreen don't need changes (inherit default). macOS overrides in NativeScreen.

## macOS Implementation

**File:** `crates/desktop/src/perception.rs` (new `#[cfg(target_os = "macos")]` section)

### ScreenCaptureKit Flow

```
1. SCShareableContent::getCurrentWithCompletionHandler
   → enumerate displays, pick primary (or matching region)

2. SCContentFilter::initWithDisplay_excludingApplications
   → filter to target display, exclude Aleph's own windows

3. SCStreamConfiguration
   → width/height from display, fps, showsCursor=true, capturesAudio

4. SCStream::initWithFilter_configuration_delegate
   → delegate = custom Rust class implementing SCStreamOutput protocol
   → delegate sends CMSampleBuffer via mpsc::Sender

5. AVAssetWriter + AVAssetWriterInput (H.264) + optional audio input (AAC)
   → write to temp file, finalize on stop

6. Duration timer → stopCapture → finalize writer → return file path
```

### SCStreamOutput Delegate

Use `objc2::declare_class!` to create a Rust class implementing the `SCStreamOutput` protocol:

```rust
declare_class!(
    struct StreamDelegate {
        video_tx: IvarDrop<Box<mpsc::Sender<CMSampleBuffer>>, "_video_tx">,
        audio_tx: IvarDrop<Box<mpsc::Sender<CMSampleBuffer>>, "_audio_tx">,
    }

    unsafe impl ClassType for StreamDelegate {
        type Super = NSObject;
    }

    // stream:didOutputSampleBuffer:ofType:
    unsafe impl StreamDelegate {
        #[method(stream:didOutputSampleBuffer:ofType:)]
        fn did_output_sample_buffer(&self, stream: &SCStream, buffer: &CMSampleBuffer, output_type: SCStreamOutputType) {
            match output_type {
                SCStreamOutputType::Screen => { let _ = self.video_tx.send(buffer.retain()); }
                SCStreamOutputType::Audio => { let _ = self.audio_tx.send(buffer.retain()); }
            }
        }
    }
);
```

### Recording Thread

The entire recording runs in `spawn_blocking`:

```
1. Create delegate with channels
2. Get shareable content (block2 + channel)
3. Build filter + config
4. Create SCStream
5. Add stream output (delegate)
6. Start capture (block2 + channel)
7. Spawn writer thread: loop recv video/audio buffers, write to AVAssetWriter
8. Sleep for duration
9. Stop capture (block2 + channel)
10. Finalize AVAssetWriter
11. Return file path
```

### Output File

- Path: `{workspace}/_media/screen_record_{timestamp}.mp4`
- Workspace directory resolved via existing `ToolContext` workspace convention
- Fallback: `~/.aleph/data/_media/` if no workspace context
- Video: H.264, display native resolution (or region dimensions)
- Audio: AAC, 1 channel, 44.1kHz (when enabled)

### AVAssetWriter Setup

```
AVAssetWriter → outputURL (temp MP4 path), fileType: .mp4

Video Input:
  AVAssetWriterInput(mediaType: .video, outputSettings: [
    AVVideoCodecKey: .h264,
    AVVideoWidthKey: width,
    AVVideoHeightKey: height,
  ])
  expectsMediaDataInRealTime = true

Audio Input (optional):
  AVAssetWriterInput(mediaType: .audio, outputSettings: [
    AVFormatIDKey: kAudioFormatMPEG4AAC,
    AVSampleRateKey: 44100,
    AVNumberOfChannelsKey: 1,
  ])
  expectsMediaDataInRealTime = true
```

## Dependencies

### crates/desktop/Cargo.toml macOS additions

```toml
objc2-screen-capture-kit = { version = "0.3", features = [
    "SCStream", "SCShareableContent", "SCContentFilter",
    "SCStreamConfiguration", "SCStreamOutput",
] }
objc2-core-media = { version = "0.3", features = ["CMSampleBuffer"] }
objc2-av-foundation = { version = "0.3", features = [
    "AVAssetWriter", "AVAssetWriterInput", "AVMediaFormat",
    "AVVideoSettings", "AVAudioSettings",
] }
```

Note: Some features may need adjustment based on actual crate API. The implementor should verify feature flags against crate source.

## Tool Integration

**File:** `core/src/builtin_tools/desktop/mod.rs`

New action `screen_record` on the existing `desktop` tool:

```json
{
  "action": "screen_record",
  "duration": 5.0,
  "fps": 30,
  "with_audio": false
}
```

Response:
```json
{
  "success": true,
  "data": {
    "file_path": "/path/to/_media/screen_record_1711500000.mp4",
    "duration_secs": 5.0,
    "has_audio": false
  }
}
```

## Error Handling

All failures map to `DesktopError::ScreenCapture(msg)`:
- Permission denied → "screen recording permission denied"
- No display → "no display available for recording"
- SCStream start failure → "screen capture stream failed: {detail}"
- AVAssetWriter failure → "video writer failed: {detail}"
- Timeout → "screen recording setup timed out"

## Permission Integration

- No hardcoded permission pre-check (R8 LLM Sovereignty)
- ScreenCaptureKit returns errors when permission is missing
- LLM uses `permission` tool to check/request before recording
- System prompt guidance: "Check screen_recording permission before using screen_record"

## Testing

- `test_screen_record_config_clamp` — verify parameter clamping logic
- `test_screen_record_config_default` — verify defaults (5s, 30fps, no audio)
- `test_screen_record_no_display` — CI: verify correct error without display (no panic)

Integration tests require macOS with display + ScreenRecording TCC permission.

## File Changes

| Action | File | Responsibility |
|--------|------|---------------|
| Create | `crates/desktop/src/screen_types.rs` | ScreenRecordConfig, ScreenRecordResult |
| Modify | `crates/desktop/src/traits/screen.rs` | Add screen_record() with default impl |
| Modify | `crates/desktop/src/lib.rs` | Export screen_types |
| Modify | `crates/desktop/src/perception.rs` | macOS ScreenCaptureKit implementation |
| Modify | `crates/desktop/src/native_screen.rs` | NativeScreen delegates screen_record |
| Modify | `crates/desktop/Cargo.toml` | Add SCK, CoreMedia, AVFoundation deps |
| Modify | `core/src/builtin_tools/desktop/mod.rs` | Add screen_record action |

## Non-Goals

- Real-time screen streaming
- Window-specific recording (display-only in v1)
- Linux/Windows recording
- Recording pause/resume
- Custom video codec selection
