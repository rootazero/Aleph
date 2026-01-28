import Foundation
import Combine
import os.log

/// Gateway connection state
enum GatewayConnectionState: Equatable {
    case disconnected
    case connecting
    case connected
    case reconnecting(attempt: Int)
    case failed(Error)

    static func == (lhs: GatewayConnectionState, rhs: GatewayConnectionState) -> Bool {
        switch (lhs, rhs) {
        case (.disconnected, .disconnected): return true
        case (.connecting, .connecting): return true
        case (.connected, .connected): return true
        case (.reconnecting(let a), .reconnecting(let b)): return a == b
        case (.failed, .failed): return true
        default: return false
        }
    }
}

/// Gateway client configuration
struct GatewayClientConfig {
    let host: String
    let port: UInt16
    let maxReconnectAttempts: Int
    let reconnectBaseDelay: TimeInterval
    let reconnectMaxDelay: TimeInterval
    let pingInterval: TimeInterval

    static let `default` = GatewayClientConfig(
        host: "127.0.0.1",
        port: 18789,
        maxReconnectAttempts: 10,
        reconnectBaseDelay: 1.0,
        reconnectMaxDelay: 30.0,
        pingInterval: 30.0
    )

    var url: URL {
        URL(string: "ws://\(host):\(port)")!
    }
}

/// WebSocket client for communicating with the Aether Gateway
@MainActor
final class GatewayClient: ObservableObject {
    // MARK: - Published Properties

    @Published private(set) var connectionState: GatewayConnectionState = .disconnected
    @Published private(set) var isConnected: Bool = false

    // MARK: - Private Properties

    private let config: GatewayClientConfig
    private let logger = Logger(subsystem: "com.aether", category: "GatewayClient")

    private var webSocketTask: URLSessionWebSocketTask?
    private var session: URLSession?
    private var pingTask: Task<Void, Never>?
    private var receiveTask: Task<Void, Never>?

    private var pendingRequests: [String: CheckedContinuation<JsonRpcResponse, Error>] = [:]
    private var eventContinuations: [UUID: AsyncStream<StreamEvent>.Continuation] = [:]

    private var reconnectAttempt = 0

    // MARK: - Initialization

    init(config: GatewayClientConfig = .default) {
        self.config = config
    }

    // Note: Disconnection is handled by GatewayManager.shutdown()
    // Cannot call MainActor-isolated disconnect() from nonisolated deinit

    // MARK: - Connection Management

    /// Connect to the Gateway
    func connect() async throws {
        guard connectionState == .disconnected || connectionState.isReconnecting else {
            logger.debug("Already connected or connecting")
            return
        }

        connectionState = .connecting
        logger.info("Connecting to Gateway at \(self.config.url)")

        let configuration = URLSessionConfiguration.default
        configuration.waitsForConnectivity = true
        session = URLSession(configuration: configuration)

        guard let session = session else {
            throw GatewayError.connectionFailed("Failed to create URLSession")
        }

        let task = session.webSocketTask(with: config.url)
        webSocketTask = task
        task.resume()

        // Wait for connection to establish
        do {
            try await waitForConnection()
            connectionState = .connected
            isConnected = true
            reconnectAttempt = 0
            logger.info("Connected to Gateway")

            // Start receive loop
            startReceiveLoop()
            startPingLoop()
        } catch {
            connectionState = .failed(error)
            isConnected = false
            throw error
        }
    }

    /// Disconnect from the Gateway
    func disconnect() {
        logger.info("Disconnecting from Gateway")

        pingTask?.cancel()
        pingTask = nil

        receiveTask?.cancel()
        receiveTask = nil

        webSocketTask?.cancel(with: .goingAway, reason: nil)
        webSocketTask = nil

        session?.invalidateAndCancel()
        session = nil

        // Cancel all pending requests
        for (_, continuation) in pendingRequests {
            continuation.resume(throwing: GatewayError.disconnected)
        }
        pendingRequests.removeAll()

        // Close all event streams
        for (_, continuation) in eventContinuations {
            continuation.finish()
        }
        eventContinuations.removeAll()

        connectionState = .disconnected
        isConnected = false
    }

    // MARK: - RPC Methods

    /// Send a JSON-RPC request and wait for response
    func call<T: Decodable>(method: String, params: Any? = nil) async throws -> T {
        let response = try await sendRequest(method: method, params: params)

        if let error = response.error {
            throw error
        }

        guard let result = response.result else {
            throw GatewayError.invalidResponse("Missing result in response")
        }

        let data = try JSONSerialization.data(withJSONObject: result.value)
        return try JSONDecoder().decode(T.self, from: data)
    }

    /// Send a JSON-RPC request
    func sendRequest(method: String, params: Any? = nil) async throws -> JsonRpcResponse {
        guard isConnected, let webSocketTask = webSocketTask else {
            throw GatewayError.notConnected
        }

        let id = JsonRpcId.generate()
        let request = JsonRpcRequest(method: method, params: params, id: id)

        let data = try JSONEncoder().encode(request)
        guard let jsonString = String(data: data, encoding: .utf8) else {
            throw GatewayError.encodingFailed
        }

        logger.debug("Sending request: \(method)")

        return try await withCheckedThrowingContinuation { continuation in
            if case .string(let idString) = id {
                pendingRequests[idString] = continuation
            }

            Task {
                do {
                    try await webSocketTask.send(.string(jsonString))
                } catch {
                    if case .string(let idString) = id {
                        pendingRequests.removeValue(forKey: idString)
                    }
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    /// Send a notification (no response expected)
    func notify(method: String, params: Any? = nil) async throws {
        guard isConnected, let webSocketTask = webSocketTask else {
            throw GatewayError.notConnected
        }

        let request = JsonRpcRequest.notification(method: method, params: params)
        let data = try JSONEncoder().encode(request)

        guard let jsonString = String(data: data, encoding: .utf8) else {
            throw GatewayError.encodingFailed
        }

        try await webSocketTask.send(.string(jsonString))
    }

    // MARK: - Agent Methods

    /// Run an agent with the given input
    func agentRun(input: String, sessionKey: String? = nil, channel: String? = nil) async throws -> (AgentRunResult, AsyncStream<StreamEvent>) {
        let params = AgentRunParams(input: input, sessionKey: sessionKey, channel: channel)

        // Create event stream first
        let streamId = UUID()
        let stream = AsyncStream<StreamEvent> { continuation in
            eventContinuations[streamId] = continuation

            continuation.onTermination = { [weak self] _ in
                Task { @MainActor in
                    self?.eventContinuations.removeValue(forKey: streamId)
                }
            }
        }

        // Send the request
        let result: AgentRunResult = try await call(method: "agent.run", params: params)

        return (result, stream)
    }

    /// Check Gateway health
    func health() async throws -> HealthResult {
        try await call(method: "health")
    }

    /// Get Gateway version
    func version() async throws -> VersionResult {
        try await call(method: "version")
    }

    // MARK: - Private Methods

    private func waitForConnection() async throws {
        guard let webSocketTask = webSocketTask else {
            throw GatewayError.notConnected
        }

        // Send a ping to verify connection
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            webSocketTask.sendPing { error in
                if let error = error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume()
                }
            }
        }
    }

    private func startReceiveLoop() {
        receiveTask = Task { [weak self] in
            guard let self = self else { return }

            while !Task.isCancelled {
                do {
                    guard let webSocketTask = await self.webSocketTask else { break }
                    let message = try await webSocketTask.receive()
                    await self.handleMessage(message)
                } catch {
                    if !Task.isCancelled {
                        await self.handleDisconnection(error: error)
                    }
                    break
                }
            }
        }
    }

    private func startPingLoop() {
        pingTask = Task { [weak self] in
            guard let self = self else { return }

            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: UInt64(self.config.pingInterval * 1_000_000_000))

                guard !Task.isCancelled, let webSocketTask = await self.webSocketTask else { break }

                webSocketTask.sendPing { error in
                    if error != nil {
                        Task { @MainActor in
                            self.handleDisconnection(error: error!)
                        }
                    }
                }
            }
        }
    }

    private func handleMessage(_ message: URLSessionWebSocketTask.Message) {
        switch message {
        case .string(let text):
            handleTextMessage(text)
        case .data(let data):
            if let text = String(data: data, encoding: .utf8) {
                handleTextMessage(text)
            }
        @unknown default:
            logger.warning("Unknown message type received")
        }
    }

    private func handleTextMessage(_ text: String) {
        guard let data = text.data(using: .utf8) else { return }

        // Try to decode as response first
        if let response = try? JSONDecoder().decode(JsonRpcResponse.self, from: data) {
            handleResponse(response)
            return
        }

        // Try to decode as notification (event)
        if let notification = try? JSONDecoder().decode(JsonRpcRequest.self, from: data) {
            handleNotification(notification)
            return
        }

        logger.warning("Failed to decode message: \(text.prefix(200))")
    }

    private func handleResponse(_ response: JsonRpcResponse) {
        guard let id = response.id else { return }

        var idString: String?
        switch id {
        case .string(let s): idString = s
        case .number(let n): idString = String(n)
        case .null: return
        }

        guard let key = idString, let continuation = pendingRequests.removeValue(forKey: key) else {
            logger.warning("No pending request for id: \(String(describing: idString))")
            return
        }

        continuation.resume(returning: response)
    }

    private func handleNotification(_ notification: JsonRpcRequest) {
        // Check if it's a stream event
        guard notification.method.hasPrefix("stream.") else { return }

        guard let params = notification.params,
              let paramsData = try? JSONSerialization.data(withJSONObject: params.value),
              let event = try? JSONDecoder().decode(StreamEvent.self, from: paramsData) else {
            logger.warning("Failed to decode stream event: \(notification.method)")
            return
        }

        // Forward to all event continuations
        for (_, continuation) in eventContinuations {
            continuation.yield(event)
        }

        // Complete streams on run complete/error
        switch event {
        case .runComplete, .runError:
            // Events will be completed when the stream is closed by the consumer
            break
        default:
            break
        }
    }

    private func handleDisconnection(error: Error) {
        logger.error("Disconnected: \(error.localizedDescription)")

        isConnected = false
        webSocketTask = nil

        // Attempt reconnection
        if reconnectAttempt < config.maxReconnectAttempts {
            reconnectAttempt += 1
            connectionState = .reconnecting(attempt: reconnectAttempt)

            Task {
                let delay = min(
                    config.reconnectBaseDelay * pow(2.0, Double(reconnectAttempt - 1)),
                    config.reconnectMaxDelay
                )
                logger.info("Reconnecting in \(delay)s (attempt \(self.reconnectAttempt))")

                try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))

                if !Task.isCancelled {
                    try? await connect()
                }
            }
        } else {
            connectionState = .failed(error)
        }
    }
}

// MARK: - Helper Extensions

extension GatewayConnectionState {
    var isReconnecting: Bool {
        if case .reconnecting = self { return true }
        return false
    }
}

// MARK: - Gateway Errors

enum GatewayError: LocalizedError {
    case notConnected
    case connectionFailed(String)
    case disconnected
    case encodingFailed
    case invalidResponse(String)
    case timeout

    var errorDescription: String? {
        switch self {
        case .notConnected: return "Not connected to Gateway"
        case .connectionFailed(let msg): return "Connection failed: \(msg)"
        case .disconnected: return "Disconnected from Gateway"
        case .encodingFailed: return "Failed to encode request"
        case .invalidResponse(let msg): return "Invalid response: \(msg)"
        case .timeout: return "Request timed out"
        }
    }
}
