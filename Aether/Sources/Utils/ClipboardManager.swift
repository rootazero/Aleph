// ClipboardManager.swift
// Native clipboard operations using macOS NSPasteboard API
//
// This implementation replaces Rust-based clipboard access (arboard)
// to eliminate FFI overhead and provide full access to macOS clipboard features.
//
// Key advantages over Rust/arboard:
// - Zero FFI calls
// - Full support for macOS pasteboard types (text, images, RTF, URLs, etc.)
// - Native type conversions (NSImage, NSString)
// - Can detect clipboard changes via changeCount
// - Thread-safe (NSPasteboard is thread-safe)
//
// Multimodal content support (add-multimodal-content-support):
// - Content extractors for images, RTFD, file URLs
// - getMixedContent() for comprehensive content extraction

import Cocoa
import os.log

/// Native clipboard manager using NSPasteboard
///
/// Provides read/write access to the system clipboard for text and images.
/// Uses macOS NSPasteboard API directly for optimal performance and compatibility.
class ClipboardManager {

    // MARK: - Singleton

    /// Shared instance for convenient access
    static let shared = ClipboardManager()

    // MARK: - Private Properties

    private let logger = Logger(subsystem: "com.aether", category: "ClipboardManager")
    private var extractorsSetup = false

    /// Private initializer to encourage singleton usage
    private init() {
        setupContentExtractors()
    }

    // MARK: - Content Extractor Setup

    /// Setup content extractors for multimodal content support
    ///
    /// Registers all extractors with the ContentExtractorRegistry.
    /// Called automatically during initialization.
    private func setupContentExtractors() {
        guard !extractorsSetup else { return }

        let registry = ContentExtractorRegistry.shared

        // Image support extractors (in priority order)
        registry.register(DirectImageExtractor())
        registry.register(RTFDExtractor())
        registry.register(FileURLExtractor())
        // Document support extractors
        registry.register(DocumentExtractor())
        // Fallback text extractor
        registry.register(PlainTextExtractor())

        extractorsSetup = true
        logger.info("Content extractors registered: \(registry.registeredCount)")
    }

    // MARK: - Text Operations

    /// Read text from clipboard
    ///
    /// - Returns: Text content if available, nil otherwise
    func getText() -> String? {
        return NSPasteboard.general.string(forType: .string)
    }

    /// Write text to clipboard
    ///
    /// - Parameter text: Text to write to clipboard
    /// - Returns: True if successful, false otherwise
    @discardableResult
    func setText(_ text: String) -> Bool {
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        return pasteboard.setString(text, forType: .string)
    }

    /// Check if clipboard contains text
    ///
    /// - Returns: True if clipboard has text content
    func hasText() -> Bool {
        let types = NSPasteboard.general.types ?? []
        return types.contains(.string)
    }

    // MARK: - Image Operations

    /// Read image from clipboard
    ///
    /// - Returns: NSImage if available, nil otherwise
    func getImage() -> NSImage? {
        guard let objects = NSPasteboard.general.readObjects(
            forClasses: [NSImage.self],
            options: nil
        ) else {
            return nil
        }

        return objects.first as? NSImage
    }

    /// Write image to clipboard
    ///
    /// - Parameter image: NSImage to write to clipboard
    /// - Returns: True if successful, false otherwise
    @discardableResult
    func setImage(_ image: NSImage) -> Bool {
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        return pasteboard.writeObjects([image])
    }

    /// Check if clipboard contains image
    ///
    /// - Returns: True if clipboard has image content
    func hasImage() -> Bool {
        let types = NSPasteboard.general.types ?? []
        return types.contains(.tiff) ||
               types.contains(.png) ||
               types.contains(NSPasteboard.PasteboardType("public.jpeg"))
    }

    // MARK: - Advanced Operations

    /// Get clipboard change count
    ///
    /// Used to detect if clipboard was modified externally.
    /// Each clipboard write increments this count.
    ///
    /// - Returns: Current change count
    func changeCount() -> Int {
        return NSPasteboard.general.changeCount
    }

    /// Clear clipboard contents
    func clear() {
        NSPasteboard.general.clearContents()
    }

    /// Get RTF (Rich Text Format) content
    ///
    /// - Returns: RTF data if available, nil otherwise
    func getRTF() -> Data? {
        return NSPasteboard.general.data(forType: .rtf)
    }

    /// Get URL from clipboard
    ///
    /// - Returns: URL if available, nil otherwise
    func getURL() -> URL? {
        guard let urlString = NSPasteboard.general.string(forType: .URL) else {
            return nil
        }
        return URL(string: urlString)
    }

    /// Get all available pasteboard types
    ///
    /// Useful for debugging clipboard content.
    ///
    /// - Returns: Array of available type identifiers
    func availableTypes() -> [String] {
        return NSPasteboard.general.types?.map { $0.rawValue } ?? []
    }

    // MARK: - Content Detection

    /// Get clipboard content type
    ///
    /// - Returns: Best guess of clipboard content type
    func contentType() -> ClipboardContentType {
        let types = NSPasteboard.general.types ?? []

        if types.contains(.tiff) || types.contains(.png) {
            return .image
        } else if types.contains(.URL) {
            return .url
        } else if types.contains(.rtf) {
            return .richText
        } else if types.contains(.string) {
            return .plainText
        } else {
            return .unknown
        }
    }

    /// Check if clipboard is empty
    ///
    /// - Returns: True if clipboard has no content
    func isEmpty() -> Bool {
        return NSPasteboard.general.types?.isEmpty ?? true
    }

    // MARK: - Multimodal Content Operations (add-multimodal-content-support)

    /// Get mixed content from clipboard (text + media attachments)
    ///
    /// Uses the ContentExtractorRegistry to extract all available content
    /// from the clipboard, including text and media attachments.
    ///
    /// - Returns: Tuple of (text, attachments, error)
    func getMixedContent() -> (text: String?, attachments: [MediaAttachment], error: String?) {
        let pasteboard = NSPasteboard.general

        // Log available types for debugging
        if let types = pasteboard.types {
            logger.debug("Clipboard types: \(types.map(\.rawValue).joined(separator: ", "))")
        }

        // Delegate to ContentExtractorRegistry
        let result = ContentExtractorRegistry.shared.extractAll(from: pasteboard)

        // Check for errors
        if let error = result.error {
            logger.error("Content extraction error: \(error)")
            return (nil, [], error)
        }

        // CRITICAL FIX: If extractors didn't return text, fall back to direct getText()
        // This ensures command prefixes like /en are always captured correctly
        var text = result.text
        if text == nil || text?.isEmpty == true {
            text = getText()
            if text != nil {
                logger.debug("Fallback to getText() returned text: \(text!.prefix(50))...")
            }
        }

        // Log extraction summary
        if result.attachments.isEmpty {
            logger.debug("No media attachments found in clipboard")
        } else {
            logger.info("Extracted \(result.attachments.count) media attachments from clipboard")
        }

        return (text, result.attachments, nil)
    }

    /// Get image as Base64 string (legacy wrapper)
    ///
    /// Convenience method for getting a single image as Base64.
    /// For multiple images or mixed content, use getMixedContent() instead.
    ///
    /// - Returns: Base64-encoded image data, or nil if no image available
    func getImageAsBase64() -> String? {
        let (_, attachments, _) = getMixedContent()
        return attachments.first?.data
    }

    /// Check if clipboard has file URLs
    ///
    /// - Returns: True if clipboard contains file URL references
    func hasFileURLs() -> Bool {
        let types = NSPasteboard.general.types ?? []
        return types.contains(NSPasteboard.PasteboardType("public.file-url"))
    }

    /// Get file URLs from clipboard
    ///
    /// - Returns: Array of file URLs, or empty array if none
    func getFileURLs() -> [URL] {
        guard let urls = NSPasteboard.general.readObjects(forClasses: [NSURL.self], options: [
            .urlReadingFileURLsOnly: true
        ]) as? [URL] else {
            return []
        }
        return urls
    }
}

// MARK: - Supporting Types

/// Clipboard content type enum
enum ClipboardContentType {
    case plainText
    case richText
    case image
    case url
    case unknown

    var description: String {
        switch self {
        case .plainText: return "Plain Text"
        case .richText: return "Rich Text"
        case .image: return "Image"
        case .url: return "URL"
        case .unknown: return "Unknown"
        }
    }
}
