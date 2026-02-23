import Foundation

// MARK: - JSON-RPC 2.0 Protocol Types

/// JSON-RPC 2.0 Request
struct JsonRpcRequest: Codable, Sendable {
    let jsonrpc: String
    let method: String
    let params: AnyCodable?
    let id: JsonRpcId?

    init(method: String, params: Any? = nil, id: JsonRpcId? = nil) {
        self.jsonrpc = "2.0"
        self.method = method
        self.params = params.map { AnyCodable($0) }
        self.id = id
    }

    /// Create a notification (request without id)
    static func notification(method: String, params: Any? = nil) -> JsonRpcRequest {
        JsonRpcRequest(method: method, params: params, id: nil)
    }
}

/// JSON-RPC 2.0 Response
struct JsonRpcResponse: Codable, Sendable {
    let jsonrpc: String
    let result: AnyCodable?
    let error: JsonRpcError?
    let id: JsonRpcId?

    var isSuccess: Bool { error == nil }
    var isError: Bool { error != nil }
}

/// JSON-RPC 2.0 Error
struct JsonRpcError: Codable, Error, Sendable {
    let code: Int
    let message: String
    let data: AnyCodable?

    // Standard error codes
    static let parseError = -32700
    static let invalidRequest = -32600
    static let methodNotFound = -32601
    static let invalidParams = -32602
    static let internalError = -32603
}

/// JSON-RPC ID (can be string, number, or null)
enum JsonRpcId: Codable, Equatable, Sendable {
    case string(String)
    case number(Int)
    case null

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let string = try? container.decode(String.self) {
            self = .string(string)
        } else if let number = try? container.decode(Int.self) {
            self = .number(number)
        } else {
            throw DecodingError.typeMismatch(
                JsonRpcId.self,
                DecodingError.Context(codingPath: decoder.codingPath, debugDescription: "Invalid JSON-RPC id type")
            )
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .string(let s): try container.encode(s)
        case .number(let n): try container.encode(n)
        case .null: try container.encodeNil()
        }
    }

    static func generate() -> JsonRpcId {
        .string(UUID().uuidString)
    }
}

// MARK: - Stream Event Types

/// Stream events from the Gateway
enum StreamEvent: Codable, Sendable {
    case runAccepted(RunAcceptedEvent)
    case reasoning(ReasoningEvent)
    case toolStart(ToolStartEvent)
    case toolUpdate(ToolUpdateEvent)
    case toolEnd(ToolEndEvent)
    case responseChunk(ResponseChunkEvent)
    case runComplete(RunCompleteEvent)
    case runError(RunErrorEvent)
    case askUser(AskUserEvent)
    case unknown(String)

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let type = try container.decode(String.self, forKey: .type)

        switch type {
        case "run_accepted":
            self = .runAccepted(try RunAcceptedEvent(from: decoder))
        case "reasoning":
            self = .reasoning(try ReasoningEvent(from: decoder))
        case "tool_start":
            self = .toolStart(try ToolStartEvent(from: decoder))
        case "tool_update":
            self = .toolUpdate(try ToolUpdateEvent(from: decoder))
        case "tool_end":
            self = .toolEnd(try ToolEndEvent(from: decoder))
        case "response_chunk":
            self = .responseChunk(try ResponseChunkEvent(from: decoder))
        case "run_complete":
            self = .runComplete(try RunCompleteEvent(from: decoder))
        case "run_error":
            self = .runError(try RunErrorEvent(from: decoder))
        case "ask_user":
            self = .askUser(try AskUserEvent(from: decoder))
        default:
            self = .unknown(type)
        }
    }

    func encode(to encoder: Encoder) throws {
        switch self {
        case .runAccepted(let e): try e.encode(to: encoder)
        case .reasoning(let e): try e.encode(to: encoder)
        case .toolStart(let e): try e.encode(to: encoder)
        case .toolUpdate(let e): try e.encode(to: encoder)
        case .toolEnd(let e): try e.encode(to: encoder)
        case .responseChunk(let e): try e.encode(to: encoder)
        case .runComplete(let e): try e.encode(to: encoder)
        case .runError(let e): try e.encode(to: encoder)
        case .askUser(let e): try e.encode(to: encoder)
        case .unknown: break
        }
    }

    private enum CodingKeys: String, CodingKey {
        case type
    }

    var runId: String {
        switch self {
        case .runAccepted(let e): return e.runId
        case .reasoning(let e): return e.runId
        case .toolStart(let e): return e.runId
        case .toolUpdate(let e): return e.runId
        case .toolEnd(let e): return e.runId
        case .responseChunk(let e): return e.runId
        case .runComplete(let e): return e.runId
        case .runError(let e): return e.runId
        case .askUser(let e): return e.runId
        case .unknown: return ""
        }
    }

    var seq: UInt64 {
        switch self {
        case .runAccepted: return 0
        case .reasoning(let e): return e.seq
        case .toolStart(let e): return e.seq
        case .toolUpdate(let e): return e.seq
        case .toolEnd(let e): return e.seq
        case .responseChunk(let e): return e.seq
        case .runComplete(let e): return e.seq
        case .runError(let e): return e.seq
        case .askUser(let e): return e.seq
        case .unknown: return 0
        }
    }
}

struct RunAcceptedEvent: Codable, Sendable {
    let runId: String
    let sessionKey: String
    let acceptedAt: String

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case sessionKey = "session_key"
        case acceptedAt = "accepted_at"
    }
}

struct ReasoningEvent: Codable, Sendable {
    let runId: String
    let seq: UInt64
    let content: String
    let isComplete: Bool

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case seq
        case content
        case isComplete = "is_complete"
    }
}

struct ToolStartEvent: Codable, Sendable {
    let runId: String
    let seq: UInt64
    let toolName: String
    let toolId: String
    let params: AnyCodable?

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case seq
        case toolName = "tool_name"
        case toolId = "tool_id"
        case params
    }
}

struct ToolUpdateEvent: Codable, Sendable {
    let runId: String
    let seq: UInt64
    let toolId: String
    let progress: String

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case seq
        case toolId = "tool_id"
        case progress
    }
}

struct ToolEndEvent: Codable, Sendable {
    let runId: String
    let seq: UInt64
    let toolId: String
    let result: ToolResult
    let durationMs: UInt64

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case seq
        case toolId = "tool_id"
        case result
        case durationMs = "duration_ms"
    }
}

struct ToolResult: Codable, Sendable {
    let success: Bool
    let output: String?
    let error: String?
    let metadata: AnyCodable?
}

struct ResponseChunkEvent: Codable, Sendable {
    let runId: String
    let seq: UInt64
    let content: String
    let chunkIndex: UInt32
    let isFinal: Bool

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case seq
        case content
        case chunkIndex = "chunk_index"
        case isFinal = "is_final"
    }
}

struct RunCompleteEvent: Codable, Sendable {
    let runId: String
    let seq: UInt64
    let summary: RunSummary
    let totalDurationMs: UInt64

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case seq
        case summary
        case totalDurationMs = "total_duration_ms"
    }
}

struct RunSummary: Codable, Equatable, Sendable {
    let totalTokens: UInt64
    let toolCalls: UInt32
    let loops: UInt32
    let finalResponse: String?

    enum CodingKeys: String, CodingKey {
        case totalTokens = "total_tokens"
        case toolCalls = "tool_calls"
        case loops
        case finalResponse = "final_response"
    }
}

/// Enhanced run summary with tool details
struct EnhancedRunSummary: Codable, Equatable, Sendable {
    let totalTokens: UInt64
    let toolCalls: UInt32
    let loops: UInt32
    let durationMs: UInt64
    let finalResponse: String?
    let toolSummaries: [ToolSummaryItem]
    let reasoning: String?
    let errors: [ToolErrorItem]

    enum CodingKeys: String, CodingKey {
        case totalTokens = "total_tokens"
        case toolCalls = "tool_calls"
        case loops
        case durationMs = "duration_ms"
        case finalResponse = "final_response"
        case toolSummaries = "tool_summaries"
        case reasoning
        case errors
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        totalTokens = try container.decode(UInt64.self, forKey: .totalTokens)
        toolCalls = try container.decode(UInt32.self, forKey: .toolCalls)
        loops = try container.decode(UInt32.self, forKey: .loops)
        durationMs = try container.decodeIfPresent(UInt64.self, forKey: .durationMs) ?? 0
        finalResponse = try container.decodeIfPresent(String.self, forKey: .finalResponse)
        toolSummaries = try container.decodeIfPresent([ToolSummaryItem].self, forKey: .toolSummaries) ?? []
        reasoning = try container.decodeIfPresent(String.self, forKey: .reasoning)
        errors = try container.decodeIfPresent([ToolErrorItem].self, forKey: .errors) ?? []
    }

    /// Create from basic RunSummary for backwards compatibility
    init(from basic: RunSummary, durationMs: UInt64) {
        self.totalTokens = basic.totalTokens
        self.toolCalls = basic.toolCalls
        self.loops = basic.loops
        self.durationMs = durationMs
        self.finalResponse = basic.finalResponse
        self.toolSummaries = []
        self.reasoning = nil
        self.errors = []
    }

    var hasErrors: Bool { !errors.isEmpty }
}

/// Tool execution summary item
struct ToolSummaryItem: Codable, Equatable, Identifiable, Sendable {
    let toolId: String
    let toolName: String
    let emoji: String
    let displayMeta: String
    let durationMs: UInt64
    let success: Bool

    var id: String { toolId }

    enum CodingKeys: String, CodingKey {
        case toolId = "tool_id"
        case toolName = "tool_name"
        case emoji
        case displayMeta = "display_meta"
        case durationMs = "duration_ms"
        case success
    }

    /// Formatted display string: "🔨 Exec: mkdir -p /tmp"
    var formatted: String {
        if displayMeta.isEmpty {
            return "\(emoji) \(toolName)"
        }
        return "\(emoji) \(toolName): \(displayMeta)"
    }

    /// Short format for list view
    var shortFormatted: String {
        if displayMeta.isEmpty {
            return toolName
        }
        return displayMeta
    }
}

/// Tool error item
struct ToolErrorItem: Codable, Equatable, Sendable {
    let toolName: String
    let error: String
    let toolId: String

    enum CodingKeys: String, CodingKey {
        case toolName = "tool_name"
        case error
        case toolId = "tool_id"
    }
}

struct RunErrorEvent: Codable, Sendable {
    let runId: String
    let seq: UInt64
    let error: String
    let errorCode: String?

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case seq
        case error
        case errorCode = "error_code"
    }
}

struct AskUserEvent: Codable, Sendable, Identifiable {
    let runId: String
    let seq: UInt64
    let questionId: String
    let questions: [UserQuestion]

    // Legacy fields for compatibility
    let question: String?
    let options: [String]?

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case seq
        case questionId = "question_id"
        case questions
        case question
        case options
    }

    var id: String { questionId }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        runId = try container.decode(String.self, forKey: .runId)
        seq = try container.decode(UInt64.self, forKey: .seq)
        questionId = try container.decodeIfPresent(String.self, forKey: .questionId) ?? UUID().uuidString
        questions = try container.decodeIfPresent([UserQuestion].self, forKey: .questions) ?? []
        question = try container.decodeIfPresent(String.self, forKey: .question)
        options = try container.decodeIfPresent([String].self, forKey: .options)
    }
}

/// User question with options
struct UserQuestion: Codable, Sendable, Identifiable {
    let header: String
    let question: String
    let options: [QuestionOption]
    let multiSelect: Bool

    var id: String { header }

    enum CodingKeys: String, CodingKey {
        case header
        case question
        case options
        case multiSelect = "multi_select"
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        header = try container.decode(String.self, forKey: .header)
        question = try container.decode(String.self, forKey: .question)
        options = try container.decode([QuestionOption].self, forKey: .options)
        multiSelect = try container.decodeIfPresent(Bool.self, forKey: .multiSelect) ?? false
    }
}

/// Question option
struct QuestionOption: Codable, Sendable, Identifiable {
    let label: String
    let description: String?

    var id: String { label }
}

// MARK: - RPC Request/Response Types

struct AgentRunParams: Codable, Sendable {
    let input: String
    let sessionKey: String?
    let channel: String?
    let peerId: String?
    let stream: Bool

    init(input: String, sessionKey: String? = nil, channel: String? = nil, peerId: String? = nil, stream: Bool = true) {
        self.input = input
        self.sessionKey = sessionKey
        self.channel = channel
        self.peerId = peerId
        self.stream = stream
    }

    enum CodingKeys: String, CodingKey {
        case input
        case sessionKey = "session_key"
        case channel
        case peerId = "peer_id"
        case stream
    }
}

struct AgentRunResult: Codable, Sendable {
    let runId: String
    let sessionKey: String
    let acceptedAt: String

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case sessionKey = "session_key"
        case acceptedAt = "accepted_at"
    }
}

struct HealthResult: Codable, Sendable {
    let status: String
    let timestamp: String
}

struct VersionResult: Codable, Sendable {
    let name: String
    let version: String
    let `protocol`: String
}

/// Answer parameters for AskUser response
struct AnswerParams: Codable, Sendable {
    let runId: String
    let questionId: String
    let answers: [String: String]

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case questionId = "question_id"
        case answers
    }
}

/// Cancel run parameters
struct CancelParams: Codable, Sendable {
    let runId: String

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
    }
}

/// Subscribe/unsubscribe parameters
struct SubscribeParams: Codable, Sendable {
    let topic: String
}

/// Empty result for void responses
struct EmptyResult: Codable, Sendable {}

// MARK: - AnyCodable Helper

/// Type-erased Codable wrapper for arbitrary JSON values
/// Note: @unchecked Sendable because the wrapped values are JSON primitives
/// (Bool, Int, Double, String, and arrays/dictionaries of these) which are
/// effectively immutable after creation.
struct AnyCodable: Codable, @unchecked Sendable {
    let value: Any

    init(_ value: Any) {
        self.value = value
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()

        if container.decodeNil() {
            value = NSNull()
        } else if let bool = try? container.decode(Bool.self) {
            value = bool
        } else if let int = try? container.decode(Int.self) {
            value = int
        } else if let double = try? container.decode(Double.self) {
            value = double
        } else if let string = try? container.decode(String.self) {
            value = string
        } else if let array = try? container.decode([AnyCodable].self) {
            value = array.map { $0.value }
        } else if let dict = try? container.decode([String: AnyCodable].self) {
            value = dict.mapValues { $0.value }
        } else {
            throw DecodingError.typeMismatch(
                AnyCodable.self,
                DecodingError.Context(codingPath: decoder.codingPath, debugDescription: "Unsupported type")
            )
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()

        switch value {
        case is NSNull:
            try container.encodeNil()
        case let bool as Bool:
            try container.encode(bool)
        case let int as Int:
            try container.encode(int)
        case let double as Double:
            try container.encode(double)
        case let string as String:
            try container.encode(string)
        case let array as [Any]:
            try container.encode(array.map { AnyCodable($0) })
        case let dict as [String: Any]:
            try container.encode(dict.mapValues { AnyCodable($0) })
        default:
            throw EncodingError.invalidValue(
                value,
                EncodingError.Context(codingPath: encoder.codingPath, debugDescription: "Unsupported type")
            )
        }
    }
}
