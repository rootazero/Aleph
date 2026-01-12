//
//  MultiTurnInputWindow.swift
//  Aether
//
//  Input window for multi-turn conversation mode.
//  Centered on screen, supports text input and // command for topic list.
//

import Cocoa
import SwiftUI

// MARK: - MultiTurnInputWindow

/// Input window for multi-turn conversations
final class MultiTurnInputWindow: NSWindow {

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
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: 600, height: 60),
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
        let origin = NSPoint(
            x: screenFrame.midX - frame.width / 2,
            y: screenFrame.midY + 100  // Slightly above center
        )

        setFrameOrigin(origin)
        alphaValue = 0
        orderFrontRegardless()
        makeKey()

        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.15
            self.animator().alphaValue = 1.0
        }

        // Focus the text field
        viewModel.focusInput()
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
