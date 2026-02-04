//
//  NonDraggableArea.swift
//  Aether
//
//  Prevents window dragging in specific areas (e.g., text selection areas).
//

import SwiftUI
import AppKit

// MARK: - NonDraggableArea

/// A view that prevents window dragging in its area
/// Use this to allow text selection or other interactions that conflict with dragging
struct NonDraggableArea: NSViewRepresentable {

    func makeNSView(context: Context) -> NSView {
        let view = NonDraggableView()
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {}
}

// MARK: - NonDraggableView

private class NonDraggableView: NSView {

    override var mouseDownCanMoveWindow: Bool {
        // Return false to prevent window dragging in this area
        return false
    }
}
