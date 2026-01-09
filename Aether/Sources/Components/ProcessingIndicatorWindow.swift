//
//  ProcessingIndicatorWindow.swift
//  Aether
//
//  A floating indicator window that shows during AI processing.
//  Tracks cursor position with intelligent fallback based on conversation mode.
//

import SwiftUI
import AppKit

// MARK: - Processing Indicator Window

/// A floating, click-through window that displays a processing indicator
/// during AI thinking/processing operations.
final class ProcessingIndicatorWindow: NSWindow {

    // MARK: - Constants

    static let indicatorSize: CGFloat = 48
    private static let padding: CGFloat = 20

    // MARK: - Properties

    private var hostingView: NSHostingView<ProcessingIndicatorView>?

    // MARK: - Initialization

    init() {
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: Self.indicatorSize, height: Self.indicatorSize),
            styleMask: .borderless,
            backing: .buffered,
            defer: false
        )

        setupWindow()
        setupContent()
    }

    // MARK: - Setup

    private func setupWindow() {
        // Window appearance
        level = .floating
        backgroundColor = .clear
        isOpaque = false
        hasShadow = false

        // Click-through behavior
        ignoresMouseEvents = true

        // Collection behavior
        collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]
    }

    private func setupContent() {
        let indicatorView = ProcessingIndicatorView()
        let hosting = NSHostingView(rootView: indicatorView)
        hosting.frame = NSRect(x: 0, y: 0, width: Self.indicatorSize, height: Self.indicatorSize)

        contentView = hosting
        hostingView = hosting
    }

    // MARK: - Public API

    /// Show the indicator at the specified position
    /// - Parameter position: Screen position (bottom-left corner of indicator)
    func show(at position: NSPoint) {
        // Center the indicator on the position
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

        print("[ProcessingIndicator] Shown at position: \(position)")
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

        print("[ProcessingIndicator] Hidden")
    }

    /// Update the indicator position without animation
    /// - Parameter position: New screen position
    func updatePosition(_ position: NSPoint) {
        let origin = NSPoint(
            x: position.x - Self.indicatorSize / 2,
            y: position.y - Self.indicatorSize / 2
        )
        setFrameOrigin(origin)
    }

    // MARK: - Position Helpers

    /// Get indicator position based on mode
    /// - Parameters:
    ///   - mode: The positioning mode (single-turn or multi-turn)
    ///   - windowFrame: Optional window frame for multi-turn fallback
    /// - Returns: The calculated position
    static func getPosition(
        mode: IndicatorPositionMode,
        windowFrame: NSRect? = nil
    ) -> NSPoint {
        // Step 1: Try cursor position via CaretPositionHelper
        if let caretPos = CaretPositionHelper.getCaretPosition(),
           isValidPosition(caretPos) {
            return caretPos
        }

        // Step 2: Fallback based on mode
        switch mode {
        case .singleTurn:
            // Fall back to mouse position
            return NSEvent.mouseLocation

        case .multiTurnWindowVisible:
            // Fall back to window's top-left corner (with padding)
            if let frame = windowFrame {
                return NSPoint(
                    x: frame.minX + padding,
                    y: frame.maxY - padding
                )
            }
            // If no window frame, fall back to mouse
            return NSEvent.mouseLocation

        case .multiTurnWindowHidden:
            // Fall back to mouse position (same as single-turn)
            return NSEvent.mouseLocation
        }
    }

    /// Check if a position is valid (on screen and not at origin)
    private static func isValidPosition(_ point: NSPoint) -> Bool {
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

// MARK: - Indicator Position Mode

/// Defines how the indicator should determine its fallback position
enum IndicatorPositionMode {
    /// Single-turn conversation mode (falls back to mouse position)
    case singleTurn

    /// Multi-turn mode with window visible (falls back to window corner)
    case multiTurnWindowVisible

    /// Multi-turn mode with window hidden (falls back to mouse position)
    case multiTurnWindowHidden
}

// MARK: - Processing Indicator View (SwiftUI)

/// SwiftUI view for the spinning processing indicator
struct ProcessingIndicatorView: View {
    @State private var rotation: Double = 0
    @State private var isAnimating: Bool = false

    var body: some View {
        ZStack {
            // Blur background circle
            Circle()
                .fill(.ultraThinMaterial)
                .frame(width: 44, height: 44)

            // Spinning arc (accent color)
            Circle()
                .trim(from: 0, to: 0.7)
                .stroke(
                    Color.accentColor,
                    style: StrokeStyle(lineWidth: 3, lineCap: .round)
                )
                .frame(width: 28, height: 28)
                .rotationEffect(.degrees(rotation))
        }
        .frame(width: ProcessingIndicatorWindow.indicatorSize, height: ProcessingIndicatorWindow.indicatorSize)
        .onAppear {
            startAnimation()
        }
        .onDisappear {
            stopAnimation()
        }
    }

    private func startAnimation() {
        isAnimating = true
        withAnimation(.linear(duration: 0.8).repeatForever(autoreverses: false)) {
            rotation = 360
        }
    }

    private func stopAnimation() {
        isAnimating = false
    }
}

// MARK: - Preview

#Preview {
    ProcessingIndicatorView()
        .frame(width: 100, height: 100)
        .background(Color.gray.opacity(0.3))
}
