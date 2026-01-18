//
//  PendingAttachment.swift
//  Aether
//
//  Data model for pending attachments in multi-turn conversation.
//

import AppKit
import Foundation

// MARK: - FileType

/// Type of attached file
enum AttachmentFileType {
    case image      // Show thumbnail preview
    case document   // Show file icon + name
    case other      // Generic file icon

    var iconName: String {
        switch self {
        case .image: return "photo"
        case .document: return "doc.text"
        case .other: return "doc"
        }
    }
}

// MARK: - PendingAttachment

/// Pending attachment waiting to be sent with message
struct PendingAttachment: Identifiable, Equatable {
    let id: UUID
    let url: URL
    let fileName: String
    let fileType: AttachmentFileType
    let thumbnail: NSImage?
    let data: Data

    init(url: URL) throws {
        self.id = UUID()
        self.url = url
        self.fileName = url.lastPathComponent
        self.data = try Data(contentsOf: url)
        self.fileType = Self.detectFileType(url: url)
        self.thumbnail = Self.generateThumbnail(url: url, fileType: self.fileType)
    }

    // MARK: - File Type Detection

    private static func detectFileType(url: URL) -> AttachmentFileType {
        let ext = url.pathExtension.lowercased()
        let imageExtensions = ["png", "jpg", "jpeg", "gif", "webp", "heic", "bmp", "tiff"]
        let documentExtensions = ["pdf", "doc", "docx", "txt", "rtf", "md", "pages"]

        if imageExtensions.contains(ext) {
            return .image
        } else if documentExtensions.contains(ext) {
            return .document
        } else {
            return .other
        }
    }

    // MARK: - Thumbnail Generation

    private static func generateThumbnail(url: URL, fileType: AttachmentFileType) -> NSImage? {
        switch fileType {
        case .image:
            guard let image = NSImage(contentsOf: url) else { return nil }
            // Resize to thumbnail size (64x64)
            let targetSize = NSSize(width: 64, height: 64)
            let newImage = NSImage(size: targetSize)
            newImage.lockFocus()
            image.draw(
                in: NSRect(origin: .zero, size: targetSize),
                from: NSRect(origin: .zero, size: image.size),
                operation: .copy,
                fraction: 1.0
            )
            newImage.unlockFocus()
            return newImage
        case .document, .other:
            return NSWorkspace.shared.icon(forFile: url.path)
        }
    }

    // MARK: - Conversion to MediaAttachment

    /// Convert to MediaAttachment for sending to Rust core
    func toMediaAttachment() -> MediaAttachment {
        let mediaType: String
        let mimeType: String

        switch fileType {
        case .image:
            mediaType = "image"
            let ext = url.pathExtension.lowercased()
            switch ext {
            case "png": mimeType = "image/png"
            case "jpg", "jpeg": mimeType = "image/jpeg"
            case "gif": mimeType = "image/gif"
            case "webp": mimeType = "image/webp"
            default: mimeType = "image/png"
            }
        case .document:
            mediaType = "document"
            let ext = url.pathExtension.lowercased()
            switch ext {
            case "pdf": mimeType = "application/pdf"
            case "txt", "md": mimeType = "text/plain"
            default: mimeType = "application/octet-stream"
            }
        case .other:
            mediaType = "file"
            mimeType = "application/octet-stream"
        }

        // Convert Data to base64 string for Rust core
        let base64Data = data.base64EncodedString()

        return MediaAttachment(
            mediaType: mediaType,
            mimeType: mimeType,
            data: base64Data,
            encoding: "base64",
            filename: fileName,
            sizeBytes: UInt64(data.count)
        )
    }

    // MARK: - Equatable

    static func == (lhs: PendingAttachment, rhs: PendingAttachment) -> Bool {
        lhs.id == rhs.id
    }
}
