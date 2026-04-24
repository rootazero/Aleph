import XCTest
@testable import AlephBridge

/// Golden-fixtures test: validates that the hand-written Swift Codable
/// envelope structs (Request/Response/ErrorResponse/RpcError/Notification)
/// agree with the Rust `schemars` output bundled as `Fixtures/schema.json`.
///
/// Payload-level structs (HandshakeParams, CaptureResult, etc.) are validated
/// in later stages (T1a.2, T1b, T2, T3.2, T4.2) as their typed Swift structs
/// are introduced.
final class GoldenFixturesTests: XCTestCase {
    // MARK: - Schema loading (one-shot cache)

    private static let schema: [String: Any] = {
        guard let url = Bundle.module.url(forResource: "schema", withExtension: "json", subdirectory: "Fixtures")
            ?? Bundle.module.url(forResource: "schema", withExtension: "json") else {
            fatalError("schema.json not found in test bundle; Package.swift must copy Fixtures/")
        }
        guard let data = try? Data(contentsOf: url),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            fatalError("schema.json could not be parsed at \(url.path)")
        }
        return obj
    }()

    // MARK: - Expected required-field sets (hand-listed per spec)

    private static let expectedRequired: [String: Set<String>] = [
        "Request": ["id", "jsonrpc", "method"],
        "Response": ["id", "jsonrpc", "result"],
        "ErrorResponse": ["error", "jsonrpc"],
        "RpcError": ["code", "message"],
        "Notification": ["jsonrpc", "method"],
    ]

    // MARK: - Helpers

    private func schemaPropertyKeys(typeName: String) throws -> [String] {
        guard let type = Self.schema[typeName] as? [String: Any] else {
            XCTFail("schema.json missing top-level type '\(typeName)'")
            return []
        }
        guard let props = type["properties"] as? [String: Any] else {
            XCTFail("schema type '\(typeName)' has no 'properties' object")
            return []
        }
        return props.keys.sorted()
    }

    private func schemaRequiredKeys(typeName: String) throws -> Set<String> {
        guard let type = Self.schema[typeName] as? [String: Any] else {
            XCTFail("schema.json missing top-level type '\(typeName)'")
            return []
        }
        let required = (type["required"] as? [String]) ?? []
        return Set(required)
    }

    private func swiftJsonKeys<T: Encodable>(_ value: T) throws -> [String] {
        let data = try JSONEncoder().encode(value)
        guard let obj = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            XCTFail("encoded value did not decode to a JSON object")
            return []
        }
        return obj.keys.sorted()
    }

    private func assertEnvelopeMatches<T: Encodable>(
        typeName: String,
        sample: T,
        file: StaticString = #filePath,
        line: UInt = #line
    ) throws {
        // 1. Property-key parity: Swift emitted JSON keys == schema properties keys
        let swiftKeys = try swiftJsonKeys(sample)
        let schemaKeys = try schemaPropertyKeys(typeName: typeName)
        XCTAssertEqual(
            swiftKeys,
            schemaKeys,
            """
            [\(typeName)] property-key parity mismatch
              swift keys:  \(swiftKeys)
              schema keys: \(schemaKeys)
              only in swift:  \(Set(swiftKeys).subtracting(schemaKeys).sorted())
              only in schema: \(Set(schemaKeys).subtracting(swiftKeys).sorted())
            """,
            file: file,
            line: line
        )

        // 2. Required-field parity: schema `required` matches hand-listed expected set
        let actualRequired = try schemaRequiredKeys(typeName: typeName)
        let expectedRequired = Self.expectedRequired[typeName] ?? []
        XCTAssertEqual(
            actualRequired,
            expectedRequired,
            """
            [\(typeName)] required-field parity mismatch
              schema required:   \(actualRequired.sorted())
              expected required: \(expectedRequired.sorted())
              only in schema:    \(actualRequired.subtracting(expectedRequired).sorted())
              only in expected:  \(expectedRequired.subtracting(actualRequired).sorted())
            """,
            file: file,
            line: line
        )
    }

    // MARK: - Sample factories

    private func sampleRequest() -> Request {
        Request(jsonrpc: "2.0", id: 1, method: "bridge.ping", params: .object([:]))
    }

    private func sampleResponse() -> Response {
        Response(id: 1, result: .object([:]))
    }

    private func sampleErrorResponse() -> ErrorResponse {
        ErrorResponse(id: 1, error: RpcError(code: -32_603, message: "internal", data: nil))
    }

    private func sampleRpcError() -> RpcError {
        RpcError(code: -32_601, message: "method not found", data: .null)
    }

    private func sampleNotification() -> RpcNotification {
        RpcNotification(method: "bridge.event", params: .object([:]))
    }

    // MARK: - Tests (one per envelope)

    func testRequestMatchesSchema() throws {
        try assertEnvelopeMatches(typeName: "Request", sample: sampleRequest())
    }

    func testResponseMatchesSchema() throws {
        try assertEnvelopeMatches(typeName: "Response", sample: sampleResponse())
    }

    func testErrorResponseMatchesSchema() throws {
        try assertEnvelopeMatches(typeName: "ErrorResponse", sample: sampleErrorResponse())
    }

    func testRpcErrorMatchesSchema() throws {
        try assertEnvelopeMatches(typeName: "RpcError", sample: sampleRpcError())
    }

    func testNotificationMatchesSchema() throws {
        try assertEnvelopeMatches(typeName: "Notification", sample: sampleNotification())
    }
}
