//
//  DesktopBridgeServer.swift
//  Aleph
//
//  Listens on ~/.aleph/desktop.sock and dispatches JSON-RPC 2.0 requests.
//
//  Each request opens a connection, receives one newline-terminated JSON-RPC message,
//  dispatches to the appropriate handler, and writes one response.
//

import Foundation

// Marked as @unchecked Sendable because all mutable state (serverFd, isRunning) is
// only accessed from a single background accept-loop thread after start() returns.
// The Swift 6 compiler cannot verify this statically, so we assert the invariant here.
final class DesktopBridgeServer: @unchecked Sendable {
    static let shared = DesktopBridgeServer()

    private var serverFd: Int32 = -1
    private var isRunning = false

    private var socketPath: String {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        return "\(home)/.aleph/desktop.sock"
    }

    func start() {
        guard !isRunning else { return }

        // Ensure ~/.aleph directory exists
        let dir = (socketPath as NSString).deletingLastPathComponent
        try? FileManager.default.createDirectory(
            atPath: dir,
            withIntermediateDirectories: true,
            attributes: nil
        )

        // Remove stale socket file
        try? FileManager.default.removeItem(atPath: socketPath)

        // Create UNIX domain socket
        serverFd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard serverFd >= 0 else {
            print("[DesktopBridge] Failed to create socket: \(errno)")
            return
        }

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        _ = withUnsafeMutablePointer(to: &addr.sun_path.0) { ptr in
            socketPath.withCString { cStr in strcpy(ptr, cStr) }
        }

        let bindResult = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockAddr in
                bind(serverFd, sockAddr, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }

        guard bindResult == 0 else {
            print("[DesktopBridge] Failed to bind: \(errno)")
            Darwin.close(serverFd)
            serverFd = -1
            return
        }

        guard listen(serverFd, 5) == 0 else {
            print("[DesktopBridge] Failed to listen: \(errno)")
            Darwin.close(serverFd)
            serverFd = -1
            return
        }

        isRunning = true
        print("[DesktopBridge] Listening on \(socketPath)")

        // Accept loop on background thread
        let capturedFd = serverFd
        Thread.detachNewThread {
            self.acceptLoop(serverFd: capturedFd)
        }
    }

    func stop() {
        isRunning = false
        if serverFd >= 0 {
            Darwin.close(serverFd)
            serverFd = -1
        }
        try? FileManager.default.removeItem(atPath: socketPath)
        print("[DesktopBridge] Stopped")
    }

    // MARK: - Accept Loop

    private func acceptLoop(serverFd: Int32) {
        while isRunning {
            let clientFd = Darwin.accept(serverFd, nil, nil)
            guard clientFd >= 0 else { break }
            Thread.detachNewThread {
                self.handleConnection(fd: clientFd)
            }
        }
    }

    // MARK: - Connection Handling

    private func handleConnection(fd: Int32) {
        defer { Darwin.close(fd) }

        guard let line = readLine(fd: fd), !line.isEmpty else { return }

        let response = processRequest(jsonLine: line)
        var responseWithNewline = response + "\n"
        responseWithNewline.withUTF8 { ptr in
            _ = Darwin.write(fd, ptr.baseAddress, ptr.count)
        }
    }

    private func readLine(fd: Int32) -> String? {
        var bytes = [UInt8]()
        var buf = [UInt8](repeating: 0, count: 1)
        while true {
            let n = Darwin.read(fd, &buf, 1)
            if n <= 0 { break }
            if buf[0] == 0x0A { break } // newline
            bytes.append(buf[0])
        }
        return bytes.isEmpty ? nil : String(bytes: bytes, encoding: .utf8)
    }

    // MARK: - Request Processing

    private func processRequest(jsonLine: String) -> String {
        guard let data = jsonLine.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let id = json["id"] as? String,
              let method = json["method"] as? String
        else {
            return errorResponse(id: "null", code: -32700, message: "Parse error")
        }

        let params = json["params"] as? [String: Any] ?? [:]

        let result = dispatch(method: method, params: params)

        switch result {
        case .success(let value):
            return successResponse(id: id, value: value)
        case .failure(let err):
            return errorResponse(id: id, code: -32000, message: err.localizedDescription)
        }
    }

    private func dispatch(method: String, params: [String: Any]) -> Result<Any, Error> {
        switch method {
        case "desktop.ping":
            return .success("pong")

        // Perception — stubs for now (Phase 2)
        case "desktop.screenshot":
            return .success(["stub": true, "message": "screenshot not yet implemented"])
        case "desktop.ocr":
            return .success(["stub": true, "message": "ocr not yet implemented"])
        case "desktop.ax_tree":
            return .success(["stub": true, "message": "ax_tree not yet implemented"])

        // Action — stubs for now (Phase 3)
        case "desktop.click":
            return .success(["stub": true, "message": "click not yet implemented"])
        case "desktop.type_text":
            return .success(["stub": true, "message": "type_text not yet implemented"])
        case "desktop.key_combo":
            return .success(["stub": true, "message": "key_combo not yet implemented"])
        case "desktop.launch_app":
            return .success(["stub": true, "message": "launch_app not yet implemented"])
        case "desktop.window_list":
            return .success(["stub": true, "windows": [] as [Any]])
        case "desktop.focus_window":
            return .success(["stub": true, "message": "focus_window not yet implemented"])

        // Canvas — stubs for now (Phase 4)
        case "desktop.canvas_show":
            return .success(["stub": true, "message": "canvas_show not yet implemented"])
        case "desktop.canvas_hide":
            return .success(["stub": true, "message": "canvas_hide not yet implemented"])
        case "desktop.canvas_update":
            return .success(["stub": true, "message": "canvas_update not yet implemented"])

        default:
            let err = NSError(
                domain: "DesktopBridge",
                code: -32601,
                userInfo: [NSLocalizedDescriptionKey: "Method not found: \(method)"]
            )
            return .failure(err)
        }
    }

    // MARK: - JSON-RPC Helpers

    private func successResponse(id: String, value: Any) -> String {
        let envelope: [String: Any] = [
            "jsonrpc": "2.0",
            "id": id,
            "result": value,
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: envelope),
              let str = String(data: data, encoding: .utf8)
        else {
            return errorResponse(id: id, code: -32603, message: "Internal error: failed to encode response")
        }
        return str
    }

    private func errorResponse(id: String, code: Int, message: String) -> String {
        let envelope: [String: Any] = [
            "jsonrpc": "2.0",
            "id": id,
            "error": ["code": code, "message": message],
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: envelope),
              let str = String(data: data, encoding: .utf8)
        else {
            return "{\"jsonrpc\":\"2.0\",\"id\":\"\(id)\",\"error\":{\"code\":-32603,\"message\":\"encode failed\"}}"
        }
        return str
    }
}
