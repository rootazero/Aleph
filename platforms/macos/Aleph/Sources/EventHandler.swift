//
//  EventHandler.swift
//  Aether
//
//  Implements AlephEventHandler protocol to receive callbacks from Rust core.
//  This handler works with the rig-core based AetherCore.
//

import AppKit
import SwiftUI

/// Event Handler implementing the simplified rig-core callback protocol
///
/// This handler provides callbacks for:
/// - AI thinking/processing states
/// - Tool execution lifecycle
/// - Streaming response chunks
/// - Completion and error states
/// - Memory storage confirmation
///
/// Thread Safety:
/// - Marked as @unchecked Sendable because callbacks are invoked from Rust background threads
/// - All UI updates are dispatched to MainActor via Task
/// - Weak references prevent retain cycles and allow safe access from any thread
class EventHandler: AlephEventHandler, @unchecked Sendable {

    // MARK: - Dependencies

    // Weak reference to Halo window to avoid retain cycle
    private weak var haloWindow: HaloWindow?

    // Weak reference to AetherCore for cancellation functionality
    private weak var core: AetherCore?


    // MARK: - State

    // Accumulated text for streaming responses
    private var accumulatedText: String = ""

    // Screen Recording permission toast cooldown
    private var lastScreenRecordingToastAt: Date?
    private let screenRecordingToastCooldown: TimeInterval = 600

    // Current tool being executed (for UI feedback)
    private var currentToolName: String?

    /// Check whether to use Gateway WebSocket instead of FFI
    /// Must be called from MainActor context
    @MainActor
    private func useGateway() -> Bool {
        GatewayManager.shared.isReady
    }

    // MARK: - Agentic Session State (Phase 5)

    /// Current agentic session ID
    private var currentAgenticSessionId: String?

    /// Current iteration in the agentic loop
    private var currentIteration: UInt32 = 0

    /// Current plan steps (for progress tracking)
    private var currentPlanSteps: [String] = []

    /// Completed step count
    private var completedStepCount: Int = 0

    /// Active tool calls being tracked
    private var activeToolCalls: Set<String> = []

    /// Whether we're in an active agentic session
    private var isInAgenticSession: Bool {
        currentAgenticSessionId != nil
    }

    // MARK: - Initialization

    init(haloWindow: HaloWindow?) {
        self.haloWindow = haloWindow
    }

    // Set AetherCore reference after initialization
    func setCore(_ core: AetherCore) {
        self.core = core
    }

    // Set HaloWindow reference (for DependencyContainer use)
    func setHaloWindow(_ window: HaloWindow?) {
        self.haloWindow = window
    }


    // MARK: - AlephEventHandler Protocol

    /// Called when AI is processing/thinking
    func onThinking() {
        print("[EventHandler] AI thinking...")

        Task { @MainActor [weak self] in
            guard let self = self else { return }
            self.haloWindow?.showProcessingWithAI(providerName: nil)
            self.haloWindow?.showAtCurrentPosition()
        }
    }

    /// Called when a tool execution starts
    /// - Parameter toolName: Name of the tool being executed
    func onToolStart(toolName: String) {
        print("[EventHandler] Tool started: \(toolName)")
        currentToolName = toolName

        Task { @MainActor [weak self] in
            guard let self = self else { return }
            // Show processing state with tool name
            self.haloWindow?.showProcessing(streamingText: "Using \(toolName)...")
            self.haloWindow?.showAtCurrentPosition()
        }
    }

    /// Called when a tool execution completes
    /// - Parameters:
    ///   - toolName: Name of the tool that completed
    ///   - result: Result from the tool (may be truncated for display)
    func onToolResult(toolName: String, result: String) {
        print("[EventHandler] Tool result: \(toolName) - \(result.prefix(100))...")
        currentToolName = nil
        handleSnapshotPermissionToast(toolName: toolName, result: result)

        Task { @MainActor [weak self] in
            guard let self = self else { return }
            // Update state to show tool completed
            self.haloWindow?.showProcessing(streamingText: "Completed: \(toolName)")
        }
    }

    private func handleSnapshotPermissionToast(toolName: String, result: String) {
        guard toolName == "snapshot_capture" else { return }
        guard result.contains("SCREEN_RECORDING_REQUIRED") else { return }

        let now = Date()
        if let lastShown = lastScreenRecordingToastAt,
           now.timeIntervalSince(lastShown) < screenRecordingToastCooldown {
            return
        }
        lastScreenRecordingToastAt = now

        showToast(
            type: .warning,
            title: L("permission.screen_recording.title"),
            message: L("permission.screen_recording.description"),
            autoDismiss: false,
            actionTitle: L("permission.open_settings"),
            onAction: {
                PermissionChecker.openSystemSettings(for: .screenRecording)
            }
        )
    }

    /// Called for each streaming response chunk
    /// - Parameter text: The incremental response text (delta from last chunk)
    /// IMPORTANT: After Rust-side fix, this now receives INCREMENTAL content, not accumulated
    func onStreamChunk(text: String) {
        // Accumulate the incremental text
        accumulatedText += text

        // Log for debugging on large chunks
        let isFirstOrLargeChange = text.count > 50
        if isFirstOrLargeChange {
            print("[EventHandler] Stream chunk (delta): \(text.prefix(80))... (delta: \(text.count) chars, total: \(accumulatedText.count) chars)")
        }

        Task { @MainActor [weak self] in
            guard let self = self else { return }
            self.haloWindow?.showProcessing(streamingText: self.accumulatedText)
        }
    }

    /// Called when processing completes successfully
    /// - Parameter response: The final response text
    func onComplete(response: String) {
        print("[EventHandler] Processing complete: \(response.prefix(100))...")

        // Reset state
        accumulatedText = ""
        currentToolName = nil

        Task { @MainActor [weak self] in
            guard let self = self else { return }

            // Show success state then auto-hide
            self.haloWindow?.showSuccess(message: nil)

            // Auto-hide after brief display
            try? await Task.sleep(seconds: 0.8)
            self.haloWindow?.hide()
        }
    }

    /// Called when an error occurs during processing
    /// - Parameter message: Error message describing what went wrong
    func onError(message: String) {
        print("[EventHandler] Error: \(message)")

        // Reset state
        accumulatedText = ""
        currentToolName = nil

        Task { @MainActor [weak self] in
            guard let self = self else { return }
            // Show error notification
            self.showErrorNotification(message: message)
        }
    }

    /// Called when memory is stored successfully
    func onMemoryStored() {
        print("[EventHandler] Memory stored")

        // Subtle feedback - could show toast or just log
        Task { @MainActor in
            // Optionally show brief memory stored indicator
            // For now, just log - memory storage is typically transparent to user
        }
    }

    /// Called when agent execution mode is detected
    /// - Parameter task: The executable task that was classified
    func onAgentModeDetected(task: ExecutableTaskFfi) {
        print("[EventHandler] Agent mode detected: category=\(task.category), action=\(task.action), confidence=\(task.confidence)")

        Task { @MainActor in
            // Log the detection for now
            // TODO: Integrate with AgentPlanView when AI returns __agent_plan__ JSON
            print("[EventHandler] Executable task: \(task.category) - \(task.action)")
            if let target = task.target {
                print("[EventHandler] Target: \(target)")
            }
        }
    }

    // MARK: - Hot-Reload Callbacks

    /// Called when tool registry is updated (MCP server added/removed, skill installed/deleted)
    /// - Parameter toolCount: The new total number of registered tools
    func onToolsChanged(toolCount: UInt32) {
        print("[EventHandler] Tools changed: \(toolCount) tools registered")

        Task { @MainActor in
            // Post notification for any UI that needs to refresh tool lists
            NotificationCenter.default.post(
                name: Notification.Name("ToolsDidChange"),
                object: nil,
                userInfo: ["toolCount": toolCount]
            )
        }
    }

    /// Called when MCP servers have finished starting
    /// - Parameter report: Report containing succeeded and failed servers
    func onMcpStartupComplete(report: McpStartupReportFfi) {
        print("[EventHandler] MCP startup complete: \(report.succeededServers.count) succeeded, \(report.failedServers.count) failed")

        Task { @MainActor in
            // Post notification with startup report
            NotificationCenter.default.post(
                name: Notification.Name("McpStartupComplete"),
                object: nil,
                userInfo: ["report": report]
            )

            // If there are failed servers, show a toast notification
            if !report.failedServers.isEmpty {
                let failedNames = report.failedServers.map { $0.serverName }.joined(separator: ", ")
                print("[EventHandler] MCP servers failed to start: \(failedNames)")
            }
        }
    }

    /// Called when runtime updates are available (Phase 7 - Runtime Manager)
    /// - Parameter updates: List of runtimes with available updates
    func onRuntimeUpdatesAvailable(updates: [RuntimeUpdateInfo]) {
        print("[EventHandler] Runtime updates available: \(updates.count) updates")
        for update in updates {
            print("[EventHandler]   \(update.runtimeId): \(update.currentVersion) → \(update.latestVersion)")
        }

        Task { @MainActor [weak self] in
            guard let self = self else { return }
            // Post notification for UI components
            NotificationCenter.default.post(
                name: .runtimeUpdatesAvailable,
                object: nil,
                userInfo: ["updates": updates]
            )

            // Show toast notification about available updates
            if !updates.isEmpty {
                let message = updates.count == 1
                    ? L("notification.runtime_update_single", updates[0].runtimeId, updates[0].latestVersion)
                    : L("notification.runtime_updates_multiple", String(updates.count))

                self.showToast(
                    type: .info,
                    title: L("notification.runtime_updates_title"),
                    message: message,
                    autoDismiss: true
                )
            }
        }
    }

    // MARK: - Agentic Loop Callbacks (Phase 5)

    /// Called when a new session starts
    /// - Parameter sessionId: Unique identifier for the session
    func onSessionStarted(sessionId: String) {
        print("[EventHandler] Session started: \(sessionId)")

        // Track session state
        currentAgenticSessionId = sessionId
        currentIteration = 0
        currentPlanSteps = []
        completedStepCount = 0
        activeToolCalls.removeAll()

        Task { @MainActor [weak self] in
            guard let self = self else { return }
            // Post notification for UI components
            NotificationCenter.default.post(
                name: .agenticSessionStarted,
                object: nil,
                userInfo: ["sessionId": sessionId]
            )

            // Show processing state
            self.haloWindow?.showProcessingWithAI(providerName: L("halo.agentic_session"))
            self.haloWindow?.showAtCurrentPosition()
        }
    }

    /// Called when a tool call begins
    /// - Parameters:
    ///   - callId: Unique identifier for this call
    ///   - toolName: Name of the tool being called
    func onToolCallStarted(callId: String, toolName: String) {
        print("[EventHandler] Tool call started: \(toolName) (id: \(callId))")

        // Track active tool call
        activeToolCalls.insert(callId)
        currentToolName = toolName

        Task { @MainActor [weak self] in
            guard let self = self else { return }
            // Post notification
            NotificationCenter.default.post(
                name: .agenticToolCallStarted,
                object: nil,
                userInfo: [
                    "sessionId": self.currentAgenticSessionId ?? "",
                    "callId": callId,
                    "toolName": toolName
                ]
            )

            // Update Halo to show tool execution
            if self.isInAgenticSession {
                // Show agent progress with current tool
                let progress = self.currentPlanSteps.isEmpty ? 0.0 :
                    Float(self.completedStepCount) / Float(self.currentPlanSteps.count)
                self.haloWindow?.showAgentProgress(
                    planId: self.currentAgenticSessionId ?? "",
                    progress: progress,
                    currentOperation: toolName,
                    completedCount: self.completedStepCount,
                    totalCount: self.currentPlanSteps.count
                )
            } else {
                self.haloWindow?.showProcessing(streamingText: "🔧 \(toolName)")
            }
        }
    }

    /// Called when a tool call completes successfully
    /// - Parameters:
    ///   - callId: Unique identifier for this call
    ///   - output: Output from the tool
    func onToolCallCompleted(callId: String, output: String) {
        print("[EventHandler] Tool call completed: \(callId) - \(output.prefix(100))...")

        // Update tracking
        activeToolCalls.remove(callId)
        completedStepCount += 1
        let toolName = currentToolName ?? "tool"
        currentToolName = nil
        handleSnapshotPermissionToast(toolName: toolName, result: output)

        Task { @MainActor [weak self] in
            guard let self = self else { return }
            // Post notification
            NotificationCenter.default.post(
                name: .agenticToolCallCompleted,
                object: nil,
                userInfo: [
                    "sessionId": self.currentAgenticSessionId ?? "",
                    "callId": callId,
                    "toolName": toolName,
                    "output": String(output.prefix(500))
                ]
            )

            // Update progress
            if self.isInAgenticSession && !self.currentPlanSteps.isEmpty {
                let progress = Float(self.completedStepCount) / Float(self.currentPlanSteps.count)
                self.haloWindow?.showAgentProgress(
                    planId: self.currentAgenticSessionId ?? "",
                    progress: progress,
                    currentOperation: "✓ \(toolName)",
                    completedCount: self.completedStepCount,
                    totalCount: self.currentPlanSteps.count
                )
            }
        }
    }

    /// Called when a tool call fails
    /// - Parameters:
    ///   - callId: Unique identifier for this call
    ///   - error: Error message
    ///   - isRetryable: Whether the call can be retried
    func onToolCallFailed(callId: String, error: String, isRetryable: Bool) {
        print("[EventHandler] Tool call failed: \(callId) - \(error) (retryable: \(isRetryable))")

        // Update tracking
        activeToolCalls.remove(callId)
        let toolName = currentToolName ?? "tool"

        Task { @MainActor [weak self] in
            guard let self = self else { return }
            // Post notification
            NotificationCenter.default.post(
                name: .agenticToolCallFailed,
                object: nil,
                userInfo: [
                    "sessionId": self.currentAgenticSessionId ?? "",
                    "callId": callId,
                    "toolName": toolName,
                    "error": error,
                    "isRetryable": isRetryable
                ]
            )

            // Show error in progress (if retryable, indicate retry)
            if self.isInAgenticSession {
                let statusText = isRetryable ? "⟳ \(toolName) (retrying...)" : "✗ \(toolName)"
                let progress = self.currentPlanSteps.isEmpty ? 0.0 :
                    Float(self.completedStepCount) / Float(self.currentPlanSteps.count)
                self.haloWindow?.showAgentProgress(
                    planId: self.currentAgenticSessionId ?? "",
                    progress: progress,
                    currentOperation: statusText,
                    completedCount: self.completedStepCount,
                    totalCount: self.currentPlanSteps.count
                )
            }
        }
    }

    /// Called when the agentic loop progresses
    /// - Parameters:
    ///   - sessionId: Session identifier
    ///   - iteration: Current iteration number
    ///   - status: Status message
    func onLoopProgress(sessionId: String, iteration: UInt32, status: String) {
        print("[EventHandler] Loop progress: session=\(sessionId), iteration=\(iteration), status=\(status)")

        // Update iteration count
        currentIteration = iteration

        Task { @MainActor [weak self] in
            guard let self = self else { return }
            // Post notification
            NotificationCenter.default.post(
                name: .agenticLoopProgress,
                object: nil,
                userInfo: [
                    "sessionId": sessionId,
                    "iteration": iteration,
                    "status": status
                ]
            )

            // Update Halo with iteration info
            if self.isInAgenticSession {
                let progress = self.currentPlanSteps.isEmpty ? 0.0 :
                    Float(self.completedStepCount) / Float(self.currentPlanSteps.count)
                self.haloWindow?.showAgentProgress(
                    planId: sessionId,
                    progress: progress,
                    currentOperation: status,
                    completedCount: self.completedStepCount,
                    totalCount: self.currentPlanSteps.count
                )
            }
        }
    }

    /// Called when a task plan is created
    /// - Parameters:
    ///   - sessionId: Session identifier
    ///   - steps: List of step descriptions
    func onPlanCreated(sessionId: String, steps: [String]) {
        print("[EventHandler] Plan created: session=\(sessionId), \(steps.count) steps")
        for (index, step) in steps.enumerated() {
            print("[EventHandler]   Step \(index + 1): \(step)")
        }

        // Store plan steps for progress tracking
        currentPlanSteps = steps
        completedStepCount = 0

        Task { @MainActor [weak self] in
            guard let self = self else { return }
            // Post notification
            NotificationCenter.default.post(
                name: .agenticPlanCreated,
                object: nil,
                userInfo: [
                    "sessionId": sessionId,
                    "steps": steps
                ]
            )

            // Show plan progress view
            let stepProgress = steps.enumerated().map { index, description in
                PlanStepProgressInfo(
                    index: UInt32(index + 1),
                    toolName: "",
                    description: description,
                    status: .pending,
                    resultPreview: nil,
                    errorMessage: nil
                )
            }

            self.haloWindow?.showPlanProgress(progressInfo: PlanProgressInfo(
                planId: sessionId,
                description: L("halo.executing_plan"),
                totalSteps: UInt32(steps.count),
                currentStep: 0,
                currentStepName: steps.first ?? "",
                stepProgress: stepProgress,
                status: .running,
                errorMessage: nil
            ))
            self.haloWindow?.showAtCurrentPosition()
        }
    }

    /// Called when a session completes
    /// - Parameters:
    ///   - sessionId: Session identifier
    ///   - summary: Completion summary
    func onSessionCompleted(sessionId: String, summary: String) {
        print("[EventHandler] Session completed: \(sessionId) - \(summary)")

        // Clear session state
        let wasInSession = currentAgenticSessionId == sessionId
        currentAgenticSessionId = nil
        currentIteration = 0
        currentPlanSteps = []
        completedStepCount = 0
        activeToolCalls.removeAll()

        Task { @MainActor [weak self] in
            guard let self = self else { return }
            // Post notification
            NotificationCenter.default.post(
                name: .agenticSessionCompleted,
                object: nil,
                userInfo: [
                    "sessionId": sessionId,
                    "summary": summary
                ]
            )

            // Show success toast if we were tracking this session
            if wasInSession {
                self.haloWindow?.showSuccess(message: summary)

                // Auto-hide after brief display
                try? await Task.sleep(seconds: 1.5)
                self.haloWindow?.hide()
            }
        }
    }

    /// Called when a sub-agent starts
    /// - Parameters:
    ///   - parentSessionId: Parent session identifier
    ///   - childSessionId: Child session identifier
    ///   - agentId: Agent identifier
    func onSubagentStarted(parentSessionId: String, childSessionId: String, agentId: String) {
        print("[EventHandler] Sub-agent started: agent=\(agentId), parent=\(parentSessionId), child=\(childSessionId)")

        Task { @MainActor [weak self] in
            guard let self = self else { return }
            // Post notification
            NotificationCenter.default.post(
                name: .agenticSubagentStarted,
                object: nil,
                userInfo: [
                    "parentSessionId": parentSessionId,
                    "childSessionId": childSessionId,
                    "agentId": agentId
                ]
            )

            // Show sub-agent indicator in progress
            if self.isInAgenticSession {
                let progress = self.currentPlanSteps.isEmpty ? 0.0 :
                    Float(self.completedStepCount) / Float(self.currentPlanSteps.count)
                self.haloWindow?.showAgentProgress(
                    planId: parentSessionId,
                    progress: progress,
                    currentOperation: "🤖 \(agentId)",
                    completedCount: self.completedStepCount,
                    totalCount: self.currentPlanSteps.count
                )
            }
        }
    }

    /// Called when a sub-agent completes
    /// - Parameters:
    ///   - childSessionId: Child session identifier
    ///   - success: Whether it succeeded
    ///   - summary: Completion summary
    func onSubagentCompleted(childSessionId: String, success: Bool, summary: String) {
        print("[EventHandler] Sub-agent completed: \(childSessionId) - success=\(success), summary=\(summary)")

        Task { @MainActor [weak self] in
            guard let self = self else { return }
            // Post notification
            NotificationCenter.default.post(
                name: .agenticSubagentCompleted,
                object: nil,
                userInfo: [
                    "childSessionId": childSessionId,
                    "success": success,
                    "summary": summary
                ]
            )

            // Update progress with sub-agent result
            if self.isInAgenticSession {
                let statusIcon = success ? "✓" : "✗"
                let truncatedSummary = summary.count > 30 ? String(summary.prefix(30)) + "..." : summary
                let progress = self.currentPlanSteps.isEmpty ? 0.0 :
                    Float(self.completedStepCount) / Float(self.currentPlanSteps.count)
                self.haloWindow?.showAgentProgress(
                    planId: self.currentAgenticSessionId ?? "",
                    progress: progress,
                    currentOperation: "\(statusIcon) \(truncatedSummary)",
                    completedCount: self.completedStepCount,
                    totalCount: self.currentPlanSteps.count
                )
            }
        }
    }

    // MARK: - DAG Plan Confirmation Callback

    /// Called when a DAG task plan requires user confirmation before execution
    /// - Parameters:
    ///   - planId: Unique identifier for this confirmation request
    ///   - plan: The task plan that needs confirmation
    func onPlanConfirmationRequired(planId: String, plan: DagTaskPlan) {
        print("[EventHandler] Plan confirmation required: planId=\(planId), tasks=\(plan.tasks.count), title=\(plan.title)")

        // Extract plan data to Sendable-compatible types before crossing actor boundary
        let planInfo = PlanConfirmationInfo(from: plan)

        Task { @MainActor [weak self] in
            guard let self = self, let core = self.core else {
                print("[EventHandler] Error: EventHandler or core is nil, auto-cancelling")
                _ = self?.core?.confirmTaskPlan(planId: planId, confirmed: false)
                return
            }

            // Post notification for UI components
            NotificationCenter.default.post(
                name: .dagPlanConfirmationRequired,
                object: nil,
                userInfo: [
                    "planId": planId,
                    "planInfo": planInfo,
                    "core": core
                ]
            )

            // Show confirmation dialog
            self.showPlanConfirmationDialogWithInfo(planId: planId, planInfo: planInfo, core: core)
        }
    }

    /// Sendable-compatible plan information for crossing actor boundaries
    struct PlanConfirmationInfo: Sendable {
        let id: String
        let title: String
        let tasks: [(id: String, name: String, riskLevel: String)]
        let requiresConfirmation: Bool

        init(from plan: DagTaskPlan) {
            self.id = plan.id
            self.title = plan.title
            self.tasks = plan.tasks.map { (id: $0.id, name: $0.name, riskLevel: $0.riskLevel) }
            self.requiresConfirmation = plan.requiresConfirmation
        }
    }

    /// Show a confirmation dialog for the DAG task plan (Halo mode)
    @MainActor
    private func showPlanConfirmationDialogWithInfo(planId: String, planInfo: PlanConfirmationInfo, core: AetherCore) {
        // Create a simple alert for confirmation
        let alert = NSAlert()
        alert.messageText = L("dag.confirm_title")
        alert.informativeText = formatPlanInfoDescription(planInfo)
        alert.alertStyle = .warning
        alert.addButton(withTitle: L("dag.confirm_execute"))
        alert.addButton(withTitle: L("dag.confirm_cancel"))

        // Show the alert
        let response = alert.runModal()

        // Handle user decision
        let confirmed = response == .alertFirstButtonReturn
        print("[EventHandler] Plan confirmation decision: planId=\(planId), confirmed=\(confirmed)")

        // Send decision back to Rust - prefer Gateway RPC
        if useGateway() {
            Task {
                do {
                    let success = try await GatewayManager.shared.client.agentConfirmPlan(
                        planId: planId,
                        confirmed: confirmed
                    )
                    if !success {
                        print("[EventHandler] Warning: Plan confirmation returned false: \(planId)")
                    }
                } catch {
                    print("[EventHandler] Gateway plan confirmation failed, falling back to FFI: \(error)")
                    // Fallback to FFI
                    let success = core.confirmTaskPlan(planId: planId, confirmed: confirmed)
                    if !success {
                        print("[EventHandler] Warning: Plan confirmation may have expired or not found: \(planId)")
                    }
                }
            }
        } else {
            // FFI fallback
            let success = core.confirmTaskPlan(planId: planId, confirmed: confirmed)
            if !success {
                print("[EventHandler] Warning: Plan confirmation may have expired or not found: \(planId)")
            }
        }
    }

    /// Format plan info description for display in confirmation dialog
    private func formatPlanInfoDescription(_ planInfo: PlanConfirmationInfo) -> String {
        var description = "\(planInfo.title)\n\n"
        description += L("dag.tasks_header") + ":\n"

        for (index, task) in planInfo.tasks.enumerated() {
            let riskIcon = task.riskLevel == "high" ? "⚠️" : "✓"
            description += "\(index + 1). \(riskIcon) \(task.name)\n"
        }

        if planInfo.requiresConfirmation {
            description += "\n" + L("dag.high_risk_warning")
        }

        return description
    }

    // MARK: - User Input Request Callback

    /// Called when the agent loop needs user input
    /// - Parameters:
    ///   - requestId: Unique identifier for this input request
    ///   - question: The question to ask the user
    ///   - options: Optional list of choices (empty if free-form input)
    func onUserInputRequest(requestId: String, question: String, options: [String]) {
        print("[EventHandler] User input requested: requestId=\(requestId), question=\(question), options=\(options)")

        Task { @MainActor [weak self] in
            guard let self = self, let core = self.core else {
                print("[EventHandler] Error: EventHandler or core is nil, returning empty response")
                _ = self?.core?.respondToUserInput(requestId: requestId, response: "")
                return
            }

            // Post notification for UI components
            NotificationCenter.default.post(
                name: .userInputRequested,
                object: nil,
                userInfo: [
                    "requestId": requestId,
                    "question": question,
                    "options": options,
                    "core": core
                ]
            )

            // Show input dialog
            self.showUserInputDialog(requestId: requestId, question: question, options: options, core: core)
        }
    }

    /// Show a user input dialog in Halo mode
    @MainActor
    private func showUserInputDialog(requestId: String, question: String, options: [String], core: AetherCore) {
        if options.isEmpty {
            // Free-form text input
            let alert = NSAlert()
            alert.messageText = L("user_input.title")
            alert.informativeText = question
            alert.alertStyle = .informational
            alert.addButton(withTitle: L("common.confirm"))
            alert.addButton(withTitle: L("common.cancel"))

            // Add text field for input
            let textField = NSTextField(frame: NSRect(x: 0, y: 0, width: 300, height: 24))
            textField.placeholderString = L("user_input.placeholder")
            alert.accessoryView = textField

            let response = alert.runModal()
            let userResponse = response == .alertFirstButtonReturn ? textField.stringValue : ""

            print("[EventHandler] User input response: requestId=\(requestId), response=\(userResponse)")

            // Send response back to Rust - prefer Gateway RPC
            sendUserInputResponse(requestId: requestId, response: userResponse, core: core)
        } else {
            // Multiple choice options
            let alert = NSAlert()
            alert.messageText = L("user_input.title")
            alert.informativeText = question
            alert.alertStyle = .informational

            // Add buttons for each option (up to 3)
            for option in options.prefix(3) {
                alert.addButton(withTitle: option)
            }
            alert.addButton(withTitle: L("common.cancel"))

            let response = alert.runModal()
            let buttonIndex = response.rawValue - NSApplication.ModalResponse.alertFirstButtonReturn.rawValue

            var userResponse = ""
            if buttonIndex >= 0 && buttonIndex < options.count {
                userResponse = options[buttonIndex]
            }

            print("[EventHandler] User input response: requestId=\(requestId), response=\(userResponse)")

            // Send response back to Rust - prefer Gateway RPC
            sendUserInputResponse(requestId: requestId, response: userResponse, core: core)
        }
    }

    /// Send user input response via Gateway RPC with FFI fallback
    @MainActor
    private func sendUserInputResponse(requestId: String, response: String, core: AetherCore) {
        if useGateway() {
            Task {
                do {
                    let success = try await GatewayManager.shared.client.agentRespondToInput(
                        requestId: requestId,
                        response: response
                    )
                    if !success {
                        print("[EventHandler] Warning: User input response returned false: \(requestId)")
                    }
                } catch {
                    print("[EventHandler] Gateway user input response failed, falling back to FFI: \(error)")
                    // Fallback to FFI
                    _ = core.respondToUserInput(requestId: requestId, response: response)
                }
            }
        } else {
            // FFI fallback
            _ = core.respondToUserInput(requestId: requestId, response: response)
        }
    }

    // MARK: - Error Notification

    private func showErrorNotification(message: String) {
        // Use error state in Halo
        Task { @MainActor [weak self] in
            guard let self = self else { return }
            self.haloWindow?.showError(
                ErrorContext(
                    type: .unknown,
                    message: message
                ),
                onRetry: nil,
                onDismiss: { [weak self] in
                    Task { @MainActor in
                        self?.haloWindow?.hide()
                    }
                }
            )
        }
    }

    // MARK: - Helper Methods

    /// Cancel the current operation
    func cancel() {
        core?.cancel()
        accumulatedText = ""
        currentToolName = nil

        Task { @MainActor [weak self] in
            self?.haloWindow?.hide()
        }
    }

    /// Reset handler state
    func reset() {
        accumulatedText = ""
        currentToolName = nil

        // Reset agentic session state
        currentAgenticSessionId = nil
        currentIteration = 0
        currentPlanSteps = []
        completedStepCount = 0
        activeToolCalls.removeAll()
    }

    // MARK: - Part Update Callback (Message Flow Rendering)

    /// Called when a session part is added, updated, or removed
    ///
    /// This callback enables real-time message flow rendering:
    /// - Tool calls with status transitions (Running -> Completed/Failed)
    /// - Streaming AI responses via delta field
    /// - Sub-agent progress display
    ///
    /// - Parameter event: Part update event from Rust core
    func onPartUpdate(event: PartUpdateEventFfi) {
        print("[EventHandler] Part update: partId=\(event.partId), type=\(event.partType), event=\(event.eventType)")

        // Copy event data to avoid data race issues
        // Note: PartEventTypeFfi is not Sendable, so we convert to/from string
        let sessionId = event.sessionId
        let partId = event.partId
        let partType = event.partType
        let eventTypeString = eventTypeToString(event.eventType)
        let partJson = event.partJson
        let delta = event.delta
        let timestamp = event.timestamp

        Task { @MainActor in
            let eventType = stringToEventType(eventTypeString)

            // Post notification for UI components
            NotificationCenter.default.post(
                name: .partUpdated,
                object: nil,
                userInfo: [
                    "sessionId": sessionId,
                    "partId": partId,
                    "partType": partType,
                    "eventType": eventType,
                    "partJson": partJson,
                    "delta": delta as Any,
                    "timestamp": timestamp
                ]
            )
        }
    }

    /// Convert PartEventTypeFfi to string for thread-safe passing
    private func eventTypeToString(_ eventType: PartEventTypeFfi) -> String {
        switch eventType {
        case .added: return "added"
        case .updated: return "updated"
        case .removed: return "removed"
        }
    }

    /// Convert string back to PartEventTypeFfi
    private func stringToEventType(_ str: String) -> PartEventTypeFfi {
        switch str {
        case "added": return .added
        case "updated": return .updated
        case "removed": return .removed
        default: return .updated
        }
    }

    // MARK: - Toast Display

    /// Timer for auto-dismissing toasts
    private var toastDismissTimer: Timer?

    /// Show a toast notification to the user
    /// - Parameters:
    ///   - type: The toast type (info, warning, error)
    ///   - title: Toast title
    ///   - message: Toast message
    ///   - autoDismiss: Whether to auto-dismiss (default: true for info)
    func showToast(
        type: ToastType,
        title: String,
        message: String,
        autoDismiss: Bool = true,
        actionTitle: String? = nil,
        onAction: (() -> Void)? = nil
    ) {
        print("[EventHandler] Showing toast: \(type) - \(title)")

        // Cancel any existing dismiss timer
        toastDismissTimer?.invalidate()
        toastDismissTimer = nil

        Task { @MainActor [weak self] in
            guard let self = self else { return }
            // Update Halo state using legacy bridge
            let shouldAutoDismiss = autoDismiss && type == .info && actionTitle == nil
            self.haloWindow?.showToast(
                type: type,
                title: title,
                message: message,
                autoDismiss: shouldAutoDismiss,
                actionTitle: actionTitle
            )

            // Set dismiss callback
            self.haloWindow?.viewModel.callbacks.onDismiss = { [weak self] in
                self?.dismissToast()
            }
            // Note: Action handling simplified for V2 model
            // TODO: Add proper action support in V2 if needed

            // Show at screen center
            self.haloWindow?.showCentered()

            // Set auto-dismiss timer for info toasts
            if shouldAutoDismiss {
                self.toastDismissTimer = Timer.scheduledTimer(withTimeInterval: 3.0, repeats: false) { [weak self] _ in
                    self?.dismissToast()
                }
            }
        }
    }

    /// Dismiss the current toast
    private func dismissToast() {
        toastDismissTimer?.invalidate()
        toastDismissTimer = nil

        Task { @MainActor [weak self] in
            self?.haloWindow?.hide()
        }
    }
}
