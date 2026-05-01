import Foundation
import Speech

/// Register media.speech.transcribe_file JSON-RPC handler.
///
/// Delegates to a shared serial `SpeechSession` actor so that concurrent RPC
/// calls do not race on SFSpeechRecognizer state. Mirrors the audio pattern.
func registerSpeechHandlers(_ router: Router) async {
    let session = SpeechSession()

    await router.register("media.speech.transcribe_file") { params in
        let (path, language) = try transcribeArgs(from: params)
        let text = try await session.transcribe(audioPath: path, language: language)
        return .object([
            "text": .string(text),
            "language": .string(language),
        ])
    }
}

private func transcribeArgs(from params: JSONValue?) throws -> (String, String) {
    guard case .object(let o) = params ?? .null,
          case .string(let path) = o["audio_path"] ?? .null,
          case .string(let lang) = o["language"] ?? .null else {
        throw RpcError(
            code: -32602,
            message: "media.speech.transcribe_file: missing/invalid 'audio_path' or 'language'",
            data: nil
        )
    }
    guard !path.isEmpty, !lang.isEmpty else {
        throw RpcError(
            code: -32602,
            message: "media.speech.transcribe_file: 'audio_path' and 'language' must be non-empty",
            data: nil
        )
    }
    // Reject path traversal attempts.
    guard !path.contains("..") else {
        throw RpcError(
            code: -32602,
            message: "media.speech.transcribe_file: 'audio_path' must not contain '..'",
            data: nil
        )
    }
    let mediaDir = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent(".aleph/data/_media")
    let audioURL = URL(fileURLWithPath: path)
    let resolvedAudio = audioURL.resolvingSymlinksInPath()
    let resolvedMedia = mediaDir.resolvingSymlinksInPath()
    guard resolvedAudio.path.hasPrefix(resolvedMedia.path) else {
        throw RpcError(
            code: -32602,
            message: "media.speech.transcribe_file: 'audio_path' must be inside ~/.aleph/data/_media/",
            data: nil
        )
    }
    return (path, lang)
}
