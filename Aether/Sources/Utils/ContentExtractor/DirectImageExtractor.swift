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
                // CRITICAL: Convert TIFF to PNG for API compatibility
                // Most AI APIs only support: jpeg, png, gif, webp
                // macOS clipboard often stores images as TIFF (e.g., screenshots)
                let (finalData, finalMimeType) = convertToApiCompatibleFormat(data: data, originalType: type)

                // Check size limits (after conversion)
                let sizeBytes = UInt64(finalData.count)
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

                // Convert to Base64
                let base64Data = finalData.base64EncodedString()

                let attachment = MediaAttachment(
                    mediaType: "image",
                    mimeType: finalMimeType,
                    data: base64Data,
                    filename: nil,
                    sizeBytes: sizeBytes
                )

                attachments.append(attachment)
                handledTypes.insert(type)

                logger.debug("Extracted \(type.rawValue) image as \(finalMimeType): \(sizeBytes) bytes")

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

    /// Convert image data to API-compatible format
    ///
    /// AI APIs typically only support: jpeg, png, gif, webp
    /// TIFF (common in macOS clipboard) must be converted to PNG
    ///
    /// - Parameters:
    ///   - data: Original image data
    ///   - originalType: Original pasteboard type
    /// - Returns: Tuple of (converted data, mime type)
    private func convertToApiCompatibleFormat(data: Data, originalType: NSPasteboard.PasteboardType) -> (Data, String) {
        // PNG and JPEG are already API-compatible
        if originalType == .png {
            return (data, "image/png")
        }
        if originalType == NSPasteboard.PasteboardType("public.jpeg") {
            return (data, "image/jpeg")
        }

        // TIFF needs conversion to PNG
        if originalType == .tiff {
            logger.info("Converting TIFF to PNG for API compatibility")

            // Create NSImage from TIFF data
            guard let image = NSImage(data: data) else {
                logger.error("Failed to create NSImage from TIFF data, using original")
                return (data, "image/tiff")
            }

            // Convert to PNG
            if let pngData = convertToPNG(image: image) {
                logger.info("Successfully converted TIFF to PNG (\(data.count) -> \(pngData.count) bytes)")
                return (pngData, "image/png")
            } else {
                logger.error("Failed to convert TIFF to PNG, using original")
                return (data, "image/tiff")
            }
        }

        // Unknown type - return as-is
        return (data, "application/octet-stream")
    }

    /// Convert NSImage to PNG data
    ///
    /// - Parameter image: Source image
    /// - Returns: PNG data or nil if conversion fails
    private func convertToPNG(image: NSImage) -> Data? {
        // Get the best representation
        guard let tiffData = image.tiffRepresentation,
              let bitmap = NSBitmapImageRep(data: tiffData) else {
            return nil
        }

        // Convert to PNG
        return bitmap.representation(using: .png, properties: [:])
    }

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
