import AppKit
import os

/// Application delegate responsible for lifecycle management.
///
/// Responsibilities:
/// - Set activation policy to .accessory (hide from Dock)
/// - Manage aleph-server process lifecycle via `ServerManager`
/// - Set up menu bar status item via `MenuBarController`
/// - Manage unified Panel window and Canvas overlay
/// - Handle UDS bridge connection via `BridgeServer`
/// - Register global keyboard shortcuts
/// - Wire notification-based communication between bridge handlers and UI
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {

    // MARK: - Components

    private let serverManager = ServerManager()
    private let bridge = BridgeServer()
    private let menuBarController = MenuBarController()
    private let panelWindow = PanelWindow()
    private let canvasOverlay = CanvasOverlay()
    private let globalShortcuts = GlobalShortcuts()

    private let logger = Logger(subsystem: "com.aleph.app", category: "AppDelegate")

    /// Default server port for the Leptos/WASM UI.
    private let serverPort = 18790

    // MARK: - Lifecycle

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Hide from Dock — menu bar only (also set via LSUIElement in Info.plist)
        NSApp.setActivationPolicy(.accessory)

        // Set up menu bar status item
        menuBarController.setup()

        // Register global shortcuts (Cmd+Opt+/ to show Chat)
        globalShortcuts.register()

        // Set up all notification observers
        setupNotificationObservers()

        // Start server and bridge asynchronously
        Task { @MainActor in
            await startServices()
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        // Keep running in menu bar even when all windows are closed
        return false
    }

    func applicationWillTerminate(_ notification: Notification) {
        // Stop bridge server (synchronous)
        bridge.stop()

        // Unregister global shortcuts
        globalShortcuts.unregister()

        // Send SIGTERM to server (synchronous — no time for async graceful stop)
        serverManager.terminateNow()
    }

    // MARK: - Service Startup

    /// Start aleph-server and bridge server sequentially.
    @MainActor
    private func startServices() async {
        // Start aleph-server process
        do {
            try await serverManager.start()
            logger.info("aleph-server started successfully")
        } catch {
            logger.error("Failed to start aleph-server: \(error.localizedDescription)")
        }

        // Configure panel window with server port
        panelWindow.configure(serverPort: serverPort)

        // Register all desktop handlers and PIM handlers, then start bridge server
        bridge.registerDesktopHandlers()
        bridge.registerPIMHandlers()
        do {
            try bridge.start()
            logger.info("BridgeServer started successfully")
        } catch {
            logger.error("Failed to start BridgeServer: \(error.localizedDescription)")
        }
    }

    // MARK: - Notification Observers

    /// Wire all notification observers for inter-component communication.
    private func setupNotificationObservers() {
        let nc = NotificationCenter.default

        // Menu bar actions
        nc.addObserver(
            forName: .showHalo,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.panelWindow.showChat()
        }

        nc.addObserver(
            forName: .showSettings,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.panelWindow.showSettings()
        }

        // Tray status updates from bridge
        nc.addObserver(
            forName: .updateTrayStatus,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            let status = notification.userInfo?["status"] as? String ?? "idle"
            let tooltip = notification.userInfo?["tooltip"] as? String
            self?.menuBarController.updateStatus(status, tooltip: tooltip)
        }

        // WebView show/hide/navigate from bridge
        nc.addObserver(
            forName: .webviewShow,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let self else { return }
            let label = notification.userInfo?["label"] as? String ?? "panel"
            switch label {
            case "settings":
                self.panelWindow.showSettings()
            case "dashboard":
                self.panelWindow.showDashboard()
            default:
                self.panelWindow.show()
            }
        }

        nc.addObserver(
            forName: .webviewHide,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.panelWindow.hide()
        }

        nc.addObserver(
            forName: .webviewNavigate,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let self else { return }
            guard let urlString = notification.userInfo?["url"] as? String,
                  let url = URL(string: urlString) else { return }
            self.panelWindow.navigate(to: url)
        }

        // Canvas overlay from bridge
        nc.addObserver(
            forName: .canvasShow,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let self else { return }
            let html = notification.userInfo?["html"] as? String ?? ""
            let x = notification.userInfo?["x"] as? Double ?? 0
            let y = notification.userInfo?["y"] as? Double ?? 0
            let width = notification.userInfo?["width"] as? Double ?? 400
            let height = notification.userInfo?["height"] as? Double ?? 300
            let position = CanvasPosition(x: x, y: y, width: width, height: height)
            self.canvasOverlay.show(html: html, position: position)
        }

        nc.addObserver(
            forName: .canvasHide,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            self?.canvasOverlay.hide()
        }

        nc.addObserver(
            forName: .canvasUpdate,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let patch = notification.userInfo?["patch"] as? String else { return }
            self?.canvasOverlay.update(patch: patch)
        }
    }
}
