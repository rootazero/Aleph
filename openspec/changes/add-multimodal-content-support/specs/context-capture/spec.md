# context-capture Spec Delta

## ADDED Requirements

### Requirement: Content Acquisition Method Awareness
The system SHALL understand the different capabilities of content acquisition methods and handle them appropriately.

#### Scenario: Clipboard-based acquisition supports media
- **GIVEN** user has selected content and pressed hotkey
- **WHEN** acquisition method is Cmd+C/X (clipboard)
- **THEN** system CAN capture text, images, file URLs, and rich text
- **AND** ClipboardManager.getMixedContent() is used for full extraction

#### Scenario: Accessibility API acquisition is text-only
- **GIVEN** no selection detected
- **WHEN** acquisition method is Accessibility API (kAXValueAttribute)
- **THEN** system can ONLY capture text content
- **AND** images and file references are NOT available
- **AND** attachments field in CapturedContext is nil

#### Scenario: Fallback to clipboard when Accessibility API fails
- **GIVEN** no selection detected
- **AND** Accessibility API returns empty or fails
- **WHEN** fallback to Cmd+A → Cmd+C is triggered
- **THEN** media content becomes available via clipboard
- **AND** ClipboardManager.getMixedContent() can extract images

---

### Requirement: File Reference Processing
The system SHALL process file URLs from clipboard and load content for supported types.

#### Scenario: Capture files from Finder selection
- **GIVEN** user has selected image files in Finder
- **AND** pressed hotkey
- **WHEN** Cmd+C copies file references to clipboard
- **THEN** ClipboardManager.getFileURLs() extracts file paths
- **AND** each supported file is loaded and converted to MediaAttachment
- **AND** attachments are included in CapturedContext

#### Scenario: Mixed file types in Finder selection
- **GIVEN** user has selected 3 files: image.png, document.pdf, video.mp4
- **WHEN** content is captured
- **THEN** image.png is processed (Phase 1 supported)
- **AND** document.pdf is skipped (Phase 2)
- **AND** video.mp4 is skipped (Phase 2)
- **AND** only supported attachments are included in context

---

## MODIFIED Requirements

### Requirement: Context Anchor Creation
The system SHALL package captured context as a structured data type including optional media attachments and send to Rust core.

#### Scenario: Create context anchor with text only
- **GIVEN** bundle_id = "com.apple.Notes"
- **AND** window_title = "Project Plan.txt"
- **AND** no media in clipboard
- **WHEN** context is captured
- **THEN** creates `CapturedContext` struct with bundle_id and window_title
- **AND** attachments field is nil
- **AND** sends to Rust via `core.processInput()`

#### Scenario: Create context anchor with image attachment
- **GIVEN** bundle_id = "com.apple.Preview"
- **AND** window_title = "Screenshot.png"
- **AND** clipboard contains PNG image
- **WHEN** context is captured
- **THEN** creates `CapturedContext` struct with:
  - app_bundle_id = "com.apple.Preview"
  - window_title = "Screenshot.png"
  - attachments = [MediaAttachment(media_type: "image", ...)]
- **AND** sends to Rust via `core.processInput()`

#### Scenario: Create context with multiple attachments
- **GIVEN** clipboard contains both text and image
- **WHEN** context is captured
- **THEN** text is captured via getText()
- **AND** image is captured as MediaAttachment
- **AND** both are included in the context sent to Rust

---

## ADDED Requirements

### Requirement: Media Attachment Capture
The Swift context capture flow SHALL detect and extract media attachments from the clipboard at hotkey press time.

#### Scenario: Capture image from clipboard
- **GIVEN** user has copied an image
- **WHEN** hotkey is pressed
- **THEN** Swift layer calls ClipboardManager.getMediaAttachment()
- **AND** creates MediaAttachment with Base64-encoded image
- **AND** includes attachment in CapturedContext

#### Scenario: No media in clipboard
- **GIVEN** clipboard contains only text
- **WHEN** hotkey is pressed
- **THEN** Swift layer calls ClipboardManager.getMediaAttachment()
- **AND** returns nil
- **AND** CapturedContext.attachments is nil or empty

#### Scenario: Image too large
- **GIVEN** clipboard contains 25MB image (exceeds 20MB limit)
- **WHEN** hotkey is pressed
- **THEN** ClipboardManager.getMediaAttachment() returns nil
- **AND** warning is logged
- **AND** processing continues with text only

---

### Requirement: Content Ordering
The system SHALL assemble input content in the following order: window text, clipboard text, media attachments.

#### Scenario: Full content ordering
- **GIVEN** window contains "/en Translate this"
- **AND** clipboard contains "Hello World" (text)
- **AND** clipboard contains image (PNG)
- **WHEN** content is assembled for AI
- **THEN** text input is: "/en Translate this\n\nHello World"
- **AND** image is included as separate attachment
- **AND** AI provider receives text first, then image

#### Scenario: Window text only
- **GIVEN** window contains "Analyze this code"
- **AND** clipboard is empty
- **WHEN** content is assembled for AI
- **THEN** text input is: "Analyze this code"
- **AND** attachments is nil

---

### Requirement: Acquisition Method Indicator
The system SHALL track which acquisition method was used for debugging and UX purposes.

#### Scenario: Track clipboard acquisition
- **GIVEN** user selected content before hotkey
- **WHEN** content is captured via Cmd+C
- **THEN** acquisition_method in context is "clipboard"

#### Scenario: Track Accessibility API acquisition
- **GIVEN** no selection detected
- **WHEN** content is captured via Accessibility API
- **THEN** acquisition_method in context is "accessibility"

#### Scenario: Track fallback acquisition
- **GIVEN** Accessibility API failed
- **WHEN** content is captured via Cmd+A → Cmd+C fallback
- **THEN** acquisition_method in context is "fallback_clipboard"

