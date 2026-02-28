import AppKit
import WebKit
import os

/// Unified Panel window — the single window hosting the full Leptos/WASM Panel UI.
///
/// PanelWindow is a standard titled, closable, resizable NSWindow with a WKWebView that
/// loads the Panel root from aleph-server. It replaces both HaloWindow and SettingsWindow
/// as the unified interface surface. The Panel UI handles its own internal navigation
/// (Chat, Settings, Dashboard, etc.) via Leptos client-side routing.
///
/// This window remembers its frame position across show/hide cycles using
/// `NSWindow.setFrameAutosaveName`.
///
/// Usage:
/// ```swift
/// let panel = PanelWindow()
/// panel.configure(serverPort: 18790)
/// panel.show()
/// panel.showSettings()  // Navigate to /settings within the panel
/// ```
final class PanelWindow: NSObject, NSWindowDelegate, WKUIDelegate {

    // MARK: - Constants

    private static let defaultWidth: CGFloat = 1000
    private static let defaultHeight: CGFloat = 700
    private static let minWidth: CGFloat = 800
    private static let minHeight: CGFloat = 550
    private static let frameAutosaveName = "AlephPanelWindow"

    // MARK: - Properties

    private var window: NSWindow?
    private var webView: WKWebView?
    private var serverPort: Int = 18790
    private let logger = Logger(subsystem: "com.aleph.app", category: "PanelWindow")

    // MARK: - Init

    override init() {
        super.init()
    }

    // MARK: - Public API

    /// Set the server port used to construct Panel URLs.
    func configure(serverPort: Int) {
        self.serverPort = serverPort
    }

    /// Show the panel window, creating it lazily if needed.
    ///
    /// Restores the saved frame position, or centers the window if no saved
    /// position exists. Loads the panel root URL.
    func show() {
        let win = ensureWindow()

        // Restore saved position or center
        if !win.setFrameUsingName(Self.frameAutosaveName) {
            win.center()
        }

        // Load panel root URL (only if not already loaded)
        if webView?.url == nil {
            let url = panelURL()
            webView?.load(URLRequest(url: url))
        }

        win.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        logger.info("Panel window shown")
    }

    /// Hide the panel window and save its frame.
    func hide() {
        window?.saveFrame(usingName: Self.frameAutosaveName)
        window?.orderOut(nil)
        logger.info("Panel window hidden")
    }

    /// Navigate the panel webview to a specific path within the panel.
    ///
    /// - Parameter path: The path to navigate to (e.g. "/settings", "/dashboard").
    func navigate(to path: String) {
        let url = panelURL(path: path)
        webView?.load(URLRequest(url: url))
    }

    /// Navigate the panel webview to an arbitrary URL.
    func navigate(to url: URL) {
        webView?.load(URLRequest(url: url))
    }

    /// Show the Chat view.
    func showChat() {
        show()
        navigate(to: "/chat")
    }

    /// Show the Settings view within the panel.
    func showSettings() {
        show()
        navigate(to: "/settings")
    }

    /// Show the Dashboard view within the panel.
    func showDashboard() {
        show()
        navigate(to: "/dashboard")
    }

    /// Whether the window is currently visible.
    var isVisible: Bool {
        window?.isVisible ?? false
    }

    // MARK: - NSWindowDelegate

    func windowWillClose(_ notification: Notification) {
        window?.saveFrame(usingName: Self.frameAutosaveName)
        logger.info("Panel window will close")
    }

    // MARK: - WKUIDelegate

    func webView(
        _ webView: WKWebView,
        runOpenPanelWith parameters: WKOpenPanelParameters,
        initiatedByFrame frame: WKFrameInfo,
        completionHandler: @escaping ([URL]?) -> Void
    ) {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = parameters.allowsMultipleSelection
        panel.canChooseFiles = true
        panel.canChooseDirectories = false

        panel.beginSheetModal(for: webView.window ?? NSApp.keyWindow!) { response in
            completionHandler(response == .OK ? panel.urls : nil)
        }
    }

    // MARK: - Private

    /// Lazily create the NSWindow + WKWebView.
    private func ensureWindow() -> NSWindow {
        if let existing = window { return existing }

        let styleMask: NSWindow.StyleMask = [.titled, .closable, .resizable, .miniaturizable]

        let newWindow = NSWindow(
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

        newWindow.title = "Aleph"
        newWindow.minSize = NSSize(width: Self.minWidth, height: Self.minHeight)
        newWindow.isReleasedWhenClosed = false
        newWindow.setFrameAutosaveName(Self.frameAutosaveName)
        newWindow.delegate = self

        // WKWebView filling the window
        let config = WKWebViewConfiguration()
        config.preferences.setValue(true, forKey: "developerExtrasEnabled")
        config.websiteDataStore = .nonPersistent()

        let wv = WKWebView(frame: newWindow.contentView!.bounds, configuration: config)
        wv.autoresizingMask = [.width, .height]
        wv.uiDelegate = self

        newWindow.contentView?.addSubview(wv)

        window = newWindow
        webView = wv
        return newWindow
    }

    /// Construct a Panel URL from the server port and optional path.
    private func panelURL(path: String = "/") -> URL {
        URL(string: "http://127.0.0.1:\(serverPort)\(path)")!
    }
}
