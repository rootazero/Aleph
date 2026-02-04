# Tasks: Add Multimodal Content Support

## 1. UniFFI Interface Definition

- [x] 1.1 Add `MediaAttachment` dictionary to `aleph.udl`:
  - `media_type: string`
  - `mime_type: string`
  - `data: string` (Base64)
  - `filename: string?`
  - `size_bytes: u64`

- [x] 1.2 Modify `CapturedContext` dictionary in `aleph.udl`:
  - Add `attachments: sequence<MediaAttachment>?`

- [x] 1.3 Regenerate UniFFI bindings:
  - Run `cargo run --bin uniffi-bindgen generate src/aleph.udl --language swift --out-dir ../Sources/Generated/`
  - Copy updated `aleph.swift` to Swift project

## 2. Content Extractor Architecture (Extensible Plugin System)

- [x] 2.1 Define `ContentExtractor` protocol:
  - `identifier: String` property
  - `priority: Int` property (lower = higher priority)
  - `supportedTypes: [NSPasteboard.PasteboardType]` property
  - `canExtract(from: NSPasteboard) -> Bool` method
  - `extract(from: NSPasteboard) -> ExtractionResult` method

- [x] 2.2 Define `ExtractionResult` struct:
  - `text: String?`
  - `attachments: [MediaAttachment]`
  - `handledTypes: Set<NSPasteboard.PasteboardType>`
  - `metadata: [String: Any]`

- [x] 2.3 Implement `ContentExtractorRegistry`:
  - Singleton pattern with `shared` instance
  - `register(_ extractor:)` method with priority sorting
  - `unregister(identifier:)` method
  - `extractAll(from:)` method with priority-based execution
  - Thread-safe via DispatchQueue
  - Skip already-handled types

- [x] 2.4 Add shared utilities:
  - `SupportedMediaType` enum (png, jpg, jpeg, gif, webp, tiff)
  - `maxImageSizeBytes = 20 * 1024 * 1024` (20MB)
  - `warnImageSizeBytes = 5 * 1024 * 1024` (5MB)
  - `mimeType(for extension:) -> String` helper
  - `isSupported(fileExtension:) -> Bool` helper

## 3. Built-in Extractors (Phase 1)

- [x] 3.1 Implement `DirectImageExtractor`:
  - identifier = "direct-image"
  - priority = 10 (highest - fastest path)
  - supportedTypes = [.png, .jpeg, .tiff]
  - Extract image data directly from pasteboard
  - Convert to Base64 with data URI prefix
  - Validate size limits

- [x] 3.2 Implement `RTFDExtractor`:
  - identifier = "rtfd"
  - priority = 20
  - supportedTypes = [.rtfd]
  - Parse RTFD via NSAttributedString
  - Enumerate NSTextAttachment attributes
  - Extract image data from `fileWrapper.regularFileContents`
  - Get filename from `fileWrapper.preferredFilename`
  - Filter to Phase 1 supported image types only
  - Handle parsing errors gracefully

- [x] 3.3 Implement `FileURLExtractor`:
  - identifier = "file-url"
  - priority = 40 (requires disk I/O)
  - supportedTypes = [public.file-url]
  - Use `pasteboard.readObjects(forClasses: [NSURL.self])`
  - Check file extension is supported
  - Load file content from disk
  - Validate file size
  - Convert to MediaAttachment
  - Skip unsupported types with info log

- [x] 3.4 Implement `PlainTextExtractor`:
  - identifier = "plain-text"
  - priority = 80 (fallback)
  - supportedTypes = [.string]
  - Extract plain text from pasteboard
  - Return text with empty attachments

## 4. ClipboardManager Integration

- [x] 4.1 Add `setupContentExtractors()` initialization:
  - Create and register all Phase 1 extractors
  - Call during ClipboardManager init or app startup

- [x] 4.2 Refactor `getMixedContent()`:
  - Delegate to `ContentExtractorRegistry.shared.extractAll()`
  - Keep backward-compatible API signature
  - Return tuple (text: String?, attachments: [MediaAttachment])

- [x] 4.3 Add legacy method wrappers (if needed):
  - `getImageAsBase64()` → calls DirectImageExtractor
  - `hasFileURLs()` → checks pasteboard types
  - `getFileURLs()` → calls FileURLExtractor

## 5. Swift AppDelegate Integration

- [x] 5.1 Modify `handleHotkeyPressed()` to capture media:
  - Call `ClipboardManager.shared.getMixedContent()`
  - Store attachments for context creation
  - Track acquisition method ("clipboard", "accessibility", "fallback_clipboard")

- [x] 5.2 Modify `processAfterSelection()` context creation:
  - Create `CapturedContext` with attachments field
  - Include media attachments if present

- [x] 5.3 Update logging:
  - Log extractor identifier and attachment count
  - Log image size and type when captured
  - Log "no media" when attachments empty
  - Log acquisition method for debugging

## 6. Rust Core Data Structures

- [x] 6.1 Add `MediaAttachment` struct to `core.rs`:
  - Mirror UniFFI dictionary fields
  - Implement Clone, Debug

- [x] 6.2 Modify `CapturedContext` struct:
  - Add `attachments: Option<Vec<MediaAttachment>>`

- [x] 6.3 Update `AgentContext` in `payload/mod.rs`:
  - Add `attachments: Option<Vec<MediaAttachment>>`

- [x] 6.4 Update `PayloadBuilder` to include attachments:
  - Extract attachments from `CapturedContext`
  - Pass to `AgentContext`

## 7. Rust Provider Integration

- [x] 7.1 Add vision capability detection:
  - Create `VisionCapability` trait or method
  - OpenAI: gpt-4o, gpt-4-turbo → supports vision
  - Claude: claude-3-* models → supports vision
  - Ollama: llava, bakllava → supports vision

- [x] 7.2 Modify OpenAI provider:
  - Update `process_input` to check for attachments
  - If image present, use `build_multimodal_request`
  - Parse attachments from AgentContext

- [x] 7.3 Modify Claude provider:
  - Update `process_input` to check for attachments
  - If image present, use `build_multimodal_request`
  - Parse attachments from AgentContext

- [x] 7.4 Implement vision provider fallback:
  - In router, check if routed provider supports vision
  - If not and image present, fallback to default vision provider
  - Log fallback decision

## 8. Testing

### Unit Tests
- [x] 8.1 Unit tests for ContentExtractor protocol compliance
- [x] 8.2 Unit tests for ContentExtractorRegistry (register, unregister, priority order)
- [x] 8.3 Unit tests for DirectImageExtractor
- [x] 8.4 Unit tests for RTFDExtractor
- [x] 8.5 Unit tests for FileURLExtractor
- [x] 8.6 Unit tests for PlainTextExtractor
- [x] 8.7 Unit tests for MediaAttachment parsing (Rust)

### Integration Tests
- [x] 8.8 Integration test: text + direct image to OpenAI
- [x] 8.9 Integration test: text + direct image to Claude
- [x] 8.10 Integration test: Notes copy with embedded image (RTFD path)
- [x] 8.11 Integration test: RTFD with multiple images
- [x] 8.12 Integration test: RTFD with mixed types (image + pdf + video)
- [x] 8.13 Integration test: Finder file selection → image load
- [x] 8.14 Integration test: image too large (should log warning)
- [x] 8.15 Integration test: unsupported file type (should skip with log)
- [x] 8.16 Integration test: text-only (backward compatibility)
- [x] 8.17 Integration test: Accessibility API path (verify no media)
- [x] 8.18 Integration test: Extractor failure isolation

## 9. Documentation

- [x] 9.1 Update CLAUDE.md with multimodal support notes
- [x] 9.2 Add usage examples in docs/
- [x] 9.3 Document supported image formats and size limits
- [x] 9.4 Document how to add new ContentExtractor (extension guide)

## Dependencies

```
Task 1 (UniFFI) ─────────────────────────────────────────────┐
                                                              │
Task 2 (Protocol/Registry) ──┬──────────────────────────────┼──> Task 4 (ClipboardManager)
                             │                               │
Task 3 (Built-in Extractors) ┘                               │
                                                              │
Task 6 (Rust Core) ──────────────────────────────────────────┤
                                                              │
                                                              ├──> Task 5 (AppDelegate)
Task 7 (Rust Providers) ─────────────────────────────────────┤
                                                              │
                                                              └──> Task 8 (Testing)

Task 9 (Docs) can run in parallel after Task 1
```

## Parallelizable Work

These can be done simultaneously:
- Task 2 (Protocol) and Task 6 (Rust Core) after Task 1
- All Task 3.* (Extractors) after Task 2 is complete
- Task 7.2 (OpenAI) and Task 7.3 (Claude) after Task 7.1
- All Task 8.* tests after respective implementation
- Task 9 (Docs) can start after Task 1

## Key Implementation Notes

### Extensibility Benefits

Adding new content types in Phase 2+ requires only:

1. Create new extractor class implementing `ContentExtractor`
2. Set appropriate priority
3. Register in `setupContentExtractors()`

No changes needed to:
- Existing extractors
- ClipboardManager
- AppDelegate
- Rust Core
- Providers

### Priority Ranges

| Range | Category | Examples |
|-------|----------|----------|
| 0-19 | Direct types | DirectImageExtractor (10) |
| 20-39 | Rich formats | RTFDExtractor (20), HTMLExtractor |
| 40-59 | File references | FileURLExtractor (40) |
| 60-79 | Network resources | URLImageExtractor |
| 80-99 | Fallbacks | PlainTextExtractor (80) |

### Content Acquisition Method Differences

1. **Clipboard path** (Cmd+C/X): Full media support
   - All extractors can contribute
   - Use `getMixedContent()` for comprehensive extraction

2. **Accessibility API path**: Text only
   - Only PlainTextExtractor relevant
   - Attachments will be empty
