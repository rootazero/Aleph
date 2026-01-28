//
//  GatewayMultiTurnAdapter.swift
//  Aether
//
//  Adapter that bridges Gateway WebSocket events to MultiTurnCoordinator callbacks.
//  Enables MultiTurn conversations via the Gateway control plane.
//

import Foundation

/// Adapter that translates Gateway StreamEvents to MultiTurnCoordinator callback methods
///
/// This adapter enables the MultiTurn conversation UI to work with the Gateway
/// WebSocket interface instead of FFI. It handles:
/// - Reasoning events -> thinking/stream updates
/// - Tool events -> tool start/end callbacks
/// - Response chunks -> stream chunk callbacks
/// - Run complete/error -> completion/error callbacks
@MainActor
final class GatewayMultiTurnAdapter {

    // MARK: - Properties

    /// Weak reference to the coordinator to avoid retain cycles
    weak var coordinator: MultiTurnCoordinator?

    /// Accumulated streaming text (Gateway sends chunks, we accumulate)
    private var accumulatedText: String = ""

    /// Current run ID being processed
    private var currentRunId: String?

    /// Track if we've started streaming
    private var isStreaming: Bool = false

    // MARK: - Initialization

    init() {}

    /// Configure with coordinator reference
    func configure(coordinator: MultiTurnCoordinator) {
        self.coordinator = coordinator
        print("[GatewayMultiTurnAdapter] Configured with coordinator")
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
        isStreaming = false

        // Notify coordinator that processing has started
        coordinator?.handleThinking()
    }

    private func handleReasoning(_ event: ReasoningEvent) {
        guard event.runId == currentRunId else { return }

        if event.isComplete {
            print("[GatewayMultiTurnAdapter] Reasoning complete")
        } else {
            // Stream reasoning content to UI
            // Note: MultiTurnCoordinator doesn't have a specific thinking stream handler,
            // but we can use handleThinking() to indicate processing state
            if !event.content.isEmpty {
                print("[GatewayMultiTurnAdapter] Reasoning: \(event.content.prefix(50))...")
                // For now, reasoning goes to the same stream as response
                // This matches how Claude Code shows thinking in the conversation
                accumulatedText += event.content
                coordinator?.handleStreamChunk(text: accumulatedText)
            }
        }
    }

    private func handleToolStart(_ event: ToolStartEvent) {
        guard event.runId == currentRunId else { return }

        print("[GatewayMultiTurnAdapter] Tool started: \(event.toolName)")
        coordinator?.handleToolStart(toolName: event.toolName)
    }

    private func handleToolUpdate(_ event: ToolUpdateEvent) {
        guard event.runId == currentRunId else { return }

        // Tool updates can be streamed to the response
        print("[GatewayMultiTurnAdapter] Tool update: \(event.progress.prefix(50))...")
    }

    private func handleToolEnd(_ event: ToolEndEvent) {
        guard event.runId == currentRunId else { return }

        let resultString: String
        if event.result.success {
            resultString = event.result.output ?? "Success"
        } else {
            resultString = "Error: \(event.result.error ?? "Unknown error")"
        }

        print("[GatewayMultiTurnAdapter] Tool ended (\(event.durationMs)ms): \(resultString.prefix(50))...")
        coordinator?.handleToolResult(toolName: "", result: resultString)
    }

    private func handleResponseChunk(_ event: ResponseChunkEvent) {
        guard event.runId == currentRunId else { return }

        // Accumulate response chunks
        if !event.content.isEmpty {
            accumulatedText += event.content
            isStreaming = true
            coordinator?.handleStreamChunk(text: accumulatedText)
        }

        // Final chunk signals completion
        if event.isFinal {
            print("[GatewayMultiTurnAdapter] Response complete (final chunk)")
        }
    }

    private func handleRunComplete(_ event: RunCompleteEvent) {
        guard event.runId == currentRunId else { return }

        print("[GatewayMultiTurnAdapter] Run complete: \(event.summary.loops) loops, \(event.totalDurationMs)ms")

        // Use final response from summary if available, otherwise use accumulated text
        let response = event.summary.finalResponse ?? accumulatedText

        coordinator?.handleCompletion(response: response)

        // Reset state
        currentRunId = nil
        accumulatedText = ""
        isStreaming = false
    }

    private func handleRunError(_ event: RunErrorEvent) {
        guard event.runId == currentRunId else { return }

        let errorMessage = event.errorCode.map { "[\($0)] \(event.error)" } ?? event.error
        print("[GatewayMultiTurnAdapter] Run error: \(errorMessage)")

        coordinator?.handleError(message: errorMessage)

        // Reset state
        currentRunId = nil
        accumulatedText = ""
        isStreaming = false
    }

    private func handleAskUser(_ event: AskUserEvent) {
        guard event.runId == currentRunId else { return }

        print("[GatewayMultiTurnAdapter] User question: \(event.question)")

        // TODO: Implement user question UI flow
        // For now, just log and continue
        // The Gateway will need a way to receive user responses
    }

    // MARK: - Reset

    /// Reset adapter state (e.g., when starting a new conversation)
    func reset() {
        currentRunId = nil
        accumulatedText = ""
        isStreaming = false
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

        // Use the client's agent run with streaming
        // Returns (AgentRunResult, AsyncStream<StreamEvent>)
        let (_, stream) = try await client.agentRun(input: input, sessionKey: sessionKey)

        // Process stream events
        for try await event in stream {
            await adapter.handleStreamEvent(event)
        }
    }
}
