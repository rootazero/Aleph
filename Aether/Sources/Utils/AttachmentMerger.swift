// AttachmentMerger.swift
// Aether
//
// Centralized logic for merging media attachments from multiple sources.
// Implements the data order rule from CLAUDE.md:
// Final Data = Window Text + Clipboard Text Context + Clipboard Attachments + Window Attachments

import Foundation

/// Centralized logic for merging media attachments from multiple sources
///
/// This struct implements the data order rule defined in CLAUDE.md:
/// - Window Text + Clipboard Text Context + Clipboard Attachments + Window Attachments
///
/// Key Rules:
/// - Clipboard contains ONE type only (either text OR attachments like images/videos/PDFs)
/// - Window may contain BOTH text AND attachments (e.g., Notes app with embedded images)
/// - Text always comes first to ensure command prefixes (like `/en`) are at the beginning for routing
struct AttachmentMerger {

    // MARK: - Types

    /// Context for attachment assembly containing all input sources
    struct MergeContext {
        /// Attachments from clipboard (copied by user BEFORE trigger)
        /// These represent the user's intentional context
        let clipboardAttachments: [MediaAttachment]

        /// Attachments from window (extracted AFTER Cut/Copy operation)
        /// These come from the active window content (e.g., embedded images in Notes)
        let windowAttachments: [MediaAttachment]

        /// Text from recent clipboard (within 10 seconds, if clipboard had text not attachments)
        /// Used as additional context when user recently copied text
        let clipboardTextContext: String?

        /// Text from window (captured via Cut/Copy)
        /// This is the primary user input
        let windowText: String

        /// Initialize with all sources
        init(
            clipboardAttachments: [MediaAttachment] = [],
            windowAttachments: [MediaAttachment] = [],
            clipboardTextContext: String? = nil,
            windowText: String
        ) {
            self.clipboardAttachments = clipboardAttachments
            self.windowAttachments = windowAttachments
            self.clipboardTextContext = clipboardTextContext
            self.windowText = windowText
        }
    }

    /// Merged result containing final data in correct order
    struct MergeResult {
        /// Combined text: windowText + clipboardTextContext (separated by newlines if both present)
        let finalText: String

        /// Combined attachments in correct order:
        /// 1. Clipboard attachments (user's intentional context)
        /// 2. Window attachments (from active window content)
        let finalAttachments: [MediaAttachment]

        /// Statistics for logging
        let clipboardAttachmentCount: Int
        let windowAttachmentCount: Int
        let hasClipboardContext: Bool

        /// Total attachment count (clipboard + window)
        var totalAttachmentCount: Int {
            clipboardAttachmentCount + windowAttachmentCount
        }
    }

    // MARK: - Public API

    /// Merge attachments and text following the data order rule
    ///
    /// Data order: Window Text + Clipboard Text Context + Clipboard Attachments + Window Attachments
    ///
    /// Example scenarios:
    /// | Window | Clipboard | Final Result |
    /// |--------|-----------|--------------|
    /// | "Summarize:" + ImageW | ImageC | "Summarize:" + ImageC + ImageW |
    /// | "Summarize:" + ImageW | "Context text" | "Summarize:" + "Context text" + ImageW |
    /// | "Translate this" | ImageC | "Translate this" + ImageC |
    /// | "Hello" | "World" | "Hello" + "World" |
    ///
    /// - Parameter context: Merge context with all sources
    /// - Returns: Merged result with final text and attachments in correct order
    static func merge(_ context: MergeContext) -> MergeResult {
        // 1. Combine text: window text + clipboard context (if any)
        let finalText = mergeText(
            windowText: context.windowText,
            clipboardContext: context.clipboardTextContext
        )

        // 2. Combine attachments: clipboard first, then window
        // This order ensures user's intentional context comes before embedded content
        var finalAttachments: [MediaAttachment] = []

        // Add clipboard attachments first (user's intentional context)
        if !context.clipboardAttachments.isEmpty {
            finalAttachments.append(contentsOf: context.clipboardAttachments)
        }

        // Add window attachments second (from active window content)
        if !context.windowAttachments.isEmpty {
            finalAttachments.append(contentsOf: context.windowAttachments)
        }

        return MergeResult(
            finalText: finalText,
            finalAttachments: finalAttachments,
            clipboardAttachmentCount: context.clipboardAttachments.count,
            windowAttachmentCount: context.windowAttachments.count,
            hasClipboardContext: context.clipboardTextContext != nil && !context.clipboardTextContext!.isEmpty
        )
    }

    /// Convenience method to merge only attachments (no text processing)
    ///
    /// - Parameters:
    ///   - clipboardAttachments: Attachments from clipboard
    ///   - windowAttachments: Attachments from window
    /// - Returns: Combined attachments in correct order (clipboard first, then window)
    static func mergeAttachments(
        clipboard: [MediaAttachment],
        window: [MediaAttachment]
    ) -> [MediaAttachment] {
        var result: [MediaAttachment] = []
        result.append(contentsOf: clipboard)
        result.append(contentsOf: window)
        return result
    }

    // MARK: - Private Helpers

    /// Merge window text with clipboard context
    private static func mergeText(windowText: String, clipboardContext: String?) -> String {
        guard let context = clipboardContext,
              !context.isEmpty,
              context != windowText else {
            return windowText
        }

        // Append context to window text with separator
        return "\(windowText)\n\n\(context)"
    }
}

// MARK: - Logging Extension

extension AttachmentMerger.MergeResult {
    /// Log merge statistics
    func logStatistics(prefix: String = "[AttachmentMerger]") {
        if clipboardAttachmentCount > 0 || windowAttachmentCount > 0 {
            print("\(prefix) Merged attachments: \(finalAttachments.count) total")
            if clipboardAttachmentCount > 0 {
                print("\(prefix)   - Clipboard: \(clipboardAttachmentCount)")
            }
            if windowAttachmentCount > 0 {
                print("\(prefix)   - Window: \(windowAttachmentCount)")
            }
        }
        if hasClipboardContext {
            print("\(prefix) Included clipboard text context")
        }
    }
}
