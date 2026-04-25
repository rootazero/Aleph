import AVFoundation
import Foundation

/// Register media.audio.{list_devices,record} JSON-RPC handlers.
///
/// Both delegate to a shared serial `AudioSession` actor so that concurrent
/// RPC calls do not collide on the single microphone device.
func registerAudioHandlers(_ router: Router) async {
    let session = AudioSession()

    await router.register("media.audio.list_devices") { _ in
        let devices = try await session.listDevices()
        let deviceArray = devices.map { info -> JSONValue in
            .object([
                "uid": .string(info.uid),
                "name": .string(info.name),
                "is_input": .bool(info.isInput),
                "is_default": .bool(info.isDefault),
            ])
        }
        return .object(["devices": .array(deviceArray)])
    }

    await router.register("media.audio.record") { params in
        let duration = try recordDuration(from: params)
        let (filePath, actualDuration, format) = try await session.record(
            durationSecs: duration
        )
        return .object([
            "file_path": .string(filePath),
            "duration_secs": .number(actualDuration),
            "format": .string(format),
        ])
    }
}

private func recordDuration(from params: JSONValue?) throws -> Double {
    guard case .object(let o) = params ?? .null,
          case .number(let d) = o["duration_secs"] ?? .null else {
        throw RpcError(
            code: -32602,
            message: "media.audio.record: missing/invalid 'duration_secs'",
            data: nil
        )
    }
    guard d >= 0.25, d <= 300.0 else {
        throw RpcError(
            code: -32602,
            message: "media.audio.record: 'duration_secs' out of range [0.25, 300.0]",
            data: nil
        )
    }
    return d
}
