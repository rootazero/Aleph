//
//  ProcessingIndicatorWindow.swift
//  Aether
//
//  A floating indicator window for AI processing state.
//  Used in both single-turn and multi-turn modes.
//  Follows the active theme from ThemeEngine.
//

import SwiftUI
import AppKit
import Combine

// MARK: - Positioning Mode

/// Defines how the indicator should determine its fallback position
enum IndicatorPositionMode {
    /// Single-turn mode: cursor → mouse position
    case singleTurn

    /// Multi-turn mode: cursor → unified input window top-left → mouse position
    case multiTurn(windowFrame: NSRect?)
}

// MARK: - Processing Indicator Window

/// A floating, click-through window that displays a processing indicator
/// during AI processing operations.
///
/// Positioning strategy:
/// - Single-turn: Cursor position → Mouse position
/// - Multi-turn: Cursor position → Unified input window top-left → Mouse position
final class ProcessingIndicatorWindow: NSWindow {

    // MARK: - Constants

    /// Window size matches HaloWindow's processing state size
    static let indicatorSize: CGFloat = 120
    private static let windowOffset: CGFloat = 20  // Offset from reference point

    // MARK: - Properties

    private let themeEngine: ThemeEngine
    private var hostingView: NSHostingView<ThemedProcessingIndicatorView>?
    private var themeObserver: AnyCancellable?

    // MARK: - Initialization

    init(themeEngine: ThemeEngine) {
        self.themeEngine = themeEngine

        super.init(
            contentRect: NSRect(x: 0, y: 0, width: Self.indicatorSize, height: Self.indicatorSize),
            styleMask: .borderless,
            backing: .buffered,
            defer: false
        )

        setupWindow()
        setupContent()
        observeThemeChanges()
    }

    // MARK: - Setup

    private func setupWindow() {
        // Window appearance
        level = .floating
        backgroundColor = .clear
        isOpaque = false
        hasShadow = true  // Enable soft window shadow for depth

        // Click-through behavior
        ignoresMouseEvents = true

        // Collection behavior
        collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]
    }

    private func setupContent() {
        let indicatorView = ThemedProcessingIndicatorView(themeEngine: themeEngine)
        let hosting = NSHostingView(rootView: indicatorView)
        hosting.frame = NSRect(x: 0, y: 0, width: Self.indicatorSize, height: Self.indicatorSize)

        contentView = hosting
        hostingView = hosting
    }

    private func observeThemeChanges() {
        // Observe theme changes and update the view
        themeObserver = themeEngine.$selectedTheme
            .receive(on: DispatchQueue.main)
            .sink { [weak self] _ in
                self?.updateContent()
            }
    }

    private func updateContent() {
        // Recreate the hosting view with the new theme
        let indicatorView = ThemedProcessingIndicatorView(themeEngine: themeEngine)
        let hosting = NSHostingView(rootView: indicatorView)
        hosting.frame = NSRect(x: 0, y: 0, width: Self.indicatorSize, height: Self.indicatorSize)

        contentView = hosting
        hostingView = hosting
    }

    // MARK: - Public API

    /// Show the indicator with the specified positioning mode
    ///
    /// - Parameter mode: The positioning mode (single-turn or multi-turn)
    func show(mode: IndicatorPositionMode) {
        let position = calculatePosition(mode: mode)

        // Position the indicator (center on position)
        let origin = NSPoint(
            x: position.x - Self.indicatorSize / 2,
            y: position.y - Self.indicatorSize / 2
        )
        setFrameOrigin(origin)

        // Show with fade-in animation
        alphaValue = 0
        orderFrontRegardless()

        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.2
            context.timingFunction = CAMediaTimingFunction(name: .easeOut)
            self.animator().alphaValue = 1
        }

        let modeString: String
        switch mode {
        case .singleTurn:
            modeString = "single-turn"
        case .multiTurn:
            modeString = "multi-turn"
        }
        NSLog("[ProcessingIndicator] Shown at (%.1f, %.1f), mode: %@", position.x, position.y, modeString)
    }

    /// Hide the indicator with fade-out animation
    func hideIndicator() {
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.15
            context.timingFunction = CAMediaTimingFunction(name: .easeIn)
            self.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            self?.orderOut(nil)
        })

        NSLog("[ProcessingIndicator] Hidden")
    }

    // MARK: - Position Calculation

    /// Calculate the best position for the indicator based on mode
    ///
    /// - Parameter mode: The positioning mode
    /// - Returns: The calculated position
    private func calculatePosition(mode: IndicatorPositionMode) -> NSPoint {
        // Step 1: Try cursor position (common for both modes)
        if let caretPos = CaretPositionHelper.getCaretPosition(),
           isValidPosition(caretPos) {
            NSLog("[ProcessingIndicator] Using cursor position")
            return caretPos
        }

        // Step 2: Mode-specific fallback
        switch mode {
        case .singleTurn:
            // Single-turn: fall back to mouse position
            NSLog("[ProcessingIndicator] Fallback to mouse position (single-turn)")
            return NSEvent.mouseLocation

        case .multiTurn(let windowFrame):
            // Multi-turn: fall back to unified input window's top-left corner
            if let frame = windowFrame {
                let topLeftPosition = NSPoint(
                    x: frame.minX + Self.windowOffset + Self.indicatorSize / 2,
                    y: frame.maxY - Self.windowOffset - Self.indicatorSize / 2
                )
                NSLog("[ProcessingIndicator] Fallback to window top-left (multi-turn)")
                return topLeftPosition
            }
            // If no window frame, fall back to mouse
            NSLog("[ProcessingIndicator] Fallback to mouse position (multi-turn, no window)")
            return NSEvent.mouseLocation
        }
    }

    /// Check if a position is valid (on screen and not at origin)
    private func isValidPosition(_ point: NSPoint) -> Bool {
        // Check if position is not at origin (invalid)
        guard point.x > 0 || point.y > 0 else {
            return false
        }

        // Check if position is on any screen
        for screen in NSScreen.screens {
            if screen.frame.contains(point) {
                return true
            }
        }

        return false
    }
}

// MARK: - Themed Processing Indicator View (SwiftUI)

/// SwiftUI wrapper view that uses the active theme's processingView
struct ThemedProcessingIndicatorView: View {
    @ObservedObject var themeEngine: ThemeEngine

    var body: some View {
        themeEngine.activeTheme.processingView(providerColor: nil, streamingText: nil)
            .frame(width: ProcessingIndicatorWindow.indicatorSize, height: ProcessingIndicatorWindow.indicatorSize)
    }
}

// MARK: - Preview

#Preview {
    ThemedProcessingIndicatorView(themeEngine: ThemeEngine())
        .frame(width: 120, height: 120)
        .background(Color.black.opacity(0.3))
}
