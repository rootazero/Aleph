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

    // MARK: - Managers (via DependencyContainer)

    /// Clarification manager accessed through DependencyContainer
    private var clarificationManager: any ClarificationManagerProtocol {
        DependencyContainer.shared.clarificationManager
    }

    /// Conversation manager accessed through DependencyContainer
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

    /// Keyboard event monitor for clarification mode
    private var clarificationKeyMonitor: Any?

    /// Observer for clarification notifications
    private var clarificationObserver: NSObjectProtocol?

    /// Keyboard event monitor for conversation mode (local)
    private var conversationKeyMonitor: Any?

    /// Keyboard event monitor for conversation mode (global fallback for ESC)
    private var conversationGlobalKeyMonitor: Any?

    /// Observer for conversation continuation notifications
    private var conversationObserver: NSObjectProtocol?

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

        // Subscribe to clarification notifications (Phantom Flow)
        setupClarificationObserver()

        // Subscribe to conversation continuation notifications (Multi-turn)
        setupConversationObserver()
    }

    deinit {
        // Cleanup clarification observers
        if let observer = clarificationObserver {
            NotificationCenter.default.removeObserver(observer)
        }
        if let monitor = clarificationKeyMonitor {
            NSEvent.removeMonitor(monitor)
        }
        // Cleanup conversation observers
        if let observer = conversationObserver {
            NotificationCenter.default.removeObserver(observer)
        }
        if let monitor = conversationKeyMonitor {
            NSEvent.removeMonitor(monitor)
        }
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
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { [weak self] in
                    self?.refocusTextField()
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
        // commandMode, error, permissionRequired, toast, clarification, and conversationInput states need mouse interaction
        switch state {
        case .commandMode, .error, .permissionRequired, .toast, .clarification, .conversationInput:
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

// MARK: - Clarification (Phantom Flow) Support

extension HaloWindow {
    /// Setup observer for clarification requests
    private func setupClarificationObserver() {
        clarificationObserver = NotificationCenter.default.addObserver(
            forName: .clarificationRequested,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let request = notification.object as? ClarificationRequest else { return }
            self?.showClarification(request)
        }
    }

    /// Show clarification UI at screen center
    private func showClarification(_ request: ClarificationRequest) {
        NSLog("[HaloWindow] Showing clarification: %@", request.id)

        // Calculate size based on request type
        let width: CGFloat = 320
        let height: CGFloat
        if let options = request.options {
            // Dynamic height based on options count
            height = CGFloat(80 + options.count * 48)
        } else {
            // Fixed height for text input
            height = 140
        }

        // IMPORTANT: Set the window frame BEFORE updating state
        // This prevents updateState() from animating from wrong position
        guard let screen = NSScreen.main ?? NSScreen.screens.first else {
            NSLog("[HaloWindow] Warning: No screen found, cannot display")
            return
        }

        let windowSize = NSSize(width: width, height: height)
        let screenFrame = screen.visibleFrame
        let windowOrigin = NSPoint(
            x: screenFrame.midX - windowSize.width / 2,
            y: screenFrame.midY - windowSize.height / 2
        )

        // Set frame without animation first
        self.setFrame(NSRect(origin: windowOrigin, size: windowSize), display: false)

        // Update state to clarification (this will trigger SwiftUI view update)
        // Skip dynamic resize since we already set the correct frame
        haloViewModel.state = .clarification(request: request)

        // IMPORTANT: Enable mouse events for clarification interaction
        self.ignoresMouseEvents = false

        // Record show time
        showTime = Date()
        hideSequence += 1

        // For text-type clarification, we need to activate window for TextField focus
        // For select-type, use orderFrontRegardless to preserve focus
        if request.clarificationType == .text {
            // Activate window to allow TextField to receive keyboard input
            self.makeKeyAndOrderFront(nil)
            NSLog("[HaloWindow] Text clarification - activating window for TextField focus")
        } else {
            // Show window WITHOUT activating (preserve focus for select-type)
            self.orderFrontRegardless()
        }

        // Fade in animation
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.2
            self.animator().alphaValue = 1.0
        })

        NSLog("[HaloWindow] Clarification window frame: %@", NSStringFromRect(self.frame))

        // Setup keyboard monitor for navigation
        setupClarificationKeyMonitor(for: request)
    }

    /// Show Halo at screen center with specific size
    private func showCenteredWithSize(_ windowSize: NSSize) {
        // Record show time for minimum display duration before errors
        showTime = Date()

        // CRITICAL: Invalidate any pending hide completion handlers
        hideSequence += 1

        guard let screen = NSScreen.main ?? NSScreen.screens.first else {
            NSLog("[HaloWindow] Warning: No screen found, cannot display")
            return
        }

        let screenFrame = screen.visibleFrame
        NSLog("[HaloWindow] Screen frame: %@, windowSize: %@", NSStringFromRect(screenFrame), NSStringFromSize(windowSize))

        self.setContentSize(windowSize)

        // Center on screen
        let windowOrigin = NSPoint(
            x: screenFrame.midX - windowSize.width / 2,
            y: screenFrame.midY - windowSize.height / 2
        )
        NSLog("[HaloWindow] Calculated origin: %@", NSStringFromPoint(windowOrigin))

        self.setFrameOrigin(windowOrigin)
        NSLog("[HaloWindow] Window frame after setFrameOrigin: %@", NSStringFromRect(self.frame))

        // Show window WITHOUT activating (critical for focus preservation)
        self.orderFrontRegardless()

        // Fade in animation
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.2
            self.animator().alphaValue = 1.0
        })
    }

    /// Setup keyboard event monitor for clarification navigation
    private func setupClarificationKeyMonitor(for request: ClarificationRequest) {
        // Remove any existing monitor
        if let monitor = clarificationKeyMonitor {
            NSEvent.removeMonitor(monitor)
            clarificationKeyMonitor = nil
        }

        // For text-type: window is key, use LOCAL monitor
        // For select-type: window is NOT key, use GLOBAL monitor
        if request.clarificationType == .text {
            // Local monitor for key window (text input mode)
            clarificationKeyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
                guard let self = self else { return event }

                // Only handle events when in clarification state
                guard case .clarification = self.haloViewModel.state else {
                    return event
                }

                // Handle keyboard navigation
                if self.handleClarificationKeyEvent(event, request: request) {
                    return nil  // Consume the event
                }
                return event
            }
        } else {
            // Global monitor for non-key window (select mode)
            // This is necessary because Halo window cannot become key (to prevent focus theft)
            clarificationKeyMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
                guard let self = self else { return }

                // Only handle events when in clarification state
                guard case .clarification = self.haloViewModel.state else {
                    return
                }

                // Handle keyboard navigation on main thread
                DispatchQueue.main.async {
                    _ = self.handleClarificationKeyEvent(event, request: request)
                }
            }
        }
    }

    /// Handle keyboard events for clarification navigation
    private func handleClarificationKeyEvent(_ event: NSEvent, request: ClarificationRequest) -> Bool {
        let manager = clarificationManager

        // For text mode, handle Enter and Escape
        if request.clarificationType == .text {
            if event.keyCode == 36 { // Return/Enter - submit text
                let text = manager.textInput
                if !text.isEmpty {
                    removeClarificationKeyMonitor()
                    manager.completeWithText(text)
                    hide()
                    NSLog("[HaloWindow] Text clarification submitted: %@", text)
                }
                return true
            } else if event.keyCode == 53 { // Escape - cancel
                removeClarificationKeyMonitor()
                manager.cancel()
                hide()
                return true
            }
            return false
        }

        // For select mode
        guard let options = request.options, !options.isEmpty else { return false }

        switch event.keyCode {
        case 125: // Down arrow
            let newIndex = min(manager.selectedIndex + 1, options.count - 1)
            manager.selectedIndex = newIndex
            return true

        case 126: // Up arrow
            let newIndex = max(manager.selectedIndex - 1, 0)
            manager.selectedIndex = newIndex
            return true

        case 36: // Return/Enter
            let index = manager.selectedIndex
            if index < options.count {
                removeClarificationKeyMonitor()
                manager.completeWithSelection(index: index, value: options[index].value)
                hide()
            }
            return true

        case 53: // Escape
            removeClarificationKeyMonitor()
            manager.cancel()
            hide()
            return true

        case 18...26: // Number keys 1-9
            let numberIndex = Int(event.keyCode) - 18
            if numberIndex < options.count {
                manager.selectedIndex = numberIndex
                removeClarificationKeyMonitor()
                manager.completeWithSelection(index: numberIndex, value: options[numberIndex].value)
                hide()
            }
            return true

        default:
            return false
        }
    }

    /// Remove keyboard event monitor and restore click-through behavior
    private func removeClarificationKeyMonitor() {
        if let monitor = clarificationKeyMonitor {
            NSEvent.removeMonitor(monitor)
            clarificationKeyMonitor = nil
        }

        // Restore click-through behavior for normal Halo operation
        self.ignoresMouseEvents = true
    }
}

// MARK: - Conversation (Multi-turn) Support

extension HaloWindow {
    /// Setup observer for conversation continuation requests
    private func setupConversationObserver() {
        conversationObserver = NotificationCenter.default.addObserver(
            forName: .conversationContinuationReady,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let sessionId = notification.object as? String else { return }
            self?.showConversationInput(sessionId: sessionId)
        }
    }

    /// Show conversation input UI at screen center
    ///
    /// - Parameter sessionId: The current conversation session ID
    func showConversationInput(sessionId: String) {
        let turnCount = conversationManager.turnCount
        NSLog("[HaloWindow] Showing conversation input: sessionId=%@, turn=%d", sessionId, turnCount)

        // Fixed size for conversation input (header + input + hint + padding)
        let windowSize = NSSize(width: 480, height: 118)

        guard let screen = NSScreen.main ?? NSScreen.screens.first else {
            NSLog("[HaloWindow] Warning: No screen found, cannot display")
            return
        }

        let screenFrame = screen.visibleFrame
        let windowOrigin = NSPoint(
            x: screenFrame.midX - windowSize.width / 2,
            y: screenFrame.midY - windowSize.height / 2
        )

        // Set frame without animation first
        self.setFrame(NSRect(origin: windowOrigin, size: windowSize), display: false)

        // Update state to conversation input
        haloViewModel.state = .conversationInput(sessionId: sessionId, turnCount: turnCount)

        // Enable mouse events for input interaction
        self.ignoresMouseEvents = false

        // Record show time
        showTime = Date()
        hideSequence += 1

        // Fade in animation first (so window is visible)
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.2
            self.animator().alphaValue = 1.0
        })

        // Activate app and window to allow TextField to receive keyboard input
        // Use a short delay to ensure the window is fully visible before activation
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { [weak self] in
            guard let self = self else { return }

            // NSApp.activate is crucial for floating windows to receive key events
            NSApp.activate(ignoringOtherApps: true)
            self.makeKeyAndOrderFront(nil)

            NSLog("[HaloWindow] Conversation input - window frame: %@, isKey: %@, canBecomeKey: %@",
                  NSStringFromRect(self.frame),
                  self.isKeyWindow ? "YES" : "NO",
                  self.canBecomeKey ? "YES" : "NO")

            // Setup keyboard monitor for ESC/Enter handling
            self.setupConversationKeyMonitor()

            // Retry activation if window is not key after short delay
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { [weak self] in
                guard let self = self, !self.isKeyWindow else { return }
                NSLog("[HaloWindow] Retrying window activation...")
                NSApp.activate(ignoringOtherApps: true)
                self.makeKeyAndOrderFront(nil)
            }
        }
    }

    /// Setup keyboard event monitor for conversation input
    private func setupConversationKeyMonitor() {
        // Remove any existing monitor
        removeConversationKeyMonitor()

        // Use local monitor when window is key
        conversationKeyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self = self else { return event }

            // Only handle events when in conversation input state
            guard case .conversationInput = self.haloViewModel.state else {
                return event
            }

            // Handle keyboard navigation
            if self.handleConversationKeyEvent(event) {
                return nil  // Consume the event
            }
            return event
        }

        // Also add global monitor as fallback (for when window fails to become key)
        conversationGlobalKeyMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self = self else { return }

            // Only handle ESC when in conversation input state
            guard case .conversationInput = self.haloViewModel.state else { return }

            // Only handle ESC key globally (other keys need TextField focus)
            if event.keyCode == 53 {  // Escape
                DispatchQueue.main.async { [weak self] in
                    guard let self = self else { return }
                    self.removeConversationKeyMonitor()
                    self.haloViewModel.state = .idle
                    self.conversationManager.cancelConversation()
                    self.forceHide()
                    NSLog("[HaloWindow] Conversation cancelled by user (global monitor)")
                }
            }
        }
    }

    /// Handle keyboard events for conversation input
    private func handleConversationKeyEvent(_ event: NSEvent) -> Bool {
        let manager = conversationManager

        switch event.keyCode {
        case 36:  // Return/Enter - submit input
            let text = manager.textInput.trimmingCharacters(in: .whitespacesAndNewlines)
            if !text.isEmpty {
                removeConversationKeyMonitor()
                // Reset state before hiding to allow forceHide() to complete
                haloViewModel.state = .idle
                manager.submitContinuationInput(text)
                forceHide()
                NSLog("[HaloWindow] Conversation input submitted: %@", text.prefix(50) as CVarArg)
            }
            return true

        case 53:  // Escape - cancel conversation
            removeConversationKeyMonitor()
            // Reset state before hiding to allow forceHide() to complete
            haloViewModel.state = .idle
            manager.cancelConversation()
            forceHide()
            NSLog("[HaloWindow] Conversation cancelled by user")
            return true

        default:
            return false
        }
    }

    /// Remove conversation keyboard event monitor and restore click-through behavior
    private func removeConversationKeyMonitor() {
        if let monitor = conversationKeyMonitor {
            NSEvent.removeMonitor(monitor)
            conversationKeyMonitor = nil
        }

        if let monitor = conversationGlobalKeyMonitor {
            NSEvent.removeMonitor(monitor)
            conversationGlobalKeyMonitor = nil
        }

        // Restore click-through behavior for normal Halo operation
        self.ignoresMouseEvents = true
    }
}
