//
//  EventHandler.swift
//  Aether
//
//  Implements AetherEventHandler protocol to receive callbacks from Rust core.
//  This handler works with the rig-core based AetherCore.
//

import Foundation
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
class EventHandler: AetherEventHandler {

    // MARK: - Dependencies

    // Weak reference to Halo window to avoid retain cycle
    private weak var haloWindow: HaloWindow?

    // Weak reference to AetherCore for cancellation functionality
    private weak var core: AetherCore?

    // Weak reference to InputCoordinator for output handling
    private weak var inputCoordinator: InputCoordinator?

    // Managers accessed through DependencyContainer
    private var conversationManager: any ConversationManagerProtocol {
        DependencyContainer.shared.conversationManager
    }

    // MARK: - State

    // Accumulated text for streaming responses
    private var accumulatedText: String = ""

    // Current tool being executed (for UI feedback)
    private var currentToolName: String?

    // Check for multi-turn conversation mode
    private var isInMultiTurnMode: Bool {
        conversationManager.sessionId != nil || MultiTurnCoordinator.shared.isMultiTurnActive
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

    // Set InputCoordinator reference for output handling
    func setInputCoordinator(_ coordinator: InputCoordinator?) {
        self.inputCoordinator = coordinator
    }

    // MARK: - AetherEventHandler Protocol

    /// Called when AI is processing/thinking
    func onThinking() {
        print("[EventHandler] AI thinking...")

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Skip halo in multi-turn mode
            guard !slf.isInMultiTurnMode else {
                print("[EventHandler] Skipping thinking state (multi-turn mode)")
                return
            }

            slf.haloWindow?.updateState(.processingWithAI(providerName: nil))
            slf.haloWindow?.showAtCurrentPosition()
        }
    }

    /// Called when a tool execution starts
    /// - Parameter toolName: Name of the tool being executed
    func onToolStart(toolName: String) {
        print("[EventHandler] Tool started: \(toolName)")
        currentToolName = toolName

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Skip halo in multi-turn mode
            guard !slf.isInMultiTurnMode else {
                print("[EventHandler] Skipping tool start state (multi-turn mode)")
                return
            }

            // Show processing state with tool name
            slf.haloWindow?.updateState(.processing(streamingText: "Using \(toolName)..."))
            slf.haloWindow?.showAtCurrentPosition()
        }
    }

    /// Called when a tool execution completes
    /// - Parameters:
    ///   - toolName: Name of the tool that completed
    ///   - result: Result from the tool (may be truncated for display)
    func onToolResult(toolName: String, result: String) {
        print("[EventHandler] Tool result: \(toolName) - \(result.prefix(100))...")
        currentToolName = nil

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Skip halo in multi-turn mode
            guard !slf.isInMultiTurnMode else {
                print("[EventHandler] Skipping tool result state (multi-turn mode)")
                return
            }

            // Update state to show tool completed
            slf.haloWindow?.updateState(.processing(streamingText: "Completed: \(toolName)"))
        }
    }

    /// Called for each streaming response chunk
    /// - Parameter text: The accumulated response text so far
    func onStreamChunk(text: String) {
        // Only log first call and on significant changes to avoid log spam
        if accumulatedText.isEmpty || text.count - accumulatedText.count > 50 {
            print("[EventHandler] Stream chunk: \(text.prefix(50))... (total: \(text.count) chars)")
        }

        accumulatedText = text

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Skip halo in multi-turn mode
            guard !slf.isInMultiTurnMode else { return }

            slf.haloWindow?.updateState(.processing(streamingText: text))
        }
    }

    /// Called when processing completes successfully
    /// - Parameter response: The final response text
    func onComplete(response: String) {
        print("[EventHandler] Processing complete: \(response.prefix(100))...")

        // Reset state
        accumulatedText = ""
        currentToolName = nil

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Notify InputCoordinator if processing is pending
            if slf.inputCoordinator?.isProcessingPending == true {
                slf.inputCoordinator?.handleCompletion(response: response)
                return
            }

            // Notify MultiTurnCoordinator if processing is pending
            if MultiTurnCoordinator.shared.isProcessingPending {
                MultiTurnCoordinator.shared.handleCompletion(response: response)
                return
            }

            // Skip halo in multi-turn mode - conversation UI handles it
            guard !slf.isInMultiTurnMode else {
                print("[EventHandler] Skipping completion state (multi-turn mode)")
                return
            }

            // Show success state then auto-hide
            slf.haloWindow?.updateState(.success(message: nil))

            // Auto-hide after brief display
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { [weak slf] in
                slf?.haloWindow?.hide()
            }
        }
    }

    /// Called when an error occurs during processing
    /// - Parameter message: Error message describing what went wrong
    func onError(message: String) {
        print("[EventHandler] Error: \(message)")

        // Reset state
        accumulatedText = ""
        currentToolName = nil

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Notify InputCoordinator if processing is pending
            if slf.inputCoordinator?.isProcessingPending == true {
                slf.inputCoordinator?.handleError(message: message)
                // Still show error notification
                slf.showErrorNotification(message: message)
                return
            }

            // Notify MultiTurnCoordinator if processing is pending
            if MultiTurnCoordinator.shared.isProcessingPending {
                MultiTurnCoordinator.shared.handleError(message: message)
                // Multi-turn mode shows error in conversation UI, no halo notification
                return
            }

            // Show error notification even in multi-turn mode
            slf.showErrorNotification(message: message)
        }
    }

    /// Called when memory is stored successfully
    func onMemoryStored() {
        print("[EventHandler] Memory stored")

        // Subtle feedback - could show toast or just log
        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Optionally show brief memory stored indicator
            // For now, just log - memory storage is typically transparent to user
        }
    }

    /// Called when agent execution mode is detected
    /// - Parameter task: The executable task that was classified
    func onAgentModeDetected(task: ExecutableTaskFfi) {
        print("[EventHandler] Agent mode detected: category=\(task.category), action=\(task.action), confidence=\(task.confidence)")

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Skip agent mode notification in multi-turn mode for now
            // (multi-turn conversation UI will handle agent plans separately)
            guard !slf.isInMultiTurnMode else {
                print("[EventHandler] Skipping agent mode notification (multi-turn mode)")
                return
            }

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

        DispatchQueue.mainAsync(weakRef: self) { _ in
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

        DispatchQueue.mainAsync(weakRef: self) { _ in
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

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Post notification for UI components
            NotificationCenter.default.post(
                name: .runtimeUpdatesAvailable,
                object: nil,
                userInfo: ["updates": updates]
            )

            // Show toast notification about available updates
            if !updates.isEmpty {
                let runtimeNames = updates.map { $0.runtimeId }.joined(separator: ", ")
                let message = updates.count == 1
                    ? L("notification.runtime_update_single", updates[0].runtimeId, updates[0].latestVersion)
                    : L("notification.runtime_updates_multiple", String(updates.count))

                slf.showToast(
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

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Post notification for UI components
            NotificationCenter.default.post(
                name: .agenticSessionStarted,
                object: nil,
                userInfo: ["sessionId": sessionId]
            )

            // Skip Halo in multi-turn mode
            guard !slf.isInMultiTurnMode else { return }

            // Show processing state
            slf.haloWindow?.updateState(.processingWithAI(providerName: L("halo.agentic_session")))
            slf.haloWindow?.showAtCurrentPosition()
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

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Post notification
            NotificationCenter.default.post(
                name: .agenticToolCallStarted,
                object: nil,
                userInfo: [
                    "sessionId": slf.currentAgenticSessionId ?? "",
                    "callId": callId,
                    "toolName": toolName
                ]
            )

            // Skip Halo in multi-turn mode
            guard !slf.isInMultiTurnMode else { return }

            // Update Halo to show tool execution
            if slf.isInAgenticSession {
                // Show agent progress with current tool
                let progress = slf.currentPlanSteps.isEmpty ? 0.0 :
                    Float(slf.completedStepCount) / Float(slf.currentPlanSteps.count)
                slf.haloWindow?.updateState(.agentProgress(
                    planId: slf.currentAgenticSessionId ?? "",
                    progress: progress,
                    currentOperation: toolName,
                    completedCount: slf.completedStepCount,
                    totalCount: slf.currentPlanSteps.count
                ))
            } else {
                slf.haloWindow?.updateState(.processing(streamingText: "🔧 \(toolName)"))
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

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Post notification
            NotificationCenter.default.post(
                name: .agenticToolCallCompleted,
                object: nil,
                userInfo: [
                    "sessionId": slf.currentAgenticSessionId ?? "",
                    "callId": callId,
                    "toolName": toolName,
                    "output": String(output.prefix(500))
                ]
            )

            // Skip Halo in multi-turn mode
            guard !slf.isInMultiTurnMode else { return }

            // Update progress
            if slf.isInAgenticSession && !slf.currentPlanSteps.isEmpty {
                let progress = Float(slf.completedStepCount) / Float(slf.currentPlanSteps.count)
                slf.haloWindow?.updateState(.agentProgress(
                    planId: slf.currentAgenticSessionId ?? "",
                    progress: progress,
                    currentOperation: "✓ \(toolName)",
                    completedCount: slf.completedStepCount,
                    totalCount: slf.currentPlanSteps.count
                ))
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

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Post notification
            NotificationCenter.default.post(
                name: .agenticToolCallFailed,
                object: nil,
                userInfo: [
                    "sessionId": slf.currentAgenticSessionId ?? "",
                    "callId": callId,
                    "toolName": toolName,
                    "error": error,
                    "isRetryable": isRetryable
                ]
            )

            // Skip Halo in multi-turn mode
            guard !slf.isInMultiTurnMode else { return }

            // Show error in progress (if retryable, indicate retry)
            if slf.isInAgenticSession {
                let statusText = isRetryable ? "⟳ \(toolName) (retrying...)" : "✗ \(toolName)"
                let progress = slf.currentPlanSteps.isEmpty ? 0.0 :
                    Float(slf.completedStepCount) / Float(slf.currentPlanSteps.count)
                slf.haloWindow?.updateState(.agentProgress(
                    planId: slf.currentAgenticSessionId ?? "",
                    progress: progress,
                    currentOperation: statusText,
                    completedCount: slf.completedStepCount,
                    totalCount: slf.currentPlanSteps.count
                ))
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

        DispatchQueue.mainAsync(weakRef: self) { slf in
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

            // Skip Halo in multi-turn mode
            guard !slf.isInMultiTurnMode else { return }

            // Update Halo with iteration info
            if slf.isInAgenticSession {
                let progress = slf.currentPlanSteps.isEmpty ? 0.0 :
                    Float(slf.completedStepCount) / Float(slf.currentPlanSteps.count)
                slf.haloWindow?.updateState(.agentProgress(
                    planId: sessionId,
                    progress: progress,
                    currentOperation: status,
                    completedCount: slf.completedStepCount,
                    totalCount: slf.currentPlanSteps.count
                ))
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

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Post notification
            NotificationCenter.default.post(
                name: .agenticPlanCreated,
                object: nil,
                userInfo: [
                    "sessionId": sessionId,
                    "steps": steps
                ]
            )

            // Skip Halo in multi-turn mode
            guard !slf.isInMultiTurnMode else { return }

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

            slf.haloWindow?.updateState(.planProgress(progressInfo: PlanProgressInfo(
                planId: sessionId,
                description: L("halo.executing_plan"),
                totalSteps: UInt32(steps.count),
                currentStep: 0,
                currentStepName: steps.first ?? "",
                stepProgress: stepProgress,
                status: .running,
                errorMessage: nil
            )))
            slf.haloWindow?.showAtCurrentPosition()
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

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Post notification
            NotificationCenter.default.post(
                name: .agenticSessionCompleted,
                object: nil,
                userInfo: [
                    "sessionId": sessionId,
                    "summary": summary
                ]
            )

            // Skip Halo in multi-turn mode
            guard !slf.isInMultiTurnMode else { return }

            // Show success toast if we were tracking this session
            if wasInSession {
                slf.haloWindow?.updateState(.success(message: summary))

                // Auto-hide after brief display
                DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak slf] in
                    slf?.haloWindow?.hide()
                }
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

        DispatchQueue.mainAsync(weakRef: self) { slf in
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

            // Skip Halo in multi-turn mode
            guard !slf.isInMultiTurnMode else { return }

            // Show sub-agent indicator in progress
            if slf.isInAgenticSession {
                let progress = slf.currentPlanSteps.isEmpty ? 0.0 :
                    Float(slf.completedStepCount) / Float(slf.currentPlanSteps.count)
                slf.haloWindow?.updateState(.agentProgress(
                    planId: parentSessionId,
                    progress: progress,
                    currentOperation: "🤖 \(agentId)",
                    completedCount: slf.completedStepCount,
                    totalCount: slf.currentPlanSteps.count
                ))
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

        DispatchQueue.mainAsync(weakRef: self) { slf in
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

            // Skip Halo in multi-turn mode
            guard !slf.isInMultiTurnMode else { return }

            // Update progress with sub-agent result
            if slf.isInAgenticSession {
                let statusIcon = success ? "✓" : "✗"
                let truncatedSummary = summary.count > 30 ? String(summary.prefix(30)) + "..." : summary
                let progress = slf.currentPlanSteps.isEmpty ? 0.0 :
                    Float(slf.completedStepCount) / Float(slf.currentPlanSteps.count)
                slf.haloWindow?.updateState(.agentProgress(
                    planId: slf.currentAgenticSessionId ?? "",
                    progress: progress,
                    currentOperation: "\(statusIcon) \(truncatedSummary)",
                    completedCount: slf.completedStepCount,
                    totalCount: slf.currentPlanSteps.count
                ))
            }
        }
    }

    // MARK: - Error Notification

    private func showErrorNotification(message: String) {
        // Skip halo in multi-turn mode - just show notification
        guard !isInMultiTurnMode else {
            print("[EventHandler] Showing error notification (multi-turn mode)")
            // Could show system notification here
            return
        }

        // Use toast notification in Halo
        haloWindow?.updateState(.toast(
            type: .error,
            title: L("error.aether"),
            message: message,
            autoDismiss: false
        ))

        // Set dismiss callback
        haloWindow?.viewModel.callbacks.toastOnDismiss = { [weak self] in
            self?.haloWindow?.hide()
        }

        // Show at screen center
        haloWindow?.showToastCentered()
    }

    // MARK: - Helper Methods

    /// Cancel the current operation
    func cancel() {
        core?.cancel()
        accumulatedText = ""
        currentToolName = nil

        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.haloWindow?.hide()
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

    // MARK: - Toast Display

    /// Timer for auto-dismissing toasts
    private var toastDismissTimer: Timer?

    /// Show a toast notification to the user
    /// - Parameters:
    ///   - type: The toast type (info, warning, error)
    ///   - title: Toast title
    ///   - message: Toast message
    ///   - autoDismiss: Whether to auto-dismiss (default: true for info)
    func showToast(type: ToastType, title: String, message: String, autoDismiss: Bool = true) {
        print("[EventHandler] Showing toast: \(type) - \(title)")

        // Cancel any existing dismiss timer
        toastDismissTimer?.invalidate()
        toastDismissTimer = nil

        DispatchQueue.mainAsync(weakRef: self) { slf in
            // Update Halo state to toast
            let shouldAutoDismiss = autoDismiss && type == .info
            slf.haloWindow?.updateState(.toast(
                type: type,
                title: title,
                message: message,
                autoDismiss: shouldAutoDismiss
            ))

            // Set dismiss callback
            slf.haloWindow?.viewModel.callbacks.toastOnDismiss = { [weak slf] in
                slf?.dismissToast()
            }

            // Show at screen center
            slf.haloWindow?.showToastCentered()

            // Set auto-dismiss timer for info toasts
            if shouldAutoDismiss {
                slf.toastDismissTimer = Timer.scheduledTimer(withTimeInterval: 3.0, repeats: false) { [weak slf] _ in
                    slf?.dismissToast()
                }
            }
        }
    }

    /// Dismiss the current toast
    private func dismissToast() {
        toastDismissTimer?.invalidate()
        toastDismissTimer = nil

        DispatchQueue.mainAsync(weakRef: self) { slf in
            slf.haloWindow?.hide()
        }
    }
}
