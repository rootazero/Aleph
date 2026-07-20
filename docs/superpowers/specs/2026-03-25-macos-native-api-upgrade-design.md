# macOS Native API Upgrade Design

**Date:** 2026-03-25
**Status:** Approved
**Scope:** Replace osascript/pbcopy/sw_vers command-line calls with direct objc2 API calls; fill in macOS OCR and window management gaps.

## Background

Aleph's `crates/desktop-macos/` currently implements `SystemCapability` by spawning external processes (`osascript`, `pbcopy`, `pbpaste`, `sw_vers`, `open`). This approach is:

- **Slow**: each call spawns a subprocess (~50-500ms overhead)
- **Fragile**: depends on parsing stdout/stderr text output
- **Limited**: `pbpaste` can't read images; `osascript System Events` needs Accessibility permission; notifications belong to osascript, not Aleph
- **Incomplete**: macOS OCR (`VNRecognizeTextRequest`) and window management (`CGWindowListCopyWindowInfo`) are marked `NotImplemented`

OpenClaw (reference project) uses Swift to call these APIs directly. Aleph can achieve the same — and more — using `objc2` from Rust, gaining type safety, zero-copy interop, and no Swift toolchain dependency.

## Scope

### Replace (7 call sites)

| Current | Command | Replace With |
|---------|---------|-------------|
| Clipboard read | `pbpaste` | `NSPasteboard.generalPasteboard` (text + image) |
| Clipboard write | `pbcopy` (stdin pipe) | `NSPasteboard.setString:forType:` |
| Notification | `osascript display notification` | `UNUserNotificationCenter` (fallback: osascript) |
| Quit app | `osascript tell app to quit` | `NSRunningApplication.terminate()` |
| List apps | `osascript System Events` | `NSWorkspace.runningApplications` |
| System info | `sw_vers` + `hostname` + `USER` env | `NSProcessInfo` |
| Launch app | `open -a` / `open -b` | `NSWorkspace.openApplication(at:)` |

### Add (2 NotImplemented gaps)

| Capability | API |
|-----------|-----|
| macOS OCR | Vision `VNRecognizeTextRequest` |
| macOS window list/focus | `CGWindowListCopyWindowInfo` + `NSRunningApplication` |

### Preserve (not touched)

- `automation.rs` — osascript for AppleScript/JXA execution (legitimate use)
- `pim.rs` + SwiftBridge + `aleph-bridge` — PIM stays on Swift bridge
- `enigo` — mouse/keyboard input (cross-platform)
- `xcap` — screenshot capture (cross-platform)

## Technical Approach: objc2 Ecosystem

Use `objc2` (v0.6) and its typed sub-crates for macOS framework bindings:

- `objc2-foundation` — NSString, NSArray, NSDictionary, NSProcessInfo, NSURL
- `objc2-app-kit` — NSPasteboard, NSWorkspace, NSRunningApplication, NSImage
- `objc2-user-notifications` — UNUserNotificationCenter, UNNotificationContent
- `objc2-vision` — VNRecognizeTextRequest, VNRecognizedTextObservation
- `core-graphics` crate — CGWindowListCopyWindowInfo (C-level API)

**Why objc2:** Type-safe ObjC bindings, compile-time checked selectors, actively maintained (used by Servo), covers Foundation/AppKit/Vision/UserNotifications. No Swift toolchain needed.

## File Structure

### Before

```
crates/desktop-macos/src/
├── lib.rs
├── automation.rs
├── pim.rs
└── system.rs          ← 260 lines, all osascript/pbcopy calls
```

### After

```
crates/desktop-macos/src/
├── lib.rs              # unchanged (MacOSPlatform aggregator)
├── automation.rs       # unchanged
├── pim.rs              # unchanged
├── system.rs           # DELETED (replaced by system/)
└── system/
    ├── mod.rs          # MacOSSystem struct + SystemCapability trait impl (delegates)
    ├── clipboard.rs    # NSPasteboard (text read/write + image detection/read)
    ├── workspace.rs    # NSWorkspace + NSRunningApplication (launch/quit/list)
    ├── notification.rs # UNUserNotificationCenter (+ osascript fallback)
    └── sysinfo.rs      # NSProcessInfo (version, hostname, username)

crates/desktop/src/
├── action.rs           # macOS branch: CGWindowListCopyWindowInfo + NSRunningApplication
└── perception.rs       # macOS branch: VNRecognizeTextRequest
```

## Module Designs

### clipboard.rs — NSPasteboard

```
NSPasteboard.generalPasteboard()
├── Read text: stringForType(NSPasteboardTypeString)
├── Detect image: types().containsObject(NSPasteboardTypePNG or TIFF)
├── Read image: dataForType(NSPasteboardTypePNG) → base64 encode
└── Write text: clearContents() + setString:forType:
```

- NSPasteboard is generally safe to use from background threads in practice, though Apple docs recommend main-thread for AppKit objects. In headless server mode (no NSApplication run loop), this is not a concern.
- Image read: prefer PNG type, fallback TIFF → convert to PNG via `image` crate (already in `desktop/Cargo.toml`) then base64
- Existing `ClipboardContent` type has `has_image` and `image_base64` fields — now populated correctly

### workspace.rs — NSWorkspace + NSRunningApplication

```
NSWorkspace.sharedWorkspace()
├── Launch: urlForApplication(withBundleIdentifier:) → openApplication(at:configuration:)
├── List: runningApplications → iterate [NSRunningApplication]
│         ├── .localizedName → name
│         ├── .bundleIdentifier → bundle_id
│         ├── .processIdentifier → pid (i32 → u64)
│         └── .isActive → is_active
└── Quit: NSRunningApplication.terminate() (graceful, like Cmd+Q)
```

- `runningApplications` is instant (no IPC to System Events)
- No Accessibility permission needed (unlike osascript System Events)
- Launch by name: use `NSWorkspace.fullPathForApplication(name)` → NSURL → open
- Launch by bundle ID: use `urlForApplication(withBundleIdentifier:)` → open
- **Quit by name vs bundle ID:** scan `runningApplications` matching either `localizedName` (case-insensitive) or `bundleIdentifier`; call `terminate()` on first match. If no match found, return `InputFailed`.

### notification.rs — UNUserNotificationCenter

```
Attempt order:
1. UNUserNotificationCenter.currentNotificationCenter()
   ├── requestAuthorization (once, cached)
   └── addNotificationRequest(content: title+body, trigger: nil)
   ├── Success → return
   └── Failure (no bundle ID / permission denied)
2. osascript display notification (final fallback)
```

- First use requests permission; result cached in-memory
- Notification attributed to Aleph app (shows Aleph icon)
- Fallback: aleph-server may run without bundle ID (CLI mode) — osascript still works as last resort
- Only osascript usage retained in the entire system/ module

### sysinfo.rs — NSProcessInfo

```
NSProcessInfo.processInfo()
├── .operatingSystemVersion → {major, minor, patch} → "15.3.1"
├── .hostName → hostname
└── .userName → username
```

- Pure memory reads, zero process overhead
- Replaces: `sw_vers` command, `hostname` crate, `USER` env var
- Architecture: keep `std::env::consts::ARCH` (no ObjC equivalent needed)

### perception.rs macOS OCR — Vision Framework

```
#[cfg(target_os = "macos")]
VNRecognizeTextRequest
├── Input: PNG bytes → CGImage (CGDataProvider + CGImageCreateWithPNGDataProvider)
├── Config: recognitionLevel = .accurate
│           recognitionLanguages = ["zh-Hans", "en-US"]
├── Execute: VNImageRequestHandler(cgImage:).perform([request])
└── Output: [VNRecognizedTextObservation] → OcrResult
    ├── .topCandidates(1).first.string → line text
    ├── .boundingBox → normalized (0-1) rect → multiply by image dimensions
    └── .confidence → confidence score
```

- Symmetric with Windows OCR in same file (`#[cfg(target_os = "macos")]` block)
- Runs in `spawn_blocking` (Vision is synchronous and CPU-intensive)
- Supports Chinese + English mixed text natively

### action.rs macOS Window Management — CGWindowList

```
#[cfg(target_os = "macos")]
Window list:
  CGWindowListCopyWindowInfo(
    kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
    kCGNullWindowID
  ) → [CFDictionary] → Vec<WindowInfo>
  ├── kCGWindowNumber → id
  ├── kCGWindowName → title (filter empty)
  ├── kCGWindowOwnerName → owner
  └── kCGWindowOwnerPID → pid

Window focus (window_id: u64):
  1. Scan CGWindowListCopyWindowInfo to find entry where kCGWindowNumber == window_id
  2. Extract kCGWindowOwnerPID from that entry
  3. NSRunningApplication(withProcessIdentifier: pid)
  4. activateWithOptions(.activateIgnoringOtherApps)
```

- Uses `core-graphics` crate for CGWindowList (C-level API)
- Uses `objc2-app-kit` for NSRunningApplication activate
- `focus_window` requires a CGWindowNumber → PID lookup step (scan window list for matching ID)
- Filter out windows with empty title and non-zero layer (menu bar items, etc.)
- `action.rs` macOS `launch_app` branch also replaced with NSWorkspace (~10 lines, independent impl to avoid circular crate dependency)

## Dependency Changes

### desktop-macos/Cargo.toml

```toml
# REMOVE
tokio = { version = "1", features = ["process"] }
hostname.workspace = true

# ADD
objc2 = "0.6"
objc2-foundation = { version = "0.3", features = [
    "NSString", "NSArray", "NSDictionary", "NSProcessInfo", "NSURL",
    "NSData",
] }
objc2-app-kit = { version = "0.3", features = [
    "NSPasteboard", "NSWorkspace", "NSRunningApplication", "NSImage",
] }
objc2-user-notifications = { version = "0.3", features = [
    "UNUserNotificationCenter", "UNNotificationContent",
    "UNNotificationRequest", "UNNotificationSound",
] }
```

### desktop/Cargo.toml (macOS target only)

```toml
[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.6"
objc2-foundation = { version = "0.3", features = ["NSString", "NSArray", "NSData"] }
objc2-vision = { version = "0.3", features = [
    "VNRecognizeTextRequest", "VNRecognizedTextObservation",
    "VNImageRequestHandler",
] }
objc2-app-kit = { version = "0.3", features = ["NSRunningApplication"] }
core-graphics = "0.25"
```

## Error Handling

No new `DesktopError` variants. objc2 failures map to existing variants:

- NSPasteboard failures → `InputFailed("clipboard: ...")`
- NSWorkspace failures → `InputFailed("launch_app: ...")`
- Vision OCR failures → `OcrFailed("...")`
- CGWindowList failures → `WindowFailed("...")`
- NSRunningApplication activate failures → `WindowFailed("...")`

All `unsafe` blocks are tightly scoped around individual objc2 calls, never leak beyond module boundaries.

## Async Adaptation

- **Simple calls** (NSPasteboard, NSWorkspace, NSProcessInfo): execute directly in async context — these are <1ms memory reads, `spawn_blocking` overhead would dominate
- **Heavy calls** (Vision OCR, CGWindowListCopyWindowInfo): use `spawn_blocking` — these are CPU-intensive

This matches the pragmatic principle: don't add machinery where it provides no benefit.

## Notification Fallback Detail

```
notification.rs internal logic:

static NOTIFICATION_METHOD: OnceLock<NotificationMethod> = OnceLock::new();

enum NotificationMethod {
    UserNotificationCenter,
    Osascript,  // fallback
}

fn detect_method() -> NotificationMethod {
    // Try UNUserNotificationCenter — requires bundle ID
    if can_use_un_center() {
        NotificationMethod::UserNotificationCenter
    } else {
        NotificationMethod::Osascript
    }
}
```

Detection runs once on first call, cached via `OnceLock`. The fallback is invisible to callers.

## Cleanup Checklist

| Item | Location | Action |
|------|----------|--------|
| `system.rs` (old) | `desktop-macos/src/system.rs` | Delete (replaced by `system/`) |
| `escape_applescript()` | old `system.rs` | Deleted with file |
| `tokio = { features = ["process"] }` | `desktop-macos/Cargo.toml` | Remove |
| `hostname` dependency | `desktop-macos/Cargo.toml` | Remove |
| macOS `open` command in `launch_app` | `desktop/src/action.rs` | Replace with NSWorkspace |
| macOS `NotImplemented` in `perform_ocr` | `desktop/src/perception.rs` | Replace with Vision |
| macOS `NotImplemented` in `window_list` | `desktop/src/action.rs` | Replace with CGWindowList |
| macOS `NotImplemented` in `focus_window` | `desktop/src/action.rs` | Replace with NSRunningApplication |

## Testing

### Updated existing tests

- `test_system_info` — same assertions, new implementation
- `test_clipboard_roundtrip` — same assertions, new implementation

### New tests

- `test_clipboard_read_detects_image` — write image to clipboard, verify `has_image = true`
- `test_list_running_apps_includes_finder` — Finder always runs, verify list contains it
- `test_ocr_chinese_text` — test PNG with Chinese text, verify Vision output
- `test_window_list_nonempty` — verify window list non-empty when display available
- `test_window_list_has_finder` — verify Finder appears in window list

All tests gated with `#[cfg(target_os = "macos")]`.

## Platform Requirements

- **Minimum macOS version:** 10.14 (Mojave) — required by `UNUserNotificationCenter`
- Vision framework (`VNRecognizeTextRequest`): macOS 10.15+
- In practice, Aleph targets macOS 13+ (Ventura), so all APIs are available

## Async Adaptation Detail

- `MacOSSystem` struct retains zero state (notification method cache is module-level `static OnceLock`)
- "Execute directly" means synchronous objc2 code inside an `async fn` body — no `spawn_blocking` wrapper
- This applies to: clipboard, workspace (launch/quit/list), sysinfo
- `spawn_blocking` used for: Vision OCR, CGWindowListCopyWindowInfo (heavier operations)

## Non-Goals

- **PIM migration** — EventKit/Contacts stay on SwiftBridge (objc2 bindings immature)
- **Screen recording / camera** — Direction 2 (future work)
- **Global hotkeys / voice** — Direction 3 (future work)
- **Linux/Windows improvements** — out of scope
- **Trait interface changes** — all existing trait methods preserved as-is
