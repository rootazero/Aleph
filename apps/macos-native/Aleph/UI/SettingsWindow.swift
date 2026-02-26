import AppKit
import WebKit
import os

/// Settings window — standard macOS window hosting the Leptos/WASM settings UI.
///
/// SettingsWindow is a titled, closable, resizable NSWindow with a WKWebView that
/// loads the settings page from aleph-server. It remembers its frame position
/// across show/hide cycles using `NSWindow.setFrameAutosaveName`.
///
/// This is the Swift equivalent of the Tauri "settings" window:
/// - 900x650 default, 700x500 minimum, titled, resizable, closable, miniaturizable
///
/// Usage:
/// ```swift
/// let settings = SettingsWindow()
/// settings.configure(serverPort: 3456)
/// settings.show()
/// ```
final class SettingsWindow: NSObject, NSWindowDelegate {

    // MARK: - Constants

    private static let defaultWidth: CGFloat = 900
    private static let defaultHeight: CGFloat = 650
    private static let minWidth: CGFloat = 700
    private static let minHeight: CGFloat = 500
    private static let frameAutosaveName = "AlephSettingsWindow"

    // MARK: - Properties

    private var window: NSWindow?
    private var webView: WKWebView?
    private var serverPort: Int = 3456
    private let logger = Logger(subsystem: "com.aleph.app", category: "SettingsWindow")

    // MARK: - Init

    override init() {
        super.init()
    }

    // MARK: - Public API

    /// Set the server port used to construct the settings URL.
    func configure(serverPort: Int) {
        self.serverPort = serverPort
    }

    /// Show the settings window, creating it lazily if needed.
    ///
    /// Restores the saved frame position, or centers the window if no saved
    /// position exists. Loads the settings URL.
    func show() {
        let win = ensureWindow()

        // Restore saved position or center
        if !win.setFrameUsingName(Self.frameAutosaveName) {
            win.center()
        }

        // Load settings URL
        let url = settingsURL()
        webView?.load(URLRequest(url: url))

        win.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        logger.info("Settings window shown")
    }

    /// Hide the settings window and save its frame.
    func hide() {
        window?.saveFrame(usingName: Self.frameAutosaveName)
        window?.orderOut(nil)
        logger.info("Settings window hidden")
    }

    /// Navigate the settings webview to a specific URL.
    func navigate(to url: URL) {
        webView?.load(URLRequest(url: url))
    }

    /// Whether the window is currently visible.
    var isVisible: Bool {
        window?.isVisible ?? false
    }

    // MARK: - NSWindowDelegate

    func windowWillClose(_ notification: Notification) {
        window?.saveFrame(usingName: Self.frameAutosaveName)
        logger.info("Settings window will close")
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

        newWindow.title = "Aleph Settings"
        newWindow.minSize = NSSize(width: Self.minWidth, height: Self.minHeight)
        newWindow.isReleasedWhenClosed = false
        newWindow.setFrameAutosaveName(Self.frameAutosaveName)
        newWindow.delegate = self

        // WKWebView filling the window
        let config = WKWebViewConfiguration()
        config.preferences.setValue(true, forKey: "developerExtrasEnabled")

        let wv = WKWebView(frame: newWindow.contentView!.bounds, configuration: config)
        wv.autoresizingMask = [.width, .height]

        newWindow.contentView?.addSubview(wv)

        window = newWindow
        webView = wv
        return newWindow
    }

    /// Construct the settings URL from the server port.
    private func settingsURL() -> URL {
        URL(string: "http://127.0.0.1:\(serverPort)/settings")!
    }
}
