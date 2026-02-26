import XCTest
@testable import Aleph

final class BridgeServerTests: XCTestCase {

    /// Unique socket path per test to avoid conflicts.
    ///
    /// Uses `/tmp/` directly with a short name to stay within the 104-byte
    /// `sun_path` limit for Unix Domain Sockets on macOS.
    private func tempSocketPath() -> URL {
        // Use short random suffix to keep path under 104 bytes
        let shortId = UUID().uuidString.prefix(8)
        let dir = URL(fileURLWithPath: "/tmp/abt-\(shortId)")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("b.sock")
    }

    /// Send a JSON-RPC request to the socket and return the parsed response.
    private func sendRequest(
        socketPath: URL,
        method: String,
        id: String = "test-1",
        params: [String: Any]? = nil
    ) throws -> [String: Any] {
        // Build request JSON
        var request: [String: Any] = [
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
        ]
        if let params = params {
            request["params"] = params
        }
        var line = try JSONSerialization.data(withJSONObject: request)
        line.append(0x0A) // newline

        // Connect via POSIX socket
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        XCTAssertGreaterThanOrEqual(fd, 0, "socket() failed")
        defer { close(fd) }

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let path = socketPath.path
        withUnsafeMutablePointer(to: &addr.sun_path) { ptr in
            path.utf8CString.withUnsafeBufferPointer { buf in
                UnsafeMutableRawPointer(ptr).copyMemory(
                    from: buf.baseAddress!,
                    byteCount: min(buf.count, 104)
                )
            }
        }

        let connectResult = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in
                connect(fd, sockPtr, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        XCTAssertEqual(connectResult, 0, "connect() failed: errno \(errno)")

        // Write request
        line.withUnsafeBytes { ptr in
            _ = write(fd, ptr.baseAddress!, line.count)
        }

        // Read response (up to newline)
        var responseBuffer = Data()
        var byte: UInt8 = 0
        while read(fd, &byte, 1) == 1 {
            if byte == 0x0A { break }
            responseBuffer.append(byte)
        }

        XCTAssertFalse(responseBuffer.isEmpty, "Empty response")

        let json = try JSONSerialization.jsonObject(with: responseBuffer) as! [String: Any]
        return json
    }

    // MARK: - Tests

    func testStartAndStop() throws {
        let path = tempSocketPath()
        let server = BridgeServer(socketPath: path)

        XCTAssertFalse(server.isListening)

        try server.start()
        XCTAssertTrue(server.isListening)

        // Socket file should exist
        XCTAssertTrue(FileManager.default.fileExists(atPath: path.path))

        server.stop()
        XCTAssertFalse(server.isListening)

        // Socket file should be cleaned up
        XCTAssertFalse(FileManager.default.fileExists(atPath: path.path))

        // Clean up temp directory
        try? FileManager.default.removeItem(at: path.deletingLastPathComponent())
    }

    func testStartIdempotent() throws {
        let path = tempSocketPath()
        let server = BridgeServer(socketPath: path)
        defer {
            server.stop()
            try? FileManager.default.removeItem(at: path.deletingLastPathComponent())
        }

        try server.start()
        // Second start should be a no-op (not throw)
        try server.start()
        XCTAssertTrue(server.isListening)
    }

    func testPingReturnsPong() throws {
        let path = tempSocketPath()
        let server = BridgeServer(socketPath: path)
        try server.start()
        defer {
            server.stop()
            try? FileManager.default.removeItem(at: path.deletingLastPathComponent())
        }

        // Give the accept loop a moment to start
        Thread.sleep(forTimeInterval: 0.1)

        let response = try sendRequest(socketPath: path, method: "desktop.ping")

        XCTAssertEqual(response["jsonrpc"] as? String, "2.0")
        XCTAssertEqual(response["id"] as? String, "test-1")
        XCTAssertEqual(response["result"] as? String, "pong")
        XCTAssertNil(response["error"])
    }

    func testSystemPingReturnsPongTrue() throws {
        let path = tempSocketPath()
        let server = BridgeServer(socketPath: path)
        try server.start()
        defer {
            server.stop()
            try? FileManager.default.removeItem(at: path.deletingLastPathComponent())
        }

        Thread.sleep(forTimeInterval: 0.1)

        let response = try sendRequest(socketPath: path, method: "system.ping")

        XCTAssertEqual(response["jsonrpc"] as? String, "2.0")
        XCTAssertEqual(response["id"] as? String, "test-1")

        let result = response["result"] as? [String: Any]
        XCTAssertNotNil(result)
        XCTAssertEqual(result?["pong"] as? Bool, true)
    }

    func testHandshakeReturnsCapabilities() throws {
        let path = tempSocketPath()
        let server = BridgeServer(socketPath: path)
        try server.start()
        defer {
            server.stop()
            try? FileManager.default.removeItem(at: path.deletingLastPathComponent())
        }

        Thread.sleep(forTimeInterval: 0.1)

        let response = try sendRequest(
            socketPath: path,
            method: "aleph.handshake",
            params: ["protocol_version": "1.0"]
        )

        XCTAssertEqual(response["jsonrpc"] as? String, "2.0")
        XCTAssertNil(response["error"])

        let result = response["result"] as? [String: Any]
        XCTAssertNotNil(result)
        XCTAssertEqual(result?["protocol_version"] as? String, "1.0")
        XCTAssertEqual(result?["bridge_type"] as? String, "desktop")
        XCTAssertEqual(result?["platform"] as? String, "macOS")

        let arch = result?["arch"] as? String
        XCTAssertNotNil(arch)
        #if arch(arm64)
        XCTAssertEqual(arch, "aarch64")
        #else
        XCTAssertEqual(arch, "x86_64")
        #endif

        let capabilities = result?["capabilities"] as? [[String: Any]]
        XCTAssertNotNil(capabilities)
        XCTAssertGreaterThanOrEqual(capabilities?.count ?? 0, 12)

        // Check that expected capabilities are present
        let capNames = capabilities?.compactMap { $0["name"] as? String } ?? []
        XCTAssertTrue(capNames.contains("screen_capture"))
        XCTAssertTrue(capNames.contains("ocr"))
        XCTAssertTrue(capNames.contains("keyboard_control"))
        XCTAssertTrue(capNames.contains("canvas"))
    }

    func testUnknownMethodReturnsError() throws {
        let path = tempSocketPath()
        let server = BridgeServer(socketPath: path)
        try server.start()
        defer {
            server.stop()
            try? FileManager.default.removeItem(at: path.deletingLastPathComponent())
        }

        Thread.sleep(forTimeInterval: 0.1)

        let response = try sendRequest(socketPath: path, method: "nonexistent.method")

        XCTAssertEqual(response["jsonrpc"] as? String, "2.0")
        XCTAssertEqual(response["id"] as? String, "test-1")
        XCTAssertNil(response["result"])

        let error = response["error"] as? [String: Any]
        XCTAssertNotNil(error)
        XCTAssertEqual(error?["code"] as? Int, -32601)
        XCTAssertTrue((error?["message"] as? String)?.contains("nonexistent.method") ?? false)
    }

    func testMultipleSequentialConnections() throws {
        let path = tempSocketPath()
        let server = BridgeServer(socketPath: path)
        try server.start()
        defer {
            server.stop()
            try? FileManager.default.removeItem(at: path.deletingLastPathComponent())
        }

        Thread.sleep(forTimeInterval: 0.1)

        // Send 5 sequential requests, each on a fresh connection
        for i in 1...5 {
            let response = try sendRequest(
                socketPath: path,
                method: "desktop.ping",
                id: "seq-\(i)"
            )
            XCTAssertEqual(response["id"] as? String, "seq-\(i)")
            XCTAssertEqual(response["result"] as? String, "pong")
        }
    }

    func testCustomHandlerRegistration() throws {
        let path = tempSocketPath()
        let server = BridgeServer(socketPath: path)

        // Register a custom handler before starting
        server.register(method: "custom.echo") { params in
            let text = params["text"]?.stringValue ?? "no text"
            return .success(AnyCodable(["echo": AnyCodable(text)]))
        }

        try server.start()
        defer {
            server.stop()
            try? FileManager.default.removeItem(at: path.deletingLastPathComponent())
        }

        Thread.sleep(forTimeInterval: 0.1)

        let response = try sendRequest(
            socketPath: path,
            method: "custom.echo",
            params: ["text": "hello"]
        )

        let result = response["result"] as? [String: Any]
        XCTAssertEqual(result?["echo"] as? String, "hello")
    }

    func testHandlerReturningError() throws {
        let path = tempSocketPath()
        let server = BridgeServer(socketPath: path)

        server.register(method: "custom.fail") { _ in
            .failure(BridgeServer.HandlerError(
                code: .notImplemented,
                message: "Not implemented yet"
            ))
        }

        try server.start()
        defer {
            server.stop()
            try? FileManager.default.removeItem(at: path.deletingLastPathComponent())
        }

        Thread.sleep(forTimeInterval: 0.1)

        let response = try sendRequest(socketPath: path, method: "custom.fail")

        let error = response["error"] as? [String: Any]
        XCTAssertNotNil(error)
        XCTAssertEqual(error?["code"] as? Int, -32000)
        XCTAssertEqual(error?["message"] as? String, "Not implemented yet")
    }

    func testSocketPermissions() throws {
        let path = tempSocketPath()
        let server = BridgeServer(socketPath: path)
        try server.start()
        defer {
            server.stop()
            try? FileManager.default.removeItem(at: path.deletingLastPathComponent())
        }

        // Check socket file permissions are owner-only (0o700)
        let attrs = try FileManager.default.attributesOfItem(atPath: path.path)
        let posixPerms = attrs[.posixPermissions] as? Int
        XCTAssertNotNil(posixPerms)
        // 0o700 = 448 decimal
        XCTAssertEqual(posixPerms, 0o700)
    }
}
