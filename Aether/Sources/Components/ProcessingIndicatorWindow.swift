//
//  ProcessingIndicatorWindow.swift
//  Aether
//
//  A floating indicator window for AI processing state.
//  Used in both single-turn and multi-turn modes.
//  Matches HaloWindow's ZenTheme processing style.
//

import SwiftUI
import AppKit

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
        hasShadow = true  // Enable soft window shadow for depth

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

// MARK: - Processing Indicator View (SwiftUI)

/// SwiftUI view for the spinning processing indicator
/// Matches HaloWindow's ZenTheme processing animation style exactly
struct ProcessingIndicatorView: View {
    @State private var breathingScale: CGFloat = 1.0
    @State private var rotation: Double = 0

    private let indicatorColor = Color.purple

    var body: some View {
        ZStack {
            // Soft glow effect behind the animation (like ZenSuccessView)
            Circle()
                .fill(indicatorColor.opacity(0.3))
                .frame(width: 90, height: 90)
                .blur(radius: 15)

            // Soft circular gradient background (matches ZenTheme)
            Circle()
                .fill(
                    RadialGradient(
                        colors: [indicatorColor.opacity(0.6), indicatorColor.opacity(0.1), .clear],
                        center: .center,
                        startRadius: 20,
                        endRadius: 60
                    )
                )
                .frame(width: 100, height: 100)

            // Breathing outer circle (matches ZenTheme)
            Circle()
                .stroke(indicatorColor.opacity(0.5), lineWidth: 2)
                .frame(width: 80, height: 80)
                .scaleEffect(breathingScale)

            // Rotating segments (matches ZenTheme - 3 arcs)
            ForEach(0..<3, id: \.self) { i in
                Circle()
                    .trim(from: 0.0, to: 0.15)
                    .stroke(indicatorColor, lineWidth: 3)
                    .frame(width: 60, height: 60)
                    .rotationEffect(.degrees(Double(i) * 120 + rotation))
            }
        }
        .frame(width: ProcessingIndicatorWindow.indicatorSize, height: ProcessingIndicatorWindow.indicatorSize)
        .onAppear {
            startAnimation()
        }
    }

    private func startAnimation() {
        // Breathing animation (matches ZenTheme)
        withAnimation(.easeInOut(duration: 1.5).repeatForever(autoreverses: true)) {
            breathingScale = 1.1
        }
        // Rotation animation (matches ZenTheme)
        withAnimation(.linear(duration: 3).repeatForever(autoreverses: false)) {
            rotation = 360
        }
    }
}

// MARK: - Preview

#Preview {
    ProcessingIndicatorView()
        .frame(width: 120, height: 120)
        .background(Color.black.opacity(0.3))
}
