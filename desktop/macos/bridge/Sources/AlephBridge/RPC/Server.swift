import Foundation

/// JSON-RPC 2.0 server over stdin/stdout with newline-delimited messages.
///
/// Reads a line at a time from stdin, parses it as a `Request`, dispatches to
/// the `Router` from inside a detached child task (so concurrent requests do
/// not head-of-line block each other), and writes the encoded Response /
/// ErrorResponse back to stdout.
///
/// `Server` is a class (not an actor) on purpose: the reader loop calls the
/// synchronous blocking `read(2)` syscall and would otherwise hold actor
/// isolation across that block, starving handler child tasks. State accessed
/// from handlers (`router`) is itself an actor, and stdout writes are
/// serialized by `stdoutLock`.
final class Server {
    let router: Router
    private let stderr = FileHandle.standardError
    private let stdoutLock = NSLock()

    init(router: Router) {
        self.router = router
    }

    func run() async {
        let reader = LineReader(fileDescriptor: FileHandle.standardInput.fileDescriptor)
        // TaskGroup so all in-flight handlers finish before run() returns —
        // guarantees responses flush before the process exits on stdin close.
        await withTaskGroup(of: Void.self) { group in
            while let line = reader.nextLine() {
                let snapshot = line
                group.addTask { [weak self] in
                    await self?.handleLine(snapshot)
                }
            }
            await group.waitForAll()
        }
        stderr.write("aleph-bridge: stdin closed, exiting\n".data(using: .utf8)!)
    }

    private func handleLine(_ line: Data) async {
        do {
            let req = try Codec.decode(line, as: Request.self)
            do {
                let result = try await router.handle(method: req.method, params: req.params)
                write(Response(id: req.id, result: result))
            } catch let err as RpcError {
                write(ErrorResponse(id: req.id, error: err))
            } catch {
                write(ErrorResponse(
                    id: req.id,
                    error: RpcError(code: -32003, message: "\(error)", data: nil)
                ))
            }
        } catch {
            stderr.write("aleph-bridge: parse error: \(error)\n".data(using: .utf8)!)
            write(ErrorResponse(
                id: nil,
                error: RpcError(code: -32700, message: "\(error)", data: nil)
            ))
        }
    }

    private func write<T: Encodable>(_ msg: T) {
        guard let data = try? Codec.encode(msg) else { return }
        let stdout = FileHandle.standardOutput
        stdoutLock.lock()
        defer { stdoutLock.unlock() }
        do {
            try stdout.write(contentsOf: data)
        } catch {
            stderr.write("aleph-bridge: write failed: \(error)\n".data(using: .utf8)!)
        }
    }
}

/// Minimal POSIX line reader — reads bytes until `\n` from the given fd.
/// Uses `read(2)` directly so it composes well with the Rust client's
/// `tokio::process::Command` piping.
final class LineReader {
    private let fd: Int32
    private var buffer = Data()

    /// Hard cap on the partial-line buffer.
    ///
    /// The previous shape grew `buffer` without bound until `\n` arrived, so a
    /// hostile caller that wrote a JSON-RPC line with no terminator could pin
    /// the helper at hundreds of MB of `Data` — and a single hostile call is
    /// enough: a long-lived helper has no other reason to drop the connection.
    /// 64 MB is well past every method the helper serves (the largest
    /// `params` shape is a base64-encoded screenshot, capped at the desktop
    /// layer's 1.5 MB screenshot budget) and well under the threshold where
    /// `Data` itself becomes a memory-pressure risk.
    private static let maxBufferBytes = 64 * 1024 * 1024

    init(fileDescriptor fd: Int32) {
        self.fd = fd
    }

    func nextLine() -> Data? {
        // Drain complete lines already buffered.
        if let line = takeLineFromBuffer() {
            return line
        }
        var chunk = [UInt8](repeating: 0, count: 4096)
        while true {
            let n = read(fd, &chunk, chunk.count)
            if n < 0 {
                // Retry on EINTR; otherwise give up.
                if errno == EINTR { continue }
                return buffer.isEmpty ? nil : takeRemainderAsLine()
            }
            if n == 0 {
                return buffer.isEmpty ? nil : takeRemainderAsLine()
            }
            buffer.append(contentsOf: chunk.prefix(n))
            // The buffer overflowed the cap and still no terminator: the peer
            // is hostile (or buggy) and is feeding bytes we will never parse.
            // Discard what we have so far — the caller will see `EOF` on the
            // next iteration and tear the session down — rather than let the
            // helper's RSS climb forever. A legitimate caller never gets close:
            // the largest `params` shape is a base64 screenshot, and the desktop
            // layer caps that at 1.5 MB.
            if buffer.count > Self.maxBufferBytes {
                buffer.removeAll(keepingCapacity: false)
            }
            if let line = takeLineFromBuffer() {
                return line
            }
        }
    }

    private func takeLineFromBuffer() -> Data? {
        guard let idx = buffer.firstIndex(of: 0x0A) else { return nil }
        let line = buffer[buffer.startIndex..<idx]
        buffer.removeSubrange(buffer.startIndex...idx)
        return Data(line)
    }

    private func takeRemainderAsLine() -> Data {
        let line = buffer
        buffer.removeAll(keepingCapacity: false)
        return line
    }
}
