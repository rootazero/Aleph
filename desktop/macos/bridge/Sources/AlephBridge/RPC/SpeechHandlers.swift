import Foundation
import Speech

/// Register the media.speech.* JSON-RPC handlers.
///
/// A request without `offline_only` keeps the original behavior through the
/// shared serial `SpeechSession` actor. An `offline_only` request runs in
/// `OfflineSpeechSession`, which never uses a network recognizer and which a
/// `media.speech.cancel` for the same `operation_id` stops mid-recognition.
func registerSpeechHandlers(_ router: Router) async {
    let session = SpeechSession()
    let offline = OfflineSpeechSession(engine: SystemOfflineSpeechEngine())

    await router.register("media.speech.transcribe_file") { params in
        switch try transcribeRequest(from: params) {
        case .legacy(let path, let language):
            let text = try await session.transcribe(audioPath: path, language: language)
            return .object([
                "text": .string(text),
                "language": .string(language),
            ])
        case .offline(let path, let operationId, let language):
            let result = try await offline.transcribe(
                audioPath: path,
                operationId: operationId,
                language: language
            )
            return .object([
                "text": .string(result.text),
                "language": .string(result.language),
            ])
        }
    }

    await router.register("media.speech.cancel") { params in
        guard case .object(let o) = params ?? .null,
              case .string(let operationId) = o["operation_id"] ?? .null,
              !operationId.isEmpty else {
            throw RpcError(
                code: -32602,
                message: "media.speech.cancel: missing/invalid 'operation_id'",
                data: nil
            )
        }
        return .object(["cancelled": .bool(await offline.cancel(operationId: operationId))])
    }
}

enum TranscribeRequest: Equatable {
    /// The original request: `language` is required and the recognizer may use
    /// the network.
    case legacy(path: String, language: String)
    /// On-device only; `language` nil means the OS locale.
    case offline(path: String, operationId: String, language: String?)
}

func transcribeRequest(from params: JSONValue?) throws -> TranscribeRequest {
    guard case .object(let o) = params ?? .null,
          case .string(let path) = o["audio_path"] ?? .null else {
        throw RpcError(
            code: -32602,
            message: "media.speech.transcribe_file: missing/invalid 'audio_path' or 'language'",
            data: nil
        )
    }
    let policy = o["offline_only"] ?? .null
    if case .null = policy {
        guard case .string(let lang) = o["language"] ?? .null else {
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
        return .legacy(path: try permittedAudioPath(path), language: lang)
    }
    // The offline caller never sends the legacy field; a request carrying both
    // is ambiguous about which recognizer it allows.
    let invalidPolicy = RpcError(
        code: -32602,
        message: "media.speech.transcribe_file: invalid 'offline_only' request",
        data: nil
    )
    guard o["language"] == nil,
          case .object(let p) = policy,
          case .string(let operationId) = p["operation_id"] ?? .null,
          !operationId.isEmpty, !path.isEmpty else {
        throw invalidPolicy
    }
    let language: String?
    switch p["language"] ?? .null {
    case .null: language = nil
    case .string(let tag): language = tag
    default: throw invalidPolicy
    }
    return .offline(path: try permittedAudioPath(path), operationId: operationId, language: language)
}

/// Accept only files inside `~/.aleph/data/_media/`.
private func permittedAudioPath(_ path: String) throws -> String {
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
    return path
}
