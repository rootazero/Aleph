import AVFoundation
import AppKit
import Foundation

/// Register media.camera.{snap,clip} JSON-RPC handlers.
///
/// Both delegate to a shared serial `CameraSession` actor so that concurrent
/// RPC calls do not collide on the AVCaptureSession's single hardware slot.
func registerCameraHandlers(_ router: Router) async {
    let session = CameraSession()

    await router.register("media.camera.snap") { params in
        let quality = try snapQuality(from: params)
        let (jpeg, width, height) = try await session.snap(quality: quality)
        let b64 = jpeg.base64EncodedString()
        return .object([
            "image_base64": .string(b64),
            "width": .number(Double(width)),
            "height": .number(Double(height)),
        ])
    }

    await router.register("media.camera.clip") { params in
        let (duration, withAudio) = try clipArgs(from: params)
        let (path, actualDuration, hasAudio) = try await session.clip(
            duration: duration, withAudio: withAudio
        )
        return .object([
            "file_path": .string(path),
            "duration_secs": .number(actualDuration),
            "has_audio": .bool(hasAudio),
        ])
    }
}

private func snapQuality(from params: JSONValue?) throws -> Float {
    guard case .object(let o) = params ?? .null,
          case .number(let q) = o["quality"] ?? .null else {
        throw RpcError(
            code: -32602,
            message: "media.camera.snap: missing/invalid 'quality'",
            data: nil
        )
    }
    guard q >= 0.05, q <= 1.0 else {
        throw RpcError(
            code: -32602,
            message: "media.camera.snap: 'quality' out of range [0.05, 1.0]",
            data: nil
        )
    }
    return Float(q)
}

private func clipArgs(from params: JSONValue?) throws -> (Double, Bool) {
    guard case .object(let o) = params ?? .null,
          case .number(let d) = o["duration_secs"] ?? .null,
          case .bool(let withAudio) = o["with_audio"] ?? .null else {
        throw RpcError(
            code: -32602,
            message: "media.camera.clip: bad params",
            data: nil
        )
    }
    guard d >= 0.25, d <= 60.0 else {
        throw RpcError(
            code: -32602,
            message: "media.camera.clip: 'duration_secs' out of range [0.25, 60.0]",
            data: nil
        )
    }
    return (d, withAudio)
}
