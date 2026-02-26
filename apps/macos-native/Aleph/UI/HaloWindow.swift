import AppKit
import WebKit
import os

/// Floating Halo input panel — the primary quick-interaction surface.
///
/// HaloWindow is a borderless, non-activating NSPanel with a WKWebView that
/// loads the Leptos/WASM halo UI from aleph-server. It floats above all windows,
/// centered horizontally at 30% from the bottom of the screen.
///
/// This is the Swift equivalent of the Tauri "halo" window:
/// - 800x80, transparent, no decorations, always on top, skip taskbar
///
/// Usage:
/// ```swift
/// let halo = HaloWindow()
/// halo.configure(serverPort: 3456)
/// halo.show()
/// ```
final class HaloWindow: NSObject {

    // MARK: - Constants

    private static let defaultWidth: CGFloat = 800
    private static let defaultHeight: CGFloat = 80

    // MARK: - Properties

    private var panel: NSPanel?
    private var webView: WKWebView?
    private var serverPort: Int = 3456
    private let logger = Logger(subsystem: "com.aleph.app", category: "HaloWindow")

    // MARK: - Init

    override init() {
        super.init()
    }

    // MARK: - Public API

    /// Set the server port used to construct the halo URL.
    func configure(serverPort: Int) {
        self.serverPort = serverPort
    }

    /// Show the halo panel, creating it lazily if needed.
    ///
    /// Positions the panel centered horizontally, 30% from the bottom of the
    /// main screen, then loads the halo URL.
    func show() {
        let panel = ensurePanel()

        // Position: center horizontally, 30% from bottom (= 70% from top)
        if let screen = NSScreen.main {
            let screenFrame = screen.visibleFrame
            let panelSize = panel.frame.size

            let x = screenFrame.origin.x + (screenFrame.width - panelSize.width) / 2
            // In macOS coordinates, origin is bottom-left, so 30% from bottom
            // means y = screenFrame.origin.y + screenFrame.height * 0.30 - panelSize.height / 2
            let y = screenFrame.origin.y + screenFrame.height * 0.30 - panelSize.height / 2

            panel.setFrameOrigin(NSPoint(x: x, y: y))
        }

        // Load halo URL
        let url = haloURL()
        webView?.load(URLRequest(url: url))

        panel.makeKeyAndOrderFront(nil)
        logger.info("Halo window shown")
    }

    /// Hide the halo panel.
    func hide() {
        panel?.orderOut(nil)
        logger.info("Halo window hidden")
    }

    /// Navigate the halo webview to a specific URL.
    func navigate(to url: URL) {
        webView?.load(URLRequest(url: url))
    }

    /// Whether the panel is currently visible.
    var isVisible: Bool {
        panel?.isVisible ?? false
    }

    // MARK: - Private

    /// Lazily create the NSPanel + WKWebView.
    private func ensurePanel() -> NSPanel {
        if let existing = panel { return existing }

        // Style: borderless, non-activating, floating (like Spotlight)
        let styleMask: NSWindow.StyleMask = [.borderless, .nonactivatingPanel]

        let newPanel = NSPanel(
            contentRect: NSRect(
                x: 0,
                y: 0,
                width: Self.defaultWidth,
                height: Self.defaultHeight
            ),
            styleMask: styleMask,
            backing: .buffered,
            defer: false
        )

        newPanel.level = .floating
        newPanel.isOpaque = false
        newPanel.backgroundColor = .clear
        newPanel.hasShadow = false
        newPanel.isMovableByWindowBackground = false
        newPanel.hidesOnDeactivate = false
        newPanel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
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

    /// Construct the halo URL from the server port.
    private func haloURL() -> URL {
        URL(string: "http://127.0.0.1:\(serverPort)/halo")!
    }
}
