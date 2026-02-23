//
//  GatewayWebSocketClient.swift
//  Aleph
//
//  WebSocket client for connecting to Aleph Gateway
//  Implements JSON-RPC 2.0 protocol
//

import Foundation
import Combine

/// WebSocket client for Aleph Gateway communication
@MainActor
class GatewayWebSocketClient: ObservableObject {
    // MARK: - Published Properties

    @Published private(set) var connectionState: ConnectionState = .disconnected
    @Published private(set) var lastError: String?

    // MARK: - Types

    enum ConnectionState {
        case disconnected
        case connecting
        case connected
        case reconnecting
    }

    // MARK: - Private Properties

    private var webSocketTask: URLSessionWebSocketTask?
    private var session: URLSession
    private let gatewayURL: URL
    private var reconnectTimer: Timer?
    private var reconnectAttempts = 0
    private let maxReconnectAttempts = 5
    private var pendingRequests: [String: CheckedContinuation<JSONRPCResponse, Error>] = [:]
    private var nextRequestId = 1

    // MARK: - Initialization

    init(gatewayURL: URL = URL(string: "ws://127.0.0.1:18789")!) {
        self.gatewayURL = gatewayURL

        let configuration = URLSessionConfiguration.default
        configuration.timeoutIntervalForRequest = 30
        configuration.timeoutIntervalForResource = 300
        self.session = URLSession(configuration: configuration)
    }

    // MARK: - Public Methods

    /// Connect to Gateway
    func connect() {
        guard connectionState == .disconnected else {
            print("[WebSocket] Already connected or connecting")
            return
        }

        connectionState = .connecting
        lastError = nil

        print("[WebSocket] Connecting to \(gatewayURL.absoluteString)")

        webSocketTask = session.webSocketTask(with: gatewayURL)
        webSocketTask?.resume()

        // Start receiving messages
        receiveMessage()

        // Send ping to verify connection
        Task {
            try? await Task.sleep(nanoseconds: 1_000_000_000) // 1 second
            await sendPing()
        }
    }

    /// Disconnect from Gateway
    func disconnect() {
        print("[WebSocket] Disconnecting")
        reconnectTimer?.invalidate()
        reconnectTimer = nil
        webSocketTask?.cancel(with: .goingAway, reason: nil)
        webSocketTask = nil
        connectionState = .disconnected
    }

    /// Send JSON-RPC request
    func sendRequest<T: Decodable>(method: String, params: [String: Any]? = nil) async throws -> T {
        guard connectionState == .connected else {
            throw WebSocketError.notConnected
        }

        let requestId = String(nextRequestId)
        nextRequestId += 1

        let request = JSONRPCRequest(
            jsonrpc: "2.0",
            id: requestId,
            method: method,
            params: params
        )

        let encoder = JSONEncoder()
        let data = try encoder.encode(request)
        let message = URLSessionWebSocketTask.Message.data(data)

        return try await withCheckedThrowingContinuation { continuation in
            pendingRequests[requestId] = continuation

            Task {
                do {
                    try await webSocketTask?.send(message)
                    print("[WebSocket] Sent request: \(method) (id: \(requestId))")
                } catch {
                    pendingRequests.removeValue(forKey: requestId)
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    /// Send ping to keep connection alive
    private func sendPing() async {
        do {
            try await webSocketTask?.sendPing { error in
                if let error = error {
                    print("[WebSocket] Ping failed: \(error)")
                    Task { @MainActor in
                        self.handleDisconnection(error: error)
                    }
                } else {
                    Task { @MainActor in
                        if self.connectionState == .connecting {
                            self.connectionState = .connected
                            self.reconnectAttempts = 0
                            print("[WebSocket] Connected successfully")
                        }
                    }
                }
            }
        } catch {
            print("[WebSocket] Failed to send ping: \(error)")
            handleDisconnection(error: error)
        }
    }

    // MARK: - Private Methods

    private func receiveMessage() {
        webSocketTask?.receive { [weak self] result in
            guard let self = self else { return }

            Task { @MainActor in
                switch result {
                case .success(let message):
                    await self.handleMessage(message)
                    self.receiveMessage() // Continue receiving

                case .failure(let error):
                    print("[WebSocket] Receive error: \(error)")
                    self.handleDisconnection(error: error)
                }
            }
        }
    }

    private func handleMessage(_ message: URLSessionWebSocketTask.Message) async {
        do {
            let data: Data
            switch message {
            case .data(let messageData):
                data = messageData
            case .string(let text):
                guard let textData = text.data(using: .utf8) else {
                    print("[WebSocket] Failed to convert string to data")
                    return
                }
                data = textData
            @unknown default:
                print("[WebSocket] Unknown message type")
                return
            }

            let decoder = JSONDecoder()
            let response = try decoder.decode(JSONRPCResponse.self, from: data)

            // Handle response
            if let requestId = response.id,
               let continuation = pendingRequests.removeValue(forKey: requestId) {
                if let error = response.error {
                    continuation.resume(throwing: WebSocketError.rpcError(error.message))
                } else if let result = response.result {
                    continuation.resume(returning: response)
                } else {
                    continuation.resume(throwing: WebSocketError.invalidResponse)
                }
            } else {
                // Handle notification (no id)
                print("[WebSocket] Received notification: \(String(data: data, encoding: .utf8) ?? "unknown")")
            }
        } catch {
            print("[WebSocket] Failed to decode message: \(error)")
        }
    }

    private func handleDisconnection(error: Error) {
        print("[WebSocket] Disconnected: \(error)")
        lastError = error.localizedDescription

        webSocketTask = nil

        // Cancel all pending requests
        for (_, continuation) in pendingRequests {
            continuation.resume(throwing: WebSocketError.disconnected)
        }
        pendingRequests.removeAll()

        // Attempt reconnection
        if reconnectAttempts < maxReconnectAttempts {
            connectionState = .reconnecting
            reconnectAttempts += 1

            let delay = min(pow(2.0, Double(reconnectAttempts)), 30.0) // Exponential backoff, max 30s
            print("[WebSocket] Reconnecting in \(delay)s (attempt \(reconnectAttempts)/\(maxReconnectAttempts))")

            reconnectTimer = Timer.scheduledTimer(withTimeInterval: delay, repeats: false) { [weak self] _ in
                Task { @MainActor in
                    self?.connectionState = .disconnected
                    self?.connect()
                }
            }
        } else {
            connectionState = .disconnected
            print("[WebSocket] Max reconnection attempts reached")
        }
    }
}

// MARK: - JSON-RPC Types

struct JSONRPCRequest: Codable {
    let jsonrpc: String
    let id: String
    let method: String
    let params: [String: Any]?

    enum CodingKeys: String, CodingKey {
        case jsonrpc, id, method, params
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(jsonrpc, forKey: .jsonrpc)
        try container.encode(id, forKey: .id)
        try container.encode(method, forKey: .method)

        if let params = params {
            let jsonData = try JSONSerialization.data(withJSONObject: params)
            let jsonObject = try JSONSerialization.jsonObject(with: jsonData)
            try container.encode(AnyCodable(jsonObject), forKey: .params)
        }
    }
}

struct JSONRPCResponse: Codable {
    let jsonrpc: String
    let id: String?
    let result: AnyCodable?
    let error: JSONRPCError?
}

struct JSONRPCError: Codable {
    let code: Int
    let message: String
    let data: AnyCodable?
}

// Helper for encoding/decoding Any
private struct AnyCodable: Codable {
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
            try container.encodeNil()
        }
    }
}

// MARK: - Errors

enum WebSocketError: LocalizedError {
    case notConnected
    case disconnected
    case rpcError(String)
    case invalidResponse

    var errorDescription: String? {
        switch self {
        case .notConnected:
            return "Not connected to Gateway"
        case .disconnected:
            return "Disconnected from Gateway"
        case .rpcError(let message):
            return "RPC Error: \(message)"
        case .invalidResponse:
            return "Invalid response from Gateway"
        }
    }
}
