//
//  ProcessingIndicatorWindow.swift
//  Aether
//
//  Minimal processing indicator - 16x16 rotating arc.
//  Replaces the complex HaloWindow system with a simple spinner.
//

import Cocoa
import SwiftUI

/// Minimal floating window that displays a processing spinner
///
/// This replaces the complex HaloWindow/HaloView/ThemeEngine system
/// with a simple 16x16 rotating arc that tracks cursor position.
class ProcessingIndicatorWindow: NSWindow {

    // MARK: - Properties

    private var hostingView: NSHostingView<SpinnerView>?

    /// Track when indicator started showing (for minimum display time)
    private(set) var showTime: Date?

    /// Hide sequence counter for animation cancellation
    private var hideSequence: Int = 0

    // MARK: - Initialization

    init() {
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: 24, height: 24),  // 16px + 4px padding each side
            styleMask: .borderless,
            backing: .buffered,
            defer: false
        )

        // Window configuration - must never steal focus
        level = .floating
        collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]
        backgroundColor = .clear
        isOpaque = false
        hasShadow = false
        ignoresMouseEvents = true  // Click-through
        hidesOnDeactivate = false

        // Setup spinner view
        let spinnerView = SpinnerView()
        hostingView = NSHostingView(rootView: spinnerView)
        if let hostingView = hostingView {
            hostingView.frame = NSRect(x: 0, y: 0, width: 24, height: 24)
            contentView = hostingView
        }

        // Start hidden
        alphaValue = 0
        orderOut(nil)
    }

    // MARK: - Focus Prevention

    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    // MARK: - Public API

    /// Show indicator at the given position (centered on position)
    func show(at position: NSPoint) {
        showTime = Date()
        hideSequence += 1

        // Find the screen containing the position
        let targetScreen = NSScreen.screens.first { screen in
            NSPointInRect(position, screen.frame)
        } ?? NSScreen.main ?? NSScreen.screens.first

        guard let screen = targetScreen else {
            NSLog("[ProcessingIndicator] No screen found")
            return
        }

        let screenFrame = screen.frame
        let windowSize = frame.size

        // Center window on position
        var windowOrigin = NSPoint(
            x: position.x - windowSize.width / 2,
            y: position.y - windowSize.height / 2
        )

        // Clamp to screen bounds
        windowOrigin.x = max(screenFrame.minX, min(windowOrigin.x, screenFrame.maxX - windowSize.width))
        windowOrigin.y = max(screenFrame.minY, min(windowOrigin.y, screenFrame.maxY - windowSize.height))

        setFrameOrigin(windowOrigin)

        // Show without activating
        orderFrontRegardless()

        // Fade in
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.15
            self.animator().alphaValue = 1.0
        }

        NSLog("[ProcessingIndicator] Showing at (%.1f, %.1f)", position.x, position.y)
    }

    /// Hide the indicator with fade out animation
    func hide() {
        showTime = nil
        hideSequence += 1
        let currentSequence = hideSequence

        // Fade out
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.2
            self.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            guard let self = self, self.hideSequence == currentSequence else { return }
            self.orderOut(nil)
        })
    }

    /// Immediately hide without animation
    func hideImmediately() {
        showTime = nil
        hideSequence += 1
        alphaValue = 0
        orderOut(nil)
    }
}

// MARK: - Spinner View

/// Simple 16x16 rotating arc spinner
private struct SpinnerView: View {
    @State private var rotation: Double = 0

    var body: some View {
        Circle()
            .trim(from: 0, to: 0.75)
            .stroke(
                Color.purple,
                style: StrokeStyle(lineWidth: 2, lineCap: .round)
            )
            .frame(width: 16, height: 16)
            .rotationEffect(.degrees(rotation))
            .onAppear {
                withAnimation(.linear(duration: 1).repeatForever(autoreverses: false)) {
                    rotation = 360
                }
            }
    }
}
