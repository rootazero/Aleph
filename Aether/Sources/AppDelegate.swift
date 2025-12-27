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

    // Theme engine
    private var themeEngine: ThemeEngine?

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Hide from Dock (menu bar only)
        NSApp.setActivationPolicy(.accessory)

        // Set up menu bar
        setupMenuBar()

        // Check and request Accessibility permission for context capture
        checkAccessibilityPermission()

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
        // Use URL scheme to open settings window (works even after window is closed)
        if let url = URL(string: "aether://settings") {
            NSWorkspace.shared.open(url)
        }

        // Activate app to bring window to front
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

        do {
            // Create AetherCore with event handler
            core = try AetherCore(handler: eventHandler)
            print("[Aether] AetherCore initialized")

            // Set core reference in event handler for retry functionality
            eventHandler.setCore(core!)

            // Start listening for hotkeys
            try core?.startListening()
            print("[Aether] Hotkey listening started (⌘~)")

            // Reset retry count on success
            coreInitRetryCount = 0

            // Update menu bar icon to show active state
            updateMenuBarIcon(state: .listening)

        } catch {
            print("[Aether] Error initializing core: \(error)")

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
                showErrorAlert(message: "Failed to initialize Aether core after \(maxRetryAttempts) attempts.\n\nError: \(error)\n\nPlease check:\n1. Accessibility permissions are granted\n2. libaethecore.dylib is properly bundled\n3. Rust core is built correctly")
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

    private func showErrorAlert(message: String) {
        let alert = NSAlert()
        alert.messageText = "Aether Error"
        alert.informativeText = message
        alert.alertStyle = .critical
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }

    // MARK: - Accessibility Permission Check

    private func checkAccessibilityPermission() {
        if !ContextCapture.hasAccessibilityPermission() {
            print("[Aether] Accessibility permission not granted, requesting...")

            // Show alert with button to open System Settings
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) {
                ContextCapture.showPermissionAlert()
            }
        } else {
            print("[Aether] Accessibility permission already granted")
        }
    }
}
