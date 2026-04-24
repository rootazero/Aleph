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
        return try await withCheckedThrowingContinuation { (cont: CheckedContinuation<String, Error>) in
            let lock = NSLock()
            var resumed = false
            func resumeOnce(_ body: () -> Void) {
                lock.lock()
                defer { lock.unlock() }
                if resumed { return }
                resumed = true
                body()
            }

            let task = recognizer.recognitionTask(with: request) { result, error in
                if let error = error {
                    resumeOnce {
                        cont.resume(throwing: RpcError(
                            code: -32003,
                            message: "media.speech.transcribe_file: \(error.localizedDescription)",
                            data: nil
                        ))
                    }
                    return
                }
                guard let result = result, result.isFinal else { return }
                let text = result.bestTranscription.formattedString
                resumeOnce { cont.resume(returning: text) }
            }

            // Safety timeout so the continuation never leaks if
            // SFSpeechRecognizer stalls.
            Task {
                try? await Task.sleep(nanoseconds: 60_000_000_000)
                task.cancel()
                resumeOnce {
                    cont.resume(throwing: RpcError(
                        code: -32003,
                        message: "media.speech.transcribe_file: timed out after 60s",
                        data: nil
                    ))
                }
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
