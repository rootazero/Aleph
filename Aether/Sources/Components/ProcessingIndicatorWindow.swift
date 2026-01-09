//
//  ProcessingIndicatorWindow.swift
//  Aether
//
//  A small floating window that shows a processing indicator at the cursor position.
//  Used during AI thinking in unified input mode.
//

import AppKit
import SwiftUI

// MARK: - Processing Indicator Window

/// A small transparent window that displays a processing indicator at the cursor position
final class ProcessingIndicatorWindow: NSWindow {

    // MARK: - Properties

    /// The hosting view for SwiftUI content
    private var hostingView: NSHostingView<ProcessingIndicatorView>?

    /// Size of the indicator
    private static let indicatorSize: CGFloat = 48

    // MARK: - Initialization

    init() {
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: Self.indicatorSize, height: Self.indicatorSize),
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )

        setupWindow()
        setupContent()
    }

    private func setupWindow() {
        // Transparent, floating window
        self.isOpaque = false
        self.backgroundColor = .clear
        self.level = .floating
        self.hasShadow = false

        // Don't steal focus
        self.ignoresMouseEvents = true
        self.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]

        // Start hidden
        self.alphaValue = 0
        self.orderOut(nil)
    }

    private func setupContent() {
        let indicatorView = ProcessingIndicatorView()
        let hostingView = NSHostingView(rootView: indicatorView)
        hostingView.frame = NSRect(x: 0, y: 0, width: Self.indicatorSize, height: Self.indicatorSize)
        self.contentView = hostingView
        self.hostingView = hostingView
    }

    // MARK: - Public Methods

    /// Show the indicator at the current cursor position
    func showAtCursor() {
        let cursorPosition = NSEvent.mouseLocation

        // Position window centered on cursor
        let origin = NSPoint(
            x: cursorPosition.x - Self.indicatorSize / 2,
            y: cursorPosition.y - Self.indicatorSize / 2
        )
        self.setFrameOrigin(origin)

        // Show with fade-in animation
        self.orderFrontRegardless()
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.2
            self.animator().alphaValue = 1.0
        })

        NSLog("[ProcessingIndicatorWindow] Showing at cursor: (%.0f, %.0f)", cursorPosition.x, cursorPosition.y)
    }

    /// Hide the indicator
    func hideIndicator() {
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.15
            self.animator().alphaValue = 0
        }, completionHandler: {
            self.orderOut(nil)
        })

        NSLog("[ProcessingIndicatorWindow] Hiding")
    }
}

// MARK: - Processing Indicator View

/// SwiftUI view for the processing indicator
private struct ProcessingIndicatorView: View {
    @State private var rotation: Double = 0

    var body: some View {
        ZStack {
            // Background blur circle
            Circle()
                .fill(.ultraThinMaterial)
                .frame(width: 44, height: 44)

            // Spinning arc
            Circle()
                .trim(from: 0, to: 0.7)
                .stroke(
                    Color.accentColor,
                    style: StrokeStyle(lineWidth: 3, lineCap: .round)
                )
                .frame(width: 28, height: 28)
                .rotationEffect(.degrees(rotation))
                .onAppear {
                    withAnimation(.linear(duration: 0.8).repeatForever(autoreverses: false)) {
                        rotation = 360
                    }
                }
        }
        .frame(width: 48, height: 48)
    }
}
