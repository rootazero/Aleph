//
//  AttachmentFileManager.swift
//  Aether
//
//  File system management for attachments.
//  Handles storage, retrieval, and cleanup of attachment files.
//

import Foundation
import AppKit

// MARK: - AttachmentFileManager

/// Manages file system operations for attachments
///
/// Directory structure:
/// ```
/// ~/.config/aether/
/// ├── conversations.db
/// └── attachments/
///     ├── user/{messageId}/         # User uploaded files
///     │   └── {uuid}_{filename}
///     ├── generated/{messageId}/    # Tool-generated files
///     │   └── {uuid}_{tool}_{ts}.{ext}
///     └── cached/{messageId}/       # Remote URL cache
///         └── {hash}_{filename}
/// ```
///
/// Thread Safety:
/// - Marked as @unchecked Sendable because FileManager operations are thread-safe
final class AttachmentFileManager: @unchecked Sendable {

    // MARK: - Singleton

    static let shared = AttachmentFileManager()

    // MARK: - Directory Paths

    /// Base attachments directory
    static var attachmentsDirectory: URL {
        let configDir = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/aether/attachments")
        return configDir
    }

    /// User uploads directory
    static var userUploadsDirectory: URL {
        attachmentsDirectory.appendingPathComponent("user")
    }

    /// Tool-generated files directory
    static var generatedDirectory: URL {
        attachmentsDirectory.appendingPathComponent("generated")
    }

    /// Cached remote files directory
    static var cachedDirectory: URL {
        attachmentsDirectory.appendingPathComponent("cached")
    }

    // MARK: - Initialization

    private init() {
        createDirectories()
    }

    /// Create all required directories
    private func createDirectories() {
        let fm = FileManager.default
        let dirs = [
            Self.attachmentsDirectory,
            Self.userUploadsDirectory,
            Self.generatedDirectory,
            Self.cachedDirectory
        ]

        for dir in dirs {
            if !fm.fileExists(atPath: dir.path) {
                do {
                    try fm.createDirectory(at: dir, withIntermediateDirectories: true)
                    print("[AttachmentFileManager] Created directory: \(dir.path)")
                } catch {
                    print("[AttachmentFileManager] Failed to create directory: \(error)")
                }
            }
        }
    }

    // MARK: - Save Operations

    /// Save user-uploaded attachment
    /// - Parameters:
    ///   - data: File data
    ///   - filename: Original filename
    ///   - messageId: Associated message ID
    /// - Returns: Relative path for database storage
    func saveUserUpload(data: Data, filename: String, messageId: String) -> String? {
        let fm = FileManager.default

        // Create message-specific directory
        let messageDir = Self.userUploadsDirectory.appendingPathComponent(messageId)
        do {
            try fm.createDirectory(at: messageDir, withIntermediateDirectories: true)
        } catch {
            print("[AttachmentFileManager] Failed to create message directory: \(error)")
            return nil
        }

        // Generate unique filename
        let uuid = UUID().uuidString.prefix(8)
        let safeFilename = sanitizeFilename(filename)
        let storedFilename = "\(uuid)_\(safeFilename)"
        let fileURL = messageDir.appendingPathComponent(storedFilename)

        // Write file
        do {
            try data.write(to: fileURL)
            let relativePath = "user/\(messageId)/\(storedFilename)"
            print("[AttachmentFileManager] Saved user upload: \(relativePath)")
            return relativePath
        } catch {
            print("[AttachmentFileManager] Failed to save user upload: \(error)")
            return nil
        }
    }

    /// Save user-uploaded attachment from PendingAttachment
    /// - Parameters:
    ///   - pending: The pending attachment
    ///   - messageId: Associated message ID
    /// - Returns: Relative path for database storage
    func saveUserUpload(from pending: PendingAttachment, messageId: String) -> String? {
        return saveUserUpload(data: pending.data, filename: pending.fileName, messageId: messageId)
    }

    /// Save tool-generated file
    /// - Parameters:
    ///   - sourceURL: Source file URL (local file)
    ///   - toolName: Name of the tool that generated it
    ///   - messageId: Associated message ID
    /// - Returns: Relative path for database storage
    func saveGeneratedFile(from sourceURL: URL, toolName: String, messageId: String) -> String? {
        let fm = FileManager.default

        // Create message-specific directory
        let messageDir = Self.generatedDirectory.appendingPathComponent(messageId)
        do {
            try fm.createDirectory(at: messageDir, withIntermediateDirectories: true)
        } catch {
            print("[AttachmentFileManager] Failed to create message directory: \(error)")
            return nil
        }

        // Generate filename: {uuid}_{tool}_{timestamp}.{ext}
        let uuid = UUID().uuidString.prefix(8)
        let timestamp = Int(Date().timeIntervalSince1970)
        let ext = sourceURL.pathExtension.isEmpty ? "bin" : sourceURL.pathExtension
        let storedFilename = "\(uuid)_\(toolName)_\(timestamp).\(ext)"
        let destURL = messageDir.appendingPathComponent(storedFilename)

        // Copy file
        do {
            if fm.fileExists(atPath: destURL.path) {
                try fm.removeItem(at: destURL)
            }
            try fm.copyItem(at: sourceURL, to: destURL)
            let relativePath = "generated/\(messageId)/\(storedFilename)"
            print("[AttachmentFileManager] Saved generated file: \(relativePath)")
            return relativePath
        } catch {
            print("[AttachmentFileManager] Failed to save generated file: \(error)")
            return nil
        }
    }

    /// Cache remote URL locally
    /// - Parameters:
    ///   - url: Remote URL
    ///   - data: Downloaded data
    ///   - messageId: Associated message ID
    /// - Returns: Relative path for database storage
    func cacheRemoteFile(url: URL, data: Data, messageId: String) -> String? {
        let fm = FileManager.default

        // Create message-specific directory
        let messageDir = Self.cachedDirectory.appendingPathComponent(messageId)
        do {
            try fm.createDirectory(at: messageDir, withIntermediateDirectories: true)
        } catch {
            print("[AttachmentFileManager] Failed to create message directory: \(error)")
            return nil
        }

        // Generate filename from URL hash + original filename
        let hash = String(url.absoluteString.hashValue.magnitude).prefix(8)
        let filename = url.lastPathComponent.isEmpty ? "cached_file" : url.lastPathComponent
        let storedFilename = "\(hash)_\(sanitizeFilename(filename))"
        let fileURL = messageDir.appendingPathComponent(storedFilename)

        // Write file
        do {
            try data.write(to: fileURL)
            let relativePath = "cached/\(messageId)/\(storedFilename)"
            print("[AttachmentFileManager] Cached remote file: \(relativePath)")
            return relativePath
        } catch {
            print("[AttachmentFileManager] Failed to cache remote file: \(error)")
            return nil
        }
    }

    // MARK: - Read Operations

    /// Get absolute URL for a relative path
    /// - Parameter relativePath: Path relative to attachments directory
    /// - Returns: Absolute file URL
    func getFileURL(relativePath: String) -> URL {
        return Self.attachmentsDirectory.appendingPathComponent(relativePath)
    }

    /// Check if file exists
    /// - Parameter relativePath: Path relative to attachments directory
    /// - Returns: True if file exists
    func fileExists(relativePath: String) -> Bool {
        let url = getFileURL(relativePath: relativePath)
        return FileManager.default.fileExists(atPath: url.path)
    }

    /// Get file data
    /// - Parameter relativePath: Path relative to attachments directory
    /// - Returns: File data, or nil if not found
    func getFileData(relativePath: String) -> Data? {
        let url = getFileURL(relativePath: relativePath)
        return try? Data(contentsOf: url)
    }

    /// Get thumbnail for image attachment
    /// - Parameters:
    ///   - relativePath: Path relative to attachments directory
    ///   - maxSize: Maximum dimension for thumbnail
    /// - Returns: Thumbnail image, or nil if failed
    func getThumbnail(relativePath: String, maxSize: CGFloat = 64) -> NSImage? {
        let url = getFileURL(relativePath: relativePath)
        guard let image = NSImage(contentsOf: url) else { return nil }

        // Calculate thumbnail size
        let originalSize = image.size
        let scale = min(maxSize / originalSize.width, maxSize / originalSize.height, 1.0)
        let targetSize = NSSize(
            width: originalSize.width * scale,
            height: originalSize.height * scale
        )

        // Create thumbnail
        let thumbnail = NSImage(size: targetSize)
        thumbnail.lockFocus()
        image.draw(
            in: NSRect(origin: .zero, size: targetSize),
            from: NSRect(origin: .zero, size: originalSize),
            operation: .copy,
            fraction: 1.0
        )
        thumbnail.unlockFocus()

        return thumbnail
    }

    // MARK: - Delete Operations

    /// Delete a single file
    /// - Parameter relativePath: Path relative to attachments directory
    /// - Returns: True if deleted successfully
    @discardableResult
    func deleteFile(relativePath: String) -> Bool {
        let url = getFileURL(relativePath: relativePath)
        do {
            try FileManager.default.removeItem(at: url)
            print("[AttachmentFileManager] Deleted file: \(relativePath)")
            return true
        } catch {
            print("[AttachmentFileManager] Failed to delete file: \(error)")
            return false
        }
    }

    /// Delete all files for a message
    /// - Parameter messageId: The message ID
    /// - Returns: Number of deleted files
    @discardableResult
    func deleteFilesForMessage(_ messageId: String) -> Int {
        let fm = FileManager.default
        var deleted = 0

        // Delete from all subdirectories
        let subDirs = ["user", "generated", "cached"]
        for subDir in subDirs {
            let messageDir = Self.attachmentsDirectory
                .appendingPathComponent(subDir)
                .appendingPathComponent(messageId)

            if fm.fileExists(atPath: messageDir.path) {
                do {
                    let files = try fm.contentsOfDirectory(at: messageDir, includingPropertiesForKeys: nil)
                    deleted += files.count
                    try fm.removeItem(at: messageDir)
                    print("[AttachmentFileManager] Deleted message directory: \(messageDir.path)")
                } catch {
                    print("[AttachmentFileManager] Failed to delete message directory: \(error)")
                }
            }
        }

        return deleted
    }

    /// Delete all files for a topic
    /// - Parameter attachmentPaths: Array of local paths to delete
    /// - Returns: Number of deleted files
    @discardableResult
    func deleteFiles(paths: [String]) -> Int {
        var deleted = 0
        for path in paths {
            if deleteFile(relativePath: path) {
                deleted += 1
            }
        }
        return deleted
    }

    /// Clean up empty directories
    func cleanupEmptyDirectories() {
        let fm = FileManager.default
        let subDirs = ["user", "generated", "cached"]

        for subDir in subDirs {
            let dir = Self.attachmentsDirectory.appendingPathComponent(subDir)
            guard let contents = try? fm.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil) else {
                continue
            }

            for item in contents {
                var isDirectory: ObjCBool = false
                if fm.fileExists(atPath: item.path, isDirectory: &isDirectory),
                   isDirectory.boolValue {
                    // Check if directory is empty
                    if let dirContents = try? fm.contentsOfDirectory(atPath: item.path),
                       dirContents.isEmpty {
                        try? fm.removeItem(at: item)
                        print("[AttachmentFileManager] Removed empty directory: \(item.path)")
                    }
                }
            }
        }
    }

    // MARK: - Storage Statistics

    /// Get total storage used by attachments
    /// - Returns: Total size in bytes
    func getTotalStorageUsed() -> UInt64 {
        let fm = FileManager.default
        var totalSize: UInt64 = 0

        let enumerator = fm.enumerator(
            at: Self.attachmentsDirectory,
            includingPropertiesForKeys: [.fileSizeKey],
            options: [.skipsHiddenFiles]
        )

        while let fileURL = enumerator?.nextObject() as? URL {
            if let resourceValues = try? fileURL.resourceValues(forKeys: [.fileSizeKey]),
               let fileSize = resourceValues.fileSize {
                totalSize += UInt64(fileSize)
            }
        }

        return totalSize
    }

    /// Format storage size for display
    /// - Parameter bytes: Size in bytes
    /// - Returns: Formatted string (e.g., "1.2 MB")
    func formatStorageSize(_ bytes: UInt64) -> String {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .file
        return formatter.string(fromByteCount: Int64(bytes))
    }

    // MARK: - Helpers

    /// Sanitize filename for safe storage
    private func sanitizeFilename(_ filename: String) -> String {
        // Remove or replace unsafe characters
        let invalidChars = CharacterSet(charactersIn: "/\\:*?\"<>|")
        var sanitized = filename.components(separatedBy: invalidChars).joined(separator: "_")

        // Limit length
        if sanitized.count > 100 {
            let ext = (filename as NSString).pathExtension
            let name = String(sanitized.prefix(90))
            sanitized = ext.isEmpty ? name : "\(name).\(ext)"
        }

        return sanitized.isEmpty ? "file" : sanitized
    }
}

// MARK: - Async Download Support

extension AttachmentFileManager {

    /// Download and cache a remote image
    /// - Parameters:
    ///   - url: Remote URL
    ///   - messageId: Associated message ID
    ///   - completion: Callback with relative path on success
    func downloadAndCache(url: URL, messageId: String, completion: @escaping (String?) -> Void) {
        let task = URLSession.shared.dataTask(with: url) { [weak self] data, response, error in
            guard let self = self else {
                completion(nil)
                return
            }

            if let error = error {
                print("[AttachmentFileManager] Download failed: \(error)")
                completion(nil)
                return
            }

            guard let data = data, !data.isEmpty else {
                print("[AttachmentFileManager] No data received")
                completion(nil)
                return
            }

            let relativePath = self.cacheRemoteFile(url: url, data: data, messageId: messageId)
            completion(relativePath)
        }
        task.resume()
    }

    /// Download and cache a remote image (async)
    /// - Parameters:
    ///   - url: Remote URL
    ///   - messageId: Associated message ID
    /// - Returns: Relative path on success, nil on failure
    func downloadAndCache(url: URL, messageId: String) async -> String? {
        do {
            let (data, _) = try await URLSession.shared.data(from: url)
            return cacheRemoteFile(url: url, data: data, messageId: messageId)
        } catch {
            print("[AttachmentFileManager] Async download failed: \(error)")
            return nil
        }
    }
}
