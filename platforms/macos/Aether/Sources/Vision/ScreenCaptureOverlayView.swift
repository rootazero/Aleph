import AppKit
import Foundation

// Debug file logger for crash investigation
private func debugLog(_ message: String) {
    let timestamp = ISO8601DateFormatter().string(from: Date())
    let logMessage = "[\(timestamp)] [OverlayView] \(message)\n"
    let logPath = NSHomeDirectory() + "/Desktop/aether_debug.log"

    if let data = logMessage.data(using: .utf8) {
        if FileManager.default.fileExists(atPath: logPath) {
            if let handle = FileHandle(forWritingAtPath: logPath) {
                handle.seekToEndOfFile()
                handle.write(data)
                handle.closeFile()
            }
        } else {
            FileManager.default.createFile(atPath: logPath, contents: data)
        }
    }
    NSLog("[OverlayView] \(message)")
}

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
    /// nonisolated(unsafe) to allow cleanup in deinit
    nonisolated(unsafe) private var trackingArea: NSTrackingArea?

    /// Flag indicating the view is being dismissed - prevents callbacks and updates
    /// CRITICAL: This flag is essential for preventing EXC_BAD_ACCESS crashes.
    /// Without it, AppKit can call updateTrackingAreas() or other methods
    /// after prepareForDismissal() but before the window is fully closed,
    /// leading to dangling references and crashes during autorelease pool drain.
    private var isDismissed = false

    /// Overlay background color
    private let overlayColor = NSColor.black.withAlphaComponent(0.3)

    /// Selection border color
    private let selectionBorderColor = NSColor.white

    /// Selection fill color
    private let selectionFillColor = NSColor.white.withAlphaComponent(0.1)

    // MARK: - Initialization

    /// Initialize with callbacks (legacy API for compatibility)
    init(onComplete: @escaping (CGRect) -> Void, onCancel: @escaping () -> Void) {
        debugLog(" >>> init START")
        self.onComplete = onComplete
        self.onCancel = onCancel
        super.init(frame: .zero)
        setupView()
        debugLog(" >>> init COMPLETE")
    }

    /// Initialize without callbacks (for window reuse pattern)
    /// Use setCallbacks() to set callbacks before showing
    override init(frame frameRect: NSRect) {
        debugLog(" >>> init(frame:) START")
        super.init(frame: frameRect)
        setupView()
        debugLog(" >>> init(frame:) COMPLETE")
    }

    @available(*, unavailable)
    required init?(coder _: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    // MARK: - Window Reuse Support

    /// Set callbacks for the next capture session
    /// Call this before showing the window
    func setCallbacks(
        onComplete: @escaping (CGRect) -> Void,
        onCancel: @escaping () -> Void
    ) {
        debugLog(" setCallbacks()")
        self.onComplete = onComplete
        self.onCancel = onCancel
    }

    /// Reset view state for reuse
    /// Call this before showing the window for a new capture session
    func reset() {
        debugLog(" >>> reset() START")

        // 1. Reset dismissed flag - CRITICAL for reuse
        isDismissed = false

        // 2. Clear selection state
        startPoint = nil
        currentRect = nil

        // 3. Recreate tracking area (safe since window is hidden)
        if let existingArea = trackingArea {
            removeTrackingArea(existingArea)
            trackingArea = nil
        }

        // Only create tracking area if we have valid bounds
        if bounds.width > 0 && bounds.height > 0 {
            let newArea = NSTrackingArea(
                rect: bounds,
                options: [.activeAlways, .mouseMoved, .mouseEnteredAndExited],
                owner: self,
                userInfo: nil
            )
            addTrackingArea(newArea)
            trackingArea = newArea
        }

        // 4. Request redraw to clear any previous selection visuals
        setNeedsDisplay(bounds)

        debugLog(" >>> reset() COMPLETE")
    }

    deinit {
        debugLog(" >>> deinit START")
        // Note: cleanup should be done by prepareForDismissal() before deinit
        // We just clear the reference here - actual removal was done earlier
        trackingArea = nil
        debugLog(" >>> deinit COMPLETE")
    }

    // MARK: - Dismissal

    /// Prepare the view for safe dismissal
    /// Must be called by the coordinator before closing the containing window
    func prepareForDismissal() {
        debugLog(" >>> prepareForDismissal() START")

        // Set flag FIRST to prevent any concurrent operations from other methods
        // This is critical for thread safety and preventing race conditions
        isDismissed = true

        // Immediately remove tracking area to break owner reference
        // This prevents AppKit from calling methods on this view via the tracking area
        if let area = trackingArea {
            debugLog(" Removing tracking area...")
            removeTrackingArea(area)
            trackingArea = nil
        }

        // Clear callbacks to prevent any pending invocations
        // This breaks potential retain cycles
        debugLog(" Clearing callbacks...")
        onComplete = nil
        onCancel = nil

        // Reset state to prevent any stale data access
        startPoint = nil
        currentRect = nil

        debugLog(" >>> prepareForDismissal() COMPLETE")
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

        // CRITICAL: Don't create new tracking areas after dismissal
        // Without this guard, AppKit can call this method after prepareForDismissal()
        // but before window.close(), creating new tracking areas that reference `self`.
        // This leads to dangling pointers and EXC_BAD_ACCESS during autorelease pool drain.
        guard !isDismissed else { return }

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
        // Guard against dismissed state
        guard !isDismissed else { return }

        startPoint = convert(event.locationInWindow, from: nil)
        currentRect = nil
        needsDisplay = true
    }

    override func mouseDragged(with event: NSEvent) {
        // Guard against dismissed state
        guard !isDismissed else { return }

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
        // Guard against dismissed state - callbacks are nil but avoid any work
        guard !isDismissed else { return }

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
        // Guard against dismissed state
        guard !isDismissed else { return }

        // Escape key cancels selection
        if event.keyCode == 53 {
            onCancel?()
        }
    }

    // MARK: - Drawing

    override func draw(_ dirtyRect: NSRect) {
        super.draw(dirtyRect)

        // Guard against dismissed state - prevent drawing after dismissal
        guard !isDismissed else { return }

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
