//
//  MultiTurnInputWindow.swift
//  Aether
//
//  Input window for multi-turn conversation mode.
//  Centered on screen, supports text input and // command for topic list.
//
//  ⚠️ DEPRECATED: This file is deprecated and will be removed in a future version.
//  Use UnifiedConversationWindow instead, which combines input and display functionality.
//

import Cocoa
import SwiftUI

// MARK: - MultiTurnInputWindow

/// Input window for multi-turn conversations
final class MultiTurnInputWindow: NSWindow {

    // MARK: - Constants

    /// Window width
    private static let windowWidth: CGFloat = 600

    /// Base input height
    private static let baseInputHeight: CGFloat = 60

    /// Maximum height for command/topic list
    private static let maxPanelHeight: CGFloat = 300

    /// Total fixed window height to accommodate expanded lists
    private static let fixedWindowHeight: CGFloat = baseInputHeight + maxPanelHeight + 20

    // MARK: - Properties

    /// View model for input state
    let viewModel = MultiTurnInputViewModel()

    /// Hosting view for SwiftUI content
    private var hostingView: NSHostingView<MultiTurnInputView>?

    /// Callbacks
    var onSubmit: ((String) -> Void)?
    var onCancel: (() -> Void)?
    var onTopicSelected: ((Topic) -> Void)?

    /// Local event monitor for ESC key
    private var escapeMonitor: Any?

    // MARK: - Initialization

    init() {
        // Use fixed window height to accommodate command/topic list expansion
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: Self.windowWidth, height: Self.fixedWindowHeight),
            styleMask: [.borderless, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )

        setupWindow()
        setupHostingView()
        setupCallbacks()
        setupEscapeHandler()
    }

    deinit {
        // Remove escape monitor
        if let monitor = escapeMonitor {
            NSEvent.removeMonitor(monitor)
        }
    }

    // MARK: - Window Setup

    private func setupWindow() {
        level = .floating
        backgroundColor = .clear
        isOpaque = false
        hasShadow = true

        // CRITICAL: Start hidden to prevent flash during lazy initialization
        alphaValue = 0

        collectionBehavior = [.canJoinAllSpaces, .stationary]
        hidesOnDeactivate = false
        isMovableByWindowBackground = true  // Enable dragging

        titlebarAppearsTransparent = true
        titleVisibility = .hidden
    }

    private func setupHostingView() {
        let inputView = MultiTurnInputView(viewModel: viewModel)
        hostingView = NSHostingView(rootView: inputView)

        if let hostingView = hostingView {
            hostingView.frame = contentView?.bounds ?? .zero
            hostingView.autoresizingMask = [.width, .height]

            // Critical: Ensure NSHostingView is fully transparent for glassEffect
            hostingView.wantsLayer = true
            hostingView.layer?.backgroundColor = .clear

            contentView = hostingView
        }
    }

    private func setupCallbacks() {
        viewModel.onSubmit = { [weak self] text in
            self?.onSubmit?(text)
        }
        viewModel.onCancel = { [weak self] in
            self?.onCancel?()
        }
        viewModel.onTopicSelected = { [weak self] topic in
            self?.onTopicSelected?(topic)
        }
    }

    private func setupEscapeHandler() {
        // Monitor ESC key to cancel
        escapeMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            if event.keyCode == 53 && self?.isVisible == true {  // ESC key
                self?.viewModel.cancel()
                return nil  // Consume the event
            }
            return event
        }
    }

    // MARK: - Show/Hide

    func showCentered() {
        guard let screen = NSScreen.main else { return }

        let screenFrame = screen.frame

        // Position window so input area is at upper portion of screen
        // This allows command/topic list to expand downward
        let inputAreaTopY = screenFrame.midY + 150
        let windowOriginY = inputAreaTopY - Self.fixedWindowHeight

        let origin = NSPoint(
            x: screenFrame.midX - frame.width / 2,
            y: windowOriginY
        )

        setFrameOrigin(origin)
        alphaValue = 0

        // Activate the app first to ensure we can receive keyboard focus
        // This is critical when the app is in background
        NSApp.activate(ignoringOtherApps: true)

        // Show window
        makeKeyAndOrderFront(nil)

        // Animate fade in
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.15
            self.animator().alphaValue = 1.0
        }, completionHandler: {
            // IMETextField has autoFocus=true, it will handle focus automatically
            // Do NOT call makeFirstResponder on hostingView - it interferes with IMETextField
            NSLog("[MultiTurnInputWindow] Window shown, IMETextField will auto-focus")
        })
    }

    func hide() {
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.15
            self.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            self?.orderOut(nil)
            self?.viewModel.reset()
        })
    }

    // MARK: - State

    func updateTurnCount(_ count: Int) {
        viewModel.turnCount = count
    }

    // MARK: - Focus

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}
