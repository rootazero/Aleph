// DirectImageExtractor.swift
// Extracts images directly from pasteboard (PNG, JPEG, TIFF)
//
// This is the highest priority extractor as it handles the most common case:
// images copied directly to clipboard (screenshots, copied images from web, etc.)
//
// add-multimodal-content-support

import Cocoa
import os.log

/// Extractor for direct image types (.png, .jpeg, .tiff) from pasteboard
///
/// Priority: 10 (highest - fastest path)
///
/// Handles images that are directly on the pasteboard, such as:
/// - Screenshots (Cmd+Shift+4)
/// - Images copied from web browsers
/// - Images copied from Preview/Photos
final class DirectImageExtractor: ContentExtractor {
    // MARK: - ContentExtractor Protocol

    let identifier = "direct-image"
    let priority = 10

    let supportedTypes: [NSPasteboard.PasteboardType] = [
        .png,
        .tiff,
        NSPasteboard.PasteboardType("public.jpeg")
    ]

    private let logger = Logger(subsystem: "com.aether", category: "DirectImageExtractor")

    // MARK: - Extraction

    func canExtract(from pasteboard: NSPasteboard) -> Bool {
        guard let types = pasteboard.types else { return false }
        return types.contains(where: supportedTypes.contains)
    }

    func extract(from pasteboard: NSPasteboard) -> ExtractionResult {
        guard let types = pasteboard.types else {
            return .empty
        }

        var attachments: [MediaAttachment] = []
        var handledTypes: Set<NSPasteboard.PasteboardType> = []

        // Try each supported type in order of preference (PNG > JPEG > TIFF)
        for type in supportedTypes {
            if types.contains(type), let data = pasteboard.data(forType: type) {
                // Check size limits
                let sizeBytes = UInt64(data.count)
                if sizeBytes > MediaSizeLimits.maxImageSizeBytes {
                    let sizeMB = Double(sizeBytes) / (1024.0 * 1024.0)
                    let errorMessage = String(format: "Image size (%.1fMB) exceeds the maximum limit of %@. Please use a smaller image.", sizeMB, MediaSizeLimits.maxImageSizeDescription)
                    logger.error("Image too large: \(sizeBytes) bytes (max: \(MediaSizeLimits.maxImageSizeBytes))")
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
                    logger.warning("Large image: \(sizeBytes) bytes")
                }

                // Determine MIME type
                let mimeType = self.mimeType(for: type)

                // Convert to Base64
                let base64Data = data.base64EncodedString()

                let attachment = MediaAttachment(
                    mediaType: "image",
                    mimeType: mimeType,
                    data: base64Data,
                    filename: nil,
                    sizeBytes: sizeBytes
                )

                attachments.append(attachment)
                handledTypes.insert(type)

                logger.debug("Extracted \(type.rawValue) image: \(sizeBytes) bytes")

                // Only extract the first available image type
                // (PNG and TIFF often both exist for same image)
                break
            }
        }

        return ExtractionResult(
            text: nil,
            attachments: attachments,
            handledTypes: handledTypes,
            metadata: ["extractor": identifier],
            error: nil
        )
    }

    // MARK: - Private Helpers

    private func mimeType(for type: NSPasteboard.PasteboardType) -> String {
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
