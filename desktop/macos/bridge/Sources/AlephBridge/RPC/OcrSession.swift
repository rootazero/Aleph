import AppKit
import Foundation
import Vision

/// Serializes VNRecognizeTextRequest calls. Vision can be invoked concurrently
/// in principle, but routing through an actor matches the audio/speech pattern
/// and bounds resource use.
actor OcrSession {
    func recognize(
        pngData: Data,
        languages: [String],
        fastMode: Bool
    ) async throws -> (fullText: String, blocks: [[String: JSONValue]]) {
        guard let nsImage = NSImage(data: pngData),
              let cgImage = nsImage.cgImage(forProposedRect: nil, context: nil, hints: nil)
        else {
            throw RpcError(
                code: -32602,
                message: "screen.ocr: failed to decode image from base64 data",
                data: nil
            )
        }

        return try await withCheckedThrowingContinuation { rawCont in
            // Vision hands the error to the request's completion handler AND
            // rethrows it out of `perform`, so both arms below fire for the same
            // failure. Resuming twice is a process-killing fatal error, not an
            // exception — see `ResumeOnce`.
            let cont = ResumeOnce<(fullText: String, blocks: [[String: JSONValue]])>(rawCont)

            let req = VNRecognizeTextRequest { request, error in
                if let error = error {
                    cont.resume(throwing: RpcError(
                        code: -32003,
                        message: "screen.ocr: VNRecognizeTextRequest failed: \(error.localizedDescription)",
                        data: nil
                    ))
                    return
                }

                guard let observations = request.results as? [VNRecognizedTextObservation] else {
                    cont.resume(returning: ("", []))
                    return
                }

                var lines: [[String: JSONValue]] = []
                var textParts: [String] = []

                for obs in observations {
                    guard let candidate = obs.topCandidates(1).first else { continue }
                    let text = candidate.string
                    let confidence = Double(candidate.confidence)
                    let bb = obs.boundingBox  // normalized, bottom-left origin

                    textParts.append(text)
                    lines.append([
                        "text": .string(text),
                        "bbox": .object([
                            "x": .number(bb.origin.x),
                            "y": .number(bb.origin.y),
                            "width": .number(bb.size.width),
                            "height": .number(bb.size.height),
                        ]),
                        "confidence": .number(confidence),
                    ])
                }

                cont.resume(returning: (textParts.joined(separator: "\n"), lines))
            }

            if !languages.isEmpty {
                req.recognitionLanguages = languages
            }
            req.recognitionLevel = fastMode ? .fast : .accurate
            req.usesLanguageCorrection = true

            let handler = VNImageRequestHandler(cgImage: cgImage, options: [:])
            do {
                try handler.perform([req])
            } catch {
                cont.resume(throwing: RpcError(
                    code: -32003,
                    message: "screen.ocr: VNImageRequestHandler.perform failed: \(error.localizedDescription)",
                    data: nil
                ))
            }
        }
    }
}
