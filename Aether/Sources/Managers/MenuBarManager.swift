//
//  MenuBarManager.swift
//  Aether
//
//  Manager for menu bar status item and icon updates.
//  Extracted from AppDelegate to improve separation of concerns.
//

import AppKit

// MARK: - Menu Bar Manager

/// Manager for menu bar status item
///
/// Responsibilities:
/// - Create and configure the status item
/// - Update menu bar icon based on state
/// - Manage menu structure
final class MenuBarManager {

    // MARK: - Properties

    /// The status item in the menu bar
    private(set) var statusItem: NSStatusItem?

    /// Settings menu item (for enable/disable control)
    private(set) var settingsMenuItem: NSMenuItem?

    /// Default provider menu item
    private(set) var defaultProviderMenuItem: NSMenuItem?

    // MARK: - Initialization

    init() {}

    // MARK: - Setup

    /// Create and configure the menu bar status item
    ///
    /// - Parameters:
    ///   - target: Target for menu item actions
    ///   - showAboutAction: Selector for "About" action
    ///   - showSettingsAction: Selector for "Settings" action
    ///   - quitAction: Selector for "Quit" action
    ///   - debugActions: Optional debug action selectors
    func setup(
        target: AnyObject,
        showAboutAction: Selector,
        showSettingsAction: Selector,
        quitAction: Selector,
        debugActions: [(title: String, action: Selector, keyEquivalent: String)]? = nil
    ) {
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

        let aboutItem = NSMenuItem(
            title: L("menu.about"),
            action: showAboutAction,
            keyEquivalent: ""
        )
        aboutItem.target = target
        menu.addItem(aboutItem)
        menu.addItem(NSMenuItem.separator())

        // Add "Default Provider" submenu (will be populated later)
        defaultProviderMenuItem = NSMenuItem(
            title: L("menu.default_provider"),
            action: nil,
            keyEquivalent: ""
        )
        defaultProviderMenuItem?.submenu = NSMenu()
        menu.addItem(defaultProviderMenuItem!)

        menu.addItem(NSMenuItem.separator())

        // Create and store Settings menu item for enable/disable control
        settingsMenuItem = NSMenuItem(
            title: L("menu.settings"),
            action: showSettingsAction,
            keyEquivalent: ","
        )
        settingsMenuItem?.target = target
        menu.addItem(settingsMenuItem!)

        // Debug menu items (only in DEBUG builds)
        #if DEBUG
        if let debugActions = debugActions {
            menu.addItem(NSMenuItem.separator())
            for debugAction in debugActions {
                let item = NSMenuItem(
                    title: debugAction.title,
                    action: debugAction.action,
                    keyEquivalent: debugAction.keyEquivalent
                )
                item.target = target
                menu.addItem(item)
            }
        }
        #endif

        menu.addItem(NSMenuItem.separator())
        let quitItem = NSMenuItem(
            title: L("menu.quit"),
            action: quitAction,
            keyEquivalent: "q"
        )
        quitItem.target = target
        menu.addItem(quitItem)

        statusItem?.menu = menu
    }

    // MARK: - Icon Updates

    /// Update menu bar icon based on processing state
    ///
    /// - Parameter state: The current processing state
    func updateIcon(for state: ProcessingState) {
        DispatchQueue.main.async { [weak self] in
            guard let button = self?.statusItem?.button else { return }

            switch state {
            case .processing:
                button.image = NSImage(systemSymbolName: "ellipsis.circle", accessibilityDescription: "Processing")
                button.image?.isTemplate = true
            default:
                if let menuBarIcon = NSImage(named: "MenuBarIcon") {
                    menuBarIcon.isTemplate = true
                    button.image = menuBarIcon
                }
            }
        }
    }

    /// Update menu bar icon with a specific system symbol
    ///
    /// - Parameter systemSymbol: SF Symbol name
    func updateIcon(systemSymbol: String) {
        DispatchQueue.main.async { [weak self] in
            guard let button = self?.statusItem?.button else { return }
            button.image = NSImage(systemSymbolName: systemSymbol, accessibilityDescription: "Aether")
            button.image?.isTemplate = true
        }
    }

    /// Reset menu bar icon to default
    func resetIcon() {
        DispatchQueue.main.async { [weak self] in
            guard let button = self?.statusItem?.button else { return }
            if let menuBarIcon = NSImage(named: "MenuBarIcon") {
                menuBarIcon.isTemplate = true
                button.image = menuBarIcon
            }
        }
    }

    // MARK: - Menu Item Management

    /// Enable or disable the settings menu item
    ///
    /// - Parameter enabled: Whether to enable the item
    func setSettingsEnabled(_ enabled: Bool) {
        settingsMenuItem?.isEnabled = enabled
    }

    /// Rebuild the providers submenu
    ///
    /// - Parameters:
    ///   - providers: List of (id, displayName) tuples
    ///   - currentSelection: Currently selected provider ID
    ///   - target: Target for menu item actions
    ///   - action: Selector for provider selection
    func rebuildProvidersMenu(
        providers: [(id: String, displayName: String)],
        currentSelection: String?,
        target: AnyObject,
        action: Selector
    ) {
        guard let submenu = defaultProviderMenuItem?.submenu else { return }
        submenu.removeAllItems()

        if providers.isEmpty {
            let placeholder = NSMenuItem(title: L("menu.no_providers"), action: nil, keyEquivalent: "")
            placeholder.isEnabled = false
            submenu.addItem(placeholder)
            return
        }

        for provider in providers {
            let item = NSMenuItem(
                title: provider.displayName,
                action: action,
                keyEquivalent: ""
            )
            item.target = target
            item.representedObject = provider.id

            // Mark current selection
            if provider.id == currentSelection {
                item.state = .on
            }

            submenu.addItem(item)
        }
    }
}
