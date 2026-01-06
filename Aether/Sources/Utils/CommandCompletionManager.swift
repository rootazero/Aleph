//
//  CommandCompletionManager.swift
//  Aether
//
//  Manages command completion state and interactions.
//  Provides commands from Rust Core's CommandRegistry for UI rendering.
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
/// - Fetching commands from Rust Core
/// - Filtering commands by prefix as user types
/// - Notifying when user selects a command (for input insertion)
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

    /// All root commands (cached)
    private var allCommands: [CommandNode] = []

    /// Callback when user selects a command
    private var onCommandSelected: ((CommandNode) -> Void)?

    // MARK: - Initialization

    init() {}

    /// Configure with AetherCore reference
    func configure(core: AetherCore?) {
        self.core = core
        refreshCommands()
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

    /// Refresh commands from Rust Core
    func refreshCommands() {
        guard let core = core else {
            allCommands = []
            displayedCommands = []
            return
        }

        allCommands = core.getRootCommands()
        filterCommands()
    }

    // MARK: - Private Methods

    /// Filter commands by current input prefix
    private func filterCommands() {
        if inputPrefix.isEmpty {
            displayedCommands = allCommands
        } else if let core = core {
            displayedCommands = core.filterCommands(prefix: inputPrefix)
        } else {
            // Fallback: local filtering
            let lowercasedPrefix = inputPrefix.lowercased()
            displayedCommands = allCommands.filter { $0.key.lowercased().hasPrefix(lowercasedPrefix) }
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
