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

    /// Track when Halo started showing (for minimum display time before errors)
    private(set) var showTime: Date?

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
    }

    // MARK: - Focus Prevention

    /// CRITICAL: Prevent Halo from becoming key window
    /// This ensures keyboard events always go to the original app
    override var canBecomeKey: Bool {
        return false
    }

    /// CRITICAL: Prevent Halo from becoming main window
    override var canBecomeMain: Bool {
        return false
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

    func hide() {
        // Reset show time
        showTime = nil

        // Fade out animation
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.3
            self.animator().alphaValue = 0
        }, completionHandler: {
            self.orderOut(nil)
        })
    }

    /// Show window at its current position (used after hide to re-show)
    func showAtCurrentPosition() {
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
        // awaitingInputMode, error, permissionRequired, and toast states need mouse interaction
        switch state {
        case .awaitingInputMode, .error, .permissionRequired, .toast:
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

            // Keep window centered during resize
            newFrame.origin.x -= widthDiff / 2
            newFrame.origin.y -= heightDiff / 2
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

        // Show window WITHOUT activating (critical for focus preservation)
        self.orderFrontRegardless()

        // Fade in animation
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.2
            self.animator().alphaValue = 1.0
        })
    }

    // MARK: - Private Helpers

    private func getWindowSize() -> NSSize {
        switch haloViewModel.state {
        case .awaitingInputMode:
            // Input mode selection buttons
            return NSSize(width: 220, height: 100)

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
}
