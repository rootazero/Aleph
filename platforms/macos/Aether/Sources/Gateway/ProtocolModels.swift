import Foundation

// MARK: - JSON-RPC 2.0 Protocol Types

/// JSON-RPC 2.0 Request
struct JsonRpcRequest: Codable {
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
struct JsonRpcResponse: Codable {
    let jsonrpc: String
    let result: AnyCodable?
    let error: JsonRpcError?
    let id: JsonRpcId?

    var isSuccess: Bool { error == nil }
    var isError: Bool { error != nil }
}

/// JSON-RPC 2.0 Error
struct JsonRpcError: Codable, Error {
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
enum JsonRpcId: Codable, Equatable {
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
enum StreamEvent: Codable {
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

struct RunAcceptedEvent: Codable {
    let runId: String
    let sessionKey: String
    let acceptedAt: String

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case sessionKey = "session_key"
        case acceptedAt = "accepted_at"
    }
}

struct ReasoningEvent: Codable {
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

struct ToolStartEvent: Codable {
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

struct ToolUpdateEvent: Codable {
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

struct ToolEndEvent: Codable {
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

struct ToolResult: Codable {
    let success: Bool
    let output: String?
    let error: String?
    let metadata: AnyCodable?
}

struct ResponseChunkEvent: Codable {
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

struct RunCompleteEvent: Codable {
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

struct RunSummary: Codable {
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

struct RunErrorEvent: Codable {
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

struct AskUserEvent: Codable {
    let runId: String
    let seq: UInt64
    let question: String
    let options: [String]

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case seq
        case question
        case options
    }
}

// MARK: - RPC Request/Response Types

struct AgentRunParams: Codable {
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

struct AgentRunResult: Codable {
    let runId: String
    let sessionKey: String
    let acceptedAt: String

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case sessionKey = "session_key"
        case acceptedAt = "accepted_at"
    }
}

struct HealthResult: Codable {
    let status: String
    let timestamp: String
}

struct VersionResult: Codable {
    let name: String
    let version: String
    let `protocol`: String
}

// MARK: - AnyCodable Helper

/// Type-erased Codable wrapper for arbitrary JSON values
struct AnyCodable: Codable {
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
