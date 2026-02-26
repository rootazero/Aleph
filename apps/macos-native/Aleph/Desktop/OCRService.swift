import Vision
import CoreImage
import Foundation

/// OCR service using the native macOS Vision framework.
///
/// Provides text recognition from images, matching the Rust Tauri implementation
/// in `apps/desktop/src-tauri/src/bridge/perception.rs` but using native Swift
/// APIs instead of ~160 lines of objc FFI.
///
/// Wire format (matches Rust `handle_ocr`):
/// - Input:  `{ "image_base64": "<base64 PNG>" }` or `{}` (capture screen first)
/// - Output: `{ "text": "full text", "lines": [{ "text": "...", "confidence": 0.95 }] }`
enum OCRService {

    /// A single recognized line of text with its confidence score.
    struct Line {
        let text: String
        let confidence: Float
    }

    /// The result of an OCR operation: full concatenated text plus per-line details.
    struct Result {
        let text: String
        let lines: [Line]
    }

    // MARK: - Public API

    /// Perform OCR on a base64-encoded PNG image.
    ///
    /// Uses `VNRecognizeTextRequest` with accurate recognition level and
    /// language correction enabled for Chinese (Simplified) and English.
    ///
    /// - Parameter imageBase64: Base64-encoded image data (PNG or JPEG).
    /// - Returns: A `Result` containing recognized text, or a `HandlerError`.
    static func recognize(imageBase64: String) -> Swift.Result<Result, BridgeServer.HandlerError> {
        // Decode base64 to raw image data
        guard let imageData = Data(base64Encoded: imageBase64) else {
            return .failure(.init(
                code: .internal,
                message: "Invalid base64 encoding"
            ))
        }

        guard let ciImage = CIImage(data: imageData) else {
            return .failure(.init(
                code: .internal,
                message: "Failed to create image from data"
            ))
        }

        return recognizeImage(ciImage)
    }

    /// Perform OCR on raw PNG/JPEG data.
    ///
    /// - Parameter imageData: Raw image bytes.
    /// - Returns: A `Result` containing recognized text, or a `HandlerError`.
    static func recognize(imageData: Data) -> Swift.Result<Result, BridgeServer.HandlerError> {
        guard let ciImage = CIImage(data: imageData) else {
            return .failure(.init(
                code: .internal,
                message: "Failed to create image from data"
            ))
        }

        return recognizeImage(ciImage)
    }

    // MARK: - Private

    /// Core recognition logic using VNRecognizeTextRequest.
    private static func recognizeImage(_ ciImage: CIImage) -> Swift.Result<Result, BridgeServer.HandlerError> {
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        request.usesLanguageCorrection = true
        request.recognitionLanguages = ["zh-Hans", "en-US"]

        let handler = VNImageRequestHandler(ciImage: ciImage, options: [:])
        do {
            try handler.perform([request])
        } catch {
            return .failure(.init(
                code: .internal,
                message: "Vision OCR failed: \(error.localizedDescription)"
            ))
        }

        guard let observations = request.results else {
            return .success(Result(text: "", lines: []))
        }

        var lines: [Line] = []
        var fullText = ""

        for observation in observations {
            guard let candidate = observation.topCandidates(1).first else { continue }
            lines.append(Line(text: candidate.string, confidence: candidate.confidence))
            if !fullText.isEmpty { fullText += "\n" }
            fullText += candidate.string
        }

        return .success(Result(text: fullText, lines: lines))
    }
}

// MARK: - AnyCodable Conversion

extension OCRService.Result {
    /// Convert to AnyCodable for JSON-RPC response.
    ///
    /// Produces: `{ "text": "...", "lines": [{ "text": "...", "confidence": 0.95 }] }`
    var asAnyCodable: AnyCodable {
        let lineValues: [AnyCodable] = lines.map { line in
            AnyCodable([
                "text": AnyCodable(line.text),
                "confidence": AnyCodable(Double(line.confidence)),
            ])
        }
        return AnyCodable([
            "text": AnyCodable(text),
            "lines": AnyCodable(lineValues),
        ])
    }
}
