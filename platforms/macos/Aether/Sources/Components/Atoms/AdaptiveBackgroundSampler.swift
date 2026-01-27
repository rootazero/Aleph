//
//  AdaptiveBackgroundSampler.swift
//  Aether
//
//  Adaptive background sampling for Liquid Glass windows.
//  Uses system appearance and simplified detection for adaptive overlay opacity.
//
//  Reference: Liquid Glass design system - adaptive vibrancy
//

import SwiftUI
import AppKit
import Combine

// MARK: - Background Sampler

/// Provides adaptive overlay opacity based on system appearance
/// Simpler approach that doesn't require screen capture permissions
@MainActor
final class BackgroundSampler: ObservableObject {

    // MARK: - Published Properties

    /// Adaptive overlay opacity (0.0-1.0)
    /// Dynamically adjusted based on system appearance
    @Published private(set) var overlayOpacity: Double = 0.40

    /// Current appearance is light mode
    @Published private(set) var isLightMode: Bool = false

    // MARK: - Private Properties

    private var appearanceObserver: NSKeyValueObservation?

    /// Minimum opacity for dark appearance
    private let minOpacity: Double = 0.30

    /// Maximum opacity for light appearance
    private let maxOpacity: Double = 0.50

    // MARK: - Lifecycle

    init() {
        setupAppearanceObserver()
        updateForCurrentAppearance()
    }

    deinit {
        appearanceObserver?.invalidate()
    }

    // MARK: - Public Methods

    /// Start observing for a specific window (for compatibility with previous API)
    func startSampling(for window: NSWindow) {
        // Update based on window's effective appearance
        updateForAppearance(window.effectiveAppearance)
    }

    /// Stop sampling (no-op in this implementation)
    func stopSampling() {
        // No-op: appearance observation continues
    }

    // MARK: - Appearance Detection

    private func setupAppearanceObserver() {
        // Observe system appearance changes
        appearanceObserver = NSApp.observe(\.effectiveAppearance, options: [.new]) { [weak self] _, _ in
            Task { @MainActor [weak self] in
                self?.updateForCurrentAppearance()
            }
        }
    }

    private func updateForCurrentAppearance() {
        guard let appearanceName = NSApp.effectiveAppearance.bestMatch(from: [.darkAqua, .aqua]),
              let appearance = NSAppearance(named: appearanceName) else {
            return
        }

        updateForAppearance(appearance)
    }

    private func updateForAppearance(_ appearance: NSAppearance) {
        let isDark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua

        isLightMode = !isDark

        // Calculate opacity based on appearance
        // Light mode: higher opacity (more darkening) for better contrast
        // Dark mode: lower opacity (less darkening) to show dark UI behind
        let newOpacity = isDark ? minOpacity : maxOpacity

        // Smooth transition
        if abs(newOpacity - overlayOpacity) > 0.01 {
            withAnimation(.easeInOut(duration: 0.3)) {
                overlayOpacity = newOpacity
            }
        }
    }
}

// MARK: - Window-Aware Adaptive Overlay

/// NSView-based adaptive overlay with smooth color transitions
struct WindowAwareAdaptiveOverlay: NSViewRepresentable {

    @ObservedObject var sampler: BackgroundSampler
    let baseColor: NSColor

    init(sampler: BackgroundSampler, baseColor: NSColor = .black) {
        self.sampler = sampler
        self.baseColor = baseColor
    }

    func makeNSView(context: Context) -> AdaptiveOverlayView {
        let view = AdaptiveOverlayView()
        view.baseColor = baseColor
        view.overlayOpacity = sampler.overlayOpacity
        return view
    }

    func updateNSView(_ nsView: AdaptiveOverlayView, context: Context) {
        nsView.overlayOpacity = sampler.overlayOpacity
    }

    final class AdaptiveOverlayView: NSView {
        var baseColor: NSColor = .black {
            didSet {
                needsDisplay = true
            }
        }

        var overlayOpacity: Double = 0.4 {
            didSet {
                if abs(overlayOpacity - oldValue) > 0.01 {
                    needsDisplay = true
                }
            }
        }

        override init(frame frameRect: NSRect) {
            super.init(frame: frameRect)
            wantsLayer = true
        }

        required init?(coder: NSCoder) {
            super.init(coder: coder)
            wantsLayer = true
        }

        override func draw(_ dirtyRect: NSRect) {
            super.draw(dirtyRect)

            // Draw overlay with current opacity
            guard let context = NSGraphicsContext.current?.cgContext else { return }

            context.setFillColor(baseColor.withAlphaComponent(overlayOpacity).cgColor)
            context.fill(dirtyRect)
        }

        override var wantsUpdateLayer: Bool {
            return true
        }

        override func updateLayer() {
            layer?.backgroundColor = baseColor.withAlphaComponent(overlayOpacity).cgColor
        }
    }
}

// MARK: - Adaptive Background Modifier

/// View modifier that applies adaptive background overlay based on system appearance
struct AdaptiveBackgroundOverlay: ViewModifier {

    @StateObject private var sampler = BackgroundSampler()

    /// Base color for overlay (typically black or dark gray)
    let baseColor: Color

    init(baseColor: Color = .black) {
        self.baseColor = baseColor
    }

    func body(content: Content) -> some View {
        content
            .background(
                baseColor.opacity(sampler.overlayOpacity)
            )
    }
}

// MARK: - View Extension

extension View {

    /// Apply adaptive background overlay that responds to system appearance
    /// - Parameter baseColor: Base color for the overlay (default: black)
    func adaptiveBackgroundOverlay(baseColor: Color = .black) -> some View {
        modifier(AdaptiveBackgroundOverlay(baseColor: baseColor))
    }
}
