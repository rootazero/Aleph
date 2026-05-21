# image-clipboard Specification

## Purpose

Enable Aleph to read and write images from/to the system clipboard, enabling multimodal AI interactions with vision-capable models (GPT-4 Vision, Claude 3 Opus). Images are encoded as Base64 for API transmission and support standard formats (PNG, JPEG, GIF, WebP).

## ADDED Requirements

### Requirement: Read Image from Clipboard

The system SHALL read image content from the system clipboard using the arboard crate's image support.

#### Scenario: Read PNG image from clipboard

- **WHEN** user copies a PNG image to clipboard
- **AND** client calls `clipboard_manager.read_image()`
- **THEN** image data is returned as `ImageData` struct
- **AND** `ImageData.format` is `ImageFormat::Png`
- **AND** `ImageData.data` contains raw PNG bytes
- **AND** operation completes within 500ms

#### Scenario: Read JPEG image from clipboard

- **WHEN** user copies a JPEG image to clipboard
- **AND** client calls `clipboard_manager.read_image()`
- **THEN** image data is returned with `ImageFormat::Jpeg`
- **AND** original quality is preserved
- **AND** EXIF metadata is stripped for privacy

#### Scenario: Handle clipboard with no image

- **WHEN** clipboard contains only text
- **AND** client calls `read_image()`
- **THEN** operation returns `AlephError::ClipboardError`
- **AND** error message indicates "No image content available"
- **AND** text content remains accessible via `read_text()`

#### Scenario: Handle oversized images

- **WHEN** clipboard contains image larger than `max_image_size_mb` config (default: 10MB)
- **AND** client calls `read_image()`
- **THEN** operation returns `AlephError::ClipboardError`
- **AND** error message indicates size limit exceeded
- **AND** suggestion recommends resizing image

#### Scenario: Handle corrupted image data

- **WHEN** clipboard contains corrupted image bytes
- **AND** client calls `read_image()`
- **THEN** operation returns `AlephError::ClipboardError`
- **AND** error message indicates "Invalid image format"
- **AND** no panic or crash occurs

### Requirement: Write Image to Clipboard

The system SHALL write image content to the system clipboard in standard formats.

#### Scenario: Write PNG image to clipboard

- **WHEN** client calls `clipboard_manager.write_image(image_data)`
- **AND** `image_data.format` is `ImageFormat::Png`
- **THEN** clipboard content is set to PNG image
- **AND** subsequent `read_image()` returns identical data
- **AND** other applications can paste the image

#### Scenario: Overwrite text with image

- **WHEN** clipboard contains text
- **AND** client calls `write_image(image_data)`
- **THEN** text content is replaced with image
- **AND** subsequent `read_text()` returns error
- **AND** `has_image()` returns true

### Requirement: Detect Image in Clipboard

The system SHALL detect whether clipboard currently contains image content.

#### Scenario: Detect image presence

- **WHEN** clipboard contains an image
- **AND** client calls `has_image()`
- **THEN** method returns true
- **AND** operation completes synchronously (<10ms)

#### Scenario: Detect text-only clipboard

- **WHEN** clipboard contains only text
- **AND** client calls `has_image()`
- **THEN** method returns false
- **AND** no error is raised

#### Scenario: Detect mixed content

- **WHEN** clipboard contains both text and image (macOS rich text)
- **AND** client calls `has_image()`
- **THEN** method returns true (image takes priority)
- **AND** both `read_text()` and `read_image()` succeed

### Requirement: Base64 Image Encoding

The system SHALL encode images as Base64 strings for AI API transmission.

#### Scenario: Encode PNG to Base64

- **WHEN** client calls `image_data.to_base64()`
- **AND** image format is PNG
- **THEN** Base64-encoded string is returned
- **AND** string starts with "data:image/png;base64,"
- **AND** encoded data is valid Base64 (RFC 4648)

#### Scenario: Encode JPEG to Base64

- **WHEN** client calls `image_data.to_base64()`
- **AND** image format is JPEG
- **THEN** Base64-encoded string includes "data:image/jpeg;base64," prefix
- **AND** encoding preserves image quality

#### Scenario: Decode Base64 to image

- **WHEN** client calls `ImageData::from_base64(base64_string)`
- **AND** string has valid data URI format
- **THEN** `ImageData` struct is created
- **AND** format is detected from MIME type
- **AND** decoded bytes match original image

### Requirement: Image Format Detection

The system SHALL automatically detect image format from clipboard data.

#### Scenario: Detect format from magic bytes

- **WHEN** clipboard contains image data
- **AND** first bytes are PNG signature (89 50 4E 47)
- **THEN** `ImageFormat::Png` is detected
- **AND** no explicit format specification required

#### Scenario: Detect JPEG format

- **WHEN** clipboard contains JPEG data
- **AND** first bytes are JPEG signature (FF D8 FF)
- **THEN** `ImageFormat::Jpeg` is detected

#### Scenario: Handle unsupported formats

- **WHEN** clipboard contains BMP or TIFF image
- **AND** client calls `read_image()`
- **THEN** operation returns `AlephError::UnsupportedFormat`
- **AND** error suggests converting to PNG/JPEG
- **AND** user can paste into image editor for conversion

### Requirement: Image Clipboard Integration with AI Providers

The system SHALL send clipboard images to vision-capable AI providers using their respective APIs.

#### Scenario: Send image to OpenAI GPT-4 Vision

- **WHEN** clipboard contains image
- **AND** router selects OpenAI provider
- **THEN** image is Base64-encoded
- **AND** request uses `image_url` field in messages API
- **AND** model defaults to "gpt-4-vision-preview" if not configured
- **AND** `max_tokens` is set to 4096 for detailed responses

#### Scenario: Send image to Claude 3 Opus

- **WHEN** clipboard contains image
- **AND** router selects Claude provider
- **THEN** image is Base64-encoded
- **AND** request uses `image` content type in Messages API
- **AND** model defaults to "claude-3-opus-20240229"
- **AND** request includes both text and image content blocks

#### Scenario: Fallback when provider lacks vision support

- **WHEN** clipboard contains image
- **AND** router selects Ollama provider without vision model
- **THEN** system returns `AlephError::ProviderError`
- **AND** error message indicates "Vision not supported by current provider"
- **AND** suggestion recommends switching to OpenAI or Claude

### Requirement: Image Size Optimization

The system SHALL optimize image size before API transmission to reduce latency and costs.

#### Scenario: Compress large images

- **WHEN** clipboard image exceeds `max_image_size_mb` (default: 10MB)
- **AND** config option `auto_compress_images` is enabled
- **THEN** image is resized to fit under limit (preserve aspect ratio)
- **AND** JPEG quality is reduced to 85% if needed
- **AND** user is notified via warning log

#### Scenario: Skip compression for small images

- **WHEN** clipboard image is under 1MB
- **THEN** image is sent without compression
- **AND** original quality is preserved

## MODIFIED Requirements

### Requirement: Clipboard Manager Trait (Extended)

The system SHALL extend the `ClipboardManager` trait with image operations while maintaining text compatibility.

#### Scenario: Implement extended trait

- **WHEN** creating a new clipboard backend
- **THEN** it must implement `read_image()`, `write_image()`, and `has_image()`
- **AND** existing `read_text()` and `write_text()` continue to work
- **AND** trait remains mockable for testing

## References

- **Related Spec**: `clipboard-management` - Extends text clipboard with image support
- **Related Spec**: `openai-provider` - Vision API integration
- **Related Spec**: `claude-provider` - Image content block support
- **Related Spec**: `ai-routing` - Vision capability detection
