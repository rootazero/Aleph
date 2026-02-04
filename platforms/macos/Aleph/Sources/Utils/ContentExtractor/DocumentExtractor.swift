// DocumentExtractor.swift
// Extracts text content from document files (PDF, TXT, MD) copied to clipboard
//
// This extractor handles document files copied from Finder, extracting their
// text content for use as context in AI conversations.
//
// Supported formats:
// - PDF: Text extracted using PDFKit
// - TXT: Plain text read directly
// - MD: Markdown text read directly
//
// add-document-attachment-support

import Cocoa
import os.log
import PDFKit

/// Extractor for document files (PDF, TXT, MD) from file URLs
///
/// Priority: 45 (after FileURLExtractor for images)
///
/// Handles document files copied from:
/// - Finder file selection (Cmd+C on files)
/// - Desktop file selection
/// - Other file browsers
final class DocumentExtractor: ContentExtractor {
    // MARK: - ContentExtractor Protocol

    let identifier = "document"
    let priority = 45

    let supportedTypes: [NSPasteboard.PasteboardType] = [
        NSPasteboard.PasteboardType("public.file-url")
    ]

    private let logger = Logger(subsystem: "com.aleph", category: "DocumentExtractor")

    // MARK: - Extraction

    func canExtract(from pasteboard: NSPasteboard) -> Bool {
        guard let types = pasteboard.types else { return false }

        // Check if pasteboard has file URLs
        guard types.contains(NSPasteboard.PasteboardType("public.file-url")) else {
            return false
        }

        // Check if any file URL is a document type
        guard let urls = pasteboard.readObjects(forClasses: [NSURL.self], options: [
            .urlReadingFileURLsOnly: true
        ]) as? [URL] else {
            return false
        }

        return urls.contains { url in
            SupportedMediaType.isDocumentSupported(url.pathExtension)
        }
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
            // Check file extension is a supported document type
            let ext = url.pathExtension.lowercased()
            guard SupportedMediaType.isDocumentSupported(ext) else {
                continue
            }

            // Check file size before loading
            do {
                let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
                if let fileSize = attributes[.size] as? UInt64 {
                    if fileSize > MediaSizeLimits.maxDocumentSizeBytes {
                        let sizeMB = Double(fileSize) / (1024.0 * 1024.0)
                        let errorMessage = String(
                            format: "Document \"%@\" (%.1fMB) exceeds the maximum limit of %@.",
                            url.lastPathComponent,
                            sizeMB,
                            MediaSizeLimits.maxDocumentSizeDescription
                        )
                        logger.error("Document too large: \(url.lastPathComponent) (\(fileSize) bytes)")
                        return ExtractionResult(
                            text: nil,
                            attachments: [],
                            handledTypes: [],
                            metadata: ["extractor": identifier],
                            error: errorMessage
                        )
                    }
                }
            } catch {
                logger.error("Failed to get file attributes: \(error.localizedDescription)")
            }

            // Extract text content based on file type
            guard let textContent = extractTextContent(from: url, ext: ext) else {
                logger.warning("Failed to extract text from: \(url.lastPathComponent)")
                continue
            }

            // Truncate if needed
            let truncatedContent = truncateIfNeeded(textContent)
            let wasTruncated = truncatedContent.count < textContent.count

            if wasTruncated {
                logger.info("Document truncated to \(MediaSizeLimits.maxDocumentTextLength) chars: \(url.lastPathComponent)")
            }

            // Get MIME type
            let mimeType = SupportedMediaType.from(extension: ext)?.mimeType ?? "text/plain"

            let attachment = MediaAttachment(
                mediaType: "document",
                mimeType: mimeType,
                data: truncatedContent,
                encoding: "utf8",
                filename: url.lastPathComponent,
                sizeBytes: UInt64(truncatedContent.utf8.count)
            )

            attachments.append(attachment)
            logger.debug("Extracted document: \(url.lastPathComponent) (\(truncatedContent.count) chars)")
        }

        // Don't mark public.file-url as handled so FileURLExtractor can also process it
        // Each extractor filters for specific file types internally
        return ExtractionResult(
            text: nil,
            attachments: attachments,
            handledTypes: [],
            metadata: [
                "extractor": identifier,
                "document_count": attachments.count
            ],
            error: nil
        )
    }

    // MARK: - Private Helpers

    /// Extract text content from a document file
    ///
    /// - Parameters:
    ///   - url: File URL to extract from
    ///   - ext: File extension (lowercase)
    /// - Returns: Extracted text content or nil if extraction fails
    private func extractTextContent(from url: URL, ext: String) -> String? {
        switch ext {
        case "pdf":
            return extractPDFText(from: url)
        case "txt", "md":
            return readTextFile(from: url)
        default:
            return nil
        }
    }

    /// Extract text from PDF using PDFKit
    ///
    /// - Parameter url: PDF file URL
    /// - Returns: Extracted text or nil if extraction fails
    private func extractPDFText(from url: URL) -> String? {
        guard let document = PDFDocument(url: url) else {
            logger.error("Failed to open PDF: \(url.lastPathComponent)")
            return nil
        }

        var text = ""
        for i in 0..<document.pageCount {
            if let page = document.page(at: i),
               let pageText = page.string {
                text += pageText
                // Add page separator for readability
                if i < document.pageCount - 1 {
                    text += "\n\n"
                }
            }
        }

        if text.isEmpty {
            logger.warning("PDF has no extractable text: \(url.lastPathComponent)")
            return nil
        }

        return text
    }

    /// Read text file content directly
    ///
    /// - Parameter url: Text file URL
    /// - Returns: File content or nil if reading fails
    private func readTextFile(from url: URL) -> String? {
        do {
            // Check if file exists and is readable
            guard FileManager.default.fileExists(atPath: url.path) else {
                logger.error("File not found: \(url.path)")
                return nil
            }

            guard FileManager.default.isReadableFile(atPath: url.path) else {
                logger.error("No read permission: \(url.path)")
                return nil
            }

            return try String(contentsOf: url, encoding: .utf8)
        } catch {
            logger.error("Failed to read text file: \(url.path) - \(error.localizedDescription)")
            return nil
        }
    }

    /// Truncate text content if it exceeds the maximum length
    ///
    /// - Parameter content: Original text content
    /// - Returns: Truncated content with ellipsis if needed
    private func truncateIfNeeded(_ content: String) -> String {
        let maxLength = MediaSizeLimits.maxDocumentTextLength

        if content.count <= maxLength {
            return content
        }

        // Truncate and add indicator
        let truncated = String(content.prefix(maxLength))
        return truncated + "\n\n[... Document truncated at \(maxLength) characters ...]"
    }
}
