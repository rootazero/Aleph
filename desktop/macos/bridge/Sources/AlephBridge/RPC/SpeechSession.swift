import Foundation
import Speech

/// Serializes SFSpeechRecognizer access. The recognizer itself is
/// thread-confined in practice and TCC authorization is process-global, so we
/// gate transcribe() calls through an actor to match audio/camera sessions.
actor SpeechSession {
    func transcribe(audioPath: String, language: String) async throws -> String {
        // Verify file exists (early, clear error).
        guard FileManager.default.fileExists(atPath: audioPath) else {
            throw RpcError(
                code: -32602,
                message: "media.speech.transcribe_file: audio file not found at \(audioPath)",
                data: nil
            )
        }

        // Request TCC authorization if not yet granted.
        let authStatus = await requestAuthorization()
        guard authStatus == .authorized else {
            throw RpcError(
                code: -32004,
                message: "media.speech.transcribe_file: speech recognition permission not granted (\(authStatus.rawValue))",
                data: nil
            )
        }

        // Build recognizer for the requested locale.
        let locale = Locale(identifier: language)
        guard let recognizer = SFSpeechRecognizer(locale: locale) else {
            throw RpcError(
                code: -32602,
                message: "media.speech.transcribe_file: unsupported locale '\(language)'",
                data: nil
            )
        }
        guard recognizer.isAvailable else {
            throw RpcError(
                code: -32003,
                message: "media.speech.transcribe_file: recognizer unavailable for '\(language)'",
                data: nil
            )
        }

        // Build URL request.
        let url = URL(fileURLWithPath: audioPath)
        let request = SFSpeechURLRecognitionRequest(url: url)

        // Bridge callback → continuation. 60-second hard timeout (Apple's
        // on-device recognition cap).
        return try await withCheckedThrowingContinuation { (rawCont: CheckedContinuation<String, Error>) in
            // The recognition callback and the safety timeout below race for the
            // same continuation; `ResumeOnce` is the shared version of the
            // lock-and-flag this used to spell inline (see its doc comment for
            // what the third site that skipped it cost).
            let cont = ResumeOnce(rawCont)

            let task = recognizer.recognitionTask(with: request) { result, error in
                if let error = error {
                    cont.resume(throwing: RpcError(
                        code: -32003,
                        message: "media.speech.transcribe_file: \(error.localizedDescription)",
                        data: nil
                    ))
                    return
                }
                guard let result = result, result.isFinal else { return }
                cont.resume(returning: result.bestTranscription.formattedString)
            }

            // Safety timeout so the continuation never leaks if
            // SFSpeechRecognizer stalls.
            Task {
                try? await Task.sleep(nanoseconds: 60_000_000_000)
                task.cancel()
                cont.resume(throwing: RpcError(
                    code: -32003,
                    message: "media.speech.transcribe_file: timed out after 60s",
                    data: nil
                ))
            }
        }
    }

    private func requestAuthorization() async -> SFSpeechRecognizerAuthorizationStatus {
        await withCheckedContinuation { cont in
            SFSpeechRecognizer.requestAuthorization { status in
                cont.resume(returning: status)
            }
        }
    }
}

// MARK: - Offline-only recognition

/// The installed recognizer, behind a seam so the offline policy is testable
/// without TCC or a downloaded on-device model.
protocol OfflineSpeechEngine: Sendable {
    func authorize() async -> SFSpeechRecognizerAuthorizationStatus
    /// nil when the OS has no recognizer for `locale`.
    func recognizer(locale: Locale) -> OfflineRecognizer?
}

protocol OfflineRecognizer {
    var supportsOnDeviceRecognition: Bool { get }
    var isAvailable: Bool { get }
    /// Start on-device-only recognition of `url`. `completion` may be called
    /// more than once; the session uses only the first call.
    func start(
        url: URL,
        completion: @escaping @Sendable (Result<String, Error>) -> Void
    ) -> OfflineRecognitionTask
}

protocol OfflineRecognitionTask: AnyObject, Sendable {
    func cancel()
}

struct SystemOfflineSpeechEngine: OfflineSpeechEngine {
    func authorize() async -> SFSpeechRecognizerAuthorizationStatus {
        await withCheckedContinuation { cont in
            SFSpeechRecognizer.requestAuthorization { status in
                cont.resume(returning: status)
            }
        }
    }

    func recognizer(locale: Locale) -> OfflineRecognizer? {
        SFSpeechRecognizer(locale: locale).map(SystemOfflineRecognizer.init)
    }
}

final class SystemOfflineRecognizer: OfflineRecognizer {
    private let recognizer: SFSpeechRecognizer

    init(_ recognizer: SFSpeechRecognizer) {
        self.recognizer = recognizer
    }

    var supportsOnDeviceRecognition: Bool { recognizer.supportsOnDeviceRecognition }
    var isAvailable: Bool { recognizer.isAvailable }

    /// The only place the offline request is built: it must never be allowed
    /// to reach Apple's servers.
    static func makeRequest(url: URL) -> SFSpeechURLRecognitionRequest {
        let request = SFSpeechURLRecognitionRequest(url: url)
        request.requiresOnDeviceRecognition = true
        request.shouldReportPartialResults = false
        return request
    }

    func start(
        url: URL,
        completion: @escaping @Sendable (Result<String, Error>) -> Void
    ) -> OfflineRecognitionTask {
        let task = recognizer.recognitionTask(with: Self.makeRequest(url: url)) { result, error in
            if let error = error {
                completion(.failure(error))
                return
            }
            guard let result = result, result.isFinal else { return }
            completion(.success(result.bestTranscription.formattedString))
        }
        return SystemRecognitionTask(task)
    }
}

final class SystemRecognitionTask: OfflineRecognitionTask, @unchecked Sendable {
    private let task: SFSpeechRecognitionTask

    init(_ task: SFSpeechRecognitionTask) {
        self.task = task
    }

    func cancel() {
        task.cancel()
    }
}

/// Offline-only recognitions keyed by the caller's `operation_id`.
///
/// Unlike `SpeechSession`, a recognition here can be stopped by
/// `media.speech.cancel`. The recognizer is cancelled before the pending
/// transcription replies, so once the caller sees that reply nothing here still
/// reads its clip. The clip itself belongs to the caller.
actor OfflineSpeechSession {
    /// `OFFLINE_SPEECH_DEADLINE_MS` in the Rust contract.
    static let deadlineNanoseconds: UInt64 = 30_000_000_000
    /// A cancel can arrive before the recognition it names; remember a few ids
    /// so a late start cannot resurrect a cancelled operation.
    private static let maxEarlyCancels = 32

    static let cancelledError = RpcError(
        code: -32003,
        message: "media.speech.transcribe_file: offline recognition cancelled",
        data: nil
    )

    private final class Operation {
        var cancelled = false
        var task: OfflineRecognitionTask?
        var finish: ResumeOnce<String>?
        var deadline: Task<Void, Never>?
    }

    private let engine: OfflineSpeechEngine
    private let deadlineNanoseconds: UInt64
    private var operations: [String: Operation] = [:]
    private var earlyCancels: [String] = []

    init(
        engine: OfflineSpeechEngine,
        deadlineNanoseconds: UInt64 = OfflineSpeechSession.deadlineNanoseconds
    ) {
        self.engine = engine
        self.deadlineNanoseconds = deadlineNanoseconds
    }

    /// Recognitions currently holding a clip.
    var activeOperations: Int { operations.count }

    func transcribe(
        audioPath: String,
        operationId: String,
        language: String?
    ) async throws -> (text: String, language: String) {
        if let index = earlyCancels.firstIndex(of: operationId) {
            earlyCancels.remove(at: index)
            throw Self.cancelledError
        }
        guard operations[operationId] == nil else {
            throw RpcError(
                code: -32602,
                message: "media.speech.transcribe_file: duplicate 'operation_id'",
                data: nil
            )
        }
        // Registered before the first suspension so a concurrent cancel finds it.
        let operation = Operation()
        operations[operationId] = operation
        defer {
            operation.deadline?.cancel()
            operations[operationId] = nil
        }

        guard FileManager.default.fileExists(atPath: audioPath) else {
            throw RpcError(
                code: -32602,
                message: "media.speech.transcribe_file: audio file not found",
                data: nil
            )
        }
        let status = await engine.authorize()
        if operation.cancelled { throw Self.cancelledError }
        guard status == .authorized else {
            throw RpcError(
                code: -32001,
                message: "media.speech.transcribe_file: speech recognition permission not granted",
                data: try? encodeCodable(Perm.guide(.speech_recognition))
            )
        }
        let identifier = language ?? Locale.current.identifier
        guard !identifier.isEmpty,
              let recognizer = engine.recognizer(locale: Locale(identifier: identifier)) else {
            throw RpcError(
                code: -32007,
                message: "media.speech.transcribe_file: unsupported language",
                data: nil
            )
        }
        guard recognizer.supportsOnDeviceRecognition, recognizer.isAvailable else {
            throw RpcError(
                code: -32002,
                message: "media.speech.transcribe_file: on-device recognition unavailable",
                data: nil
            )
        }

        let deadline = deadlineNanoseconds
        let text: String = try await withCheckedThrowingContinuation { rawCont in
            let finish = ResumeOnce(rawCont)
            operation.finish = finish
            let task = recognizer.start(url: URL(fileURLWithPath: audioPath)) { result in
                switch result {
                case .success(let text):
                    finish.resume(returning: text)
                case .failure:
                    finish.resume(throwing: RpcError(
                        code: -32003,
                        message: "media.speech.transcribe_file: offline recognition failed",
                        data: nil
                    ))
                }
            }
            operation.task = task
            operation.deadline = Task { [weak self] in
                try? await Task.sleep(nanoseconds: deadline)
                guard !Task.isCancelled else { return }
                await self?.expire(operationId: operationId)
            }
        }
        return (text, identifier)
    }

    /// The bounded deadline: like `cancel`, on this actor so the recognizer is
    /// stopped before the pending transcription can reply.
    private func expire(operationId: String) {
        // A cancelled deadline belongs to a finished operation, even if a new
        // one has since reused the id.
        guard !Task.isCancelled, let operation = operations[operationId] else { return }
        operation.finish?.resume(throwing: RpcError(
            code: -32004,
            message: "media.speech.transcribe_file: offline recognition timed out",
            data: nil
        ))
        operation.task?.cancel()
    }

    /// Stop the named recognition. `true` only when one was in flight.
    func cancel(operationId: String) -> Bool {
        guard let operation = operations[operationId] else {
            earlyCancels.append(operationId)
            if earlyCancels.count > Self.maxEarlyCancels {
                earlyCancels.removeFirst()
            }
            return false
        }
        operation.cancelled = true
        // Claim the reply first so the recognizer's own cancellation error is
        // not what the caller sees. The pending transcription resumes on this
        // actor, so it cannot reply before the recognizer below is stopped.
        operation.finish?.resume(throwing: Self.cancelledError)
        operation.task?.cancel()
        return true
    }
}
