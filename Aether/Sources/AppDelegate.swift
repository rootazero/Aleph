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

// TextSource, OutputSessionType, OutputContext moved to OutputCoordinator.swift

class AppDelegate: NSObject, NSApplicationDelegate, ObservableObject {
    // Menu bar manager for status item
    private var menuBarManager: MenuBarManager?

    // Permission coordinator for permission gate
    private var permissionCoordinator: PermissionCoordinator?

    // Legacy properties for gradual migration
    private var statusItem: NSStatusItem? { menuBarManager?.statusItem }
    private var settingsMenuItem: NSMenuItem? { menuBarManager?.settingsMenuItem }

    // Rust core instance (internal for access from AetherApp)
    // Published to trigger UI updates when initialized
    @Published internal var core: AetherCore?

    // Event handler for Rust callbacks (internal for toast access)
    internal var eventHandler: EventHandler?

    // Halo overlay window
    private var haloWindow: HaloWindow?

    // Halo window controller (new architecture - will replace direct haloWindow access)
    private var haloWindowController: HaloWindowController?

    // Output coordinator for managing AI response output
    private var outputCoordinator: OutputCoordinator?

    // Input coordinator for managing input capture
    private var inputCoordinator: InputCoordinator?

    // Conversation coordinator for managing multi-turn conversations
    private var conversationCoordinator: ConversationCoordinator?

    // Settings window (used by legacy Settings scene and WindowGroup)
    private var settingsWindow: NSWindow?

    // Permission gate active state (backward compatibility - synced with permissionCoordinator.isActive)
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

    // Note: typewriterCancellation and escapeKeyMonitor moved to OutputCoordinator
    // Note: previousFrontmostApp moved to InputCoordinator

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

        // Stop output coordinator (removes ESC key monitor)
        outputCoordinator?.stop()

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

    // MARK: - Providers Menu Management

    /// Rebuild the providers submenu with enabled providers
    private func rebuildProvidersMenu() {
        guard let core = core else { return }

        // Get enabled providers and current default
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
            if let core = core {
                haloWindowController?.configureCore(core)
                // Configure output coordinator with core and halo window controller
                outputCoordinator?.configure(core: core, haloWindowController: haloWindowController)
                // Configure conversation coordinator with core, output coordinator, and halo window controller
                conversationCoordinator?.configure(core: core, outputCoordinator: outputCoordinator, haloWindowController: haloWindowController)
                // Configure input coordinator with all dependencies (must be after outputCoordinator and conversationCoordinator)
                inputCoordinator?.configure(
                    core: core,
                    haloWindowController: haloWindowController,
                    eventHandler: eventHandler,
                    outputCoordinator: outputCoordinator,
                    conversationCoordinator: conversationCoordinator
                )
            }
            print("[Aether] All coordinators configured")

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

        // Initialize theme engine
        themeEngine = ThemeEngine()

        // Create Halo window controller (new architecture)
        haloWindowController = HaloWindowController(themeEngine: themeEngine!)
        haloWindowController?.createWindow()

        // Keep reference to raw haloWindow for gradual migration
        haloWindow = haloWindowController?.window

        // Initialize event handler
        eventHandler = EventHandler(haloWindow: haloWindow)

        // Connect event handler to halo window for error action callbacks
        haloWindowController?.setEventHandler(eventHandler!)

        // Initialize output coordinator (will configure with core after Rust core init)
        outputCoordinator = OutputCoordinator()
        outputCoordinator?.start()

        // Initialize input coordinator (will configure with core after Rust core init)
        inputCoordinator = InputCoordinator()

        // Initialize conversation coordinator
        conversationCoordinator = ConversationCoordinator()
        conversationCoordinator?.startObserving()

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

        // NOTE: Conversation notifications now observed by ConversationCoordinator
    }

    /// Handle config change notification (rebuild providers menu)
    @objc private func onConfigChanged() {
        print("[AppDelegate] Config changed, rebuilding providers menu")
        rebuildProvidersMenu()
    }

    // NOTE: Hotkey handling (handleHotkeyPressed, processWithInputMode) moved to InputCoordinator.swift
    // NOTE: Multi-turn conversation support moved to ConversationCoordinator.swift
    // NOTE: Output pipeline (performOutput, executeTypewriterOutput, etc.) moved to OutputCoordinator.swift

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

    // NOTE: handleReplaceTriggered() and handleAppendTriggered() moved to InputCoordinator.swift

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
