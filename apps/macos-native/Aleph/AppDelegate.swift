import AppKit

/// Application delegate responsible for lifecycle management.
///
/// Responsibilities:
/// - Set activation policy to .accessory (hide from Dock)
/// - Manage aleph-server process lifecycle
/// - Set up menu bar status item
/// - Manage Halo floating window and Settings window
/// - Handle UDS bridge connection
final class AppDelegate: NSObject, NSApplicationDelegate {

    private let menuBarController = MenuBarController()

    // MARK: - Lifecycle

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Hide from Dock — menu bar only (also set via LSUIElement in Info.plist)
        NSApp.setActivationPolicy(.accessory)

        // Set up menu bar status item
        menuBarController.setup()

        // Listen for tray status updates from the bridge
        NotificationCenter.default.addObserver(
            forName: .updateTrayStatus,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            let status = notification.userInfo?["status"] as? String ?? "idle"
            let tooltip = notification.userInfo?["tooltip"] as? String
            self?.menuBarController.updateStatus(status, tooltip: tooltip)
        }

        // TODO: Task 3 — Start ServerManager (launch aleph-server)
        // TODO: Task 5 — Start BridgeServer (UDS listener)
        // TODO: Task 12 — Initialize HaloWindow and SettingsWindow
        // TODO: Task 15 — Register global shortcuts
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        // Keep running in menu bar even when all windows are closed
        return false
    }

    func applicationWillTerminate(_ notification: Notification) {
        // TODO: Task 3 — Stop aleph-server process gracefully
        // TODO: Task 5 — Close UDS socket
    }
}
