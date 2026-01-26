//
//  UnifiedConversationView.swift
//  Aether
//
//  Main SwiftUI view for unified conversation window.
//  Displays conversation/commands/topics above input, with attachment preview.
//

import SwiftUI
import UniformTypeIdentifiers

// MARK: - UnifiedConversationView

/// Main view for unified conversation window
struct UnifiedConversationView: View {
    @Bindable var viewModel: UnifiedConversationViewModel

    /// Maximum height for content area (conversation or command list)
    private let maxContentHeight: CGFloat = 600


    var body: some View {
        VStack(spacing: 0) {
            // Spacer pushes content to bottom
            Spacer(minLength: 0)

            // Main content with glass background
            contentWithBackground
        }
        .onDrop(of: [.fileURL], isTargeted: nil) { providers in
            handleDrop(providers: providers)
        }
    }

    // MARK: - Content with Background

    // MARK: - Content with Background
    
    private var contentWithBackground: some View {
        VStack(spacing: 0) {
            // Content area (mutually exclusive)
            contentArea

            // Status bar: appears together with conversation area as a unit
            // Always visible when conversation is shown, prevents height jitter
            if viewModel.shouldShowConversation {
                Divider()
                    .opacity(0.3)
                    .padding(.horizontal, 12)

                infoStreamStatusBar

                // Divider between status bar and input area
                Divider()
                    .opacity(0.3)
                    .padding(.horizontal, 12)
            }

            // Input area (always visible)
            InputAreaView(viewModel: viewModel)
        }
        .frame(width: 800)
        // Apply Liquid Glass effect for floating navigation layer
        // Reference: Apple's Liquid Glass design system (WWDC 2025)
        // Using .clear for maximum transparency (true glass effect)
        // "Liquid Glass is exclusively for the navigation layer that floats above app content"
        .modifier(LiquidGlassWindowModifier())
        .animation(.smooth(duration: 0.25), value: viewModel.displayState)
    }

    // MARK: - Status Bar (Minimal Single-Line Design)

    /// Minimal status bar: single line, ~20px height
    /// Shows what Agent is doing in plain language, not technical details
    private var infoStreamStatusBar: some View {
        HStack(spacing: 6) {
            // Spinner when active
            if viewModel.statusIsLoading {
                ProgressView()
                    .scaleEffect(0.5)
                    .frame(width: 12, height: 12)
            }

            // Single line status text (plain language)
            Text(viewModel.statusText)
                .font(.system(size: 11))
                .foregroundColor(GlassColors.secondaryText)
                .lineLimit(1)
                .truncationMode(.tail)

            Spacer()
        }
        .frame(height: 20)
        .padding(.horizontal, 14)
    }

    // MARK: - Content Area (Mutually Exclusive)

    @ViewBuilder
    private var contentArea: some View {
        switch viewModel.displayState {
        case .empty:
            EmptyView()

        case .conversation:
            ConversationAreaView(
                viewModel: viewModel,
                maxHeight: maxContentHeight
            )

        case .commandList(let prefix):
            if prefix == "//" {
                TopicListView(
                    viewModel: viewModel,
                    maxHeight: maxContentHeight
                )
            } else {
                CommandListView(
                    viewModel: viewModel,
                    maxHeight: maxContentHeight
                )
            }
        }
    }

    // MARK: - Drag & Drop

    private func handleDrop(providers: [NSItemProvider]) -> Bool {
        // Process files sequentially to avoid Sendable issues with NSItemProvider
        Task { @MainActor in
            var urls: [URL] = []
            for provider in providers {
                if provider.hasItemConformingToTypeIdentifier("public.file-url") {
                    if let url = await loadURL(from: provider) {
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

    /// Load URL from an item provider using async/await
    @MainActor
    private func loadURL(from provider: NSItemProvider) async -> URL? {
        await withCheckedContinuation { continuation in
            provider.loadItem(forTypeIdentifier: "public.file-url", options: nil) { item, _ in
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

// MARK: - Liquid Glass Window Modifier

/// Applies Liquid Glass effect to the conversation window
/// Uses native glassEffect with .clear type for maximum transparency
struct LiquidGlassWindowModifier: ViewModifier {
    func body(content: Content) -> some View {
        if #available(macOS 26.0, *) {
            // macOS 26+: Use .clear for high transparency (true glass effect)
            // Add darker dimming layer for text legibility and dark appearance
            content
                .background(
                    RoundedRectangle(cornerRadius: 20, style: .continuous)
                        .fill(Color.black.opacity(0.35))
                )
                .glassEffect(.clear, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
        } else {
            // macOS 15-25: Use underWindowBackground for maximum transparency
            content
                .background(
                    ZStack {
                        VisualEffectBackground(
                            material: .underWindowBackground,
                            blendingMode: .behindWindow
                        )
                        // Black gradient for darker glass appearance
                        LinearGradient(
                            colors: [
                                Color.black.opacity(0.45),
                                Color.black.opacity(0.35)
                            ],
                            startPoint: .topLeading,
                            endPoint: .bottomTrailing
                        )
                    }
                )
                .clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
        }
    }
}
