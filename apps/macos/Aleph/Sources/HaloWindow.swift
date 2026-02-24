//
//  HaloWindow.swift
//  Aleph
//
//  Floating overlay window that hosts the Leptos Halo chat UI via WKWebView.
//  Manages window visibility, positioning, and animations.
//

import Cocoa
import WebKit

// MARK: - HaloWindow

/// Floating overlay window for Halo UI (WKWebView-backed)
final class HaloWindow: NSWindow {

    // MARK: - Properties

    /// Time when window was shown (for minimum display time calculations)
    private(set) var showTime: Date?

    /// Hide sequence counter for animation cancellation
    private var hideSequence: Int = 0

    /// WKWebView hosting the Leptos Halo UI
    private var webView: HaloWebView?

    // MARK: - Initialization

    init() {
        super.init(
            contentRect: NSRect(x: 0, y: 0, width: 300, height: 200),
            styleMask: .borderless,
            backing: .buffered,
            defer: false
        )

        setupWindow()
        setupWebView()
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

        // Focus handling
        hidesOnDeactivate = false

        // Enable mouse events for WebView interaction
        ignoresMouseEvents = false
    }

    private func setupWebView() {
        let haloWeb = HaloWebView()
        haloWeb.frame = contentView?.bounds ?? .zero
        haloWeb.autoresizingMask = [.width, .height]
        contentView = haloWeb
        self.webView = haloWeb
    }

    // MARK: - Focus

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }

    // MARK: - WebView

    /// Reload the Halo web page (e.g., after server restart).
    func reloadWebView() {
        webView?.reload()
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

    // MARK: - Hide Methods

    /// Hide with animation
    func hide() {
        showTime = nil
        hideSequence += 1
        let currentSequence = hideSequence

        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.2
            self.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            MainActor.assumeIsolated {
                guard let self = self, self.hideSequence == currentSequence else { return }
                self.orderOut(nil)
            }
        })
    }

    /// Force hide immediately without animation
    func forceHide() {
        showTime = nil
        hideSequence += 1
        alphaValue = 0
        orderOut(nil)
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
