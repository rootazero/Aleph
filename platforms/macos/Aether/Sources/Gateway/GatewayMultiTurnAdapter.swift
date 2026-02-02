//
//  GatewayMultiTurnAdapter.swift
//  Aether
//
//  Adapter that bridges Gateway WebSocket events to MultiTurnCoordinator callbacks.
//  Enables MultiTurn conversations via the Gateway control plane.
//

import Foundation
import Combine

/// Callback type for user question events
typealias AskUserCallback = (AskUserEvent) -> Void

/// Callback type for submitting answers
typealias AnswerSubmitCallback = (String, String, [String: String]) async throws -> Void

/// Part types for Part-driven UI rendering
enum MessagePart: Identifiable, Equatable {
    case text(id: String, content: String, isStreaming: Bool)
    case reasoning(id: String, content: String, isComplete: Bool)
    case toolCall(id: String, toolName: String, status: ToolPartStatus, result: String?, durationMs: UInt64?)
    case askUser(id: String, event: AskUserEvent)

    var id: String {
        switch self {
        case .text(let id, _, _): return id
        case .reasoning(let id, _, _): return id
        case .toolCall(let id, _, _, _, _): return id
        case .askUser(let id, _): return id
        }
    }

    static func == (lhs: MessagePart, rhs: MessagePart) -> Bool {
        switch (lhs, rhs) {
        case (.text(let id1, let c1, let s1), .text(let id2, let c2, let s2)):
            return id1 == id2 && c1 == c2 && s1 == s2
        case (.reasoning(let id1, let c1, let comp1), .reasoning(let id2, let c2, let comp2)):
            return id1 == id2 && c1 == c2 && comp1 == comp2
        case (.toolCall(let id1, let n1, let s1, let r1, let d1), .toolCall(let id2, let n2, let s2, let r2, let d2)):
            return id1 == id2 && n1 == n2 && s1 == s2 && r1 == r2 && d1 == d2
        case (.askUser(let id1, _), .askUser(let id2, _)):
            return id1 == id2
        default:
            return false
        }
    }
}

/// Tool execution status
enum ToolPartStatus: Equatable {
    case running
    case success
    case error
}

/// Adapter that translates Gateway StreamEvents to MultiTurnCoordinator callback methods
///
/// This adapter enables the MultiTurn conversation UI to work with the Gateway
/// WebSocket interface instead of FFI. It handles:
/// - Reasoning events -> thinking/stream updates
/// - Tool events -> tool start/end callbacks
/// - Response chunks -> stream chunk callbacks
/// - Run complete/error -> completion/error callbacks
/// - AskUser events -> user question modals
@MainActor
final class GatewayMultiTurnAdapter: ObservableObject {

    // MARK: - Published Properties

    /// Current message parts for Part-driven UI
    @Published private(set) var parts: [MessagePart] = []

    /// Current pending user question (nil when no question pending)
    @Published private(set) var pendingQuestion: AskUserEvent?

    /// Run summary when complete
    @Published private(set) var runSummary: RunSummary?

    // MARK: - Properties

    // Coordinator reference removed - using event-based callbacks instead

    /// Callback for user questions (alternative to @Published)
    var onAskUser: AskUserCallback?

    /// Callback for submitting answers
    var onAnswer: AnswerSubmitCallback?

    /// Accumulated streaming text (Gateway sends chunks, we accumulate)
    private var accumulatedText: String = ""

    /// Current reasoning text
    private var reasoningText: String = ""

    /// Current run ID being processed
    private(set) var currentRunId: String?

    /// Track if we've started streaming
    private var isStreaming: Bool = false

    /// Tool call tracking for Part updates
    private var activeToolCalls: [String: (name: String, partId: String)] = [:]

    /// Part ID counter
    private var partIdCounter: Int = 0

    // MARK: - Initialization

    init() {}

    /// Configure adapter (coordinator dependency removed)
    func configure() {
        print("[GatewayMultiTurnAdapter] Configured")
    }

    // MARK: - Event Handling

    /// Handle a StreamEvent from the Gateway
    ///
    /// Maps Gateway events to MultiTurnCoordinator callbacks:
    /// - reasoning -> handleThinkingStream / handleThinking
    /// - toolStart -> handleToolStart
    /// - toolEnd -> handleToolResult
    /// - responseChunk -> handleStreamChunk
    /// - runComplete -> handleCompletion
    /// - runError -> handleError
    func handleStreamEvent(_ event: StreamEvent) {
        switch event {
        case .runAccepted(let e):
            handleRunAccepted(e)

        case .reasoning(let e):
            handleReasoning(e)

        case .toolStart(let e):
            handleToolStart(e)

        case .toolUpdate(let e):
            handleToolUpdate(e)

        case .toolEnd(let e):
            handleToolEnd(e)

        case .responseChunk(let e):
            handleResponseChunk(e)

        case .runComplete(let e):
            handleRunComplete(e)

        case .runError(let e):
            handleRunError(e)

        case .askUser(let e):
            handleAskUser(e)

        case .unknown(let type):
            print("[GatewayMultiTurnAdapter] Unknown event type: \(type)")
        }
    }

    // MARK: - Private Event Handlers

    private func handleRunAccepted(_ event: RunAcceptedEvent) {
        print("[GatewayMultiTurnAdapter] Run accepted: \(event.runId)")
        currentRunId = event.runId
        accumulatedText = ""
        reasoningText = ""
        isStreaming = false
        parts = []
        activeToolCalls = [:]
        runSummary = nil
        pendingQuestion = nil
        partIdCounter = 0

        // Thinking state handled via events
    }

    private func handleReasoning(_ event: ReasoningEvent) {
        guard event.runId == currentRunId else { return }

        // Accumulate reasoning text
        reasoningText += event.content

        // Update or create reasoning part
        let partId = "reasoning-\(currentRunId ?? "unknown")"
        updateOrAddPart(.reasoning(id: partId, content: reasoningText, isComplete: event.isComplete))

        if event.isComplete {
            print("[GatewayMultiTurnAdapter] Reasoning complete")
        } else if !event.content.isEmpty {
            print("[GatewayMultiTurnAdapter] Reasoning: \(event.content.prefix(50))...")
        }
    }

    private func handleToolStart(_ event: ToolStartEvent) {
        guard event.runId == currentRunId else { return }

        print("[GatewayMultiTurnAdapter] Tool started: \(event.toolName)")

        // Create tool call part
        let partId = "tool-\(event.toolId)"
        activeToolCalls[event.toolId] = (name: event.toolName, partId: partId)
        addPart(.toolCall(id: partId, toolName: event.toolName, status: .running, result: nil, durationMs: nil))

        // Tool start handled via events
    }

    private func handleToolUpdate(_ event: ToolUpdateEvent) {
        guard event.runId == currentRunId else { return }

        // Tool updates can be streamed to the response
        print("[GatewayMultiTurnAdapter] Tool update: \(event.progress.prefix(50))...")

        // Update tool part with progress (if we want to show it)
        // For now, just log it
    }

    private func handleToolEnd(_ event: ToolEndEvent) {
        guard event.runId == currentRunId else { return }

        let resultString: String
        let status: ToolPartStatus
        if event.result.success {
            resultString = event.result.output ?? "Success"
            status = .success
        } else {
            resultString = "Error: \(event.result.error ?? "Unknown error")"
            status = .error
        }

        print("[GatewayMultiTurnAdapter] Tool ended (\(event.durationMs)ms): \(resultString.prefix(50))...")

        // Update tool call part
        if let toolInfo = activeToolCalls[event.toolId] {
            updateOrAddPart(.toolCall(
                id: toolInfo.partId,
                toolName: toolInfo.name,
                status: status,
                result: resultString,
                durationMs: event.durationMs
            ))
            activeToolCalls.removeValue(forKey: event.toolId)
        }

        // Tool result handled via events
    }

    private func handleResponseChunk(_ event: ResponseChunkEvent) {
        guard event.runId == currentRunId else { return }

        // Accumulate response chunks
        if !event.content.isEmpty {
            accumulatedText += event.content
            isStreaming = true

            // Update or create text part
            let partId = "text-\(currentRunId ?? "unknown")"
            updateOrAddPart(.text(id: partId, content: accumulatedText, isStreaming: !event.isFinal))

            // Stream chunk handled via events
        }

        // Final chunk signals completion
        if event.isFinal {
            print("[GatewayMultiTurnAdapter] Response complete (final chunk)")
        }
    }

    private func handleRunComplete(_ event: RunCompleteEvent) {
        guard event.runId == currentRunId else { return }

        print("[GatewayMultiTurnAdapter] Run complete: \(event.summary.loops) loops, \(event.totalDurationMs)ms")

        // Save run summary
        runSummary = event.summary

        // Use final response from summary if available, otherwise use accumulated text
        let response = event.summary.finalResponse ?? accumulatedText

        // Finalize text part
        if !accumulatedText.isEmpty {
            let partId = "text-\(currentRunId ?? "unknown")"
            updateOrAddPart(.text(id: partId, content: accumulatedText, isStreaming: false))
        }

        // Completion handled via events

        // Reset state (but keep parts and summary for display)
        currentRunId = nil
        accumulatedText = ""
        reasoningText = ""
        isStreaming = false
        activeToolCalls = [:]
    }

    private func handleRunError(_ event: RunErrorEvent) {
        guard event.runId == currentRunId else { return }

        let errorMessage = event.errorCode.map { "[\($0)] \(event.error)" } ?? event.error
        print("[GatewayMultiTurnAdapter] Run error: \(errorMessage)")

        // Error handled via events

        // Reset state
        currentRunId = nil
        accumulatedText = ""
        reasoningText = ""
        isStreaming = false
        activeToolCalls = [:]
    }

    private func handleAskUser(_ event: AskUserEvent) {
        guard event.runId == currentRunId else { return }

        let questionText = event.questions.first?.question ?? event.question ?? "Question"
        print("[GatewayMultiTurnAdapter] User question: \(questionText)")

        // Add AskUser part
        let partId = "askuser-\(event.questionId)"
        addPart(.askUser(id: partId, event: event))

        // Set pending question for UI
        pendingQuestion = event

        // Notify via callback
        onAskUser?(event)
    }

    // MARK: - Answer Submission

    /// Submit answer for a user question
    func submitAnswer(questionId: String, answers: [String: String]) async throws {
        guard let runId = currentRunId else {
            throw GatewayError.invalidResponse("No active run")
        }

        // Call the answer callback
        try await onAnswer?(runId, questionId, answers)

        // Clear pending question
        pendingQuestion = nil

        // Remove the AskUser part (or update it to show answered)
        let partId = "askuser-\(questionId)"
        parts.removeAll { $0.id == partId }
    }

    /// Cancel the current question (cancel the run)
    func cancelQuestion() {
        pendingQuestion = nil
    }

    // MARK: - Part Management

    private func nextPartId() -> String {
        partIdCounter += 1
        return "part-\(partIdCounter)"
    }

    private func addPart(_ part: MessagePart) {
        parts.append(part)
    }

    private func updateOrAddPart(_ part: MessagePart) {
        if let index = parts.firstIndex(where: { $0.id == part.id }) {
            parts[index] = part
        } else {
            parts.append(part)
        }
    }

    // MARK: - Reset

    /// Reset adapter state (e.g., when starting a new conversation)
    func reset() {
        currentRunId = nil
        accumulatedText = ""
        reasoningText = ""
        isStreaming = false
        parts = []
        activeToolCalls = [:]
        runSummary = nil
        pendingQuestion = nil
        partIdCounter = 0
    }

    /// Get all parts for display
    func getAllParts() -> [MessagePart] {
        parts
    }

    /// Check if there's a pending question
    var hasPendingQuestion: Bool {
        pendingQuestion != nil
    }
}

// MARK: - GatewayManager Extension

extension GatewayManager {

    /// Convenience method to run agent and stream events to adapter
    func runAgentWithAdapter(
        input: String,
        sessionKey: String,
        adapter: GatewayMultiTurnAdapter
    ) async throws {
        guard isReady else {
            throw GatewayError.notConnected
        }

        // Reset adapter for new run
        await adapter.reset()

        // Configure answer callback
        adapter.onAnswer = { [weak self] runId, questionId, answers in
            guard let self = self else {
                throw GatewayError.notConnected
            }
            try await self.client.answer(runId: runId, questionId: questionId, answers: answers)
        }

        // Use the client's agent run with streaming
        // Returns (AgentRunResult, AsyncStream<StreamEvent>)
        let (_, stream) = try await client.agentRun(input: input, sessionKey: sessionKey)

        // Process stream events
        for try await event in stream {
            await adapter.handleStreamEvent(event)
        }
    }

    /// Submit an answer for a pending user question
    func submitAnswer(runId: String, questionId: String, answers: [String: String]) async throws {
        guard isReady else {
            throw GatewayError.notConnected
        }

        try await client.answer(runId: runId, questionId: questionId, answers: answers)
    }

    /// Cancel a running agent
    func cancelRun(runId: String) async throws {
        guard isReady else {
            throw GatewayError.notConnected
        }

        try await client.cancelRun(runId: runId)
    }
}
