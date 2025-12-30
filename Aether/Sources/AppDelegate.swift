//
//  AppDelegate.swift
//  Aether
//
//  Application delegate managing menu bar, Rust core lifecycle, and permissions.
//

import Cocoa
import SwiftUI
import Combine

class AppDelegate: NSObject, NSApplicationDelegate, ObservableObject {
    // Menu bar status item
    private var statusItem: NSStatusItem?

    // Menu item for Settings (stored separately for enable/disable)
    private var settingsMenuItem: NSMenuItem?

    // Rust core instance (internal for access from AetherApp)
    // Published to trigger UI updates when initialized
    @Published internal var core: AetherCore?

    // Keychain manager for secure API key storage (internal for access from AetherApp)
    internal var keychainManager: KeychainManagerImpl = KeychainManagerImpl()

    // Event handler for Rust callbacks
    private var eventHandler: EventHandler?

    // Halo overlay window
    private var haloWindow: HaloWindow?

    // Settings window (used by legacy Settings scene and WindowGroup)
    private var settingsWindow: NSWindow?

    // Permission gate window
    private var permissionGateWindow: NSWindow?

    // Permission gate active state
    private var isPermissionGateActive: Bool = false

    // Theme engine
    private var themeEngine: ThemeEngine?

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Hide from Dock (menu bar only)
        NSApp.setActivationPolicy(.accessory)

        // Set up menu bar
        setupMenuBar()

        // CRITICAL FIX: Delay permission check to allow macOS to sync permission state
        // macOS needs time to update permission status after app launch
        // Without this delay, AXIsProcessTrusted() and IOHIDRequestAccess() may return
        // cached/stale values, causing false negatives even when permissions are granted
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { [weak self] in
            guard let self = self else { return }

            print("[Aether] Checking permissions after startup delay...")

            // Check all required permissions (Accessibility + Input Monitoring)
            if !self.checkAllRequiredPermissions() {
                // Show mandatory permission gate if any permission is missing
                self.showPermissionGate()
            } else {
                print("[Aether] ✅ All permissions granted, proceeding with initialization")
                // All permissions granted, proceed with normal initialization
                self.initializeAppComponents()
            }
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        // Clean up Rust core (only if initialized)
        if let core = core {
            do {
                try core.stopListening()
                print("[Aether] Core stopped successfully")
            } catch {
                print("[Aether] Error stopping core: \(error)")
            }
        } else {
            print("[Aether] Application terminating (Core was not initialized)")
        }
    }

    // MARK: - Menu Bar Setup

    private func setupMenuBar() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)

        if let button = statusItem?.button {
            // Use SF Symbol for menu bar icon
            button.image = NSImage(systemSymbolName: "sparkles", accessibilityDescription: "Aether")
            button.image?.isTemplate = true
        }

        // Create menu
        let menu = NSMenu()

        menu.addItem(NSMenuItem(
            title: NSLocalizedString("menu.about", comment: "About menu item"),
            action: #selector(showAbout),
            keyEquivalent: ""
        ))
        menu.addItem(NSMenuItem.separator())

        // Create and store Settings menu item for enable/disable control
        settingsMenuItem = NSMenuItem(
            title: NSLocalizedString("menu.settings", comment: "Settings menu item"),
            action: #selector(showSettings),
            keyEquivalent: ","
        )
        menu.addItem(settingsMenuItem!)

        menu.addItem(NSMenuItem.separator())
        menu.addItem(NSMenuItem(
            title: NSLocalizedString("menu.quit", comment: "Quit menu item"),
            action: #selector(quit),
            keyEquivalent: "q"
        ))

        statusItem?.menu = menu

        // Initially disable Settings menu if permissions not granted
        settingsMenuItem?.isEnabled = !isPermissionGateActive
    }

    @objc private func showAbout() {
        let alert = NSAlert()
        alert.messageText = NSLocalizedString("alert.about.title", comment: "")
        alert.informativeText = String(format: NSLocalizedString("alert.about.message", comment: ""), "0.1.0 (Phase 2)")
        alert.alertStyle = .informational
        alert.addButton(withTitle: NSLocalizedString("common.ok", comment: "OK button"))
        alert.runModal()
    }

    @objc private func showSettings() {
        // Block settings access if permission gate is active
        if isPermissionGateActive {
            print("[Aether] Settings blocked - permission gate is active")
            return
        }

        // Check if settings window already exists
        if let window = settingsWindow, window.isVisible {
            // Window exists and is visible, bring to front
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return
        }

        // Create new settings window with RootContentView
        let settingsView = RootContentView(
            core: core,
            keychainManager: keychainManager
        )
        .environmentObject(self)
        .frame(minWidth: 980, minHeight: 750)

        let hostingController = NSHostingController(rootView: settingsView)

        let window = NSWindow(contentViewController: hostingController)
        window.title = "Settings"
        window.setContentSize(NSSize(width: 980, height: 750))
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable]
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.center()

        // Prevent window from hiding when losing focus
        window.hidesOnDeactivate = false
        window.isReleasedWhenClosed = false

        // Store window reference
        settingsWindow = window

        // Show window
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    @objc private func quit() {
        NSApplication.shared.terminate(nil)
    }

    // MARK: - Rust Core Initialization

    private var coreInitRetryCount = 0
    private let maxRetryAttempts = 3

    private func initializeRustCore() {
        guard let eventHandler = eventHandler else {
            print("[Aether] Error: EventHandler not initialized")
            return
        }

        // CRITICAL: Re-verify permissions before initializing Core
        // This prevents crashes if permissions were revoked or not fully applied
        let hasAccessibility = PermissionChecker.hasAccessibilityPermission()
        let hasInputMonitoring = PermissionChecker.hasInputMonitoringPermission()

        print("[Aether] Pre-Core init permission check - Accessibility: \(hasAccessibility), InputMonitoring: \(hasInputMonitoring)")

        if !hasAccessibility || !hasInputMonitoring {
            print("[Aether] ERROR: Permissions not fully granted, BLOCKING Core initialization")
            print("[Aether] Missing permissions:")
            if !hasAccessibility {
                print("[Aether]   - Accessibility: REQUIRED for global hotkey detection")
            }
            if !hasInputMonitoring {
                print("[Aether]   - Input Monitoring: REQUIRED to prevent rdev crashes")
            }

            // CRITICAL: DO NOT initialize Core or call start_listening() without permissions
            // This will cause rdev::listen() to crash the entire application

            // Show permission gate again
            DispatchQueue.main.async { [weak self] in
                self?.showPermissionGate()
            }
            return
        }

        do {
            // Create AetherCore with event handler
            core = try AetherCore(handler: eventHandler)
            print("[Aether] AetherCore initialized successfully")

            // Set core reference in event handler for retry functionality
            eventHandler.setCore(core!)

            // IMPORTANT: Only start listening if permissions are confirmed
            // start_listening() will call rdev::listen() which REQUIRES Input Monitoring permission
            print("[Aether] Starting hotkey listener (this requires Input Monitoring permission)...")
            try core?.startListening()
            print("[Aether] ✅ Hotkey listening started successfully (default: ` key)")

            // Reset retry count on success
            coreInitRetryCount = 0

            // Update menu bar icon to show active state
            updateMenuBarIcon(state: .listening)

        } catch {
            print("[Aether] ❌ Error initializing core: \(error)")

            // Attempt retry with exponential backoff
            if coreInitRetryCount < maxRetryAttempts {
                coreInitRetryCount += 1
                let retryDelay = Double(coreInitRetryCount) * 2.0 // 2s, 4s, 6s

                print("[Aether] Retrying initialization in \(retryDelay)s (attempt \(coreInitRetryCount)/\(maxRetryAttempts))")

                DispatchQueue.main.asyncAfter(deadline: .now() + retryDelay) { [weak self] in
                    self?.initializeRustCore()
                }
            } else {
                // Max retries exceeded, show error to user
                print("[Aether] Max retry attempts exceeded, giving up")
                showErrorAlert(message: "Failed to initialize Aether core after \(maxRetryAttempts) attempts.\n\nError: \(error)\n\nPlease check:\n1. Accessibility permissions are granted\n2. Input Monitoring permissions are granted\n3. libaethecore.dylib is properly bundled\n4. Rust core is built correctly\n\nYou may need to restart your Mac for permissions to take full effect.")
            }
        }
    }

    private func updateMenuBarIcon(state: ProcessingState) {
        DispatchQueue.main.async { [weak self] in
            guard let button = self?.statusItem?.button else { return }

            switch state {
            case .idle:
                button.image = NSImage(systemSymbolName: "sparkles", accessibilityDescription: "Aether")
            case .listening:
                button.image = NSImage(systemSymbolName: "sparkles", accessibilityDescription: "Aether Listening")
                // Could add color tint here
            case .retrievingMemory:
                button.image = NSImage(systemSymbolName: "brain.head.profile", accessibilityDescription: "Retrieving Memory")
            case .processingWithAi:
                button.image = NSImage(systemSymbolName: "cpu", accessibilityDescription: "Processing with AI")
            case .processing:
                button.image = NSImage(systemSymbolName: "sparkles.square.filled.on.square", accessibilityDescription: "Aether Processing")
            case .typewriting:
                button.image = NSImage(systemSymbolName: "keyboard", accessibilityDescription: "Typewriting")
            case .success:
                button.image = NSImage(systemSymbolName: "checkmark.circle", accessibilityDescription: "Success")
            case .error:
                button.image = NSImage(systemSymbolName: "exclamationmark.triangle", accessibilityDescription: "Error")
            }
        }
    }

    /// Update menu bar icon with custom symbol (for permission gate states)
    private func updateMenuBarIcon(systemSymbol: String) {
        DispatchQueue.main.async { [weak self] in
            guard let button = self?.statusItem?.button else { return }
            button.image = NSImage(systemSymbolName: systemSymbol, accessibilityDescription: "Aether")
        }
    }

    private func showErrorAlert(message: String) {
        let alert = NSAlert()
        alert.messageText = NSLocalizedString("error.aether", comment: "")
        alert.informativeText = message
        alert.alertStyle = .critical
        alert.addButton(withTitle: NSLocalizedString("common.ok", comment: ""))
        alert.runModal()
    }

    // MARK: - Permission Gate Management

    /// Check if all required permissions are granted
    /// - Returns: true if both Accessibility and Input Monitoring permissions are granted
    private func checkAllRequiredPermissions() -> Bool {
        let hasAccessibility = PermissionChecker.hasAccessibilityPermission()
        let hasInputMonitoring = PermissionChecker.hasInputMonitoringPermission()

        print("[Aether] Permission status - Accessibility: \(hasAccessibility), InputMonitoring: \(hasInputMonitoring)")

        return hasAccessibility && hasInputMonitoring
    }

    /// Show mandatory permission gate window
    private func showPermissionGate() {
        print("[Aether] Showing permission gate - permissions not granted")

        isPermissionGateActive = true

        // Disable settings menu item
        settingsMenuItem?.isEnabled = false

        // Update menu bar icon to show "waiting" state
        updateMenuBarIcon(systemSymbol: "exclamationmark.triangle")

        // Create permission gate view
        let permissionGateView = PermissionGateView {
            // Callback when all permissions are granted
            self.onPermissionGateDismissed()
        }

        let hostingController = NSHostingController(rootView: permissionGateView)

        // Create window for permission gate
        let window = NSWindow(contentViewController: hostingController)
        window.title = "Aether 需要权限"
        window.setContentSize(NSSize(width: 600, height: 600))
        window.styleMask = [.titled, .closable, .miniaturizable]
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.center()

        // CRITICAL: Prevent window from hiding when losing focus
        // This ensures the permission gate stays visible even when user switches to System Settings
        window.hidesOnDeactivate = false
        window.isReleasedWhenClosed = false

        // Set window level to modal panel (less aggressive than floating)
        // This keeps the window visible without conflicting with system windows
        window.level = .modalPanel

        // Keep window in front of other apps' windows
        window.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]

        // Make window non-closable by overriding close button behavior
        window.standardWindowButton(.closeButton)?.isEnabled = false

        // Store window reference
        permissionGateWindow = window

        // Show window and bring to front
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)

        print("[Aether] Permission gate window shown with floating level")
    }

    /// Called when permission gate is dismissed (all permissions granted)
    private func onPermissionGateDismissed() {
        print("[Aether] Permission gate dismissed - all permissions granted")

        isPermissionGateActive = false

        // Enable settings menu item
        settingsMenuItem?.isEnabled = true

        // Close permission gate window
        permissionGateWindow?.close()
        permissionGateWindow = nil

        // Initialize app components now that permissions are granted
        initializeAppComponents()
    }

    /// Initialize all app components (theme, halo, event handler, core)
    /// This is called either directly on launch (if permissions already granted)
    /// or after permission gate is dismissed
    private func initializeAppComponents() {
        print("[Aether] Initializing app components")

        // Initialize theme engine
        themeEngine = ThemeEngine()

        // Create Halo window with theme engine
        haloWindow = HaloWindow(themeEngine: themeEngine!)

        // Initialize event handler
        eventHandler = EventHandler(haloWindow: haloWindow)

        // Connect event handler to halo window for error action callbacks
        haloWindow?.setEventHandler(eventHandler!)

        // Initialize Rust core
        initializeRustCore()
    }

    // MARK: - Accessibility Permission Check (Legacy - now using PermissionGate)

    private func checkAccessibilityPermission() {
        // This method is now deprecated in favor of the permission gate
        // Kept for backward compatibility but not used in current flow
        if !ContextCapture.hasAccessibilityPermission() {
            print("[Aether] Accessibility permission not granted, showing prompt...")

            // Use unified software popup instead of system NSAlert
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in
                self?.eventHandler?.showPermissionPrompt(type: .accessibility)
            }
        } else {
            print("[Aether] Accessibility permission already granted")
        }
    }
}
