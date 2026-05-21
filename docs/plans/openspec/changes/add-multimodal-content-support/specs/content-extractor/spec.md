# content-extractor Spec Delta

## ADDED Requirements

### Requirement: ContentExtractor Protocol
The system SHALL define a protocol for pluggable content extractors that can be registered and executed independently.

#### Scenario: Protocol defines required properties
- **GIVEN** a class implementing ContentExtractor protocol
- **WHEN** the protocol is defined
- **THEN** it requires:
  - `identifier: String` - unique extractor identifier
  - `priority: Int` - execution order (lower = higher priority)
  - `supportedTypes: [NSPasteboard.PasteboardType]` - pasteboard types this extractor handles
  - `canExtract(from:) -> Bool` - capability check method
  - `extract(from:) -> ExtractionResult` - extraction method

#### Scenario: ExtractionResult contains complete output
- **GIVEN** an extraction operation completes
- **WHEN** ExtractionResult is returned
- **THEN** it contains:
  - `text: String?` - extracted text content
  - `attachments: [MediaAttachment]` - extracted media attachments
  - `handledTypes: Set<NSPasteboard.PasteboardType>` - types that were processed
  - `metadata: [String: Any]` - debugging/logging information

---

### Requirement: ContentExtractorRegistry
The system SHALL provide a central registry for managing content extractors.

#### Scenario: Register new extractor
- **GIVEN** ContentExtractorRegistry.shared exists
- **AND** a new DirectImageExtractor is created
- **WHEN** registry.register(extractor) is called
- **THEN** extractor is added to the registry
- **AND** extractors are sorted by priority (ascending)

#### Scenario: Unregister extractor by identifier
- **GIVEN** registry contains RTFDExtractor with identifier "rtfd"
- **WHEN** registry.unregister(identifier: "rtfd") is called
- **THEN** RTFDExtractor is removed from registry
- **AND** other extractors remain unchanged

#### Scenario: Thread-safe registration
- **GIVEN** multiple threads accessing registry simultaneously
- **WHEN** one thread registers and another extracts
- **THEN** operations are serialized via dispatch queue
- **AND** no race conditions occur

---

### Requirement: Priority-based Extraction Order
The system SHALL execute extractors in priority order and skip already-handled types.

#### Scenario: Execute extractors in priority order
- **GIVEN** registry contains:
  - DirectImageExtractor (priority: 10)
  - RTFDExtractor (priority: 20)
  - FileURLExtractor (priority: 40)
- **WHEN** extractAll(from: pasteboard) is called
- **THEN** DirectImageExtractor executes first
- **AND** RTFDExtractor executes second
- **AND** FileURLExtractor executes third

#### Scenario: Skip already-handled types
- **GIVEN** DirectImageExtractor (priority: 10) handles .png type
- **AND** RTFDExtractor (priority: 20) also handles .png embedded in RTFD
- **AND** pasteboard contains direct .png
- **WHEN** extractAll is called
- **THEN** DirectImageExtractor extracts .png
- **AND** RTFDExtractor skips (type already handled)

#### Scenario: Multiple extractors contribute attachments
- **GIVEN** pasteboard contains .png AND file URLs
- **AND** DirectImageExtractor extracts 1 image
- **AND** FileURLExtractor extracts 2 file-based images
- **WHEN** extractAll is called
- **THEN** result contains 3 total attachments
- **AND** all extractors contribute to final result

---

### Requirement: Priority Ranges
The system SHALL define recommended priority ranges for different extractor categories.

#### Scenario: Direct types priority range (0-19)
- **GIVEN** DirectImageExtractor with priority 10
- **WHEN** extractor processes .png/.jpeg/.tiff
- **THEN** it executes before all other extractors
- **AND** represents the fastest extraction path (no parsing)

#### Scenario: Rich format priority range (20-39)
- **GIVEN** RTFDExtractor with priority 20
- **WHEN** extractor processes .rtfd
- **THEN** it executes after direct types
- **AND** represents formats requiring parsing

#### Scenario: File reference priority range (40-59)
- **GIVEN** FileURLExtractor with priority 40
- **WHEN** extractor processes file URLs
- **THEN** it executes after rich formats
- **AND** represents operations requiring disk I/O

#### Scenario: Fallback priority range (80-99)
- **GIVEN** PlainTextExtractor with priority 80
- **WHEN** no other extractor handles content
- **THEN** it provides fallback text extraction

---

### Requirement: Built-in Extractors (Phase 1)
The system SHALL provide built-in extractors for Phase 1 content types.

#### Scenario: DirectImageExtractor registration
- **GIVEN** app startup
- **WHEN** setupContentExtractors() is called
- **THEN** DirectImageExtractor is registered
- **AND** handles .png, .jpeg, .tiff pasteboard types
- **AND** has priority 10

#### Scenario: RTFDExtractor registration
- **GIVEN** app startup
- **WHEN** setupContentExtractors() is called
- **THEN** RTFDExtractor is registered
- **AND** handles .rtfd pasteboard type
- **AND** has priority 20

#### Scenario: FileURLExtractor registration
- **GIVEN** app startup
- **WHEN** setupContentExtractors() is called
- **THEN** FileURLExtractor is registered
- **AND** handles public.file-url pasteboard type
- **AND** has priority 40

#### Scenario: PlainTextExtractor registration
- **GIVEN** app startup
- **WHEN** setupContentExtractors() is called
- **THEN** PlainTextExtractor is registered
- **AND** handles .string pasteboard type
- **AND** has priority 80

---

### Requirement: Extractor Isolation
Each extractor SHALL be isolated and not depend on other extractors.

#### Scenario: Extractor independence
- **GIVEN** DirectImageExtractor is implemented
- **WHEN** checking its dependencies
- **THEN** it does not import or reference RTFDExtractor
- **AND** it does not import or reference FileURLExtractor

#### Scenario: Extractor failure isolation
- **GIVEN** RTFDExtractor throws an error during extraction
- **WHEN** extractAll continues to next extractor
- **THEN** FileURLExtractor still executes
- **AND** error is logged but does not crash

#### Scenario: Independent testing
- **GIVEN** DirectImageExtractor unit test
- **WHEN** test creates mock pasteboard with .png
- **THEN** extractor can be tested in isolation
- **AND** no other extractors are needed

---

### Requirement: Future Extensibility
The system SHALL support adding new extractors without modifying existing code.

#### Scenario: Add PDF extractor in Phase 2
- **GIVEN** existing system with Phase 1 extractors
- **WHEN** PDFExtractor class is created implementing ContentExtractor
- **AND** registry.register(PDFExtractor()) is called
- **THEN** PDF extraction is enabled
- **AND** no changes to DirectImageExtractor or RTFDExtractor

#### Scenario: Add video thumbnail extractor
- **GIVEN** existing system with Phase 1 extractors
- **WHEN** VideoThumbnailExtractor is created with priority 45
- **AND** registered with supportedTypes = ["public.movie"]
- **THEN** video thumbnail extraction is enabled
- **AND** executes between FileURLExtractor and PlainTextExtractor

#### Scenario: Runtime extractor management
- **GIVEN** app is running with default extractors
- **WHEN** user enables PDF support in settings
- **THEN** PDFExtractor can be registered at runtime
- **AND** immediately available for next extraction

