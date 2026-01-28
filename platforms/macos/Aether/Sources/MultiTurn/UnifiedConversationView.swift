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

            // Status bar: appears when conversation is shown OR when processing
            // This ensures users see status updates even before first message appears
            if viewModel.shouldShowConversation || viewModel.shouldShowStatus {
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

    // MARK: - Status Bar (Multi-Layer Intelligent Progress Display)

    /// Multi-layer status bar: shows plan progress + thinking + tool activity
    /// Height adapts to content (1-3 lines, ~24px per line + padding)
    private var infoStreamStatusBar: some View {
        HStack(alignment: .top, spacing: 8) {
            // Spinner when active
            if viewModel.statusIsLoading {
                ProgressView()
                    .scaleEffect(0.5)
                    .frame(width: 12, height: 12)
                    .padding(.top, 2)
            }

            // Multi-layer status display
            VStack(alignment: .leading, spacing: 3) {
                ForEach(Array(viewModel.statusMessages.enumerated()), id: \.offset) { index, message in
                    Text(message)
                        .font(.system(size: index == 0 ? 12 : 11, weight: index == 0 ? .medium : .regular))
                        .foregroundColor(index == 0 ? GlassColors.secondaryText : GlassColors.secondaryText.opacity(0.8))
                        .lineLimit(1)
                        .transition(.asymmetric(
                            insertion: .opacity.combined(with: .move(edge: .top)),
                            removal: .opacity
                        ))
                }
            }
            .animation(.smooth(duration: 0.25), value: viewModel.statusMessages.count)

            Spacer()
        }
        .frame(height: viewModel.dynamicStatusBarHeight)  // Use dynamic height
        .animation(.smooth(duration: 0.25), value: viewModel.dynamicStatusBarHeight)  // Animate height changes
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .modifier(StatusStreamGlassModifier())
        .padding(.horizontal, 16)  // Outer padding for alignment
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

/// Applies Liquid Glass effect to the conversation window with adaptive background
/// Uses native glassEffect on macOS 26+ with adaptive overlay for text legibility
/// Falls back to NSVisualEffectView on earlier versions
struct LiquidGlassWindowModifier: ViewModifier {

    @StateObject private var backgroundSampler = BackgroundSampler()

    func body(content: Content) -> some View {
        if #available(macOS 26.0, *) {
            // macOS 26+: Use .clear for high transparency (true glass effect)
            // Add adaptive dimming layer for text legibility
            content
                .background(
                    GeometryReader { _ in
                        // Adaptive overlay using WindowAwareAdaptiveOverlay for direct window access
                        WindowAwareAdaptiveOverlay(
                            sampler: backgroundSampler,
                            baseColor: .black
                        )
                    }
                )
                .clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
                .glassEffect(.clear, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
        } else {
            // macOS 15-25: Use underWindowBackground for maximum transparency
            // with adaptive overlay
            content
                .background(
                    ZStack {
                        VisualEffectBackground(
                            material: .underWindowBackground,
                            blendingMode: .behindWindow
                        )

                        // Adaptive overlay that responds to background brightness
                        GeometryReader { _ in
                            WindowAwareAdaptiveOverlay(
                                sampler: backgroundSampler,
                                baseColor: .black
                            )
                        }
                    }
                )
                .clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
        }
    }
}

// MARK: - Status Stream Glass Modifier

/// Applies glass effect to the status stream area with subtle background differentiation
/// Uses lighter overlay than input area to create visual hierarchy
struct StatusStreamGlassModifier: ViewModifier {

    func body(content: Content) -> some View {
        if #available(macOS 26.0, *) {
            // macOS 26+: Use .clear with subtle dimming for visual distinction
            content
                .background(
                    RoundedRectangle(cornerRadius: 10, style: .continuous)
                        // Lighter dimming than input area (0.15 vs 0.25) for subtle differentiation
                        .fill(Color.white.opacity(0.06))
                )
                .glassEffect(
                    .clear,
                    in: RoundedRectangle(cornerRadius: 10, style: .continuous)
                )
        } else {
            // macOS 15-25: Fallback using subtle background
            content
                .background(
                    ZStack {
                        // Base glass layer
                        VisualEffectBackground(
                            material: .underWindowBackground,
                            blendingMode: .withinWindow
                        )

                        // Light overlay for subtle visual feedback
                        RoundedRectangle(cornerRadius: 10)
                            .fill(Color.white.opacity(0.06))

                        // Subtle border with white color
                        RoundedRectangle(cornerRadius: 10)
                            .stroke(Color.white.opacity(0.08), lineWidth: 0.5)
                    }
                    .clipShape(RoundedRectangle(cornerRadius: 10))
                )
        }
    }
}
