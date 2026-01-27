//
//  ConditionalDragArea.swift
//  Aether
//
//  Smart window drag area that enables dragging in blank/padding areas.
//  Automatically detects if mouse is over interactive content.
//

import SwiftUI
import AppKit

// MARK: - ConditionalDragArea

/// A view that enables smart window dragging in blank areas
/// - Blank/padding areas: Allow window dragging (passes through to window)
/// - Interactive content: Allows normal interaction (buttons, text fields block this)
struct ConditionalDragArea: NSViewRepresentable {

    func makeNSView(context: Context) -> NSView {
        let view = SmartDraggableView()
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {}
}

// MARK: - SmartDraggableView

private class SmartDraggableView: NSView {

    override var mouseDownCanMoveWindow: Bool {
        // Always return true - this view is placed in blank areas only
        // Interactive content (buttons, text fields) will block events naturally
        return true
    }
}
