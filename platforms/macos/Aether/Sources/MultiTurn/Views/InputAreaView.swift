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
    
    // Track focus for visual enhancements
    @FocusState private var isFocused: Bool
    // Track drag-and-drop targeting for "Energy Flow" effect
    @State private var isTargeted: Bool = false

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
                .focused($isFocused)
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
            .padding(.vertical, 6)
            .background {
                ZStack {
                    // 1. Visual Effect Background (Glass Material)
                    // Using .selection material for that "popped out" look, similar to detailed design
                    // 1. Visual Effect Background (Glass Material)
                    // Using .selection material with Vibrancy enabled
                    VisualEffectBackground(
                        material: .selection,
                        blendingMode: .withinWindow,
                        state: .active,
                        isEmphasized: true
                    )
                    
                    // 2. Dynamic Inner Shadow / Content Shield
                    // Adds depth and protects text contrast on complex wallpapers
                    RoundedRectangle(cornerRadius: 12)
                        .fill(isTargeted ? Color.cyan.opacity(0.05) : Color.black.opacity(isFocused ? 0.02 : 0.05))
                    
                    // 3. Dynamic Border (Stroke)
                    // Implements the "1% Rule" and "Energy Flow" on drag hover
                    RoundedRectangle(cornerRadius: 12)
                        .stroke(
                            isTargeted ? AnyShapeStyle(Color.cyan.gradient) : 
                                (isFocused ? AnyShapeStyle(.primary.opacity(0.2)) : AnyShapeStyle(LinearGradient(
                                    colors: [.white.opacity(0.35), .clear, .white.opacity(0.1)],
                                    startPoint: .topLeading,
                                    endPoint: .bottomTrailing
                                ))),
                            lineWidth: isTargeted ? 2 : 1
                        )
                }
                .clipShape(RoundedRectangle(cornerRadius: 12))
            }
            .onDrop(of: [.fileURL], isTargeted: $isTargeted) { providers in
                // Forward drop handling to view model
                 Task { @MainActor in
                    var urls: [URL] = []
                    for provider in providers {
                        if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
                             if let data = await loadItemData(from: provider),
                                let url = URL(dataRepresentation: data, relativeTo: nil) {
                                 urls.append(url)
                             }
                        }
                    }
                    if !urls.isEmpty {
                        viewModel.addAttachments(urls)
                    }
                }
                return true
            }
            .scaleEffect(isTargeted ? 1.02 : 1.0)
            .animation(.spring(response: 0.3, dampingFraction: 0.7), value: isFocused)
            .animation(.snappy, value: isTargeted)


            // Attachment button
            AttachmentButton(onFilesSelected: viewModel.addAttachments)

            // Submit button
            Button(action: viewModel.submit) {
                Image(systemName: "arrow.up")
                    .symbolEffect(.bounce, value: isTargeted) // Add bounce effect on target
            }
            .buttonStyle(GlassProminentButtonStyle())
            .disabled(
                viewModel.inputText.trimmingCharacters(in: .whitespaces).isEmpty &&
                viewModel.pendingAttachments.isEmpty
            )
        }
        .padding(16)
    }

    // Helper to load item data safely
    private func loadItemData(from provider: NSItemProvider) async -> Data? {
        await withCheckedContinuation { continuation in
            provider.loadItem(forTypeIdentifier: UTType.fileURL.identifier, options: nil) { item, _ in
                continuation.resume(returning: item as? Data)
            }
        }
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
        // Simplified drop handling here since main input handles it now too, 
        // but keeping it for direct button drops if users prefer exact targeting
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
        // Process files sequentially to avoid Sendable issues with NSItemProvider
        Task { @MainActor in
            var urls: [URL] = []
            for provider in providers {
                if provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier) {
                    if let url = await loadURL(from: provider) {
                        urls.append(url)
                    }
                }
            }
            if !urls.isEmpty {
                onFilesSelected(urls)
            }
        }

        return true
    }

    /// Load URL from an item provider using async/await
    @MainActor
    private func loadURL(from provider: NSItemProvider) async -> URL? {
        await withCheckedContinuation { continuation in
            provider.loadItem(forTypeIdentifier: UTType.fileURL.identifier, options: nil) { item, _ in
                if let data = item as? Data,
                   let url = URL(dataRepresentation: data, relativeTo: nil) {
                    continuation.resume(returning: url)
                } else {
                    continuation.resume(returning: nil)
                }
            }
        }
    }
}
