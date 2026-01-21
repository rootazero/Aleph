//
//  AttachmentModels.swift
//  Aether
//
//  Data models for stored attachments.
//  Attachments are linked to messages and persisted to SQLite.
//

import Foundation
import GRDB

// MARK: - AttachmentSourceType

/// Source type of the attachment
enum AttachmentSourceType: String, Codable, DatabaseValueConvertible {
    /// User uploaded file (drag & drop, paste, file picker)
    case userUpload = "user_upload"
    /// Tool-generated output (image_generate, pdf_generate, etc.)
    case toolOutput = "tool_output"
    /// Remote URL from AI response (cached locally)
    case remoteUrl = "remote_url"
}

// MARK: - AttachmentMediaType

/// Media type of the attachment
enum AttachmentMediaType: String, Codable, DatabaseValueConvertible {
    case image
    case document
    case video
    case audio
    case file

    /// Detect media type from MIME type
    static func from(mimeType: String) -> AttachmentMediaType {
        let mime = mimeType.lowercased()
        if mime.hasPrefix("image/") {
            return .image
        } else if mime.hasPrefix("video/") {
            return .video
        } else if mime.hasPrefix("audio/") {
            return .audio
        } else if mime == "application/pdf" || mime.hasPrefix("text/") {
            return .document
        } else {
            return .file
        }
    }

    /// Detect media type from file extension
    static func from(extension ext: String) -> AttachmentMediaType {
        let ext = ext.lowercased()
        switch ext {
        case "png", "jpg", "jpeg", "gif", "webp", "heic", "bmp", "tiff", "svg":
            return .image
        case "mp4", "mov", "avi", "mkv", "webm":
            return .video
        case "mp3", "wav", "aac", "m4a", "flac", "ogg":
            return .audio
        case "pdf", "txt", "md", "rtf", "doc", "docx", "pages":
            return .document
        default:
            return .file
        }
    }
}

// MARK: - StoredAttachment

/// A persisted attachment linked to a message
struct StoredAttachment: Identifiable, Codable, FetchableRecord, PersistableRecord {

    // MARK: - Properties

    /// Unique identifier (UUID)
    var id: String

    /// ID of the message this attachment belongs to
    var messageId: String

    /// Source of the attachment
    var attachmentType: AttachmentSourceType

    /// Media type (image, document, etc.)
    var mediaType: AttachmentMediaType

    /// MIME type (image/png, application/pdf, etc.)
    var mimeType: String

    /// Original filename
    var filename: String?

    /// Local path relative to attachments directory
    /// e.g., "user/msg123/uuid_file.png"
    var localPath: String?

    /// Original remote URL (for remote_url type)
    var remoteUrl: String?

    /// File size in bytes
    var sizeBytes: Int64

    /// Creation timestamp
    var createdAt: Date

    // MARK: - Table Configuration

    static let databaseTableName = "attachments"

    // MARK: - Initialization

    /// Create a new stored attachment
    init(
        id: String = UUID().uuidString,
        messageId: String,
        attachmentType: AttachmentSourceType,
        mediaType: AttachmentMediaType,
        mimeType: String,
        filename: String? = nil,
        localPath: String? = nil,
        remoteUrl: String? = nil,
        sizeBytes: Int64 = 0,
        createdAt: Date = Date()
    ) {
        self.id = id
        self.messageId = messageId
        self.attachmentType = attachmentType
        self.mediaType = mediaType
        self.mimeType = mimeType
        self.filename = filename
        self.localPath = localPath
        self.remoteUrl = remoteUrl
        self.sizeBytes = sizeBytes
        self.createdAt = createdAt
    }

    // MARK: - Computed Properties

    /// Absolute URL to the local file (if exists)
    var fileURL: URL? {
        guard let localPath = localPath else { return nil }
        let baseDir = AttachmentFileManager.attachmentsDirectory
        return baseDir.appendingPathComponent(localPath)
    }

    /// Display URL (local file if available, otherwise remote URL)
    var displayURL: URL? {
        // Prefer local file if it exists
        if let fileURL = fileURL,
           FileManager.default.fileExists(atPath: fileURL.path) {
            return fileURL
        }
        // Fall back to remote URL
        if let remoteUrl = remoteUrl {
            return URL(string: remoteUrl)
        }
        return nil
    }

    /// Display filename (original name or generated name)
    var displayFilename: String {
        filename ?? (localPath.map { URL(fileURLWithPath: $0).lastPathComponent }) ?? "attachment"
    }

    /// Check if local file exists
    var hasLocalFile: Bool {
        guard let fileURL = fileURL else { return false }
        return FileManager.default.fileExists(atPath: fileURL.path)
    }
}

// MARK: - Conversion from PendingAttachment

extension StoredAttachment {

    /// Create StoredAttachment from PendingAttachment
    /// - Parameters:
    ///   - pending: The pending attachment to convert
    ///   - messageId: The message ID to link to
    ///   - localPath: The relative local path where file is stored
    init(from pending: PendingAttachment, messageId: String, localPath: String) {
        let mimeType: String
        switch pending.fileType {
        case .image:
            let ext = pending.url.pathExtension.lowercased()
            switch ext {
            case "png": mimeType = "image/png"
            case "jpg", "jpeg": mimeType = "image/jpeg"
            case "gif": mimeType = "image/gif"
            case "webp": mimeType = "image/webp"
            default: mimeType = "image/png"
            }
        case .document:
            let ext = pending.url.pathExtension.lowercased()
            switch ext {
            case "pdf": mimeType = "application/pdf"
            case "txt", "md": mimeType = "text/plain"
            default: mimeType = "application/octet-stream"
            }
        case .other:
            mimeType = "application/octet-stream"
        }

        self.init(
            messageId: messageId,
            attachmentType: .userUpload,
            mediaType: AttachmentMediaType.from(mimeType: mimeType),
            mimeType: mimeType,
            filename: pending.fileName,
            localPath: localPath,
            remoteUrl: nil,
            sizeBytes: Int64(pending.data.count)
        )
    }
}

// MARK: - Conversion for Tool Output

extension StoredAttachment {

    /// Create StoredAttachment for tool-generated output
    /// - Parameters:
    ///   - messageId: The message ID to link to
    ///   - toolName: Name of the tool that generated the output
    ///   - sourceURL: Source URL (local file or remote URL)
    ///   - localPath: Relative local path if file was copied
    static func forToolOutput(
        messageId: String,
        toolName: String,
        sourceURL: URL,
        localPath: String?,
        mimeType: String? = nil
    ) -> StoredAttachment {
        let isRemote = sourceURL.scheme?.hasPrefix("http") ?? false
        let detectedMime = mimeType ?? Self.detectMimeType(from: sourceURL)
        let mediaType = AttachmentMediaType.from(mimeType: detectedMime)

        return StoredAttachment(
            messageId: messageId,
            attachmentType: isRemote && localPath == nil ? .remoteUrl : .toolOutput,
            mediaType: mediaType,
            mimeType: detectedMime,
            filename: sourceURL.lastPathComponent,
            localPath: localPath,
            remoteUrl: isRemote ? sourceURL.absoluteString : nil,
            sizeBytes: Self.getFileSize(at: localPath)
        )
    }

    /// Detect MIME type from URL
    private static func detectMimeType(from url: URL) -> String {
        let ext = url.pathExtension.lowercased()
        switch ext {
        case "png": return "image/png"
        case "jpg", "jpeg": return "image/jpeg"
        case "gif": return "image/gif"
        case "webp": return "image/webp"
        case "pdf": return "application/pdf"
        case "txt", "md": return "text/plain"
        default: return "application/octet-stream"
        }
    }

    /// Get file size from local path
    private static func getFileSize(at relativePath: String?) -> Int64 {
        guard let relativePath = relativePath else { return 0 }
        let fullPath = AttachmentFileManager.attachmentsDirectory
            .appendingPathComponent(relativePath)
        guard let attrs = try? FileManager.default.attributesOfItem(atPath: fullPath.path),
              let size = attrs[.size] as? Int64 else {
            return 0
        }
        return size
    }
}
