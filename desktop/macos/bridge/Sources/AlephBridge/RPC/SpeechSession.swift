import AVFoundation
import Dispatch
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
    func authorize(completion: @escaping @Sendable (SFSpeechRecognizerAuthorizationStatus) -> Void)
    /// nil when the OS has no recognizer for `locale`.
    func recognizer(locale: Locale) -> OfflineRecognizer?
}

protocol OfflineRecognizer {
    var supportsOnDeviceRecognition: Bool { get }
    var isAvailable: Bool { get }
    /// Start on-device-only recognition of detached PCM. The session accepts
    /// at most one terminal callback.
    func start(
        buffer: AVAudioPCMBuffer,
        completion: @escaping @Sendable (Result<String, Error>) -> Void
    ) -> OfflineRecognitionTask
}

protocol OfflineRecognitionTask: AnyObject, Sendable {
    func cancel()
}

struct SystemOfflineSpeechEngine: OfflineSpeechEngine {
    func authorize(completion: @escaping @Sendable (SFSpeechRecognizerAuthorizationStatus) -> Void) {
        SFSpeechRecognizer.requestAuthorization { status in
            completion(status)
        }
    }

    func recognizer(locale: Locale) -> OfflineRecognizer? {
        SFSpeechRecognizer(locale: locale).map(SystemOfflineRecognizer.init)
    }
}

final class SystemOfflineRecognizer: OfflineRecognizer {
    private let recognizer: SFSpeechRecognizer

    init(_ recognizer: SFSpeechRecognizer) {
        let callbackQueue = OperationQueue()
        callbackQueue.maxConcurrentOperationCount = 1
        recognizer.queue = callbackQueue
        self.recognizer = recognizer
    }

    var supportsOnDeviceRecognition: Bool { recognizer.supportsOnDeviceRecognition }
    var isAvailable: Bool { recognizer.isAvailable }

    /// The only place the offline request is built: it must never be allowed
    /// to reach Apple's servers.
    static func makeRequest() -> SFSpeechAudioBufferRecognitionRequest {
        let request = SFSpeechAudioBufferRecognitionRequest()
        request.requiresOnDeviceRecognition = true
        request.shouldReportPartialResults = false
        return request
    }

    func start(
        buffer: AVAudioPCMBuffer,
        completion: @escaping @Sendable (Result<String, Error>) -> Void
    ) -> OfflineRecognitionTask {
        let request = Self.makeRequest()
        let task = recognizer.recognitionTask(with: request) { [request, buffer] result, error in
            // Also retain inputs via the native callback if cancel outlives our task handle.
            defer { withExtendedLifetime((request, buffer)) {} }
            if let error = error {
                completion(.failure(error))
                return
            }
            guard let result = result, result.isFinal else { return }
            completion(.success(result.bestTranscription.formattedString))
        }
        request.append(buffer)
        request.endAudio()
        return SystemRecognitionTask(task, request: request, buffer: buffer)
    }
}

final class SystemRecognitionTask: OfflineRecognitionTask, @unchecked Sendable {
    private let task: SFSpeechRecognitionTask
    private let request: SFSpeechAudioBufferRecognitionRequest
    private let buffer: AVAudioPCMBuffer

    init(
        _ task: SFSpeechRecognitionTask,
        request: SFSpeechAudioBufferRecognitionRequest,
        buffer: AVAudioPCMBuffer
    ) {
        self.task = task
        self.request = request
        self.buffer = buffer
    }

    func cancel() {
        task.cancel()
    }
}

struct OfflineSpeechResult: Sendable {
    let operationId: String
    let text: String
    let language: String
}

/// Offline-only recognitions keyed by the caller's `operation_id`.
///
/// `media.speech.cancel` withdraws the operation and requests cancellation of
/// any installed recognition task. The reply classifies the operation outcome;
/// it does not attest that Speech has released the caller's clip.
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

    private static let timeoutError = RpcError(
        code: -32004,
        message: "media.speech.transcribe_file: offline recognition timed out",
        data: nil
    )

    private static let preparationError = RpcError(
        code: -32003,
        message: "media.speech.transcribe_file: offline audio preparation failed",
        data: nil
    )

    private final class Operation {
        enum Phase {
            case reserved
            case authorizing(ResumeOnce<SFSpeechRecognizerAuthorizationStatus>)
            case preparing
            case withdrawingPreparation(RpcError)
            case recognizing(ResumeOnce<String>)
            case withdrawn(RpcError)
        }

        var phase: Phase = .reserved
        var task: OfflineRecognitionTask?
        var deadline: Task<Void, Never>?
        var preparationWaiters: [CheckedContinuation<Void, Never>] = []
    }

    private let engine: OfflineSpeechEngine
    private let loadAudio: @Sendable (String) throws -> OfflineSpeechAudio
    private let deadlineNanoseconds: UInt64
    private var operations: [String: Operation] = [:]
    private var earlyCancels: [String] = []

    init(
        engine: OfflineSpeechEngine,
        deadlineNanoseconds: UInt64 = OfflineSpeechSession.deadlineNanoseconds,
        loadAudio: @escaping @Sendable (String) throws -> OfflineSpeechAudio = {
            try OfflineSpeechAudio.load(fromTrustedPath: $0)
        }
    ) {
        self.engine = engine
        self.deadlineNanoseconds = deadlineNanoseconds
        self.loadAudio = loadAudio
    }

    /// Operations currently reserved, including those awaiting preparation join.
    var activeOperations: Int { operations.count }

    func transcribe(
        audioPath: String,
        operationId: String,
        language: String?
    ) async throws -> OfflineSpeechResult {
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
        let clock = ContinuousClock()
        let expiresAt = clock.now.advanced(by: .nanoseconds(deadlineNanoseconds))
        let operation = Operation()
        operations[operationId] = operation
        operation.deadline = Task { [weak self, weak operation] in
            do {
                try await clock.sleep(until: expiresAt, tolerance: nil)
            } catch {
                return
            }
            guard !Task.isCancelled, let operation else { return }
            await self?.withdraw(operationId: operationId, operation: operation, error: Self.timeoutError)
        }
        defer {
            operation.deadline?.cancel()
            if operations[operationId] === operation {
                operations[operationId] = nil
            }
            for waiter in operation.preparationWaiters { waiter.resume() }
        }

        let status: SFSpeechRecognizerAuthorizationStatus = try await withCheckedThrowingContinuation { rawCont in
            let finish = ResumeOnce(rawCont)
            operation.phase = .authorizing(finish)
            engine.authorize { status in
                finish.resume(returning: status)
            }
        }
        // Authorization may have claimed its continuation before withdrawal,
        // without the suspended actor turn having resumed yet.
        if case .withdrawn(let error) = operation.phase { throw error }
        guard clock.now < expiresAt else { throw Self.timeoutError }
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

        guard clock.now < expiresAt else { throw Self.timeoutError }
        operation.phase = .preparing
        // GCD handles blocking file I/O; the transcribe RPC joins it even when
        // withdrawn. The continuation is completed only after the loader returns.
        let loaded: Result<OfflineSpeechAudio, Error> = await withCheckedContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async { [loadAudio] in
                continuation.resume(returning: Result { try loadAudio(audioPath) })
            }
        }
        if case .withdrawingPreparation(let error) = operation.phase { throw error }
        guard operations[operationId] === operation, case .preparing = operation.phase else {
            throw Self.cancelledError
        }
        guard clock.now < expiresAt else { throw Self.timeoutError }
        guard case .success(let audio) = loaded, let buffer = audio.makePCMBuffer() else {
            throw Self.preparationError
        }

        guard clock.now < expiresAt else { throw Self.timeoutError }
        let text: String = try await withCheckedThrowingContinuation { rawCont in
            let finish = ResumeOnce(rawCont)
            operation.phase = .recognizing(finish)
            let task = recognizer.start(buffer: buffer) { result in
                // Leave late replies to the deadline task so it can cancel the installed task.
                guard clock.now < expiresAt else { return }
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
        }
        if case .withdrawn(let error) = operation.phase { throw error }
        guard clock.now < expiresAt else {
            withdraw(operationId: operationId, operation: operation, error: Self.timeoutError)
            throw Self.timeoutError
        }
        return OfflineSpeechResult(operationId: operationId, text: text, language: identifier)
    }

    private func withdraw(operationId: String, operation: Operation, error: RpcError) {
        guard operations[operationId] === operation else { return }
        let phase = operation.phase
        switch phase {
        case .preparing:
            // The loader might still hold a descriptor. Keep the reservation
            // and leave both RPCs suspended until the actual read returns.
            operation.phase = .withdrawingPreparation(error)
            operation.deadline?.cancel()
            return
        case .withdrawingPreparation:
            return
        default:
            break
        }
        operations[operationId] = nil
        operation.deadline?.cancel()
        operation.phase = .withdrawn(error)
        // Claim the classification before any cancellation callback can reply.
        switch phase {
        case .authorizing(let finish):
            finish.resume(throwing: error)
        case .recognizing(let finish):
            finish.resume(throwing: error)
            operation.task?.cancel()
        case .reserved, .preparing, .withdrawingPreparation, .withdrawn:
            break
        }
    }

    /// Withdraw the named operation. `true` only when one was in flight.
    func cancel(operationId: String) async -> Bool {
        guard let operation = operations[operationId] else {
            earlyCancels.append(operationId)
            if earlyCancels.count > Self.maxEarlyCancels {
                earlyCancels.removeFirst()
            }
            return false
        }
        withdraw(operationId: operationId, operation: operation, error: Self.cancelledError)
        if case .withdrawingPreparation = operation.phase {
            await withCheckedContinuation { continuation in
                operation.preparationWaiters.append(continuation)
            }
        }
        return true
    }
}
