import Foundation

// MARK: - JSONValue (type-erased JSON scalar)

/// Codable JSON tree value: null / bool / number / string / array / object.
/// Mirrors Rust's `serde_json::Value` so we can decode `params` once and
/// re-encode the result back out.
enum JSONValue: Codable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
            return
        }
        if let b = try? container.decode(Bool.self) {
            self = .bool(b); return
        }
        if let n = try? container.decode(Double.self) {
            self = .number(n); return
        }
        if let s = try? container.decode(String.self) {
            self = .string(s); return
        }
        if let a = try? container.decode([JSONValue].self) {
            self = .array(a); return
        }
        if let o = try? container.decode([String: JSONValue].self) {
            self = .object(o); return
        }
        throw DecodingError.typeMismatch(
            JSONValue.self,
            DecodingError.Context(
                codingPath: container.codingPath,
                debugDescription: "Unknown JSON value"
            )
        )
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null: try container.encodeNil()
        case .bool(let b): try container.encode(b)
        case .number(let n): try container.encode(n)
        case .string(let s): try container.encode(s)
        case .array(let a): try container.encode(a)
        case .object(let o): try container.encode(o)
        }
    }
}

// MARK: - JSON-RPC envelopes

struct Request: Codable {
    let jsonrpc: String
    let id: UInt64
    let method: String
    let params: JSONValue?
}

struct Response: Codable {
    var jsonrpc: String = "2.0"
    let id: UInt64
    let result: JSONValue
}

struct ErrorResponse: Codable {
    var jsonrpc: String = "2.0"
    let id: UInt64?
    let error: RpcError
}

struct RpcError: Codable, Error {
    let code: Int32
    let message: String
    let data: JSONValue?
}

struct Notification: Codable {
    var jsonrpc: String = "2.0"
    let method: String
    let params: JSONValue?
}
