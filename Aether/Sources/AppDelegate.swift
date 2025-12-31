//
//  AppDelegate.swift
//  Aether
//
//  Application delegate managing menu bar, Rust core lifecycle, and permissions.
//

import Cocoa
import SwiftUI
import Combine

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

        // Stop clipboard monitoring
        ClipboardMonitor.shared.stopMonitoring()

        // Remove ESC key monitor
        removeEscapeKeyMonitor()

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

        // CRITICAL: Check if core is initialized before opening settings
        guard let core = core else {
            print("[Aether] ERROR: Core not initialized, cannot open settings")

            // Show error alert
            let alert = NSAlert()
            alert.messageText = NSLocalizedString("error.core_not_initialized", comment: "")
            alert.informativeText = "Aether核心未初始化，请等待权限授予后重试。"
            alert.alertStyle = .warning
            alert.addButton(withTitle: NSLocalizedString("common.ok", comment: ""))
            alert.runModal()
            return
        }

        // Check if settings window already exists and is valid
        // First check if reference exists and window is still alive (not released)
        if let window = settingsWindow {
            // Safely check if window is still valid before accessing properties
            if window.isVisible {
                // Window exists and is visible, reset to minimum size and bring to front
                window.setContentSize(NSSize(width: 980, height: 750))
                window.center()
                window.makeKeyAndOrderFront(nil)
                NSApp.activate(ignoringOtherApps: true)
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

        // Create window with explicit size
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 980, height: 750),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )

        window.title = "Settings"
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.contentViewController = hostingController

        // Set size constraints
        window.minSize = NSSize(width: 980, height: 750)
        window.center()

        // Window management
        window.hidesOnDeactivate = false
        window.isReleasedWhenClosed = false
        window.delegate = self

        settingsWindow = window
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

            // Load hotkey configuration from config
            let hotkeyMode = loadHotkeyConfiguration()
            hotkeyMonitor = GlobalHotkeyMonitor(hotkeyMode: hotkeyMode) { [weak self] in
                self?.handleHotkeyPressed()
            }

            // Start monitoring for hotkey
            if hotkeyMonitor?.startMonitoring() == true {
                print("[Aether] ✅ Global hotkey monitoring started successfully (\(hotkeyMode.displayString))")
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

        let mouseLocation = NSEvent.mouseLocation

        switch inputMode {
        case .cut:
            // Direct cut mode: Show Halo immediately and process with replace
            print("[AppDelegate] Mode: cut - directly executing Cmd+X")
            DispatchQueue.main.async { [weak self] in
                self?.haloWindow?.show(at: mouseLocation)
                self?.haloWindow?.updateState(.listening)
            }
            processWithInputMode(.replace)

        case .copy:
            // Direct copy mode: Show Halo immediately and process with append
            print("[AppDelegate] Mode: copy - directly executing Cmd+C")
            DispatchQueue.main.async { [weak self] in
                self?.haloWindow?.show(at: mouseLocation)
                self?.haloWindow?.updateState(.listening)
            }
            processWithInputMode(.append)

        case .halo:
            // Halo selection mode: Show selection UI first
            print("[AppDelegate] Mode: halo - showing selection UI")
            DispatchQueue.main.async { [weak self] in
                guard let self = self else { return }
                self.haloWindow?.show(at: mouseLocation)
                self.haloWindow?.updateState(.awaitingInputMode { [weak self] choice in
                    guard let self = self else {
                        print("[AppDelegate] ❌ Self is nil when user selected input mode")
                        return
                    }

                    // User selected input mode, proceed with processing
                    print("[AppDelegate] 📋 User selected: \(choice)")

                    // CRITICAL: Hide the input mode selection and show listening state
                    // This must happen before processing to avoid focus issues
                    self.haloWindow?.updateState(.listening)

                    // CRITICAL: Add a small delay to let the Halo UI update
                    // and ensure the original app regains focus
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { [weak self] in
                        self?.processWithInputMode(choice)
                    }
                })
            }
        }
    }

    /// Process input after input mode is determined (either from config or user selection)
    private func processWithInputMode(_ choice: InputModeChoice) {
        print("[AppDelegate] Processing with input mode choice: \(choice)")

        guard core != nil else {
            print("[AppDelegate] ⚠️ Core not initialized")
            // Show error in Halo
            DispatchQueue.main.async { [weak self] in
                self?.haloWindow?.updateState(.error(
                    type: .unknown,
                    message: NSLocalizedString("error.core_not_initialized", comment: "Core not initialized"),
                    suggestion: NSLocalizedString("error.core_not_initialized.suggestion", comment: "Please restart the app")
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

        let useCutMode = (choice == .replace)
        print("[AppDelegate] 📋 Using cut mode: \(useCutMode)")

        // Track where the text came from - this determines output strategy
        var textSource: TextSource = .selectedText

        // CRITICAL: Save original clipboard content to restore later
        // This protects user's pre-existing clipboard data
        let originalClipboardText = ClipboardManager.shared.getText()
        let originalChangeCount = ClipboardManager.shared.changeCount()
        print("[AppDelegate] 💾 Saved original clipboard state (changeCount: \(originalChangeCount))")

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
                    DispatchQueue.main.async { [weak self] in
                        self?.haloWindow?.show(at: NSEvent.mouseLocation)
                        self?.haloWindow?.updateState(.error(
                            type: .unknown,
                            message: NSLocalizedString("error.no_text_in_window", comment: "No text content in current window"),
                            suggestion: NSLocalizedString("error.no_text_in_window.suggestion", comment: "Please open a text document first")
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

        // Get the captured clipboard content
        guard let clipboardText = ClipboardManager.shared.getText() else {
            print("[AppDelegate] ❌ Clipboard is empty after copy operation")
            // Restore original clipboard
            if let original = originalClipboardText {
                ClipboardManager.shared.setText(original)
            }
            return
        }

        print("[AppDelegate] Clipboard text: \(clipboardText.prefix(50))...")

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

        // Update Halo to listening state (Halo is already shown from handleHotkeyPressed)
        DispatchQueue.main.async { [weak self] in
            self?.haloWindow?.updateState(.listening)
        }

        // Process input asynchronously to avoid blocking UI
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self = self else { return }

            do {
                // Create captured context for Rust
                let capturedContext = CapturedContext(
                    appBundleId: windowContext.bundleId ?? "unknown",
                    windowTitle: windowContext.windowTitle
                )

                // CRITICAL: Construct user input with clipboard context if available
                // This provides additional context to the AI for better responses
                let userInput: String
                if let clipContext = clipboardContext {
                    // Format: Current text + Clipboard context
                    userInput = """
                    Current content:
                    \(clipboardText)

                    Clipboard context (recent copy):
                    \(clipContext)
                    """
                    print("[AppDelegate] 🤖 Sending to AI: current text (\(clipboardText.count) chars) + clipboard context (\(clipContext.count) chars)")
                } else {
                    // No clipboard context, just send current text
                    userInput = clipboardText
                    print("[AppDelegate] 🤖 Sending to AI: current text only (\(clipboardText.count) chars)")
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

                // Output AI response using paste (more reliable than typewriter)
                // NOTE: Using paste instead of typewriter because CGEvent-based typing
                // can fail silently when the target app doesn't properly receive events
                DispatchQueue.main.async { [weak self] in
                    guard let self = self else { return }

                    print("[AppDelegate] 🎯 Starting output phase...")
                    print("[AppDelegate] 📍 Text source: \(textSource), Replace mode: \(useCutMode)")

                    // CRITICAL: Add small delay to ensure UI is stable before keyboard simulation
                    // This helps when focus might have shifted during AI processing
                    Thread.sleep(forTimeInterval: 0.1)

                    // CRITICAL: Prepare cursor position based on text source and input mode
                    // This ensures AI response is placed correctly (replace vs append)
                    self.prepareOutputPosition(textSource: textSource, useCutMode: useCutMode)

                    // Small delay after cursor positioning
                    Thread.sleep(forTimeInterval: 0.05)

                    // Use paste for reliable output (typewriter can fail silently)
                    print("[AppDelegate] 📋 Setting clipboard with AI response (\(truncatedResponse.count) chars)")
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
            print("[AppDelegate] ESC pressed but no typewriter is running")
            return
        }

        print("[AppDelegate] ESC pressed - cancelling typewriter animation")
        cancellation.cancel()

        // Show brief feedback
        DispatchQueue.main.async { [weak self] in
            self?.haloWindow?.updateState(.success(finalText: "⏸ Typewriter cancelled"))
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) { [weak self] in
                self?.haloWindow?.hide()
            }
        }
    }

    // MARK: - Hotkey Configuration

    /// Load hotkey configuration from config file
    /// - Returns: The configured hotkey mode, or default (double-tap Space) if not configured
    private func loadHotkeyConfiguration() -> HotkeyMode {
        // Try to load from user config
        let configPath = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/aether/config.toml")

        if FileManager.default.fileExists(atPath: configPath.path) {
            do {
                let content = try String(contentsOf: configPath, encoding: .utf8)

                // Parse [shortcuts] section for summon key
                if let summonLine = content.split(separator: "\n").first(where: { $0.hasPrefix("summon") }) {
                    let value = summonLine
                        .split(separator: "=")
                        .last?
                        .trimmingCharacters(in: .whitespaces)
                        .replacingOccurrences(of: "\"", with: "")
                        ?? ""

                    if let mode = HotkeyMode.from(configString: value) {
                        print("[Aether] Loaded hotkey from config: \(mode.displayString)")
                        return mode
                    }
                }
            } catch {
                print("[Aether] Failed to read config file: \(error)")
            }
        }

        // Default: double-tap Space
        print("[Aether] Using default hotkey: double-tap Space")
        return .default
    }

    /// Update hotkey configuration at runtime
    func updateHotkeyConfiguration(_ mode: HotkeyMode) {
        hotkeyMonitor?.updateHotkey(mode)
        print("[AppDelegate] Hotkey updated to: \(mode.displayString)")
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
