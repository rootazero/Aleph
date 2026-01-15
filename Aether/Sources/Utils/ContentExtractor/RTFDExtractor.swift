// RTFDExtractor.swift
// Extracts embedded images from RTFD (Rich Text Format Directory) content
//
// RTFD is used by Notes, Mail, and other Apple applications when copying
// rich content with embedded images. The images are stored as NSTextAttachment
// objects within the attributed string.
//
// add-multimodal-content-support

import Cocoa
import os.log

/// Extractor for RTFD (Rich Text Format Directory) with embedded attachments
///
/// Priority: 20 (after direct images)
///
/// Handles content copied from:
/// - Apple Notes with embedded images
/// - Apple Mail with inline images
/// - TextEdit with embedded images
/// - Other apps using NSTextView with attachments
final class RTFDExtractor: ContentExtractor {
    // MARK: - ContentExtractor Protocol

    let identifier = "rtfd"
    let priority = 20

    let supportedTypes: [NSPasteboard.PasteboardType] = [.rtfd]

    private let logger = Logger(subsystem: "com.aether", category: "RTFDExtractor")

    // MARK: - Extraction

    func canExtract(from pasteboard: NSPasteboard) -> Bool {
        guard let types = pasteboard.types else { return false }
        return types.contains(.rtfd)
    }

    func extract(from pasteboard: NSPasteboard) -> ExtractionResult {
        guard let rtfdData = pasteboard.data(forType: .rtfd) else {
            return .empty
        }

        let result = extractImagesFromRTFD(rtfdData)

        // Check for error first
        if let error = result.error {
            return ExtractionResult(
                text: nil,
                attachments: [],
                handledTypes: [],
                metadata: ["extractor": identifier],
                error: error
            )
        }

        if result.attachments.isEmpty {
            logger.debug("No images found in RTFD content")
        }

        return ExtractionResult(
            text: nil,
            attachments: result.attachments,
            handledTypes: [.rtfd],
            metadata: [
                "extractor": identifier,
                "rtfd_size": rtfdData.count
            ],
            error: nil
        )
    }

    // MARK: - Private Helpers

    /// Result type for RTFD image extraction
    private struct RTFDExtractionResult {
        let attachments: [MediaAttachment]
        let error: String?
    }

    /// Extract embedded images from RTFD data
    ///
    /// RTFD content is parsed as NSAttributedString, then we enumerate
    /// NSTextAttachment attributes to find embedded images.
    ///
    /// - Parameter rtfdData: Raw RTFD data from pasteboard
    /// - Returns: RTFDExtractionResult with attachments or error
    private func extractImagesFromRTFD(_ rtfdData: Data) -> RTFDExtractionResult {
        var attachments: [MediaAttachment] = []
        var oversizeError: String?

        // Parse RTFD data as NSAttributedString
        guard let attrString = try? NSAttributedString(
            data: rtfdData,
            options: [.documentType: NSAttributedString.DocumentType.rtfd],
            documentAttributes: nil
        ) else {
            logger.error("Failed to parse RTFD data")
            return RTFDExtractionResult(attachments: [], error: nil)
        }

        // Enumerate NSTextAttachment attributes
        let fullRange = NSRange(location: 0, length: attrString.length)
        attrString.enumerateAttribute(.attachment, in: fullRange, options: []) { value, _, stop in
            guard let textAttachment = value as? NSTextAttachment,
                  let fileWrapper = textAttachment.fileWrapper else {
                return
            }

            // IMPORTANT: Use fileWrapper.regularFileContents, not attachment.contents
            // attachment.contents may be empty when converting RTFD back to NSAttributedString
            guard let data = fileWrapper.regularFileContents else {
                logger.debug("No file contents in attachment")
                return
            }

            // Get filename from fileWrapper
            let filename = fileWrapper.preferredFilename ?? "attachment"
            let ext = (filename as NSString).pathExtension.lowercased()

            // Only process images in Phase 1
            guard SupportedMediaType.isSupported(ext) else {
                logger.info("Skipping unsupported attachment type: \(ext)")
                return
            }

            // Check size limits - return error immediately if exceeded
            let sizeBytes = UInt64(data.count)
            if sizeBytes > MediaSizeLimits.maxImageSizeBytes {
                let sizeMB = Double(sizeBytes) / (1024.0 * 1024.0)
                oversizeError = String(format: "Embedded image \"%@\" (%.1fMB) exceeds the maximum limit of %@. Please use a smaller image.", filename, sizeMB, MediaSizeLimits.maxImageSizeDescription)
                logger.error("RTFD attachment too large: \(filename) (\(sizeBytes) bytes)")
                // Stop enumeration
                stop.pointee = true
                return
            }

            if sizeBytes > MediaSizeLimits.warnImageSizeBytes {
                logger.warning("Large RTFD attachment: \(sizeBytes) bytes")
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
                encoding: "base64",
                filename: filename,
                sizeBytes: finalSizeBytes
            )

            attachments.append(attachment)
            logger.debug("Extracted RTFD attachment: \(filename) (\(sizeBytes) bytes)")
        }

        return RTFDExtractionResult(attachments: attachments, error: oversizeError)
    }
}
