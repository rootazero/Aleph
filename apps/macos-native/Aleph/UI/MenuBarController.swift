import Cocoa

/// Manages the system menu bar status item for Aleph.
///
/// Provides:
/// - A persistent menu bar icon (brain.head.profile SF Symbol)
/// - A dropdown menu with About, Show Halo, Settings, and Quit actions
/// - Dynamic status updates from the Rust core via `tray.update_status` bridge RPC
///
/// Menu structure mirrors the Tauri tray (`apps/desktop/src-tauri/src/tray.rs`).
final class MenuBarController: NSObject, ObservableObject {

    // MARK: - Properties

    private var statusItem: NSStatusItem?

    /// The current agent status (e.g. "idle", "thinking", "acting", "error").
    @Published var currentStatus: String = "idle"

    // MARK: - Public API

    /// Create the NSStatusItem and attach the dropdown menu.
    ///
    /// Call once during `applicationDidFinishLaunching`.
    func setup() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        if let button = statusItem?.button {
            button.image = NSImage(systemSymbolName: "brain.head.profile", accessibilityDescription: "Aleph")
            button.toolTip = "Aleph - AI Assistant"
        }
        statusItem?.menu = buildMenu()
    }

    /// Update the tray icon tooltip based on agent status.
    ///
    /// Called from the `tray.update_status` bridge handler.
    ///
    /// - Parameters:
    ///   - status: Agent status string ("idle", "thinking", "acting", "error").
    ///   - tooltip: Optional explicit tooltip text. If nil, derived from status.
    func updateStatus(_ status: String, tooltip: String? = nil) {
        currentStatus = status
        let tooltipText = tooltip ?? {
            switch status {
            case "thinking": return "Aleph - Thinking..."
            case "acting": return "Aleph - Acting..."
            case "error": return "Aleph - Error"
            default: return "Aleph - AI Assistant"
            }
        }()
        statusItem?.button?.toolTip = tooltipText
    }

    // MARK: - Menu Construction

    private func buildMenu() -> NSMenu {
        let menu = NSMenu()

        // About
        let aboutItem = NSMenuItem(title: "About Aleph", action: #selector(showAbout), keyEquivalent: "")
        aboutItem.target = self
        menu.addItem(aboutItem)

        // Version (disabled, informational)
        let versionItem = NSMenuItem(title: "Version 0.1.0", action: nil, keyEquivalent: "")
        versionItem.isEnabled = false
        menu.addItem(versionItem)

        menu.addItem(.separator())

        // Show Halo — Cmd+Option+/
        let haloItem = NSMenuItem(title: "Show Halo", action: #selector(showHalo), keyEquivalent: "/")
        haloItem.keyEquivalentModifierMask = [.command, .option]
        haloItem.target = self
        menu.addItem(haloItem)

        menu.addItem(.separator())

        // Settings — Cmd+,
        let settingsItem = NSMenuItem(title: "Settings...", action: #selector(showSettings), keyEquivalent: ",")
        settingsItem.target = self
        menu.addItem(settingsItem)

        menu.addItem(.separator())

        // Quit — Cmd+Q
        let quitItem = NSMenuItem(title: "Quit Aleph", action: #selector(quitApp), keyEquivalent: "q")
        quitItem.target = self
        menu.addItem(quitItem)

        return menu
    }

    // MARK: - Actions

    @objc private func showAbout() {
        NSApp.orderFrontStandardAboutPanel(nil)
    }

    @objc private func showHalo() {
        NotificationCenter.default.post(name: .showHalo, object: nil)
    }

    @objc private func showSettings() {
        NotificationCenter.default.post(name: .showSettings, object: nil)
    }

    @objc private func quitApp() {
        NSApp.terminate(nil)
    }
}

// MARK: - Notification Names

extension Notification.Name {
    /// Posted when the user selects "Show Halo" from the menu bar.
    static let showHalo = Notification.Name("com.aleph.showHalo")

    /// Posted when the user selects "Settings..." from the menu bar.
    static let showSettings = Notification.Name("com.aleph.showSettings")

    /// Posted by the `tray.update_status` bridge handler with userInfo:
    /// `["status": String, "tooltip": String?]`.
    static let updateTrayStatus = Notification.Name("com.aleph.updateTrayStatus")
}
