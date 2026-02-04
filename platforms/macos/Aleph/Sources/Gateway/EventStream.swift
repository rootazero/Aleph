import Foundation
import Combine
import os.log

/// Manages event streams for agent runs
///
/// Provides a high-level interface for subscribing to events from agent runs,
/// with automatic reordering and aggregation of out-of-order events.
@MainActor
final class EventStreamManager: ObservableObject {
    // MARK: - Published State

    /// Current reasoning text (accumulated)
    @Published private(set) var currentReasoning: String = ""

    /// Whether reasoning is in progress
    @Published private(set) var isReasoning: Bool = false

    /// Current response text (accumulated from chunks)
    @Published private(set) var currentResponse: String = ""

    /// Active tool executions
    @Published private(set) var activeTools: [String: ToolExecution] = [:]

    /// Current run state
    @Published private(set) var runState: RunState = .idle

    // MARK: - Private Properties

    private let logger = Logger(subsystem: "com.aether", category: "EventStream")
    private var eventBuffer: [UInt64: StreamEvent] = [:]
    private var nextExpectedSeq: UInt64 = 0
    private var currentRunId: String?

    // MARK: - Types

    enum RunState: Equatable {
        case idle
        case running(runId: String)
        case completed(runId: String, summary: RunSummary)
        case failed(runId: String, error: String)
    }

    struct ToolExecution: Identifiable {
        let id: String
        let name: String
        let params: Any?
        var progress: String?
        var result: ToolResult?
        var durationMs: UInt64?
        var isComplete: Bool = false
        let startedAt: Date
    }

    // MARK: - Public Methods

    /// Start processing events for a new run
    func startRun(runId: String, stream: AsyncStream<StreamEvent>) {
        reset()
        currentRunId = runId
        runState = .running(runId: runId)

        Task {
            for await event in stream {
                await processEvent(event)
            }
        }
    }

    /// Reset the stream manager state
    func reset() {
        currentReasoning = ""
        isReasoning = false
        currentResponse = ""
        activeTools.removeAll()
        runState = .idle
        eventBuffer.removeAll()
        nextExpectedSeq = 0
        currentRunId = nil
    }

    // MARK: - Event Processing

    private func processEvent(_ event: StreamEvent) async {
        // Buffer out-of-order events
        let seq = event.seq
        if seq > 0 && seq != nextExpectedSeq {
            eventBuffer[seq] = event
            return
        }

        // Process this event
        await handleEvent(event)
        nextExpectedSeq = seq + 1

        // Process any buffered events that are now in order
        while let bufferedEvent = eventBuffer.removeValue(forKey: nextExpectedSeq) {
            await handleEvent(bufferedEvent)
            nextExpectedSeq += 1
        }
    }

    private func handleEvent(_ event: StreamEvent) async {
        switch event {
        case .runAccepted(let e):
            logger.debug("Run accepted: \(e.runId)")

        case .reasoning(let e):
            isReasoning = !e.isComplete
            if !e.content.isEmpty {
                currentReasoning += e.content
            }
            logger.debug("Reasoning: \(e.content.prefix(50))... (complete: \(e.isComplete))")

        case .toolStart(let e):
            let execution = ToolExecution(
                id: e.toolId,
                name: e.toolName,
                params: e.params?.value,
                startedAt: Date()
            )
            activeTools[e.toolId] = execution
            logger.debug("Tool started: \(e.toolName)")

        case .toolUpdate(let e):
            if var tool = activeTools[e.toolId] {
                tool.progress = e.progress
                activeTools[e.toolId] = tool
            }
            logger.debug("Tool update: \(e.progress)")

        case .toolEnd(let e):
            if var tool = activeTools[e.toolId] {
                tool.result = e.result
                tool.durationMs = e.durationMs
                tool.isComplete = true
                activeTools[e.toolId] = tool
            }
            logger.debug("Tool completed: \(e.toolId) in \(e.durationMs)ms")

        case .responseChunk(let e):
            currentResponse += e.content
            if e.isFinal {
                logger.debug("Response complete: \(self.currentResponse.count) chars")
            }

        case .runComplete(let e):
            runState = .completed(runId: e.runId, summary: e.summary)
            logger.info("Run completed: \(e.runId) in \(e.totalDurationMs)ms")

        case .runError(let e):
            runState = .failed(runId: e.runId, error: e.error)
            logger.error("Run failed: \(e.error)")

        case .askUser(let e):
            // TODO: Handle user questions through UI
            let questionText = e.questions.first?.question ?? e.question ?? "Question"
            logger.info("User question: \(questionText)")

        case .unknown(let type):
            logger.warning("Unknown event type: \(type)")
        }
    }
}

// MARK: - Event Aggregator

/// Aggregates and buffers events for smooth UI updates
///
/// Coalesces rapid events to prevent UI jank while maintaining responsiveness.
actor EventAggregator {
    private var buffer: [StreamEvent] = []
    private var flushTask: Task<Void, Never>?
    private let flushInterval: TimeInterval = 0.05 // 50ms

    private let onFlush: ([StreamEvent]) async -> Void

    init(onFlush: @escaping ([StreamEvent]) async -> Void) {
        self.onFlush = onFlush
    }

    func add(_ event: StreamEvent) {
        buffer.append(event)

        // Schedule flush if not already scheduled
        if flushTask == nil {
            flushTask = Task {
                try? await Task.sleep(nanoseconds: UInt64(flushInterval * 1_000_000_000))
                await flush()
            }
        }
    }

    private func flush() async {
        guard !buffer.isEmpty else {
            flushTask = nil
            return
        }

        let events = buffer
        buffer.removeAll()
        flushTask = nil

        await onFlush(events)
    }

    func flushNow() async {
        flushTask?.cancel()
        flushTask = nil
        await flush()
    }
}

// MARK: - Text Chunker

/// Implements Moltbot-style block chunking for smooth text streaming
///
/// Buffers text and flushes based on:
/// - Min chars: 200 (buffer before sending)
/// - Max chars: 2000 (hard limit, force send)
/// - Break preference: paragraph → newline → sentence → whitespace
struct TextChunker {
    private var buffer: String = ""

    let minChars: Int = 200
    let maxChars: Int = 2000

    mutating func add(_ text: String) -> String? {
        buffer += text

        if buffer.count >= maxChars {
            return forceFlush()
        }

        if buffer.count >= minChars {
            if let breakPoint = findBreakPoint() {
                let chunk = String(buffer.prefix(breakPoint))
                buffer = String(buffer.dropFirst(breakPoint))
                return chunk
            }
        }

        return nil
    }

    mutating func flush() -> String? {
        guard !buffer.isEmpty else { return nil }
        let result = buffer
        buffer = ""
        return result
    }

    private mutating func forceFlush() -> String {
        let breakPoint = findBreakPoint() ?? min(buffer.count, maxChars)
        let chunk = String(buffer.prefix(breakPoint))
        buffer = String(buffer.dropFirst(breakPoint))
        return chunk
    }

    private func findBreakPoint() -> Int? {
        // Priority: paragraph → newline → sentence → whitespace
        let text = buffer

        // Paragraph break (double newline)
        if let range = text.range(of: "\n\n", options: .backwards) {
            let index = text.distance(from: text.startIndex, to: range.upperBound)
            if index >= minChars / 2 {
                return index
            }
        }

        // Single newline
        if let range = text.range(of: "\n", options: .backwards) {
            let index = text.distance(from: text.startIndex, to: range.upperBound)
            if index >= minChars / 2 {
                return index
            }
        }

        // Sentence end
        let sentenceEnders = [". ", "! ", "? ", "。", "！", "？"]
        for ender in sentenceEnders {
            if let range = text.range(of: ender, options: .backwards) {
                let index = text.distance(from: text.startIndex, to: range.upperBound)
                if index >= minChars / 2 {
                    return index
                }
            }
        }

        // Whitespace
        if let range = text.range(of: " ", options: .backwards) {
            let index = text.distance(from: text.startIndex, to: range.upperBound)
            if index >= minChars / 2 {
                return index
            }
        }

        return nil
    }
}
