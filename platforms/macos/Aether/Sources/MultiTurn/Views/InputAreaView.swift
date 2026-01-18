//
//  InputAreaView.swift
//  Aether
//
//  Input area component with text field, attachment button, and send button.
//

import SwiftUI
import UniformTypeIdentifiers

// MARK: - InputAreaView

/// Input area with text field, attachment button, and send button
struct InputAreaView: View {
    @Bindable var viewModel: UnifiedConversationViewModel

    var body: some View {
        HStack(spacing: 12) {
            // Turn indicator
            if viewModel.turnCount > 0 {
                Text("Turn \(viewModel.turnCount + 1)")
                    .font(.caption)
                    .foregroundColor(.primary.opacity(0.7))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(.primary.opacity(0.1))
                    .clipShape(RoundedRectangle(cornerRadius: 4))
            }

            // Text field
            IMETextField(
                text: $viewModel.inputText,
                placeholder: NSLocalizedString("multiturn.input.placeholder", comment: ""),
                font: .systemFont(ofSize: 16),
                textColor: .labelColor,
                placeholderColor: NSColor.secondaryLabelColor,
                backgroundColor: .clear,
                autoFocus: true,
                onSubmit: { viewModel.submit() },
                onEscape: { viewModel.handleEscape() },
                onTextChange: { _ in viewModel.refreshDisplayState() },
                onArrowUp: { viewModel.moveSelectionUp() },
                onArrowDown: { viewModel.moveSelectionDown() },
                onTab: { viewModel.handleTab() }
            )
            .frame(height: 24)

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
                .foregroundColor(.primary.opacity(isHovering ? 1.0 : 0.7))
        }
        .buttonStyle(.plain)
        .frame(width: 28, height: 28)
        .background(
            RoundedRectangle(cornerRadius: 6)
                .fill(.primary.opacity(isTargeted ? 0.2 : (isHovering ? 0.1 : 0.05)))
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
