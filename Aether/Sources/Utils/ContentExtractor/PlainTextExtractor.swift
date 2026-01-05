// PlainTextExtractor.swift
// Fallback extractor for plain text content
//
// This is the lowest priority extractor and handles text-only clipboard content.
// It's mainly used as a fallback when no media is present.
//
// add-multimodal-content-support

import Cocoa
import os.log

/// Extractor for plain text from pasteboard
///
/// Priority: 80 (fallback)
///
/// Handles:
/// - Plain text copied from any application
/// - Fallback when no media extractors match
final class PlainTextExtractor: ContentExtractor {
    // MARK: - ContentExtractor Protocol

    let identifier = "plain-text"
    let priority = 80

    let supportedTypes: [NSPasteboard.PasteboardType] = [.string]

    private let logger = Logger(subsystem: "com.aether", category: "PlainTextExtractor")

    // MARK: - Extraction

    func canExtract(from pasteboard: NSPasteboard) -> Bool {
        guard let types = pasteboard.types else { return false }
        return types.contains(.string)
    }

    func extract(from pasteboard: NSPasteboard) -> ExtractionResult {
        guard let text = pasteboard.string(forType: .string) else {
            return .empty
        }

        logger.debug("Extracted plain text: \(text.prefix(50))...")

        return ExtractionResult(
            text: text,
            attachments: [],
            handledTypes: [.string],
            metadata: [
                "extractor": identifier,
                "text_length": text.count
            ],
            error: nil
        )
    }
}
