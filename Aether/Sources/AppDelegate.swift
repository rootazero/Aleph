//
//  AppDelegate.swift
//  Aether
//
//  Application delegate managing menu bar, Rust core lifecycle, and permissions.
//

import Cocoa
import SwiftUI

class AppDelegate: NSObject, NSApplicationDelegate {
    // Menu bar status item
    private var statusItem: NSStatusItem?

    // Rust core instance
    private var core: AetherCore?

    // Event handler for Rust callbacks
    private var eventHandler: EventHandler?

    // Halo overlay window
    private var haloWindow: HaloWindow?

    // Settings window
    private var settingsWindow: NSWindow?

    // Permission manager
    private var permissionManager = PermissionManager()

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Hide from Dock (menu bar only)
        NSApp.setActivationPolicy(.accessory)

        // Set up menu bar
        setupMenuBar()

        // Create Halo window
        haloWindow = HaloWindow()

        // Initialize event handler
        eventHandler = EventHandler(haloWindow: haloWindow)

        // Check permissions before initializing core
        checkAndRequestPermissions()
    }

    func applicationWillTerminate(_ notification: Notification) {
        // Clean up Rust core
        do {
            try core?.stopListening()
            print("[Aether] Core stopped successfully")
        } catch {
            print("[Aether] Error stopping core: \(error)")
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

        menu.addItem(NSMenuItem(title: "About Aether", action: #selector(showAbout), keyEquivalent: ""))
        menu.addItem(NSMenuItem.separator())
        menu.addItem(NSMenuItem(title: "Settings...", action: #selector(showSettings), keyEquivalent: ","))
        menu.addItem(NSMenuItem.separator())
        menu.addItem(NSMenuItem(title: "Quit Aether", action: #selector(quit), keyEquivalent: "q"))

        statusItem?.menu = menu
    }

    @objc private func showAbout() {
        let alert = NSAlert()
        alert.messageText = "Aether"
        alert.informativeText = "AI Middleware for macOS\nVersion 0.1.0 (Phase 2)\n\nBrings AI intelligence to your cursor."
        alert.alertStyle = .informational
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }

    @objc private func showSettings() {
        // Always recreate the window to ensure default size
        if let existingWindow = settingsWindow {
            existingWindow.close()
            settingsWindow = nil
        }

        let settingsView = SettingsView()
        let hostingController = NSHostingController(rootView: settingsView)

        let window = NSWindow(contentViewController: hostingController)
        window.title = "Aether Settings"
        window.setContentSize(NSSize(width: 800, height: 550))
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable]
        window.minSize = NSSize(width: 700, height: 500)
        window.maxSize = NSSize(width: 1200, height: 800)
        window.center()

        settingsWindow = window

        // Show and activate the window
        settingsWindow?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    @objc private func quit() {
        NSApplication.shared.terminate(nil)
    }

    // MARK: - Permissions

    private func checkAndRequestPermissions() {
        if permissionManager.checkAccessibility() {
            print("[Aether] Accessibility permission granted")
            initializeRustCore()
        } else {
            print("[Aether] Accessibility permission not granted")
            // Request permission (this adds app to Accessibility list but doesn't show system dialog)
            permissionManager.requestAccessibility()
            // Show our custom alert immediately
            showPermissionAlert()
        }
    }

    private func showPermissionAlert() {
        let alert = NSAlert()
        alert.messageText = "Accessibility Permission Required"
        alert.informativeText = """
        Aether needs Accessibility permission to:
        • Detect global hotkey (⌘~)
        • Read clipboard content
        • Simulate keyboard input

        Please grant permission in System Settings.
        """
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Open System Settings")
        alert.addButton(withTitle: "Quit")

        let response = alert.runModal()
        if response == .alertFirstButtonReturn {
            // First, request accessibility permission (this adds app to the list)
            permissionManager.requestAccessibility()
            // Then open System Settings for user to enable it
            permissionManager.openAccessibilitySettings()
            // Start polling for permission grant
            startPermissionPolling()
        } else {
            NSApplication.shared.terminate(nil)
        }
    }

    private func startPermissionPolling() {
        Timer.scheduledTimer(withTimeInterval: 2.0, repeats: true) { [weak self] timer in
            guard let self = self else {
                timer.invalidate()
                return
            }

            if self.permissionManager.checkAccessibility() {
                print("[Aether] Accessibility permission granted (polled)")
                timer.invalidate()
                self.initializeRustCore()
            }
        }
    }

    // MARK: - Rust Core Initialization

    private func initializeRustCore() {
        guard let eventHandler = eventHandler else {
            print("[Aether] Error: EventHandler not initialized")
            return
        }

        do {
            // Create AetherCore with event handler
            core = try AetherCore(handler: eventHandler)
            print("[Aether] AetherCore initialized")

            // Start listening for hotkeys
            try core?.startListening()
            print("[Aether] Hotkey listening started (⌘~)")

            // Update menu bar icon to show active state
            updateMenuBarIcon(state: .listening)

        } catch {
            print("[Aether] Error initializing core: \(error)")
            showErrorAlert(message: "Failed to initialize Aether core: \(error)")
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
            case .processing:
                button.image = NSImage(systemSymbolName: "sparkles.square.filled.on.square", accessibilityDescription: "Aether Processing")
            case .success:
                button.image = NSImage(systemSymbolName: "checkmark.circle", accessibilityDescription: "Success")
            case .error:
                button.image = NSImage(systemSymbolName: "exclamationmark.triangle", accessibilityDescription: "Error")
            }
        }
    }

    private func showErrorAlert(message: String) {
        let alert = NSAlert()
        alert.messageText = "Aether Error"
        alert.informativeText = message
        alert.alertStyle = .critical
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }
}
