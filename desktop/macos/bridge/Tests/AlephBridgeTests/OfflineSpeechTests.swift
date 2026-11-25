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
        let task = FakeTask()
        private let lock = NSLock()
        private var startCount = 0

        init(onDevice: Bool = true, available: Bool = true, reply: Result<String, Error>?) {
            supportsOnDeviceRecognition = onDevice
            isAvailable = available
            self.reply = reply
        }

        var starts: Int { lock.locked { startCount } }

        func start(
            url: URL,
            completion: @escaping @Sendable (Result<String, Error>) -> Void
        ) -> OfflineRecognitionTask {
            lock.locked { startCount += 1 }
            if let reply = reply {
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
        private let lock = NSLock()
        private var locales: [String] = []

        init(_ status: SFSpeechRecognizerAuthorizationStatus = .authorized, _ installed: FakeRecognizer?) {
            self.status = status
            self.installed = installed
        }

        var requestedLocales: [String] { lock.locked { locales } }

        func authorize() async -> SFSpeechRecognizerAuthorizationStatus { status }

        func recognizer(locale: Locale) -> OfflineRecognizer? {
            lock.locked { locales.append(locale.identifier) }
            return installed
        }
    }

    // MARK: - Helpers

    private var clips: [URL] = []

    override func tearDown() {
        for clip in clips { try? FileManager.default.removeItem(at: clip) }
        super.tearDown()
    }

    private func makeClip() throws -> String {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("offline-speech-\(UUID().uuidString).wav")
        try Data("RIFF".utf8).write(to: url)
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
        XCTAssertTrue(SystemOfflineRecognizer.makeRequest(url: URL(fileURLWithPath: path))
            .requiresOnDeviceRecognition)

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
        XCTAssertEqual(engine.requestedLocales, [Locale.current.identifier])
        let active = await session.activeOperations
        XCTAssertEqual(active, 0)
    }

    func testCancelledOfflineRecognitionReleasesScratch() async throws {
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
        // The recognizer was stopped before the reply, and nothing here holds
        // the clip any more; the clip itself is the caller's to delete.
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

        // The bounded deadline also stops the recognizer.
        let slow = FakeRecognizer(reply: nil)
        let bounded = OfflineSpeechSession(engine: FakeEngine(.authorized, slow), deadlineNanoseconds: 10_000_000)
        let timedOut = await errorCode(bounded, path, "op-slow", "en-US")
        XCTAssertEqual(timedOut, -32004)
        try await waitUntil { slow.task.cancelled }
        let boundedActive = await bounded.activeOperations
        XCTAssertEqual(boundedActive, 0)
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

        XCTAssertEqual(
            try transcribeRequest(from: .object([
                "audio_path": .string(path),
                "offline_only": .object(["operation_id": .string("op")]),
            ])),
            .offline(path: path, operationId: "op", language: nil)
        )
        XCTAssertEqual(
            try transcribeRequest(from: .object([
                "audio_path": .string(path),
                "offline_only": .object(["operation_id": .string("op"), "language": .string("zh-CN")]),
            ])),
            .offline(path: path, operationId: "op", language: "zh-CN")
        )
        // Both fields at once is ambiguous about which recognizer is allowed.
        XCTAssertThrowsError(try transcribeRequest(from: .object([
            "audio_path": .string(path),
            "language": .string("en-US"),
            "offline_only": .object(["operation_id": .string("op")]),
        ]))) { error in
            XCTAssertEqual((error as? RpcError)?.code, -32602)
        }
        // Offline clips obey the same media root as legacy ones.
        XCTAssertThrowsError(try transcribeRequest(from: .object([
            "audio_path": .string("/tmp/turn.wav"),
            "offline_only": .object(["operation_id": .string("op")]),
        ])))
    }
}

private extension NSLock {
    func locked<T>(_ body: () throws -> T) rethrows -> T {
        lock()
        defer { unlock() }
        return try body()
    }
}
