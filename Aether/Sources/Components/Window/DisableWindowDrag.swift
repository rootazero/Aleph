//
//  DisableWindowDrag.swift
//  Aether
//
//  Temporarily disables window dragging for views that need drag interactions (like List reordering).
//  Prevents conflict between SwiftUI drag gestures and window background dragging.
//

import SwiftUI
import AppKit

/// View modifier that disables window background dragging for its content
///
/// Use this modifier on views that implement their own drag interactions (like List with .onMove)
/// to prevent the window from being dragged when the user tries to reorder items.
struct DisableWindowDrag: ViewModifier {
    func body(content: Content) -> some View {
        content
            .background(WindowDragDisabler())
    }
}

/// NSViewRepresentable that disables window dragging by setting isMovableByWindowBackground to false
private struct WindowDragDisabler: NSViewRepresentable {
    func makeNSView(context: Context) -> WindowDragDisablerView {
        WindowDragDisablerView()
    }

    func updateNSView(_ nsView: WindowDragDisablerView, context: Context) {
        // No updates needed
    }
}

/// Custom NSView that disables window dragging when added to the view hierarchy
private class WindowDragDisablerView: NSView {
    private var originalWindowDraggable: Bool = true

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()

        guard let window = window else {
            return
        }

        // Defer window property modification to avoid KVO notification during SwiftUI's
        // AttributeGraph update cycle. Synchronous modification can crash SwiftUI internals.
        let capturedWindow = window
        DispatchQueue.main.async { [weak self] in
            guard let self = self, self.window === capturedWindow else { return }
            self.originalWindowDraggable = capturedWindow.isMovableByWindowBackground
            capturedWindow.isMovableByWindowBackground = false
        }
    }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
        // Restore original state when view is removed
        if let window = window, newWindow == nil {
            // Use async to match the async disable operation
            let capturedWindow = window
            let originalValue = originalWindowDraggable
            DispatchQueue.main.async {
                capturedWindow.isMovableByWindowBackground = originalValue
            }
        }

        super.viewWillMove(toWindow: newWindow)
    }
}

extension View {
    /// Disables window background dragging for this view
    ///
    /// Use this on views that implement their own drag interactions (like List with .onMove)
    /// to prevent the window from being dragged when the user tries to drag items.
    ///
    /// Example:
    /// ```swift
    /// List {
    ///     ForEach(items) { item in
    ///         Text(item.name)
    ///     }
    ///     .onMove(perform: moveItems)
    /// }
    /// .disableWindowDrag()
    /// ```
    func disableWindowDrag() -> some View {
        modifier(DisableWindowDrag())
    }
}
