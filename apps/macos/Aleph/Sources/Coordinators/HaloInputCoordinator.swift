//
//  HaloInputCoordinator.swift
//  Aleph
//
//  Lightweight coordinator for Halo input handling.
//  Routes input based on prefix for command-based interaction.
//
//  Routes input based on prefix:
//  - "//" -> History list
//  - "/"  -> Command list (skills)
//  - Normal text -> AI processing
//

import AppKit

// MARK: - CommandItem (Lightweight replacement for deleted HaloStreamingTypes.CommandItem)

/// Represents a command/skill item for the command list
struct CommandItem: Identifiable {
    let id: String
    let name: String
    let description: String
    let icon: String
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
    private weak var core: AlephCore?

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
    ///   - core: The AlephCore FFI instance
    func configure(haloWindow: HaloWindow?, core: AlephCore?) {
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

        // Show Halo — WebView handles listening state display
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
            // WebView handles state display
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

        // TODO: Send history list command to WebView via Gateway
        // For now, just show the window
        haloWindow?.showCentered()
    }

    // MARK: - Command List

    /// Show the command list view (/ command)
    ///
    /// - Parameter query: Optional search query to filter commands
    private func showCommandList(query: String = "") {
        print("[HaloInputCoordinator] Showing command list with query: \(query)")

        // TODO: Send command list to WebView via Gateway
        // For now, just show the window
        Task { @MainActor [weak self] in
            guard let self = self else { return }
            let commands = await self.fetchCommands()
            print("[HaloInputCoordinator] Found \(commands.count) commands")
            self.haloWindow?.showCentered()
        }
    }

    /// Fetch available commands/skills from Gateway or core
    private func fetchCommands() async -> [CommandItem] {
        // Try Gateway first
        if GatewayManager.shared.isReady {
            do {
                let skills = try await GatewayManager.shared.client.skillsList()
                return skills.map { skill in
                    CommandItem(
                        id: skill.id,
                        name: skill.name,
                        description: skill.description ?? "",
                        icon: "command.circle.fill"
                    )
                }
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
                    icon: "wrench.and.screwdriver.fill"
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

        // WebView handles streaming display via Gateway events

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
                // WebView handles error display — log for diagnostics
                NSLog("[HaloInputCoordinator] Error: %@", error.localizedDescription)
            }
        }
    }

    /// Process input via FFI
    private func processViaFFI(input: String) {
        guard let core = core else {
            print("[HaloInputCoordinator] Core not available for FFI processing")
            isProcessing = false
            NSLog("[HaloInputCoordinator] Error: Core not initialized")
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
            NSLog("[HaloInputCoordinator] Error: %@", error.localizedDescription)
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
