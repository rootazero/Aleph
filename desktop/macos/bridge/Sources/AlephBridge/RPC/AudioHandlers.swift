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
func registerAudioHandlers(_ router: Router, session: AudioSession) async {

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

    await router.register("media.audio.voice_capture_start") { params in
        let args = try voiceParams(params, as: VoiceCaptureStart.self)
        guard args.sampleRateHz == VoiceCaptureBuffer.sampleRate,
              args.frameSamples == VoiceCaptureBuffer.frameSamples,
              args.maxClipSamples == VoiceCaptureBuffer.maxClipSamples,
              let ingress = Server.requestIngress else { throw invalidVoiceParams() }
        try await session.voiceCaptureStart(id: args.captureID, clipLimit: args.maxClipSamples,
                                            ingress: ingress.receivedAt, input: ingress.input)
        return .object([:])
    }

    await router.register("media.audio.voice_capture_poll") { params in
        let args = try voiceParams(params, as: VoiceCapturePoll.self)
        let observations = try await session.voiceCapturePoll(id: args.captureID, after: args.afterSample)
        return try encodeCodable(VoiceCapturePollReply(observations: observations))
    }

    await router.register("media.audio.voice_capture_seal") { params in
        let args = try voiceParams(params, as: VoiceCaptureSeal.self)
        let wav = try await session.voiceCaptureSeal(id: args.captureID, first: args.firstSample, end: args.endSample)
        return try encodeCodable(VoiceCaptureSealReply(wavBase64: wav.base64EncodedString()))
    }

    await router.register("media.audio.voice_capture_cancel") { params in
        let args = try voiceParams(params, as: VoiceCaptureCancel.self)
        try await session.voiceCaptureCancel(id: args.captureID)
        return .object([:])
    }
}

private struct VoiceCaptureStart: Decodable {
    let captureID: UUID
    let sampleRateHz: Int
    let frameSamples: Int
    let maxClipSamples: Int

    enum CodingKeys: String, CodingKey {
        case captureID = "capture_id", sampleRateHz = "sample_rate_hz"
        case frameSamples = "frame_samples", maxClipSamples = "max_clip_samples"
    }
}

private struct VoiceCapturePoll: Decodable {
    let captureID: UUID
    let afterSample: Int

    enum CodingKeys: String, CodingKey {
        case captureID = "capture_id", afterSample = "after_sample"
    }
}

private struct VoiceCaptureSeal: Decodable {
    let captureID: UUID
    let firstSample: Int
    let endSample: Int

    enum CodingKeys: String, CodingKey {
        case captureID = "capture_id", firstSample = "first_sample", endSample = "end_sample"
    }
}

private struct VoiceCaptureCancel: Decodable {
    let captureID: UUID

    enum CodingKeys: String, CodingKey { case captureID = "capture_id" }
}

private struct VoiceCapturePollReply: Encodable {
    let observations: [VoiceCaptureObservation]
}

private struct VoiceCaptureSealReply: Encodable {
    let wavBase64: String

    enum CodingKeys: String, CodingKey { case wavBase64 = "wav_base64" }
}

private func voiceParams<T: Decodable>(_ params: JSONValue?, as type: T.Type) throws -> T {
    do { return try decodeCodable(params, as: type) }
    catch { throw invalidVoiceParams() }
}

private func invalidVoiceParams() -> RpcError {
    RpcError(code: -32602, message: "media.audio.voice_capture: invalid parameters", data: nil)
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
