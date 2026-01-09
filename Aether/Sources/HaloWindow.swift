//
//  HaloWindow.swift
//  Aether
//
//  Transparent, floating NSWindow for Halo overlay.
//  CRITICAL: Must never steal focus from active application.
//

import Cocoa
import SwiftUI
import Combine

class HaloWindow: NSWindow {
    private var haloHostingView: NSHostingView<HaloView>?
    private var haloViewModel: HaloViewModel
    private let themeEngine: ThemeEngine
    private weak var eventHandler: EventHandler?

    // MARK: - Flow Handlers (Phase 2 Refactoring)

    /// Handler for Clarification (Phantom Flow) keyboard events
    private let clarificationHandler = ClarificationFlowHandler()

    /// Handler for Conversation (Multi-turn) keyboard events
    private let conversationHandler = ConversationFlowHandler()

    // MARK: - Managers (via DependencyContainer)

    /// Conversation manager accessed through DependencyContainer (for hide blocking)
    private var conversationManager: any ConversationManagerProtocol {
        DependencyContainer.shared.conversationManager
    }

    /// Public accessor for viewModel (used by AppDelegate for command mode)
    var viewModel: HaloViewModel {
        return haloViewModel
    }

    /// Track when Halo started showing (for minimum display time before errors)
    private(set) var showTime: Date?

    /// Hide sequence counter - used to cancel pending hide completion handlers
    /// when a new show request comes in before hide animation completes
    private var hideSequence: Int = 0

    init(themeEngine: ThemeEngine) {
        self.themeEngine = themeEngine

        // Create HaloViewModel (ObservableObject) and HaloView
        haloViewModel = HaloViewModel()
        let haloView = HaloView(viewModel: haloViewModel, themeEngine: themeEngine)

        // Initialize window with borderless style
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: 120, height: 120),
            styleMask: .borderless,
            backing: .buffered,
            defer: false
        )

        // CRITICAL: Window configuration to prevent focus theft
        self.level = .floating  // Above all apps
        self.collectionBehavior = [
            .canJoinAllSpaces,   // Visible on all desktops
            .stationary,         // Don't move with desktop
            .ignoresCycle        // Don't appear in Cmd+Tab
        ]

        // CRITICAL: Transparency and click-through
        self.backgroundColor = .clear
        self.isOpaque = false
        self.hasShadow = false
        self.ignoresMouseEvents = true  // Click-through

        // CRITICAL: Never steal focus
        self.hidesOnDeactivate = false

        // Set up hosting view for SwiftUI content
        haloHostingView = NSHostingView(rootView: haloView)
        if let contentView = self.contentView, let hostingView = haloHostingView {
            hostingView.frame = contentView.bounds
            hostingView.autoresizingMask = [.width, .height]
            contentView.addSubview(hostingView)
        }

        // Start hidden
        self.alphaValue = 0
        self.orderOut(nil)

        // Setup flow handlers (Phase 2 Refactoring)
        clarificationHandler.delegate = self
        clarificationHandler.activate(window: self)

        conversationHandler.delegate = self
        conversationHandler.activate(window: self)
    }

    deinit {
        // Flow handlers will clean up in their own deinit
        // No need to explicitly call deactivate() here
    }

    // MARK: - Focus Prevention

    /// CRITICAL: Prevent Halo from becoming key window in most cases
    /// Exception: Text-type clarification and conversation input need key window for TextField input
    override var canBecomeKey: Bool {
        // Allow key window only for text-type clarification (TextField needs focus)
        if case .clarification(let request) = haloViewModel.state {
            return request.clarificationType == .text
        }
        // Allow key window for conversation input (TextField needs focus)
        if case .conversationInput = haloViewModel.state {
            return true
        }
        return false
    }

    /// CRITICAL: Prevent Halo from becoming main window
    override var canBecomeMain: Bool {
        return false
    }

    // MARK: - Click to Reactivate

    /// Handle all events to reactivate window when it has lost focus
    ///
    /// When the window is in conversationInput or text clarification mode and loses
    /// focus (e.g., user clicked on another app), clicking on the window should
    /// reactivate it so the TextField can receive keyboard input again.
    ///
    /// We use sendEvent instead of mouseDown because mouseDown may be consumed
    /// by child views (like TextField) before reaching the window.
    override func sendEvent(_ event: NSEvent) {
        // Only handle left mouse down events
        if event.type == .leftMouseDown {
            // Check if window should be activatable
            let shouldActivate: Bool
            switch haloViewModel.state {
            case .conversationInput:
                shouldActivate = true
            case .clarification(let request):
                shouldActivate = request.clarificationType == .text
            default:
                shouldActivate = false
            }

            // If window should be key but isn't, reactivate it
            if shouldActivate && !isKeyWindow {
                NSLog("[HaloWindow] Reactivating window on click (was not key window)")
                NSApp.activate(ignoringOtherApps: true)
                makeKeyAndOrderFront(nil)

                // Refocus TextField after window activation
                // Use async to ensure window is fully activated first
                DispatchQueue.mainAsyncAfter(delay: 0.05, weakRef: self) { slf in
                    slf.refocusTextField()
                }
            }
        }

        super.sendEvent(event)
    }

    /// Find and focus the editable TextField in the window
    ///
    /// This is called when reactivating the window to ensure the input field
    /// regains keyboard focus for continued typing.
    private func refocusTextField() {
        guard let contentView = contentView else { return }

        if let textField = findEditableTextField(in: contentView) {
            let success = makeFirstResponder(textField)
            NSLog("[HaloWindow] TextField refocused: %@", success ? "success" : "failed")
        }
    }

    /// Recursively search for an editable NSTextField in the view hierarchy
    private func findEditableTextField(in view: NSView) -> NSTextField? {
        // Check if this view is an editable text field
        if let textField = view as? NSTextField, textField.isEditable {
            return textField
        }

        // Recursively search subviews
        for subview in view.subviews {
            if let textField = findEditableTextField(in: subview) {
                return textField
            }
        }

        return nil
    }

    // MARK: - Public API

    /// Set event handler reference for error action callbacks
    func setEventHandler(_ handler: EventHandler) {
        self.eventHandler = handler
        haloViewModel.eventHandler = handler
    }

    /// Show conversation input UI (proxy to ConversationFlowHandler)
    ///
    /// - Parameter sessionId: The conversation session ID
    func showConversationInput(sessionId: String) {
        conversationHandler.showConversationInput(sessionId: sessionId)
    }

    func show(at position: NSPoint) {
        // Record show time for minimum display duration before errors
        showTime = Date()

        // CRITICAL: Invalidate any pending hide completion handlers
        hideSequence += 1

        // Find the screen containing the cursor position
        // This properly handles multi-monitor setups
        let targetScreen = NSScreen.screens.first { screen in
            NSPointInRect(position, screen.frame)
        } ?? NSScreen.main ?? NSScreen.screens.first

        guard let screen = targetScreen else {
            print("[HaloWindow] Warning: No screen found, cannot display Halo")
            return
        }

        let screenFrame = screen.frame

        // Get dynamic window size based on current state
        let windowSize = getWindowSize()
        self.setContentSize(windowSize)

        var windowOrigin = position

        // Center window on cursor
        windowOrigin.x -= windowSize.width / 2
        windowOrigin.y -= windowSize.height / 2

        // Clamp to screen bounds (prevents Halo from appearing off-screen)
        windowOrigin.x = max(screenFrame.minX, min(windowOrigin.x, screenFrame.maxX - windowSize.width))
        windowOrigin.y = max(screenFrame.minY, min(windowOrigin.y, screenFrame.maxY - windowSize.height))

        self.setFrameOrigin(windowOrigin)

        // Show window WITHOUT activating (critical for focus preservation)
        self.orderFrontRegardless()  // NOT makeKeyAndOrderFront()

        // Fade in animation
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.2
            self.animator().alphaValue = 1.0
        })
    }

    /// Show window with top-left corner aligned to the given position
    ///
    /// The window appears below and to the right of the caret/mouse,
    /// ensuring the input position is never obscured.
    /// - Parameter position: The caret or mouse position (window's top-left aligns here)
    func showBelow(at position: NSPoint) {
        // Record show time for minimum display duration before errors
        showTime = Date()

        // CRITICAL: Invalidate any pending hide completion handlers
        hideSequence += 1

        // Find the screen containing the cursor position
        let targetScreen = NSScreen.screens.first { screen in
            NSPointInRect(position, screen.frame)
        } ?? NSScreen.main ?? NSScreen.screens.first

        guard let screen = targetScreen else {
            print("[HaloWindow] Warning: No screen found, cannot display Halo")
            return
        }

        let screenFrame = screen.frame

        // Get dynamic window size based on current state
        let windowSize = getWindowSize()
        self.setContentSize(windowSize)

        // Position window with TOP-LEFT corner below the text line
        // This ensures the input line is fully visible above the window
        // NSWindow origin is at BOTTOM-LEFT, so:
        // - x: same as position (window extends to the right)
        // - y: position minus window height minus offset (window extends downward)
        //
        // The position should be the bottom of the caret, but due to coordinate
        // system complexities, we add a generous offset to clear typical text sizes
        let verticalOffset: CGFloat = 32  // Comfortable gap below text line
        var windowOrigin = NSPoint(
            x: position.x,
            y: position.y - windowSize.height - verticalOffset
        )

        // If window would go off the bottom of the screen, show it ABOVE instead
        if windowOrigin.y < screenFrame.minY {
            // Place window above with a small gap
            windowOrigin.y = position.y + verticalOffset
        }

        // If window would go off the right edge, shift left
        if windowOrigin.x + windowSize.width > screenFrame.maxX {
            windowOrigin.x = screenFrame.maxX - windowSize.width
        }

        // Clamp to screen bounds
        windowOrigin.x = max(screenFrame.minX, windowOrigin.x)
        windowOrigin.y = max(screenFrame.minY, windowOrigin.y)

        self.setFrameOrigin(windowOrigin)

        // Show window WITHOUT activating (critical for focus preservation)
        self.orderFrontRegardless()

        // Fade in animation
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.2
            self.animator().alphaValue = 1.0
        })

        NSLog("[HaloWindow] showBelow - size: (%.0f, %.0f), input position: (%.1f, %.1f), window origin: (%.1f, %.1f)",
              windowSize.width, windowSize.height, position.x, position.y, windowOrigin.x, windowOrigin.y)
    }

    func hide() {
        // CRITICAL: Do NOT hide when in conversation input mode
        // The conversation input UI should remain visible until user explicitly ends the conversation
        if case .conversationInput = haloViewModel.state {
            NSLog("[HaloWindow] Hide blocked - conversation input mode active")
            return
        }

        // Reset show time
        showTime = nil

        // Increment hide sequence to invalidate any pending completions
        hideSequence += 1
        let currentSequence = hideSequence

        // Fade out animation
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.3
            self.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            // CRITICAL: Only execute orderOut if no new show request came in
            // This prevents "error only shows once" bug where orderOut was called
            // after a new toast was already shown
            guard let self = self, self.hideSequence == currentSequence else {
                print("[HaloWindow] Hide completion skipped (window was re-shown)")
                return
            }
            self.orderOut(nil)
        })
    }

    /// Force hide window, bypassing conversation mode protection
    /// Use this when explicitly ending a conversation session
    func forceHide() {
        NSLog("[HaloWindow] Force hide called")

        // Reset show time
        showTime = nil

        // Increment hide sequence to invalidate any pending completions
        hideSequence += 1
        let currentSequence = hideSequence

        // Fade out animation
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.3
            self.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            guard let self = self, self.hideSequence == currentSequence else {
                return
            }
            self.orderOut(nil)
        })
    }

    /// Show window at its current position (used after hide to re-show)
    func showAtCurrentPosition() {
        // CRITICAL: Invalidate any pending hide completion handlers
        hideSequence += 1

        // Show window WITHOUT activating (critical for focus preservation)
        self.orderFrontRegardless()

        // Fade in animation
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.2
            self.animator().alphaValue = 1.0
        })
    }

    func updateState(_ state: HaloState) {
        // Skip update if the state is visually identical (prevents flickering)
        if isVisuallyIdentical(current: haloViewModel.state, new: state) {
            return
        }

        // Update via ViewModel (ObservableObject) to propagate changes to SwiftUI
        haloViewModel.state = state

        // Enable/disable mouse events based on state
        // commandMode, error, permissionRequired, toast, clarification, conversationInput, and toolConfirmation states need mouse interaction
        switch state {
        case .commandMode, .error, .permissionRequired, .toast, .clarification, .conversationInput, .toolConfirmation:
            self.ignoresMouseEvents = false
        default:
            self.ignoresMouseEvents = true
        }

        // Dynamically resize window based on new state
        let newSize = getWindowSize()
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.4
            context.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)

            // Update window size with animation
            var newFrame = self.frame
            let widthDiff = newSize.width - newFrame.size.width
            let heightDiff = newSize.height - newFrame.size.height

            // For command mode, keep TOP-LEFT corner fixed (like IDE autocomplete)
            // For other states, keep window centered during resize
            if case .commandMode = state {
                // TOP-LEFT fixed: only adjust y to account for height change
                // NSWindow origin is BOTTOM-LEFT, so when height increases,
                // we need to move origin DOWN to keep top-left fixed
                newFrame.origin.y -= heightDiff
                // x stays the same (left edge fixed)
            } else {
                // Keep window centered during resize
                newFrame.origin.x -= widthDiff / 2
                newFrame.origin.y -= heightDiff / 2
            }
            newFrame.size = newSize

            self.animator().setFrame(newFrame, display: true)
        })
    }

    /// Update typewriter progress (0.0-1.0)
    func updateTypewriterProgress(_ progress: Float) {
        // Update state with new progress value
        haloViewModel.state = .typewriting(progress: progress)
    }

    /// Show Halo at screen center (for initialization feedback, errors, etc.)
    func showCentered() {
        // Record show time for minimum display duration before errors
        showTime = Date()

        // CRITICAL: Invalidate any pending hide completion handlers
        hideSequence += 1

        guard let screen = NSScreen.main ?? NSScreen.screens.first else {
            print("[HaloWindow] Warning: No screen found, cannot display")
            return
        }

        let screenFrame = screen.visibleFrame
        let windowSize = NSSize(width: 120, height: 120)  // Standard Halo size
        self.setContentSize(windowSize)

        // Center on screen
        let windowOrigin = NSPoint(
            x: screenFrame.midX - windowSize.width / 2,
            y: screenFrame.midY - windowSize.height / 2
        )

        self.setFrameOrigin(windowOrigin)

        // Show window WITHOUT activating (critical for focus preservation)
        self.orderFrontRegardless()

        // Fade in animation
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.2
            self.animator().alphaValue = 1.0
        })
    }

    /// Show toast at screen center (unlike regular Halo which shows at cursor)
    func showToastCentered() {
        // CRITICAL: Invalidate any pending hide completion handlers
        hideSequence += 1

        guard let screen = NSScreen.main ?? NSScreen.screens.first else {
            print("[HaloWindow] Warning: No screen found, cannot display toast")
            return
        }

        let screenFrame = screen.visibleFrame
        let windowSize = getWindowSize()
        self.setContentSize(windowSize)

        // Center on screen
        let windowOrigin = NSPoint(
            x: screenFrame.midX - windowSize.width / 2,
            y: screenFrame.midY - windowSize.height / 2
        )

        self.setFrameOrigin(windowOrigin)

        // CRITICAL: Cancel any pending hide animations to prevent conflicts
        // This fixes the "error only shows once" issue where hide()'s completion
        // handler would orderOut() the window after toast was already shown
        self.animator().alphaValue = 1.0  // Stop any ongoing animation
        NSAnimationContext.current.duration = 0  // Immediate

        // Show window WITHOUT activating (critical for focus preservation)
        self.orderFrontRegardless()

        // Fade in animation (start fresh)
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.2
            self.animator().alphaValue = 1.0
        })

        print("[HaloWindow] Toast shown centered at (\(windowOrigin.x), \(windowOrigin.y))")
    }

    /// Show tool confirmation dialog (Phase 6 async confirmation)
    func showToolConfirmation(
        confirmationId: String,
        toolName: String,
        toolDescription: String,
        reason: String,
        confidence: Float,
        onExecute: @escaping () -> Void,
        onCancel: @escaping () -> Void
    ) {
        print("[HaloWindow] Showing tool confirmation: \(toolName)")

        // Update state to show confirmation UI
        haloViewModel.state = .toolConfirmation(
            confirmationId: confirmationId,
            toolName: toolName,
            toolDescription: toolDescription,
            reason: reason,
            confidence: confidence,
            onExecute: onExecute,
            onCancel: onCancel
        )

        // Show centered
        showToastCentered()
    }

    // MARK: - Private Helpers

    private func getWindowSize() -> NSSize {
        switch haloViewModel.state {
        case .commandMode:
            // Fixed height for command mode to prevent window jumping during filtering
            return NSSize(width: 400, height: 320)

        case .processing(_, let text), .success(let text):
            let width: CGFloat = text != nil ? 300 : 120
            let height: CGFloat
            if case .processing = haloViewModel.state {
                height = text != nil ? 200 : 120
            } else {
                height = text != nil ? 150 : 120
            }
            return NSSize(width: width, height: height)

        case .typewriting:
            // Typewriter state with progress bar
            return NSSize(width: 200, height: 120)

        case .error:
            return NSSize(width: 300, height: 180)

        case .toast(_, _, let message, _, _):
            // Dynamic height based on message length
            let lineCount = min(5, max(1, message.count / 50 + 1))
            let height = CGFloat(80 + lineCount * 16)
            return NSSize(width: 400, height: height)

        case .clarification(let request):
            // Dynamic height based on options count or text input
            if let options = request.options {
                let optionCount = options.count
                let height = CGFloat(80 + optionCount * 48)
                return NSSize(width: 320, height: height)
            }
            return NSSize(width: 320, height: 140)  // Height for text input

        case .conversationInput:
            return NSSize(width: 480, height: 118)  // Width 1.5x, adjusted for 18pt font

        case .toolConfirmation:
            // Tool confirmation UI: tool name, description, reason, confidence, two buttons
            return NSSize(width: 380, height: 220)

        default:
            return NSSize(width: 120, height: 120)
        }
    }

    /// Check if two states are visually identical (same animation/icon)
    /// Used to prevent flickering when transitioning between states that look the same
    private func isVisuallyIdentical(current: HaloState, new: HaloState) -> Bool {
        switch (current, new) {
        // listening and processingWithAI both show processing animation
        case (.processing, .processing):
            // Same base state - skip update to prevent flicker
            return true
        case (.processing, .processingWithAI):
            // Both show processing animation
            return true
        case (.processingWithAI, .processing):
            // Both show processing animation
            return true
        case (.processingWithAI, .processingWithAI):
            return true
        default:
            return false
        }
    }
}

// MARK: - HaloViewModel (ObservableObject for SwiftUI state propagation)

class HaloViewModel: ObservableObject {
    @Published var state: HaloState = .idle
    weak var eventHandler: EventHandler?

    /// Command completion manager (add-command-completion-system)
    let commandManager = CommandCompletionManager()

    /// Cancellable for forwarding commandManager changes
    private var commandManagerCancellable: AnyCancellable?

    init() {
        // Forward commandManager's objectWillChange to this ViewModel
        // This ensures HaloView re-renders when displayedCommands changes
        // Use receive(on:) to ensure main thread and debounce to prevent rapid updates
        commandManagerCancellable = commandManager.objectWillChange
            .receive(on: DispatchQueue.main)
            .sink { [weak self] _ in
                self?.objectWillChange.send()
            }
    }
}

// MARK: - KeyboardFlowDelegate (Phase 2 Refactoring)

extension HaloWindow: KeyboardFlowDelegate {
    func flowDidRequestHide() {
        hide()
    }

    func flowDidRequestForceHide() {
        forceHide()
    }

    func flowDidComplete(with result: Any?) {
        // Flow completed successfully - no additional action needed
        NSLog("[HaloWindow] Flow completed with result: %@", String(describing: result))
    }

    func flowDidCancel() {
        // Flow cancelled - no additional action needed
        NSLog("[HaloWindow] Flow cancelled")
    }

    func setIgnoresMouseEvents(_ ignores: Bool) {
        self.ignoresMouseEvents = ignores
    }
}
