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

import Cocoa

/// Native clipboard manager using NSPasteboard
///
/// Provides read/write access to the system clipboard for text and images.
/// Uses macOS NSPasteboard API directly for optimal performance and compatibility.
class ClipboardManager {

    // MARK: - Singleton

    /// Shared instance for convenient access
    static let shared = ClipboardManager()

    /// Private initializer to encourage singleton usage
    private init() {}

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
