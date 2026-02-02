//
//  HaloInputCoordinator.swift
//  Aether
//
//  Lightweight coordinator for Halo input handling.
//  Replaces the deleted MultiTurnCoordinator with simpler command-based routing.
//
//  Routes input based on prefix:
//  - "//" -> History list
//  - "/"  -> Command list (skills)
//  - Normal text -> AI processing
//

import AppKit
import SwiftUI

// MARK: - Command Item (Forward Declaration)

/// Represents a command/skill item in the command list
/// Used by HaloCommandListView to display available skills
struct CommandItem: Identifiable, Equatable {
    let id: String
    let name: String
    let description: String?
    let source: String?

    init(id: String, name: String, description: String? = nil, source: String? = nil) {
        self.id = id
        self.name = name
        self.description = description
        self.source = source
    }

    /// Create from Gateway skill info
    init(from skillInfo: GWSkillInfo) {
        self.id = skillInfo.id
        self.name = skillInfo.name
        self.description = skillInfo.description
        self.source = skillInfo.source
    }
}

// MARK: - Command List Context (Forward Declaration)

/// Context for command list state (/ command)
struct CommandListContext: Equatable {
    /// All available commands/skills
    let commands: [CommandItem]
    /// Current search query (text after /)
    var searchQuery: String
    /// Currently selected command index
    var selectedIndex: Int?

    /// Commands filtered by search query
    var filteredCommands: [CommandItem] {
        if searchQuery.isEmpty {
            return commands
        }
        return commands.filter { command in
            command.name.localizedCaseInsensitiveContains(searchQuery) ||
            (command.description?.localizedCaseInsensitiveContains(searchQuery) ?? false)
        }
    }

    init(commands: [CommandItem], searchQuery: String = "", selectedIndex: Int? = nil) {
        self.commands = commands
        self.searchQuery = searchQuery
        self.selectedIndex = selectedIndex
    }
}

// MARK: - HaloInputCoordinator

/// Lightweight coordinator for Halo input handling
///
/// Responsibilities:
/// - Handle hotkey triggers
/// - Capture clipboard content
/// - Route input based on prefix (// for history, / for commands, else AI)
/// - Manage Halo window state transitions
@MainActor
final class HaloInputCoordinator {

    // MARK: - Singleton

    static let shared = HaloInputCoordinator()

    // MARK: - Dependencies (Weak References)

    private weak var haloWindow: HaloWindow?
    private weak var core: AetherCore?

    // MARK: - State

    /// Current input text being processed
    private var currentInput: String = ""

    /// Whether we're currently processing input
    private var isProcessing: Bool = false

    // MARK: - Initialization

    private init() {
        // Private init for singleton
    }

    // MARK: - Configuration

    /// Configure the coordinator with dependencies
    ///
    /// - Parameters:
    ///   - haloWindow: The Halo overlay window
    ///   - core: The AetherCore FFI instance
    func configure(haloWindow: HaloWindow?, core: AetherCore?) {
        self.haloWindow = haloWindow
        self.core = core
        print("[HaloInputCoordinator] Configured with haloWindow: \(haloWindow != nil), core: \(core != nil)")
    }

    // MARK: - Public Methods

    /// Handle hotkey trigger (e.g., Option+Space)
    ///
    /// This method:
    /// 1. Captures current clipboard content
    /// 2. Shows Halo in listening state
    /// 3. Processes input after brief delay
    func handleHotkey() {
        print("[HaloInputCoordinator] Hotkey triggered")

        // Capture clipboard content
        let clipboardContent = NSPasteboard.general.string(forType: .string) ?? ""
        currentInput = clipboardContent.trimmingCharacters(in: .whitespacesAndNewlines)

        print("[HaloInputCoordinator] Clipboard content: \(currentInput.prefix(50))...")

        // Show Halo in listening state
        haloWindow?.updateState(.listening)
        haloWindow?.showAtCurrentPosition()

        // Process input after brief delay (allows user to see listening state)
        Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 200_000_000) // 200ms
            self?.processInput()
        }
    }

    /// Process the current input based on prefix
    func processInput() {
        guard !currentInput.isEmpty else {
            print("[HaloInputCoordinator] Empty input, hiding Halo")
            haloWindow?.hide()
            return
        }

        // Route based on prefix
        if currentInput.hasPrefix("//") {
            // History list command
            let query = String(currentInput.dropFirst(2))
            showHistoryList(query: query)
        } else if currentInput.hasPrefix("/") {
            // Command list (skills)
            let query = String(currentInput.dropFirst(1))
            showCommandList(query: query)
        } else {
            // Normal AI processing
            processAIInput(currentInput)
        }
    }

    /// Show the Halo window or bring to front if already visible
    func showOrBringToFront() {
        if haloWindow?.isVisible == true {
            haloWindow?.orderFrontRegardless()
        } else {
            haloWindow?.updateState(.listening)
            haloWindow?.showCentered()
        }
    }

    /// Cancel the current operation
    func cancel() {
        print("[HaloInputCoordinator] Cancelling current operation")

        isProcessing = false
        currentInput = ""

        // Cancel core processing if active
        core?.cancel()

        // Hide Halo
        haloWindow?.hide()
    }

    // MARK: - History List

    /// Show the history list view (// command)
    ///
    /// - Parameter query: Optional search query to filter history
    private func showHistoryList(query: String = "") {
        print("[HaloInputCoordinator] Showing history list with query: \(query)")

        // For now, show empty history list
        // TODO: Fetch actual history from conversation store
        let context = HistoryListContext(
            topics: [],
            searchQuery: query
        )

        haloWindow?.updateState(.historyList(context))
        haloWindow?.showCentered()
    }

    // MARK: - Command List

    /// Show the command list view (/ command)
    ///
    /// - Parameter query: Optional search query to filter commands
    private func showCommandList(query: String = "") {
        print("[HaloInputCoordinator] Showing command list with query: \(query)")

        // Fetch skills from Gateway if available
        Task { @MainActor [weak self] in
            guard let self = self else { return }

            let commands = await self.fetchCommands()
            let context = CommandListContext(
                commands: commands,
                searchQuery: query
            )

            // Note: commandList state will be added to HaloState in Task 2
            // For now, show as streaming with the command info
            let streamingContext = StreamingContext(
                runId: UUID().uuidString,
                text: "Commands: \(commands.count) skills available",
                phase: .thinking
            )
            self.haloWindow?.updateState(.streaming(streamingContext))
            self.haloWindow?.showCentered()

            // TODO: Update to use .commandList(context) when HaloState is extended
        }
    }

    /// Fetch available commands/skills from Gateway or core
    private func fetchCommands() async -> [CommandItem] {
        // Try Gateway first
        if GatewayManager.shared.isReady {
            do {
                let skills = try await GatewayManager.shared.client.skillsList()
                return skills.map { CommandItem(from: $0) }
            } catch {
                print("[HaloInputCoordinator] Gateway skills fetch failed: \(error)")
            }
        }

        // Fallback to core tools
        if let tools = core?.listTools() {
            return tools.map { tool in
                CommandItem(
                    id: tool.name,
                    name: tool.name,
                    description: tool.description,
                    source: "builtin"
                )
            }
        }

        return []
    }

    // MARK: - AI Processing

    /// Process normal AI input
    ///
    /// - Parameter input: The user input text to process
    private func processAIInput(_ input: String) {
        print("[HaloInputCoordinator] Processing AI input: \(input.prefix(50))...")

        guard !isProcessing else {
            print("[HaloInputCoordinator] Already processing, ignoring")
            return
        }

        isProcessing = true

        // Show streaming state
        let streamingContext = StreamingContext(
            runId: UUID().uuidString,
            text: "",
            phase: .thinking
        )
        haloWindow?.updateState(.streaming(streamingContext))

        // Process via Gateway or FFI
        if GatewayManager.shared.isReady {
            processViaGateway(input: input)
        } else {
            processViaFFI(input: input)
        }
    }

    /// Process input via Gateway WebSocket
    private func processViaGateway(input: String) {
        Task { @MainActor [weak self] in
            do {
                try await GatewayManager.shared.runAgent(input: input)
                print("[HaloInputCoordinator] Gateway processing started")
                // EventHandler will handle the streaming callbacks
            } catch {
                print("[HaloInputCoordinator] Gateway processing failed: \(error)")
                self?.isProcessing = false

                // Show error
                let errorContext = ErrorContext(
                    type: .provider,
                    message: error.localizedDescription
                )
                self?.haloWindow?.updateState(.error(errorContext))
            }
        }
    }

    /// Process input via FFI
    private func processViaFFI(input: String) {
        guard let core = core else {
            print("[HaloInputCoordinator] Core not available for FFI processing")
            isProcessing = false

            let errorContext = ErrorContext(
                type: .unknown,
                message: L("error.core_not_initialized")
            )
            haloWindow?.updateState(.error(errorContext))
            return
        }

        do {
            let options = ProcessOptions(
                appContext: nil,
                windowTitle: nil,
                topicId: nil,
                stream: true,
                attachments: nil,
                preferredLanguage: LocalizationManager.shared.currentLanguage
            )
            try core.process(input: input, options: options)
            print("[HaloInputCoordinator] FFI processing started")
            // EventHandler will handle the callbacks
        } catch {
            print("[HaloInputCoordinator] FFI processing failed: \(error)")
            isProcessing = false

            let errorContext = ErrorContext(
                type: .unknown,
                message: error.localizedDescription
            )
            haloWindow?.updateState(.error(errorContext))
        }
    }

    // MARK: - Completion Handling

    /// Called when processing completes (from EventHandler)
    func onProcessingComplete() {
        isProcessing = false
        currentInput = ""
    }

    /// Called when processing errors (from EventHandler)
    func onProcessingError(_ message: String) {
        isProcessing = false
        currentInput = ""
    }
}
