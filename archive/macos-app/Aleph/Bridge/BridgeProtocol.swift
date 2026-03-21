import Foundation

// MARK: - JSON-RPC 2.0 Types
// Wire-compatible with shared/protocol/src/desktop_bridge.rs

/// JSON-RPC 2.0 request for Desktop Bridge.
///
/// Matches Rust `BridgeRequest`:
/// ```rust
/// pub struct BridgeRequest {
///     pub jsonrpc: String,
///     pub id: String,
///     pub method: String,
///     pub params: Option<serde_json::Value>,
/// }
/// ```
struct BridgeRequest: Codable, Equatable {
    let jsonrpc: String
    let id: String
    let method: String
    let params: [String: AnyCodable]?

    init(id: String, method: String, params: [String: AnyCodable]? = nil) {
        self.jsonrpc = "2.0"
        self.id = id
        self.method = method
        self.params = params
    }
}

/// JSON-RPC 2.0 response — either success or error.
///
/// On the wire, success and error are distinct JSON shapes:
/// - Success: `{ "jsonrpc": "2.0", "id": "...", "result": ... }`
/// - Error:   `{ "jsonrpc": "2.0", "id": "...", "error": { "code": ..., "message": "..." } }`
enum BridgeResponse: Equatable {
    case success(id: String, result: AnyCodable)
    case error(id: String, error: BridgeRpcError)
}

extension BridgeResponse: Encodable {
    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode("2.0", forKey: .jsonrpc)

        switch self {
        case .success(let id, let result):
            try container.encode(id, forKey: .id)
            try container.encode(result, forKey: .result)
        case .error(let id, let error):
            try container.encode(id, forKey: .id)
            try container.encode(error, forKey: .error)
        }
    }

    private enum CodingKeys: String, CodingKey {
        case jsonrpc, id, result, error
    }
}

/// JSON-RPC 2.0 error object.
///
/// Matches Rust `BridgeRpcError`:
/// ```rust
/// pub struct BridgeRpcError { pub code: i32, pub message: String }
/// ```
struct BridgeRpcError: Codable, Equatable {
    let code: Int32
    let message: String

    init(code: BridgeErrorCode, message: String) {
        self.code = code.rawValue
        self.message = message
    }

    init(code: Int32, message: String) {
        self.code = code
        self.message = message
    }
}

// MARK: - Method Constants

/// Desktop Bridge method names.
///
/// Matches the `METHOD_*` constants in `desktop_bridge.rs`.
enum BridgeMethod: String, CaseIterable {
    // Desktop control (Server -> Bridge)
    case ping = "desktop.ping"
    case screenshot = "desktop.screenshot"
    case ocr = "desktop.ocr"
    case axTree = "desktop.ax_tree"
    case click = "desktop.click"
    case typeText = "desktop.type_text"
    case keyCombo = "desktop.key_combo"
    case scroll = "desktop.scroll"
    case launchApp = "desktop.launch_app"
    case windowList = "desktop.window_list"
    case focusWindow = "desktop.focus_window"
    case canvasShow = "desktop.canvas_show"
    case canvasHide = "desktop.canvas_hide"
    case canvasUpdate = "desktop.canvas_update"

    // WebView control (Server -> Bridge)
    case webviewShow = "webview.show"
    case webviewHide = "webview.hide"
    case webviewNavigate = "webview.navigate"

    // Tray control (Server -> Bridge)
    case trayUpdateStatus = "tray.update_status"

    // Bridge lifecycle
    case bridgeShutdown = "bridge.shutdown"

    // Server <-> Bridge handshake / health
    case handshake = "aleph.handshake"
    case systemPing = "system.ping"

    // Capability registration
    case capabilityRegister = "capability.register"
}

// MARK: - Error Codes

/// Standard JSON-RPC 2.0 error codes plus application-specific codes.
///
/// Matches the `ERR_*` constants in `desktop_bridge.rs`.
enum BridgeErrorCode: Int32 {
    case parse = -32700
    case methodNotFound = -32601
    case `internal` = -32603
    case notImplemented = -32000
}

// MARK: - AnyCodable

/// A type-erased `Codable` wrapper for arbitrary JSON values.
///
/// Supports: null, bool, integers, doubles, strings, arrays, and nested dictionaries.
/// This is the Swift equivalent of `serde_json::Value` on the Rust side.
struct AnyCodable: Equatable {
    let value: Any

    init(_ value: Any) {
        self.value = value
    }

    // Convenience accessors

    var boolValue: Bool? { value as? Bool }
    var intValue: Int? {
        if let int = value as? Int { return int }
        if let double = value as? Double, double == double.rounded(.towardZero), !double.isNaN, !double.isInfinite {
            return Int(exactly: double)
        }
        return nil
    }
    var doubleValue: Double? {
        if let double = value as? Double { return double }
        if let int = value as? Int { return Double(int) }
        return nil
    }
    var stringValue: String? { value as? String }
    var arrayValue: [AnyCodable]? { value as? [AnyCodable] }
    var dictValue: [String: AnyCodable]? { value as? [String: AnyCodable] }
    var isNull: Bool { value is NSNull }

    static func == (lhs: AnyCodable, rhs: AnyCodable) -> Bool {
        switch (lhs.value, rhs.value) {
        case is (NSNull, NSNull):
            return true
        case let (l as Bool, r as Bool):
            return l == r
        case let (l as Int, r as Int):
            return l == r
        case let (l as Double, r as Double):
            return l == r
        case let (l as String, r as String):
            return l == r
        case let (l as [AnyCodable], r as [AnyCodable]):
            return l == r
        case let (l as [String: AnyCodable], r as [String: AnyCodable]):
            return l == r
        default:
            return false
        }
    }
}

extension AnyCodable: Codable {
    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()

        if container.decodeNil() {
            self.init(NSNull())
        } else if let bool = try? container.decode(Bool.self) {
            self.init(bool)
        } else if let int = try? container.decode(Int.self) {
            self.init(int)
        } else if let double = try? container.decode(Double.self) {
            self.init(double)
        } else if let string = try? container.decode(String.self) {
            self.init(string)
        } else if let array = try? container.decode([AnyCodable].self) {
            self.init(array)
        } else if let dict = try? container.decode([String: AnyCodable].self) {
            self.init(dict)
        } else {
            throw DecodingError.dataCorruptedError(
                in: container,
                debugDescription: "AnyCodable: unsupported JSON value"
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
        case let array as [AnyCodable]:
            try container.encode(array)
        case let dict as [String: AnyCodable]:
            try container.encode(dict)
        default:
            throw EncodingError.invalidValue(
                value,
                EncodingError.Context(
                    codingPath: encoder.codingPath,
                    debugDescription: "AnyCodable: unsupported value type \(type(of: value))"
                )
            )
        }
    }
}

extension AnyCodable: CustomStringConvertible {
    var description: String {
        switch value {
        case is NSNull:
            return "null"
        case let bool as Bool:
            return bool.description
        case let int as Int:
            return int.description
        case let double as Double:
            return double.description
        case let string as String:
            return "\"\(string)\""
        case let array as [AnyCodable]:
            return array.description
        case let dict as [String: AnyCodable]:
            return dict.description
        default:
            return String(describing: value)
        }
    }
}

// MARK: - AnyCodable Expressible Literals

extension AnyCodable: ExpressibleByNilLiteral {
    init(nilLiteral: ()) {
        self.init(NSNull())
    }
}

extension AnyCodable: ExpressibleByBooleanLiteral {
    init(booleanLiteral value: Bool) {
        self.init(value)
    }
}

extension AnyCodable: ExpressibleByIntegerLiteral {
    init(integerLiteral value: Int) {
        self.init(value)
    }
}

extension AnyCodable: ExpressibleByFloatLiteral {
    init(floatLiteral value: Double) {
        self.init(value)
    }
}

extension AnyCodable: ExpressibleByStringLiteral {
    init(stringLiteral value: String) {
        self.init(value)
    }
}

extension AnyCodable: ExpressibleByArrayLiteral {
    init(arrayLiteral elements: AnyCodable...) {
        self.init(elements)
    }
}

extension AnyCodable: ExpressibleByDictionaryLiteral {
    init(dictionaryLiteral elements: (String, AnyCodable)...) {
        self.init(Dictionary(uniqueKeysWithValues: elements))
    }
}

// MARK: - Shared Value Types

/// Screen region for screenshot/OCR.
///
/// Matches Rust `ScreenRegion`.
struct ScreenRegion: Codable, Equatable {
    let x: Double
    let y: Double
    let width: Double
    let height: Double
}

/// Canvas overlay position.
///
/// Matches Rust `CanvasPosition`.
struct CanvasPosition: Codable, Equatable {
    let x: Double
    let y: Double
    let width: Double
    let height: Double
}

// MARK: - Capability Registration Types

/// A single capability declared by the bridge.
///
/// Matches Rust `BridgeCapabilityInfo`.
struct BridgeCapabilityInfo: Codable, Equatable {
    let name: String
    let version: String
}

/// Capability registration payload (bridge -> server during handshake).
///
/// Matches Rust `CapabilityRegistration`.
struct CapabilityRegistration: Codable, Equatable {
    let platform: String
    let arch: String
    let capabilities: [BridgeCapabilityInfo]
}
