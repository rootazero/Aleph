//
//  PerformantTextView.swift
//  Aether
//
//  High-performance text view using NSTextView for streaming updates.
//  Avoids SwiftUI re-rendering overhead by directly manipulating the text storage.
//

import AppKit
import SwiftUI

// MARK: - PerformantTextView

/// High-performance text view that wraps NSTextView for efficient text updates.
/// Uses intrinsic content size to properly communicate height to SwiftUI.
struct PerformantTextView: NSViewRepresentable {
    let text: String
    let isUser: Bool
    var fontSize: CGFloat = 13
    var maxWidth: CGFloat = 700

    class Coordinator: NSObject, NSTextStorageDelegate, @unchecked Sendable {
        var parent: PerformantTextView
        var lastTextLength: Int = 0
        weak var textView: AutoSizingTextView?

        init(_ parent: PerformantTextView) {
            self.parent = parent
        }

        func textStorage(
            _ textStorage: NSTextStorage,
            didProcessEditing editedMask: NSTextStorageEditActions,
            range editedRange: NSRange,
            changeInLength delta: Int
        ) {
            // Trigger size recalculation when text changes
            // Capture textView reference before Task to avoid sending self
            let tv = textView
            Task { @MainActor in
                tv?.invalidateIntrinsicContentSize()
            }
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(self)
    }

    func makeNSView(context: Context) -> AutoSizingTextView {
        let textView = AutoSizingTextView()
        textView.isEditable = false
        textView.isSelectable = true
        textView.drawsBackground = false
        textView.textContainerInset = NSSize(width: 0, height: 0)
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.maxLayoutWidth = maxWidth

        // Configure text container for proper wrapping
        textView.textContainer?.containerSize = NSSize(
            width: maxWidth,
            height: CGFloat.greatestFiniteMagnitude
        )
        textView.textContainer?.widthTracksTextView = false
        textView.textContainer?.lineFragmentPadding = 0

        // Set delegate for size updates
        textView.textStorage?.delegate = context.coordinator
        context.coordinator.textView = textView

        // Set initial text
        updateTextView(textView, with: text, context: context)
        context.coordinator.lastTextLength = text.count

        return textView
    }

    func updateNSView(_ textView: AutoSizingTextView, context: Context) {
        let currentLength = text.count
        let lastLength = context.coordinator.lastTextLength

        // Skip if no change
        if currentLength == lastLength && textView.string == text {
            return
        }

        // Efficient incremental update: append only new characters
        if currentLength > lastLength && text.hasPrefix(textView.string) {
            let newPart = String(text.dropFirst(lastLength))
            let attributes: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: fontSize),
                .foregroundColor: NSColor.labelColor
            ]
            let attributedNew = NSAttributedString(string: newPart, attributes: attributes)
            textView.textStorage?.append(attributedNew)
        } else {
            // Full update for significant changes
            updateTextView(textView, with: text, context: context)
        }

        context.coordinator.lastTextLength = currentLength
    }

    private func updateTextView(_ textView: NSTextView, with text: String, context: Context) {
        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: fontSize),
            .foregroundColor: NSColor.labelColor
        ]
        let attributedString = NSAttributedString(string: text, attributes: attributes)
        textView.textStorage?.setAttributedString(attributedString)
    }
}

// MARK: - AutoSizingTextView

/// NSTextView subclass that reports proper intrinsic content size to SwiftUI
class AutoSizingTextView: NSTextView {
    var maxLayoutWidth: CGFloat = 700

    override var intrinsicContentSize: NSSize {
        guard let layoutManager = layoutManager,
              let textContainer = textContainer else {
            return NSSize(width: NSView.noIntrinsicMetric, height: 20)
        }

        // Ensure layout is complete
        layoutManager.ensureLayout(for: textContainer)

        // Get the used rect
        let usedRect = layoutManager.usedRect(for: textContainer)

        return NSSize(
            width: min(usedRect.width, maxLayoutWidth),
            height: max(usedRect.height, 20)
        )
    }

    override func didChangeText() {
        super.didChangeText()
        invalidateIntrinsicContentSize()
    }
}

// MARK: - StreamingMessageBubble

/// A message bubble optimized for streaming content.
struct StreamingMessageBubble: View {
    let messageId: String
    let content: String
    let isUser: Bool
    let isStreaming: Bool
    let onCopy: () -> Void

    @State private var isHovering = false

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            if isUser { Spacer(minLength: 40) }

            VStack(alignment: isUser ? .trailing : .leading, spacing: 8) {
                streamingContent

                if isHovering && !content.isEmpty {
                    Button(action: onCopy) {
                        HStack(spacing: 2) {
                            Image(systemName: "doc.on.doc")
                            Text("Copy")
                        }
                        .font(.caption2)
                        .liquidGlassSecondaryText()
                    }
                    .buttonStyle(.plain)
                }
            }

            if !isUser { Spacer(minLength: 40) }
        }
        .onHover { hovering in
            withAnimation(.easeInOut(duration: 0.15)) {
                isHovering = hovering
            }
        }
    }

    private var streamingContent: some View {
        PerformantTextView(
            text: content,
            isUser: isUser,
            fontSize: 13,
            maxWidth: 700
        )
        .fixedSize(horizontal: false, vertical: true)
        .frame(maxWidth: 700, alignment: isUser ? .trailing : .leading)
        .padding(12)
        .background(
            RoundedRectangle(cornerRadius: 16)
                .fill(.ultraThinMaterial)
        )
    }
}

// MARK: - Preview

#Preview("Streaming Message") {
    VStack(spacing: 20) {
        StreamingMessageBubble(
            messageId: "1",
            content: "This is a streaming message that updates efficiently. It should grow in height as more text is added. Let's see how it handles multiple lines of content that wrap around.",
            isUser: false,
            isStreaming: true,
            onCopy: {}
        )

        StreamingMessageBubble(
            messageId: "2",
            content: "User message",
            isUser: true,
            isStreaming: false,
            onCopy: {}
        )
    }
    .padding()
    .frame(width: 800, height: 400)
    .background(Color.gray.opacity(0.2))
}
