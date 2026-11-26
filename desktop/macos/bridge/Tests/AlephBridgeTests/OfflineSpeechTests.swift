import AVFoundation
import Darwin
import Dispatch
import Foundation
import Speech
import XCTest
@testable import AlephBridge

/// `media.speech.transcribe_file` with `offline_only`: the installed on-device
/// recognizer or a classified failure, never a network recognizer.
final class OfflineSpeechTests: XCTestCase {

    // MARK: - Fakes

    private final class FakeTask: OfflineRecognitionTask, @unchecked Sendable {
        private let lock = NSLock()
        private var cancelledFlag = false
        private var onCancel: (() -> Void)?

        var cancelled: Bool { lock.locked { cancelledFlag } }

        func whenCancelled(_ body: @escaping () -> Void) {
            lock.locked { onCancel = body }
        }

        func cancel() {
            let body: (() -> Void)? = lock.locked {
                cancelledFlag = true
                return onCancel
            }
            body?()
        }
    }

    private final class FakeRecognizer: OfflineRecognizer, @unchecked Sendable {
        let supportsOnDeviceRecognition: Bool
        let isAvailable: Bool
        /// nil holds the recognition until it is cancelled.
        private let reply: Result<String, Error>?
        private let beforeReply: (@Sendable () -> Void)?
        let task = FakeTask()
        private let lock = NSLock()
        private var startCount = 0
        private var lastSamples: [Int16]?

        init(
            onDevice: Bool = true,
            available: Bool = true,
            reply: Result<String, Error>?,
            beforeReply: (@Sendable () -> Void)? = nil
        ) {
            supportsOnDeviceRecognition = onDevice
            isAvailable = available
            self.reply = reply
            self.beforeReply = beforeReply
        }

        var starts: Int { lock.locked { startCount } }
        var samples: [Int16]? { lock.locked { lastSamples } }

        func start(
            buffer: AVAudioPCMBuffer,
            completion: @escaping @Sendable (Result<String, Error>) -> Void
        ) -> OfflineRecognitionTask {
            lock.locked {
                startCount += 1
                lastSamples = buffer.int16ChannelData.map {
                    Array(UnsafeBufferPointer(start: $0[0], count: Int(buffer.frameLength)))
                }
            }
            if let reply = reply {
                beforeReply?()
                completion(reply)
            } else {
                task.whenCancelled { completion(.failure(CancellationError())) }
            }
            return task
        }
    }

    private final class FakeEngine: OfflineSpeechEngine, @unchecked Sendable {
        private let status: SFSpeechRecognizerAuthorizationStatus
        private let installed: FakeRecognizer?
        private let authorizationRequested: XCTestExpectation?
        private let lock = NSLock()
        private var pendingAuthorization: (@Sendable (SFSpeechRecognizerAuthorizationStatus) -> Void)?
        private var locales: [String] = []

        init(
            _ status: SFSpeechRecognizerAuthorizationStatus = .authorized,
            _ installed: FakeRecognizer?,
            authorizationRequested: XCTestExpectation? = nil
        ) {
            self.status = status
            self.installed = installed
            self.authorizationRequested = authorizationRequested
        }

        var requestedLocales: [String] { lock.locked { locales } }

        func authorize(completion: @escaping @Sendable (SFSpeechRecognizerAuthorizationStatus) -> Void) {
            if let authorizationRequested {
                lock.locked { pendingAuthorization = completion }
                authorizationRequested.fulfill()
            } else {
                completion(status)
            }
        }

        func deliverAuthorization() -> Bool {
            let completion = lock.locked {
                defer { pendingAuthorization = nil }
                return pendingAuthorization
            }
            guard let completion else { return false }
            completion(status)
            return true
        }

        func recognizer(locale: Locale) -> OfflineRecognizer? {
            lock.locked { locales.append(locale.identifier) }
            return installed
        }
    }

    private final class HeldPreparation: @unchecked Sendable {
        let started: XCTestExpectation
        private let release = DispatchSemaphore(value: 0)
        private let lock = NSLock()
        private var completions = 0

        init(started: XCTestExpectation) { self.started = started }

        func load(_ path: String) throws -> OfflineSpeechAudio {
            started.fulfill()
            guard release.wait(timeout: .now() + 5) == .success else {
                throw CancellationError()
            }
            return try OfflineSpeechAudio.load(fromTrustedPath: path)
        }

        func resume() { release.signal() }
        func complete() { lock.locked { completions += 1 } }
        var completed: Int { lock.locked { completions } }
    }

    // MARK: - Helpers

    private var clips: [URL] = []

    override func tearDown() {
        for clip in clips { try? FileManager.default.removeItem(at: clip) }
        super.tearDown()
    }

    private static let clipSamples: [Int16] = [-1234, 0x1234]

    private static func canonicalWAV() -> Data {
        var wav = Data([
            0x52, 0x49, 0x46, 0x46, 40, 0, 0, 0, // RIFF, 36 + 4 bytes
            0x57, 0x41, 0x56, 0x45, 0x66, 0x6d, 0x74, 0x20, // WAVE, fmt
            16, 0, 0, 0, 1, 0, 1, 0, // PCM, mono
            0x80, 0x3e, 0, 0, 0, 0x7d, 0, 0, // 16 kHz, 32 kB/s
            2, 0, 16, 0, 0x64, 0x61, 0x74, 0x61, 4, 0, 0, 0, // PCM16, data
        ])
        for sample in clipSamples {
            let bits = UInt16(bitPattern: sample)
            wav.append(UInt8(truncatingIfNeeded: bits))
            wav.append(UInt8(bits >> 8))
        }
        return wav
    }

    private func makeClip(_ wav: Data = OfflineSpeechTests.canonicalWAV()) throws -> String {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("offline-speech-\(UUID().uuidString).wav")
        try wav.write(to: url)
        clips.append(url)
        return url.path
    }

    private func errorCode(
        _ session: OfflineSpeechSession,
        _ path: String,
        _ operationId: String,
        _ language: String?
    ) async -> Int32? {
        do {
            _ = try await session.transcribe(audioPath: path, operationId: operationId, language: language)
            return nil
        } catch let error as RpcError {
            return error.code
        } catch {
            return -1
        }
    }

    private func waitUntil(_ condition: @escaping () async -> Bool) async throws {
        for _ in 0..<2_000 {
            if await condition() { return }
            try await Task.sleep(nanoseconds: 1_000_000)
        }
        XCTFail("condition not reached")
    }

    // MARK: - Tests

    func testOfflineRequiresLocalRecognition() async throws {
        let path = try makeClip()
        let request = SystemOfflineRecognizer.makeRequest()
        XCTAssertTrue(request.requiresOnDeviceRecognition)
        XCTAssertFalse(request.shouldReportPartialResults)

        // No recognizer for the language.
        let none = FakeEngine(.authorized, nil)
        let code = await errorCode(OfflineSpeechSession(engine: none), path, "op-lang", "xx-XX")
        XCTAssertEqual(code, -32007)
        XCTAssertEqual(none.requestedLocales, ["xx-XX"])
        let blank = await errorCode(OfflineSpeechSession(engine: none), path, "op-blank", "")
        XCTAssertEqual(blank, -32007)

        // A recognizer that would need the network, or has no model right now.
        for recognizer in [
            FakeRecognizer(onDevice: false, reply: .success("network")),
            FakeRecognizer(available: false, reply: .success("network")),
        ] {
            let code = await errorCode(
                OfflineSpeechSession(engine: FakeEngine(.authorized, recognizer)), path, "op-offline", "en-US")
            XCTAssertEqual(code, -32002)
            XCTAssertEqual(recognizer.starts, 0)
        }

        // Denied permission is classified before any recognizer is looked up.
        for status: SFSpeechRecognizerAuthorizationStatus in [.denied, .restricted, .notDetermined] {
            let recognizer = FakeRecognizer(reply: .success("never"))
            let engine = FakeEngine(status, recognizer)
            let code = await errorCode(OfflineSpeechSession(engine: engine), path, "op-perm", "en-US")
            XCTAssertEqual(code, -32001)
            XCTAssertEqual(engine.requestedLocales, [])
            XCTAssertEqual(recognizer.starts, 0)
        }

        // An engine failure is classified, not retried.
        let failing = FakeRecognizer(reply: .failure(CancellationError()))
        let failed = await errorCode(
            OfflineSpeechSession(engine: FakeEngine(.authorized, failing)), path, "op-fail", "en-US")
        XCTAssertEqual(failed, -32003)
        XCTAssertEqual(failing.starts, 1)

        // Absent language resolves the OS locale only.
        let local = FakeRecognizer(reply: .success("hello"))
        let engine = FakeEngine(.authorized, local)
        let session = OfflineSpeechSession(engine: engine)
        let result = try await session.transcribe(audioPath: path, operationId: "op-ok", language: nil)
        XCTAssertEqual(result.text, "hello")
        XCTAssertEqual(result.language, Locale.current.identifier)
        XCTAssertEqual(local.samples, Self.clipSamples)
        guard case .object(let fields) = result.rpcResult,
              case .string(let operationId) = fields["operation_id"] ?? .null,
              case .string(let release) = fields["input_release"] ?? .null,
              case .string(let text) = fields["text"] ?? .null,
              case .string(let language) = fields["language"] ?? .null else {
            return XCTFail("missing detached offline reply fields")
        }
        XCTAssertEqual(fields.count, 4)
        XCTAssertEqual(operationId, "op-ok")
        XCTAssertEqual(release, "detached")
        XCTAssertEqual(text, result.text)
        XCTAssertEqual(language, result.language)
        XCTAssertEqual(engine.requestedLocales, [Locale.current.identifier])
        let active = await session.activeOperations
        XCTAssertEqual(active, 0)
    }

    func testOfflineCancellationWithdrawsOperationWithoutAuthorizationCallback() async throws {
        let path = try makeClip()
        let recognizer = FakeRecognizer(reply: .success("must not start"))
        let requested = expectation(description: "authorization callback installed")
        let completed = expectation(description: "transcription classified without authorization callback")
        let engine = FakeEngine(.authorized, recognizer, authorizationRequested: requested)
        let session = OfflineSpeechSession(engine: engine)
        let operationId = "op-auth-cancel"
        Task {
            let code = await errorCode(session, path, operationId, "en-US")
            XCTAssertEqual(code, OfflineSpeechSession.cancelledError.code)
            completed.fulfill()
        }

        await fulfillment(of: [requested], timeout: 2)
        let reserved = await session.activeOperations
        XCTAssertEqual(reserved, 1)
        let cancelled = await session.cancel(operationId: operationId)
        XCTAssertTrue(cancelled)
        let active = await session.activeOperations
        XCTAssertEqual(active, 0)
        await fulfillment(of: [completed], timeout: 2)
        XCTAssertEqual(engine.requestedLocales, [])
        XCTAssertEqual(recognizer.starts, 0)
    }

    func testOfflineDeadlineWithdrawsOperationBeforeLateAuthorizationCallback() async throws {
        let path = try makeClip()
        let recognizer = FakeRecognizer(reply: .success("must not start"))
        let requested = expectation(description: "authorization callback installed")
        let completed = expectation(description: "transcription classified before authorization callback")
        let engine = FakeEngine(.authorized, recognizer, authorizationRequested: requested)
        let session = OfflineSpeechSession(engine: engine, deadlineNanoseconds: 10_000_000)
        Task {
            let code = await errorCode(session, path, "op-auth-deadline", "en-US")
            XCTAssertEqual(code, -32004)
            completed.fulfill()
        }

        await fulfillment(of: [requested, completed], timeout: 2, enforceOrder: true)
        let beforeCallback = await session.activeOperations
        XCTAssertEqual(beforeCallback, 0)
        XCTAssertEqual(recognizer.starts, 0)
        XCTAssertTrue(engine.deliverAuthorization())
        let afterCallback = await session.activeOperations
        XCTAssertEqual(afterCallback, 0)
        XCTAssertEqual(engine.requestedLocales, [])
        XCTAssertEqual(recognizer.starts, 0)
    }

    func testOfflineDeadlineRejectsSynchronousRecognitionReplyAfterExpiry() async throws {
        let path = try makeClip()
        let deadlineNanoseconds: UInt64 = 500_000_000
        let recognizer = FakeRecognizer(reply: .success("late result"), beforeReply: {
            // Hold start's actor turn past the operation's earlier deadline.
            let clock = ContinuousClock()
            let replyAt = clock.now.advanced(by: .nanoseconds(deadlineNanoseconds))
            repeat {
                Thread.sleep(forTimeInterval: Double(deadlineNanoseconds) / 1_000_000_000)
            } while clock.now < replyAt
        })
        let session = OfflineSpeechSession(engine: FakeEngine(.authorized, recognizer), deadlineNanoseconds: deadlineNanoseconds)

        let code = await errorCode(session, path, "op-start-deadline", "en-US")

        XCTAssertEqual(code, -32004)
        XCTAssertEqual(recognizer.starts, 1)
        XCTAssertTrue(recognizer.task.cancelled)
        let active = await session.activeOperations
        XCTAssertEqual(active, 0)
    }

    func testOfflineRecognitionRequestsTaskCancellationAndClassifiesReply() async throws {
        let path = try makeClip()
        let held = FakeRecognizer(reply: nil)
        let session = OfflineSpeechSession(engine: FakeEngine(.authorized, held))
        let pending = Task {
            try await session.transcribe(audioPath: path, operationId: "op-held", language: "en-US")
        }
        try await waitUntil { held.starts == 1 }

        let cancelled = await session.cancel(operationId: "op-held")
        XCTAssertTrue(cancelled)
        do {
            _ = try await pending.value
            XCTFail("cancelled recognition returned text")
        } catch let error as RpcError {
            XCTAssertEqual(error.code, OfflineSpeechSession.cancelledError.code)
            XCTAssertEqual(error.message, OfflineSpeechSession.cancelledError.message)
        }
        // The fake observes the cancellation request; only the caller owns the clip.
        XCTAssertTrue(held.task.cancelled)
        let active = await session.activeOperations
        XCTAssertEqual(active, 0)
        XCTAssertTrue(FileManager.default.fileExists(atPath: path))
        let again = await session.cancel(operationId: "op-held")
        XCTAssertFalse(again)

        // A cancel that arrives first refuses the late start.
        let early = await session.cancel(operationId: "op-late")
        XCTAssertFalse(early)
        let late = await errorCode(session, path, "op-late", "en-US")
        XCTAssertEqual(late, -32003)
        XCTAssertEqual(held.starts, 1)

        // An installed recognition also receives a cancellation request at the deadline.
        let slow = FakeRecognizer(reply: nil)
        let bounded = OfflineSpeechSession(engine: FakeEngine(.authorized, slow), deadlineNanoseconds: 500_000_000)
        let timedOut = await errorCode(bounded, path, "op-slow", "en-US")
        XCTAssertEqual(timedOut, -32004)
        XCTAssertEqual(slow.starts, 1)
        XCTAssertTrue(slow.task.cancelled)
        let boundedActive = await bounded.activeOperations
        XCTAssertEqual(boundedActive, 0)
    }

    func testOfflinePreparationFailureDoesNotStartRecognition() async throws {
        let path = try makeClip(Data(Self.canonicalWAV().dropLast()))
        let recognizer = FakeRecognizer(reply: .success("must not start"))
        let session = OfflineSpeechSession(engine: FakeEngine(.authorized, recognizer))

        do {
            _ = try await session.transcribe(audioPath: path, operationId: "op-bad-wav", language: "en-US")
            XCTFail("a truncated clip returned text")
        } catch let error as RpcError {
            XCTAssertEqual(error.code, -32003)
            XCTAssertEqual(error.message, "media.speech.transcribe_file: offline audio preparation failed")
        }
        XCTAssertEqual(recognizer.starts, 0)
        let active = await session.activeOperations
        XCTAssertEqual(active, 0)
    }

    func testOfflineDeadlineJoinsPreparationWithoutStartingRecognition() async throws {
        let path = try makeClip()
        let held = HeldPreparation(started: expectation(description: "preparation started"))
        defer { held.resume() }
        let recognizer = FakeRecognizer(reply: .success("must not start"))
        let session = OfflineSpeechSession(
            engine: FakeEngine(.authorized, recognizer),
            deadlineNanoseconds: 500_000_000,
            loadAudio: { try held.load($0) }
        )
        let pending = Task {
            defer { held.complete() }
            return try await session.transcribe(audioPath: path, operationId: "op-prep-deadline", language: "en-US")
        }
        await fulfillment(of: [held.started], timeout: 2)
        try await Task.sleep(nanoseconds: 600_000_000)
        XCTAssertEqual(held.completed, 0)
        let heldActive = await session.activeOperations
        XCTAssertEqual(heldActive, 1)
        XCTAssertEqual(recognizer.starts, 0)

        held.resume()
        do {
            _ = try await pending.value
            XCTFail("late preparation returned text")
        } catch let error as RpcError {
            XCTAssertEqual(error.code, -32004)
        }
        XCTAssertEqual(recognizer.starts, 0)
        let active = await session.activeOperations
        XCTAssertEqual(active, 0)
    }

    func testOfflineCancellationJoinsPreparationWithoutStartingRecognition() async throws {
        let path = try makeClip()
        let held = HeldPreparation(started: expectation(description: "preparation started"))
        defer { held.resume() }
        let recognizer = FakeRecognizer(reply: .success("must not start"))
        let session = OfflineSpeechSession(
            engine: FakeEngine(.authorized, recognizer), loadAudio: { try held.load($0) }
        )
        let operationId = "op-prep-cancel"
        let pending = Task {
            defer { held.complete() }
            return try await session.transcribe(audioPath: path, operationId: operationId, language: "en-US")
        }
        await fulfillment(of: [held.started], timeout: 2)

        let cancelIssued = expectation(description: "cancellation issued while preparation is held")
        let cancel = Task {
            defer { held.complete() }
            cancelIssued.fulfill()
            return await session.cancel(operationId: operationId)
        }
        await fulfillment(of: [cancelIssued], timeout: 2)
        try await Task.sleep(nanoseconds: 100_000_000)
        XCTAssertEqual(held.completed, 0)
        let heldActive = await session.activeOperations
        XCTAssertEqual(heldActive, 1)
        XCTAssertEqual(recognizer.starts, 0)

        held.resume()
        let cancelled = await cancel.value
        XCTAssertTrue(cancelled)
        do {
            _ = try await pending.value
            XCTFail("withdrawn preparation returned text")
        } catch let error as RpcError {
            XCTAssertEqual(error.code, OfflineSpeechSession.cancelledError.code)
            XCTAssertEqual(error.message, OfflineSpeechSession.cancelledError.message)
        }
        XCTAssertEqual(recognizer.starts, 0)
        let active = await session.activeOperations
        XCTAssertEqual(active, 0)
    }

    func testTranscribeRequestKeepsLegacyCallers() throws {
        let path = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".aleph/data/_media/turn.wav").path

        // A request from before the policy field decodes exactly as it did.
        XCTAssertEqual(
            try transcribeRequest(from: .object([
                "audio_path": .string(path),
                "language": .string("en-US"),
            ])),
            .legacy(path: path, language: "en-US")
        )
        XCTAssertThrowsError(try transcribeRequest(from: .object(["audio_path": .string(path)])))

        // Both fields at once are ambiguous about which recognizer is allowed.
        XCTAssertThrowsError(try transcribeRequest(from: .object([
            "audio_path": .string(path),
            "language": .string("en-US"),
            "offline_only": .object(["operation_id": .string("op")]),
        ]))) { error in
            XCTAssertEqual((error as? RpcError)?.code, -32602)
        }
    }

    func testOfflineRequestRequiresCanonicalClipInConfiguredRoot() throws {
        let fm = FileManager.default
        let fixture = fm.temporaryDirectory.appendingPathComponent("offline-root-\(UUID().uuidString)")
        let root = fixture.appendingPathComponent("staged")
        let sibling = fixture.appendingPathComponent("staged-other")
        try fm.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? fm.removeItem(at: fixture) }
        try fm.createDirectory(at: sibling, withIntermediateDirectories: false)

        let name = "voice-turn-123456789abc4def8123456789abcdef.wav"
        let url = root.appendingPathComponent(name)
        let outside = sibling.appendingPathComponent(name)
        let symlink = root.appendingPathComponent("voice-turn-123456789abc4def8123456789abcdee.wav")
        let wrongName = root.appendingPathComponent("turn.wav")
        for clip in [url, outside, wrongName] { try Self.canonicalWAV().write(to: clip) }
        try fm.createSymbolicLink(at: symlink, withDestinationURL: url)
        let canonicalRoot = try XCTUnwrap(realpath(root.path, nil))
        defer { free(canonicalRoot) }
        let configuredRoot = String(cString: canonicalRoot)
        let canonicalPath = configuredRoot + "/" + name

        func params(path: String, language: String? = nil) -> JSONValue {
            var policy: [String: JSONValue] = ["operation_id": .string("op")]
            if let language { policy["language"] = .string(language) }
            return .object(["audio_path": .string(path), "offline_only": .object(policy)])
        }

        for language in [nil, "zh-CN"] {
            XCTAssertEqual(
                try transcribeRequest(from: params(path: url.path, language: language), offlineStagingRoot: configuredRoot),
                .offline(path: canonicalPath, operationId: "op", language: language)
            )
        }
        XCTAssertThrowsError(try transcribeRequest(from: params(path: url.path))) { error in
            XCTAssertEqual((error as? RpcError)?.code, -32602)
        }
        for rejected in [outside.path, symlink.path, wrongName.path, root.path + "/./" + name] {
            XCTAssertThrowsError(try transcribeRequest(from: params(path: rejected), offlineStagingRoot: configuredRoot)) {
                error in XCTAssertEqual((error as? RpcError)?.code, -32602)
            }
        }

        try fm.moveItem(at: root, to: fixture.appendingPathComponent("staged-original"))
        try fm.createSymbolicLink(at: root, withDestinationURL: sibling)
        XCTAssertThrowsError(try transcribeRequest(from: params(path: outside.path), offlineStagingRoot: configuredRoot)) {
            error in XCTAssertEqual((error as? RpcError)?.code, -32602)
        }
    }
}

private extension NSLock {
    func locked<T>(_ body: () throws -> T) rethrows -> T {
        lock()
        defer { unlock() }
        return try body()
    }
}
