# uniffi-bridge Spec Delta

## ADDED Requirements

### Requirement: MediaAttachment Dictionary Type
The UniFFI interface SHALL define a MediaAttachment dictionary type for cross-FFI media transfer.

#### Scenario: Define MediaAttachment structure
- **GIVEN** aether.udl interface definition
- **WHEN** MediaAttachment type is defined
- **THEN** structure includes:
  - media_type: string ("image", "video", "file")
  - mime_type: string ("image/png", "image/jpeg", etc.)
  - data: string (Base64-encoded content)
  - filename: string? (optional original filename)
  - size_bytes: u64 (original byte size for logging)

#### Scenario: Swift binding generation
- **WHEN** uniffi-bindgen generates Swift bindings
- **THEN** MediaAttachment struct is generated in Swift
- **AND** all fields are accessible as Swift properties

---

## MODIFIED Requirements

### Requirement: CapturedContext Dictionary Type
The UniFFI interface SHALL define CapturedContext with optional media attachments.

#### Scenario: CapturedContext with attachments
- **GIVEN** aether.udl interface definition
- **WHEN** CapturedContext dictionary is defined
- **THEN** structure includes:
  - app_bundle_id: string
  - window_title: string?
  - attachments: sequence<MediaAttachment>? (NEW)

#### Scenario: Backward compatibility
- **GIVEN** existing Swift code creating CapturedContext
- **WHEN** attachments field is nil
- **THEN** Rust core processes as text-only input
- **AND** no breaking change for existing callers

---

### Requirement: process_input Interface
The AetherCore interface SHALL accept multimodal input through CapturedContext attachments.

#### Scenario: Text-only input
- **GIVEN** CapturedContext with attachments = nil
- **WHEN** core.processInput(userInput, context) is called
- **THEN** Rust processes as text-only (current behavior)

#### Scenario: Multimodal input with image
- **GIVEN** CapturedContext with one image attachment
- **WHEN** core.processInput(userInput, context) is called
- **THEN** Rust extracts attachment from context
- **AND** routes to vision-capable provider
- **AND** includes image in AI request

