# Change: Add Multimodal Content Support

## Status
🟢 Implemented - Phase 1 Complete

## Why

Currently, Aleph's `process_input` API only accepts string input, limiting users to text-only interactions. Modern AI providers (OpenAI GPT-4o, Claude 3.5) support multimodal input including images, and users frequently need to process:
1. **Images** from clipboard (screenshots, photos)
2. **Videos** or video frames (for future AI video understanding)
3. **Text files** dragged into applications

The current architecture:
- `process_input(user_input: String, context: CapturedContext)` - text only
- `CapturedContext` only contains app bundle ID and window title
- No infrastructure for file/media attachments
- Image processing exists in providers (OpenAI, Claude) but isn't exposed through the API

## Key Research Findings

### Content Acquisition Methods Comparison

Aleph uses two methods to capture content, which have **fundamentally different capabilities**:

| Content Type | Command+C/X (Clipboard) | Accessibility API |
|-------------|------------------------|-------------------|
| Text | ✅ `NSPasteboard.string` | ✅ `kAXValueAttribute` |
| Images | ✅ `.png/.tiff/.jpeg` types | ❌ **Not supported** |
| File References | ✅ `public.file-url` type | ❌ **Not supported** |
| Rich Text w/ media | ✅ `.rtf` with embedded images | ❌ Text only |

**Critical Insight:** Media content (images, files) can ONLY be captured through the clipboard path, never through Accessibility API.

### Acquisition Method by Scenario

| Scenario | Acquisition Path | Media Available? |
|----------|-----------------|------------------|
| User selects image, presses hotkey | Cmd+C → Clipboard | ✅ Yes |
| User selects file in Finder | Cmd+C → Clipboard | ✅ Yes (file URL) |
| No selection, window has image | Accessibility API | ❌ **No** |
| No selection, Cmd+A fallback | Cmd+A → Cmd+C → Clipboard | ✅ Yes |
| Notes/Mail with embedded images | Cmd+C → Clipboard | ✅ Yes (mixed content) |

**Sources:**
- [NSPasteboard Documentation](https://developer.apple.com/documentation/appkit/nspasteboard)
- [Maccy Clipboard Implementation](https://github.com/p0deje/Maccy/blob/master/Maccy/Clipboard.swift)
- [Apple Developer Forums: Pasteboard Types](https://developer.apple.com/forums/thread/77926)

## What Changes

### Data Structure Enhancements
- **ADDED**: `MediaAttachment` type for images, videos, and files
- **ADDED**: `MultimodalInput` type combining text + attachments
- **ADDED**: `attachments` field to `CapturedContext` or a new context type
- **MODIFIED**: `process_input` signature to accept multimodal content

### Content Ordering (User Requirement)
The data fed to AI follows this sequence:
1. Window text content (may contain commands like `/en`)
2. Clipboard text content
3. Media attachments (images/videos/files from window + clipboard combined)

### Swift Layer
- **MODIFIED**: `ClipboardManager` to detect and extract image/file content
- **ADDED**: Image data conversion to Base64 for AI provider consumption
- **ADDED**: File type detection (image, video, text file)
- **ADDED**: `getFileURLs()` method to extract file references from clipboard
- **ADDED**: File content loading from disk for Finder-copied files
- **ADDED**: Mixed content extraction for Notes/Mail (text + embedded images)

### Rust Layer
- **MODIFIED**: `process_input` to handle `MultimodalInput`
- **MODIFIED**: Provider routing to pass media to vision-capable providers
- **MODIFIED**: Payload assembly to include media in context

## Impact

### Affected specs
- `clipboard-management` - Add media extraction capabilities
- `context-capture` - Extend with attachment support
- `uniffi-bridge` - Add new types for media transfer
- `core-library` - Modify process_input signature
- `content-extractor` - **NEW** Extensible content extractor architecture

### Affected code
- `Aleph/Sources/Utils/ClipboardManager.swift`
- `Aleph/Sources/AppDelegate.swift`
- `Aleph/core/src/core.rs`
- `Aleph/core/src/aleph.udl`
- `Aleph/core/src/payload/mod.rs`
- `Aleph/core/src/providers/openai.rs`
- `Aleph/core/src/providers/claude.rs`

### Breaking Changes
- **BREAKING**: `process_input` API signature change (requires Swift layer update)
- Backward compatibility: If no media provided, behavior identical to current

## Phase 1 Scope (MVP)
Focus on image support only:
1. Detect images in clipboard
2. Convert to Base64 for provider APIs
3. Route to vision-capable providers (OpenAI GPT-4o, Claude 3.5)
4. Return text response

## Future Phases (Out of Scope)
- Video frame extraction
- Text file content extraction
- PDF document parsing
- Multiple file batching
