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

class AppDelegate: NSObject, NSApplicationDelegate, ObservableObject {
    // Menu bar manager for status item
    private var menuBarManager: MenuBarManager?

    // Permission coordinator for permission gate
    private var permissionCoordinator: PermissionCoordinator?

    // Legacy properties for gradual migration
    private var statusItem: NSStatusItem? { menuBarManager?.statusItem }
    private var settingsMenuItem: NSMenuItem? { menuBarManager?.settingsMenuItem }

    // interface (rig-core based) - unified AI processing interface
    @Published internal var core: AetherCore?
    internal var eventHandler: EventHandler?

    // Halo overlay window
    private var haloWindow: HaloWindow?

    // Output coordinator for managing AI response output
    private var outputCoordinator: OutputCoordinator?

    // Input coordinator for managing input capture
    private var inputCoordinator: InputCoordinator?

    // Settings window (used by legacy Settings scene and WindowGroup)
    private var settingsWindow: NSWindow?

    // Permission gate active state (backward compatibility - synced with permissionCoordinator.isActive)
    private var isPermissionGateActive: Bool = false

    // First-time initialization window
    private var initializationWindow: NSWindow?

    // Theme engine removed - using unified visual style

    // Global hotkey monitor (Swift layer)
    private var hotkeyMonitor: GlobalHotkeyMonitor?

    // Vision hotkey manager for screen capture OCR
    private var visionHotkeyManager: VisionHotkeyManager?

    // Multi-turn conversation hotkey monitors (Cmd+Opt+/)
    // Global monitor for when other apps are active
    private var multiTurnHotkeyGlobalMonitor: Any?
    // Local monitor for when Aether is active
    private var multiTurnHotkeyLocalMonitor: Any?

    // MARK: - Managers (via DependencyContainer)

    /// Clipboard monitor accessed through DependencyContainer
    private var clipboardMonitor: any ClipboardMonitorProtocol {
        DependencyContainer.shared.clipboardMonitor
    }

    /// Check if running in UI testing mode
    private var isUITesting: Bool {
        CommandLine.arguments.contains("--uitesting")
    }

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

        // UI Testing mode: Skip permission gate and open settings directly
        if isUITesting {
            print("[Aether] UI Testing mode detected, skipping permission gate")
            DispatchQueue.mainAsyncAfter(delay: 0.3, weakRef: self) { slf in
                slf.initializeRustCore()
                // Open settings window for UI tests
                DispatchQueue.mainAsyncAfter(delay: 0.5, weakRef: slf) { s in
                    s.showSettings()
                }
            }
            return
        }

        // CRITICAL FIX: Delay permission check to allow macOS to sync permission state
        // macOS needs time to update permission status after app launch
        // Without this delay, AXIsProcessTrusted() and IOHIDRequestAccess() may return
        // cached/stale values, causing false negatives even when permissions are granted
        DispatchQueue.mainAsyncAfter(delay: 0.5, weakRef: self) { slf in
            print("[Aether] Checking permissions after startup delay...")

            // Check all required permissions (Accessibility + Input Monitoring)
            let hasAccessibility = PermissionChecker.hasAccessibilityPermission()
            let hasInputMonitoring = PermissionChecker.hasInputMonitoringPermission()

            print("[Aether] Permission status - Accessibility: \(hasAccessibility), InputMonitoring: \(hasInputMonitoring)")

            if !hasAccessibility || !hasInputMonitoring {
                // Show mandatory permission gate if any permission is missing
                slf.showPermissionGate()
            } else {
                print("[Aether] ✅ All permissions granted, checking if first-run initialization needed...")

                // Check if this is a fresh installation
                slf.checkAndRunFirstTimeInit()
            }
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        // Stop hotkey monitoring
        hotkeyMonitor?.stopMonitoring()

        // Stop vision hotkey monitoring
        visionHotkeyManager?.unregisterHotkeys()

        // Stop multi-turn hotkey monitoring
        if let monitor = multiTurnHotkeyGlobalMonitor {
            NSEvent.removeMonitor(monitor)
            multiTurnHotkeyGlobalMonitor = nil
        }
        if let monitor = multiTurnHotkeyLocalMonitor {
            NSEvent.removeMonitor(monitor)
            multiTurnHotkeyLocalMonitor = nil
        }

        // Stop clipboard monitoring
        clipboardMonitor.stopMonitoring()

        // Stop output coordinator (removes ESC key monitor)
        outputCoordinator?.stop()

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
            action: #selector(NSText.cut(_:)),
            keyEquivalent: "x"
        )
        editMenu.addItem(cutItem)

        // Copy
        let copyItem = NSMenuItem(
            title: L("menu.edit.copy"),
            action: #selector(NSText.copy(_:)),
            keyEquivalent: "c"
        )
        editMenu.addItem(copyItem)

        // Paste
        let pasteItem = NSMenuItem(
            title: L("menu.edit.paste"),
            action: #selector(NSText.paste(_:)),
            keyEquivalent: "v"
        )
        editMenu.addItem(pasteItem)

        // Paste and Match Style (for rich text compatibility)
        let pasteAndMatchStyleItem = NSMenuItem(
            title: L("menu.edit.paste_match_style"),
            action: #selector(NSTextView.pasteAsPlainText(_:)),
            keyEquivalent: "V"
        )
        pasteAndMatchStyleItem.keyEquivalentModifierMask = [.command, .option]
        editMenu.addItem(pasteAndMatchStyleItem)

        // Delete
        let deleteItem = NSMenuItem(
            title: L("menu.edit.delete"),
            action: #selector(NSText.delete(_:)),
            keyEquivalent: ""
        )
        editMenu.addItem(deleteItem)

        editMenu.addItem(NSMenuItem.separator())

        // Select All
        let selectAllItem = NSMenuItem(
            title: L("menu.edit.select_all"),
            action: #selector(NSText.selectAll(_:)),
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
        // Initialize MenuBarManager if not already created
        if menuBarManager == nil {
            menuBarManager = MenuBarManager()
        }

        // Debug actions for DEBUG builds
        #if DEBUG
        let debugActions: [(title: String, action: Selector, keyEquivalent: String)] = [
            ("Test Clarification (Select)", #selector(testClarificationSelect), "d"),
            ("Test Clarification (Text)", #selector(testClarificationText), "t")
        ]
        #else
        let debugActions: [(title: String, action: Selector, keyEquivalent: String)]? = nil
        #endif

        // Setup menu bar via MenuBarManager
        menuBarManager?.setup(
            target: self,
            showAboutAction: #selector(showAbout),
            showSettingsAction: #selector(showSettings),
            quitAction: #selector(quit),
            debugActions: debugActions
        )

        // Initially disable Settings menu if permissions not granted
        menuBarManager?.setSettingsEnabled(!isPermissionGateActive)
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
        // Skip checks in UI testing mode
        if !isUITesting {
            // Block settings access if permission gate is active
            if isPermissionGateActive {
                print("[Aether] Settings blocked - permission gate is active")
                return
            }

            // CRITICAL: Check if core is initialized before opening settings
            guard core != nil else {
                print("[Aether] ERROR: Core not initialized, cannot open settings")
                eventHandler?.showToast(
                    type: .warning,
                    title: L("error.core_not_initialized"),
                    message: L("error.core_not_initialized.suggestion"),
                    autoDismiss: false
                )
                return
            }
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
        // RootContentView gets core from appDelegate internally
        let settingsView = RootContentView()
            .environmentObject(self)

        let hostingController = NSHostingController(rootView: settingsView)
        hostingController.sizingOptions = []  // Disable auto-sizing

        // UI Testing Mode: Use standard NSWindow for XCTest accessibility detection
        // NSPanel with nonactivatingPanel style is invisible to XCTest
        if isUITesting {
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
            window.identifier = NSUserInterfaceItemIdentifier("SettingsWindow")
            window.minSize = NSSize(width: 980, height: 750)
            window.center()
            window.isReleasedWhenClosed = false
            window.delegate = self

            settingsWindow = window

            // Show window and activate app for XCTest
            NSApp.setActivationPolicy(.regular)
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return
        }

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

        // Accessibility identifier for UI testing
        panel.identifier = NSUserInterfaceItemIdentifier("SettingsWindow")

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

    // MARK: - Providers Menu Management

    /// Rebuild the providers submenu with enabled providers
    private func rebuildProvidersMenu() {
        guard let core = core else {
            return
        }

        let enabledProviders = core.getEnabledProviders().sorted()
        let defaultProvider = core.getDefaultProvider()

        // Map providers to (id, displayName) tuples
        let items = enabledProviders.map { ($0, $0) }

        // Delegate to MenuBarManager
        menuBarManager?.rebuildProvidersMenu(
            providers: items,
            currentSelection: defaultProvider,
            target: self,
            action: #selector(selectDefaultProvider(_:))
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

    // MARK: - Core Initialization

    /// Initialize the Rust core systems (triggers, hotkeys, vision)
    /// This is called after core is initialized
    private func initializeRustCore() {
        // Show Halo animation immediately on startup (better UX feedback)
        haloWindow?.updateState(.processing(streamingText: nil))
        haloWindow?.showCentered()
        print("[Aether] Showing Halo startup animation")

        // CRITICAL: Re-verify permissions before initializing trigger system
        // This prevents crashes if permissions were revoked or not fully applied
        let hasAccessibility = PermissionChecker.hasAccessibilityPermission()
        let hasInputMonitoring = PermissionChecker.hasInputMonitoringPermission()

        print("[Aether] Pre-trigger init permission check - Accessibility: \(hasAccessibility), InputMonitoring: \(hasInputMonitoring)")

        if !hasAccessibility || !hasInputMonitoring {
            print("[Aether] ERROR: Permissions not fully granted, BLOCKING trigger system initialization")
            print("[Aether] Missing permissions:")
            if !hasAccessibility {
                print("[Aether]   - Accessibility: REQUIRED for global hotkey detection")
            }
            if !hasInputMonitoring {
                print("[Aether]   - Input Monitoring: REQUIRED for full functionality")
            }

            // Show permission gate again
            DispatchQueue.mainAsync(weakRef: self) { slf in
                slf.showPermissionGate()
            }
            return
        }

        // Initialize the trigger-based hotkey system (GlobalHotkeyMonitor)
        // This uses the two-callback architecture:
        // - Replace hotkey (default: double-tap left Shift) → handleReplaceTriggered()
        // - Append hotkey (default: double-tap right Shift) → handleAppendTriggered()
        print("[Aether] Initializing trigger-based hotkey system...")
        initializeTriggerSystem()

        // Initialize vision hotkey manager for screen capture OCR
        // Hotkeys: Cmd+Option+O (Region selection capture + OCR)
        NSLog("[Aether] Initializing VisionHotkeys...")
        initializeVisionHotkeys()
        NSLog("[Aether] VisionHotkeys initialized")

        // Check if monitoring started successfully
        guard hotkeyMonitor != nil else {
            print("[Aether] ❌ Failed to initialize trigger system")
            // Fall back to showing permission gate
            DispatchQueue.mainAsync(weakRef: self) { slf in
                slf.showPermissionGate()
            }
            return
        }

        print("[Aether] Trigger system initialized successfully")

        // Setup Cmd+Opt+/ hotkey to route to MultiTurnCoordinator
        setupMultiTurnHotkey()
        print("[Aether] MultiTurn hotkey installed")

        // Hide startup Halo animation (initialization succeeded)
        haloWindow?.hide()
        print("[Aether] Hiding Halo startup animation (init succeeded)")

        // Update menu bar icon to show active state
        updateMenuBarIcon(state: .listening)
    }

    // MARK: - Core Initialization (rig-core based)

    /// Initialize AetherCore using the rig-core based interface
    /// This is the unified AI processing core for all Aether functionality
    private func initializeCore() {
        guard let eventHandler = eventHandler else {
            print("[Aether] Error: EventHandler not initialized")
            return
        }

        // Config path for v2 interface
        let configPath = NSHomeDirectory() + "/.config/aether/config.toml"

        // Check if config file exists
        if !FileManager.default.fileExists(atPath: configPath) {
            print("[Aether] Warning: Config file not found at \(configPath)")
            print("[Aether] initialization skipped - create config file first")
            return
        }

        do {
            // Initialize core using initCore()
            core = try initCore(configPath: configPath, handler: eventHandler)
            print("[Aether] AetherCore initialized successfully")

            // Set core reference in event handler for cancellation
            eventHandler.setCore(core!)

            // Configure OutputCoordinator with dependencies
            outputCoordinator?.configure(core: core, haloWindow: haloWindow)

            // Configure InputCoordinator with dependencies
            inputCoordinator?.configure(
                core: core,
                haloWindow: haloWindow,
                eventHandler: eventHandler,
                outputCoordinator: outputCoordinator
            )

            // Set InputCoordinator reference in EventHandler for callbacks
            eventHandler.setInputCoordinator(inputCoordinator)

            // Configure MultiTurnCoordinator with dependencies
            MultiTurnCoordinator.shared.configure(core: core)

            print("[Aether] coordinators configured")

            // Log available tools
            if let tools = core?.listTools() {
                print("[Aether] has \(tools.count) tools available:")
                for tool in tools.prefix(5) {
                    print("[Aether]   - \(tool.name): \(tool.description)")
                }
                if tools.count > 5 {
                    print("[Aether]   ... and \(tools.count - 5) more")
                }
            }

        } catch {
            print("[Aether] Error initializing core: \(error)")
            // failure prevents app from functioning - show error to user
            eventHandler.onError(message: "Failed to initialize core: \(error.localizedDescription)")
        }
    }

    /// Process input using interface (rig-core based)
    /// This is an async operation - results come via EventHandler callbacks
    ///
    /// - Parameters:
    ///   - input: User input text to process
    ///   - appContext: Optional app context (e.g., "Safari" or "Xcode")
    ///   - windowTitle: Optional window title for context
    ///   - stream: Whether to stream response chunks (default: true)
    func process(input: String, appContext: String? = nil, windowTitle: String? = nil, stream: Bool = true) {
        guard let core = core else {
            print("[Aether] Error: core not initialized")
            eventHandler?.onError(message: "core not initialized")
            return
        }

        print("[Aether] Processing with interface: \(input.prefix(50))...")

        do {
            let options = ProcessOptions(
                appContext: appContext,
                windowTitle: windowTitle,
                stream: stream
            )
            try core.process(input: input, options: options)
        } catch {
            print("[Aether] processing error: \(error)")
            eventHandler?.onError(message: error.localizedDescription)
        }
    }

    /// Cancel current processing operation
    func cancelProcessing() {
        core?.cancel()
        print("[Aether] processing cancelled")
    }

    /// Check if core is available and initialized
    var isCoreAvailable: Bool {
        core != nil
    }

    /// List available tools from core
    func listTools() -> [ToolInfoFfi] {
        return core?.listTools() ?? []
    }

    /// Search memory using interface
    /// - Parameters:
    ///   - query: Search query
    ///   - limit: Maximum number of results
    /// - Returns: Array of memory items matching the query
    func searchMemory(query: String, limit: UInt32 = 10) -> [MemoryItem] {
        guard let core = core else {
            print("[Aether] Error: core not initialized for memory search")
            return []
        }

        do {
            return try core.searchMemory(query: query, limit: limit)
        } catch {
            print("[Aether] memory search error: \(error)")
            return []
        }
    }

    /// Clear all memory
    func clearMemory() -> Bool {
        guard let core = core else {
            print("[Aether] Error: core not initialized for memory clear")
            return false
        }

        do {
            try core.clearMemory()
            print("[Aether] memory cleared")
            return true
        } catch {
            print("[Aether] memory clear error: \(error)")
            return false
        }
    }

    /// Reload configuration
    func reloadConfig() -> Bool {
        guard let core = core else {
            print("[Aether] Error: core not initialized for config reload")
            return false
        }

        do {
            try core.reloadConfig()
            print("[Aether] config reloaded")
            return true
        } catch {
            print("[Aether] config reload error: \(error)")
            return false
        }
    }

    private func updateMenuBarIcon(state: ProcessingState) {
        // Delegate to MenuBarManager
        menuBarManager?.updateIcon(for: state)
    }

    /// Update menu bar icon with custom symbol (for permission gate states)
    private func updateMenuBarIcon(systemSymbol: String) {
        // Delegate to MenuBarManager
        menuBarManager?.updateIcon(systemSymbol: systemSymbol)
    }


    // MARK: - Permission Gate Management

    /// Show mandatory permission gate window
    private func showPermissionGate() {
        // Initialize and configure PermissionCoordinator if needed
        if permissionCoordinator == nil {
            permissionCoordinator = PermissionCoordinator()
        }

        // Configure with dependencies
        permissionCoordinator?.configure(
            menuBarManager: menuBarManager,
            inputCoordinator: inputCoordinator
        )

        // Set callback for when permissions are granted
        permissionCoordinator?.onPermissionGranted = { [weak self] in
            self?.onPermissionGateDismissed()
        }

        // Delegate to PermissionCoordinator
        permissionCoordinator?.showPermissionGate()

        // Sync local state (for backward compatibility)
        isPermissionGateActive = true
    }

    /// Called when permission gate is dismissed (all permissions granted)
    private func onPermissionGateDismissed() {
        print("[AppDelegate] Permission gate dismissed - all permissions granted")

        // Update local state (for backward compatibility)
        isPermissionGateActive = false

        // Reset menu bar icon to default
        menuBarManager?.resetIcon()

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

        // Create Halo window directly (simplified, no controller wrapper needed)
        haloWindow = HaloWindow()

        // Initialize event handler (rig-core based)
        eventHandler = EventHandler(haloWindow: haloWindow)

        // Initialize output coordinator (will configure with core after Rust core init)
        outputCoordinator = OutputCoordinator()
        outputCoordinator?.start()

        // Initialize input coordinator (will configure with core after Rust core init)
        inputCoordinator = InputCoordinator()

        // Start clipboard monitoring for context tracking
        clipboardMonitor.startMonitoring()
        print("[Aether] Clipboard monitoring started for context tracking")

        // Initialize core (rig-core based) - unified AI processing interface
        initializeCore()

        // Initialize trigger system and hotkeys (requires core for config)
        initializeRustCore()

        // Observe config changes to rebuild providers menu
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(onConfigChanged),
            name: .aetherConfigSavedInternally,
            object: nil
        )

    }

    /// Handle config change notification (rebuild providers menu)
    @objc private func onConfigChanged() {
        print("[AppDelegate] Config changed, rebuilding providers menu")
        rebuildProvidersMenu()
    }

    // MARK: - Unified Input Hotkey

    /// Update unified input hotkey at runtime (called from ShortcutsView)
    func updateCommandPromptHotkey(_ shortcuts: ShortcutsConfig) {
        // Update multi-turn hotkey configuration
        updateMultiTurnHotkeyConfig(shortcuts)
    }

    /// Update OCR capture hotkey configuration at runtime
    func updateOcrCaptureHotkey(_ shortcuts: ShortcutsConfig) {
        visionHotkeyManager?.updateHotkey(from: shortcuts)
    }

    /// Get HaloWindow for external components (e.g., OCR feedback)
    func getHaloWindow() -> HaloWindow? {
        return haloWindow
    }

    // MARK: - Multi-Turn Hotkey Configuration

    /// Multi-turn hotkey modifiers (default: Cmd+Opt)
    private var multiTurnHotkeyModifiers: NSEvent.ModifierFlags = [.command, .option]

    /// Multi-turn hotkey key code (default: / = 44)
    private var multiTurnHotkeyKeyCode: UInt16 = 44

    /// Setup global hotkey for multi-turn conversation (configurable, default: Cmd+Opt+/)
    private func setupMultiTurnHotkey() {
        // Load hotkey configuration from config
        loadMultiTurnHotkeyConfig()

        // Hotkey handler closure - shared between global and local monitors
        let hotkeyHandler: (NSEvent) -> Bool = { [weak self] event in
            guard let self = self else { return false }

            // Check for configured hotkey
            var modifiersMatch = true
            for modifier in [NSEvent.ModifierFlags.command, .option, .control, .shift] {
                if self.multiTurnHotkeyModifiers.contains(modifier) {
                    if !event.modifierFlags.contains(modifier) {
                        modifiersMatch = false
                        break
                    }
                }
            }
            if modifiersMatch && event.keyCode == self.multiTurnHotkeyKeyCode {
                MultiTurnCoordinator.shared.handleHotkey()
                return true  // Event handled
            }
            return false
        }

        // Global monitor - captures hotkey when OTHER apps are active
        multiTurnHotkeyGlobalMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { event in
            _ = hotkeyHandler(event)
        }

        // Local monitor - captures hotkey when AETHER is active
        multiTurnHotkeyLocalMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            if hotkeyHandler(event) {
                return nil  // Consume the event
            }
            return event  // Pass through
        }

        print("[AppDelegate] Multi-turn hotkey monitors installed (keyCode: \(multiTurnHotkeyKeyCode), modifiers: \(multiTurnHotkeyModifiers))")
    }

    /// Load multi-turn hotkey configuration from config
    private func loadMultiTurnHotkeyConfig() {
        guard let core = core else {
            return
        }

        do {
            let config = try core.loadConfig()
            if let shortcuts = config.shortcuts {
                parseAndApplyMultiTurnHotkey(shortcuts.commandPrompt)
            }
        } catch {
            print("[AppDelegate] Failed to load multi-turn hotkey config: \(error)")
        }
    }

    /// Parse multi-turn hotkey config string (e.g., "Command+Option+/") and apply it
    private func parseAndApplyMultiTurnHotkey(_ configString: String) {
        let parts = configString.split(separator: "+").map { String($0) }
        guard parts.count >= 2 else {
            print("[AppDelegate] Invalid multi-turn hotkey config: \(configString)")
            return
        }

        var modifiers: NSEvent.ModifierFlags = []

        // Parse modifiers (all parts except the last)
        for i in 0..<(parts.count - 1) {
            switch parts[i] {
            case "Command": modifiers.insert(.command)
            case "Option": modifiers.insert(.option)
            case "Control": modifiers.insert(.control)
            case "Shift": modifiers.insert(.shift)
            default: break
            }
        }

        // Parse last part as key code
        let keyCode: UInt16
        switch parts[parts.count - 1] {
        case "/": keyCode = 44
        case "`": keyCode = 50
        case "\\": keyCode = 42
        case ";": keyCode = 41
        case ",": keyCode = 43
        case ".": keyCode = 47
        case "Space": keyCode = 49
        default: keyCode = 44  // Default to /
        }

        multiTurnHotkeyModifiers = modifiers
        multiTurnHotkeyKeyCode = keyCode
        print("[AppDelegate] Multi-turn hotkey configured: \(configString) (keyCode: \(keyCode), modifiers: \(modifiers))")
    }

    /// Update multi-turn hotkey at runtime (called from ShortcutsView)
    private func updateMultiTurnHotkeyConfig(_ shortcuts: ShortcutsConfig) {
        parseAndApplyMultiTurnHotkey(shortcuts.commandPrompt)

        // Reinstall the monitors with new settings
        if let monitor = multiTurnHotkeyGlobalMonitor {
            NSEvent.removeMonitor(monitor)
            multiTurnHotkeyGlobalMonitor = nil
        }
        if let monitor = multiTurnHotkeyLocalMonitor {
            NSEvent.removeMonitor(monitor)
            multiTurnHotkeyLocalMonitor = nil
        }
        setupMultiTurnHotkey()
        print("[AppDelegate] Multi-turn hotkey updated and monitors reinstalled")
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
        // Delegate to InputCoordinator for trigger handling
        hotkeyMonitor = GlobalHotkeyMonitor(
            replaceKey: replaceKey,
            appendKey: appendKey,
            onReplaceTriggered: { [weak self] in
                self?.inputCoordinator?.handleReplaceTriggered()
            },
            onAppendTriggered: { [weak self] in
                self?.inputCoordinator?.handleAppendTriggered()
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

    // MARK: - Vision Hotkeys

    /// Initialize vision hotkey manager for screen capture OCR
    ///
    /// Default hotkey: Cmd+Shift+Ctrl+4 (Region selection capture + OCR)
    /// Configurable via Settings → Shortcuts
    private func initializeVisionHotkeys() {
        visionHotkeyManager = VisionHotkeyManager()

        // Load hotkey configuration from core
        if let core = core {
            Task {
                do {
                    let config = try core.loadConfig()
                    if let shortcuts = config.shortcuts {
                        await MainActor.run {
                            visionHotkeyManager?.updateHotkey(from: shortcuts)
                        }
                    }
                } catch {
                    print("[AppDelegate] Failed to load OCR hotkey config: \(error)")
                }
            }
        }

        visionHotkeyManager?.registerHotkeys()
        print("[AppDelegate] ✅ Vision hotkey registered")
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
