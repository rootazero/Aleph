import AVFoundation
import Foundation

/// Register media.audio.{list_devices,record,record_start,record_stop} JSON-RPC
/// handlers.
///
/// All of them delegate to a shared serial `AudioSession` actor so that
/// concurrent RPC calls do not collide on the single microphone device.
///
/// A fifth handler, `media.audio.mic_meter`, was registered here until
/// 2026-08-09. It kept a separate long-lived `MicMeterSession` with an
/// `AVAudioEngine` tap installed across calls; the core-side consumer that
/// polled it never had a subscriber of its own and was removed, so the tap
/// went with it.
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

    await router.register("media.audio.record_start") { _ in
        try await session.recordStart()
        return .object([:])
    }

    await router.register("media.audio.record_stop") { _ in
        let (filePath, duration, format) = try await session.recordStop()
        return .object([
            "file_path": .string(filePath),
            "duration_secs": .number(duration),
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
