# Design: Multimodal Content Support

## Context

Aether is a system-level AI middleware that captures user content via clipboard and routes to AI providers. The current architecture only supports text input, but modern vision-language models (GPT-4o, Claude 3.5, Gemini) can process images. Users need to analyze screenshots, diagrams, and photos directly from their workflow.

**Stakeholders:**
- End users: Need frictionless image analysis
- AI providers: Already support vision APIs
- Rust core: Needs multimodal data structures
- Swift UI: Needs media capture capabilities

**Constraints:**
- Must maintain "Ghost" aesthetic (no dialog popups for file selection)
- Must preserve focus protection (no window stealing during capture)
- Must work with existing hotkey flow
- Image size limits: OpenAI (20MB), Claude (varies by model)

## Content Acquisition Methods Analysis

Aether uses two different methods to capture window content, and they have **fundamentally different capabilities** for media content:

### Method 1: Command+C/X (Clipboard-based)

Used when user has selected content before pressing hotkey.

| Content Type | Pasteboard Type | Capability |
|-------------|-----------------|------------|
| Text | `NSPasteboard.PasteboardType.string` | ✅ Full support |
| Images | `.png`, `.tiff`, `.jpeg` | ✅ Full support |
| File References | `public.file-url` | ✅ Can read file URLs |
| Rich Text | `.rtf` | ✅ Full support |

**How it works:**
```swift
// Detect content types
NSPasteboard.general.types  // Returns available types

// Read file URLs (for copied files in Finder)
pasteboard.readObjects(forClasses: [NSURL.self], options: [
    .urlReadingFileURLsOnly: true
])
```

### Method 2: Accessibility API (Silent read)

Used when no selection detected - reads focused element content directly.

| Content Type | API Attribute | Capability |
|-------------|---------------|------------|
| Text | `kAXValueAttribute`, `kAXSelectedTextAttribute` | ✅ Supported |
| Images | N/A | ❌ **NOT SUPPORTED** |
| Files | N/A | ❌ **NOT SUPPORTED** |

**Critical Limitation:** Accessibility API only returns String values. It cannot read images, file references, or binary data.

### Implications for Multimodal Support

| Scenario | Acquisition Method | Media Available? |
|----------|-------------------|------------------|
| User selects image, presses hotkey | Cmd+C → Clipboard | ✅ Yes |
| User selects file in Finder, presses hotkey | Cmd+C → Clipboard | ✅ Yes (file URL) |
| No selection, window has image | Accessibility API | ❌ **No** |
| No selection, Cmd+A fallback | Cmd+A → Cmd+C → Clipboard | ✅ Yes |

**Key Insight:** Media content (images, files) can ONLY be captured through the clipboard path, never through Accessibility API.

### File URL Processing

When files are copied (e.g., from Finder), the clipboard contains file URLs, not file content:

```swift
// Clipboard contains: public.file-url → "file:///Users/.../image.png"
// Need to:
// 1. Extract file URL from clipboard
// 2. Determine file type from extension
// 3. Load file content
// 4. Convert to MediaAttachment
```

**Supported file types for Phase 1:**
- Images: `.png`, `.jpg`, `.jpeg`, `.gif`, `.webp`
- (Phase 2: `.pdf`, `.txt`, `.md`, video files)

### Content Acquisition Scenarios

Based on the two acquisition methods, here are the key scenarios Aether must handle:

**Scenario A: User selects image in application and presses hotkey**
```
User Flow: Select image → Press hotkey
Acquisition: Cmd+C → Clipboard
Clipboard contains: .png/.jpeg/.tiff data
Action: ClipboardManager detects image type → Extract → Convert to Base64
Result: ✅ Image captured successfully
```

**Scenario B: User in image viewer without selection**
```
User Flow: Open Preview with image → Press hotkey (no selection)
Acquisition: Accessibility API (kAXValueAttribute)
Clipboard contains: Only text (window title, filename, etc.)
Action: Accessibility API returns string only
Result: ❌ Cannot capture image! Only text metadata available
Fallback: Cmd+A → Cmd+C captures entire content if available
```

**Scenario C: User selects file(s) in Finder and presses hotkey**
```
User Flow: Select file in Finder → Press hotkey
Acquisition: Cmd+C → Clipboard
Clipboard contains: public.file-url → "file:///Users/.../image.png"
Action:
  1. ClipboardManager.getFileURLs() extracts file paths
  2. Determine file type from extension
  3. Load file content from disk
  4. Convert to MediaAttachment
Result: ✅ File content captured (if supported type)
```

**Scenario D: User copies content in Notes/Mail with embedded attachments**
```
User Flow: Select text+image in Notes → Press hotkey
Acquisition: Cmd+C → Clipboard
Clipboard contains (multiple representations):
  - .string (plain text, images stripped)
  - .rtf (rich text without embedded binary)
  - .rtfd (Rich Text Format Directory - contains embedded attachments!)
  - .png/.jpeg (may or may not be present separately)
  - web archive (another representation)

Action:
  1. Check if .png/.jpeg types present → extract directly (preferred, simpler)
  2. If no direct image types, check for .rtfd type
  3. Parse RTFD via NSAttributedString → enumerate NSTextAttachment
  4. Extract image data from attachment.fileWrapper.regularFileContents
  5. ClipboardManager.getText() gets plain text from .string
  6. Combine text + extracted images in result

Result: ✅ Text + images captured together
```

**RTFD Parsing Implementation Detail:**
```swift
// Extract embedded images from RTFD data
func extractImagesFromRTFD(_ rtfdData: Data) -> [MediaAttachment] {
    var attachments: [MediaAttachment] = []

    guard let attrString = try? NSAttributedString(
        data: rtfdData,
        options: [.documentType: NSAttributedString.DocumentType.rtfd],
        documentAttributes: nil
    ) else { return [] }

    attrString.enumerateAttribute(.attachment, in: NSRange(location: 0, length: attrString.length)) { value, _, _ in
        if let textAttachment = value as? NSTextAttachment,
           let fileWrapper = textAttachment.fileWrapper,
           let data = fileWrapper.regularFileContents,
           let filename = fileWrapper.preferredFilename {

            // Only process images in Phase 1
            let ext = (filename as NSString).pathExtension.lowercased()
            if ["png", "jpg", "jpeg", "gif", "webp"].contains(ext) {
                let mimeType = mimeTypeForExtension(ext)
                attachments.append(MediaAttachment(
                    media_type: "image",
                    mime_type: mimeType,
                    data: data.base64EncodedString(),
                    filename: filename,
                    size_bytes: UInt64(data.count)
                ))
            }
        }
    }
    return attachments
}
```

**Known Issue:** When converting RTFD back to NSAttributedString, `attachment.contents` may be empty. Must use `fileWrapper.regularFileContents` instead.

### Design Implications

1. **ClipboardManager needs multiple content extraction**:
   - `getText()` - existing
   - `getImageAsBase64()` - new for Phase 1
   - `getFileURLs()` - new for file references
   - `getMixedContent()` - for Notes/Mail scenarios

2. **Accessibility API path is text-only**:
   - When Accessibility API is used (no selection), only text can be captured
   - UI should not indicate "image capture" is possible in this mode
   - Future: Could use Cmd+A fallback to get full content

3. **File URL processing adds file system dependency**:
   - Need to load file content from disk
   - Must handle permission errors gracefully
   - Must validate file type before loading

## Extensible Content Extractor Architecture

### Design Principles
- **Open/Closed Principle**: Open for extension, closed for modification
- **Single Responsibility**: Each extractor handles one content type
- **Dependency Inversion**: Depend on abstractions (protocols), not concrete implementations
- **Plugin Architecture**: New extractors can be added without modifying existing code

### Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    ClipboardManager                          │
│  ┌─────────────────────────────────────────────────────┐    │
│  │           ContentExtractorRegistry                   │    │
│  │  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐   │    │
│  │  │ Direct  │ │  RTFD   │ │ FileURL │ │  Future │   │    │
│  │  │ Image   │ │Extractor│ │Extractor│ │Extractor│   │    │
│  │  │Extractor│ │         │ │         │ │  (PDF)  │   │    │
│  │  └────┬────┘ └────┬────┘ └────┬────┘ └────┬────┘   │    │
│  │       │           │           │           │         │    │
│  │       └───────────┴───────────┴───────────┘         │    │
│  │                        ▼                             │    │
│  │              ContentExtractor Protocol               │    │
│  └─────────────────────────────────────────────────────┘    │
│                           ▼                                  │
│                  getMixedContent() → (text, attachments)     │
└─────────────────────────────────────────────────────────────┘
```

### ContentExtractor Protocol

```swift
/// Protocol for pluggable content extractors
protocol ContentExtractor {
    /// Unique identifier for this extractor
    var identifier: String { get }

    /// Priority (lower = higher priority, executed first)
    /// Recommended ranges:
    ///   0-19: Direct types (png, jpeg) - fastest path
    ///  20-39: Rich formats (RTFD, HTML) - requires parsing
    ///  40-59: File references - requires disk I/O
    ///  60-79: Network resources - requires network I/O
    ///  80-99: Fallback extractors
    var priority: Int { get }

    /// Supported pasteboard types this extractor can handle
    var supportedTypes: [NSPasteboard.PasteboardType] { get }

    /// Check if this extractor can process the current pasteboard
    func canExtract(from pasteboard: NSPasteboard) -> Bool

    /// Extract content from pasteboard
    /// - Returns: Extraction result with text, attachments, and metadata
    func extract(from pasteboard: NSPasteboard) -> ExtractionResult
}

/// Result of a content extraction operation
struct ExtractionResult {
    let text: String?
    let attachments: [MediaAttachment]
    let handledTypes: Set<NSPasteboard.PasteboardType>
    let metadata: [String: Any]  // For debugging/logging
}
```

### ContentExtractorRegistry

```swift
/// Central registry for content extractors
/// Manages extractor lifecycle and orchestrates extraction
final class ContentExtractorRegistry {
    static let shared = ContentExtractorRegistry()

    private var extractors: [ContentExtractor] = []
    private let queue = DispatchQueue(label: "extractor.registry")

    /// Register a new extractor
    func register(_ extractor: ContentExtractor) {
        queue.sync {
            extractors.append(extractor)
            extractors.sort { $0.priority < $1.priority }
        }
    }

    /// Unregister an extractor by identifier
    func unregister(identifier: String) {
        queue.sync {
            extractors.removeAll { $0.identifier == identifier }
        }
    }

    /// Extract all content from pasteboard using registered extractors
    func extractAll(from pasteboard: NSPasteboard) -> (text: String?, attachments: [MediaAttachment]) {
        var allAttachments: [MediaAttachment] = []
        var text: String?
        var handledTypes: Set<NSPasteboard.PasteboardType> = []

        for extractor in extractors {
            // Skip if this extractor's types are already handled
            let extractorTypes = Set(extractor.supportedTypes)
            if !extractorTypes.isDisjoint(with: handledTypes) {
                continue  // Already handled by higher priority extractor
            }

            if extractor.canExtract(from: pasteboard) {
                let result = extractor.extract(from: pasteboard)
                if text == nil { text = result.text }
                allAttachments.append(contentsOf: result.attachments)
                handledTypes.formUnion(result.handledTypes)

                log.debug("[\(extractor.identifier)] extracted \(result.attachments.count) attachments")
            }
        }

        return (text, allAttachments)
    }
}
```

### Built-in Extractors (Phase 1)

```swift
/// Extractor for direct image types (.png, .jpeg, .tiff)
final class DirectImageExtractor: ContentExtractor {
    let identifier = "direct-image"
    let priority = 10  // Highest priority - fastest path
    let supportedTypes: [NSPasteboard.PasteboardType] = [.png, .jpeg, .tiff]

    func canExtract(from pasteboard: NSPasteboard) -> Bool {
        pasteboard.types?.contains(where: supportedTypes.contains) ?? false
    }

    func extract(from pasteboard: NSPasteboard) -> ExtractionResult {
        // Extract image data directly from pasteboard
    }
}

/// Extractor for RTFD (Rich Text Format Directory) with embedded attachments
final class RTFDExtractor: ContentExtractor {
    let identifier = "rtfd"
    let priority = 20  // After direct images
    let supportedTypes: [NSPasteboard.PasteboardType] = [.rtfd]

    func canExtract(from pasteboard: NSPasteboard) -> Bool {
        pasteboard.types?.contains(.rtfd) ?? false
    }

    func extract(from pasteboard: NSPasteboard) -> ExtractionResult {
        // Parse RTFD and extract NSTextAttachment images
    }
}

/// Extractor for file URLs (Finder copied files)
final class FileURLExtractor: ContentExtractor {
    let identifier = "file-url"
    let priority = 40  // Requires disk I/O
    let supportedTypes: [NSPasteboard.PasteboardType] = [
        NSPasteboard.PasteboardType("public.file-url")
    ]

    func canExtract(from pasteboard: NSPasteboard) -> Bool {
        // Check for file URL type
    }

    func extract(from pasteboard: NSPasteboard) -> ExtractionResult {
        // Load file content from disk
    }
}

/// Extractor for plain text (fallback)
final class PlainTextExtractor: ContentExtractor {
    let identifier = "plain-text"
    let priority = 80  // Fallback
    let supportedTypes: [NSPasteboard.PasteboardType] = [.string]

    func canExtract(from pasteboard: NSPasteboard) -> Bool {
        pasteboard.types?.contains(.string) ?? false
    }

    func extract(from pasteboard: NSPasteboard) -> ExtractionResult {
        // Extract plain text
    }
}
```

### Registration at App Startup

```swift
// In AppDelegate or ClipboardManager initialization
func setupContentExtractors() {
    let registry = ContentExtractorRegistry.shared

    // Phase 1: Image support
    registry.register(DirectImageExtractor())
    registry.register(RTFDExtractor())
    registry.register(FileURLExtractor())
    registry.register(PlainTextExtractor())

    // Phase 2: Additional formats (future)
    // registry.register(PDFExtractor())
    // registry.register(VideoExtractor())
    // registry.register(HTMLExtractor())
}
```

### Adding New Extractors (Future)

To add support for a new content type, simply:

1. **Create a new extractor class** implementing `ContentExtractor`
2. **Set appropriate priority** based on extraction cost
3. **Register in startup** - no changes to existing code needed

```swift
// Example: Adding PDF support in Phase 2
final class PDFExtractor: ContentExtractor {
    let identifier = "pdf"
    let priority = 25  // Between RTFD and FileURL
    let supportedTypes: [NSPasteboard.PasteboardType] = [.pdf]

    func canExtract(from pasteboard: NSPasteboard) -> Bool {
        pasteboard.types?.contains(.pdf) ?? false
    }

    func extract(from pasteboard: NSPasteboard) -> ExtractionResult {
        // Extract PDF and convert to images or text
    }
}

// Example: Adding video thumbnail extraction
final class VideoThumbnailExtractor: ContentExtractor {
    let identifier = "video-thumbnail"
    let priority = 45
    let supportedTypes: [NSPasteboard.PasteboardType] = [
        NSPasteboard.PasteboardType("public.movie")
    ]

    func canExtract(from pasteboard: NSPasteboard) -> Bool {
        // Check for video types
    }

    func extract(from pasteboard: NSPasteboard) -> ExtractionResult {
        // Extract thumbnail frame from video
    }
}

// Just register - no other code changes needed
registry.register(PDFExtractor())
registry.register(VideoThumbnailExtractor())
```

### Benefits of This Architecture

| Benefit | Description |
|---------|-------------|
| **Low Coupling** | Extractors don't know about each other |
| **High Cohesion** | Each extractor handles one content type |
| **Easy Testing** | Each extractor can be unit tested independently |
| **Priority Control** | Fine-grained control over extraction order |
| **No Code Changes** | Adding new types doesn't touch existing extractors |
| **Runtime Registration** | Extractors can be added/removed dynamically |
| **Graceful Degradation** | Failed extractors don't affect others |

## Goals / Non-Goals

**Goals:**
1. Enable image capture from clipboard and route to vision-capable AI
2. Maintain backward compatibility with text-only workflows
3. Keep data structure extensible for future media types (video, files)
4. Preserve existing content ordering (window text → clipboard text → media)

**Non-Goals:**
1. Video transcoding or frame extraction (Phase 2)
2. Text file parsing (Phase 2)
3. Multiple file batch processing (Phase 2)
4. Image editing or annotation
5. Local image storage/caching

## Decisions

### Decision 1: Media Data Transfer via UniFFI

**What:** Transfer media as `MediaAttachment` dictionary with Base64-encoded data through UniFFI.

**Why:**
- UniFFI doesn't support raw byte arrays directly across FFI boundary
- Base64 is already used by AI provider APIs
- Encoding can happen in Swift layer where NSImage is available
- Avoids memory management complexity at FFI boundary

**Alternatives considered:**
1. **Raw bytes via `sequence<u8>`**: UniFFI supports but inefficient for large images
2. **File path transfer**: Rust reads file - adds file system dependency to core
3. **Shared memory**: Complex, overkill for single-image use case

### Decision 2: Input Data Structure

**What:** Extend `CapturedContext` with optional `attachments` field rather than creating new type.

```
dictionary CapturedContext {
    string app_bundle_id;
    string? window_title;
    sequence<MediaAttachment>? attachments;  // NEW
};

dictionary MediaAttachment {
    string media_type;   // "image", "video", "file"
    string mime_type;    // "image/png", "image/jpeg"
    string data;         // Base64-encoded content
    string? filename;    // Optional original filename
    u64 size_bytes;      // Original size for logging
};
```

**Why:**
- Minimal API surface change
- Context captures full input state at hotkey moment
- Attachments naturally belong with context metadata
- Easier migration path for existing code

**Alternatives considered:**
1. **New `MultimodalInput` wrapper**: More breaking changes, less cohesive
2. **Separate `process_input_with_media` method**: API fragmentation

### Decision 3: Content Ordering in Payload

**What:** Assemble user message content in this order:
1. Window text content (command prefix + text)
2. Clipboard text content (appended after)
3. Media attachments (added to multimodal content blocks)

**Why:**
- Matches user expectation: "I see this text, I have this in clipboard, plus these attachments"
- Routing rules (regex) match against text content first
- Media provides supplementary context rather than primary input
- Consistent with current AppDelegate implementation

### Decision 4: Provider Selection for Vision

**What:** Route to vision-capable provider when image is present.

**Routing logic:**
1. If attachments contain images AND matched rule's provider supports vision → use that provider
2. If attachments contain images BUT provider doesn't support vision → fallback to default vision provider
3. If no images → existing routing logic unchanged

**Vision-capable providers (initial):**
- OpenAI: GPT-4o, GPT-4-turbo
- Claude: claude-3-5-sonnet-*, claude-3-opus-*, claude-3-haiku-*
- Gemini: gemini-1.5-pro, gemini-1.5-flash (future)
- Ollama: llava, bakllava (future)

## Risks / Trade-offs

### Risk: Large Image Memory Usage
- **Issue**: Base64 encoding increases size by ~33%
- **Mitigation**: Log image size, warn if > 5MB, fail if > 20MB
- **Trade-off**: Accept memory overhead for simplicity

### Risk: Provider Capability Mismatch
- **Issue**: User routes to non-vision provider with image
- **Mitigation**: Auto-fallback to vision-capable default provider
- **Trade-off**: May surprise user if default differs from rule target

### Risk: FFI Data Transfer Latency
- **Issue**: Large Base64 strings through UniFFI may be slow
- **Mitigation**: Profile actual latency, optimize if > 100ms
- **Trade-off**: Accept some latency for architectural simplicity

## Migration Plan

### Phase 1: API Extension (This Change)
1. Add `MediaAttachment` dictionary to aether.udl
2. Add optional `attachments` to `CapturedContext`
3. Modify `process_input` to check for attachments
4. Update Swift layer to capture and encode images
5. Wire through to provider APIs

### Rollback
- Remove attachments field from context
- Swift layer gracefully handles nil attachments
- No data migration needed (stateless)

## Open Questions

1. **Should we compress images before Base64?**
   - Pro: Smaller transfer size
   - Con: Added complexity, lossy
   - Current answer: No, send as-is for MVP

2. **How to handle multiple images?**
   - OpenAI supports multiple images in one message
   - Claude supports multiple images
   - Current answer: Support sequence, let provider handle limits

3. **Should video be treated as image sequence?**
   - Deferred to Phase 2
   - Could extract keyframes and process as images
