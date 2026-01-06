// FileURLExtractor.swift
// Extracts images from file URLs copied to clipboard (e.g., from Finder)
//
// When files are copied in Finder, the clipboard contains file URLs, not the
// actual file content. This extractor loads the file content from disk.
//
// add-multimodal-content-support

import Cocoa
import os.log

/// Extractor for file URLs (Finder copied files)
///
/// Priority: 40 (requires disk I/O)
///
/// Handles files copied from:
/// - Finder file selection (Cmd+C on files)
/// - Desktop file selection
/// - Other file browsers
final class FileURLExtractor: ContentExtractor {
    // MARK: - ContentExtractor Protocol

    let identifier = "file-url"
    let priority = 40

    let supportedTypes: [NSPasteboard.PasteboardType] = [
        NSPasteboard.PasteboardType("public.file-url")
    ]

    private let logger = Logger(subsystem: "com.aether", category: "FileURLExtractor")

    // MARK: - Extraction

    func canExtract(from pasteboard: NSPasteboard) -> Bool {
        guard let types = pasteboard.types else { return false }
        return types.contains(NSPasteboard.PasteboardType("public.file-url"))
    }

    func extract(from pasteboard: NSPasteboard) -> ExtractionResult {
        // Read file URLs from pasteboard
        guard let urls = pasteboard.readObjects(forClasses: [NSURL.self], options: [
            .urlReadingFileURLsOnly: true
        ]) as? [URL] else {
            return .empty
        }

        var attachments: [MediaAttachment] = []

        for url in urls {
            // Check file extension is a supported image type (not document)
            // Documents (PDF, TXT, MD) are handled by DocumentExtractor
            let ext = url.pathExtension.lowercased()
            guard SupportedMediaType.isImageSupported(ext) else {
                logger.info("Skipping non-image file type: \(ext)")
                continue
            }

            // Load file content from disk
            guard let data = loadFileContent(from: url) else {
                continue
            }

            // Check size limits
            let sizeBytes = UInt64(data.count)
            if sizeBytes > MediaSizeLimits.maxImageSizeBytes {
                let sizeMB = Double(sizeBytes) / (1024.0 * 1024.0)
                let errorMessage = String(format: "File \"%@\" (%.1fMB) exceeds the maximum limit of %@. Please use a smaller file.", url.lastPathComponent, sizeMB, MediaSizeLimits.maxImageSizeDescription)
                logger.error("File too large: \(url.lastPathComponent) (\(sizeBytes) bytes)")
                // Return error immediately to stop processing
                return ExtractionResult(
                    text: nil,
                    attachments: [],
                    handledTypes: [],
                    metadata: ["extractor": identifier],
                    error: errorMessage
                )
            }

            if sizeBytes > MediaSizeLimits.warnImageSizeBytes {
                logger.warning("Large file: \(url.lastPathComponent) (\(sizeBytes) bytes)")
            }

            // Get MIME type and convert to API-compatible format if needed
            let originalMimeType = SupportedMediaType.from(extension: ext)?.mimeType ?? "application/octet-stream"
            let (finalData, finalMimeType) = ImageFormatConverter.convertIfNeeded(data: data, mimeType: originalMimeType)

            // Update size after conversion
            let finalSizeBytes = UInt64(finalData.count)

            // Convert to Base64
            let base64Data = finalData.base64EncodedString()

            let attachment = MediaAttachment(
                mediaType: "image",
                mimeType: finalMimeType,
                data: base64Data,
                filename: url.lastPathComponent,
                sizeBytes: finalSizeBytes
            )

            attachments.append(attachment)
            logger.debug("Extracted file: \(url.lastPathComponent) (\(sizeBytes) bytes)")
        }

        // Don't mark public.file-url as handled so DocumentExtractor can also process it
        // Each extractor filters for specific file types internally
        return ExtractionResult(
            text: nil,
            attachments: attachments,
            handledTypes: [],
            metadata: [
                "extractor": identifier,
                "file_count": urls.count
            ],
            error: nil
        )
    }

    // MARK: - Private Helpers

    /// Load file content from disk
    ///
    /// - Parameter url: File URL to load
    /// - Returns: File data or nil if loading fails
    private func loadFileContent(from url: URL) -> Data? {
        do {
            // Check if file exists
            guard FileManager.default.fileExists(atPath: url.path) else {
                logger.error("File not found: \(url.path)")
                return nil
            }

            // Check if we have read access
            guard FileManager.default.isReadableFile(atPath: url.path) else {
                logger.error("No read permission: \(url.path)")
                return nil
            }

            return try Data(contentsOf: url)
        } catch {
            logger.error("Failed to load file: \(url.path) - \(error.localizedDescription)")
            return nil
        }
    }
}
