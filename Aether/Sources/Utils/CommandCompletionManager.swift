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
//  ## Flat Namespace Mode
//
//  All tools (MCP, Skill, Custom) are displayed as flat root-level commands.
//  There is no namespace navigation (no /mcp or /skill prefixes).
//  Tool source is shown via badges in the UI, not via command prefixes.
//

import Combine
import Foundation
import SwiftUI

/// Manages command completion mode state and command list (Flat Namespace Mode)
///
/// This class is responsible for:
/// - Toggling command mode on/off (Cmd+Opt+/)
/// - Fetching commands from ToolRegistry (single source of truth)
/// - Filtering commands by prefix as user types
/// - Listening for tool changes to auto-refresh
///
/// In flat namespace mode:
/// - All commands are at root level
/// - No namespace navigation needed
/// - Source shown via badges (System, MCP, Skill, Custom)
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

    // MARK: - Private Properties

    /// Reference to Rust Core
    private weak var core: AetherCore?

    /// All commands (cached from ToolRegistry)
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
        isCommandModeActive = true
    }

    /// Deactivate command mode
    func deactivateCommandMode() {
        isCommandModeActive = false
        onCommandSelected = nil
        inputPrefix = ""
        selectedIndex = 0
    }

    /// Select the currently highlighted command
    func selectCurrentCommand() {
        guard isCommandModeActive,
              selectedIndex >= 0,
              selectedIndex < displayedCommands.count else {
            return
        }

        let command = displayedCommands[selectedIndex]

        // In flat namespace mode, all commands are directly invocable
        // No namespace navigation needed
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

    /// Refresh commands from ToolRegistry (single source of truth)
    func refreshCommands() {
        guard let core = core else {
            NSLog("[CommandCompletionManager] refreshCommands: core is nil!")
            allCommands = []
            displayedCommands = []
            return
        }

        // Use registry-based method (single source of truth)
        // In flat namespace mode, all commands are at root level
        allCommands = core.getRootCommandsFromRegistry()
        NSLog("[CommandCompletionManager] refreshCommands: loaded %d commands from registry (flat namespace)", allCommands.count)
        #if DEBUG
        for cmd in allCommands {
            NSLog("[CommandCompletionManager]   - /%@ [%@]", cmd.key, cmd.sourceId ?? "unknown")
        }
        #endif
        filterCommands()
    }

    // MARK: - Private Methods

    /// Filter commands by current input prefix
    private func filterCommands() {
        NSLog("[CommandCompletionManager] filterCommands: prefix='%@'", inputPrefix)

        if inputPrefix.isEmpty {
            displayedCommands = allCommands
            NSLog("[CommandCompletionManager] Empty prefix, showing all %d commands", allCommands.count)
        } else {
            // Local filtering by prefix (case-insensitive)
            let lowercasedPrefix = inputPrefix.lowercased()
            displayedCommands = allCommands.filter {
                $0.key.lowercased().hasPrefix(lowercasedPrefix) ||
                $0.description.lowercased().contains(lowercasedPrefix)
            }
            NSLog("[CommandCompletionManager] Filtered by prefix: %d results", displayedCommands.count)
        }

        // Reset selection if out of bounds
        if selectedIndex >= displayedCommands.count {
            selectedIndex = max(0, displayedCommands.count - 1)
        }
    }
}

// MARK: - CommandNode Extensions (Flat Namespace Mode)

extension CommandNode {
    /// SF Symbol name for command source (flat namespace mode)
    ///
    /// In flat namespace mode, icon represents the tool source:
    /// - System (Builtin): command.circle.fill
    /// - MCP: bolt.fill
    /// - Skill: lightbulb.fill
    /// - Custom: command
    var sourceIcon: String {
        switch sourceType {
        case .builtin, .native:
            return "command.circle.fill"
        case .mcp:
            return "bolt.fill"
        case .skill:
            return "lightbulb.fill"
        case .custom:
            return "command"
        }
    }

    /// Color for command source (flat namespace mode)
    var sourceColor: Color {
        switch sourceType {
        case .builtin, .native:
            return .blue
        case .mcp:
            return .orange
        case .skill:
            return .purple
        case .custom:
            return .green
        }
    }

    /// Badge text for command source
    var sourceBadgeText: String {
        switch sourceType {
        case .builtin, .native:
            return NSLocalizedString("source.system", comment: "System source badge")
        case .mcp:
            return "MCP"
        case .skill:
            return NSLocalizedString("source.skill", comment: "Skill source badge")
        case .custom:
            return NSLocalizedString("source.custom", comment: "Custom source badge")
        }
    }

    // MARK: - Legacy type-based properties (deprecated in flat namespace)

    /// SF Symbol name for command type
    @available(*, deprecated, message: "Use sourceIcon instead in flat namespace mode")
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
    @available(*, deprecated, message: "Use sourceColor instead in flat namespace mode")
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
    @available(*, deprecated, message: "Use sourceBadgeText instead in flat namespace mode")
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
