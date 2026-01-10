//
//  UnifiedInputWindow.swift
//  Aether
//
//  An independent floating window for unified multi-turn conversation input.
//  Can coexist with HaloWindow's processing/error/toast states.
//
//  Part of: refactor-unified-halo-window
//

import SwiftUI
import AppKit

// MARK: - UnifiedInputWindow

/// A floating window for unified multi-turn conversation input
///
/// This window is independent from HaloWindow, allowing it to coexist
/// with HaloWindow's processing indicator during AI operations.
///
/// Features:
/// - Contains UnifiedInputView + SubPanel
/// - Supports dynamic height based on SubPanel content
/// - Supports keyboard events (ESC to cancel)
/// - Supports window dragging
/// - Can become key window for text input
final class UnifiedInputWindow: NSWindow {

    // MARK: - Constants

    /// Base width for unified input
    private static let windowWidth: CGFloat = 480

    /// Base height (header + input + hints + padding)
    private static let baseHeight: CGFloat = 100

    /// Fixed window height = base + max SubPanel height
    /// Window never resizes - content expands within this fixed frame
    /// This eliminates window animation jittering completely
    private static let fixedWindowHeight: CGFloat = baseHeight + SubPanelState.maxHeight + 20

    // MARK: - State

    /// SubPanel state for dynamic content
    let subPanelState = SubPanelState()

    /// Current session ID
    private var sessionId: String = ""

    /// Current turn count
    private var turnCount: UInt32 = 0

    /// Hosting view for SwiftUI content
    private var hostingView: NSHostingView<UnifiedInputView>?

    /// Keyboard monitors for ESC handling
    private var keyMonitor: Any?
    private var globalKeyMonitor: Any?

    // MARK: - Callbacks

    /// Called when user submits input
    var onSubmit: ((String) -> Void)?

    /// Called when user cancels (ESC key or explicit cancel)
    var onCancel: (() -> Void)?

    /// Called when user selects a command from SubPanel
    var onCommandSelected: ((CommandNode) -> Void)?

    // MARK: - Initialization

    init() {
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: Self.windowWidth, height: Self.fixedWindowHeight),
            styleMask: .borderless,
            backing: .buffered,
            defer: false
        )

        setupWindow()
        // No need to observe SubPanel changes - window size is fixed
    }

    // MARK: - Window Configuration

    private func setupWindow() {
        // Window appearance
        level = .floating
        backgroundColor = .clear
        isOpaque = false
        hasShadow = false  // SwiftUI view has its own shadow

        // Collection behavior
        collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle]

        // Enable interactions
        ignoresMouseEvents = false
        isMovableByWindowBackground = true

        // Never auto-hide on deactivation
        hidesOnDeactivate = false
    }

    /// Allow becoming key window for text input
    override var canBecomeKey: Bool {
        return true
    }

    /// Never become main window to preserve focus
    override var canBecomeMain: Bool {
        return false
    }

    // MARK: - Public API

    /// Show the unified input window centered on screen
    ///
    /// - Parameters:
    ///   - sessionId: Conversation session identifier
    ///   - turnCount: Current turn count
    ///   - onSubmit: Callback when user submits input
    ///   - onCancel: Callback when user cancels
    ///   - onCommandSelected: Callback when command is selected
    func show(
        sessionId: String,
        turnCount: UInt32,
        onSubmit: @escaping (String) -> Void,
        onCancel: @escaping () -> Void,
        onCommandSelected: @escaping (CommandNode) -> Void
    ) {
        NSLog("[UnifiedInputWindow] show: sessionId=%@, turn=%d", sessionId, turnCount)

        // Store callbacks
        self.sessionId = sessionId
        self.turnCount = turnCount
        self.onSubmit = onSubmit
        self.onCancel = onCancel
        self.onCommandSelected = onCommandSelected

        // Reset SubPanel
        subPanelState.hide()

        // Create/update content view
        setupContentView()

        // Window size is fixed - no need to adjust
        // Center on screen (uses fixed height)
        centerOnScreen()

        // Show with fade-in
        alphaValue = 0
        orderFrontRegardless()

        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.2
            context.timingFunction = CAMediaTimingFunction(name: .easeOut)
            self.animator().alphaValue = 1
        }

        // Activate window for text input after a short delay
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { [weak self] in
            NSApp.activate(ignoringOtherApps: true)
            self?.makeKeyAndOrderFront(nil)
            self?.setupKeyMonitors()
        }
    }

    /// Update session and turn count (for continuing conversations)
    func updateSession(sessionId: String, turnCount: UInt32) {
        self.sessionId = sessionId
        self.turnCount = turnCount
        setupContentView()
    }

    /// Hide the window with fade-out animation
    func hideWindow() {
        NSLog("[UnifiedInputWindow] hide")

        removeKeyMonitors()

        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.15
            context.timingFunction = CAMediaTimingFunction(name: .easeIn)
            self.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            self?.orderOut(nil)
        })
    }

    // MARK: - Content View

    private func setupContentView() {
        let inputView = UnifiedInputView(
            sessionId: sessionId,
            turnCount: turnCount,
            subPanelState: subPanelState,
            onSubmit: { [weak self] text in
                self?.onSubmit?(text)
            },
            onCancel: { [weak self] in
                self?.onCancel?()
            },
            onCommandSelected: { [weak self] command in
                self?.onCommandSelected?(command)
            }
        )

        let hosting = NSHostingView(rootView: inputView)
        contentView = hosting
        hostingView = hosting
    }

    // MARK: - Window Positioning

    private func centerOnScreen() {
        guard let screen = NSScreen.main ?? NSScreen.screens.first else {
            NSLog("[UnifiedInputWindow] Warning: No screen found")
            return
        }

        let screenFrame = screen.visibleFrame
        let windowSize = frame.size

        // Position like Raycast: input area at upper portion of screen
        // so SubPanel can expand downward without going off-screen
        // Place input area at ~60% height (upper 40% of screen)
        let inputAreaTopY = screenFrame.minY + screenFrame.height * 0.6
        let windowOriginY = inputAreaTopY - windowSize.height

        let origin = NSPoint(
            x: screenFrame.midX - windowSize.width / 2,
            y: windowOriginY
        )

        setFrameOrigin(origin)
    }

    // MARK: - Keyboard Handling

    private func setupKeyMonitors() {
        removeKeyMonitors()

        // Local monitor when window is key
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self = self else { return event }

            // ESC key
            if event.keyCode == 53 {
                NSLog("[UnifiedInputWindow] ESC pressed (local)")
                self.onCancel?()
                return nil  // Consume the event
            }
            return event
        }

        // Global monitor as fallback (for ESC when window loses key)
        globalKeyMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self = self else { return }

            // Only handle ESC key globally
            if event.keyCode == 53 {
                NSLog("[UnifiedInputWindow] ESC pressed (global)")
                DispatchQueue.main.async {
                    self.onCancel?()
                }
            }
        }

        NSLog("[UnifiedInputWindow] Key monitors installed")
    }

    private func removeKeyMonitors() {
        if let monitor = keyMonitor {
            NSEvent.removeMonitor(monitor)
            keyMonitor = nil
        }
        if let monitor = globalKeyMonitor {
            NSEvent.removeMonitor(monitor)
            globalKeyMonitor = nil
        }
    }

    // MARK: - Click to Reactivate

    override func sendEvent(_ event: NSEvent) {
        // Reactivate window on click if not key
        if event.type == .leftMouseDown && !isKeyWindow {
            NSLog("[UnifiedInputWindow] Reactivating window on click")
            NSApp.activate(ignoringOtherApps: true)
            makeKeyAndOrderFront(nil)
        }
        super.sendEvent(event)
    }

    // MARK: - Cleanup

    deinit {
        removeKeyMonitors()
        // subPanelSizeCancellable no longer used - window size is fixed
    }
}
