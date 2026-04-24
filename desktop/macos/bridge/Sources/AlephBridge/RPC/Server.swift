import Foundation

/// JSON-RPC 2.0 server over stdin/stdout with newline-delimited messages.
///
/// Reads a line at a time from stdin on a background dispatch source, parses
/// it as a `Request`, dispatches to the `Router` from inside a detached task
/// (to keep concurrent requests from head-of-line blocking each other), and
/// writes the encoded Response / ErrorResponse back to stdout.
actor Server {
    let router: Router
    private let stderr = FileHandle.standardError

    init(router: Router) {
        self.router = router
    }

    func run() async {
        let reader = LineReader(fileDescriptor: FileHandle.standardInput.fileDescriptor)
        // Use a TaskGroup so all in-flight request handlers finish before run()
        // returns — guarantees responses are flushed before the process exits.
        await withTaskGroup(of: Void.self) { group in
            while let line = reader.nextLine() {
                let snapshot = line
                group.addTask { [weak self] in
                    await self?.handleLine(snapshot)
                }
            }
            // Drain remaining tasks before falling through.
            await group.waitForAll()
        }
        stderr.write("aleph-bridge: stdin closed, exiting\n".data(using: .utf8)!)
    }

    private func handleLine(_ line: Data) async {
        do {
            let req = try Codec.decode(line, as: Request.self)
            do {
                let result = try await router.handle(method: req.method, params: req.params)
                let resp = Response(id: req.id, result: result)
                await write(resp)
            } catch let err as RpcError {
                let resp = ErrorResponse(id: req.id, error: err)
                await write(resp)
            } catch {
                let resp = ErrorResponse(
                    id: req.id,
                    error: RpcError(
                        code: -32003, // ERR_PLATFORM
                        message: "\(error)",
                        data: nil
                    )
                )
                await write(resp)
            }
        } catch {
            stderr.write("aleph-bridge: parse error: \(error)\n".data(using: .utf8)!)
            // Emit a parse-error with no id so the client at least sees something.
            let resp = ErrorResponse(
                id: nil,
                error: RpcError(code: -32700, message: "\(error)", data: nil)
            )
            await write(resp)
        }
    }

    private func write<T: Encodable>(_ msg: T) async {
        guard let data = try? Codec.encode(msg) else { return }
        let stdout = FileHandle.standardOutput
        // FileHandle writes are synchronous; wrap in a do/catch to survive a
        // broken pipe if the Rust client has exited.
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
