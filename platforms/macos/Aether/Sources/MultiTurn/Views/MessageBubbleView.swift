//
//  MessageBubbleView.swift
//  Aether
//
//  Message bubble with support for text and images.
//  Detects image URLs in message content and displays them inline.
//

import SwiftUI
import AppKit
import UniformTypeIdentifiers

// MARK: - MessageBubbleView

/// Individual message bubble with glass effect and image support
struct MessageBubbleView: View {
    let message: ConversationMessage
    let onCopy: () -> Void

    @State private var isHovering = false
    @State private var storedAttachments: [StoredAttachment] = []

    private var isUser: Bool {
        message.role == .user
    }

    var body: some View {
        HStack(alignment: .bottom, spacing: 0) {
            if isUser { Spacer(minLength: 40) }

            VStack(alignment: isUser ? .trailing : .leading, spacing: 8) {
                // Rich message content (text + images from content)
                RichMessageContentView(
                    content: message.content,
                    isUser: isUser
                )

                // Stored attachments (displayed below text for both user and AI)
                if !storedAttachments.isEmpty {
                    AttachmentGridView(attachments: storedAttachments, isUser: isUser)
                }
            }
            .overlay(alignment: isUser ? .leadingLastTextBaseline : .trailingLastTextBaseline) {
                // Copy button floated over bubble (doesn't affect layout)
                if isHovering {
                    copyButton
                        .padding(isUser ? .trailing : .leading, 4)
                        .transition(.opacity)
                }
            }
            // Prevent window dragging in message bubble area to allow text selection
            .background(NonDraggableArea())

            if !isUser { Spacer(minLength: 40) }
        }
        .onHover { hovering in
            withAnimation(.easeInOut(duration: 0.15)) {
                isHovering = hovering
            }
        }
        .onAppear {
            loadStoredAttachments()
        }
    }

    // MARK: - Copy Button

    private var copyButton: some View {
        Button(action: onCopy) {
            Image(systemName: "doc.on.doc")
                .font(.caption2)
                .liquidGlassSecondaryText()
        }
        .buttonStyle(.plain)
    }

    /// Load stored attachments for this message
    private func loadStoredAttachments() {
        // Load from database
        storedAttachments = AttachmentStore.shared.getAttachments(forMessage: message.id)
        if !storedAttachments.isEmpty {
            print("[MessageBubble] Loaded \(storedAttachments.count) attachments for message: \(message.id)")
        }
    }
}

// MARK: - RichMessageContentView

/// Displays message text content only
/// Images/attachments are displayed separately below via AttachmentGridView
struct RichMessageContentView: View {
    let content: String
    let isUser: Bool

    /// Parsed content segments
    private var segments: [ContentSegment] {
        ContentParser.parse(content)
    }

    /// Extract only text content (remove image URLs for clean display)
    private var textOnlyContent: String {
        segments.compactMap { segment -> String? in
            if case .text(let text) = segment {
                return text
            }
            return nil
        }.joined()
    }

    var body: some View {
        // Display only text, images shown in AttachmentGridView below
        if !textOnlyContent.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            Text(textOnlyContent)
                .font(.system(size: 13))
                .liquidGlassText()
                .textSelection(.enabled)
                .padding(10)
                .glassBubble(isUser: isUser)
        }
    }
}

// MARK: - ImageContentView

/// Displays an image from URL with loading state and download button
struct ImageContentView: View {
    let urlString: String

    @State private var isHovering = false
    @State private var downloadState: DownloadState = .idle

    private enum DownloadState: Equatable {
        case idle
        case downloading
        case success
        case failed(String)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            // Image with loading placeholder
            AsyncImage(url: URL(string: urlString)) { phase in
                switch phase {
                case .empty:
                    loadingPlaceholder
                case .success(let image):
                    image
                        .resizable()
                        .aspectRatio(contentMode: .fit)
                        .frame(maxWidth: 400, maxHeight: 400)
                        .clipShape(RoundedRectangle(cornerRadius: 12))
                        .shadow(color: .black.opacity(0.2), radius: 4, x: 0, y: 2)
                case .failure:
                    errorPlaceholder
                @unknown default:
                    loadingPlaceholder
                }
            }
            .frame(maxWidth: 400)

            // Action buttons (on hover)
            if isHovering {
                HStack(spacing: 12) {
                    // Download button
                    Button(action: downloadImage) {
                        HStack(spacing: 4) {
                            downloadIcon
                            Text(downloadButtonText)
                        }
                        .font(.caption)
                        .liquidGlassSecondaryText()
                    }
                    .buttonStyle(.plain)
                    .disabled(downloadState == .downloading)

                    // Copy URL button
                    Button(action: copyURL) {
                        HStack(spacing: 4) {
                            Image(systemName: "link")
                            Text("Copy URL")
                        }
                        .font(.caption)
                        .liquidGlassSecondaryText()
                    }
                    .buttonStyle(.plain)

                    // Open in browser button
                    Button(action: openInBrowser) {
                        HStack(spacing: 4) {
                            Image(systemName: "safari")
                            Text("Open")
                        }
                        .font(.caption)
                        .liquidGlassSecondaryText()
                    }
                    .buttonStyle(.plain)
                }
                .padding(.horizontal, 4)
            }
        }
        .padding(8)
        .background(.ultraThinMaterial.opacity(0.5), in: RoundedRectangle(cornerRadius: 16))
        .onHover { hovering in
            withAnimation(.easeInOut(duration: 0.15)) {
                isHovering = hovering
            }
        }
    }

    // MARK: - Placeholders

    private var loadingPlaceholder: some View {
        VStack(spacing: 8) {
            ProgressView()
                .scaleEffect(0.8)
            Text("Loading image...")
                .font(.caption)
                .liquidGlassSecondaryText()
        }
        .frame(width: 200, height: 150)
        .background(.ultraThinMaterial.opacity(0.3), in: RoundedRectangle(cornerRadius: 12))
    }

    private var errorPlaceholder: some View {
        VStack(spacing: 8) {
            Image(systemName: "photo.badge.exclamationmark")
                .font(.title)
                .liquidGlassSecondaryText()
            Text("Failed to load image")
                .font(.caption)
                .liquidGlassSecondaryText()
        }
        .frame(width: 200, height: 150)
        .background(.ultraThinMaterial.opacity(0.3), in: RoundedRectangle(cornerRadius: 12))
    }

    // MARK: - Download State UI

    private var downloadIcon: some View {
        Group {
            switch downloadState {
            case .idle:
                Image(systemName: "arrow.down.circle")
            case .downloading:
                ProgressView()
                    .scaleEffect(0.6)
                    .frame(width: 12, height: 12)
            case .success:
                Image(systemName: "checkmark.circle.fill")
                    .foregroundColor(.green)
            case .failed:
                Image(systemName: "xmark.circle.fill")
                    .foregroundColor(.red)
            }
        }
    }

    private var downloadButtonText: String {
        switch downloadState {
        case .idle:
            return NSLocalizedString("Download", comment: "")
        case .downloading:
            return NSLocalizedString("Downloading...", comment: "")
        case .success:
            return NSLocalizedString("Saved", comment: "")
        case .failed(let error):
            return error
        }
    }

    // MARK: - Actions

    private func downloadImage() {
        guard let url = URL(string: urlString) else {
            downloadState = .failed("Invalid URL")
            return
        }

        downloadState = .downloading

        // Use NSSavePanel to let user choose location
        let savePanel = NSSavePanel()
        savePanel.allowedContentTypes = [.jpeg, .png, .gif, .webP]
        savePanel.nameFieldStringValue = url.lastPathComponent.isEmpty
            ? "generated_image.jpg"
            : url.lastPathComponent
        savePanel.canCreateDirectories = true

        savePanel.begin { response in
            guard response == .OK, let saveURL = savePanel.url else {
                DispatchQueue.main.async {
                    downloadState = .idle
                }
                return
            }

            // Download the image
            URLSession.shared.downloadTask(with: url) { tempURL, _, error in
                DispatchQueue.main.async {
                    if let error = error {
                        downloadState = .failed("Error")
                        print("[ImageContentView] Download error: \(error.localizedDescription)")
                        return
                    }

                    guard let tempURL = tempURL else {
                        downloadState = .failed("No data")
                        return
                    }

                    do {
                        // Move file to save location
                        if FileManager.default.fileExists(atPath: saveURL.path) {
                            try FileManager.default.removeItem(at: saveURL)
                        }
                        try FileManager.default.moveItem(at: tempURL, to: saveURL)

                        downloadState = .success

                        // Reset after delay
                        DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
                            downloadState = .idle
                        }
                    } catch {
                        downloadState = .failed("Save failed")
                        print("[ImageContentView] Save error: \(error.localizedDescription)")
                    }
                }
            }.resume()
        }
    }

    private func copyURL() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(urlString, forType: .string)
    }

    private func openInBrowser() {
        if let url = URL(string: urlString) {
            NSWorkspace.shared.open(url)
        }
    }
}

// MARK: - ContentParser

/// Parses message content into text and image segments
enum ContentParser {
    /// Parse content string into segments
    static func parse(_ content: String) -> [ContentSegment] {
        var segments: [ContentSegment] = []

        // Regex pattern for image URLs
        // Matches: https://....(jpg|jpeg|png|gif|webp)
        // Also matches markdown image syntax: ![alt](url)
        let patterns = [
            // Direct image URLs
            #"(https?://[^\s<>\[\]]+\.(?:jpg|jpeg|png|gif|webp|JPG|JPEG|PNG|GIF|WEBP)(?:\?[^\s<>\[\]]*)?)"#,
            // Markdown image syntax
            #"!\[[^\]]*\]\((https?://[^\s<>\[\]]+)\)"#
        ]

        let remainingContent = content
        var foundImages: [(range: Range<String.Index>, url: String)] = []

        // Find all image URLs
        for pattern in patterns {
            if let regex = try? NSRegularExpression(pattern: pattern, options: []) {
                let nsRange = NSRange(remainingContent.startIndex..., in: remainingContent)
                let matches = regex.matches(in: remainingContent, options: [], range: nsRange)

                for match in matches {
                    // Get the URL (last capture group)
                    let urlRangeIndex = match.numberOfRanges - 1
                    if let urlRange = Range(match.range(at: urlRangeIndex), in: remainingContent),
                       let fullRange = Range(match.range, in: remainingContent) {
                        let url = String(remainingContent[urlRange])
                        foundImages.append((range: fullRange, url: url))
                    }
                }
            }
        }

        // Sort by position and remove duplicates
        foundImages.sort { $0.range.lowerBound < $1.range.lowerBound }

        if foundImages.isEmpty {
            // No images found, return content as single text segment
            return [.text(content)]
        }

        // Build segments
        var currentIndex = remainingContent.startIndex

        for (range, url) in foundImages {
            // Skip if this range overlaps with previous
            if range.lowerBound < currentIndex {
                continue
            }

            // Add text before this image
            if currentIndex < range.lowerBound {
                let textBefore = String(remainingContent[currentIndex..<range.lowerBound])
                if !textBefore.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                    segments.append(.text(textBefore))
                }
            }

            // Add image segment
            segments.append(.image(url))

            currentIndex = range.upperBound
        }

        // Add remaining text after last image
        if currentIndex < remainingContent.endIndex {
            let textAfter = String(remainingContent[currentIndex...])
            if !textAfter.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                segments.append(.text(textAfter))
            }
        }

        return segments
    }
}

// MARK: - ContentSegment

/// A segment of message content
enum ContentSegment {
    case text(String)
    case image(String)
}
