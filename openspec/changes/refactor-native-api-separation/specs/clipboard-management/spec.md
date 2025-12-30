# clipboard-management Spec Delta (refactor-native-api-separation)

## MODIFIED Requirements

### Requirement: Clipboard Text Operations
The system SHALL provide text read/write operations using native platform APIs (NSPasteboard on macOS).

**Previous**: Used `arboard` crate in Rust layer for cross-platform clipboard access.
**Changed to**: Use native `NSPasteboard` API in Swift layer for better type support and zero FFI overhead.

#### Scenario: Read text from clipboard (MODIFIED)
- **WHEN** Swift calls `ClipboardManager.getText()`
- **THEN** calls `NSPasteboard.general.string(forType: .string)`
- **AND** returns `String?` (nil if no text)
- **AND** no FFI call to Rust
- **AND** completes within < 5ms

#### Scenario: Write text to clipboard (MODIFIED)
- **WHEN** Swift calls `ClipboardManager.setText(_:)`
- **THEN** clears pasteboard with `NSPasteboard.general.clearContents()`
- **AND** writes string with `setString(_:forType: .string)`
- **AND** no FFI call to Rust
- **AND** completes within < 5ms

### Requirement: Clipboard Image Operations
The system SHALL support image read/write operations using native NSImage type.

**Previous**: Used `arboard::ImageData` with format detection and binary conversion.
**Changed to**: Use `NSImage` directly with `NSPasteboard.readObjects()` and `writeObjects()`.

#### Scenario: Read image from clipboard (MODIFIED)
- **WHEN** Swift calls `ClipboardManager.getImage()`
- **THEN** calls `NSPasteboard.general.readObjects(forClasses: [NSImage.self])`
- **AND** returns `NSImage?` (nil if no image)
- **AND** automatically handles PNG, JPEG, TIFF, GIF formats
- **AND** no manual format detection needed
- **AND** no FFI call to Rust

#### Scenario: Write image to clipboard (MODIFIED)
- **WHEN** Swift calls `ClipboardManager.setImage(_:)`
- **THEN** clears pasteboard with `clearContents()`
- **AND** writes image with `writeObjects([image])`
- **AND** preserves original image format
- **AND** no FFI call to Rust

#### Scenario: Check if clipboard contains image (MODIFIED)
- **WHEN** Swift calls `ClipboardManager.hasImage()`
- **THEN** checks if `NSPasteboard.general.types` contains `.tiff` or `.png`
- **AND** returns `Bool` immediately
- **AND** no FFI call to Rust

## REMOVED Requirements

### Requirement: Cross-Platform Clipboard Abstraction (REMOVED)
**Reason**: Clipboard operations are now platform-specific. Each platform uses its native clipboard API.

**Previous scenarios removed**:
- arboard-based clipboard initialization
- Platform-agnostic image format detection
- UniFFI-based clipboard data transfer

### Requirement: Rust Clipboard Manager Trait (REMOVED)
**Reason**: Clipboard manager is no longer in Rust core. Swift implements `ClipboardManager` as a concrete class.

**Previous scenarios removed**:
- Implement ClipboardManager trait
- Mock clipboard in Rust tests

## ADDED Requirements

### Requirement: Native macOS Clipboard Implementation
The system SHALL use NSPasteboard API for all clipboard operations on macOS, providing complete clipboard type support.

#### Scenario: Support all NSPasteboard types
- **WHEN** reading from clipboard
- **THEN** supports `.string` (plain text)
- **AND** supports `.tiff`, `.png` (images)
- **AND** supports `.rtf` (rich text format) via `data(forType: .rtf)`
- **AND** supports `.pdf` (PDF documents)
- **AND** supports `.URL` (URLs)
- **AND** can extend to custom UTI types

#### Scenario: Detect clipboard changes
- **WHEN** Swift calls `ClipboardManager.changeCount()`
- **THEN** returns `NSPasteboard.general.changeCount`
- **AND** increments whenever clipboard content changes (by any app)
- **AND** can be used to detect if clipboard was modified externally

#### Scenario: Get RTF content (advanced)
- **WHEN** Swift calls `ClipboardManager.getRTF()`
- **THEN** returns `Data?` containing RTF content
- **AND** preserves rich text formatting
- **AND** can be used for AI context (formatted text input)

#### Scenario: Zero-copy clipboard access
- **WHEN** performing any clipboard operation
- **THEN** NO data is copied across FFI boundary
- **AND** NO type conversion between Rust and Swift types
- **AND** operations complete in < 5ms (p95)

### Requirement: Clipboard Ownership
The Swift layer SHALL own all clipboard operations, with Rust core having no direct clipboard access.

#### Scenario: Swift handles clipboard lifecycle
- **WHEN** processing user input
- **THEN** Swift reads clipboard BEFORE calling Rust
- **AND** Rust receives pre-processed input string
- **AND** Rust does NOT call clipboard APIs

#### Scenario: No UniFFI clipboard methods
- **WHEN** defining AetherCore interface
- **THEN** `get_clipboard_text()` is REMOVED from UniFFI
- **AND** `read_clipboard_image()` is REMOVED from UniFFI
- **AND** `write_clipboard_image()` is REMOVED from UniFFI
- **AND** reduces UniFFI interface surface area

---

**Related specs**: `macos-client`, `uniffi-bridge`, `core-library`
**Relationship**: This change enables `macos-client` to implement native clipboard management, and removes clipboard-related interfaces from `uniffi-bridge`.
