import Foundation
import os

/// Manages the lifecycle of the embedded aleph-server process.
@MainActor
final class ServerManager: ObservableObject {

    enum Error: Swift.Error {
        case binaryNotFound
        case alreadyRunning
        case startFailed(String)
        case socketTimeout
    }

    enum State: Equatable {
        case stopped
        case starting
        case running
        case stopping
        case crashed(String)
    }

    @Published private(set) var state: State = .stopped
    let socketPath: URL
    private var process: Process?
    private let logger = Logger(subsystem: "com.aleph.app", category: "ServerManager")

    var isRunning: Bool { state == .running }

    init(socketPath: URL? = nil) {
        self.socketPath = socketPath ?? ServerPaths.bridgeSocket
    }

    /// Start aleph-server, or reuse existing instance.
    func start() async throws {
        guard state != .running else { throw Error.alreadyRunning }

        // Check for existing server
        if checkExistingServer() {
            logger.info("Reusing existing aleph-server")
            state = .running
            return
        }

        guard let binaryPath = ServerPaths.serverBinary else {
            throw Error.binaryNotFound
        }

        state = .starting
        try ServerPaths.ensureDirectories()
        try? FileManager.default.removeItem(at: socketPath) // clean stale socket

        let proc = Process()
        proc.executableURL = binaryPath
        proc.arguments = []

        let stdout = Pipe()
        let stderr = Pipe()
        proc.standardOutput = stdout
        proc.standardError = stderr

        proc.terminationHandler = { [weak self] proc in
            Task { @MainActor in
                guard let self else { return }
                if self.state == .stopping {
                    self.state = .stopped
                } else {
                    self.state = .crashed("Exit code: \(proc.terminationStatus)")
                }
            }
        }

        do {
            try proc.run()
            self.process = proc
        } catch {
            state = .stopped
            throw Error.startFailed(error.localizedDescription)
        }

        try await waitForHTTP(port: 18790, timeout: 15.0)
        state = .running
    }

    /// Synchronous termination: sends SIGTERM to the server process.
    ///
    /// Use this during `applicationWillTerminate` when there is no time
    /// to wait for the async graceful shutdown. The OS will clean up the
    /// child process after the parent exits.
    func terminateNow() {
        guard let proc = process, proc.isRunning else { return }
        proc.terminate()
    }

    /// Graceful stop: SIGTERM -> 5s wait -> SIGKILL.
    func stop() async {
        guard let proc = process, proc.isRunning else {
            state = .stopped
            return
        }
        state = .stopping
        proc.terminate()

        let deadline = Date().addingTimeInterval(5.0)
        while proc.isRunning && Date() < deadline {
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        if proc.isRunning {
            kill(proc.processIdentifier, SIGKILL)
        }
        proc.waitUntilExit()
        self.process = nil
        state = .stopped
        try? FileManager.default.removeItem(at: socketPath)
    }

    // MARK: - Private

    private func checkExistingServer() -> Bool {
        guard FileManager.default.fileExists(atPath: socketPath.path) else { return false }
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { return false }
        defer { close(fd) }

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        let pathBytes = socketPath.path.utf8CString
        withUnsafeMutablePointer(to: &addr.sun_path) { ptr in
            pathBytes.withUnsafeBufferPointer { buf in
                UnsafeMutableRawPointer(ptr).copyMemory(
                    from: buf.baseAddress!,
                    byteCount: min(buf.count, 104)
                )
            }
        }
        return withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sockPtr in
                connect(fd, sockPtr, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        } == 0
    }

    private func waitForSocket(timeout: TimeInterval) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if checkExistingServer() { return }
            try await Task.sleep(nanoseconds: 200_000_000)
        }
        throw Error.socketTimeout
    }

    private func waitForHTTP(port: Int, timeout: TimeInterval) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if checkHTTPPort(port: port) { return }
            try await Task.sleep(nanoseconds: 500_000_000)
        }
        throw Error.socketTimeout
    }

    private func checkHTTPPort(port: Int) -> Bool {
        guard let url = URL(string: "http://127.0.0.1:\(port)/") else { return false }
        let sem = DispatchSemaphore(value: 0)
        var ok = false
        let task = URLSession.shared.dataTask(with: url) { _, response, _ in
            if let http = response as? HTTPURLResponse, http.statusCode == 200 {
                ok = true
            }
            sem.signal()
        }
        task.resume()
        _ = sem.wait(timeout: .now() + 2)
        task.cancel()
        return ok
    }
}
