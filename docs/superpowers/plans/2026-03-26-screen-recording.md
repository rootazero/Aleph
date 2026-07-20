# Screen Recording Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add screen recording capability to ScreenCapability trait, outputting MP4 files with optional audio.

**Architecture:** Two-tier macOS implementation: (1) SCRecordingOutput (macOS 15+) for zero-complexity recording, (2) `screencapture -V` CLI fallback for macOS 13-14. Both output MP4. The runtime detection picks the best available method.

**Tech Stack:** `objc2-screen-capture-kit` 0.3 (SCRecordingOutput, SCStream, SCShareableContent, SCContentFilter, SCStreamConfiguration), `block2` 0.6, `dispatch2` 0.3

**Spec:** `docs/superpowers/specs/2026-03-26-screen-recording-design.md`

**Key Discovery:** `SCRecordingOutput` (macOS 15+) handles all encoding internally — no AVAssetWriter, no CMSampleBuffer delegate, no define_class! needed. Just configure output URL + codec, add to stream, start/stop capture. For macOS 13-14, fall back to `screencapture -V duration` which is a built-in macOS CLI that records screen video.

---

## File Map

| Action | File | Responsibility |
|--------|------|---------------|
| Create | `crates/desktop/src/screen_types.rs` | ScreenRecordConfig, ScreenRecordResult |
| Modify | `crates/desktop/src/traits/screen.rs` | Add screen_record() with default impl |
| Modify | `crates/desktop/src/lib.rs` | Export screen_types |
| Modify | `crates/desktop/src/native_screen.rs` | NativeScreen delegates screen_record |
| Modify | `crates/desktop/src/perception.rs` | macOS screen_record implementation |
| Modify | `crates/desktop/Cargo.toml` | Add objc2-screen-capture-kit, dispatch2 |
| Modify | `src/builtin_tools/desktop/mod.rs` | Add screen_record action |

---

### Task 1: Types + Trait Extension

**Files:**
- Create: `crates/desktop/src/screen_types.rs`
- Modify: `crates/desktop/src/traits/screen.rs`
- Modify: `crates/desktop/src/lib.rs`

- [ ] **Step 1: Create screen_types.rs**

```rust
//! Types for screen recording.

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
    pub file_path: String,
    pub duration_secs: f64,
    pub has_audio: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_clamp() {
        let config = ScreenRecordConfig {
            duration_secs: 100.0,
            fps: 120,
            with_audio: false,
            region: None,
        }.clamped();
        assert_eq!(config.duration_secs, 60.0);
        assert_eq!(config.fps, 60);
    }

    #[test]
    fn test_config_clamp_min() {
        let config = ScreenRecordConfig {
            duration_secs: 0.01,
            fps: 0,
            with_audio: false,
            region: None,
        }.clamped();
        assert_eq!(config.duration_secs, 0.25);
        assert_eq!(config.fps, 1);
    }

    #[test]
    fn test_config_default() {
        let config = ScreenRecordConfig::default();
        assert_eq!(config.duration_secs, 5.0);
        assert_eq!(config.fps, 30);
        assert!(!config.with_audio);
        assert!(config.region.is_none());
    }
}
```

- [ ] **Step 2: Add screen_record to ScreenCapability trait**

In `crates/desktop/src/traits/screen.rs`, add import and method:

Add to imports: `use crate::screen_types::{ScreenRecordConfig, ScreenRecordResult};`

Add method with default impl at the end of the trait:
```rust
    /// Record the screen for a specified duration, output as MP4.
    async fn screen_record(&self, config: ScreenRecordConfig) -> Result<ScreenRecordResult> {
        let _ = config;
        Err(crate::DesktopError::NotImplemented(
            "screen recording not available on this platform".into(),
        ))
    }
```

- [ ] **Step 3: Export screen_types from lib.rs**

In `crates/desktop/src/lib.rs`, add `pub mod screen_types;` after `pub mod permission_types;`.

- [ ] **Step 4: Verify**

Run: `cargo check -p aleph-desktop`

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/src/screen_types.rs crates/desktop/src/traits/screen.rs crates/desktop/src/lib.rs
git commit -m "desktop: add ScreenRecordConfig types and screen_record trait method"
```

---

### Task 2: Add Dependencies

**Files:**
- Modify: `crates/desktop/Cargo.toml`

- [ ] **Step 1: Add macOS dependencies**

In `crates/desktop/Cargo.toml`, add to `[target.'cfg(target_os = "macos")'.dependencies]`:

```toml
objc2-screen-capture-kit = "0.3"
dispatch2 = "0.3"
```

Note: `objc2-screen-capture-kit` default features include SCStream, SCShareableContent, SCContentFilter, SCStreamConfiguration, SCRecordingOutput, block2, objc2-av-foundation, objc2-core-media, dispatch2, etc. — everything we need.

- [ ] **Step 2: Verify**

Run: `cargo check -p aleph-desktop`

- [ ] **Step 3: Commit**

```bash
git add crates/desktop/Cargo.toml Cargo.lock
git commit -m "desktop: add objc2-screen-capture-kit dependency for macOS"
```

---

### Task 3: macOS Screen Recording Implementation

**Files:**
- Modify: `crates/desktop/src/perception.rs`

This is the core task. Implement two recording methods with runtime detection.

- [ ] **Step 1: Implement the recording function**

Add to `crates/desktop/src/perception.rs` in a new `#[cfg(target_os = "macos")]` section (after the OCR section):

```rust
// ── macOS Screen Recording ──────────────────────────────────────

#[cfg(target_os = "macos")]
pub fn screen_record(config: &crate::screen_types::ScreenRecordConfig) -> Result<crate::screen_types::ScreenRecordResult> {
    let config = config.clone().clamped();

    // Generate output path
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let output_dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(".aleph/data/_media");
    std::fs::create_dir_all(&output_dir)
        .map_err(|e| DesktopError::ScreenCapture(format!("failed to create media dir: {e}")))?;

    let output_path = output_dir.join(format!("screen_record_{timestamp}.mp4"));

    // Try SCRecordingOutput (macOS 15+), fall back to screencapture CLI
    if can_use_sc_recording_output() {
        screen_record_via_sck(&config, &output_path)
    } else {
        screen_record_via_cli(&config, &output_path)
    }
}

#[cfg(target_os = "macos")]
fn can_use_sc_recording_output() -> bool {
    // SCRecordingOutput requires macOS 15.0+
    let info = objc2_foundation::NSProcessInfo::processInfo();
    let version = info.operatingSystemVersion();
    version.majorVersion >= 15
}
```

- [ ] **Step 2: Implement SCRecordingOutput path (macOS 15+)**

```rust
#[cfg(target_os = "macos")]
fn screen_record_via_sck(
    config: &crate::screen_types::ScreenRecordConfig,
    output_path: &std::path::Path,
) -> Result<crate::screen_types::ScreenRecordResult> {
    use std::sync::mpsc;
    use std::time::Duration;
    use block2::RcBlock;
    use objc2_foundation::{NSArray, NSError, NSString, NSURL};
    use objc2_screen_capture_kit::*;
    use objc2_core_media::CMTime;

    // 1. Get shareable content
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        if content.is_null() {
            let _ = tx.send(Err(format!("failed to get shareable content")));
        } else {
            // SAFETY: content is non-null, retain it
            let content = unsafe { objc2::rc::Retained::retain(content).unwrap() };
            let _ = tx.send(Ok(content));
        }
    });

    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, true, &block
        );
    }

    let content = rx.recv_timeout(Duration::from_secs(10))
        .map_err(|_| DesktopError::ScreenCapture("shareable content request timed out".into()))?
        .map_err(|e| DesktopError::ScreenCapture(e))?;

    // 2. Get primary display
    let displays = unsafe { content.displays() };
    let display = displays.first()
        .ok_or_else(|| DesktopError::ScreenCapture("no display available".into()))?;

    // 3. Create content filter (display, exclude nothing)
    let empty_apps: objc2::rc::Retained<NSArray<SCRunningApplication>> = NSArray::new();
    let empty_windows: objc2::rc::Retained<NSArray<SCWindow>> = NSArray::new();
    let filter = unsafe {
        SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
            SCContentFilter::alloc(),
            display,
            &empty_apps,
            &empty_windows,
        )
    };

    // 4. Configure stream
    let stream_config = unsafe {
        let c = SCStreamConfiguration::new();
        let scale = filter.pointPixelScale() as usize;
        let w = (unsafe { display.width() } as usize) * scale.max(1);
        let h = (unsafe { display.height() } as usize) * scale.max(1);
        c.setWidth(w);
        c.setHeight(h);
        c.setMinimumFrameInterval(CMTime {
            value: 1,
            timescale: config.fps as i32,
            flags: 1, // kCMTimeFlags_Valid
            epoch: 0,
        });
        c.setShowsCursor(true);
        c.setCapturesAudio(config.with_audio);
        if config.with_audio {
            c.setSampleRate(44100);
            c.setChannelCount(1);
        }
        c
    };

    // 5. Create recording output with configuration
    let recording_config = unsafe {
        let rc = SCRecordingOutputConfiguration::new();
        let url = NSURL::fileURLWithPath(&NSString::from_str(&output_path.to_string_lossy()));
        rc.setOutputURL(&url);
        rc
    };

    // Create a minimal recording delegate (just tracks completion)
    // We need a delegate that implements SCRecordingOutputDelegate
    // For simplicity, use a bare NSObject — the optional delegate methods
    // will just not be called, which is fine for our use case.
    // The recording still works without implementing delegate methods.

    // Actually, SCRecordingOutput requires a delegate. Let's use define_class!
    // to create a minimal one.
    use objc2::define_class;
    use objc2::rc::Retained;

    struct RecordingDelegateIvars {
        done_tx: std::sync::mpsc::Sender<std::result::Result<(), String>>,
    }

    define_class!(
        #[unsafe(super(objc2::runtime::NSObject))]
        #[name = "AlephRecordingDelegate"]
        #[ivars = RecordingDelegateIvars]
        struct RecordingDelegate;

        unsafe impl SCRecordingOutputDelegate for RecordingDelegate {
            #[unsafe(method(recordingOutputDidStartRecording:))]
            fn did_start(&self, _output: &SCRecordingOutput) {
                tracing::debug!("Screen recording started");
            }

            #[unsafe(method(recordingOutput:didFailWithError:))]
            fn did_fail(&self, _output: &SCRecordingOutput, error: &NSError) {
                let msg = error.localizedDescription().to_string();
                let _ = self.ivars().done_tx.send(Err(msg));
            }

            #[unsafe(method(recordingOutputDidFinishRecording:))]
            fn did_finish(&self, _output: &SCRecordingOutput) {
                let _ = self.ivars().done_tx.send(Ok(()));
            }
        }
    );

    let (done_tx, done_rx) = mpsc::channel();

    // Create the delegate — need to use mtm (main thread marker) or unsafe alloc
    let delegate = RecordingDelegate::alloc().set_ivars(RecordingDelegateIvars { done_tx });
    let delegate: Retained<RecordingDelegate> = unsafe { objc2::msg_send![super(delegate), init] };

    let recording_output = unsafe {
        SCRecordingOutput::initWithConfiguration_delegate(
            SCRecordingOutput::alloc(),
            &recording_config,
            objc2::runtime::ProtocolObject::from_ref(&*delegate),
        )
    };

    // 6. Create stream and add recording output
    let stream = unsafe {
        SCStream::initWithFilter_configuration_delegate(
            SCStream::alloc(),
            &filter,
            &stream_config,
            None, // no stream delegate needed, recording output handles it
        )
    };

    unsafe {
        stream.addRecordingOutput_error(&recording_output)
            .map_err(|e| DesktopError::ScreenCapture(format!("failed to add recording output: {e}")))?;
    }

    // 7. Start capture
    let (start_tx, start_rx) = mpsc::channel();
    let start_block = RcBlock::new(move |error: *mut NSError| {
        if error.is_null() {
            let _ = start_tx.send(Ok(()));
        } else {
            let msg = unsafe { (*error).localizedDescription().to_string() };
            let _ = start_tx.send(Err(msg));
        }
    });

    unsafe { stream.startCaptureWithCompletionHandler(Some(&start_block)); }

    start_rx.recv_timeout(Duration::from_secs(10))
        .map_err(|_| DesktopError::ScreenCapture("start capture timed out".into()))?
        .map_err(|e| DesktopError::ScreenCapture(format!("start capture failed: {e}")))?;

    // 8. Wait for recording duration
    std::thread::sleep(Duration::from_secs_f64(config.duration_secs));

    // 9. Stop capture (recording auto-finalizes)
    let (stop_tx, stop_rx) = mpsc::channel();
    let stop_block = RcBlock::new(move |error: *mut NSError| {
        if error.is_null() {
            let _ = stop_tx.send(Ok(()));
        } else {
            let msg = unsafe { (*error).localizedDescription().to_string() };
            let _ = stop_tx.send(Err(msg));
        }
    });

    unsafe { stream.stopCaptureWithCompletionHandler(Some(&stop_block)); }

    stop_rx.recv_timeout(Duration::from_secs(10))
        .map_err(|_| DesktopError::ScreenCapture("stop capture timed out".into()))?
        .map_err(|e| DesktopError::ScreenCapture(format!("stop capture failed: {e}")))?;

    // 10. Wait for recording to finish writing
    let _ = done_rx.recv_timeout(Duration::from_secs(10));

    Ok(crate::screen_types::ScreenRecordResult {
        file_path: output_path.to_string_lossy().to_string(),
        duration_secs: config.duration_secs,
        has_audio: config.with_audio,
    })
}
```

**IMPORTANT NOTES for implementor:**
- The `define_class!` syntax, ivars access pattern, and ProtocolObject construction MUST be verified against actual objc2 0.6 docs/source. The above is the intended pattern but exact syntax may differ.
- `SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler` — verify the exact method name and block signature in the crate source.
- `CMTime` struct fields: verify `flags` value for `kCMTimeFlags_Valid` (should be 1).
- `filter.pointPixelScale()` gives the Retina scale factor for pixel dimensions.

- [ ] **Step 3: Implement screencapture CLI fallback (macOS 13-14)**

```rust
#[cfg(target_os = "macos")]
fn screen_record_via_cli(
    config: &crate::screen_types::ScreenRecordConfig,
    output_path: &std::path::Path,
) -> Result<crate::screen_types::ScreenRecordResult> {
    use std::process::Command;
    use std::time::Duration;

    // macOS built-in screencapture can record video:
    // screencapture -V <duration> -v <output.mp4>
    // -V: record video for N seconds
    // -v: video recording mode
    let duration_int = config.duration_secs.ceil() as u64;

    let output = Command::new("screencapture")
        .args([
            "-V", &duration_int.to_string(),
            "-v",
            &output_path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| DesktopError::ScreenCapture(format!("screencapture failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DesktopError::ScreenCapture(format!(
            "screencapture failed: {stderr}"
        )));
    }

    Ok(crate::screen_types::ScreenRecordResult {
        file_path: output_path.to_string_lossy().to_string(),
        duration_secs: duration_int as f64,
        has_audio: false, // screencapture CLI doesn't support audio easily
    })
}
```

Note: `screencapture -V` may not be available on all macOS versions with exact same flags. The implementor should test on macOS 13/14 and adjust flags if needed. Worst case, this path returns NotImplemented with a helpful message.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p aleph-desktop`

- [ ] **Step 5: Commit**

```bash
git add crates/desktop/src/perception.rs
git commit -m "desktop: implement macOS screen recording via SCRecordingOutput + CLI fallback"
```

---

### Task 4: Wire NativeScreen + Desktop Tool

**Files:**
- Modify: `crates/desktop/src/native_screen.rs`
- Modify: `src/builtin_tools/desktop/mod.rs`

- [ ] **Step 1: Add screen_record to NativeScreen**

In `crates/desktop/src/native_screen.rs`, add to the `ScreenCapability` impl:

```rust
    async fn screen_record(&self, config: crate::screen_types::ScreenRecordConfig) -> Result<crate::screen_types::ScreenRecordResult> {
        #[cfg(target_os = "macos")]
        {
            tokio::task::spawn_blocking(move || perception::screen_record(&config))
                .await
                .map_err(|e| DesktopError::ScreenCapture(format!("task join error: {e}")))?
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = config;
            Err(DesktopError::NotImplemented(
                "screen recording not available on this platform".into(),
            ))
        }
    }
```

- [ ] **Step 2: Add screen_record action to desktop tool**

In `src/builtin_tools/desktop/mod.rs`, add:

1. Add to the tool DESCRIPTION string:
```
- screen_record: Record the screen as MP4 video. Optional: duration (seconds, default 5), fps (default 30), with_audio (default false)
```

2. Add to the DesktopArgs struct (or wherever action params are parsed):
```rust
    pub duration: Option<f64>,
    pub fps: Option<u32>,
    pub with_audio: Option<bool>,
```

3. Add the action handler in the match block:
```rust
"screen_record" => {
    let config = aleph_desktop::screen_types::ScreenRecordConfig {
        duration_secs: args.duration.unwrap_or(5.0),
        fps: args.fps.unwrap_or(30),
        with_audio: args.with_audio.unwrap_or(false),
        region: None,
    };
    match screen.screen_record(config).await {
        Ok(result) => Ok(DesktopOutput {
            success: true,
            data: Some(serde_json::to_value(&result).unwrap_or_default()),
            message: None,
        }),
        Err(e) => Ok(DesktopOutput {
            success: false,
            data: None,
            message: Some(e.to_string()),
        }),
    }
}
```

- [ ] **Step 3: Verify full compilation**

Run: `cargo check -p alephcore`

- [ ] **Step 4: Commit**

```bash
git add crates/desktop/src/native_screen.rs src/builtin_tools/desktop/mod.rs
git commit -m "desktop: wire screen_record into NativeScreen and desktop tool"
```

---

### Task 5: Full Verification

- [ ] **Step 1: Full workspace check**

Run: `cargo check`

- [ ] **Step 2: Run all desktop tests**

Run: `cargo test -p aleph-desktop --lib`

- [ ] **Step 3: Clippy**

Run: `cargo clippy -p aleph-desktop -- -D warnings`

- [ ] **Step 4: Commit if any fixes**

```bash
git add crates/desktop/
git commit -m "desktop: fix clippy warnings from screen recording"
```
