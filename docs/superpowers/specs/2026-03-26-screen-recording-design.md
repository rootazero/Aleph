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

2. SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows
   → filter to target display (3 params: display, apps to exclude, windows to except)

3. SCStreamConfiguration
   → width/height in PIXELS (display points × scale factor for Retina)
   → setMinimumFrameInterval(CMTime { value: 1, timescale: fps })
   → showsCursor=true, capturesAudio

4. SCStream::initWithFilter_configuration_delegate
   → delegate = custom Rust class implementing SCStreamOutput protocol
   → delegate sends CMSampleBuffer via mpsc::Sender

5. AVAssetWriter + AVAssetWriterInput (H.264) + optional audio input (AAC)
   → write to temp file, finalize on stop

6. Duration timer → stopCapture → finalize writer → return file path
```

### SCStreamOutput Delegate

Use `objc2::define_class!` (objc2 0.6 syntax, NOT the old `declare_class!`) to create a Rust class implementing the `SCStreamOutput` protocol:

```rust
struct StreamDelegateIvars {
    video_tx: std::sync::mpsc::Sender<Vec<u8>>,  // extracted pixel data, NOT raw CMSampleBuffer
    audio_tx: std::sync::mpsc::Sender<Vec<u8>>,  // extracted audio data
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "AlephStreamDelegate"]
    #[ivars = StreamDelegateIvars]
    struct StreamDelegate;

    unsafe impl SCStreamOutput for StreamDelegate {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output_sample_buffer(
            &self,
            _stream: &SCStream,
            buffer: &CMSampleBuffer,
            output_type: SCStreamOutputType,
        ) {
            // Extract data from CMSampleBuffer in-callback
            // (CMSampleBuffer is a CF type — cannot .retain() like NSObject)
            // Use CMSampleBufferGetDataBuffer or CVPixelBuffer extraction
            // Send extracted bytes over channel
        }
    }
);
```

**Critical notes:**
- `CMSampleBuffer` is a CoreFoundation type, NOT an NSObject — cannot use `.retain()`. Must extract pixel/audio data in the delegate callback and send the extracted data over the channel.
- Callbacks require a dispatch queue — pass one via `addStreamOutput_type_sampleHandlerQueue_error`. Use `dispatch_queue_create` or the global concurrent queue.

### Recording Thread

The entire recording runs in `spawn_blocking`:

```
1. Create delegate with channels
2. Get shareable content (block2 + channel)
3. Build filter + config
4. Create SCStream with a dispatch queue for callbacks
5. Add stream output (delegate, type, sampleHandlerQueue)
6. Start capture (block2 + channel, recv_timeout 10s)
7. Writer loop: recv buffers from channel
   a. On first buffer: startWriting() + startSessionAtSourceTime(firstSampleTime)
   b. Check readyForMoreMediaData before each appendSampleBuffer
   c. Continue until duration elapsed
8. Stop capture (block2 + channel, recv_timeout 10s)
9. Finalize: finishWritingWithCompletionHandler (block2 + channel, recv_timeout 10s)
10. Return file path
```

**Cancellation:** If the process is interrupted, the Drop guard on the writer should call `cancelWriting()` to clean up the partial file.

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
    AVVideoWidthKey: pixel_width,   // display points × scale factor
    AVVideoHeightKey: pixel_height,
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

**Writer lifecycle:**
1. `startWriting()` — before first buffer
2. `startSessionAtSourceTime(firstSamplePresentationTime)` — REQUIRED before appendSampleBuffer
3. Loop: check `readyForMoreMediaData` → `appendSampleBuffer` (drop frame if not ready)
4. `finishWritingWithCompletionHandler` — async, needs block2 + channel pattern
5. On error/cancellation: `cancelWriting()` + delete partial file

**Tip:** Consider using `assetWriterInputWithMediaType_outputSettings_sourceFormatHint` with the CMFormatDescription from the first sample buffer — simplifies settings and avoids hardcoding dimensions.

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

**File:** `src/builtin_tools/desktop/mod.rs`

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
| Modify | `src/builtin_tools/desktop/mod.rs` | Add screen_record action |

## Non-Goals

- Real-time screen streaming
- Window-specific recording (display-only in v1)
- Linux/Windows recording
- Recording pause/resume
- Custom video codec selection
