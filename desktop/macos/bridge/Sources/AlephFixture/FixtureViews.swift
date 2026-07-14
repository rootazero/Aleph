import AppKit

/// A window AppKit will not drag back onto a screen.
///
/// Headless mode parks the window off every display (see `main.swift`), and
/// `constrainFrameRect` is the thing that would otherwise yank a titled window
/// back into view.
final class FixtureWindow: NSWindow {
    override func constrainFrameRect(_ frameRect: NSRect, to screen: NSScreen?) -> NSRect {
        frameRect
    }

    // A borderless/accessory window is not key by default, and the type_text test
    // needs one: a keyboard event posted to a pid routes to that process's KEY
    // window, so without this the typed text would land nowhere.
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { true }
}

/// Mouse target for the synthetic (CGEvent) rail.
///
/// It is a plain `NSView` on purpose: it is not an accessibility element, so it
/// advertises none of the actions that stand in for a left click (`AXPress` /
/// `AXConfirm` / `AXOpen`). The bridge's AX-first click ladder therefore finds no
/// rung-1 candidate here and falls through to rung 2 — a real synthesized mouse
/// event. That is what makes this view a test of the CGEvent rail rather than of
/// the AX rail, and it is why the counter BUTTON (which does advertise `AXPress`)
/// is a separate control: one view per rail, so a green never means "the other
/// rail covered for it".
///
/// What it records is what only the receiving app can testify to:
///
/// * `click_count` — carried ON the event. A double-click is not "two clicks sent
///   quickly": the app reads the count off the event, so a rail that posts two
///   independent singles delivers two `clickCount == 1` events. This view is
///   where that distinction becomes observable.
/// * `steps` — the number of intermediate `mouseDragged` events. A drag that is
///   just a down at the start and an up at the end reads to an app as a CLICK at
///   the start point; `steps == 0` is how that failure gets caught.
final class PadView: NSView {
    var onEvent: ((_ kind: String, _ value: String) -> Void)?

    private var dragOrigin: NSPoint?
    private var dragSteps = 0

    /// Never take focus: the type_text test depends on the text field holding
    /// first responder, and a click on this pad must not quietly steal it.
    override var acceptsFirstResponder: Bool { false }

    /// Accept a click even when the window is not key, so a click lands on the
    /// first event rather than being eaten as a window-activation click.
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.systemGray.setFill()
        dirtyRect.fill()
    }

    override func mouseDown(with event: NSEvent) {
        dragOrigin = convert(event.locationInWindow, from: nil)
        dragSteps = 0
        onEvent?("click", "click_count=\(event.clickCount)")
    }

    override func mouseDragged(with event: NSEvent) {
        dragSteps += 1
    }

    override func mouseUp(with event: NSEvent) {
        // A mouseUp with no motion behind it is the tail of a click, and the click
        // was already reported on mouseDown. Only a path that was actually walked
        // is a drag.
        guard dragSteps > 0, let origin = dragOrigin else { return }
        let end = convert(event.locationInWindow, from: nil)
        // The step count is interpolated rather than `%d`-formatted: `Int` is 64-bit
        // and `%d` takes an Int32.
        let from = String(format: "%.0f,%.0f", origin.x, origin.y)
        let to = String(format: "%.0f,%.0f", end.x, end.y)
        onEvent?("drag", "steps=\(dragSteps);from=\(from);to=\(to)")
        dragOrigin = nil
    }
}

/// Document view for the scroll area.
///
/// Flipped so that scrolling DOWN increases `bounds.origin.y`. With AppKit's
/// default bottom-left origin the sign is inverted, and an assertion about which
/// way the content moved would read backwards.
final class FlippedView: NSView {
    override var isFlipped: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        NSColor.textBackgroundColor.setFill()
        dirtyRect.fill()
    }
}
