import Foundation
import Combine
import os.log

/// Manages the Gateway process lifecycle and client connection
///
/// Responsibilities:
/// - Launch Gateway process if not running
/// - Monitor Gateway health
/// - Manage client connection
/// - Handle process lifecycle
@MainActor
final class GatewayManager: ObservableObject {
    // MARK: - Singleton

    static let shared = GatewayManager()

    // MARK: - Published State

    @Published private(set) var processState: ProcessState = .unknown
    @Published private(set) var clientState: GatewayConnectionState = .disconnected
    @Published private(set) var isReady: Bool = false
    @Published private(set) var lastError: Error?

    // MARK: - Public Properties

    let client: GatewayClient
    let eventStream: EventStreamManager

    // MARK: - Private Properties

    private let logger = Logger(subsystem: "com.aleph", category: "GatewayManager")
    private let config: GatewayClientConfig
    private var gatewayProcess: Process?
    private var healthCheckTask: Task<Void, Never>?

    // MARK: - Types

    enum ProcessState: Equatable {
        case unknown
        case notRunning
        case starting
        case running(pid: Int32)
        case failed(String)
    }

    // MARK: - Initialization

    private init(config: GatewayClientConfig = .default) {
        self.config = config
        self.client = GatewayClient(config: config)
        self.eventStream = EventStreamManager()
    }

    // MARK: - Lifecycle

    /// Initialize the Gateway (launch process if needed and connect)
    func initialize() async throws {
        logger.info("Initializing Gateway")

        // Check if Gateway is already running
        if await isGatewayRunning() {
            logger.info("Gateway already running")
            processState = .running(pid: 0) // PID unknown for external process
        } else {
            // Launch Gateway process
            try await launchGateway()
        }

        // Wait for port to be available
        try await waitForPort()

        // Connect client
        try await client.connect()

        // Verify connection with health check
        let health = try await client.health()
        logger.info("Gateway health: \(health.status)")

        isReady = true
        startHealthMonitoring()
    }

    /// Shutdown the Gateway connection (but not the process)
    func shutdown() {
        logger.info("Shutting down Gateway connection")

        healthCheckTask?.cancel()
        healthCheckTask = nil

        client.disconnect()
        isReady = false
    }

    /// Terminate the Gateway process
    func terminate() {
        logger.info("Terminating Gateway process")
        shutdown()

        gatewayProcess?.terminate()
        gatewayProcess = nil
        processState = .notRunning
    }

    // MARK: - Agent Operations

    /// Run an agent with input
    func runAgent(input: String, sessionKey: String? = nil) async throws {
        guard isReady else {
            throw GatewayError.notConnected
        }

        let (result, stream) = try await client.agentRun(input: input, sessionKey: sessionKey)
        logger.info("Agent run started: \(result.runId)")

        // Start processing events
        eventStream.startRun(runId: result.runId, stream: stream)
    }

    // MARK: - Private Methods

    private func isGatewayRunning() async -> Bool {
        // Try to connect to the port
        let socket = Socket()
        defer { socket.close() }

        return socket.canConnect(host: config.host, port: config.port)
    }

    private func launchGateway() async throws {
        processState = .starting
        logger.info("Launching Gateway process")

        // Find the gateway binary
        let binaryPath = findGatewayBinary()

        guard let path = binaryPath else {
            let error = "Gateway binary not found"
            processState = .failed(error)
            throw GatewayError.connectionFailed(error)
        }

        let process = Process()
        process.executableURL = URL(fileURLWithPath: path)
        process.arguments = [
            "--bind", config.host,
            "--port", String(config.port)
        ]

        // Redirect output to log
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe

        pipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            if let output = String(data: data, encoding: .utf8), !output.isEmpty {
                self?.logger.debug("Gateway: \(output.trimmingCharacters(in: .whitespacesAndNewlines))")
            }
        }

        process.terminationHandler = { [weak self] process in
            Task { @MainActor in
                self?.logger.info("Gateway process terminated with code: \(process.terminationStatus)")
                self?.processState = .notRunning
                self?.isReady = false
            }
        }

        do {
            try process.run()
            gatewayProcess = process
            processState = .running(pid: process.processIdentifier)
            logger.info("Gateway process started with PID: \(process.processIdentifier)")
        } catch {
            processState = .failed(error.localizedDescription)
            throw error
        }
    }

    private func findGatewayBinary() -> String? {
        // Check common locations
        let candidates = [
            // Development build
            Bundle.main.bundlePath + "/Contents/MacOS/aleph-gateway",
            // Cargo target directory (debug)
            FileManager.default.homeDirectoryForCurrentUser.path + "/Workspace/Aleph/target/debug/aleph-gateway",
            // Cargo target directory (release)
            FileManager.default.homeDirectoryForCurrentUser.path + "/Workspace/Aleph/target/release/aleph-gateway",
            // System path
            "/usr/local/bin/aleph-gateway",
            "/opt/homebrew/bin/aleph-gateway"
        ]

        for path in candidates {
            if FileManager.default.isExecutableFile(atPath: path) {
                return path
            }
        }

        return nil
    }

    private func waitForPort(timeout: TimeInterval = 5.0) async throws {
        let startTime = Date()
        var delay: TimeInterval = 0.1

        while Date().timeIntervalSince(startTime) < timeout {
            if await isGatewayRunning() {
                logger.debug("Port \(self.config.port) is available")
                return
            }

            try await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
            delay = min(delay * 2, 1.0) // Exponential backoff, max 1s
        }

        throw GatewayError.timeout(method: "waitForPort", timeout: timeout)
    }

    private func startHealthMonitoring() {
        healthCheckTask = Task {
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 30_000_000_000) // 30s

                guard !Task.isCancelled else { break }

                do {
                    let health = try await client.health()
                    if health.status != "healthy" {
                        logger.warning("Gateway health check failed: \(health.status)")
                    }
                } catch {
                    logger.error("Health check error: \(error.localizedDescription)")
                    lastError = error
                }
            }
        }
    }
}

// MARK: - Simple Socket Helper

private class Socket {
    private var socketFd: Int32 = -1

    func canConnect(host: String, port: UInt16) -> Bool {
        socketFd = socket(AF_INET, SOCK_STREAM, 0)
        guard socketFd >= 0 else { return false }

        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = port.bigEndian
        addr.sin_addr.s_addr = inet_addr(host)

        // Set non-blocking
        let flags = fcntl(socketFd, F_GETFL, 0)
        fcntl(socketFd, F_SETFL, flags | O_NONBLOCK)

        let result = withUnsafePointer(to: &addr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.connect(socketFd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }

        if result == 0 {
            return true
        }

        // Check if connection is in progress
        if errno == EINPROGRESS {
            var writeSet = fd_set()
            __darwin_fd_zero(&writeSet)

            let fd = socketFd
            withUnsafeMutablePointer(to: &writeSet) { ptr in
                let rawPtr = UnsafeMutableRawPointer(ptr)
                let fdSetPtr = rawPtr.assumingMemoryBound(to: Int32.self)
                fdSetPtr[Int(fd) / 32] |= Int32(1 << (Int(fd) % 32))
            }

            var timeout = timeval(tv_sec: 0, tv_usec: 100_000) // 100ms
            let selectResult = select(socketFd + 1, nil, &writeSet, nil, &timeout)

            if selectResult > 0 {
                var error: Int32 = 0
                var len = socklen_t(MemoryLayout<Int32>.size)
                getsockopt(socketFd, SOL_SOCKET, SO_ERROR, &error, &len)
                return error == 0
            }
        }

        return false
    }

    func close() {
        if socketFd >= 0 {
            Darwin.close(socketFd)
            socketFd = -1
        }
    }
}

// MARK: - fd_set helpers

private func __darwin_fd_zero(_ set: inout fd_set) {
    withUnsafeMutableBytes(of: &set) { buffer in
        buffer.baseAddress?.initializeMemory(as: UInt8.self, repeating: 0, count: buffer.count)
    }
}
