//
//  WindowConfigurator.swift
//  Aether
//
//  Configures NSWindow to hide native traffic lights for custom window design.
//  Also enforces strict window size constraints that SwiftUI's Settings scene ignores.
//

import SwiftUI
import AppKit

/// Minimum window dimensions (shared constant)
private let kMinWindowWidth: CGFloat = 980
private let kMinWindowHeight: CGFloat = 750

/// NSViewRepresentable that configures the parent window to hide native traffic lights
/// and enforce strict size constraints
struct WindowConfigurator: NSViewRepresentable {

    func makeNSView(context: Context) -> NSView {
        // WindowConfiguratorView handles all window configuration in viewDidMoveToWindow
        return WindowConfiguratorView()
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        // Re-configure appearance if window changes (but not size/position)
        if let window = nsView.window {
            configureWindowAppearance(window)
        }
    }

    /// Configure window appearance only (not size/position)
    private func configureWindowAppearance(_ window: NSWindow) {
        // Make titlebar transparent
        window.titlebarAppearsTransparent = true

        // Hide standard window buttons (traffic lights)
        window.standardWindowButton(.closeButton)?.isHidden = true
        window.standardWindowButton(.miniaturizeButton)?.isHidden = true
        window.standardWindowButton(.zoomButton)?.isHidden = true

        // Ensure titlebar is hidden but window remains resizable
        window.titleVisibility = .hidden

        // Allow window to be dragged from content area
        window.isMovableByWindowBackground = true
    }
}

/// Custom NSView subclass that monitors window size changes
/// and enforces minimum size constraints using notifications (not delegate)
@MainActor
private class WindowConfiguratorView: NSView {

    /// Notification observers - nonisolated(unsafe) for cleanup in deinit
    nonisolated(unsafe) private var resizeObserver: NSObjectProtocol?
    nonisolated(unsafe) private var willResizeObserver: NSObjectProtocol?
    private var hasConfiguredInitialPosition = false

    /// Track which windows have been initially positioned to avoid re-centering
    nonisolated(unsafe) private static var positionedWindows = Set<ObjectIdentifier>()

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()

        guard let window = window else {
            // Clean up observers when removed from window
            cleanupObservers()
            return
        }

        // CRITICAL: Configure window size, position, and appearance IMMEDIATELY
        // This runs before the window is fully displayed, preventing the "jump" effect
        let windowId = ObjectIdentifier(window)
        if !Self.positionedWindows.contains(windowId) {
            // Configure window appearance first
            window.titlebarAppearsTransparent = true
            window.standardWindowButton(.closeButton)?.isHidden = true
            window.standardWindowButton(.miniaturizeButton)?.isHidden = true
            window.standardWindowButton(.zoomButton)?.isHidden = true
            window.titleVisibility = .hidden
            window.isMovableByWindowBackground = true

            // Set minimum size constraints
            window.minSize = NSSize(width: kMinWindowWidth, height: kMinWindowHeight)
            window.contentMinSize = NSSize(width: kMinWindowWidth, height: kMinWindowHeight)

            // Ensure window is at least minimum size
            let currentSize = window.frame.size
            if currentSize.width < kMinWindowWidth || currentSize.height < kMinWindowHeight {
                window.setContentSize(NSSize(width: kMinWindowWidth, height: kMinWindowHeight))
            }

            // Center the window
            window.center()

            Self.positionedWindows.insert(windowId)
            hasConfiguredInitialPosition = true
        }

        // Observe window resize events to enforce minimum size AFTER resize
        // Note: closure runs on .main queue so MainActor.assumeIsolated is safe
        resizeObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.didResizeNotification,
            object: window,
            queue: .main
        ) { [weak self, weak window] _ in
            MainActor.assumeIsolated {
                guard let window = window else { return }
                self?.enforceMinimumSize(for: window)
            }
        }

        // Schedule periodic check during resize
        // This catches any resize that somehow bypasses minSize
        willResizeObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.willStartLiveResizeNotification,
            object: window,
            queue: .main
        ) { [weak self, weak window] _ in
            MainActor.assumeIsolated {
                guard let window = window else { return }
                // Re-apply constraints at start of resize
                window.minSize = NSSize(width: kMinWindowWidth, height: kMinWindowHeight)
                window.contentMinSize = NSSize(width: kMinWindowWidth, height: kMinWindowHeight)
                self?.startResizeMonitor(for: window)
            }
        }
    }

    /// Resize timer - nonisolated(unsafe) for access from Timer callback
    nonisolated(unsafe) private var resizeTimer: Timer?

    private func startResizeMonitor(for window: NSWindow) {
        // Monitor during live resize to enforce constraints
        resizeTimer?.invalidate()
        let timer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self, weak window] _ in
            // Timer callbacks run on main thread
            MainActor.assumeIsolated {
                guard let window = window else {
                    self?.resizeTimer?.invalidate()
                    self?.resizeTimer = nil
                    return
                }

                // Check if still resizing
                if !window.inLiveResize {
                    self?.resizeTimer?.invalidate()
                    self?.resizeTimer = nil
                    // Final enforcement after resize ends
                    self?.enforceMinimumSize(for: window)
                    return
                }

                // Enforce during resize
                self?.enforceMinimumSize(for: window)
            }
        }
        resizeTimer = timer
    }

    private func enforceMinimumSize(for window: NSWindow) {
        let frame = window.frame
        if frame.width < kMinWindowWidth || frame.height < kMinWindowHeight {
            var newFrame = frame
            newFrame.size.width = max(frame.width, kMinWindowWidth)
            newFrame.size.height = max(frame.height, kMinWindowHeight)
            // Adjust origin to keep top-left corner in place
            newFrame.origin.y -= (newFrame.height - frame.height)
            window.setFrame(newFrame, display: true, animate: false)
        }
    }

    nonisolated private func cleanupObserversNonisolated() {
        // Safe cleanup that can be called from deinit
        // NotificationCenter.removeObserver is thread-safe
        if let observer = resizeObserver {
            NotificationCenter.default.removeObserver(observer)
        }
        if let observer = willResizeObserver {
            NotificationCenter.default.removeObserver(observer)
        }
    }

    private func cleanupObservers() {
        if let observer = resizeObserver {
            NotificationCenter.default.removeObserver(observer)
            resizeObserver = nil
        }
        if let observer = willResizeObserver {
            NotificationCenter.default.removeObserver(observer)
            willResizeObserver = nil
        }
        resizeTimer?.invalidate()
        resizeTimer = nil
    }

    deinit {
        cleanupObserversNonisolated()
    }
}

/// View modifier to apply window configuration
struct HideNativeTrafficLights: ViewModifier {
    func body(content: Content) -> some View {
        content
            .background(WindowConfigurator())
    }
}

extension View {
    /// Hides the native macOS traffic light buttons for custom window design
    func hideNativeTrafficLights() -> some View {
        modifier(HideNativeTrafficLights())
    }
}
