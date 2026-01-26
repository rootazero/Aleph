//
//  UnifiedConversationView.swift
//  Aether
//
//  Main SwiftUI view for unified conversation window.
//  Displays conversation/commands/topics above input, with attachment preview.
//

import SwiftUI
import UniformTypeIdentifiers
import simd

// MARK: - UnifiedConversationView

/// Main view for unified conversation window
struct UnifiedConversationView: View {
    @Bindable var viewModel: UnifiedConversationViewModel

    /// Maximum height for content area (conversation or command list)
    private let maxContentHeight: CGFloat = 600

    // MARK: - Liquid Glass State
    @StateObject private var colorSampler = WallpaperColorSampler()
    @StateObject private var bubbleCollector = BubbleDataCollector()
    @State private var scrollOffset: CGFloat = 0
    @State private var scrollVelocity: CGFloat = 0
    @State private var mousePosition: CGPoint = .zero
    @State private var inputFocused: Bool = false

    var body: some View {
        ZStack {
            // Metal background layer for Liquid Glass effect
            LiquidGlassMetalView(
                bubbles: $bubbleCollector.bubbles,
                scrollOffset: $scrollOffset,
                mousePosition: $mousePosition,
                hoveredBubbleIndex: $bubbleCollector.hoveredIndex,
                inputFocused: $inputFocused,
                scrollVelocity: $scrollVelocity,
                accentColor: .constant(colorSampler.accentColor),
                dominantColors: .constant(colorSampler.dominantColors)
            )

            // SwiftUI content overlay
            VStack(spacing: 0) {
                // Spacer pushes content to bottom
                Spacer(minLength: 0)

                // Main content with glass background
                contentWithBackground
            }
        }
        .coordinateSpace(name: "liquidGlass")
        .onPreferenceChange(BubbleGeometryPreferenceKey.self) { geometries in
            bubbleCollector.updateGeometries(geometries, viewportSize: CGSize(width: 800, height: 600))
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
        // Metal layer provides the glass background, so we use transparent background here
        .background(Color.clear)
        .clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
        // Layer 3: Specular Highlight (The "1% Rule")
        // A subtle gradient stroke that defines the edge, fading from light to clear
        .overlay(
            RoundedRectangle(cornerRadius: 20, style: .continuous)
                .stroke(
                    LinearGradient(
                        colors: [
                            .white.opacity(0.35), // Highlight top-left
                            .white.opacity(0.1),  // Subtle mid
                            .white.opacity(0.02)  // Fade to almost clear bottom-right
                        ],
                        startPoint: .topLeading,
                        endPoint: .bottomTrailing
                    ),
                    lineWidth: 1
                )
        )
        // Layer 4: Deep Shadow for "Floating" effect
        .shadow(color: .black.opacity(0.2), radius: 15, x: 0, y: 8)
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
