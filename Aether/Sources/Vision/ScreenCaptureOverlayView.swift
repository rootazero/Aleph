import AppKit
import Foundation

/// Overlay view for screen region selection
///
/// This view displays a semi-transparent overlay and allows users to
/// draw a selection rectangle for screen capture.
final class ScreenCaptureOverlayView: NSView {
    // MARK: - Properties

    /// Completion handler called when selection is complete
    private var onComplete: ((CGRect) -> Void)?

    /// Cancellation handler called when selection is cancelled
    private var onCancel: (() -> Void)?

    /// Starting point of the selection
    private var startPoint: CGPoint?

    /// Current selection rectangle (in view coordinates)
    private var currentRect: CGRect?

    /// Tracking area for mouse events
    private var trackingArea: NSTrackingArea?

    /// Overlay background color
    private let overlayColor = NSColor.black.withAlphaComponent(0.3)

    /// Selection border color
    private let selectionBorderColor = NSColor.white

    /// Selection fill color
    private let selectionFillColor = NSColor.white.withAlphaComponent(0.1)

    // MARK: - Initialization

    init(onComplete: @escaping (CGRect) -> Void, onCancel: @escaping () -> Void) {
        self.onComplete = onComplete
        self.onCancel = onCancel
        super.init(frame: .zero)
        setupView()
    }

    @available(*, unavailable)
    required init?(coder _: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    deinit {
        // Defensive cleanup - should already be done by prepareForDismissal()
        if let area = trackingArea {
            removeTrackingArea(area)
            trackingArea = nil
        }
    }

    // MARK: - Dismissal

    /// Prepare the view for safe dismissal
    /// Must be called by the coordinator before closing the containing window
    func prepareForDismissal() {
        // Remove tracking area to break owner reference
        if let area = trackingArea {
            removeTrackingArea(area)
            trackingArea = nil
        }

        // Clear callbacks - this is the key safety mechanism
        // After this, onComplete?() and onCancel?() become no-ops
        onComplete = nil
        onCancel = nil
    }

    // MARK: - Setup

    private func setupView() {
        // Enable layer backing for better performance
        wantsLayer = true
        layer?.backgroundColor = overlayColor.cgColor

        // Set up cursor
        addCursorRect(bounds, cursor: .crosshair)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()

        // Remove old tracking area
        if let existingArea = trackingArea {
            removeTrackingArea(existingArea)
            trackingArea = nil
        }

        // Add new tracking area
        let newArea = NSTrackingArea(
            rect: bounds,
            options: [.activeAlways, .mouseMoved, .mouseEnteredAndExited],
            owner: self,
            userInfo: nil
        )
        addTrackingArea(newArea)
        trackingArea = newArea
    }

    override func resetCursorRects() {
        super.resetCursorRects()
        addCursorRect(bounds, cursor: .crosshair)
    }

    // MARK: - Mouse Events

    override func mouseDown(with event: NSEvent) {
        startPoint = convert(event.locationInWindow, from: nil)
        currentRect = nil
        needsDisplay = true
    }

    override func mouseDragged(with event: NSEvent) {
        guard let start = startPoint else { return }

        let current = convert(event.locationInWindow, from: nil)

        // Calculate selection rectangle
        let minX = min(start.x, current.x)
        let minY = min(start.y, current.y)
        let width = abs(current.x - start.x)
        let height = abs(current.y - start.y)

        currentRect = CGRect(x: minX, y: minY, width: width, height: height)
        needsDisplay = true
    }

    override func mouseUp(with _: NSEvent) {
        guard let rect = currentRect, rect.width > 10, rect.height > 10 else {
            // Selection too small, cancel
            onCancel?()
            return
        }

        // Convert to screen coordinates and complete
        let screenRect = convertToScreenCoordinates(rect)
        onComplete?(screenRect)
    }

    // MARK: - Keyboard Events

    override var acceptsFirstResponder: Bool {
        true
    }

    override func keyDown(with event: NSEvent) {
        // Escape key cancels selection
        if event.keyCode == 53 {
            onCancel?()
        }
    }

    // MARK: - Drawing

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)

        guard let context = NSGraphicsContext.current?.cgContext else { return }

        // Draw semi-transparent overlay
        context.setFillColor(overlayColor.cgColor)
        context.fill(bounds)

        // Draw selection rectangle if exists
        if let rect = currentRect {
            // Clear the selection area (make it transparent)
            context.setBlendMode(.clear)
            context.fill(rect)

            // Reset blend mode
            context.setBlendMode(.normal)

            // Draw selection fill
            context.setFillColor(selectionFillColor.cgColor)
            context.fill(rect)

            // Draw selection border
            context.setStrokeColor(selectionBorderColor.cgColor)
            context.setLineWidth(2.0)
            context.stroke(rect)

            // Draw size indicator
            drawSizeIndicator(for: rect, in: context)
        }

        // Draw instructions
        drawInstructions(in: context)
    }

    private func drawSizeIndicator(for rect: CGRect, in context: CGContext) {
        let sizeText = "\(Int(rect.width)) x \(Int(rect.height))"

        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.monospacedSystemFont(ofSize: 12, weight: .medium),
            .foregroundColor: NSColor.white,
        ]

        let attributedString = NSAttributedString(string: sizeText, attributes: attributes)
        let textSize = attributedString.size()

        // Position below the selection rectangle
        let textX = rect.midX - textSize.width / 2
        let textY = rect.minY - textSize.height - 8

        // Draw background for text
        let textRect = CGRect(
            x: textX - 4,
            y: textY - 2,
            width: textSize.width + 8,
            height: textSize.height + 4
        )

        context.setFillColor(NSColor.black.withAlphaComponent(0.7).cgColor)
        let bgPath = NSBezierPath(roundedRect: textRect, xRadius: 4, yRadius: 4)
        bgPath.fill()

        // Draw text
        attributedString.draw(at: NSPoint(x: textX, y: textY))
    }

    private func drawInstructions(in _: CGContext) {
        let instructions = "Drag to select region • Press ESC to cancel"

        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 14, weight: .medium),
            .foregroundColor: NSColor.white.withAlphaComponent(0.8),
        ]

        let attributedString = NSAttributedString(string: instructions, attributes: attributes)
        let textSize = attributedString.size()

        // Position at top center of screen
        let textX = bounds.midX - textSize.width / 2
        let textY = bounds.maxY - textSize.height - 40

        // Draw background
        let textRect = CGRect(
            x: textX - 12,
            y: textY - 6,
            width: textSize.width + 24,
            height: textSize.height + 12
        )

        NSColor.black.withAlphaComponent(0.6).setFill()
        let bgPath = NSBezierPath(roundedRect: textRect, xRadius: 8, yRadius: 8)
        bgPath.fill()

        // Draw text
        attributedString.draw(at: NSPoint(x: textX, y: textY))
    }

    // MARK: - Coordinate Conversion

    /// Convert view coordinates to screen coordinates
    private func convertToScreenCoordinates(_ rect: CGRect) -> CGRect {
        guard let window = window, let screen = window.screen else {
            return rect
        }

        // Convert from view coordinates to window coordinates
        let windowRect = convert(rect, to: nil)

        // Convert from window coordinates to screen coordinates
        let screenOrigin = window.convertPoint(toScreen: windowRect.origin)

        // Flip Y coordinate (AppKit uses bottom-left origin, but CGWindowListCreateImage uses top-left)
        let screenHeight = screen.frame.height
        let flippedY = screenHeight - screenOrigin.y - windowRect.height

        return CGRect(
            x: screenOrigin.x,
            y: flippedY,
            width: windowRect.width,
            height: windowRect.height
        )
    }
}
