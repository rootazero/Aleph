# clipboard-management Spec Delta

## ADDED Requirements

### Requirement: Image Content Detection
The ClipboardManager SHALL detect when the clipboard contains image data.

#### Scenario: Detect PNG image in clipboard
- **GIVEN** user has copied a PNG screenshot
- **WHEN** ClipboardManager.hasImage() is called
- **THEN** returns true
- **AND** contentType() returns .image

#### Scenario: Detect JPEG image in clipboard
- **GIVEN** user has copied a JPEG photo
- **WHEN** ClipboardManager.hasImage() is called
- **THEN** returns true

#### Scenario: No image in clipboard
- **GIVEN** clipboard contains only text
- **WHEN** ClipboardManager.hasImage() is called
- **THEN** returns false

---

### Requirement: Image Content Extraction
The ClipboardManager SHALL extract image data and convert to Base64 format for AI provider APIs.

#### Scenario: Extract image as Base64
- **GIVEN** clipboard contains a PNG image
- **WHEN** ClipboardManager.getImageAsBase64() is called
- **THEN** returns Base64-encoded string with data URI prefix
- **AND** format is "data:image/png;base64,..."
- **AND** operation completes within 200ms for images under 5MB

#### Scenario: Extract JPEG image
- **GIVEN** clipboard contains a JPEG image
- **WHEN** ClipboardManager.getImageAsBase64() is called
- **THEN** returns Base64 string with mime type "image/jpeg"

#### Scenario: No image available
- **GIVEN** clipboard contains only text
- **WHEN** ClipboardManager.getImageAsBase64() is called
- **THEN** returns nil

---

### Requirement: Image Size Validation
The ClipboardManager SHALL validate image size before extraction.

#### Scenario: Image within size limit
- **GIVEN** clipboard contains 3MB image
- **AND** size limit is 20MB
- **WHEN** getImageAsBase64() is called
- **THEN** returns Base64 data successfully

#### Scenario: Image exceeds size limit
- **GIVEN** clipboard contains 25MB image
- **AND** size limit is 20MB
- **WHEN** getImageAsBase64() is called
- **THEN** returns nil
- **AND** logs warning "Image size 25MB exceeds limit 20MB"

---

### Requirement: Media Attachment Creation
The ClipboardManager SHALL create MediaAttachment structures for Rust core consumption.

#### Scenario: Create attachment from clipboard image
- **GIVEN** clipboard contains PNG image (1.5MB)
- **WHEN** ClipboardManager.getMediaAttachment() is called
- **THEN** returns MediaAttachment with:
  - media_type = "image"
  - mime_type = "image/png"
  - data = Base64-encoded content
  - size_bytes = 1572864

#### Scenario: No media in clipboard
- **GIVEN** clipboard contains only text
- **WHEN** ClipboardManager.getMediaAttachment() is called
- **THEN** returns nil

---

### Requirement: File URL Detection and Extraction
The ClipboardManager SHALL detect file references in clipboard and load content from disk.

#### Scenario: Detect file URLs in clipboard (Finder copy)
- **GIVEN** user has selected and copied files in Finder
- **WHEN** ClipboardManager.hasFileURLs() is called
- **THEN** returns true
- **AND** pasteboard contains `public.file-url` type

#### Scenario: Extract file URLs from clipboard
- **GIVEN** user has copied 2 image files in Finder
- **WHEN** ClipboardManager.getFileURLs() is called
- **THEN** returns array of file URLs
- **AND** URLs are valid file:// paths

#### Scenario: Load image file from URL
- **GIVEN** clipboard contains file URL to PNG image
- **AND** file exists at path
- **AND** file size < 20MB
- **WHEN** ClipboardManager.loadMediaFromFileURL(url) is called
- **THEN** returns MediaAttachment with:
  - media_type = "image"
  - mime_type = "image/png"
  - data = Base64-encoded file content
  - filename = original filename

#### Scenario: File not found
- **GIVEN** clipboard contains file URL
- **AND** file does not exist at path
- **WHEN** ClipboardManager.loadMediaFromFileURL(url) is called
- **THEN** returns nil
- **AND** logs warning "File not found: {path}"

#### Scenario: Unsupported file type
- **GIVEN** clipboard contains file URL to .mp4 video
- **AND** Phase 1 only supports images
- **WHEN** ClipboardManager.loadMediaFromFileURL(url) is called
- **THEN** returns nil
- **AND** logs info "Unsupported file type: video/mp4"

---

### Requirement: Mixed Content Extraction
The ClipboardManager SHALL handle rich content with both text and embedded media (e.g., Notes, Mail).

#### Scenario: Extract mixed content with direct image types
- **GIVEN** user has copied content from Notes with text and embedded image
- **AND** clipboard contains .string AND .png types directly
- **WHEN** ClipboardManager.getMixedContent() is called
- **THEN** extracts text from .string type
- **AND** extracts image from .png type (preferred path)
- **AND** returns tuple of (text: String?, attachments: [MediaAttachment])

#### Scenario: Extract embedded images from RTFD
- **GIVEN** user has copied rich text with embedded image from Notes
- **AND** clipboard contains .rtfd type (Rich Text Format Directory)
- **AND** clipboard does NOT contain direct .png/.jpeg types
- **WHEN** ClipboardManager.getMixedContent() is called
- **THEN** reads RTFD data via `pasteboard.data(forType: .rtfd)`
- **AND** parses via NSAttributedString with documentType: .rtfd
- **AND** enumerates NSTextAttachment attributes
- **AND** extracts image data from `fileWrapper.regularFileContents`
- **AND** creates MediaAttachment for each image attachment

#### Scenario: RTFD with multiple embedded images
- **GIVEN** user has copied Notes content with 3 embedded images
- **AND** clipboard contains .rtfd type
- **WHEN** ClipboardManager.getMixedContent() is called
- **THEN** extracts all 3 images from RTFD
- **AND** returns attachments array with 3 MediaAttachment items
- **AND** each attachment has correct filename from fileWrapper.preferredFilename

#### Scenario: RTFD with mixed attachment types (Phase 1 filtering)
- **GIVEN** user has copied Notes content with image.png, document.pdf, video.mp4
- **AND** clipboard contains .rtfd type
- **WHEN** ClipboardManager.getMixedContent() is called
- **THEN** extracts image.png (Phase 1 supported)
- **AND** skips document.pdf (Phase 2)
- **AND** skips video.mp4 (Phase 2)
- **AND** logs info for skipped attachments

#### Scenario: RTFD parsing failure graceful handling
- **GIVEN** clipboard contains corrupted .rtfd data
- **WHEN** ClipboardManager.getMixedContent() is called
- **THEN** NSAttributedString parsing fails
- **AND** logs warning "Failed to parse RTFD data"
- **AND** falls back to text-only extraction
- **AND** returns (text: String?, attachments: [])

#### Scenario: Prefer direct image types over RTFD parsing
- **GIVEN** clipboard contains both .png AND .rtfd types
- **WHEN** ClipboardManager.getMixedContent() is called
- **THEN** uses .png type directly (simpler, faster)
- **AND** does NOT parse RTFD (unnecessary)

#### Scenario: Text only content
- **GIVEN** clipboard contains only .string type
- **AND** no .png/.jpeg/.rtfd types present
- **WHEN** ClipboardManager.getMixedContent() is called
- **THEN** returns (text: "content", attachments: [])

