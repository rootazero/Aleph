import Foundation
import os

/// UDS JSON-RPC 2.0 server for the Desktop Bridge.
///
/// Listens on a Unix Domain Socket (`~/.aleph/bridge.sock`), accepts one
/// connection at a time, reads a single JSON-RPC request line, dispatches
/// it to a registered handler, writes the response line, and closes.
///
/// This is the Swift equivalent of the Tauri `bridge/mod.rs`.
///
/// Usage:
/// ```swift
/// let server = BridgeServer()
/// server.register(method: "desktop.screenshot") { params in
///     // ... take screenshot ...
///     return .success(AnyCodable(["image": AnyCodable(base64)]))
/// }
/// try server.start()
/// ```
final class BridgeServer {

    // MARK: - Types

    /// Error returned from a handler to produce a JSON-RPC error response.
    struct HandlerError: Error {
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

    /// A handler receives params and returns either a result or an error.
    typealias Handler = (_ params: [String: AnyCodable]) -> Result<AnyCodable, HandlerError>

    // MARK: - Properties

    private let socketPath: URL
    private var serverFD: Int32 = -1
    private var isAccepting = false
    private var handlers: [String: Handler] = [:]
    private let handlerQueue = DispatchQueue(label: "com.aleph.bridge.handlers")
    private let logger = Logger(subsystem: "com.aleph.app", category: "BridgeServer")

    /// Whether the server is currently listening for connections.
    private(set) var isListening = false

    // MARK: - Init

    init(socketPath: URL? = nil) {
        self.socketPath = socketPath ?? ServerPaths.bridgeSocket
        registerDefaultHandlers()
    }

    // MARK: - Public API

    /// Register a method handler (thread-safe).
    func register(method: String, handler: @escaping Handler) {
        handlerQueue.sync {
            handlers[method] = handler
        }
    }

    /// Start listening on the Unix Domain Socket.
    ///
    /// Creates the socket, binds, sets permissions to 0o700, and starts
    /// the accept loop on a background queue.
    func start() throws {
        guard !isListening else { return }

        // Ensure parent directory exists
        let parentDir = socketPath.deletingLastPathComponent()
        try? FileManager.default.createDirectory(
            at: parentDir,
            withIntermediateDirectories: true
        )

        // Remove stale socket file
        try? FileManager.default.removeItem(at: socketPath)

        // Create UNIX stream socket
        serverFD = socket(AF_UNIX, SOCK_STREAM, 0)
        guard serverFD >= 0 else {
            throw HandlerError(code: .internal, message: "socket() failed: errno \(errno)")
        }

        // Bind to socket path
        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let pathString = socketPath.path
        withUnsafeMutablePointer(to: &addr.sun_path) { ptr in
            pathString.utf8CString.withUnsafeBufferPointer { buf in
                let count = min(buf.count, 104) // sun_path max length
                UnsafeMutableRawPointer(ptr).copyMemory(
                    from: buf.baseAddress!,
                    byteCount: count
                )
            }
        }

        let bindResult = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in
                Darwin.bind(serverFD, sockPtr, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard bindResult == 0 else {
            let err = errno
            Darwin.close(serverFD)
            serverFD = -1
            throw HandlerError(code: .internal, message: "bind() failed: errno \(err)")
        }

        // Set socket file permissions to owner-only (0o700)
        chmod(pathString, 0o700)

        // Start listening with backlog of 5
        guard Darwin.listen(serverFD, 5) == 0 else {
            let err = errno
            Darwin.close(serverFD)
            serverFD = -1
            throw HandlerError(code: .internal, message: "listen() failed: errno \(err)")
        }

        isListening = true
        isAccepting = true
        logger.info("BridgeServer listening on \(pathString)")

        // Run accept loop on a background thread
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            self?.acceptLoop()
        }
    }

    /// Stop the server and clean up the socket file.
    func stop() {
        isAccepting = false
        if serverFD >= 0 {
            Darwin.close(serverFD)
            serverFD = -1
        }
        isListening = false
        try? FileManager.default.removeItem(at: socketPath)
        logger.info("BridgeServer stopped")
    }

    // MARK: - Accept Loop

    private func acceptLoop() {
        while isAccepting {
            var clientAddr = sockaddr_un()
            var addrLen = socklen_t(MemoryLayout<sockaddr_un>.size)

            let clientFD = withUnsafeMutablePointer(to: &clientAddr) { ptr in
                ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in
                    Darwin.accept(serverFD, sockPtr, &addrLen)
                }
            }

            guard clientFD >= 0 else {
                if isAccepting {
                    logger.error("accept() failed: errno \(errno)")
                }
                continue
            }

            // Set 5-second read timeout to prevent idle connections from accumulating
            var tv = timeval(tv_sec: 5, tv_usec: 0)
            setsockopt(clientFD, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))

            // Handle each connection on a separate thread
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                self?.handleConnection(clientFD)
            }
        }
    }

    // MARK: - Connection Handling

    /// Handle a single connection: read one line, parse, dispatch, write response, close.
    private func handleConnection(_ fd: Int32) {
        defer { Darwin.close(fd) }

        // Read one line (up to newline or 64KB limit)
        var buffer = Data()
        buffer.reserveCapacity(4096)
        var byte: UInt8 = 0

        while Darwin.read(fd, &byte, 1) == 1 {
            if byte == 0x0A { break } // newline terminates the request
            buffer.append(byte)
            if buffer.count > 65536 { return } // reject oversized requests
        }

        guard !buffer.isEmpty else { return }

        // Process and respond
        let responseData = processRequest(buffer)
        var output = responseData
        output.append(0x0A) // append newline
        output.withUnsafeBytes { ptr in
            _ = Darwin.write(fd, ptr.baseAddress!, output.count)
        }
    }

    // MARK: - Request Processing

    /// Parse a JSON-RPC request, dispatch to the handler, and return the serialized response.
    private func processRequest(_ data: Data) -> Data {
        // Try to decode using Codable BridgeRequest
        let decoder = JSONDecoder()
        let encoder = JSONEncoder()

        guard let request = try? decoder.decode(BridgeRequest.self, from: data) else {
            // Parse error — cannot determine id
            let response = BridgeResponse.error(
                id: "",
                error: BridgeRpcError(code: .parse, message: "Parse error")
            )
            return (try? encoder.encode(response)) ?? Data()
        }

        let method = request.method
        let requestId = request.id
        let params = request.params ?? [:]

        // Look up handler (thread-safe)
        let handler: Handler? = handlerQueue.sync { handlers[method] }

        let response: BridgeResponse
        if let handler = handler {
            switch handler(params) {
            case .success(let value):
                response = .success(id: requestId, result: value)
            case .failure(let error):
                response = .error(
                    id: requestId,
                    error: BridgeRpcError(code: error.code, message: error.message)
                )
            }
        } else {
            response = .error(
                id: requestId,
                error: BridgeRpcError(code: .methodNotFound, message: "Method not found: \(method)")
            )
        }

        return (try? encoder.encode(response)) ?? Data()
    }

    // MARK: - Default Handlers

    /// Register built-in handlers: ping, system.ping, handshake.
    private func registerDefaultHandlers() {
        // desktop.ping -> "pong"
        handlers[BridgeMethod.ping.rawValue] = { _ in
            .success(AnyCodable("pong"))
        }

        // system.ping -> {"pong": true}
        handlers[BridgeMethod.systemPing.rawValue] = { _ in
            .success(AnyCodable(["pong": AnyCodable(true)]))
        }

        // aleph.handshake -> capabilities list
        handlers[BridgeMethod.handshake.rawValue] = { params in
            let protocolVersion = params["protocol_version"]?.stringValue ?? "1.0"

            let arch: String
            #if arch(arm64)
            arch = "aarch64"
            #else
            arch = "x86_64"
            #endif

            let capabilities: [AnyCodable] = [
                AnyCodable(["name": AnyCodable("screen_capture"), "version": AnyCodable("1.0")]),
                AnyCodable(["name": AnyCodable("webview"), "version": AnyCodable("1.0")]),
                AnyCodable(["name": AnyCodable("tray"), "version": AnyCodable("1.0")]),
                AnyCodable(["name": AnyCodable("global_hotkey"), "version": AnyCodable("1.0")]),
                AnyCodable(["name": AnyCodable("notification"), "version": AnyCodable("1.0")]),
                AnyCodable(["name": AnyCodable("keyboard_control"), "version": AnyCodable("1.0")]),
                AnyCodable(["name": AnyCodable("mouse_control"), "version": AnyCodable("1.0")]),
                AnyCodable(["name": AnyCodable("scroll"), "version": AnyCodable("1.0")]),
                AnyCodable(["name": AnyCodable("canvas"), "version": AnyCodable("1.0")]),
                AnyCodable(["name": AnyCodable("launch_app"), "version": AnyCodable("1.0")]),
                AnyCodable(["name": AnyCodable("window_list"), "version": AnyCodable("1.0")]),
                AnyCodable(["name": AnyCodable("focus_window"), "version": AnyCodable("1.0")]),
                AnyCodable(["name": AnyCodable("ocr"), "version": AnyCodable("1.0")]),
                AnyCodable(["name": AnyCodable("ax_inspect"), "version": AnyCodable("1.0")]),
            ]

            let result: [String: AnyCodable] = [
                "protocol_version": AnyCodable(protocolVersion),
                "bridge_type": AnyCodable("desktop"),
                "platform": AnyCodable("macOS"),
                "arch": AnyCodable(arch),
                "capabilities": AnyCodable(capabilities),
            ]

            return .success(AnyCodable(result))
        }
    }
}
