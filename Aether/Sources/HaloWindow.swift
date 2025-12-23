//
//  HaloWindow.swift
//  Aether
//
//  Transparent, floating NSWindow for Halo overlay.
//  CRITICAL: Must never steal focus from active application.
//

import Cocoa
import SwiftUI

class HaloWindow: NSWindow {
    private var haloHostingView: NSHostingView<HaloView>?
    private var haloView: HaloView

    init() {
        // Create HaloView
        haloView = HaloView()

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
        haloHostingView?.frame = self.contentView!.bounds
        haloHostingView?.autoresizingMask = [.width, .height]

        self.contentView?.addSubview(haloHostingView!)

        // Start hidden
        self.alphaValue = 0
        self.orderOut(nil)
    }

    // MARK: - Public API

    func show(at position: NSPoint) {
        // Position window at cursor
        let screenFrame = NSScreen.main?.frame ?? .zero
        var windowOrigin = position

        // Center window on cursor
        windowOrigin.x -= self.frame.width / 2
        windowOrigin.y -= self.frame.height / 2

        // Clamp to screen bounds
        windowOrigin.x = max(0, min(windowOrigin.x, screenFrame.width - self.frame.width))
        windowOrigin.y = max(0, min(windowOrigin.y, screenFrame.height - self.frame.height))

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
        // Fade out animation
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.3
            self.animator().alphaValue = 0
        }, completionHandler: {
            self.orderOut(nil)
        })
    }

    func updateState(_ state: HaloState) {
        haloView.state = state
    }
}
