import Foundation

actor Router {
    private var handlers: [String: (JSONValue?) async throws -> JSONValue] = [:]

    func register(
        _ method: String,
        handler: @escaping @Sendable (JSONValue?) async throws -> JSONValue
    ) {
        handlers[method] = handler
    }

    func supportedMethods() -> [String] {
        Array(handlers.keys).sorted()
    }

    func handle(method: String, params: JSONValue?) async throws -> JSONValue {
        guard let h = handlers[method] else {
            throw RpcError(
                code: -32601,
                message: "method not found: \(method)",
                data: nil
            )
        }
        return try await h(params)
    }
}
