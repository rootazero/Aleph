//
//  AppDelegate.swift
//  Aleph
//
//  Application delegate managing menu bar, Rust core lifecycle, and permissions.
//

import Cocoa
import SwiftUI
import Combine
import Carbon.HIToolbox

@MainActor
class AppDelegate: NSObject, NSApplicationDelegate, ObservableObject {
    // Menu bar manager for status item
    private var menuBarManager: MenuBarManager?

    // Permission coordinator for permission gate
    private var permissionCoordinator: PermissionCoordinator?

    // Legacy properties for gradual migration
    private var statusItem: NSStatusItem? { menuBarManager?.statusItem }
    private var settingsMenuItem: NSMenuItem? { menuBarManager?.settingsMenuItem }

    // interface (rig-core based) - unified AI processing interface
    @Published internal var core: AlephCore?
    internal var eventHandler: EventHandler?

    // Halo overlay window
    private var haloWindow: HaloWindow?


    // Settings window (used by legacy Settings scene and WindowGroup)
    private var settingsWindow: NSWindow?

    // Permission gate active state (backward compatibility - synced with permissionCoordinator.isActive)
    private var isPermissionGateActive: Bool = false

    // First-time initialization window
    private var initializationWindow: NSWindow?

    // Unified hotkey service (manages all hotkey systems)
    private var hotkeyService: HotkeyService?

    // Conversation hotkey monitors (legacy - now managed by HotkeyService)
    // Global monitor for when other apps are active
    private var hotkeyGlobalMonitor: Any?
    // Local monitor for when Aleph is active
    private var hotkeyLocalMonitor: Any?

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
            print("[Aleph] UI Testing mode detected, skipping permission gate")
            Task { @MainActor [weak self] in
                try? await Task.sleep(seconds: 0.3)
                self?.initializeRustCore()
                // Open settings window for UI tests
                try? await Task.sleep(seconds: 0.5)
                self?.showSettings()
            }
            return
        }

        // CRITICAL FIX: Delay permission check to allow macOS to sync permission state
        // macOS needs time to update permission status after app launch
        // Without this delay, AXIsProcessTrusted() and IOHIDRequestAccess() may return
        // cached/stale values, causing false negatives even when permissions are granted
        Task { @MainActor [weak self] in
            try? await Task.sleep(seconds: 0.5)
            guard let self = self else { return }

            NSLog("[Aleph] Checking permissions after startup delay...")
            print("[Aleph] Checking permissions after startup delay...")

            // Check all required permissions (Accessibility + Input Monitoring)
            let hasAccessibility = PermissionChecker.hasAccessibilityPermission()
            let hasInputMonitoring = PermissionChecker.hasInputMonitoringPermission()

            NSLog("[Aleph] Permission status - Accessibility: %@, InputMonitoring: %@",
                  hasAccessibility ? "YES" : "NO", hasInputMonitoring ? "YES" : "NO")
            print("[Aleph] Permission status - Accessibility: \(hasAccessibility), InputMonitoring: \(hasInputMonitoring)")

            if !hasAccessibility || !hasInputMonitoring {
                // Show mandatory permission gate if any permission is missing
                NSLog("[Aleph] Missing permissions - showing permission gate")
                self.showPermissionGate()
            } else {
                NSLog("[Aleph] ✅ All permissions granted, calling checkAndRunFirstTimeInit()...")
                print("[Aleph] ✅ All permissions granted, checking if first-run initialization needed...")

                // Check if this is a fresh installation
                self.checkAndRunFirstTimeInit()
            }
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        // Stop unified hotkey service (manages all hotkey systems)
        hotkeyService?.stopAllHotkeys()

        // Legacy cleanup (in case HotkeyService wasn't used)
        if let monitor = hotkeyGlobalMonitor {
            NSEvent.removeMonitor(monitor)
            hotkeyGlobalMonitor = nil
        }
        if let monitor = hotkeyLocalMonitor {
            NSEvent.removeMonitor(monitor)
            hotkeyLocalMonitor = nil
        }

        // Stop clipboard monitoring
        clipboardMonitor.stopMonitoring()


        // Shutdown Gateway connection
        GatewayManager.shared.shutdown()
        print("[Aleph] Gateway shutdown")

        // Clean up Rust core (only if initialized)
        print("[Aleph] Application terminating")
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
            showConversationAction: #selector(showConversation),
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

    /// Show or bring conversation window to front
    /// Called from menu bar item
    @objc private func showConversation() {
        HaloInputCoordinator.shared.showOrBringToFront()
    }

    @objc private func showSettings() {
        // Skip checks in UI testing mode
        if !isUITesting {
            // Block settings access if permission gate is active
            if isPermissionGateActive {
                print("[Aleph] Settings blocked - permission gate is active")
                return
            }

            // CRITICAL: Check if core is initialized before opening settings
            guard core != nil else {
                print("[Aleph] ERROR: Core not initialized, cannot open settings")
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

        // Prefer Gateway RPC when available
        if GatewayManager.shared.isReady {
            Task { @MainActor in
                do {
                    let success = try await GatewayManager.shared.client.providersSetDefault(name: providerName)
                    if success {
                        print("[AppDelegate] ✅ Default provider set to: \(providerName) via Gateway")
                        self.rebuildProvidersMenu()
                    } else {
                        print("[AppDelegate] ❌ Gateway returned false for setDefaultProvider")
                    }
                } catch {
                    print("[AppDelegate] Gateway setDefaultProvider failed, falling back to FFI: \(error)")
                    self.setDefaultProviderViaFFI(providerName: providerName)
                }
            }
            return
        }

        setDefaultProviderViaFFI(providerName: providerName)
    }

    /// Set default provider via FFI (fallback)
    private func setDefaultProviderViaFFI(providerName: String) {
        guard let core = core else {
            print("[AppDelegate] ERROR: Core not initialized")
            return
        }

        do {
            try core.setDefaultProvider(providerName: providerName)

            print("[AppDelegate] ✅ Default provider set to: \(providerName) via FFI")

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
        haloWindow?.updateState(.streaming(StreamingContext(runId: "startup", phase: .thinking)))
        haloWindow?.showCentered()
        print("[Aleph] Showing Halo startup animation")

        // CRITICAL: Re-verify permissions before initializing trigger system
        // This prevents crashes if permissions were revoked or not fully applied
        let hasAccessibility = PermissionChecker.hasAccessibilityPermission()
        let hasInputMonitoring = PermissionChecker.hasInputMonitoringPermission()

        print("[Aleph] Pre-trigger init permission check - Accessibility: \(hasAccessibility), InputMonitoring: \(hasInputMonitoring)")

        if !hasAccessibility || !hasInputMonitoring {
            print("[Aleph] ERROR: Permissions not fully granted, BLOCKING trigger system initialization")
            print("[Aleph] Missing permissions:")
            if !hasAccessibility {
                print("[Aleph]   - Accessibility: REQUIRED for global hotkey detection")
            }
            if !hasInputMonitoring {
                print("[Aleph]   - Input Monitoring: REQUIRED for full functionality")
            }

            // Show permission gate again
            Task { @MainActor [weak self] in
                self?.showPermissionGate()
            }
            return
        }

        // Initialize unified HotkeyService (manages hotkey systems)
        // - Multi-turn conversation hotkey (Option+Space)
        print("[Aleph] Initializing HotkeyService...")

        hotkeyService = HotkeyService()
        hotkeyService?.configure(core: core)
        hotkeyService?.startAllHotkeys()

        print("[Aleph] HotkeyService initialized successfully")

        // Hide startup Halo animation (initialization succeeded)
        haloWindow?.hide()
        print("[Aleph] Hiding Halo startup animation (init succeeded)")

        // Update menu bar icon to show active state
        updateMenuBarIcon(state: .listening)

    }

    // MARK: - Core Initialization (rig-core based)

    /// Initialize AlephCore using the rig-core based interface
    /// This is the unified AI processing core for all Aleph functionality
    private func initializeCore() {
        guard let eventHandler = eventHandler else {
            print("[Aleph] Error: EventHandler not initialized")
            return
        }

        // Config path for v2 interface (unified path: ~/.aleph/)
        let configPath = NSHomeDirectory() + "/.aleph/config.toml"

        // Check if config file exists
        if !FileManager.default.fileExists(atPath: configPath) {
            print("[Aleph] Warning: Config file not found at \(configPath)")
            print("[Aleph] initialization skipped - create config file first")
            return
        }

        do {
            // Initialize core using initCore()
            core = try initCore(configPath: configPath, handler: eventHandler)
            print("[Aleph] AlephCore initialized successfully")

            // Initialize providers menu with enabled providers
            rebuildProvidersMenu()

            // Set core reference in event handler for cancellation
            if let coreInstance = core {
                eventHandler.setCore(coreInstance)
            }

            // Configure HaloInputCoordinator with dependencies
            HaloInputCoordinator.shared.configure(haloWindow: haloWindow, core: core)

            print("[Aleph] coordinators configured")

            // Log available tools
            if let tools = core?.listTools() {
                print("[Aleph] has \(tools.count) tools available:")
                for tool in tools.prefix(5) {
                    print("[Aleph]   - \(tool.name): \(tool.description)")
                }
                if tools.count > 5 {
                    print("[Aleph]   ... and \(tools.count - 5) more")
                }
            }

        } catch {
            print("[Aleph] Error initializing core: \(error)")
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
            print("[Aleph] Error: core not initialized")
            eventHandler?.onError(message: "core not initialized")
            return
        }

        print("[Aleph] Processing with interface: \(input.prefix(50))...")

        do {
            let options = ProcessOptions(
                appContext: appContext,
                windowTitle: windowTitle,
                topicId: nil,  // Will be set by HaloInputCoordinator for conversations
                stream: stream,
                attachments: nil,  // No attachments for direct process calls
                preferredLanguage: LocalizationManager.shared.currentLanguage
            )
            try core.process(input: input, options: options)
        } catch {
            print("[Aleph] processing error: \(error)")
            eventHandler?.onError(message: error.localizedDescription)
        }
    }

    /// Cancel current processing operation
    func cancelProcessing() {
        core?.cancel()
        print("[Aleph] processing cancelled")
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
            print("[Aleph] Error: core not initialized for memory search")
            return []
        }

        do {
            return try core.searchMemory(query: query, limit: limit)
        } catch {
            print("[Aleph] memory search error: \(error)")
            return []
        }
    }

    /// Clear all memory
    func clearMemory() -> Bool {
        // Prefer Gateway RPC when available
        if GatewayManager.shared.isReady {
            Task {
                do {
                    try await GatewayManager.shared.client.memoryClear()
                    print("[Aleph] memory cleared via Gateway")
                } catch {
                    print("[Aleph] Gateway memory clear failed: \(error)")
                    // Fallback to FFI
                    self.clearMemoryViaFFI()
                }
            }
            return true // Optimistic return since async
        }

        return clearMemoryViaFFI()
    }

    /// Clear memory via FFI (fallback)
    private func clearMemoryViaFFI() -> Bool {
        guard let core = core else {
            print("[Aleph] Error: core not initialized for memory clear")
            return false
        }

        do {
            try core.clearMemory()
            print("[Aleph] memory cleared via FFI")
            return true
        } catch {
            print("[Aleph] memory clear error: \(error)")
            return false
        }
    }

    /// Reload configuration
    func reloadConfig() -> Bool {
        guard let core = core else {
            print("[Aleph] Error: core not initialized for config reload")
            return false
        }

        do {
            try core.reloadConfig()
            print("[Aleph] config reloaded")
            return true
        } catch {
            print("[Aleph] config reload error: \(error)")
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
            menuBarManager: menuBarManager
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
        let needsInit = needsFirstTimeInit()
        NSLog("[Aleph] needsFirstTimeInit=%@", needsInit ? "true" : "false")

        if needsInit {
            NSLog("[Aleph] 🆕 Fresh install detected - showing initialization window")
            // Show blocking initialization window
            showInitializationWindow()
        } else {
            NSLog("[Aleph] ✅ Existing installation - proceeding with app startup")
            initializeAppComponents()
        }
    }

    /// Show the first-time initialization progress window
    ///
    /// This shows a blocking NSPanel that displays progress for all 6 initialization phases.
    /// The window cannot be closed until initialization completes or fails with retry option.
    private func showInitializationWindow() {
        let initView = InitializationProgressView(
            onCompletion: { [weak self] in
                Task { @MainActor in
                    print("[Aleph] ✅ Initialization completed - proceeding with app startup")
                    self?.closeInitializationWindow()
                    self?.initializeAppComponents()
                }
            },
            onFailure: { error in
                Task { @MainActor in
                    print("[Aleph] ❌ Initialization failed: \(error)")
                    // The InitializationProgressView handles retry internally
                    // If user can't retry, they can quit from the window
                }
            }
        )

        let hostingController = NSHostingController(rootView: initView)

        // Create NSPanel (non-closable during initialization)
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 480, height: 420),
            styleMask: [.titled, .fullSizeContentView],  // No close button
            backing: .buffered,
            defer: false
        )

        panel.title = "正在初始化 Aleph"
        panel.titlebarAppearsTransparent = true
        panel.titleVisibility = .hidden
        panel.contentViewController = hostingController
        panel.center()
        panel.level = .floating
        panel.isReleasedWhenClosed = false
        panel.becomesKeyOnlyIfNeeded = false  // Can become key window

        initializationWindow = panel
        panel.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)

        print("[Aleph] Initialization window shown (6 phases)")
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
        print("[Aleph] Initializing app components")

        // Create Halo window directly (simplified, no controller wrapper needed)
        haloWindow = HaloWindow()

        // Initialize event handler (rig-core based)
        eventHandler = EventHandler(haloWindow: haloWindow)

        // Start clipboard monitoring for context tracking
        clipboardMonitor.startMonitoring()
        print("[Aleph] Clipboard monitoring started for context tracking")

        // Initialize Gateway connection (non-blocking)
        // Gateway provides WebSocket-based agent communication as alternative to FFI
        initializeGateway()

        // Initialize core (rig-core based) - unified AI processing interface
        initializeCore()

        // Initialize trigger system and hotkeys (requires core for config)
        initializeRustCore()

        // Observe config changes to rebuild providers menu
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(onConfigChanged),
            name: .alephConfigSavedInternally,
            object: nil
        )

    }

    /// Initialize Gateway connection (non-blocking)
    ///
    /// Attempts to connect to the Gateway WebSocket server.
    /// If Gateway is running, it will be used for agent execution.
    /// If not, falls back to FFI-based processing.
    private func initializeGateway() {
        Task {
            do {
                try await GatewayManager.shared.initialize()
                print("[Aleph] 🌐 Gateway connected - WebSocket mode available")
            } catch {
                print("[Aleph] Gateway not available (using FFI): \(error.localizedDescription)")
            }
        }
    }

    /// Handle config change notification (rebuild providers menu)
    @objc private func onConfigChanged() {
        print("[AppDelegate] Config changed, rebuilding providers menu")
        rebuildProvidersMenu()
    }

    // MARK: - Unified Input Hotkey

    /// Update unified input hotkey at runtime (called from ShortcutsView)
    func updateCommandPromptHotkey(_ shortcuts: ShortcutsConfig) {
        // Delegate to HotkeyService
        hotkeyService?.updateConversationHotkey(shortcuts: shortcuts)
    }

    /// Get HaloWindow for external components
    func getHaloWindow() -> HaloWindow? {
        return haloWindow
    }

    // MARK: - Legacy Hotkey Configuration (managed by HotkeyService)

    /// Legacy hotkey modifiers (default: Option)
    private var legacyHotkeyModifiers: NSEvent.ModifierFlags = [.option]

    /// Legacy hotkey key code (default: Space = 49)
    private var legacyHotkeyKeyCode: UInt16 = 49

    /// Setup global hotkey for conversation (configurable, default: Option+Space)
    private func setupLegacyHotkey() {
        // Load hotkey configuration from config
        loadLegacyHotkeyConfig()

        // Hotkey handler closure - shared between global and local monitors
        let hotkeyHandler: (NSEvent) -> Bool = { [weak self] event in
            guard let self = self else { return false }

            // Check for configured hotkey
            var modifiersMatch = true
            for modifier in [NSEvent.ModifierFlags.command, .option, .control, .shift] {
                if self.legacyHotkeyModifiers.contains(modifier) {
                    if !event.modifierFlags.contains(modifier) {
                        modifiersMatch = false
                        break
                    }
                }
            }
            if modifiersMatch && event.keyCode == self.legacyHotkeyKeyCode {
                HaloInputCoordinator.shared.handleHotkey()
                return true  // Event handled
            }
            return false
        }

        // Global monitor - captures hotkey when OTHER apps are active
        hotkeyGlobalMonitor = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { event in
            _ = hotkeyHandler(event)
        }

        // Local monitor - captures hotkey when AETHER is active
        hotkeyLocalMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            if hotkeyHandler(event) {
                return nil  // Consume the event
            }
            return event  // Pass through
        }

        print("[AppDelegate] Legacy hotkey monitors installed (keyCode: \(legacyHotkeyKeyCode), modifiers: \(legacyHotkeyModifiers))")
    }

    /// Load legacy hotkey configuration from config
    private func loadLegacyHotkeyConfig() {
        guard let core = core else {
            return
        }

        do {
            let config = try core.loadConfig()
            if let shortcuts = config.shortcuts {
                parseAndApplyLegacyHotkey(shortcuts.commandPrompt)
            }
        } catch {
            print("[AppDelegate] Failed to load hotkey config: \(error)")
        }
    }

    /// Parse hotkey config string (e.g., "Option+Space") and apply it
    private func parseAndApplyLegacyHotkey(_ configString: String) {
        let parts = configString.split(separator: "+").map { String($0) }
        guard parts.count >= 2 else {
            print("[AppDelegate] Invalid hotkey config: \(configString)")
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

        legacyHotkeyModifiers = modifiers
        legacyHotkeyKeyCode = keyCode
        print("[AppDelegate] Legacy hotkey configured: \(configString) (keyCode: \(keyCode), modifiers: \(modifiers))")
    }

    /// Update legacy hotkey at runtime (called from ShortcutsView)
    private func updateLegacyHotkeyConfig(_ shortcuts: ShortcutsConfig) {
        parseAndApplyLegacyHotkey(shortcuts.commandPrompt)

        // Reinstall the monitors with new settings
        if let monitor = hotkeyGlobalMonitor {
            NSEvent.removeMonitor(monitor)
            hotkeyGlobalMonitor = nil
        }
        if let monitor = hotkeyLocalMonitor {
            NSEvent.removeMonitor(monitor)
            hotkeyLocalMonitor = nil
        }
        setupLegacyHotkey()
        print("[AppDelegate] Legacy hotkey updated and monitors reinstalled")
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
            groups: nil,
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
            groups: nil,
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
