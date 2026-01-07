//
//  AppDelegate.swift
//  Aether
//
//  Application delegate managing menu bar, Rust core lifecycle, and permissions.
//

import Cocoa
import SwiftUI
import Combine
import Carbon.HIToolbox

/// Tracks where the input text came from, used to determine output strategy
enum TextSource {
    case selectedText      // User had text selected, Cmd+X/C captured it
    case accessibilityAPI  // No selection, read full window text via Accessibility API (text NOT deleted)
    case selectAll         // Accessibility failed, used Cmd+A then Cmd+X/C
}

class AppDelegate: NSObject, NSApplicationDelegate, ObservableObject {
    // Menu bar status item
    private var statusItem: NSStatusItem?

    // Menu item for Settings (stored separately for enable/disable)
    private var settingsMenuItem: NSMenuItem?

    // Rust core instance (internal for access from AetherApp)
    // Published to trigger UI updates when initialized
    @Published internal var core: AetherCore?

    // Event handler for Rust callbacks (internal for toast access)
    internal var eventHandler: EventHandler?

    // Halo overlay window
    private var haloWindow: HaloWindow?

    // Settings window (used by legacy Settings scene and WindowGroup)
    private var settingsWindow: NSWindow?

    // Permission gate window
    private var permissionGateWindow: NSWindow?

    // Permission gate active state
    private var isPermissionGateActive: Bool = false

    // First-time initialization window
    private var initializationWindow: NSWindow?

    // Theme engine (accessible for settings UI)
    var themeEngine: ThemeEngine?

    // Global hotkey monitor (Swift layer)
    private var hotkeyMonitor: GlobalHotkeyMonitor?

    // Command mode hotkey monitor (configurable, default: Cmd+Opt+/)
    private var commandHotkeyMonitor: Any?
    private var commandHotkeyModifiers: NSEvent.ModifierFlags = [.command, .option]
    private var commandHotkeyKeyCode: UInt16 = 44  // "/" key

    // Command mode input listener (captures keyboard input while command mode is active)
    private var commandModeInputMonitor: Any?

    // Typewriter cancellation token
    private var typewriterCancellation: CancellationToken?

    // ESC key monitor for cancelling typewriter
    private var escapeKeyMonitor: Any?

    // Store the frontmost app when hotkey is pressed
    // Used to reactivate the correct app after Halo input mode selection
    private var previousFrontmostApp: NSRunningApplication?

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Hide from Dock (menu bar only)
        NSApp.setActivationPolicy(.accessory)

        // Apply language preference before UI initialization
        applyLanguagePreference()

        // CRITICAL: Set up main menu with Edit menu for keyboard shortcuts (Cmd+V, Cmd+C, etc.)
        // Without this, TextField/TextEditor won't respond to standard editing shortcuts
        setupMainMenu()

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
            let hasAccessibility = PermissionChecker.hasAccessibilityPermission()
            let hasInputMonitoring = PermissionChecker.hasInputMonitoringPermission()

            print("[Aether] Permission status - Accessibility: \(hasAccessibility), InputMonitoring: \(hasInputMonitoring)")

            if !hasAccessibility || !hasInputMonitoring {
                // Show mandatory permission gate if any permission is missing
                self.showPermissionGate()
            } else {
                print("[Aether] ✅ All permissions granted, checking if first-run initialization needed...")

                // Check if this is a fresh installation
                self.checkAndRunFirstTimeInit()
            }
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        // Stop hotkey monitoring
        hotkeyMonitor?.stopMonitoring()

        // Stop clipboard monitoring
        ClipboardMonitor.shared.stopMonitoring()

        // Remove ESC key monitor
        removeEscapeKeyMonitor()

        // Remove command mode hotkey monitor
        removeCommandModeHotkey()

        // Clean up Rust core (only if initialized)
        // Note: No need to call stopListening() as hotkey monitoring is now in Swift
        print("[Aether] Application terminating")
    }

    // MARK: - Main Menu Setup (for Edit shortcuts)

    /// Set up the application's main menu with Edit menu for standard keyboard shortcuts
    ///
    /// This is essential for SwiftUI TextField/TextEditor in accessory apps.
    /// Without the Edit menu, Cmd+V (Paste), Cmd+C (Copy), Cmd+X (Cut) won't work.
    private func setupMainMenu() {
        let mainMenu = NSMenu()

        // App menu (required for macOS app structure)
        let appMenuItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(NSMenuItem(
            title: L("menu.about"),
            action: #selector(showAbout),
            keyEquivalent: ""
        ))
        appMenu.addItem(NSMenuItem.separator())
        appMenu.addItem(NSMenuItem(
            title: L("menu.quit"),
            action: #selector(quit),
            keyEquivalent: "q"
        ))
        appMenuItem.submenu = appMenu
        mainMenu.addItem(appMenuItem)

        // Edit menu (CRITICAL for Cmd+V, Cmd+C, Cmd+X in TextFields)
        let editMenuItem = NSMenuItem()
        let editMenu = NSMenu(title: "Edit")

        // Undo
        let undoItem = NSMenuItem(
            title: L("menu.edit.undo"),
            action: Selector(("undo:")),
            keyEquivalent: "z"
        )
        editMenu.addItem(undoItem)

        // Redo
        let redoItem = NSMenuItem(
            title: L("menu.edit.redo"),
            action: Selector(("redo:")),
            keyEquivalent: "Z"
        )
        redoItem.keyEquivalentModifierMask = [.command, .shift]
        editMenu.addItem(redoItem)

        editMenu.addItem(NSMenuItem.separator())

        // Cut - use generic selector for responder chain to find correct target
        let cutItem = NSMenuItem(
            title: L("menu.edit.cut"),
            action: Selector(("cut:")),
            keyEquivalent: "x"
        )
        editMenu.addItem(cutItem)

        // Copy
        let copyItem = NSMenuItem(
            title: L("menu.edit.copy"),
            action: Selector(("copy:")),
            keyEquivalent: "c"
        )
        editMenu.addItem(copyItem)

        // Paste
        let pasteItem = NSMenuItem(
            title: L("menu.edit.paste"),
            action: Selector(("paste:")),
            keyEquivalent: "v"
        )
        editMenu.addItem(pasteItem)

        // Paste and Match Style (for rich text compatibility)
        let pasteAndMatchStyleItem = NSMenuItem(
            title: L("menu.edit.paste_match_style"),
            action: Selector(("pasteAsPlainText:")),
            keyEquivalent: "V"
        )
        pasteAndMatchStyleItem.keyEquivalentModifierMask = [.command, .option]
        editMenu.addItem(pasteAndMatchStyleItem)

        // Delete
        let deleteItem = NSMenuItem(
            title: L("menu.edit.delete"),
            action: Selector(("delete:")),
            keyEquivalent: ""
        )
        editMenu.addItem(deleteItem)

        editMenu.addItem(NSMenuItem.separator())

        // Select All
        let selectAllItem = NSMenuItem(
            title: L("menu.edit.select_all"),
            action: Selector(("selectAll:")),
            keyEquivalent: "a"
        )
        editMenu.addItem(selectAllItem)

        editMenuItem.submenu = editMenu
        mainMenu.addItem(editMenuItem)

        // Set the main menu
        NSApp.mainMenu = mainMenu

        print("[AppDelegate] ✅ Main menu with Edit shortcuts configured")
    }

    // MARK: - Menu Bar Setup

    private func setupMenuBar() {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)

        if let button = statusItem?.button {
            // Use custom menu bar icon from Assets.xcassets
            if let menuBarIcon = NSImage(named: "MenuBarIcon") {
                menuBarIcon.isTemplate = true
                button.image = menuBarIcon
            } else {
                // Fallback to SF Symbol if custom icon not found
                button.image = NSImage(systemSymbolName: "sparkles", accessibilityDescription: "Aether")
                button.image?.isTemplate = true
            }
        }

        // Create menu
        let menu = NSMenu()

        menu.addItem(NSMenuItem(
            title: L("menu.about"),
            action: #selector(showAbout),
            keyEquivalent: ""
        ))
        menu.addItem(NSMenuItem.separator())

        // Add "Default Provider" submenu (will be populated by rebuildProvidersMenu)
        let defaultProviderMenuItem = NSMenuItem(
            title: L("menu.default_provider"),
            action: nil,
            keyEquivalent: ""
        )
        defaultProviderMenuItem.submenu = NSMenu()  // Create empty submenu for now
        menu.addItem(defaultProviderMenuItem)

        menu.addItem(NSMenuItem.separator())

        // Create and store Settings menu item for enable/disable control
        settingsMenuItem = NSMenuItem(
            title: L("menu.settings"),
            action: #selector(showSettings),
            keyEquivalent: ","
        )
        menu.addItem(settingsMenuItem!)

        // Debug menu items (only in DEBUG builds)
        #if DEBUG
        menu.addItem(NSMenuItem.separator())
        menu.addItem(NSMenuItem(
            title: "Test Clarification (Select)",
            action: #selector(testClarificationSelect),
            keyEquivalent: "d"
        ))
        menu.addItem(NSMenuItem(
            title: "Test Clarification (Text)",
            action: #selector(testClarificationText),
            keyEquivalent: "t"
        ))
        #endif

        menu.addItem(NSMenuItem.separator())
        menu.addItem(NSMenuItem(
            title: L("menu.quit"),
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
        eventHandler?.showToast(
            type: .info,
            title: L("alert.about.title"),
            message: L("alert.about.message", "0.1.0 (Phase 2)"),
            autoDismiss: true
        )
    }

    @objc private func showSettings() {
        // Block settings access if permission gate is active
        if isPermissionGateActive {
            print("[Aether] Settings blocked - permission gate is active")
            return
        }

        // CRITICAL: Check if core is initialized before opening settings
        guard let core = core else {
            print("[Aether] ERROR: Core not initialized, cannot open settings")
            eventHandler?.showToast(
                type: .warning,
                title: L("error.core_not_initialized"),
                message: L("error.core_not_initialized.suggestion"),
                autoDismiss: false
            )
            return
        }

        // GHOST MODE: Stay in accessory mode (no Dock icon)
        // Using NSPanel with proper configuration allows keyboard shortcuts to work
        // without needing to switch to regular activation policy
        print("[AppDelegate] Opening settings panel in GHOST MODE (no Dock icon)")

        // Check if settings window already exists and is valid
        // First check if reference exists and window is still alive (not released)
        if let window = settingsWindow {
            // Safely check if window is still valid before accessing properties
            if window.isVisible {
                // Window exists and is visible, reset to minimum size and bring to front
                window.setContentSize(NSSize(width: 980, height: 750))
                window.center()
                // GHOST MODE: Bring to front without activating app
                window.orderFrontRegardless()
                window.makeKey()
                return
            } else {
                // Window exists but not visible, clean up stale reference
                settingsWindow = nil
            }
        }

        // Create new settings window with RootContentView
        let settingsView = RootContentView(core: core)
            .environmentObject(self)

        let hostingController = NSHostingController(rootView: settingsView)
        hostingController.sizingOptions = []  // Disable auto-sizing

        // GHOST MODE: Use NSPanel instead of NSWindow
        // NSPanel can receive keyboard events even when app is in accessory mode
        // This allows Cmd+V/C/X to work without showing Dock icon
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 980, height: 750),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )

        panel.title = "Settings"
        panel.titlebarAppearsTransparent = true
        panel.titleVisibility = .hidden
        panel.contentViewController = hostingController

        // Set size constraints
        panel.minSize = NSSize(width: 980, height: 750)
        panel.center()

        // GHOST MODE: Always stay on top (floating level)
        panel.level = .floating

        // CRITICAL: Allow panel to become key window for keyboard input
        // This is essential for TextField/TextEditor to receive keystrokes
        panel.becomesKeyOnlyIfNeeded = false

        // Window management
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.delegate = self

        settingsWindow = panel

        // GHOST MODE: Show panel without activating the app (avoids Dock icon)
        // Use orderFrontRegardless() instead of makeKeyAndOrderFront() to avoid activation
        panel.orderFrontRegardless()

        // Make the panel key window for keyboard input, but don't activate the app
        panel.makeKey()
    }

    @objc private func quit() {
        NSApplication.shared.terminate(nil)
    }

    // MARK: - Providers Menu Management (NEW for default provider management)

    /// Generic menu rebuilder to eliminate duplication
    ///
    /// This helper method extracts the common pattern used by `rebuildProvidersMenu()`.
    ///
    /// - Parameters:
    ///   - menuItemTitle: Localized title of the menu item containing the submenu
    ///   - items: Array of (id, displayName) tuples representing menu items
    ///   - currentSelection: Currently selected item ID (for checkmark)
    ///   - action: Selector to invoke when an item is clicked
    ///   - placeholderText: Optional text to show when items array is empty
    private func rebuildMenu(
        menuItemTitle: String,
        items: [(id: String, displayName: String)],
        currentSelection: String?,
        action: Selector,
        placeholderText: String? = nil
    ) {
        guard let menu = statusItem?.menu else { return }

        // Find the menu item
        guard let targetMenuItem = menu.items.first(where: { $0.title == menuItemTitle }),
              let submenu = targetMenuItem.submenu else {
            print("[AppDelegate] ERROR: Menu item '\(menuItemTitle)' submenu not found")
            return
        }

        // Clear existing submenu items
        submenu.removeAllItems()

        if !items.isEmpty {
            // Add menu items for each entry
            for (id, displayName) in items {
                let item = NSMenuItem(
                    title: displayName,
                    action: action,
                    keyEquivalent: ""
                )
                item.representedObject = id

                // Add checkmark if this is the current selection
                if let currentSelection = currentSelection, id == currentSelection {
                    item.state = .on
                } else {
                    item.state = .off
                }

                submenu.addItem(item)
            }

            // Enable the parent menu item
            targetMenuItem.isEnabled = true

            print("[AppDelegate] Rebuilt '\(menuItemTitle)' submenu with \(items.count) items")
        } else {
            // No items available - handle empty state
            if let placeholder = placeholderText {
                let placeholderItem = NSMenuItem(
                    title: placeholder,
                    action: nil,
                    keyEquivalent: ""
                )
                placeholderItem.isEnabled = false
                submenu.addItem(placeholderItem)
            }

            targetMenuItem.isEnabled = false
            print("[AppDelegate] No items for '\(menuItemTitle)', disabling submenu")
        }
    }

    /// Rebuild the providers submenu with enabled providers
    private func rebuildProvidersMenu() {
        guard let core = core else { return }

        // Get enabled providers and current default
        let enabledProviders = core.getEnabledProviders().sorted()
        let defaultProvider = core.getDefaultProvider()

        // Map providers to (id, displayName) tuples
        let items = enabledProviders.map { ($0, $0) }

        // Use generic menu builder
        rebuildMenu(
            menuItemTitle: L("menu.default_provider"),
            items: items,
            currentSelection: defaultProvider,
            action: #selector(selectDefaultProvider(_:)),
            placeholderText: L("menu.no_providers")
        )
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
            eventHandler?.showToast(
                type: .warning,
                title: "Failed to set default provider",
                message: "Could not set '\(providerName)' as default provider.\n\nError: \(error.localizedDescription)",
                autoDismiss: false
            )
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

        // Show Halo animation immediately on startup (better UX feedback)
        // Only show on first attempt to avoid repeated animations during retries
        if coreInitRetryCount == 0 {
            haloWindow?.updateState(.processing(providerColor: .blue, streamingText: nil))
            haloWindow?.showCentered()
            print("[Aether] Showing Halo startup animation")
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

            // IMPORTANT: Initialize the trigger-based hotkey system
            // This uses the two-callback architecture:
            // - Replace hotkey (default: double-tap left Shift) → handleReplaceTriggered()
            // - Append hotkey (default: double-tap right Shift) → handleAppendTriggered()
            print("[Aether] Initializing trigger-based hotkey system...")
            initializeTriggerSystem()

            // Check if monitoring started successfully
            guard hotkeyMonitor != nil else {
                print("[Aether] ❌ Failed to initialize trigger system")
                // Fall back to showing permission gate
                DispatchQueue.main.async { [weak self] in
                    self?.showPermissionGate()
                }
                return
            }

            // Reset retry count on success
            coreInitRetryCount = 0

            // Configure command completion manager with core reference
            haloWindow?.viewModel.commandManager.configure(core: core)
            print("[Aether] CommandCompletionManager configured")

            // Set up command mode hotkey (Cmd+Opt+/)
            setupCommandModeHotkey()

            // Hide startup Halo animation (initialization succeeded)
            // Note: "No providers" error will be shown when user presses hotkey, not at startup
            haloWindow?.hide()
            print("[Aether] Hiding Halo startup animation (init succeeded)")

            // Update menu bar icon to show active state
            updateMenuBarIcon(state: .listening)

            // Rebuild providers menu now that core is initialized
            rebuildProvidersMenu()

        } catch {
            print("[Aether] ❌ Error initializing core: \(error)")

            // Retry with exponential backoff for transient errors (permissions, library loading)
            // Note: "No providers" case is handled separately after successful init
            if coreInitRetryCount < maxRetryAttempts {
                coreInitRetryCount += 1
                let retryDelay = Double(coreInitRetryCount) * 2.0 // 2s, 4s, 6s

                print("[Aether] Retrying initialization in \(retryDelay)s (attempt \(coreInitRetryCount)/\(maxRetryAttempts))")

                DispatchQueue.main.asyncAfter(deadline: .now() + retryDelay) { [weak self] in
                    self?.initializeRustCore()
                }
            } else {
                // Max retries exceeded - show error
                print("[Aether] Max retry attempts exceeded, giving up")

                let errorMessage = "Failed to initialize Aether core.\n\nError: \(error)\n\nPlease check:\n1. Accessibility permissions are granted\n2. Input Monitoring permissions are granted\n3. libaethecore.dylib is properly bundled"

                // Halo is already showing (from start of initializeRustCore)
                // After 0.8s animation, show error toast
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { [weak self] in
                    // Try toast first, fallback to NSAlert if eventHandler not available
                    if let handler = self?.eventHandler {
                        handler.showToast(
                            type: .error,
                            title: L("error.aether"),
                            message: errorMessage,
                            autoDismiss: false
                        )
                    } else {
                        // Fallback: eventHandler not available during early init failure
                        showErrorAlert(title: L("error.aether"), message: errorMessage)
                    }
                }
            }
        }
    }

    private func updateMenuBarIcon(state: ProcessingState) {
        DispatchQueue.main.async { [weak self] in
            guard let button = self?.statusItem?.button else { return }

            switch state {
            case .idle:
                // Use custom menu bar icon for idle state
                if let menuBarIcon = NSImage(named: "MenuBarIcon") {
                    menuBarIcon.isTemplate = true
                    button.image = menuBarIcon
                } else {
                    button.image = NSImage(systemSymbolName: "sparkles", accessibilityDescription: "Aether")
                }
            case .listening:
                // Use custom icon for listening state too
                if let menuBarIcon = NSImage(named: "MenuBarIcon") {
                    menuBarIcon.isTemplate = true
                    button.image = menuBarIcon
                } else {
                    button.image = NSImage(systemSymbolName: "sparkles", accessibilityDescription: "Aether Listening")
                }
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


    // MARK: - Permission Gate Management

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

        // Check if first-time initialization is needed
        checkAndRunFirstTimeInit()
    }

    // MARK: - First-Time Initialization

    /// Check if this is a fresh install and run initialization if needed
    private func checkAndRunFirstTimeInit() {
        do {
            let isFresh = try isFreshInstall()

            if isFresh {
                print("[Aether] 🆕 Fresh installation detected - running first-time initialization...")
                showInitializationWindow()
            } else {
                print("[Aether] Existing installation detected - skipping initialization")
                initializeAppComponents()
            }
        } catch {
            print("[Aether] ❌ Error checking installation status: \(error)")
            print("[Aether] Proceeding with normal initialization anyway")
            initializeAppComponents()
        }
    }

    /// Show the first-time initialization progress window
    private func showInitializationWindow() {
        let initView = InitializationProgressView(
            onCompletion: { [weak self] in
                DispatchQueue.main.async {
                    print("[Aether] Initialization completed - proceeding with app startup")
                    self?.closeInitializationWindow()
                    self?.initializeAppComponents()
                }
            },
            onFailure: { [weak self] error in
                DispatchQueue.main.async {
                    print("[Aether] Initialization failed: \(error)")

                    // Show error alert
                    let alert = NSAlert()
                    alert.messageText = "Initialization Failed"
                    alert.informativeText = "Aether failed to complete first-time initialization.\n\nError: \(error)\n\nPlease check your internet connection and try again."
                    alert.alertStyle = .critical
                    alert.addButton(withTitle: "Quit")
                    alert.addButton(withTitle: "Retry")

                    let response = alert.runModal()
                    if response == .alertFirstButtonReturn {
                        // Quit
                        NSApp.terminate(nil)
                    } else {
                        // Retry
                        self?.showInitializationWindow()
                    }
                }
            }
        )

        let hostingController = NSHostingController(rootView: initView)

        // Create window
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 480, height: 400),
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false
        )

        window.title = "Initializing Aether"
        window.contentViewController = hostingController
        window.center()
        window.level = .floating
        window.isReleasedWhenClosed = false

        initializationWindow = window
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)

        print("[Aether] Initialization window shown")
    }

    /// Close the initialization window
    private func closeInitializationWindow() {
        initializationWindow?.close()
        initializationWindow = nil
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

        // Setup ESC key monitor for cancelling typewriter
        setupEscapeKeyMonitor()

        // Start clipboard monitoring for context tracking
        ClipboardMonitor.shared.startMonitoring()
        print("[Aether] Clipboard monitoring started for context tracking")

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
    ///
    /// Three input modes are supported:
    /// - cut: Directly execute Cmd+X, then show Halo (listening) → AI → replace original
    /// - copy: Directly execute Cmd+C, then show Halo (listening) → AI → append after original
    /// - halo: Show Halo with selection UI → user clicks → then execute based on choice
    private func handleHotkeyPressed() {
        print("[AppDelegate] Hotkey pressed - handling in Swift layer")

        // CRITICAL: Block hotkey if permission gate is active or core not initialized
        // This prevents "noise prompt" and unnecessary clipboard operations
        if isPermissionGateActive {
            print("[AppDelegate] ⚠️ Hotkey blocked - permission gate is active")
            NSSound.beep()
            return
        }

        guard let core = core else {
            print("[AppDelegate] ⚠️ Hotkey blocked - core not initialized")
            NSSound.beep()
            return
        }

        // CRITICAL: Store the current frontmost app BEFORE showing Halo
        // This is essential for Halo mode to correctly reactivate the target app
        previousFrontmostApp = NSWorkspace.shared.frontmostApplication
        print("[AppDelegate] 📱 Stored frontmost app: \(previousFrontmostApp?.localizedName ?? "Unknown")")

        // Load input_mode from config (cut, copy, or halo)
        var inputModeString = "cut"
        do {
            let config = try core.loadConfig()
            if let behavior = config.behavior {
                inputModeString = behavior.inputMode
            }
        } catch {
            print("[AppDelegate] ⚠️ Failed to load config, using default input_mode=cut: \(error)")
        }

        let inputMode = InputMode.from(string: inputModeString)
        print("[AppDelegate] 📋 Input mode from config: \(inputMode.rawValue)")

        // Get best position: caret position (preferred) or mouse position (fallback)
        let haloPosition = CaretPositionHelper.getBestPosition()

        switch inputMode {
        case .cut:
            // Direct cut mode: Show Halo immediately and process with replace
            print("[AppDelegate] Mode: cut - directly executing Cmd+X")
            // CRITICAL: Show Halo SYNCHRONOUSLY before processing to ensure showTime is set
            // This fixes the race condition where error callback fires before Halo is shown
            // Use .processing state to show the theme's processing animation (purple + 3 arcs for Zen theme)
            if Thread.isMainThread {
                haloWindow?.show(at: haloPosition)
                haloWindow?.updateState(.processing(providerColor: .purple, streamingText: nil))
            } else {
                DispatchQueue.main.sync { [weak self] in
                    self?.haloWindow?.show(at: haloPosition)
                    self?.haloWindow?.updateState(.processing(providerColor: .purple, streamingText: nil))
                }
            }
            processWithInputMode(useCutMode: true)

        case .copy:
            // Direct copy mode: Show Halo immediately and process with append
            print("[AppDelegate] Mode: copy - directly executing Cmd+C")
            // CRITICAL: Show Halo SYNCHRONOUSLY before processing to ensure showTime is set
            // Use .processing state to show the theme's processing animation (purple + 3 arcs for Zen theme)
            if Thread.isMainThread {
                haloWindow?.show(at: haloPosition)
                haloWindow?.updateState(.processing(providerColor: .purple, streamingText: nil))
            } else {
                DispatchQueue.main.sync { [weak self] in
                    self?.haloWindow?.show(at: haloPosition)
                    self?.haloWindow?.updateState(.processing(providerColor: .purple, streamingText: nil))
                }
            }
            processWithInputMode(useCutMode: false)
        }
    }

    /// Process input with specified mode (cut = replace original, copy = append to original)
    private func processWithInputMode(useCutMode: Bool) {
        print("[AppDelegate] Processing with cut mode: \(useCutMode)")

        guard core != nil else {
            print("[AppDelegate] ⚠️ Core not initialized")
            // Show error in Halo
            DispatchQueue.main.async { [weak self] in
                self?.haloWindow?.updateState(.error(
                    type: .unknown,
                    message: L("error.core_not_initialized"),
                    suggestion: L("error.core_not_initialized.suggestion")
                ))
            }
            return
        }

        // Halo state is already set to .listening by the caller (handleHotkeyPressed)
        // No need to update it again here

        // CRITICAL: Reactivate the previous frontmost app for keyboard events
        // This is essential when coming from Halo input mode selection
        if let previousApp = previousFrontmostApp,
           previousApp.bundleIdentifier != Bundle.main.bundleIdentifier {
            print("[AppDelegate] 🔄 Reactivating previous app: \(previousApp.localizedName ?? "Unknown")")
            previousApp.activate(options: [])
            Thread.sleep(forTimeInterval: 0.15)  // Give time for activation
        }

        print("[AppDelegate] 📋 Using cut mode: \(useCutMode)")

        // Track where the text came from - this determines output strategy
        var textSource: TextSource = .selectedText

        // CRITICAL: Save original clipboard content to restore later
        // This protects user's pre-existing clipboard data
        let originalClipboardText = ClipboardManager.shared.getText()
        let originalChangeCount = ClipboardManager.shared.changeCount()
        print("[AppDelegate] 💾 Saved original clipboard state (changeCount: \(originalChangeCount))")

        // CRITICAL: Save original clipboard media attachments BEFORE Cut/Copy
        // This preserves images/files that user manually copied to clipboard
        // Without this, simulateCut()/simulateCopy() would overwrite the clipboard
        // and lose the media attachments that user intended to send to AI
        let (_, originalMediaAttachments, _) = ClipboardManager.shared.getMixedContent()
        if !originalMediaAttachments.isEmpty {
            print("[AppDelegate] 📎 Saved \(originalMediaAttachments.count) original media attachment(s) from clipboard")
            for (index, attachment) in originalMediaAttachments.enumerated() {
                print("[AppDelegate]   [\(index + 1)] \(attachment.mediaType)/\(attachment.mimeType) - \(attachment.sizeBytes) bytes")
            }
        }

        // Step 1: Try to cut/copy selected text based on input_mode
        if useCutMode {
            print("[AppDelegate] Simulating Cmd+X to cut selected text...")
            KeyboardSimulator.shared.simulateCut()
        } else {
            print("[AppDelegate] Simulating Cmd+C to copy selected text...")
            KeyboardSimulator.shared.simulateCopy()
        }

        // Wait for clipboard to update (macOS needs a small delay)
        Thread.sleep(forTimeInterval: 0.1)  // 100ms delay

        // Check if clipboard changed (means there was selected text)
        let afterCopyChangeCount = ClipboardManager.shared.changeCount()
        let hasSelectedText = (afterCopyChangeCount != originalChangeCount)

        if !hasSelectedText {
            // Step 2: No selected text detected
            // Try elegant Accessibility API first (silent, no visible selection)
            print("[AppDelegate] ⚠️ No selected text detected, trying Accessibility API to read window text...")

            let accessibilityResult = AccessibilityTextReader.shared.readFocusedText()

            switch accessibilityResult {
            case .success(let text):
                // Successfully read text via Accessibility API!
                // IMPORTANT: Text is NOT deleted from window, just read
                print("[AppDelegate] ✅ Read text via Accessibility API (\(text.count) chars) - completely silent!")
                textSource = .accessibilityAPI  // Mark source as Accessibility API
                // Temporarily set the text to clipboard for processing
                ClipboardManager.shared.setText(text)

            case .noTextContent, .noFocusedElement, .unsupported:
                // Accessibility API couldn't get text, fallback to Cmd+A
                print("[AppDelegate] ⚠️ Accessibility API failed, falling back to Cmd+A method...")
                textSource = .selectAll  // Mark source as select all
                KeyboardSimulator.shared.simulateSelectAll()
                Thread.sleep(forTimeInterval: 0.05)  // 50ms delay

                // Cut/Copy again after selecting all (based on input_mode)
                if useCutMode {
                    KeyboardSimulator.shared.simulateCut()
                } else {
                    KeyboardSimulator.shared.simulateCopy()
                }
                Thread.sleep(forTimeInterval: 0.1)  // 100ms delay

                let afterSelectAllChangeCount = ClipboardManager.shared.changeCount()
                if afterSelectAllChangeCount == afterCopyChangeCount {
                    print("[AppDelegate] ❌ No text content found even after Cmd+A")
                    // Restore original clipboard
                    if let original = originalClipboardText {
                        ClipboardManager.shared.setText(original)
                    }

                    // Show error
                    let errorPosition = CaretPositionHelper.getBestPosition()
                    DispatchQueue.main.async { [weak self] in
                        self?.haloWindow?.show(at: errorPosition)
                        self?.haloWindow?.updateState(.error(
                            type: .unknown,
                            message: L("error.no_text_in_window"),
                            suggestion: L("error.no_text_in_window.suggestion")
                        ))
                        // Auto-hide after 2 seconds
                        DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) { [weak self] in
                            self?.haloWindow?.hide()
                        }
                    }
                    return
                } else {
                    print("[AppDelegate] ✓ Selected all text in current window (via Cmd+A)")
                }

            case .accessibilityDenied:
                // This shouldn't happen as we check permissions at startup
                print("[AppDelegate] ❌ Accessibility permission denied, using Cmd+A fallback")
                textSource = .selectAll
                KeyboardSimulator.shared.simulateSelectAll()
                Thread.sleep(forTimeInterval: 0.05)
                if useCutMode {
                    KeyboardSimulator.shared.simulateCut()
                } else {
                    KeyboardSimulator.shared.simulateCopy()
                }
                Thread.sleep(forTimeInterval: 0.1)

            case .error(let message):
                print("[AppDelegate] ❌ Accessibility error: \(message), using Cmd+A fallback")
                textSource = .selectAll
                KeyboardSimulator.shared.simulateSelectAll()
                Thread.sleep(forTimeInterval: 0.05)
                if useCutMode {
                    KeyboardSimulator.shared.simulateCut()
                } else {
                    KeyboardSimulator.shared.simulateCopy()
                }
                Thread.sleep(forTimeInterval: 0.1)
            }
        } else {
            print("[AppDelegate] ✓ Detected selected text")
            textSource = .selectedText
        }

        print("[AppDelegate] 📍 Text source: \(textSource), Input mode: \(useCutMode ? "replace" : "append")")

        // Get the captured clipboard content (text + media attachments)
        // add-multimodal-content-support: Use getMixedContent() for comprehensive extraction
        let (extractedText, mediaAttachments, extractionError) = ClipboardManager.shared.getMixedContent()

        // Check for extraction errors (e.g., file too large)
        if let error = extractionError {
            print("[AppDelegate] ❌ Content extraction error: \(error)")
            // Restore original clipboard
            if let original = originalClipboardText {
                ClipboardManager.shared.setText(original)
            }
            // Hide Halo and show error toast to user
            DispatchQueue.main.async { [weak self] in
                self?.haloWindow?.hide()
                self?.eventHandler?.showToast(
                    type: .warning,
                    title: L("error.file_size"),
                    message: error,
                    autoDismiss: false
                )
            }
            return
        }

        guard let clipboardText = extractedText else {
            print("[AppDelegate] ❌ Clipboard is empty after copy operation")
            // Restore original clipboard
            if let original = originalClipboardText {
                ClipboardManager.shared.setText(original)
            }
            return
        }

        print("[AppDelegate] Clipboard text: \(clipboardText.prefix(50))...")

        // Log media attachments if present (add-multimodal-content-support)
        // NOTE: mediaAttachments from getMixedContent() after Cut/Copy is usually empty
        // because Cut/Copy overwrites the clipboard. The actual attachments were saved
        // in originalMediaAttachments BEFORE the Cut/Copy operation.
        if !mediaAttachments.isEmpty {
            print("[AppDelegate] 📎 Extracted \(mediaAttachments.count) media attachment(s) from current clipboard:")
            for (index, attachment) in mediaAttachments.enumerated() {
                print("[AppDelegate]   [\(index + 1)] \(attachment.mediaType)/\(attachment.mimeType) - \(attachment.sizeBytes) bytes")
            }
        }

        // CRITICAL FIX: Merge attachments in correct order
        // Data order rule: Window text + Clipboard text/attachment + Window attachment
        //
        // - originalMediaAttachments: Clipboard attachments (saved BEFORE Cut/Copy)
        // - mediaAttachments: Window attachments (from Cut/Copy operation)
        //
        // Final order: Clipboard attachments first, then Window attachments
        // This keeps text content (from clipboardContext) logically before window attachments
        var finalMediaAttachments: [MediaAttachment] = []

        // 1. Add clipboard attachments first (user's copied context)
        if !originalMediaAttachments.isEmpty {
            finalMediaAttachments.append(contentsOf: originalMediaAttachments)
            print("[AppDelegate] 📎 Added \(originalMediaAttachments.count) clipboard attachment(s)")
        }

        // 2. Add window attachments (from Cut/Copy of window content)
        if !mediaAttachments.isEmpty {
            finalMediaAttachments.append(contentsOf: mediaAttachments)
            print("[AppDelegate] 📎 Added \(mediaAttachments.count) window attachment(s)")
        }

        if !finalMediaAttachments.isEmpty {
            print("[AppDelegate] 📎 Total: \(finalMediaAttachments.count) attachment(s) (clipboard: \(originalMediaAttachments.count), window: \(mediaAttachments.count))")
        }

        // IMPORTANT: Check for recent clipboard content (within 10 seconds)
        // This allows us to use previous clipboard as additional context
        let recentClipboardContent = ClipboardMonitor.shared.getRecentClipboardContent()
        let clipboardContext: String? = {
            // Only use clipboard as context if:
            // 1. We have recent clipboard content
            // 2. It's different from current text
            // 3. It's not empty
            guard let recentContent = recentClipboardContent,
                  !recentContent.isEmpty,
                  recentContent != clipboardText else {
                return nil
            }
            return recentContent
        }()

        if let context = clipboardContext {
            print("[AppDelegate] 📋 Found clipboard context (\(context.count) chars, within 10s)")
        } else {
            print("[AppDelegate] No clipboard context to use")
        }

        // Capture current window context
        let windowContext = ContextCapture.captureContext()
        print("[AppDelegate] Context: app=\(windowContext.bundleId ?? "unknown"), window=\(windowContext.windowTitle ?? "nil")")

        // Update Halo to processing state (Halo is already shown from handleHotkeyPressed)
        // Use .processing state to show the theme's processing animation (purple + 3 arcs for Zen theme)
        DispatchQueue.main.async { [weak self] in
            self?.haloWindow?.updateState(.processing(providerColor: .purple, streamingText: nil))
        }

        // Process input asynchronously to avoid blocking UI
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self = self else { return }

            do {
                // Create captured context for Rust (add-multimodal-content-support)
                // Include media attachments if present
                // CRITICAL: Use finalMediaAttachments which preserves images from BEFORE Cut/Copy
                let capturedContext = CapturedContext(
                    appBundleId: windowContext.bundleId ?? "unknown",
                    windowTitle: windowContext.windowTitle,
                    attachments: finalMediaAttachments.isEmpty ? nil : finalMediaAttachments
                )

                // CRITICAL: Construct user input - clipboard content appended after window content
                // Format: Window content first (may contain command like /en), then clipboard content
                // This ensures routing rules like "^/en" can match the command prefix
                let userInput: String
                if let clipContext = clipboardContext {
                    userInput = "\(clipboardText)\n\n\(clipContext)"
                    print("[AppDelegate] 🤖 Sending to AI: window (\(clipboardText.count) chars) + clipboard (\(clipContext.count) chars)")
                } else {
                    userInput = clipboardText
                    print("[AppDelegate] 🤖 Sending to AI: window text only (\(clipboardText.count) chars)")
                }

                // Call Rust core's process_input() - this replaces the old onHotkeyDetected flow
                // The method will handle: Memory retrieval → AI routing → Provider call → Memory storage
                // It returns the AI response text which we need to output using KeyboardSimulator
                guard let core = self.core else {
                    print("[AppDelegate] ERROR: Core became nil during processing")
                    return
                }
                let response = try core.processInput(
                    userInput: userInput,
                    context: capturedContext
                )

                print("[AppDelegate] Received AI response (\(response.count) chars)")

                // CRITICAL: Limit response length to prevent infinite output
                let maxResponseLength = 5000  // Max 5000 characters
                let truncatedResponse: String
                if response.count > maxResponseLength {
                    print("[AppDelegate] ⚠ Response too long (\(response.count) chars), truncating to \(maxResponseLength)")
                    truncatedResponse = String(response.prefix(maxResponseLength)) + "\n\n[... response truncated due to length limit ...]"
                } else {
                    truncatedResponse = response
                }

                // Load output mode from config (typewriter or instant)
                var outputMode = "instant"  // Default to instant if config fails
                var typingSpeed: Int = 50   // Default typing speed (chars/sec)
                do {
                    let config = try core.loadConfig()
                    if let behavior = config.behavior {
                        outputMode = behavior.outputMode
                        typingSpeed = Int(behavior.typingSpeed)
                    }
                    print("[AppDelegate] 📋 Output mode from config: \(outputMode), typing speed: \(typingSpeed) chars/sec")
                } catch {
                    print("[AppDelegate] ⚠️ Failed to load config, using default output_mode=instant: \(error)")
                }

                // Output AI response based on configured mode
                DispatchQueue.main.async { [weak self] in
                    guard let self = self else { return }

                    print("[AppDelegate] 🎯 Starting output phase...")
                    print("[AppDelegate] 📍 Text source: \(textSource), Replace mode: \(useCutMode)")
                    print("[AppDelegate] 📍 Output mode: \(outputMode)")

                    // CRITICAL: Add small delay to ensure UI is stable before keyboard simulation
                    // This helps when focus might have shifted during AI processing
                    Thread.sleep(forTimeInterval: 0.1)

                    // CRITICAL: Prepare cursor position based on text source and input mode
                    // This ensures AI response is placed correctly (replace vs append)
                    self.prepareOutputPosition(textSource: textSource, useCutMode: useCutMode)

                    // Small delay after cursor positioning
                    Thread.sleep(forTimeInterval: 0.05)

                    if outputMode == "typewriter" {
                        // Typewriter mode: Type character by character
                        print("[AppDelegate] ⌨️ Using typewriter mode at \(typingSpeed) chars/sec")

                        // Create cancellation token for ESC key to cancel typewriter
                        self.typewriterCancellation = CancellationToken()

                        // Hide Halo during typewriting (keyboard icon distracted users)
                        self.haloWindow?.hide()

                        Task {
                            let typedCount = await KeyboardSimulator.shared.typeText(
                                truncatedResponse,
                                speed: typingSpeed,
                                cancellationToken: self.typewriterCancellation
                            )
                            print("[AppDelegate] ⌨️ Typed \(typedCount)/\(truncatedResponse.count) characters")

                            // Clear cancellation token after completion
                            self.typewriterCancellation = nil

                            await MainActor.run {
                                // Show Halo again and update to success state
                                print("[AppDelegate] ✅ Output complete, showing success state")
                                self.haloWindow?.showAtCurrentPosition()
                                self.haloWindow?.updateState(.success(finalText: String(truncatedResponse.prefix(100))))

                                // Auto-hide Halo after 1.5 seconds
                                DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in
                                    self?.haloWindow?.hide()
                                }
                            }
                        }
                    } else {
                        // Instant mode: Use paste for reliable output
                        print("[AppDelegate] 📋 Using instant mode (paste)")
                        ClipboardManager.shared.setText(truncatedResponse)

                        // Small delay to ensure clipboard is updated
                        Thread.sleep(forTimeInterval: 0.05)

                        // Simulate paste
                        print("[AppDelegate] 📋 Simulating Cmd+V to paste response")
                        let pasteSuccess = KeyboardSimulator.shared.simulatePaste()
                        print("[AppDelegate] 📋 Paste result: \(pasteSuccess ? "success" : "failed")")

                        // CRITICAL: Restore original clipboard after a delay
                        // This ensures user's pre-existing clipboard data is not lost
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                            if let original = originalClipboardText {
                                ClipboardManager.shared.setText(original)
                                print("[AppDelegate] ♻️ Restored original clipboard content")
                            } else {
                                ClipboardManager.shared.clear()
                                print("[AppDelegate] ♻️ Cleared clipboard (original was empty)")
                            }
                        }

                        // Update Halo to success state and hide
                        print("[AppDelegate] ✅ Output complete, updating Halo to success state")
                        self.haloWindow?.updateState(.success(finalText: String(truncatedResponse.prefix(100))))

                        // Auto-hide Halo after 1.5 seconds
                        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in
                            self?.haloWindow?.hide()
                        }
                    }
                }
            } catch {
                print("[AppDelegate] ❌ Error processing input: \(error)")

                // CRITICAL: Clear clipboard monitor history to prevent error messages from being used as context
                ClipboardMonitor.shared.clearHistory()
                print("[AppDelegate] 🗑️ Cleared clipboard monitor history after error")

                // CRITICAL: Restore original clipboard on error
                DispatchQueue.main.async {
                    if let original = originalClipboardText {
                        ClipboardManager.shared.setText(original)
                        print("[AppDelegate] ♻️ Restored original clipboard content (after AI error)")
                    }
                }

                // For AetherException, the error details have already been sent via callback
                // in Rust before throwing the exception. We just need to log it here.
                if error is AetherException {
                    print("[AppDelegate] AetherException caught - error details already sent via callback")
                    // Error already displayed via EventHandler.onError callback from Rust
                    // No need to show alert again
                } else {
                    // For non-Rust errors (e.g., Swift KeyboardSimulator errors)
                    let errorMessage = error.localizedDescription
                    let nsError = error as NSError
                    let suggestion = nsError.userInfo["suggestion"] as? String

                    DispatchQueue.main.async { [weak self] in
                        self?.eventHandler?.onError(
                            message: errorMessage,
                            suggestion: suggestion ?? L("error.check_connection")
                        )
                    }
                }
            }
        }
    }

    // MARK: - Output Preparation

    /// Prepare cursor position before outputting AI response
    ///
    /// This method ensures the cursor is in the correct position based on:
    /// - Text source: Where the input text came from
    /// - Input mode: Whether user wants to replace or append
    ///
    /// | Text Source      | Replace Mode      | Append Mode             |
    /// |------------------|-------------------|-------------------------|
    /// | selectedText     | No action needed  | Move to selection end   |
    /// | accessibilityAPI | Cmd+A to select   | Cmd+Down to move to end |
    /// | selectAll        | No action needed  | Cmd+Down to move to end |
    private func prepareOutputPosition(textSource: TextSource, useCutMode: Bool) {
        print("[AppDelegate] 🎯 Preparing output position: source=\(textSource), replace=\(useCutMode)")

        switch (textSource, useCutMode) {
        case (.selectedText, true):
            // Replace selected text: Cursor is already at the right position after Cmd+X
            print("[AppDelegate] ➡️ selectedText + replace: No preparation needed")

        case (.selectedText, false):
            // Append after selected text: Move cursor to end of selection
            // After Cmd+C, the selection is still active, pressing Right arrow moves to end
            print("[AppDelegate] ➡️ selectedText + append: Moving to end of selection")
            KeyboardSimulator.shared.simulateKeyPress(.rightArrow)
            Thread.sleep(forTimeInterval: 0.05)

        case (.accessibilityAPI, true):
            // Replace full window text: Need to select all first, then typing will replace
            // Because Accessibility API only read the text, didn't delete it
            print("[AppDelegate] ➡️ accessibilityAPI + replace: Selecting all to replace")
            KeyboardSimulator.shared.simulateSelectAll()
            Thread.sleep(forTimeInterval: 0.05)

        case (.accessibilityAPI, false):
            // Append to full window text: Move cursor to end of document
            print("[AppDelegate] ➡️ accessibilityAPI + append: Moving to end of document")
            KeyboardSimulator.shared.simulateMoveToEnd()
            Thread.sleep(forTimeInterval: 0.05)

        case (.selectAll, true):
            // Replace after Cmd+A + Cmd+X: Cursor is already at the right position
            print("[AppDelegate] ➡️ selectAll + replace: No preparation needed")

        case (.selectAll, false):
            // Append after Cmd+A + Cmd+C: Move cursor to end of document
            print("[AppDelegate] ➡️ selectAll + append: Moving to end of document")
            KeyboardSimulator.shared.simulateMoveToEnd()
            Thread.sleep(forTimeInterval: 0.05)
        }
    }

    // MARK: - ESC Key Monitoring for Typewriter Cancellation

    /// Setup global ESC key monitor to cancel typewriter animation
    private func setupEscapeKeyMonitor() {
        escapeKeyMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
            // Check if ESC key was pressed (keyCode 53)
            if event.keyCode == 53 {
                self?.handleEscapeKey()
            }
        }
        print("[AppDelegate] ESC key monitor installed for typewriter cancellation")
    }

    /// Remove ESC key monitor
    private func removeEscapeKeyMonitor() {
        if let monitor = escapeKeyMonitor {
            NSEvent.removeMonitor(monitor)
            escapeKeyMonitor = nil
            print("[AppDelegate] ESC key monitor removed")
        }
    }

    /// Handle ESC key press - cancel typewriter animation
    private func handleEscapeKey() {
        guard let cancellation = typewriterCancellation else {
            // Check if in command mode - ESC should dismiss it
            if let haloWindow = haloWindow, case .commandMode = haloWindow.viewModel.state {
                print("[AppDelegate] ESC pressed - dismissing command mode")
                haloWindow.viewModel.commandManager.deactivateCommandMode()
                haloWindow.updateState(.idle)
                haloWindow.hide()
                return
            }
            print("[AppDelegate] ESC pressed but no typewriter is running")
            return
        }

        print("[AppDelegate] ESC pressed - cancelling typewriter animation")
        cancellation.cancel()

        // Clear the cancellation token immediately
        typewriterCancellation = nil

        // Show brief feedback
        DispatchQueue.main.async { [weak self] in
            self?.haloWindow?.updateState(.success(finalText: "⏸ Typewriter cancelled"))
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) { [weak self] in
                self?.haloWindow?.hide()
            }
        }
    }

    // MARK: - Command Mode Hotkey (add-command-completion-system)

    /// Setup global hotkey for command mode (configurable, default: Cmd+Opt+/)
    private func setupCommandModeHotkey() {
        // Load command prompt hotkey from config
        loadCommandPromptConfig()

        commandHotkeyMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self = self else { return }
            // Check for configured hotkey
            var modifiersMatch = true
            for modifier in [NSEvent.ModifierFlags.command, .option, .control, .shift] {
                if self.commandHotkeyModifiers.contains(modifier) {
                    if !event.modifierFlags.contains(modifier) {
                        modifiersMatch = false
                        break
                    }
                }
            }
            if modifiersMatch && event.keyCode == self.commandHotkeyKeyCode {
                self.handleCommandModeHotkey()
            }
        }
        print("[AppDelegate] Command mode hotkey monitor installed (keyCode: \(commandHotkeyKeyCode), modifiers: \(commandHotkeyModifiers))")
    }

    /// Load command prompt hotkey configuration from config
    private func loadCommandPromptConfig() {
        guard let core = core else { return }

        do {
            let config = try core.loadConfig()
            if let shortcuts = config.shortcuts {
                parseAndApplyCommandPromptHotkey(shortcuts.commandPrompt)
            }
        } catch {
            print("[AppDelegate] Failed to load command prompt config: \(error)")
        }
    }

    /// Parse command prompt config string (e.g., "Command+Option+/") and apply it
    private func parseAndApplyCommandPromptHotkey(_ configString: String) {
        let parts = configString.split(separator: "+").map { String($0) }
        guard parts.count == 3 else {
            print("[AppDelegate] Invalid command prompt config: \(configString)")
            return
        }

        var modifiers: NSEvent.ModifierFlags = []

        // Parse first two parts as modifiers
        for i in 0..<2 {
            switch parts[i] {
            case "Command": modifiers.insert(.command)
            case "Option": modifiers.insert(.option)
            case "Control": modifiers.insert(.control)
            case "Shift": modifiers.insert(.shift)
            default: break
            }
        }

        // Parse third part as key code
        let keyCode: UInt16
        switch parts[2] {
        case "/": keyCode = 44
        case "`": keyCode = 50
        case "\\": keyCode = 42
        case ";": keyCode = 41
        case ",": keyCode = 43
        case ".": keyCode = 47
        case "Space": keyCode = 49
        default: keyCode = 44  // Default to /
        }

        commandHotkeyModifiers = modifiers
        commandHotkeyKeyCode = keyCode
        print("[AppDelegate] Command prompt hotkey configured: \(configString) (keyCode: \(keyCode), modifiers: \(modifiers))")
    }

    /// Update command prompt hotkey at runtime (called from ShortcutsView)
    func updateCommandPromptHotkey(_ shortcuts: ShortcutsConfig) {
        parseAndApplyCommandPromptHotkey(shortcuts.commandPrompt)

        // Reinstall the monitor with new settings
        removeCommandModeHotkey()
        commandHotkeyMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self = self else { return }
            var modifiersMatch = true
            for modifier in [NSEvent.ModifierFlags.command, .option, .control, .shift] {
                if self.commandHotkeyModifiers.contains(modifier) {
                    if !event.modifierFlags.contains(modifier) {
                        modifiersMatch = false
                        break
                    }
                }
            }
            if modifiersMatch && event.keyCode == self.commandHotkeyKeyCode {
                self.handleCommandModeHotkey()
            }
        }
        print("[AppDelegate] Command prompt hotkey updated and monitor reinstalled")
    }

    /// Remove command mode hotkey monitor
    private func removeCommandModeHotkey() {
        if let monitor = commandHotkeyMonitor {
            NSEvent.removeMonitor(monitor)
            commandHotkeyMonitor = nil
            print("[AppDelegate] Command mode hotkey monitor removed")
        }
    }

    /// Handle command mode hotkey (Cmd+Opt+/)
    private func handleCommandModeHotkey() {
        print("[AppDelegate] Command mode hotkey pressed")

        guard let haloWindow = haloWindow else {
            print("[AppDelegate] ❌ HaloWindow not available")
            return
        }

        // If already in command mode, toggle off
        if case .commandMode = haloWindow.viewModel.state {
            exitCommandMode()
            return
        }

        // Get best position: caret position (preferred) or mouse position (fallback)
        let haloPosition = CaretPositionHelper.getBestPosition()
        print("[AppDelegate] Command mode - haloPosition: (\(haloPosition.x), \(haloPosition.y))")

        // Type "/" character to the active application
        print("[AppDelegate] Typing '/' to active application")
        _ = KeyboardSimulator.shared.typeTextInstant("/")
        usleep(30_000) // 30ms delay

        // Activate command mode
        haloWindow.viewModel.commandManager.activateCommandMode { [weak self] selectedCommand in
            // When user selects a command, complete the input
            self?.handleCommandSelected(selectedCommand)
        }

        // CRITICAL: Set state directly (without animation) BEFORE showBelow
        // This ensures getWindowSize() returns the correct size for command mode
        // We bypass updateState() to avoid animation conflicts with showBelow()
        haloWindow.viewModel.state = .commandMode
        haloWindow.ignoresMouseEvents = false  // Enable mouse events for clicking commands

        // Show halo BELOW the caret (like IDE autocomplete)
        haloWindow.showBelow(at: haloPosition)

        // Start keyboard input listener for command mode
        startCommandModeInputListener()
    }

    /// Start listening for keyboard input during command mode
    private func startCommandModeInputListener() {
        // Remove existing monitor if any
        stopCommandModeInputListener()

        print("[AppDelegate] Starting command mode input listener")

        // Monitor global keyboard events
        commandModeInputMonitor = NSEvent.addGlobalMonitorForEvents(matching: [.keyDown]) { [weak self] event in
            self?.handleCommandModeKeyEvent(event)
        }
    }

    /// Stop listening for keyboard input
    private func stopCommandModeInputListener() {
        if let monitor = commandModeInputMonitor {
            NSEvent.removeMonitor(monitor)
            commandModeInputMonitor = nil
            print("[AppDelegate] Stopped command mode input listener")
        }
    }

    /// Handle keyboard event during command mode
    private func handleCommandModeKeyEvent(_ event: NSEvent) {
        guard let haloWindow = haloWindow,
              case .commandMode = haloWindow.viewModel.state else {
            return
        }

        let commandManager = haloWindow.viewModel.commandManager
        let keyCode = event.keyCode

        // Handle special keys
        switch Int(keyCode) {
        case kVK_Escape:
            // Exit command mode
            print("[AppDelegate] Escape pressed, exiting command mode")
            exitCommandMode()
            return

        case kVK_Return:
            // Select current command
            print("[AppDelegate] Enter pressed, selecting current command")
            commandManager.selectCurrentCommand()
            return

        case kVK_UpArrow:
            // Move selection up
            commandManager.moveSelectionUp()
            return

        case kVK_DownArrow:
            // Move selection down
            commandManager.moveSelectionDown()
            return

        case kVK_Delete:
            // Backspace - remove last character from prefix
            var prefix = commandManager.inputPrefix
            if !prefix.isEmpty {
                prefix.removeLast()
                commandManager.inputPrefix = prefix
                print("[AppDelegate] Backspace, prefix now: '\(prefix)'")
            } else {
                // If prefix is empty and backspace, exit command mode
                print("[AppDelegate] Backspace on empty prefix, exiting command mode")
                exitCommandMode()
            }
            return

        case kVK_Tab:
            // Tab could auto-complete to first match
            if let firstCommand = commandManager.displayedCommands.first {
                commandManager.inputPrefix = firstCommand.key
            }
            return

        default:
            break
        }

        // Handle character input
        if let characters = event.charactersIgnoringModifiers, !characters.isEmpty {
            let char = characters.first!

            // Only accept alphanumeric and common command characters
            if char.isLetter || char.isNumber || char == "-" || char == "_" {
                let newPrefix = commandManager.inputPrefix + String(char)
                commandManager.inputPrefix = newPrefix
                print("[AppDelegate] Character input: '\(char)', prefix now: '\(newPrefix)'")
            }
        }
    }

    /// Exit command mode and clean up
    private func exitCommandMode() {
        print("[AppDelegate] Exiting command mode")

        // Stop input listener first
        stopCommandModeInputListener()

        // Deactivate command manager
        haloWindow?.viewModel.commandManager.deactivateCommandMode()

        // Hide Halo
        haloWindow?.updateState(.idle)
        haloWindow?.hide()
    }

    /// Handle command selection from command completion
    private func handleCommandSelected(_ command: CommandNode) {
        print("[AppDelegate] Command selected: /\(command.key)")

        // Get the current input prefix (what user has typed so far, without the "/")
        let inputPrefix = haloWindow?.viewModel.commandManager.inputPrefix ?? ""

        // Stop input listener first
        stopCommandModeInputListener()

        // CRITICAL: Wait for Enter key event to be fully processed by the target app.
        usleep(100_000) // 100ms delay

        // NO-FLASH APPROACH: Use Accessibility API to read text and find "/" position.
        // Then use backspaces to delete exactly the right amount - no visual selection.

        var charsToDelete = 1 + inputPrefix.count  // Default: "/" + inputPrefix

        // Try to get text content via Accessibility API
        if let textBeforeCursor = getTextBeforeCursor(maxChars: charsToDelete + 5) {
            NSLog("[AppDelegate] DEBUG: Text before cursor: '%@'", textBeforeCursor)

            // Find "/" position from the end (rightmost "/")
            if let slashRange = textBeforeCursor.range(of: "/", options: .backwards) {
                let slashIndex = textBeforeCursor.distance(from: textBeforeCursor.startIndex, to: slashRange.lowerBound)
                charsToDelete = textBeforeCursor.count - slashIndex  // From "/" to end
                NSLog("[AppDelegate] DEBUG: Found '/' at index %d, will delete %d chars", slashIndex, charsToDelete)
            }
        } else {
            NSLog("[AppDelegate] DEBUG: Could not read text via Accessibility, using default count: %d", charsToDelete)
        }

        // Delete using backspaces (no visual selection)
        NSLog("[AppDelegate] Deleting %d characters with backspaces", charsToDelete)
        _ = KeyboardSimulator.shared.typeBackspaces(count: charsToDelete)
        usleep(50_000)

        // Type the complete command
        let commandText = "/\(command.key) "
        NSLog("[AppDelegate] Typing command: '%@'", commandText)
        _ = KeyboardSimulator.shared.typeTextInstant(commandText)

        // Note: deactivateCommandMode() will be called by selectCurrentCommand() after this callback returns

        // Hide Halo immediately (no success feedback needed since command is typed directly)
        haloWindow?.updateState(.idle)
        haloWindow?.hide()
    }

    /// Get text before cursor using Accessibility API (no visual selection)
    ///
    /// - Parameter maxChars: Maximum number of characters to retrieve
    /// - Returns: Text before cursor, or nil if unavailable
    private func getTextBeforeCursor(maxChars: Int) -> String? {
        let systemWide = AXUIElementCreateSystemWide()

        // Get focused element
        var focusedRef: CFTypeRef?
        guard AXUIElementCopyAttributeValue(systemWide, kAXFocusedUIElementAttribute as CFString, &focusedRef) == .success,
              let focused = focusedRef else {
            return nil
        }

        let element = focused as! AXUIElement

        // Get selected text range (cursor position)
        var rangeRef: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, kAXSelectedTextRangeAttribute as CFString, &rangeRef) == .success,
              let rangeValue = rangeRef else {
            return nil
        }

        // Extract range
        var range = CFRange(location: 0, length: 0)
        guard AXValueGetValue(rangeValue as! AXValue, .cfRange, &range) else {
            return nil
        }

        // Calculate range for text before cursor
        let cursorPosition = range.location
        let startPosition = max(0, cursorPosition - maxChars)
        let length = cursorPosition - startPosition

        guard length > 0 else {
            return ""
        }

        // Create range for text before cursor
        var textRange = CFRange(location: startPosition, length: length)
        guard let textRangeValue = AXValueCreate(.cfRange, &textRange) else {
            return nil
        }

        // Get text for range
        var textRef: CFTypeRef?
        guard AXUIElementCopyParameterizedAttributeValue(
            element,
            kAXStringForRangeParameterizedAttribute as CFString,
            textRangeValue,
            &textRef
        ) == .success,
              let text = textRef as? String else {
            return nil
        }

        return text
    }

    // MARK: - Trigger System Configuration

    /// Load trigger configuration from Rust Core
    /// - Returns: TriggerConfig with mode, cut/copy hotkeys
    private func loadTriggerConfiguration() -> TriggerConfig {
        guard let core = core else {
            print("[AppDelegate] Core not initialized, using default TriggerConfig")
            return TriggerConfig.defaultConfig
        }

        do {
            let config = try core.loadConfig()
            if let trigger = config.trigger {
                print("[AppDelegate] Loaded TriggerConfig: replace=\(trigger.replaceHotkey), append=\(trigger.appendHotkey)")
                return trigger
            }
        } catch {
            print("[AppDelegate] Failed to load TriggerConfig: \(error)")
        }

        return TriggerConfig.defaultConfig
    }

    /// Initialize the trigger-based hotkey system (new architecture)
    ///
    /// Uses two callbacks for Replace and Append hotkeys:
    /// - onReplaceTriggered: Double-tap replace key (default: left Shift) - AI replaces original text
    /// - onAppendTriggered: Double-tap append key (default: right Shift) - AI appends after original text
    private func initializeTriggerSystem() {
        let triggerConfig = loadTriggerConfiguration()

        // Extract Swift types from TriggerConfig
        let replaceKey = triggerConfig.replaceKey
        let appendKey = triggerConfig.appendKey

        print("[AppDelegate] Initializing trigger system: replace=\(replaceKey.rawValue), append=\(appendKey.rawValue)")

        // Create GlobalHotkeyMonitor with Replace/Append callbacks
        hotkeyMonitor = GlobalHotkeyMonitor(
            replaceKey: replaceKey,
            appendKey: appendKey,
            onReplaceTriggered: { [weak self] in
                self?.handleReplaceTriggered()
            },
            onAppendTriggered: { [weak self] in
                self?.handleAppendTriggered()
            }
        )

        // Start monitoring
        if hotkeyMonitor?.startMonitoring() == true {
            print("[AppDelegate] ✅ Trigger system started")
            print("[AppDelegate]   Replace: \(replaceKey.shortDisplayName), Append: \(appendKey.shortDisplayName)")
        } else {
            print("[AppDelegate] ❌ Failed to start trigger system")
        }
    }

    /// Update trigger configuration at runtime
    func updateTriggerConfiguration(_ triggerConfig: TriggerConfig) {
        let replaceKey = triggerConfig.replaceKey
        let appendKey = triggerConfig.appendKey

        hotkeyMonitor?.configureTrigger(replaceKey: replaceKey, appendKey: appendKey)

        print("[AppDelegate] Trigger config updated: replace=\(replaceKey.rawValue), append=\(appendKey.rawValue)")
    }

    // MARK: - Trigger Handlers (New Architecture)

    /// Handle Replace trigger (double-tap replace hotkey, default: left Shift)
    ///
    /// AI response replaces the original selected text.
    private func handleReplaceTriggered() {
        print("[AppDelegate] 🔄 Replace triggered")

        // Block if permission gate is active or core not initialized
        guard !isPermissionGateActive, core != nil else {
            print("[AppDelegate] ⚠️ Replace blocked - permission gate or core not ready")
            NSSound.beep()
            return
        }

        // Store frontmost app
        previousFrontmostApp = NSWorkspace.shared.frontmostApplication
        print("[AppDelegate] 📱 Stored frontmost app: \(previousFrontmostApp?.localizedName ?? "Unknown")")

        // Get best position for Halo
        let haloPosition = CaretPositionHelper.getBestPosition()

        // Show Halo immediately with processing state
        if Thread.isMainThread {
            haloWindow?.show(at: haloPosition)
            haloWindow?.updateState(.processing(providerColor: .purple, streamingText: nil))
        } else {
            DispatchQueue.main.sync { [weak self] in
                self?.haloWindow?.show(at: haloPosition)
                self?.haloWindow?.updateState(.processing(providerColor: .purple, streamingText: nil))
            }
        }

        // Process with replace mode (AI response replaces original text)
        processWithInputMode(useCutMode: true)
    }

    /// Handle Append trigger (double-tap append hotkey, default: right Shift)
    ///
    /// AI response appends after the original selected text.
    private func handleAppendTriggered() {
        print("[AppDelegate] ➕ Append triggered")

        // Block if permission gate is active or core not initialized
        guard !isPermissionGateActive, core != nil else {
            print("[AppDelegate] ⚠️ Append blocked - permission gate or core not ready")
            NSSound.beep()
            return
        }

        // Store frontmost app
        previousFrontmostApp = NSWorkspace.shared.frontmostApplication
        print("[AppDelegate] 📱 Stored frontmost app: \(previousFrontmostApp?.localizedName ?? "Unknown")")

        // Get best position for Halo
        let haloPosition = CaretPositionHelper.getBestPosition()

        // Show Halo immediately with processing state
        if Thread.isMainThread {
            haloWindow?.show(at: haloPosition)
            haloWindow?.updateState(.processing(providerColor: .purple, streamingText: nil))
        } else {
            DispatchQueue.main.sync { [weak self] in
                self?.haloWindow?.show(at: haloPosition)
                self?.haloWindow?.updateState(.processing(providerColor: .purple, streamingText: nil))
            }
        }

        // Process with append mode (AI response appends after original text)
        processWithInputMode(useCutMode: false)
    }

    // MARK: - Language Preference

    /// Initialize localization system on app launch
    ///
    /// This initializes the LocalizationManager which:
    /// 1. If user has manually set language in config → Use user's choice
    /// 2. If no config exists or no language set → Use system language
    /// 3. If system language not supported → Fallback to English
    private func applyLanguagePreference() {
        // Initialize LocalizationManager - this determines the correct language
        // and loads the appropriate bundle
        _ = LocalizationManager.shared
        print("[AppDelegate] ✅ LocalizationManager initialized with language: \(LocalizationManager.shared.currentLanguage)")
    }

    // MARK: - Application Lifecycle

    /// Prevent app from terminating when last window closes
    /// This is essential for menu bar apps - they should keep running with no windows open
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        return false
    }

    // MARK: - Debug: Clarification Testing

    #if DEBUG
    /// Test select-type clarification UI
    @objc private func testClarificationSelect() {
        print("[Debug] Testing select-type clarification...")

        let request = ClarificationRequest(
            id: "test-style",
            prompt: "What style would you like?",
            clarificationType: .select,
            options: [
                ClarificationOption(label: "Professional", value: "professional", description: "Formal business tone"),
                ClarificationOption(label: "Casual", value: "casual", description: "Friendly and relaxed"),
                ClarificationOption(label: "Humorous", value: "humorous", description: "Light and playful"),
            ],
            defaultValue: "0",
            placeholder: nil,
            source: "skill:refine-text"
        )

        // Trigger via notification (same as Rust core would do)
        NotificationCenter.default.post(
            name: .clarificationRequested,
            object: request
        )
    }

    /// Test text-type clarification UI
    @objc private func testClarificationText() {
        print("[Debug] Testing text-type clarification...")

        let request = ClarificationRequest(
            id: "test-language",
            prompt: "Enter target language:",
            clarificationType: .text,
            options: nil,
            defaultValue: nil,
            placeholder: "e.g., Spanish, French...",
            source: "skill:translate"
        )

        NotificationCenter.default.post(
            name: .clarificationRequested,
            object: request
        )
    }
    #endif
}

// MARK: - NSWindowDelegate Extension

extension AppDelegate: NSWindowDelegate {
    /// Called when settings window is about to close
    /// Clear the window reference (GHOST MODE: no policy switching needed)
    func windowWillClose(_ notification: Notification) {
        if let window = notification.object as? NSWindow, window == settingsWindow {
            print("[AppDelegate] Settings panel closing, clearing reference")
            settingsWindow = nil
            // GHOST MODE: We stay in accessory mode throughout, no policy switch needed
        }
    }
}
