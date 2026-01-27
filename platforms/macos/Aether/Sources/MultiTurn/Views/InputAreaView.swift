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
    return .white
}

/// Get appropriate placeholder color for glass effect
private var glassPlaceholderNSColor: NSColor {
    return NSColor.white.withAlphaComponent(0.6)
}

/// Get appropriate indicator background for glass effect
private var glassIndicatorBackground: Color {
    return Color.white.opacity(0.15)
}

/// Get appropriate button background for glass effect
private var glassButtonBackground: Color {
    return Color.white.opacity(0.08)
}

/// Get appropriate button hover background for glass effect
private var glassButtonHoverBackground: Color {
    return Color.white.opacity(0.15)
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
            .modifier(LiquidGlassInputModifier(isTargeted: isTargeted, isFocused: isFocused))
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
                        viewModel.addAttachments(urls: urls)
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
        // Blank areas around interactive elements allow window dragging
        // (enabled by isMovableByWindowBackground=true in window setup)
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

/// Button to add attachments via file picker or drag-drop with glass effect
struct AttachmentButton: View {
    let onFilesSelected: ([URL]) -> Void

    @State private var isHovering = false
    @State private var isTargeted = false
    @State private var isPressed = false

    var body: some View {
        Button(action: openFilePicker) {
            Image(systemName: "paperclip")
                .font(.system(size: 14, weight: .medium))
                .liquidGlassIcon()
                .opacity(buttonOpacity)
                .symbolEffect(.bounce, value: isTargeted)
        }
        .buttonStyle(.plain)
        .frame(width: 32, height: 32)
        .background(buttonBackground)
        .overlay(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .stroke(Color.white.opacity(strokeOpacity), lineWidth: 0.5)
        )
        .scaleEffect(isPressed ? 0.92 : (isTargeted ? 1.05 : 1.0))
        .onHover { hovering in
            withAnimation(.easeInOut(duration: 0.15)) {
                isHovering = hovering
            }
        }
        .simultaneousGesture(
            DragGesture(minimumDistance: 0)
                .onChanged { _ in isPressed = true }
                .onEnded { _ in isPressed = false }
        )
        .onDrop(of: [.fileURL], isTargeted: $isTargeted) { providers in
            handleDrop(providers: providers)
        }
        .animation(.spring(response: 0.3, dampingFraction: 0.7), value: isTargeted)
        .animation(.easeInOut(duration: 0.1), value: isPressed)
        .help(NSLocalizedString("multiturn.attachment.add", comment: ""))
    }

    /// Adaptive background with glass effect
    @ViewBuilder
    private var buttonBackground: some View {
        if #available(macOS 26.0, *) {
            // macOS 26+: Use glass effect
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(baseFillColor)
                .glassEffect(.regular, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        } else {
            // Fallback: Semi-transparent background
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(baseFillColor)
        }
    }

    /// Base fill color adapts to state
    private var baseFillColor: Color {
        if isTargeted {
            return Color.white.opacity(0.22)
        } else if isPressed {
            return Color.white.opacity(0.15)
        } else if isHovering {
            return Color.white.opacity(0.12)
        } else {
            return Color.white.opacity(0.08)
        }
    }

    /// Icon opacity adapts to state
    private var buttonOpacity: Double {
        isTargeted || isHovering ? 1.0 : 0.7
    }

    /// Stroke opacity for border
    private var strokeOpacity: Double {
        isTargeted ? 0.35 : (isHovering ? 0.20 : 0.10)
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

// MARK: - Liquid Glass Input Modifier

/// Applies Liquid Glass effect to the input area with dynamic feedback
/// Uses .clear type for high transparency matching the window
struct LiquidGlassInputModifier: ViewModifier {
    let isTargeted: Bool
    let isFocused: Bool

    func body(content: Content) -> some View {
        if #available(macOS 26.0, *) {
            // macOS 26+: Use .clear for high transparency with darker dimming
            content
                .background(
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        // Darker dimming layer: stronger when targeted for visual feedback
                        .fill(isTargeted ? Color.black.opacity(0.40) : Color.black.opacity(0.25))
                )
                .glassEffect(
                    // Use .clear for transparency, add .interactive() when targeted
                    isTargeted ? .clear.interactive() : .clear,
                    in: RoundedRectangle(cornerRadius: 12, style: .continuous)
                )
                // Scale effect for drag-and-drop visual feedback
                .scaleEffect(isTargeted ? 1.02 : 1.0)
                .animation(.snappy, value: isTargeted)
        } else {
            // macOS 15-25: Fallback using VisualEffectBackground
            content
                .background {
                    ZStack {
                        // Base glass layer
                        VisualEffectBackground(
                            material: .underWindowBackground,
                            blendingMode: .withinWindow
                        )

                        // Dark overlay for visual feedback
                        RoundedRectangle(cornerRadius: 12)
                            .fill(
                                isTargeted ? Color.black.opacity(0.40) :
                                    Color.black.opacity(isFocused ? 0.25 : 0.20)
                            )

                        // Dynamic border with white color
                        RoundedRectangle(cornerRadius: 12)
                            .stroke(
                                isTargeted ? Color.white.opacity(0.5) :
                                    (isFocused ? Color.white.opacity(0.3) :
                                        Color.white.opacity(0.15)),
                                lineWidth: isTargeted ? 1.5 : 0.5
                            )
                    }
                    .clipShape(RoundedRectangle(cornerRadius: 12))
                }
                .scaleEffect(isTargeted ? 1.02 : 1.0)
                .animation(.snappy, value: isTargeted)
        }
    }
}
