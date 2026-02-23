// BaseContentExtractor.swift
// Aleph
//
// Base class providing common functionality for content extractors.
// Reduces code duplication across DirectImageExtractor, RTFDExtractor,
// FileImageExtractor, and DocumentExtractor.

import Cocoa
import os.log

// MARK: - Base Content Extractor

/// Base class providing common functionality for content extractors
///
/// This class provides shared utilities for:
/// - Size limit checking (images and documents)
/// - Error message generation
/// - Base64 encoding
/// - API-compatible format conversion
/// - MediaAttachment creation
///
/// Extractors can either inherit from this class or use it as a helper.
class BaseContentExtractor {

    // MARK: - Size Validation

    /// Check if image data exceeds size limits
    ///
    /// - Parameters:
    ///   - sizeBytes: Size in bytes
    ///   - filename: Optional filename for error message
    /// - Returns: Error message if size exceeds limit, nil otherwise
    func checkImageSizeLimit(sizeBytes: UInt64, filename: String? = nil) -> String? {
        guard sizeBytes > MediaSizeLimits.maxImageSizeBytes else {
            return nil
        }

        let sizeMB = Double(sizeBytes) / (1024.0 * 1024.0)
        if let name = filename {
            return String(
                format: "File \"%@\" (%.1fMB) exceeds the maximum limit of %@. Please use a smaller file.",
                name, sizeMB, MediaSizeLimits.maxImageSizeDescription
            )
        } else {
            return String(
                format: "Image size (%.1fMB) exceeds the maximum limit of %@. Please use a smaller image.",
                sizeMB, MediaSizeLimits.maxImageSizeDescription
            )
        }
    }

    /// Check if document data exceeds size limits
    ///
    /// - Parameters:
    ///   - sizeBytes: Size in bytes
    ///   - filename: Filename for error message
    /// - Returns: Error message if size exceeds limit, nil otherwise
    func checkDocumentSizeLimit(sizeBytes: UInt64, filename: String) -> String? {
        guard sizeBytes > MediaSizeLimits.maxDocumentSizeBytes else {
            return nil
        }

        let sizeMB = Double(sizeBytes) / (1024.0 * 1024.0)
        return String(
            format: "Document \"%@\" (%.1fMB) exceeds the maximum limit of %@.",
            filename, sizeMB, MediaSizeLimits.maxDocumentSizeDescription
        )
    }

    /// Log warning for large but acceptable files
    ///
    /// - Parameters:
    ///   - sizeBytes: File size in bytes
    ///   - logger: Logger instance to use
    ///   - context: Context description for the log message
    func warnIfLarge(sizeBytes: UInt64, logger: Logger, context: String) {
        if sizeBytes > MediaSizeLimits.warnImageSizeBytes {
            logger.warning("Large \(context): \(sizeBytes) bytes")
        }
    }

    // MARK: - Encoding

    /// Encode data to Base64 string
    ///
    /// - Parameter data: Data to encode
    /// - Returns: Base64 encoded string
    func encodeBase64(_ data: Data) -> String {
        return data.base64EncodedString()
    }

    // MARK: - Format Conversion

    /// Convert image data to API-compatible format if needed
    ///
    /// AI APIs typically only support: jpeg, png, gif, webp
    /// TIFF (common in macOS clipboard) must be converted to PNG
    ///
    /// - Parameters:
    ///   - data: Original image data
    ///   - mimeType: Original MIME type
    /// - Returns: Tuple of (converted data, final mime type)
    func convertToApiCompatible(data: Data, mimeType: String) -> (Data, String) {
        return ImageFormatConverter.convertIfNeeded(data: data, mimeType: mimeType)
    }

    /// Convert pasteboard type to API-compatible format
    ///
    /// - Parameters:
    ///   - data: Original image data
    ///   - pasteboardType: Original pasteboard type
    /// - Returns: Tuple of (converted data, mime type)
    func convertToApiCompatible(data: Data, pasteboardType: NSPasteboard.PasteboardType) -> (Data, String) {
        let originalMimeType = mimeTypeFromPasteboardType(pasteboardType)
        return ImageFormatConverter.convertIfNeeded(data: data, mimeType: originalMimeType)
    }

    // MARK: - Attachment Creation

    /// Create an image MediaAttachment from processed data
    ///
    /// - Parameters:
    ///   - data: Processed (converted if needed) image data
    ///   - mimeType: Final MIME type (after conversion)
    ///   - filename: Optional filename
    /// - Returns: MediaAttachment ready for use
    func createImageAttachment(data: Data, mimeType: String, filename: String? = nil) -> MediaAttachment {
        let base64Data = encodeBase64(data)
        return MediaAttachment(
            mediaType: "image",
            mimeType: mimeType,
            data: base64Data,
            encoding: "base64",
            filename: filename,
            sizeBytes: UInt64(data.count)
        )
    }

    /// Create a document MediaAttachment from text content
    ///
    /// - Parameters:
    ///   - content: Text content of the document
    ///   - mimeType: MIME type (e.g., "text/plain", "text/markdown")
    ///   - filename: Filename for the document
    /// - Returns: MediaAttachment ready for use
    func createDocumentAttachment(content: String, mimeType: String, filename: String) -> MediaAttachment {
        return MediaAttachment(
            mediaType: "document",
            mimeType: mimeType,
            data: content,
            encoding: "utf8",
            filename: filename,
            sizeBytes: UInt64(content.utf8.count)
        )
    }

    // MARK: - Extraction Result Helpers

    /// Create an extraction result with an error
    ///
    /// - Parameters:
    ///   - error: Error message
    ///   - extractorId: Identifier of the extractor
    /// - Returns: ExtractionResult with error set
    func createErrorResult(error: String, extractorId: String) -> ExtractionResult {
        return ExtractionResult(
            text: nil,
            attachments: [],
            handledTypes: [],
            metadata: ["extractor": extractorId],
            error: error
        )
    }

    /// Create an extraction result with attachments
    ///
    /// - Parameters:
    ///   - attachments: Extracted attachments
    ///   - handledTypes: Pasteboard types that were handled
    ///   - extractorId: Identifier of the extractor
    /// - Returns: ExtractionResult with attachments
    func createAttachmentResult(
        attachments: [MediaAttachment],
        handledTypes: Set<NSPasteboard.PasteboardType>,
        extractorId: String
    ) -> ExtractionResult {
        return ExtractionResult(
            text: nil,
            attachments: attachments,
            handledTypes: handledTypes,
            metadata: ["extractor": extractorId],
            error: nil
        )
    }

    // MARK: - Private Helpers

    /// Get MIME type from pasteboard type
    private func mimeTypeFromPasteboardType(_ type: NSPasteboard.PasteboardType) -> String {
        switch type {
        case .png:
            return "image/png"
        case .tiff:
            return "image/tiff"
        case NSPasteboard.PasteboardType("public.jpeg"):
            return "image/jpeg"
        default:
            return "application/octet-stream"
        }
    }
}

// MARK: - Protocol Extension for Default Implementations

/// Extension providing default implementations for common operations
extension ContentExtractor {

    /// Default helper instance for shared utilities
    var helper: BaseContentExtractor {
        return BaseContentExtractor()
    }
}
