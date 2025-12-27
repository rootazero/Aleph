//
//  WindowConfigurator.swift
//  Aether
//
//  Configures NSWindow to hide native traffic lights for custom window design.
//

import SwiftUI
import AppKit

/// NSViewRepresentable that configures the parent window to hide native traffic lights
struct WindowConfigurator: NSViewRepresentable {

    func makeNSView(context: Context) -> NSView {
        let view = NSView()

        // Configure window on next run loop to ensure window is available
        DispatchQueue.main.async {
            guard let window = view.window else { return }
            self.configureWindow(window)
        }

        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        // Re-configure if window changes
        if let window = nsView.window {
            configureWindow(window)
        }
    }

    /// Configure window to hide native traffic lights
    private func configureWindow(_ window: NSWindow) {
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
