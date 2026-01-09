//
//  SubPanelState.swift
//  Aether
//
//  State management for the SubPanel component in unified Halo window.
//  The SubPanel is a Raycast-style expandable area below the main input field.
//
//  Part of: refactor-unified-halo-window
//

import Combine
import SwiftUI

// MARK: - SubPanel Mode

/// Display modes for the SubPanel component
///
/// The SubPanel dynamically shows different content based on context:
/// - Command completion when input starts with "/"
/// - Selector when AI needs user choice
/// - CLI output for background operations
/// - Confirmation for authorization requests
enum SubPanelMode: Equatable {
    /// Panel is hidden (height = 0)
    case hidden

    /// Command completion list (triggered by "/" prefix)
    case commandCompletion(commands: [CommandNode], selectedIndex: Int, inputPrefix: String)

    /// AI selector for user choices (yes/no, multiple choice, etc.)
    case selector(options: [SelectorOption], prompt: String, multiSelect: Bool)

    /// CLI output stream (shows AI backend operations like a terminal)
    case cliOutput(lines: [CLIOutputLine], isStreaming: Bool)

    /// Confirmation dialog (authorization, dangerous actions)
    case confirmation(
        title: String,
        message: String,
        confirmLabel: String,
        cancelLabel: String
    )

    /// Check if panel should be visible
    var isVisible: Bool {
        if case .hidden = self { return false }
        return true
    }

    /// Get description for debugging
    var debugDescription: String {
        switch self {
        case .hidden:
            return "hidden"
        case .commandCompletion(let commands, let idx, let prefix):
            return "commandCompletion(\(commands.count) commands, selected=\(idx), prefix='\(prefix)')"
        case .selector(let options, _, let multi):
            return "selector(\(options.count) options, multi=\(multi))"
        case .cliOutput(let lines, let streaming):
            return "cliOutput(\(lines.count) lines, streaming=\(streaming))"
        case .confirmation(let title, _, _, _):
            return "confirmation('\(title)')"
        }
    }
}

// MARK: - Selector Option

/// A single option in a selector panel
struct SelectorOption: Identifiable, Equatable {
    /// Unique identifier
    let id: String

    /// Display label
    let label: String

    /// Optional description text
    let description: String?

    /// Whether this option is currently selected
    var isSelected: Bool

    /// Optional icon name (SF Symbol)
    let iconName: String?

    init(
        id: String = UUID().uuidString,
        label: String,
        description: String? = nil,
        isSelected: Bool = false,
        iconName: String? = nil
    ) {
        self.id = id
        self.label = label
        self.description = description
        self.isSelected = isSelected
        self.iconName = iconName
    }
}

// MARK: - CLI Output Line

/// A single line of CLI output
struct CLIOutputLine: Identifiable, Equatable {
    /// Unique identifier
    let id: UUID

    /// Timestamp when line was added
    let timestamp: Date

    /// Type of output (affects styling)
    let type: CLIOutputType

    /// The actual content
    let content: String

    init(
        id: UUID = UUID(),
        timestamp: Date = Date(),
        type: CLIOutputType,
        content: String
    ) {
        self.id = id
        self.timestamp = timestamp
        self.type = type
        self.content = content
    }
}

/// Types of CLI output for styling
enum CLIOutputType: Equatable {
    /// Informational message (default)
    case info

    /// Success message (green)
    case success

    /// Warning message (orange)
    case warning

    /// Error message (red)
    case error

    /// Command being executed (bold/cyan)
    case command

    /// Thinking/processing indicator (dimmed)
    case thinking

    /// Color for this output type
    var color: Color {
        switch self {
        case .info:
            return .secondary
        case .success:
            return .green
        case .warning:
            return .orange
        case .error:
            return .red
        case .command:
            return .cyan
        case .thinking:
            return .secondary.opacity(0.6)
        }
    }

    /// Font weight for this output type
    var fontWeight: Font.Weight {
        switch self {
        case .command:
            return .semibold
        case .error:
            return .medium
        default:
            return .regular
        }
    }
}

// MARK: - SubPanel State

/// Observable state manager for SubPanel
///
/// This class manages the SubPanel's display mode and provides
/// methods for updating content. It also handles keyboard navigation
/// for command completion and selector modes.
final class SubPanelState: ObservableObject {

    // MARK: - Published Properties

    /// Current display mode
    @Published var mode: SubPanelMode = .hidden {
        didSet {
            NSLog("[SubPanelState] Mode changed: %@", mode.debugDescription)
        }
    }

    // MARK: - Height Calculation

    /// Maximum height for the SubPanel
    static let maxHeight: CGFloat = 300

    /// Row height for command items
    static let commandRowHeight: CGFloat = 36

    /// Row height for selector options
    static let selectorRowHeight: CGFloat = 44

    /// Row height for CLI output lines
    static let cliLineHeight: CGFloat = 20

    /// Fixed height for confirmation dialog
    static let confirmationHeight: CGFloat = 120

    /// Header/padding height for command completion
    static let commandHeaderHeight: CGFloat = 40

    /// Header/padding height for selector
    static let selectorHeaderHeight: CGFloat = 60

    /// Padding for CLI output
    static let cliPadding: CGFloat = 20

    /// Calculate the appropriate height for current mode
    var calculatedHeight: CGFloat {
        switch mode {
        case .hidden:
            return 0

        case .commandCompletion(let commands, _, _):
            let contentHeight = CGFloat(commands.count) * Self.commandRowHeight + Self.commandHeaderHeight
            return min(contentHeight, Self.maxHeight)

        case .selector(let options, _, _):
            let contentHeight = CGFloat(options.count) * Self.selectorRowHeight + Self.selectorHeaderHeight
            return min(contentHeight, Self.maxHeight)

        case .cliOutput(let lines, _):
            let contentHeight = CGFloat(lines.count) * Self.cliLineHeight + Self.cliPadding
            return min(max(contentHeight, 60), Self.maxHeight)  // Min 60px when visible

        case .confirmation:
            return Self.confirmationHeight
        }
    }

    // MARK: - Mode Transitions

    /// Hide the SubPanel
    func hide() {
        mode = .hidden
    }

    /// Show command completion with given commands
    func showCommandCompletion(commands: [CommandNode], inputPrefix: String = "") {
        mode = .commandCompletion(commands: commands, selectedIndex: 0, inputPrefix: inputPrefix)
    }

    /// Show selector with options
    func showSelector(options: [SelectorOption], prompt: String, multiSelect: Bool = false) {
        mode = .selector(options: options, prompt: prompt, multiSelect: multiSelect)
    }

    /// Show CLI output view
    func showCLIOutput(initialLines: [CLIOutputLine] = []) {
        mode = .cliOutput(lines: initialLines, isStreaming: true)
    }

    /// Show confirmation dialog
    func showConfirmation(
        title: String,
        message: String,
        confirmLabel: String = "Confirm",
        cancelLabel: String = "Cancel"
    ) {
        mode = .confirmation(
            title: title,
            message: message,
            confirmLabel: confirmLabel,
            cancelLabel: cancelLabel
        )
    }

    // MARK: - Command Completion Navigation

    /// Move selection up in command completion
    func moveSelectionUp() {
        guard case .commandCompletion(let commands, var idx, let prefix) = mode,
              !commands.isEmpty else { return }
        idx = (idx - 1 + commands.count) % commands.count
        mode = .commandCompletion(commands: commands, selectedIndex: idx, inputPrefix: prefix)
    }

    /// Move selection down in command completion
    func moveSelectionDown() {
        guard case .commandCompletion(let commands, var idx, let prefix) = mode,
              !commands.isEmpty else { return }
        idx = (idx + 1) % commands.count
        mode = .commandCompletion(commands: commands, selectedIndex: idx, inputPrefix: prefix)
    }

    /// Get currently selected command
    func getSelectedCommand() -> CommandNode? {
        guard case .commandCompletion(let commands, let idx, _) = mode,
              idx >= 0 && idx < commands.count else { return nil }
        return commands[idx]
    }

    /// Update command list with new filter prefix
    func updateCommands(_ commands: [CommandNode], inputPrefix: String) {
        let selectedIndex: Int
        if case .commandCompletion(_, let oldIdx, _) = mode {
            // Try to preserve selection, but clamp to bounds
            selectedIndex = min(oldIdx, max(0, commands.count - 1))
        } else {
            selectedIndex = 0
        }
        mode = .commandCompletion(commands: commands, selectedIndex: selectedIndex, inputPrefix: inputPrefix)
    }

    // MARK: - Selector Navigation

    /// Toggle selection of option at index
    func toggleSelectorOption(at index: Int) {
        guard case .selector(var options, let prompt, let multi) = mode,
              index >= 0 && index < options.count else { return }

        if multi {
            // Multi-select: toggle individual option
            options[index].isSelected.toggle()
        } else {
            // Single-select: deselect others, select this one
            for i in 0..<options.count {
                options[i].isSelected = (i == index)
            }
        }

        mode = .selector(options: options, prompt: prompt, multiSelect: multi)
    }

    /// Get selected options
    func getSelectedOptions() -> [SelectorOption] {
        guard case .selector(let options, _, _) = mode else { return [] }
        return options.filter { $0.isSelected }
    }

    // MARK: - CLI Output

    /// Append a line to CLI output
    func appendCLILine(_ line: CLIOutputLine) {
        guard case .cliOutput(var lines, let streaming) = mode else { return }
        lines.append(line)

        // Keep only last 100 lines to prevent memory issues
        if lines.count > 100 {
            lines.removeFirst(lines.count - 100)
        }

        mode = .cliOutput(lines: lines, isStreaming: streaming)
    }

    /// Append a simple text line to CLI output
    func appendCLIText(_ text: String, type: CLIOutputType = .info) {
        appendCLILine(CLIOutputLine(type: type, content: text))
    }

    /// Mark CLI output as complete (stop streaming indicator)
    func completeCLIOutput() {
        guard case .cliOutput(let lines, _) = mode else { return }
        mode = .cliOutput(lines: lines, isStreaming: false)
    }

    /// Clear CLI output
    func clearCLIOutput() {
        guard case .cliOutput(_, let streaming) = mode else { return }
        mode = .cliOutput(lines: [], isStreaming: streaming)
    }
}
