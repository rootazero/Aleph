import Darwin
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
    let offlineStagingRoot = ProcessInfo.processInfo.environment["ALEPH_OFFLINE_STT_ROOT"]

    await router.register("media.speech.transcribe_file") { params in
        switch try transcribeRequest(from: params, offlineStagingRoot: offlineStagingRoot) {
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
            return result.rpcResult
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

extension OfflineSpeechResult {
    var rpcResult: JSONValue {
        .object([
            "operation_id": .string(operationId),
            "input_release": .string("detached"),
            "text": .string(text),
            "language": .string(language),
        ])
    }
}

enum TranscribeRequest: Equatable {
    /// The original request: `language` is required and the recognizer may use
    /// the network.
    case legacy(path: String, language: String)
    /// On-device only; `language` nil means the OS locale.
    case offline(path: String, operationId: String, language: String?)
}

func transcribeRequest(
    from params: JSONValue?,
    offlineStagingRoot: String? = nil
) throws -> TranscribeRequest {
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
    return .offline(
        path: try permittedOfflineAudioPath(path, stagingRoot: offlineStagingRoot),
        operationId: operationId,
        language: language
    )
}

private func permittedOfflineAudioPath(_ path: String, stagingRoot: String?) throws -> String {
    let invalidPath = RpcError(
        code: -32602,
        message: "media.speech.transcribe_file: 'audio_path' must be a staged offline clip",
        data: nil
    )
    let bytes = path.utf8
    let slash: UInt8 = 0x2f
    var rootInfo = stat()
    guard bytes.first == slash,
          !bytes.contains(0),
          !bytes.split(separator: slash).contains(where: {
              $0.elementsEqual(".".utf8) || $0.elementsEqual("..".utf8)
          }),
          let lastSlash = bytes.lastIndex(of: slash),
          let stagingRoot,
          let root = resolvedAbsolutePath(stagingRoot),
          root.utf8.elementsEqual(stagingRoot.utf8),
          lstat(root, &rootInfo) == 0,
          (rootInfo.st_mode & mode_t(S_IFMT)) == mode_t(S_IFDIR) else {
        throw invalidPath
    }

    let name = String(decoding: bytes[bytes.index(after: lastSlash)...], as: UTF8.self)
    // Core's ScratchClip uses a lowercase simple v4 UUID, not the RPC operation id.
    let stagedName = #/voice-turn-[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}\.wav/#
    let parent = String(decoding: bytes[..<lastSlash], as: UTF8.self)
    guard name.wholeMatch(of: stagedName) != nil,
          let resolvedParent = resolvedAbsolutePath(parent.isEmpty ? "/" : parent),
          resolvedParent.utf8.elementsEqual(root.utf8) else {
        throw invalidPath
    }

    let clip = (root == "/" ? root : root + "/") + name
    // This admission check is not the read boundary: the loader independently
    // opens without following a leaf symlink and checks the descriptor.
    var info = stat()
    guard lstat(clip, &info) == 0, (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG) else {
        throw invalidPath
    }
    return clip
}

private func resolvedAbsolutePath(_ path: String) -> String? {
    guard path.utf8.first == 0x2f, !path.utf8.contains(0), let resolved = realpath(path, nil) else {
        return nil
    }
    defer { free(resolved) }
    return String(validatingCString: resolved)
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
