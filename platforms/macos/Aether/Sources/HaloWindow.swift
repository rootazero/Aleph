//
//  HaloWindow.swift
//  Aether
//
//  Simplified Halo overlay window without theme support.
//  Uses unified visual style with 16x16 purple spinner.
//

import Cocoa
import SwiftUI

// MARK: - HaloWindow

/// Floating overlay window for Halo UI (simplified, no themes)
final class HaloWindow: NSWindow {

    // MARK: - Properties

    /// View model for HaloView state
    let viewModel = HaloViewModel()

    /// Time when window was shown (for minimum display time calculations)
    private(set) var showTime: Date?

    /// Hide sequence counter for animation cancellation
    private var hideSequence: Int = 0

    /// Hosting view for SwiftUI content
    private var hostingView: NSHostingView<HaloView>?

    // MARK: - Initialization

    init() {
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: 300, height: 200),
            styleMask: .borderless,
            backing: .buffered,
            defer: false
        )

        setupWindow()
        setupHostingView()
    }

    // MARK: - Window Setup

    private func setupWindow() {
        // Window appearance
        level = .floating
        backgroundColor = .clear
        isOpaque = false
        hasShadow = false

        // Collection behavior
        collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]

        // Focus handling - never steal focus
        hidesOnDeactivate = false

        // Enable mouse events for interactive states (error, toast)
        ignoresMouseEvents = false
    }

    private func setupHostingView() {
        let haloView = HaloView(viewModel: viewModel)
        hostingView = NSHostingView(rootView: haloView)

        if let hostingView = hostingView {
            hostingView.frame = contentView?.bounds ?? .zero
            hostingView.autoresizingMask = [.width, .height]
            contentView = hostingView
        }
    }

    // MARK: - Focus Prevention

    override var canBecomeKey: Bool {
        // Allow key status for interactive states (error buttons, toast dismiss)
        switch viewModel.state {
        case .error, .toast, .clarification, .toolConfirmation, .planConfirmation, .planProgress:
            return true
        default:
            return false
        }
    }

    override var canBecomeMain: Bool { false }

    // MARK: - State Management

    /// Update the Halo state
    func updateState(_ state: HaloState) {
        Task { @MainActor [weak self] in
            self?.viewModel.state = state
            self?.updateWindowSize()
            self?.updateInteractivity()
        }
    }

    /// Update typewriter progress
    func updateTypewriterProgress(_ progress: Float) {
        Task { @MainActor [weak self] in
            self?.viewModel.state = .typewriting(progress: progress)
        }
    }

    private func updateWindowSize() {
        // Size based on state
        let size: NSSize
        switch viewModel.state {
        case .idle:
            size = NSSize(width: 0, height: 0)
        case .listening, .processing, .processingWithAI, .retrievingMemory, .success:
            size = NSSize(width: 80, height: 60)
        case .typewriting:
            size = NSSize(width: 100, height: 60)
        case .error, .toast, .clarification, .toolConfirmation:
            size = NSSize(width: 320, height: 200)
        case .planConfirmation:
            size = NSSize(width: 360, height: 400)  // Plan confirmation needs more space
        case .planProgress:
            size = NSSize(width: 380, height: 420)  // Plan progress needs most space
        case .conversationInput:
            size = NSSize(width: 0, height: 0)  // Handled by MultiTurnInputWindow
        case .coworkConfirmation:
            size = NSSize(width: 400, height: 450)  // Cowork confirmation with DAG view
        case .coworkProgress:
            size = NSSize(width: 400, height: 480)  // Cowork progress with task list
        case .agentPlan:
            size = NSSize(width: 340, height: 360)  // Agent plan confirmation
        case .agentProgress:
            size = NSSize(width: 340, height: 180)  // Agent progress view
        case .agentConflict:
            size = NSSize(width: 320, height: 200)  // Agent conflict resolution
        }

        setContentSize(size)
    }

    private func updateInteractivity() {
        // Enable mouse events for interactive states
        switch viewModel.state {
        case .error, .toast, .clarification, .toolConfirmation, .planConfirmation, .planProgress,
             .coworkConfirmation, .coworkProgress, .agentPlan, .agentProgress, .agentConflict:
            ignoresMouseEvents = false
        default:
            ignoresMouseEvents = true
        }
    }

    // MARK: - Show Methods

    /// Show at a specific position
    func show(at position: NSPoint) {
        showTime = Date()
        hideSequence += 1

        positionWindow(at: position)
        orderFrontRegardless()

        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.15
            self.animator().alphaValue = 1.0
        }
    }

    /// Show centered on screen
    func showCentered() {
        showTime = Date()
        hideSequence += 1

        guard let screen = NSScreen.main else { return }
        let screenFrame = screen.frame
        let windowSize = frame.size

        let origin = NSPoint(
            x: screenFrame.midX - windowSize.width / 2,
            y: screenFrame.midY - windowSize.height / 2
        )

        setFrameOrigin(origin)
        orderFrontRegardless()

        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.15
            self.animator().alphaValue = 1.0
        }
    }

    /// Show at current tracked position (caret or mouse fallback)
    func showAtCurrentPosition() {
        let position = CaretPositionHelper.getBestPosition()
        show(at: position)
    }

    /// Show toast centered on screen
    func showToastCentered() {
        showCentered()
    }

    /// Show toast notification (convenience method)
    func showToast(type: ToastType, title: String, message: String, autoDismiss: Bool) {
        updateState(.toast(type: type, title: title, message: message, autoDismiss: autoDismiss))
        showToastCentered()
    }

    /// Show below a specific position
    func showBelow(at position: NSPoint) {
        // Show 20 points below the specified position
        let belowPosition = NSPoint(x: position.x, y: position.y - 20)
        show(at: belowPosition)
    }

    // MARK: - Hide Methods

    /// Hide with animation
    func hide() {
        showTime = nil
        hideSequence += 1
        let currentSequence = hideSequence

        viewModel.callbacks.reset()

        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.2
            self.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            guard let self = self, self.hideSequence == currentSequence else { return }
            self.orderOut(nil)
            self.viewModel.state = .idle
        })
    }

    /// Force hide immediately without animation
    func forceHide() {
        showTime = nil
        hideSequence += 1
        viewModel.callbacks.reset()
        alphaValue = 0
        orderOut(nil)
        viewModel.state = .idle
    }

    // MARK: - Tool Confirmation

    /// Show tool confirmation dialog
    func showToolConfirmation(
        confirmationId: String,
        toolName: String,
        toolDescription: String,
        reason: String,
        confidence: Float,
        onExecute: @escaping () -> Void,
        onCancel: @escaping () -> Void
    ) {
        viewModel.callbacks.toolConfirmationOnExecute = onExecute
        viewModel.callbacks.toolConfirmationOnCancel = onCancel

        updateState(.toolConfirmation(
            confirmationId: confirmationId,
            toolName: toolName,
            toolDescription: toolDescription,
            reason: reason,
            confidence: confidence
        ))

        showCentered()
    }

    // MARK: - Positioning

    private func positionWindow(at point: NSPoint) {
        let windowSize = frame.size

        // Find target screen
        let targetScreen = NSScreen.screens.first { screen in
            screen.frame.contains(point)
        } ?? NSScreen.main ?? NSScreen.screens.first

        guard let screen = targetScreen else { return }

        let screenFrame = screen.frame

        // Center window on point
        var origin = NSPoint(
            x: point.x - windowSize.width / 2,
            y: point.y - windowSize.height / 2
        )

        // Clamp to screen bounds
        origin.x = max(screenFrame.minX, min(origin.x, screenFrame.maxX - windowSize.width))
        origin.y = max(screenFrame.minY, min(origin.y, screenFrame.maxY - windowSize.height))

        setFrameOrigin(origin)
    }
}
