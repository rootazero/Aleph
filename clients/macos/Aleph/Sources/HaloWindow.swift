//
//  HaloWindow.swift
//  Aleph
//
//  Simplified Halo overlay window without theme support.
//  Uses unified visual style with 16x16 purple spinner.
//

import Cocoa
import SwiftUI

// MARK: - HaloWindow

/// Floating overlay window for Halo UI (V2 simplified state model)
final class HaloWindow: NSWindow {

    // MARK: - Properties

    /// View model for HaloViewV2 state
    let viewModel = HaloViewModelV2()

    /// Time when window was shown (for minimum display time calculations)
    private(set) var showTime: Date?

    /// Hide sequence counter for animation cancellation
    private var hideSequence: Int = 0

    /// Hosting view for SwiftUI content
    private var hostingView: NSHostingView<HaloViewV2>?

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
        let haloView = HaloViewV2(viewModel: viewModel)
        hostingView = NSHostingView(rootView: haloView)

        if let hostingView = hostingView {
            hostingView.frame = contentView?.bounds ?? .zero
            hostingView.autoresizingMask = [.width, .height]
            contentView = hostingView
        }
    }

    // MARK: - Focus Prevention

    override var canBecomeKey: Bool {
        viewModel.state.isInteractive
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

    private func updateWindowSize() {
        let size = viewModel.state.windowSize
        setContentSize(size)
    }

    private func updateInteractivity() {
        ignoresMouseEvents = !viewModel.state.isInteractive
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

    /// Show below a specific position
    func showBelow(at position: NSPoint) {
        // Show 20 points below the specified position
        let belowPosition = NSPoint(x: position.x, y: position.y - 20)
        show(at: belowPosition)
    }

    /// Show streaming state at current position (convenience method)
    func showStreaming(_ context: StreamingContext) {
        updateState(.streaming(context))
        showAtCurrentPosition()
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
            // Completion handler runs on main thread
            MainActor.assumeIsolated {
                guard let self = self, self.hideSequence == currentSequence else { return }
                self.orderOut(nil)
                self.viewModel.state = .idle
            }
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

    // MARK: - Confirmation

    /// Show confirmation dialog (V2)
    func showConfirmation(
        _ context: ConfirmationContext,
        onConfirm: @escaping (String) -> Void,
        onCancel: @escaping () -> Void
    ) {
        viewModel.callbacks.onConfirm = onConfirm
        viewModel.callbacks.onCancel = onCancel
        updateState(.confirmation(context))
        showCentered()
    }

    // MARK: - Error

    /// Show error state (V2)
    func showError(
        _ context: ErrorContext,
        onRetry: (() -> Void)? = nil,
        onDismiss: @escaping () -> Void
    ) {
        viewModel.callbacks.onRetry = onRetry
        viewModel.callbacks.onDismiss = onDismiss
        updateState(.error(context))
        showCentered()
    }

    // MARK: - Result

    /// Show result state (V2)
    func showResult(
        _ context: ResultContext,
        onDismiss: (() -> Void)? = nil,
        onCopy: (() -> Void)? = nil
    ) {
        viewModel.callbacks.onDismiss = onDismiss
        viewModel.callbacks.onCopy = onCopy
        updateState(.result(context))
        showAtCurrentPosition()
    }

    // MARK: - Legacy State Bridge (for EventHandler compatibility)
    // TODO: Migrate EventHandler to use new state model directly

    /// Legacy: Show processing with AI state (maps to streaming.thinking)
    @available(*, deprecated, message: "Use updateState(.streaming) directly")
    func showProcessingWithAI(providerName: String?) {
        let context = StreamingContext(
            runId: UUID().uuidString,
            text: "",
            reasoning: providerName,
            phase: .thinking
        )
        updateState(.streaming(context))
    }

    /// Legacy: Show processing state (maps to streaming.responding)
    @available(*, deprecated, message: "Use updateState(.streaming) directly")
    func showProcessing(streamingText: String?) {
        let context = StreamingContext(
            runId: UUID().uuidString,
            text: streamingText ?? "",
            phase: .responding
        )
        updateState(.streaming(context))
    }

    /// Legacy: Show success state (maps to result.success)
    @available(*, deprecated, message: "Use showResult() directly")
    func showSuccess(message: String?) {
        let summary = ResultSummary.success(
            message: message,
            durationMs: 0,
            finalResponse: message ?? ""
        )
        let context = ResultContext(runId: UUID().uuidString, summary: summary)
        updateState(.result(context))
    }

    /// Legacy: Show agent progress state (maps to streaming.toolExecuting)
    @available(*, deprecated, message: "Use updateState(.streaming) directly")
    func showAgentProgress(
        planId: String,
        progress: Float,
        currentOperation: String,
        completedCount: Int,
        totalCount: Int
    ) {
        let context = StreamingContext(
            runId: planId,
            text: "\(completedCount)/\(totalCount)",
            toolCalls: [ToolCallInfo(
                id: "current",
                name: currentOperation,
                status: .running
            )],
            phase: .toolExecuting
        )
        updateState(.streaming(context))
    }

    /// Legacy: Show plan progress state (maps to streaming.toolExecuting)
    @available(*, deprecated, message: "Use updateState(.streaming) directly")
    func showPlanProgress(progressInfo: PlanProgressInfo) {
        var toolCalls = progressInfo.stepProgress.map { step in
            let status: ToolStatus
            switch step.status {
            case .pending: status = .pending
            case .running: status = .running
            case .completed: status = .completed
            case .failed: status = .failed
            case .skipped: status = .completed
            }
            return ToolCallInfo(
                id: "\(step.index)",
                name: step.toolName.isEmpty ? step.description : step.toolName,
                status: status,
                progressText: step.resultPreview ?? step.errorMessage
            )
        }
        // Limit tool calls displayed
        if toolCalls.count > StreamingContext.maxToolCalls {
            toolCalls = Array(toolCalls.suffix(StreamingContext.maxToolCalls))
        }
        let context = StreamingContext(
            runId: progressInfo.planId,
            text: progressInfo.description,
            toolCalls: toolCalls,
            phase: .toolExecuting
        )
        updateState(.streaming(context))
    }

    /// Legacy: Show toast state (maps to error or result based on type)
    @available(*, deprecated, message: "Use showError() or showResult() directly")
    func showToast(
        type: ToastType,
        title: String,
        message: String,
        autoDismiss: Bool,
        actionTitle: String?
    ) {
        if type == .error {
            let context = ErrorContext(
                type: .unknown,
                message: message
            )
            updateState(.error(context))
        } else {
            let status: ResultStatus = type == .warning ? .partial : .success
            let summary = ResultSummary(
                status: status,
                message: message,
                toolsExecuted: 0,
                tokensUsed: nil,
                durationMs: 0,
                finalResponse: message
            )
            let context = ResultContext(runId: UUID().uuidString, summary: summary)
            updateState(.result(context))
        }
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
