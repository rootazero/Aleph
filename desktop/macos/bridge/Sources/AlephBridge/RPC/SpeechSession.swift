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
