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

        let attachments = extractImagesFromRTFD(rtfdData)

        if attachments.isEmpty {
            logger.debug("No images found in RTFD content")
        }

        return ExtractionResult(
            text: nil,
            attachments: attachments,
            handledTypes: [.rtfd],
            metadata: [
                "extractor": identifier,
                "rtfd_size": rtfdData.count
            ]
        )
    }

    // MARK: - Private Helpers

    /// Extract embedded images from RTFD data
    ///
    /// RTFD content is parsed as NSAttributedString, then we enumerate
    /// NSTextAttachment attributes to find embedded images.
    ///
    /// - Parameter rtfdData: Raw RTFD data from pasteboard
    /// - Returns: Array of MediaAttachment for each embedded image
    private func extractImagesFromRTFD(_ rtfdData: Data) -> [MediaAttachment] {
        var attachments: [MediaAttachment] = []

        // Parse RTFD data as NSAttributedString
        guard let attrString = try? NSAttributedString(
            data: rtfdData,
            options: [.documentType: NSAttributedString.DocumentType.rtfd],
            documentAttributes: nil
        ) else {
            logger.error("Failed to parse RTFD data")
            return []
        }

        // Enumerate NSTextAttachment attributes
        let fullRange = NSRange(location: 0, length: attrString.length)
        attrString.enumerateAttribute(.attachment, in: fullRange, options: []) { value, _, _ in
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

            // Check size limits
            let sizeBytes = UInt64(data.count)
            if sizeBytes > MediaSizeLimits.maxImageSizeBytes {
                logger.error("RTFD attachment too large: \(sizeBytes) bytes")
                return
            }

            if sizeBytes > MediaSizeLimits.warnImageSizeBytes {
                logger.warning("Large RTFD attachment: \(sizeBytes) bytes")
            }

            // Get MIME type
            let mimeType = SupportedMediaType.from(extension: ext)?.mimeType ?? "application/octet-stream"

            // Convert to Base64
            let base64Data = data.base64EncodedString()

            let attachment = MediaAttachment(
                mediaType: "image",
                mimeType: mimeType,
                data: base64Data,
                filename: filename,
                sizeBytes: sizeBytes
            )

            attachments.append(attachment)
            logger.debug("Extracted RTFD attachment: \(filename) (\(sizeBytes) bytes)")
        }

        return attachments
    }
}
