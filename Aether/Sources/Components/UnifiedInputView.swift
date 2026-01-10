//
//  UnifiedInputView.swift
//  Aether
//
//  Unified input view for the new Halo window design.
//  Combines the main text input with the SubPanel for command completion,
//  AI selectors, CLI output, and confirmations.
//
//  Part of: refactor-unified-halo-window
//

import SwiftUI
import AppKit

// MARK: - UnifiedInputView

/// Main unified input view for the Halo window
///
/// Layout:
/// ```
/// ┌─────────────────────────────────────────┐
/// │  Turn 1                      ESC 退出    │  ← Header
/// │ ┌─────────────────────────────────────┐ │
/// │ │ 输入对话或命令...                    │ │  ← Main Input
/// │ └─────────────────────────────────────┘ │
/// │  Enter 发送                             │  ← Hints
/// ├─────────────────────────────────────────┤
/// │                                         │
/// │         SubPanel (dynamic)              │  ← SubPanel
/// │                                         │
/// └─────────────────────────────────────────┘
/// ```
struct UnifiedInputView: View {
    let sessionId: String
    let turnCount: UInt32
    @ObservedObject var subPanelState: SubPanelState

    /// Callback when user submits input (Enter key)
    var onSubmit: ((String) -> Void)?

    /// Callback when user cancels (Escape key)
    var onCancel: (() -> Void)?

    /// Callback when command is selected from SubPanel
    var onCommandSelected: ((CommandNode) -> Void)?

    /// Current input text
    @State private var inputText: String = ""

    /// Reference to AetherCore for command filtering
    /// Access via AppDelegate since HaloWindow doesn't have SwiftUI environment
    private var core: AetherCore? {
        (NSApplication.shared.delegate as? AppDelegate)?.core
    }

    // MARK: - Colors

    /// Text color - white for dark background
    private let textColor = Color.white

    /// Background color (dark gray)
    private let backgroundColor = Color(white: 0.1)

    /// Accent color for input field border
    private let accentColor = Color.accentColor

    // MARK: - Body

    var body: some View {
        VStack(spacing: 0) {
            // Main input area
            mainInputArea
                .padding(12)

            // SubPanel (conditionally shown)
            if subPanelState.mode.isVisible {
                SubPanelView(
                    state: subPanelState,
                    onCommandSelected: { command in
                        onCommandSelected?(command)
                    },
                    onCancelled: {
                        subPanelState.hide()
                    }
                )
            }
        }
        .frame(width: 480)
        // Gradient background: lighter at top/bottom, darker in center for 3D depth
        .background(
            LinearGradient(
                stops: [
                    .init(color: Color(white: 0.18), location: 0),
                    .init(color: Color(white: 0.08), location: 0.5),
                    .init(color: Color(white: 0.14), location: 1)
                ],
                startPoint: .top,
                endPoint: .bottom
            )
            .opacity(0.95)
        )
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .shadow(color: .black.opacity(0.3), radius: 12, x: 0, y: 6)
        .onChange(of: inputText) { _, newValue in
            print("[UnifiedInputView] onChange triggered: '\(newValue)'")
            handleTextChange(newValue)
        }
    }

    // MARK: - Main Input Area

    private var mainInputArea: some View {
        VStack(spacing: 8) {
            // Header with turn count and ESC hint
            headerView

            // Text input field
            inputField

            // Hints
            hintsView
        }
    }

    private var headerView: some View {
        HStack {
            Text(String(format: L("conversation.turn"), turnCount + 1))
                .font(.system(size: 11, weight: .medium))
                .foregroundColor(textColor.opacity(0.7))

            Spacer()

            Text("ESC \(L("unified.exit"))")
                .font(.system(size: 10))
                .foregroundColor(textColor.opacity(0.5))
        }
    }

    private var inputField: some View {
        IMETextField(
            text: $inputText,
            placeholder: L("unified.placeholder"),
            font: .systemFont(ofSize: 18),
            textColor: .white,
            placeholderColor: NSColor.white.withAlphaComponent(0.5),
            backgroundColor: .clear,
            onSubmit: { handleSubmit() },
            onEscape: { handleCancel() },
            onTextChange: { newText in
                handleTextChange(newText)
            },
            onArrowUp: {
                // Move selection up in command completion
                subPanelState.moveSelectionUp()
            },
            onArrowDown: {
                // Move selection down in command completion
                subPanelState.moveSelectionDown()
            }
        )
        .frame(height: 26)
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .overlay(
            RoundedRectangle(cornerRadius: 6)
                .stroke(accentColor.opacity(0.4), lineWidth: 1)
        )
    }

    private var hintsView: some View {
        HStack(spacing: 16) {
            Text(L("unified.enter_to_send"))
                .font(.system(size: 10))
                .foregroundColor(textColor.opacity(0.4))

            Spacer()

            // Show command hint when "/" is typed
            if inputText.hasPrefix("/") {
                Text(L("unified.command_mode"))
                    .font(.system(size: 10))
                    .foregroundColor(accentColor.opacity(0.7))
            }
        }
    }

    // MARK: - Actions

    private func handleSubmit() {
        let text = inputText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }

        // If a command is selected in SubPanel, use that
        if case .commandCompletion = subPanelState.mode,
           let selectedCommand = subPanelState.getSelectedCommand() {
            onCommandSelected?(selectedCommand)
            return
        }

        onSubmit?(text)
        inputText = ""
        subPanelState.hide()
    }

    private func handleCancel() {
        // If SubPanel is showing, hide it first
        if subPanelState.mode.isVisible {
            subPanelState.hide()
            return
        }

        // Otherwise, cancel the unified input
        onCancel?()
    }

    private func handleTextChange(_ newText: String) {
        NSLog("[UnifiedInputView] handleTextChange: '%@'", newText)

        // Check for command prefix
        if newText.hasPrefix("/") {
            // Extract prefix after "/"
            let prefix = String(newText.dropFirst())
            NSLog("[UnifiedInputView] Command prefix detected: '%@'", prefix)

            // Get filtered commands from ToolRegistry (single source of truth)
            if let core = core {
                NSLog("[UnifiedInputView] Core available, fetching commands from registry...")
                // Use registry-based method for all tools (Builtin, MCP, Skill, Custom)
                let allCommands = core.getRootCommandsFromRegistry()
                let commands: [CommandNode]
                if prefix.isEmpty {
                    commands = allCommands
                } else {
                    // Filter locally by prefix
                    let lowercasedPrefix = prefix.lowercased()
                    commands = allCommands.filter {
                        $0.key.lowercased().hasPrefix(lowercasedPrefix) ||
                        $0.description.lowercased().contains(lowercasedPrefix)
                    }
                }

                NSLog("[UnifiedInputView] Got %d commands", commands.count)

                if !commands.isEmpty {
                    subPanelState.updateCommands(commands, inputPrefix: prefix)
                    NSLog("[UnifiedInputView] SubPanel updated with commands")
                } else {
                    subPanelState.hide()
                    NSLog("[UnifiedInputView] No commands found, hiding SubPanel")
                }
            } else {
                NSLog("[UnifiedInputView] ⚠️ Core is nil!")
            }
        } else {
            // Not a command, hide SubPanel
            if case .commandCompletion = subPanelState.mode {
                subPanelState.hide()
            }
        }
    }
}

// MARK: - Localization Helper

private func L(_ key: String) -> String {
    let localized = NSLocalizedString(key, comment: "")
    return localized == key ? fallbackString(for: key) : localized
}

private func fallbackString(for key: String) -> String {
    switch key {
    case "conversation.turn": return "Turn %d"
    case "unified.exit": return "退出"
    case "unified.placeholder": return "输入对话或命令..."
    case "unified.enter_to_send": return "Enter 发送"
    case "unified.command_mode": return "命令模式"
    default: return key
    }
}

// MARK: - Preview

#if DEBUG
struct UnifiedInputView_Previews: PreviewProvider {
    static var previews: some View {
        VStack(spacing: 20) {
            // Basic input
            UnifiedInputView(
                sessionId: "test-session",
                turnCount: 0,
                subPanelState: SubPanelState()
            )
            .previewDisplayName("Turn 1")

            // With command SubPanel
            UnifiedInputView(
                sessionId: "test-session",
                turnCount: 2,
                subPanelState: {
                    let state = SubPanelState()
                    // Note: Can't preview with actual commands without Rust core
                    return state
                }()
            )
            .previewDisplayName("Turn 3 with SubPanel")
        }
        .padding()
        .background(Color.black.opacity(0.8))
    }
}
#endif
