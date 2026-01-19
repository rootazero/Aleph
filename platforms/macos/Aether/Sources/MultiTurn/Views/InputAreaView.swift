//
//  InputAreaView.swift
//  Aether
//
//  Input area component with text field, inline attachments, attachment button, and send button.
//

import SwiftUI
import UniformTypeIdentifiers

// MARK: - Glass Text Color Helper

/// Get appropriate text color for glass effect
/// System automatically applies vibrant treatment for legibility on macOS 26+
private var glassTextNSColor: NSColor {
    return .labelColor
}

/// Get appropriate placeholder color for glass effect
private var glassPlaceholderNSColor: NSColor {
    return .secondaryLabelColor
}

/// Get appropriate indicator background for glass effect
private var glassIndicatorBackground: Color {
    return Color.primary.opacity(0.1)
}

/// Get appropriate button background for glass effect
private var glassButtonBackground: Color {
    return Color.primary.opacity(0.05)
}

/// Get appropriate button hover background for glass effect
private var glassButtonHoverBackground: Color {
    return Color.primary.opacity(0.1)
}


// MARK: - InputAreaView

/// Input area with text field, inline attachments, attachment button, and send button
struct InputAreaView: View {
    @Bindable var viewModel: UnifiedConversationViewModel

    var body: some View {
        HStack(spacing: 12) {
            // Turn indicator
            if viewModel.turnCount > 0 {
                Text("Turn \(viewModel.turnCount + 1)")
                    .font(.caption)
                    .liquidGlassSecondaryText()
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(glassIndicatorBackground)
                    .clipShape(RoundedRectangle(cornerRadius: 4))
            }

            // Input container (text field + inline attachments)
            HStack(spacing: 4) {
                // Text field
                IMETextField(
                    text: $viewModel.inputText,
                    placeholder: NSLocalizedString("multiturn.input.placeholder", comment: ""),
                    font: .systemFont(ofSize: 16),
                    textColor: glassTextNSColor,
                    placeholderColor: glassPlaceholderNSColor,
                    backgroundColor: .clear,
                    autoFocus: true,
                    onSubmit: { viewModel.submit() },
                    onEscape: { viewModel.handleEscape() },
                    onTextChange: { _ in viewModel.refreshDisplayState() },
                    onArrowUp: { viewModel.moveSelectionUp() },
                    onArrowDown: { viewModel.moveSelectionDown() },
                    onTab: { viewModel.handleTab() }
                )
                .frame(maxWidth: .infinity)
                .frame(height: 24)

                // Inline attachments (right side of input)
                if !viewModel.pendingAttachments.isEmpty {
                    InlineAttachmentView(
                        attachments: viewModel.pendingAttachments,
                        onRemove: viewModel.removeAttachment
                    )
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)  // Fixed padding - always same height
            .background(
                RoundedRectangle(cornerRadius: 8)
                    .fill(.primary.opacity(0.05))
            )

            // Attachment button
            AttachmentButton(onFilesSelected: viewModel.addAttachments)

            // Submit button
            Button(action: viewModel.submit) {
                Image(systemName: "arrow.up")
            }
            .buttonStyle(GlassProminentButtonStyle())
            .disabled(
                viewModel.inputText.trimmingCharacters(in: .whitespaces).isEmpty &&
                viewModel.pendingAttachments.isEmpty
            )
        }
        .padding(16)
    }
}

// MARK: - AttachmentButton

/// Button to add attachments via file picker or drag-drop
struct AttachmentButton: View {
    let onFilesSelected: ([URL]) -> Void

    @State private var isHovering = false
    @State private var isTargeted = false

    var body: some View {
        Button(action: openFilePicker) {
            Image(systemName: "plus")
                .font(.system(size: 14, weight: .medium))
                .liquidGlassIcon()
                .opacity(isHovering ? 1.0 : 0.7)
        }
        .buttonStyle(.plain)
        .frame(width: 28, height: 28)
        .background(
            RoundedRectangle(cornerRadius: 6)
                .fill(attachmentButtonBackground)
        )
        .onHover { hovering in
            withAnimation(.easeInOut(duration: 0.1)) {
                isHovering = hovering
            }
        }
        .onDrop(of: [.fileURL], isTargeted: $isTargeted) { providers in
            handleDrop(providers: providers)
        }
        .help(NSLocalizedString("multiturn.attachment.add", comment: ""))
    }

    /// Adaptive background color for glass effect
    /// System handles vibrant colors automatically
    private var attachmentButtonBackground: Color {
        if isTargeted {
            return Color.primary.opacity(0.15)
        } else if isHovering {
            return Color.primary.opacity(0.1)
        } else {
            return Color.primary.opacity(0.05)
        }
    }

    private func openFilePicker() {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = true
        panel.canChooseDirectories = false
        panel.canChooseFiles = true
        panel.allowedContentTypes = [
            .image, .pdf, .plainText, .rtf,
            UTType(filenameExtension: "md") ?? .plainText
        ]

        if panel.runModal() == .OK {
            onFilesSelected(panel.urls)
        }
    }

    private func handleDrop(providers: [NSItemProvider]) -> Bool {
        var urls: [URL] = []
        let group = DispatchGroup()

        for provider in providers {
            if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
                group.enter()
                provider.loadItem(forTypeIdentifier: UTType.fileURL.identifier, options: nil) { item, _ in
                    if let data = item as? Data,
                       let url = URL(dataRepresentation: data, relativeTo: nil) {
                        urls.append(url)
                    }
                    group.leave()
                }
            }
        }

        group.notify(queue: .main) {
            if !urls.isEmpty {
                onFilesSelected(urls)
            }
        }

        return true
    }
}
