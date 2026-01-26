//
//  UnifiedConversationWindow.swift
//  Aether
//
//  Unified NSWindow for multi-turn conversation.
//  Replaces separate input and display windows.
//

import Cocoa
import SwiftUI

// MARK: - UnifiedConversationWindow

/// Unified window for multi-turn conversation
final class UnifiedConversationWindow: NSWindow {

    // MARK: - Constants

    private enum Layout {
        static let width: CGFloat = 800
        static let inputAreaHeight: CGFloat = 60
        static let maxContentHeight: CGFloat = 600
    }

    // MARK: - Properties

    /// View model
    let viewModel = UnifiedConversationViewModel()

    /// Hosting view
    private var hostingView: NSHostingView<UnifiedConversationView>?

    /// ESC key monitor
    /// nonisolated(unsafe) for cleanup in deinit
    nonisolated(unsafe) private var escapeMonitor: Any?

    /// Notification observers for progress tracking
    /// nonisolated(unsafe) for cleanup in deinit
    nonisolated(unsafe) private var notificationObservers: [NSObjectProtocol] = []

    /// Callbacks
    var onSubmit: ((String, [PendingAttachment]) -> Void)?
    var onCancel: (() -> Void)?
    var onTopicSelected: ((Topic) -> Void)?

    // MARK: - Initialization

    init() {
        // Start with minimal height (just input area)
        let initialHeight = Layout.inputAreaHeight + 32  // padding

        super.init(
            contentRect: NSRect(x: 0, y: 0, width: Layout.width, height: initialHeight),
            styleMask: [.borderless, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )

        setupWindow()
        setupHostingView()
        setupCallbacks()
        setupEscapeHandler()
        setupNotificationObservers()
    }

    deinit {
        if let monitor = escapeMonitor {
            NSEvent.removeMonitor(monitor)
        }
        // NotificationCenter.removeObserver is thread-safe
        for observer in notificationObservers {
            NotificationCenter.default.removeObserver(observer)
        }
    }

    // MARK: - Window Setup

    private func setupWindow() {
        level = .normal
        backgroundColor = .clear
        isOpaque = false
        hasShadow = true
        alphaValue = 0  // Start hidden

        // Use .moveToActiveSpace to ensure window appears on current desktop
        // instead of remembering last desktop (.stationary would remember)
        collectionBehavior = [.moveToActiveSpace]
        hidesOnDeactivate = false
        isMovableByWindowBackground = true

        titlebarAppearsTransparent = true
        titleVisibility = .hidden
    }

    private func setupHostingView() {
        let view = UnifiedConversationView(viewModel: viewModel)
        hostingView = NSHostingView(rootView: view)

        if let hostingView = hostingView {
            hostingView.frame = contentView?.bounds ?? .zero
            hostingView.autoresizingMask = [.width, .height]

            // Critical: Ensure NSHostingView is fully transparent for glassEffect
            hostingView.wantsLayer = true
            hostingView.layer?.backgroundColor = .clear
            hostingView.layer?.borderWidth = 0
            hostingView.layer?.borderColor = .clear

            contentView = hostingView
        }

        // Ensure window content view has no border
        contentView?.wantsLayer = true
        contentView?.layer?.backgroundColor = .clear
        contentView?.layer?.borderWidth = 0
        contentView?.layer?.borderColor = .clear

        // Height change callback
        viewModel.onHeightChanged = { [weak self] height in
            DispatchQueue.main.async {
                self?.updateWindowHeight(contentHeight: height)
            }
        }
    }

    private func setupCallbacks() {
        viewModel.onSubmit = { [weak self] text, attachments in
            self?.onSubmit?(text, attachments)
        }
        viewModel.onCancel = { [weak self] in
            self?.onCancel?()
        }
        viewModel.onTopicSelected = { [weak self] topic in
            self?.onTopicSelected?(topic)
        }
    }

    private func setupEscapeHandler() {
        escapeMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            if event.keyCode == 53 && self?.isVisible == true {
                self?.viewModel.handleEscape()
                return nil
            }
            return event
        }
    }

    // MARK: - Notification Observers

    private func setupNotificationObservers() {
        // Plan created - set up steps
        let planObserver = NotificationCenter.default.addObserver(
            forName: .agenticPlanCreated,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            // Extract data before MainActor.assumeIsolated to avoid sending Notification
            guard let steps = notification.userInfo?["steps"] as? [String] else { return }
            MainActor.assumeIsolated {
                self?.viewModel.setPlanSteps(steps)
            }
        }
        notificationObservers.append(planObserver)

        // Tool call started
        let startedObserver = NotificationCenter.default.addObserver(
            forName: .agenticToolCallStarted,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            // Extract data before MainActor.assumeIsolated to avoid sending Notification
            guard let toolName = notification.userInfo?["toolName"] as? String else { return }
            MainActor.assumeIsolated {
                self?.viewModel.setToolCallStarted(toolName)
            }
        }
        notificationObservers.append(startedObserver)

        // Tool call completed
        let completedObserver = NotificationCenter.default.addObserver(
            forName: .agenticToolCallCompleted,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.viewModel.setToolCallCompleted()
            }
        }
        notificationObservers.append(completedObserver)

        // Tool call failed
        let failedObserver = NotificationCenter.default.addObserver(
            forName: .agenticToolCallFailed,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.viewModel.setToolCallFailed()
            }
        }
        notificationObservers.append(failedObserver)

        // DAG plan confirmation required - show inline in conversation
        let dagConfirmObserver = NotificationCenter.default.addObserver(
            forName: .dagPlanConfirmationRequired,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            // Extract data from notification BEFORE MainActor block to avoid Sendable issues
            guard let planId = notification.userInfo?["planId"] as? String,
                  let core = notification.userInfo?["core"] as? AetherCore else {
                print("[UnifiedConversationWindow] Invalid DAG confirmation notification data")
                return
            }

            // Extract plan info to create PendingPlanConfirmation
            guard let info = notification.userInfo?["planInfo"] as? EventHandler.PlanConfirmationInfo else {
                print("[UnifiedConversationWindow] Missing planInfo in notification")
                return
            }

            // Convert to Sendable format for MainActor block
            let title = info.title
            let tasks: [(id: String, name: String, riskLevel: String)] = info.tasks

            MainActor.assumeIsolated {
                guard let self = self else { return }

                // Create pending confirmation and set in ViewModel
                let confirmation = PendingPlanConfirmation(
                    planId: planId,
                    title: title,
                    tasks: tasks
                )

                print("[UnifiedConversationWindow] Showing inline plan confirmation: planId=\(planId), tasks=\(tasks.count)")

                // Set the pending confirmation in ViewModel (will be displayed inline in conversation)
                self.viewModel.setPendingPlanConfirmation(confirmation, core: core)
            }
        }
        notificationObservers.append(dagConfirmObserver)

        // User input request - show inline in conversation for user to respond
        let userInputObserver = NotificationCenter.default.addObserver(
            forName: .userInputRequested,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            // Extract data from notification BEFORE MainActor block to avoid Sendable issues
            guard let requestId = notification.userInfo?["requestId"] as? String,
                  let question = notification.userInfo?["question"] as? String,
                  let options = notification.userInfo?["options"] as? [String],
                  let core = notification.userInfo?["core"] as? AetherCore else {
                print("[UnifiedConversationWindow] Invalid user input request notification data")
                return
            }

            MainActor.assumeIsolated {
                guard let self = self else { return }

                print("[UnifiedConversationWindow] Showing inline user input request: requestId=\(requestId), question=\(question)")

                // Set the pending user input request in ViewModel
                self.viewModel.setPendingUserInputRequest(
                    requestId: requestId,
                    question: question,
                    options: options,
                    core: core
                )
            }
        }
        notificationObservers.append(userInputObserver)
    }

    private func removeNotificationObservers() {
        for observer in notificationObservers {
            NotificationCenter.default.removeObserver(observer)
        }
        notificationObservers.removeAll()
    }

    // MARK: - Positioning

    /// Show window centered with input bottom at 70% screen height
    func showPositioned() {
        guard let screen = NSScreen.main else { return }

        let screenFrame = screen.frame

        // Input bottom at 70% from top (30% from bottom)
        let anchorY = screenFrame.height * 0.30

        // Calculate initial window height
        let windowHeight = calculateWindowHeight()

        // Position window
        let origin = NSPoint(
            x: screenFrame.midX - Layout.width / 2,
            y: anchorY  // Window bottom at anchor
        )

        setFrame(NSRect(origin: origin, size: NSSize(width: Layout.width, height: windowHeight)), display: true)
        alphaValue = 0

        // Activate and show
        NSApp.activate(ignoringOtherApps: true)
        makeKeyAndOrderFront(nil)

        // Fade in
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.15
            self.animator().alphaValue = 1.0
        }
    }

    /// Calculate window height based on content
    private func calculateWindowHeight() -> CGFloat {
        var height = Layout.inputAreaHeight + 32  // Base + padding

        // Add content area height based on display state
        switch viewModel.displayState {
        case .empty:
            break  // No additional height
        case .conversation:
            if !viewModel.messages.isEmpty {
                height += min(200, Layout.maxContentHeight)
            }
        case .commandList(let prefix):
            // Command/Topic list should always have minimum height
            let itemHeight: CGFloat = 44  // Approximate height per row
            if prefix == "//" {
                let topicCount = viewModel.filteredTopics.count
                let listHeight = min(CGFloat(max(topicCount, 1)) * itemHeight + 20, Layout.maxContentHeight)
                height += max(listHeight, 120)  // Minimum 120px for empty state
            } else {
                let commandCount = viewModel.commands.count
                let listHeight = min(CGFloat(max(commandCount, 1)) * itemHeight + 20, Layout.maxContentHeight)
                height += max(listHeight, 120)  // Minimum 120px for empty state
            }
        }

        return height
    }

    /// Update window height and keep bottom anchored
    private func updateWindowHeight(contentHeight: CGFloat) {
        guard let screen = NSScreen.main else { return }

        let screenFrame = screen.frame
        let anchorY = screenFrame.height * 0.30

        // Calculate new height
        var newHeight = Layout.inputAreaHeight + 32

        // Add content height (clamped)
        newHeight += min(contentHeight, Layout.maxContentHeight)

        // Update frame keeping bottom at anchor
        let newFrame = NSRect(
            x: frame.origin.x,
            y: anchorY,  // Keep bottom at anchor
            width: Layout.width,
            height: newHeight
        )

        setFrame(newFrame, display: true, animate: true)
    }

    // MARK: - Hide

    func hide() {
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.15
            self.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            // Completion handler runs on main thread
            MainActor.assumeIsolated {
                self?.orderOut(nil)
                self?.viewModel.reset()
            }
        })
    }

    // MARK: - State

    func updateTurnCount(_ count: Int) {
        viewModel.turnCount = count
    }

    // MARK: - Focus

    override var canBecomeKey: Bool { true }
    // Note: Must be true for glassEffect to render in active state
    // Otherwise glass degrades to simple blur
    override var canBecomeMain: Bool { true }
}
