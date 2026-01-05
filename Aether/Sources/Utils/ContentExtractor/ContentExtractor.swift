// ContentExtractor.swift
// Extensible content extraction protocol for multimodal support
//
// This architecture allows new content types to be added without modifying existing code.
// Each extractor handles a specific content type (images, RTFD, file URLs, etc.)
//
// add-multimodal-content-support

import Cocoa
import os.log

// MARK: - Content Extractor Protocol

/// Protocol for pluggable content extractors
///
/// Extractors are registered with ContentExtractorRegistry and called in priority order.
/// Each extractor handles a specific content type and returns extracted attachments.
///
/// # Priority Guidelines
///
/// | Range  | Category         | Examples                        |
/// |--------|------------------|---------------------------------|
/// | 0-19   | Direct types     | DirectImageExtractor (10)       |
/// | 20-39  | Rich formats     | RTFDExtractor (20)              |
/// | 40-59  | File references  | FileURLExtractor (40)           |
/// | 60-79  | Network resources| URLImageExtractor (reserved)    |
/// | 80-99  | Fallbacks        | PlainTextExtractor (80)         |
protocol ContentExtractor {
    /// Unique identifier for this extractor
    var identifier: String { get }

    /// Priority (lower = higher priority, executed first)
    var priority: Int { get }

    /// Supported pasteboard types this extractor can handle
    var supportedTypes: [NSPasteboard.PasteboardType] { get }

    /// Check if this extractor can process the current pasteboard
    ///
    /// - Parameter pasteboard: The system pasteboard to check
    /// - Returns: true if this extractor can extract content from the pasteboard
    func canExtract(from pasteboard: NSPasteboard) -> Bool

    /// Extract content from pasteboard
    ///
    /// - Parameter pasteboard: The system pasteboard to extract from
    /// - Returns: Extraction result with text, attachments, and metadata
    func extract(from pasteboard: NSPasteboard) -> ExtractionResult
}

// MARK: - Extraction Result

/// Result of a content extraction operation
struct ExtractionResult {
    /// Extracted text content (if any)
    let text: String?

    /// Extracted media attachments
    let attachments: [MediaAttachment]

    /// Types that were handled by this extractor
    let handledTypes: Set<NSPasteboard.PasteboardType>

    /// Additional metadata for debugging/logging
    let metadata: [String: Any]

    // MARK: - Static Constructors

    /// Create an empty result
    static var empty: ExtractionResult {
        ExtractionResult(text: nil, attachments: [], handledTypes: [], metadata: [:])
    }

    /// Create a result with text only
    static func text(_ text: String) -> ExtractionResult {
        ExtractionResult(text: text, attachments: [], handledTypes: [.string], metadata: [:])
    }

    /// Create a result with attachments only
    static func attachments(_ attachments: [MediaAttachment], handledTypes: Set<NSPasteboard.PasteboardType>) -> ExtractionResult {
        ExtractionResult(text: nil, attachments: attachments, handledTypes: handledTypes, metadata: [:])
    }
}

// MARK: - Content Extractor Registry

/// Central registry for content extractors
///
/// Manages extractor lifecycle and orchestrates extraction from pasteboard.
/// Extractors are called in priority order, and types handled by higher-priority
/// extractors are skipped by lower-priority ones.
///
/// # Thread Safety
///
/// All operations are thread-safe via DispatchQueue synchronization.
final class ContentExtractorRegistry {
    // MARK: - Singleton

    static let shared = ContentExtractorRegistry()

    // MARK: - Private Properties

    private var extractors: [ContentExtractor] = []
    private let queue = DispatchQueue(label: "com.aether.content-extractor-registry")
    private let logger = Logger(subsystem: "com.aether", category: "ContentExtractor")

    // MARK: - Initialization

    private init() {}

    // MARK: - Registration

    /// Register a new extractor
    ///
    /// Extractors are automatically sorted by priority after registration.
    ///
    /// - Parameter extractor: The extractor to register
    func register(_ extractor: ContentExtractor) {
        queue.sync {
            extractors.append(extractor)
            extractors.sort { $0.priority < $1.priority }
            logger.debug("Registered extractor: \(extractor.identifier) (priority: \(extractor.priority))")
        }
    }

    /// Unregister an extractor by identifier
    ///
    /// - Parameter identifier: The identifier of the extractor to remove
    func unregister(identifier: String) {
        queue.sync {
            extractors.removeAll { $0.identifier == identifier }
            logger.debug("Unregistered extractor: \(identifier)")
        }
    }

    /// Check if an extractor is registered
    ///
    /// - Parameter identifier: The identifier to check
    /// - Returns: true if an extractor with this identifier is registered
    func isRegistered(identifier: String) -> Bool {
        queue.sync {
            extractors.contains { $0.identifier == identifier }
        }
    }

    /// Get count of registered extractors
    var registeredCount: Int {
        queue.sync { extractors.count }
    }

    // MARK: - Extraction

    /// Extract all content from pasteboard using registered extractors
    ///
    /// Extractors are called in priority order. Types handled by higher-priority
    /// extractors are skipped by lower-priority ones to avoid duplicate extraction.
    ///
    /// - Parameter pasteboard: The system pasteboard to extract from
    /// - Returns: Tuple of (text, attachments)
    func extractAll(from pasteboard: NSPasteboard) -> (text: String?, attachments: [MediaAttachment]) {
        var allAttachments: [MediaAttachment] = []
        var text: String?
        var handledTypes: Set<NSPasteboard.PasteboardType> = []

        let extractorsCopy = queue.sync { self.extractors }

        for extractor in extractorsCopy {
            // Skip if this extractor's types are already handled
            let extractorTypes = Set(extractor.supportedTypes)
            if !extractorTypes.isDisjoint(with: handledTypes) {
                logger.debug("[\(extractor.identifier)] Skipping - types already handled")
                continue
            }

            if extractor.canExtract(from: pasteboard) {
                let result = extractor.extract(from: pasteboard)

                // Take text from first extractor that provides it
                if text == nil, let extractedText = result.text {
                    text = extractedText
                }

                allAttachments.append(contentsOf: result.attachments)
                handledTypes.formUnion(result.handledTypes)

                logger.debug("[\(extractor.identifier)] Extracted \(result.attachments.count) attachments")
            }
        }

        logger.info("Extraction complete: \(allAttachments.count) total attachments")
        return (text, allAttachments)
    }

    // MARK: - Utilities

    /// Clear all registered extractors (mainly for testing)
    func clearAll() {
        queue.sync {
            extractors.removeAll()
        }
    }
}

// MARK: - Shared Utilities

/// Supported media types for Phase 1 (images only)
enum SupportedMediaType: String, CaseIterable {
    case png
    case jpg
    case jpeg
    case gif
    case webp
    case tiff

    /// Get MIME type for this media type
    var mimeType: String {
        switch self {
        case .png: return "image/png"
        case .jpg, .jpeg: return "image/jpeg"
        case .gif: return "image/gif"
        case .webp: return "image/webp"
        case .tiff: return "image/tiff"
        }
    }

    /// Check if a file extension is supported
    static func isSupported(_ extension: String) -> Bool {
        allCases.map(\.rawValue).contains(`extension`.lowercased())
    }

    /// Get SupportedMediaType from file extension
    static func from(extension ext: String) -> SupportedMediaType? {
        allCases.first { $0.rawValue == ext.lowercased() }
    }
}

/// Size limits for media content
enum MediaSizeLimits {
    /// Maximum allowed image size in bytes (20MB)
    static let maxImageSizeBytes: UInt64 = 20 * 1024 * 1024

    /// Warning threshold for image size in bytes (5MB)
    static let warnImageSizeBytes: UInt64 = 5 * 1024 * 1024
}

/// Helper to get MIME type from file extension
func mimeType(for extension: String) -> String {
    SupportedMediaType.from(extension: `extension`)?.mimeType ?? "application/octet-stream"
}

/// Helper to check if file extension is supported
func isSupported(fileExtension: String) -> Bool {
    SupportedMediaType.isSupported(fileExtension)
}
