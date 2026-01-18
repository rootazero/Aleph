//
//  ConversationDisplayWindow.swift
//  Aether
//
//  Floating window for displaying multi-turn conversation history.
//  Positioned at top-right corner, draggable, with fixed width and adaptive height.
//
//  ⚠️ DEPRECATED: This file is deprecated and will be removed in a future version.
//  Use UnifiedConversationWindow instead, which combines input and display functionality.
//

import Cocoa
import SwiftUI

// MARK: - ConversationDisplayWindow

/// Floating window for conversation display
final class ConversationDisplayWindow: NSWindow {

    // MARK: - Constants

    private enum Layout {
        static let width: CGFloat = 360
        static let minHeight: CGFloat = 200
        static let maxHeight: CGFloat = 600
        static let cornerRadius: CGFloat = 12
        static let screenPadding: CGFloat = 20
    }

    // MARK: - Properties

    /// View model for conversation state
    let viewModel = ConversationDisplayViewModel()

    /// Hosting view for SwiftUI content
    private var hostingView: NSHostingView<ConversationDisplayView>?

    // MARK: - Initialization

    init() {
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: Layout.width, height: Layout.minHeight),
            styleMask: [.borderless, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )

        setupWindow()
        setupHostingView()
        positionAtTopRight()
    }

    // MARK: - Window Setup

    private func setupWindow() {
        // Appearance
        level = .floating
        backgroundColor = .clear
        isOpaque = false
        hasShadow = true

        // CRITICAL: Start hidden to prevent flash during lazy initialization
        // The window may be displayed by setFrame(display: true) before show() is called
        alphaValue = 0

        // Behavior
        collectionBehavior = [.canJoinAllSpaces, .stationary]
        hidesOnDeactivate = false
        isMovableByWindowBackground = true

        // Content
        titlebarAppearsTransparent = true
        titleVisibility = .hidden
    }

    private func setupHostingView() {
        let displayView = ConversationDisplayView(viewModel: viewModel)
        hostingView = NSHostingView(rootView: displayView)

        if let hostingView = hostingView {
            hostingView.frame = contentView?.bounds ?? .zero
            hostingView.autoresizingMask = [.width, .height]
            contentView = hostingView
        }

        // Setup height change callback
        viewModel.onHeightChanged = { [weak self] height in
            DispatchQueue.main.async {
                self?.updateHeight(for: height)
            }
        }
    }

    // MARK: - Positioning

    private func positionAtTopRight() {
        guard let screen = NSScreen.main else { return }

        let screenFrame = screen.visibleFrame
        let origin = NSPoint(
            x: screenFrame.maxX - Layout.width - Layout.screenPadding,
            y: screenFrame.maxY - frame.height - Layout.screenPadding
        )

        setFrameOrigin(origin)
    }

    // MARK: - Height Management

    /// Update window height based on content
    func updateHeight(for contentHeight: CGFloat) {
        let clampedHeight = min(max(contentHeight, Layout.minHeight), Layout.maxHeight)

        // Skip if height hasn't changed significantly
        guard abs(clampedHeight - frame.height) > 1 else { return }

        print("[ConversationDisplayWindow] Updating height: content=\(contentHeight), clamped=\(clampedHeight), current=\(frame.height)")

        var newFrame = frame
        let heightDiff = clampedHeight - newFrame.height
        newFrame.size.height = clampedHeight
        newFrame.origin.y -= heightDiff  // Keep top edge fixed

        setFrame(newFrame, display: true, animate: true)
    }

    // MARK: - Show/Hide

    func show() {
        alphaValue = 0
        orderFrontRegardless()

        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.2
            self.animator().alphaValue = 1.0
        }
    }

    func hide() {
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.15
            self.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            self?.orderOut(nil)
        })
    }

    // MARK: - Focus Prevention

    override var canBecomeKey: Bool { true }  // Allow for copy interactions
    override var canBecomeMain: Bool { false }
}
