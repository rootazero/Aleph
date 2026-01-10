//
//  CommandCompletionManager.swift
//  Aether
//
//  Manages command completion state and interactions.
//  Uses ToolRegistry as the single source of truth for commands.
//
//  IMPORTANT: Command completion is for INPUT ASSISTANCE only.
//  It helps users type commands, but does NOT execute them.
//  Users must still double-tap hotkey to execute after command input.
//

import Combine
import Foundation
import SwiftUI

/// Manages command completion mode state and command list
///
/// This class is responsible for:
/// - Toggling command mode on/off (Cmd+Opt+/)
/// - Fetching commands from ToolRegistry (single source of truth)
/// - Filtering commands by prefix as user types
/// - Supporting namespace navigation for /mcp and /skill
/// - Listening for tool changes to auto-refresh
final class CommandCompletionManager: ObservableObject {

    // MARK: - Published State

    /// Whether command mode is currently active
    @Published private(set) var isCommandModeActive: Bool = false

    /// Current list of commands to display (filtered by prefix)
    @Published private(set) var displayedCommands: [CommandNode] = []

    /// Currently selected command index (for keyboard navigation)
    @Published var selectedIndex: Int = 0

    /// Current input prefix for filtering (e.g., "se" from "/se")
    @Published var inputPrefix: String = "" {
        didSet {
            filterCommands()
        }
    }

    /// Current parent command key for namespace navigation (e.g., "mcp", "skill")
    @Published private(set) var currentParentKey: String?

    // MARK: - Private Properties

    /// Reference to Rust Core
    private weak var core: AetherCore?

    /// All commands at current level (cached)
    private var allCommands: [CommandNode] = []

    /// Callback when user selects a command
    private var onCommandSelected: ((CommandNode) -> Void)?

    /// Subscription to tool changes notification
    private var cancellables = Set<AnyCancellable>()

    // MARK: - Initialization

    init() {
        setupNotifications()
    }

    /// Configure with AetherCore reference
    func configure(core: AetherCore?) {
        self.core = core
        refreshCommands()
    }

    // MARK: - Notification Setup

    private func setupNotifications() {
        // Listen for tool registry changes
        NotificationCenter.default.publisher(for: .toolsDidChange)
            .receive(on: DispatchQueue.main)
            .sink { [weak self] _ in
                NSLog("[CommandCompletionManager] Received toolsDidChange notification, refreshing commands")
                self?.refreshCommands()
            }
            .store(in: &cancellables)
    }

    // MARK: - Public API

    /// Toggle command mode on/off
    /// - Parameter onSelect: Callback when user selects a command
    func toggleCommandMode(onSelect: @escaping (CommandNode) -> Void) {
        if isCommandModeActive {
            deactivateCommandMode()
        } else {
            activateCommandMode(onSelect: onSelect)
        }
    }

    /// Activate command mode
    /// - Parameter onSelect: Callback when user selects a command
    func activateCommandMode(onSelect: @escaping (CommandNode) -> Void) {
        self.onCommandSelected = onSelect
        refreshCommands()
        inputPrefix = ""
        selectedIndex = 0
        currentParentKey = nil
        isCommandModeActive = true
    }

    /// Deactivate command mode
    func deactivateCommandMode() {
        isCommandModeActive = false
        onCommandSelected = nil
        inputPrefix = ""
        selectedIndex = 0
        currentParentKey = nil
    }

    /// Select the currently highlighted command
    func selectCurrentCommand() {
        guard isCommandModeActive,
              selectedIndex >= 0,
              selectedIndex < displayedCommands.count else {
            return
        }

        let command = displayedCommands[selectedIndex]

        // If namespace command with children, navigate into it
        if command.nodeType == .namespace && command.hasChildren {
            navigateIntoNamespace(command.key)
            return
        }

        onCommandSelected?(command)
        deactivateCommandMode()
    }

    /// Move selection up
    func moveSelectionUp() {
        guard isCommandModeActive, !displayedCommands.isEmpty else { return }
        selectedIndex = (selectedIndex - 1 + displayedCommands.count) % displayedCommands.count
    }

    /// Move selection down
    func moveSelectionDown() {
        guard isCommandModeActive, !displayedCommands.isEmpty else { return }
        selectedIndex = (selectedIndex + 1) % displayedCommands.count
    }

    /// Navigate into a namespace command (e.g., /mcp, /skill)
    func navigateIntoNamespace(_ parentKey: String) {
        currentParentKey = parentKey
        refreshSubcommands()
        inputPrefix = ""
        selectedIndex = 0
    }

    /// Navigate back to root commands
    func navigateToRoot() {
        currentParentKey = nil
        refreshCommands()
        inputPrefix = ""
        selectedIndex = 0
    }

    /// Check if currently in a namespace (for UI back button)
    var isInNamespace: Bool {
        currentParentKey != nil
    }

    /// Refresh commands from ToolRegistry (single source of truth)
    func refreshCommands() {
        guard let core = core else {
            NSLog("[CommandCompletionManager] refreshCommands: core is nil!")
            allCommands = []
            displayedCommands = []
            return
        }

        // Use registry-based method (single source of truth)
        allCommands = core.getRootCommandsFromRegistry()
        NSLog("[CommandCompletionManager] refreshCommands: loaded %d commands from registry", allCommands.count)
        for cmd in allCommands {
            NSLog("[CommandCompletionManager]   - /%@ (hasChildren: %@)", cmd.key, cmd.hasChildren ? "true" : "false")
        }
        filterCommands()
    }

    /// Refresh subcommands for current namespace
    private func refreshSubcommands() {
        guard let core = core, let parentKey = currentParentKey else {
            allCommands = []
            displayedCommands = []
            return
        }

        // Use registry-based method for subcommands
        allCommands = core.getSubcommandsFromRegistry(parentKey: parentKey)
        NSLog("[CommandCompletionManager] refreshSubcommands: loaded %d subcommands for /%@", allCommands.count, parentKey)
        for cmd in allCommands {
            NSLog("[CommandCompletionManager]   - %@", cmd.key)
        }
        filterCommands()
    }

    // MARK: - Private Methods

    /// Filter commands by current input prefix
    private func filterCommands() {
        NSLog("[CommandCompletionManager] filterCommands: prefix='%@', parent=%@", inputPrefix, currentParentKey ?? "root")

        if inputPrefix.isEmpty {
            displayedCommands = allCommands
            NSLog("[CommandCompletionManager] Empty prefix, showing all %d commands", allCommands.count)
        } else {
            // Local filtering by prefix
            let lowercasedPrefix = inputPrefix.lowercased()
            displayedCommands = allCommands.filter { $0.key.lowercased().hasPrefix(lowercasedPrefix) }
            NSLog("[CommandCompletionManager] Filtered by prefix: %d results", displayedCommands.count)
        }

        // Reset selection if out of bounds
        if selectedIndex >= displayedCommands.count {
            selectedIndex = max(0, displayedCommands.count - 1)
        }
    }
}

// MARK: - CommandNode Extensions

extension CommandNode {
    /// SF Symbol name for command type
    var typeIcon: String {
        switch nodeType {
        case .action:
            return "bolt.fill"
        case .prompt:
            return "text.quote"
        case .namespace:
            return "folder.fill"
        }
    }

    /// Color for command type
    var typeColor: Color {
        switch nodeType {
        case .action:
            return .orange
        case .prompt:
            return .blue
        case .namespace:
            return .purple
        }
    }

    /// Localized type description
    var typeDescription: String {
        switch nodeType {
        case .action:
            return NSLocalizedString("command.type.action", comment: "Action command type")
        case .prompt:
            return NSLocalizedString("command.type.prompt", comment: "Prompt command type")
        case .namespace:
            return NSLocalizedString("command.type.namespace", comment: "Namespace command type")
        }
    }
}
