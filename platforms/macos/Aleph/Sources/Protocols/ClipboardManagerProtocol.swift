//
//  ClipboardManagerProtocol.swift
//  Aleph
//
//  Protocol for clipboard operations, enabling dependency injection and testability.
//

import Cocoa

/// Protocol for clipboard operations
///
/// Abstracts clipboard access for dependency injection and testing.
/// The default implementation is ClipboardManager.
protocol ClipboardManagerProtocol: AnyObject {

    // MARK: - Text Operations

    /// Read text from clipboard
    func getText() -> String?

    /// Write text to clipboard
    @discardableResult
    func setText(_ text: String) -> Bool

    /// Check if clipboard contains text
    func hasText() -> Bool

    // MARK: - Image Operations

    /// Read image from clipboard
    func getImage() -> NSImage?

    /// Write image to clipboard
    @discardableResult
    func setImage(_ image: NSImage) -> Bool

    /// Check if clipboard contains image
    func hasImage() -> Bool

    // MARK: - Advanced Operations

    /// Get clipboard change count
    func changeCount() -> Int

    /// Clear clipboard contents
    func clear()

    /// Get RTF content
    func getRTF() -> Data?

    /// Get URL from clipboard
    func getURL() -> URL?

    /// Get all available pasteboard types
    func availableTypes() -> [String]

    // MARK: - Content Detection

    /// Get clipboard content type
    func contentType() -> ClipboardContentType

    /// Check if clipboard is empty
    func isEmpty() -> Bool

    // MARK: - Multimodal Content Operations

    /// Get mixed content from clipboard (text + media attachments)
    func getMixedContent() -> (text: String?, attachments: [MediaAttachment], error: String?)

    /// Get image as Base64 string
    func getImageAsBase64() -> String?

    /// Check if clipboard has file URLs
    func hasFileURLs() -> Bool

    /// Get file URLs from clipboard
    func getFileURLs() -> [URL]
}

// MARK: - Default Implementation Conformance

extension ClipboardManager: ClipboardManagerProtocol {}
