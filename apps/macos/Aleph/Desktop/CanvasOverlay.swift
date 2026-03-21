import AppKit
import WebKit
import os

/// Transparent canvas overlay for AI-generated UI (A2UI).
///
/// CanvasOverlay is a borderless, non-activating NSPanel at the screen-saver
/// window level. It is fully transparent and ignores all mouse events
/// (click-through), making it an invisible surface the server can paint on.
///
/// The server controls the canvas via three RPC methods:
/// - `desktop.canvas_show`   — set frame, load HTML, inject A2UI handler JS
/// - `desktop.canvas_hide`   — hide without destroying
/// - `desktop.canvas_update` — apply A2UI surface patches via JS eval
///
/// This is the Swift equivalent of `apps/desktop/src-tauri/src/bridge/canvas.rs`.
///
/// Usage:
/// ```swift
/// let canvas = CanvasOverlay()
/// canvas.show(html: "<h1>Hello</h1>", position: CanvasPosition(x: 100, y: 100, width: 400, height: 300))
/// canvas.update(patch: "[{\"type\":\"surfaceUpdate\",\"content\":\"<p>Updated</p>\"}]")
/// canvas.hide()
/// ```
final class CanvasOverlay: NSObject {

    // MARK: - Properties

    private var panel: NSPanel?
    private var webView: WKWebView?
    private let logger = Logger(subsystem: "com.aleph.app", category: "CanvasOverlay")

    // MARK: - Init

    override init() {
        super.init()
    }

    // MARK: - Public API

    /// Show the canvas overlay with the given HTML content and position.
    ///
    /// Creates the panel lazily, sets frame to the specified position,
    /// loads the HTML via a data URI, and injects the A2UI patch handler JS.
    ///
    /// - Parameters:
    ///   - html: HTML content to render.
    ///   - position: Screen position and size for the overlay.
    func show(html: String, position: CanvasPosition) {
        let panel = ensurePanel()

        // macOS screen coordinates: origin is bottom-left.
        // The CanvasPosition follows screen coordinates from the server (top-left origin).
        // Convert: macOS_y = screen_height - position.y - position.height
        let frame: NSRect
        if let screen = NSScreen.main {
            let screenHeight = screen.frame.height
            let macY = screenHeight - position.y - position.height
            frame = NSRect(x: position.x, y: macY, width: position.width, height: position.height)
        } else {
            frame = NSRect(x: position.x, y: position.y, width: position.width, height: position.height)
        }

        panel.setFrame(frame, display: true)

        // Load HTML via data URI
        let htmlContent = html.isEmpty ? "<html><body></body></html>" : html
        if let data = htmlContent.data(using: .utf8) {
            let base64 = data.base64EncodedString()
            let dataURI = "data:text/html;base64,\(base64)"
            if let url = URL(string: dataURI) {
                webView?.load(URLRequest(url: url))
            }
        }

        panel.orderFrontRegardless()

        // Inject A2UI handler after a brief delay to allow page load
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { [weak self] in
            self?.injectA2UIHandler()
        }

        logger.info("Canvas overlay shown at (\(position.x), \(position.y), \(position.width)x\(position.height))")
    }

    /// Hide the canvas overlay without destroying it.
    func hide() {
        panel?.orderOut(nil)
        logger.info("Canvas overlay hidden")
    }

    /// Apply an A2UI patch to the canvas via JavaScript evaluation.
    ///
    /// Calls `window.alephApplyPatch(patch)` in the webview. The patch should
    /// be a JSON string representing an array of A2UI operations.
    ///
    /// - Parameter patch: JSON string of the patch array.
    func update(patch: String) {
        guard let webView = webView else {
            logger.warning("Canvas update called but webview not available")
            return
        }

        let script = """
            if (typeof window.alephApplyPatch === 'function') {
                window.alephApplyPatch(\(patch));
            }
            """

        webView.evaluateJavaScript(script) { [weak self] _, error in
            if let error = error {
                self?.logger.error("Canvas patch eval error: \(error.localizedDescription)")
            }
        }
    }

    /// Whether the panel is currently visible.
    var isVisible: Bool {
        panel?.isVisible ?? false
    }

    // MARK: - Private

    /// Lazily create the NSPanel + WKWebView.
    private func ensurePanel() -> NSPanel {
        if let existing = panel { return existing }

        let styleMask: NSWindow.StyleMask = [.borderless, .nonactivatingPanel]

        let newPanel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 400, height: 300),
            styleMask: styleMask,
            backing: .buffered,
            defer: false
        )

        // Screen-saver level: above everything
        newPanel.level = NSWindow.Level(rawValue: Int(CGShieldingWindowLevel()))
        newPanel.isOpaque = false
        newPanel.backgroundColor = .clear
        newPanel.hasShadow = false
        newPanel.ignoresMouseEvents = true
        newPanel.hidesOnDeactivate = false
        newPanel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        newPanel.isReleasedWhenClosed = false

        // WKWebView with transparent background
        let config = WKWebViewConfiguration()
        config.preferences.setValue(true, forKey: "developerExtrasEnabled")

        let wv = WKWebView(frame: newPanel.contentView!.bounds, configuration: config)
        wv.autoresizingMask = [.width, .height]
        wv.setValue(false, forKey: "drawsBackground")

        newPanel.contentView?.addSubview(wv)

        panel = newPanel
        webView = wv
        return newPanel
    }

    /// Inject the A2UI patch handler JavaScript into the webview.
    ///
    /// Installs `window.alephApplyPatch(patch)` which the server can later
    /// invoke via `desktop.canvas_update` to apply surface mutations.
    private func injectA2UIHandler() {
        let script = """
            window.alephApplyPatch = function(patch) {
                if (!Array.isArray(patch)) return;
                patch.forEach(function(op) {
                    if (op.type === 'surfaceUpdate' && op.content) {
                        document.body.innerHTML = op.content;
                    }
                });
            };
            """

        webView?.evaluateJavaScript(script) { [weak self] _, error in
            if let error = error {
                self?.logger.error("A2UI handler injection error: \(error.localizedDescription)")
            }
        }
    }
}
