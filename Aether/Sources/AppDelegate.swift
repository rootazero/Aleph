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

    // Global hotkey monitor (Swift layer)
    private var hotkeyMonitor: GlobalHotkeyMonitor?

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
        // Stop hotkey monitoring
        hotkeyMonitor?.stopMonitoring()

        // Clean up Rust core (only if initialized)
        // Note: No need to call stopListening() as hotkey monitoring is now in Swift
        print("[Aether] Application terminating")
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

        // Add "Default Provider" submenu (will be populated by rebuildProvidersMenu)
        let defaultProviderMenuItem = NSMenuItem(
            title: NSLocalizedString("menu.default_provider", comment: "Default Provider menu item"),
            action: nil,
            keyEquivalent: ""
        )
        defaultProviderMenuItem.submenu = NSMenu()  // Create empty submenu for now
        menu.addItem(defaultProviderMenuItem)

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

        // Rebuild providers menu when core is initialized
        // (will be called later after initializeRustCore)
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
            // Window exists and is visible, reset to minimum size and bring to front
            window.setContentSize(NSSize(width: 980, height: 750))
            window.center()
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return
        }

        // Create new settings window with RootContentView
        // Pass core (may be nil if not initialized yet, RootContentView will handle gracefully)
        let settingsView = RootContentView(core: core)
            .environmentObject(self)

        let hostingController = NSHostingController(rootView: settingsView)

        // CRITICAL: Remove safe area insets to ensure content starts at window edge
        hostingController.view.wantsLayer = true
        hostingController.view.layer?.masksToBounds = false
        // Remove default safe area insets added by NSHostingController
        hostingController.safeAreaRegions = []

        let window = NSWindow(contentViewController: hostingController)
        window.title = "Settings"
        window.styleMask = [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView]
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden

        // Set minimum and initial size
        window.minSize = NSSize(width: 980, height: 750)
        window.setContentSize(NSSize(width: 980, height: 750))
        window.center()

        // Prevent window from hiding when losing focus
        window.hidesOnDeactivate = false
        // IMPORTANT: Set to true to recreate window with default size on next open
        window.isReleasedWhenClosed = true

        // Set window delegate to clear reference on close
        window.delegate = self

        // Store window reference
        settingsWindow = window

        // Show window
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    @objc private func quit() {
        NSApplication.shared.terminate(nil)
    }

    // MARK: - Providers Menu Management (NEW for default provider management)

    /// Rebuild the providers submenu with enabled providers
    private func rebuildProvidersMenu() {
        guard let menu = statusItem?.menu else { return }
        guard let core = core else { return }

        // Find the "Default Provider" menu item (should be at index 2 after About + Separator)
        guard let defaultProviderMenuItem = menu.items.first(where: { $0.title == NSLocalizedString("menu.default_provider", comment: "Default Provider menu item") }),
              let submenu = defaultProviderMenuItem.submenu else {
            print("[AppDelegate] ERROR: Default Provider submenu not found")
            return
        }

        // Clear existing submenu items
        submenu.removeAllItems()

        // Get enabled providers
        let enabledProviders = core.getEnabledProviders()

        if !enabledProviders.isEmpty {
            // Get current default provider
            let defaultProvider = try? core.getDefaultProvider()

            // Add menu items for each enabled provider (sorted alphabetically)
            for providerName in enabledProviders.sorted() {
                let item = NSMenuItem(
                    title: providerName,
                    action: #selector(selectDefaultProvider(_:)),
                    keyEquivalent: ""
                )

                // Add checkmark if this is the default provider
                if let defaultProvider = defaultProvider, providerName == defaultProvider {
                    item.state = .on
                } else {
                    item.state = .off
                }

                submenu.addItem(item)
            }

            // Enable the submenu
            defaultProviderMenuItem.isEnabled = true

            print("[AppDelegate] Rebuilt providers submenu with \(enabledProviders.count) enabled providers")
        } else {
            // No enabled providers, add placeholder and disable submenu
            let placeholderItem = NSMenuItem(
                title: NSLocalizedString("menu.no_providers", comment: "No providers available"),
                action: nil,
                keyEquivalent: ""
            )
            placeholderItem.isEnabled = false
            submenu.addItem(placeholderItem)

            defaultProviderMenuItem.isEnabled = false
            print("[AppDelegate] No enabled providers, disabling submenu")
        }
    }

    /// Handle provider selection from menu bar (set as default)
    @objc private func selectDefaultProvider(_ sender: NSMenuItem) {
        let providerName = sender.title

        print("[AppDelegate] User selected provider from menu: \(providerName)")

        guard let core = core else {
            print("[AppDelegate] ERROR: Core not initialized")
            return
        }

        do {
            try core.setDefaultProvider(providerName: providerName)
            print("[AppDelegate] ✅ Default provider set to: \(providerName)")

            // Rebuild menu to update checkmark
            rebuildProvidersMenu()

            // Optional: Show brief notification
            // (Could add a toast notification here in the future)
        } catch {
            print("[AppDelegate] ❌ Error setting default provider: \(error)")

            // Show error alert
            let alert = NSAlert()
            alert.messageText = "Failed to set default provider"
            alert.informativeText = "Could not set '\(providerName)' as default provider.\n\nError: \(error.localizedDescription)"
            alert.alertStyle = .warning
            alert.addButton(withTitle: "OK")
            alert.runModal()
        }
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
                print("[Aether]   - Input Monitoring: REQUIRED for full functionality")
            }

            // Show permission gate again
            DispatchQueue.main.async { [weak self] in
                self?.showPermissionGate()
            }
            return
        }

        do {
            // Create AetherCore with event handler
            // NOTE: Core no longer handles hotkey listening - that's now in Swift layer
            core = try AetherCore(handler: eventHandler)
            print("[Aether] AetherCore initialized successfully")

            // Set core reference in event handler for retry functionality
            eventHandler.setCore(core!)

            // IMPORTANT: Initialize Swift-based global hotkey monitor
            // This replaces the Rust-based EventTapListener to avoid thread conflicts
            print("[Aether] Initializing Swift-based global hotkey monitor...")
            hotkeyMonitor = GlobalHotkeyMonitor { [weak self] in
                // When ` key is detected, handle it in Swift layer
                self?.handleHotkeyPressed()
            }

            // Start monitoring for hotkey
            if hotkeyMonitor?.startMonitoring() == true {
                print("[Aether] ✅ Global hotkey monitoring started successfully (` key)")
            } else {
                print("[Aether] ❌ Failed to start global hotkey monitoring")
                // Fall back to showing permission gate
                DispatchQueue.main.async { [weak self] in
                    self?.showPermissionGate()
                }
                return
            }

            // Reset retry count on success
            coreInitRetryCount = 0

            // Update menu bar icon to show active state
            updateMenuBarIcon(state: .listening)

            // Rebuild providers menu now that core is initialized
            rebuildProvidersMenu()

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

        // Observe config changes to rebuild providers menu
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(onConfigChanged),
            name: NSNotification.Name("AetherConfigSavedInternally"),
            object: nil
        )
    }

    /// Handle config change notification (rebuild providers menu)
    @objc private func onConfigChanged() {
        print("[AppDelegate] Config changed, rebuilding providers menu")
        rebuildProvidersMenu()
    }

    // MARK: - Hotkey Handling

    /// Handle hotkey press in Swift layer (new architecture)
    ///
    /// This replaces the old flow where Rust detected the hotkey via rdev.
    /// Now Swift's GlobalHotkeyMonitor detects the key, and we handle everything here.
    private func handleHotkeyPressed() {
        print("[AppDelegate] Hotkey pressed - handling in Swift layer")

        // Get clipboard content using Swift ClipboardManager
        guard let clipboardText = ClipboardManager.shared.getText() else {
            print("[AppDelegate] No text in clipboard, ignoring hotkey")
            // Show brief error indication
            DispatchQueue.main.async { [weak self] in
                self?.haloWindow?.show(at: NSEvent.mouseLocation)
                self?.haloWindow?.updateState(.error(
                    type: .unknown,
                    message: NSLocalizedString("error.no_clipboard_text", comment: "No text in clipboard"),
                    suggestion: NSLocalizedString("error.no_clipboard_text.suggestion", comment: "Please select text first")
                ))
                // Auto-hide after 2 seconds
                DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) { [weak self] in
                    self?.haloWindow?.hide()
                }
            }
            return
        }

        print("[AppDelegate] Clipboard text: \(clipboardText.prefix(50))...")

        // Capture current context
        let context = ContextCapture.captureContext()
        print("[AppDelegate] Context: app=\(context.bundleId ?? "unknown"), window=\(context.windowTitle ?? "nil")")

        // Show Halo at cursor position
        DispatchQueue.main.async { [weak self] in
            let mouseLocation = NSEvent.mouseLocation
            self?.haloWindow?.show(at: mouseLocation)
            self?.haloWindow?.updateState(.listening)
        }

        // NEW ARCHITECTURE: Call Rust core's process_input() method
        guard let core = core else {
            print("[AppDelegate] ERROR: Core not initialized")
            DispatchQueue.main.async { [weak self] in
                self?.haloWindow?.updateState(.error(
                    type: .unknown,
                    message: NSLocalizedString("error.core_not_initialized", comment: "Core not initialized"),
                    suggestion: NSLocalizedString("error.core_not_initialized.suggestion", comment: "Please restart the app")
                ))
            }
            return
        }

        // Process input asynchronously to avoid blocking UI
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self = self else { return }

            do {
                // Create captured context for Rust
                let capturedContext = CapturedContext(
                    appBundleId: context.bundleId ?? "unknown",
                    windowTitle: context.windowTitle
                )

                // Call Rust core's process_input() - this replaces the old onHotkeyDetected flow
                // The method will handle: Memory retrieval → AI routing → Provider call → Memory storage
                // It returns the AI response text which we need to output using KeyboardSimulator
                let response = try core.processInput(
                    userInput: clipboardText,
                    context: capturedContext
                )

                print("[AppDelegate] Received AI response (\(response.count) chars)")

                // Type response using Swift KeyboardSimulator
                // This replaces Rust's typewriter implementation
                DispatchQueue.main.async {
                    // Get typing speed from config (default to 50 chars/sec)
                    let typingSpeed = 50 // TODO: Read from config when available

                    // Type the response
                    Task {
                        do {
                            try await KeyboardSimulator.shared.typeText(response, speed: typingSpeed)
                            print("[AppDelegate] ✅ Response typed successfully")

                            // Update Halo to success state
                            DispatchQueue.main.async { [weak self] in
                                self?.haloWindow?.updateState(.success(finalText: String(response.prefix(100))))
                                // Auto-hide after 2 seconds
                                DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) { [weak self] in
                                    self?.haloWindow?.hide()
                                }
                            }
                        } catch {
                            print("[AppDelegate] ❌ Error during typewriter: \(error)")
                            // Fall back to instant paste if typewriter fails
                            ClipboardManager.shared.setText(response)
                            KeyboardSimulator.shared.simulatePaste()
                        }
                    }
                }
            } catch {
                print("[AppDelegate] ❌ Error processing input: \(error)")

                // For AetherException, the error details have already been sent via callback
                // in Rust before throwing the exception. We just need to log it here.
                if error is AetherException {
                    print("[AppDelegate] AetherException caught - error details already sent via callback")
                    // Error already displayed via EventHandler.onError callback from Rust
                    // No need to show alert again
                } else {
                    // For non-Rust errors (e.g., Swift KeyboardSimulator errors)
                    let errorMessage = error.localizedDescription
                    let suggestion: String? = {
                        if let nsError = error as? NSError {
                            return nsError.userInfo["suggestion"] as? String
                        }
                        return nil
                    }()

                    DispatchQueue.main.async { [weak self] in
                        self?.eventHandler?.onError(
                            message: errorMessage,
                            suggestion: suggestion ?? NSLocalizedString("error.check_connection", comment: "Please check network and API config")
                        )
                    }
                }
            }
        }
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

    // MARK: - Application Lifecycle

    /// Prevent app from terminating when last window closes
    /// This is essential for menu bar apps - they should keep running with no windows open
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        return false
    }
}

// MARK: - NSWindowDelegate Extension

extension AppDelegate: NSWindowDelegate {
    /// Called when settings window is about to close
    /// Clear the window reference to ensure next open creates a fresh window with default size
    func windowWillClose(_ notification: Notification) {
        if let window = notification.object as? NSWindow, window == settingsWindow {
            print("[AppDelegate] Settings window closing, clearing reference")
            settingsWindow = nil
        }
    }
}
