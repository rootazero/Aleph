import Foundation
import Vision

/// Register the screen.ocr JSON-RPC handler.
///
/// Delegates to a shared serial `OcrSession` actor so that concurrent RPC
/// calls do not pile up on Vision resources. Mirrors the audio/speech pattern.
func registerOcrHandlers(_ router: Router) async {
    let session = OcrSession()

    await router.register("screen.ocr") { params in
        let (imageData, languages, fastMode) = try ocrArgs(from: params)
        let (fullText, blocks) = try await session.recognize(
            pngData: imageData,
            languages: languages,
            fastMode: fastMode
        )
        return .object([
            "full_text": .string(fullText),
            "blocks": .array(blocks.map { .object($0) }),
        ])
    }
}

private func ocrArgs(from params: JSONValue?) throws -> (Data, [String], Bool) {
    guard case .object(let o) = params ?? .null,
          case .string(let b64) = o["image_base64"] ?? .null
    else {
        throw RpcError(
            code: -32602,
            message: "screen.ocr: missing/invalid 'image_base64'",
            data: nil
        )
    }

    guard let imageData = Data(base64Encoded: b64), !imageData.isEmpty else {
        throw RpcError(
            code: -32602,
            message: "screen.ocr: 'image_base64' is not valid base64 or is empty",
            data: nil
        )
    }

    // languages: optional array of strings; defaults to [] (Vision picks)
    var languages: [String] = []
    if case .array(let langArray) = o["languages"] ?? .null {
        languages = langArray.compactMap {
            if case .string(let s) = $0 { return s } else { return nil }
        }
    }

    // fast_mode: optional bool; defaults to false
    var fastMode = false
    if case .bool(let fm) = o["fast_mode"] ?? .null {
        fastMode = fm
    }

    return (imageData, languages, fastMode)
}
