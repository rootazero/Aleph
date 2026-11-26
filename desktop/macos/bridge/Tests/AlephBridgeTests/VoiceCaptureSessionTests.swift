import Foundation
import XCTest
@testable import AlephBridge

final class VoiceCaptureSessionTests: XCTestCase {
    func testCancelWhilePermissionIsPendingCannotOpenLateOrAffectNextLease() async throws {
        let firstRequest = expectation(description: "permission requested")
        let answers = PermissionAnswers(firstRequest: firstRequest)
        let device = FakeVoiceDevice()
        let session = AudioSession(requestVoicePermission: { answers.request($0) }, openVoiceDevice: { _, _ in
            device.didOpen()
            return device
        })
        let cancelled = UUID()
        let first = Task { try await session.voiceCaptureStart(id: cancelled, clipLimit: 320_000, ingress: ContinuousClock().now) }
        await fulfillment(of: [firstRequest], timeout: 2)
        let staleReply = try XCTUnwrap(answers.takeFirst())
        try await session.voiceCaptureCancel(id: cancelled)
        do {
            try await first.value
            XCTFail("cancelled start returned activation")
        } catch is CancellationError {}

        let next = UUID()
        answers.enableAutoAllow()
        staleReply(true)
        try await session.voiceCaptureStart(id: next, clipLimit: 320_000, ingress: ContinuousClock().now)
        try await session.voiceCaptureCancel(id: cancelled)
        let observations = try await session.voiceCapturePoll(id: next, after: 0)
        XCTAssertEqual(observations, [])
        XCTAssertEqual(device.opens, 1)
        XCTAssertFalse(device.stopped)
        try await session.voiceCaptureCancel(id: next)
        XCTAssertTrue(device.stopped)
    }

    func testUnmatchedCancelSaturationDoesNotReleaseTheOtherActiveID() async throws {
        let device = FakeVoiceDevice()
        let session = AudioSession(requestVoicePermission: { $0(true) }, openVoiceDevice: { _, _ in device })
        let active = UUID()
        try await session.voiceCaptureStart(id: active, clipLimit: 320_000, ingress: ContinuousClock().now)
        for _ in 0..<VoiceCaptureAdmission.unmatchedLimit { try await session.voiceCaptureCancel(id: UUID()) }
        let overflow = UUID()
        do {
            try await session.voiceCaptureCancel(id: overflow)
            XCTFail("saturated cancel history must not acknowledge the unknown ID")
        } catch let error as RpcError {
            XCTAssertTrue(error.message.contains("retire helper generation"))
        }
        let observations = try await session.voiceCapturePoll(id: active, after: 0)
        XCTAssertEqual(observations, [])
        XCTAssertFalse(device.stopped)
        try await session.voiceCaptureCancel(id: active)
        do {
            try await session.voiceCaptureStart(id: overflow, clipLimit: 320_000, ingress: ContinuousClock().now)
            XCTFail("retired generation opened a possibly cancelled ID")
        } catch let error as RpcError {
            XCTAssertTrue(error.message.contains("helper generation retired"))
        }
    }

    func testInputEOFStopsPermissionPendingActivationBeforeLateCallback() async throws {
        let firstRequest = expectation(description: "permission requested")
        let answers = PermissionAnswers(firstRequest: firstRequest)
        let input = Server.InputLifetime()
        let session = AudioSession(requestVoicePermission: { answers.request($0) }, openVoiceDevice: { _, _ in
            XCTFail("EOF must prevent opening the device")
            throw CancellationError()
        })
        let id = UUID()
        let start = Task { try await session.voiceCaptureStart(id: id, clipLimit: 320_000, ingress: ContinuousClock().now, input: input) }
        await fulfillment(of: [firstRequest], timeout: 2)
        let reply = try XCTUnwrap(answers.takeFirst())
        input.close()
        reply(true)
        await session.closeVoiceCaptureInput()
        do {
            try await start.value
            XCTFail("start activated after EOF")
        } catch is CancellationError {
        } catch let error as RpcError {
            XCTAssertTrue(error.message.contains("permission denied, cancelled"))
        }
    }

    func testLateDispatchAndEarlyCancelDoNotAskForPermission() async throws {
        let session = AudioSession(requestVoicePermission: { reply in
            XCTFail("rejected start reached permission")
            reply(false)
        }, openVoiceDevice: { _, _ in throw CancellationError() })
        let cancelled = UUID()
        try await session.voiceCaptureCancel(id: cancelled)
        for (id, ingress) in [(cancelled, ContinuousClock().now), (UUID(), ContinuousClock().now.advanced(by: .seconds(-10)))] {
            do {
                try await session.voiceCaptureStart(id: id, clipLimit: 320_000, ingress: ingress)
                XCTFail("rejected start returned activation")
            } catch let error as RpcError {
                XCTAssertTrue(error.message.contains("start expired, cancelled"))
            }
        }
    }

    func testWireActivationIsNotReadinessAndSealReturnsOnlyWAVAfterStop() async throws {
        let device = FakeVoiceDevice()
        let session = AudioSession(requestVoicePermission: { $0(true) }, openVoiceDevice: { _, _ in device })
        let router = Router()
        await registerAudioHandlers(router, session: session)
        let id = UUID().uuidString
        let ingress = Server.RequestIngress(receivedAt: ContinuousClock().now, input: Server.InputLifetime())
        let start = try await Server.$requestIngress.withValue(ingress) {
            try await router.handle(method: "media.audio.voice_capture_start", params: .object([
                "capture_id": .string(id), "sample_rate_hz": .number(16_000),
                "frame_samples": .number(800), "max_clip_samples": .number(320_000),
            ]))
        }
        guard case .object(let startFields) = start else { return XCTFail("expected activation object") }
        XCTAssertTrue(startFields.isEmpty)
        let pollArgs: JSONValue = .object(["capture_id": .string(id), "after_sample": .number(0)])
        let unready = try await router.handle(method: "media.audio.voice_capture_poll", params: pollArgs)
        guard case .object(let unreadyFields) = unready, case .array(let empty)? = unreadyFields["observations"] else {
            return XCTFail("expected empty observations")
        }
        XCTAssertEqual(unreadyFields.count, 1)
        XCTAssertTrue(empty.isEmpty)
        try device.feedFrame()
        let ready = try await router.handle(method: "media.audio.voice_capture_poll", params: pollArgs)
        guard case .object(let readyFields) = ready, case .array(let frames)? = readyFields["observations"],
              frames.count == 1, case .object(let frame) = frames[0],
              case .number(800)? = frame["end_sample"], case .number(0.5)? = frame["rms"] else {
            return XCTFail("expected exactly one real observation")
        }
        XCTAssertEqual(readyFields.count, 1)
        XCTAssertEqual(frame.count, 2)
        let seal = try await router.handle(method: "media.audio.voice_capture_seal", params: .object([
            "capture_id": .string(id), "first_sample": .number(0), "end_sample": .number(800),
        ]))
        XCTAssertTrue(device.stopped)
        guard case .object(let sealFields) = seal, case .string(let encoded)? = sealFields["wav_base64"] else {
            return XCTFail("expected bounded private WAV")
        }
        XCTAssertEqual(sealFields.count, 1)
        XCTAssertEqual(Data(base64Encoded: encoded)?.count, 1_644)
    }
}

extension VoiceCaptureSessionTests {
    func testCameraClipFallsBackToVideoOnlyWhenVoiceOwnsMicrophone() async throws {
        let device = FakeVoiceDevice()
        let audio = AudioSession(requestVoicePermission: { $0(true) }, openVoiceDevice: { _, _ in device })
        let camera = CameraSession(audioSession: audio) {
            FakeCameraClipCapture(addAudio: {
                XCTFail("camera tried to attach audio while voice held the microphone")
                return false
            }, recordClip: { _ in "video-only.mov" })
        }
        let id = UUID()
        try await audio.voiceCaptureStart(id: id, clipLimit: 320_000, ingress: ContinuousClock().now)
        let clip = try await camera.clip(duration: 1, withAudio: true)
        XCTAssertEqual(clip.path, "video-only.mov")
        XCTAssertFalse(clip.hasAudio)
        XCTAssertFalse(device.stopped)
        try await audio.voiceCaptureCancel(id: id)
    }

    func testCameraClipHoldsMicrophoneThroughReentryAndCleanupOnRecordingFailure() async throws {
        let cleaningUp = expectation(description: "camera recording is cleaning up")
        let cleanup = CameraClipGate(entered: cleaningUp)
        let device = FakeVoiceDevice()
        let audio = AudioSession(requestVoicePermission: { $0(true) }, openVoiceDevice: { _, _ in
            device.didOpen()
            return device
        })
        let camera = CameraSession(audioSession: audio) {
            FakeCameraClipCapture(addAudio: { true }, recordClip: { duration in
                if duration == 1 {
                    await cleanup.wait()
                    throw FakeCameraClipFailure.recording
                }
                return "video-only.mov"
            })
        }
        let first = Task { try await camera.clip(duration: 1, withAudio: true) }
        await fulfillment(of: [cleaningUp], timeout: 2)

        do {
            let overlapping = try await camera.clip(duration: 2, withAudio: true)
            XCTAssertFalse(overlapping.hasAudio)
        } catch {
            XCTFail("overlapping camera clip failed instead of falling back: \(error)")
        }
        let blocked = UUID()
        do {
            try await audio.voiceCaptureStart(id: blocked, clipLimit: 320_000, ingress: ContinuousClock().now)
            XCTFail("voice acquired the microphone before camera cleanup completed")
            try await audio.voiceCaptureCancel(id: blocked)
        } catch let error as RpcError {
            XCTAssertTrue(error.message.contains("camera clip owns microphone"))
        } catch {
            XCTFail("unexpected busy result: \(error)")
        }
        XCTAssertEqual(device.opens, 0)

        await cleanup.release()
        do {
            _ = try await first.value
            XCTFail("camera swallowed the recording error")
        } catch FakeCameraClipFailure.recording {}
        let next = UUID()
        try await audio.voiceCaptureStart(id: next, clipLimit: 320_000, ingress: ContinuousClock().now)
        XCTAssertEqual(device.opens, 1)
        try await audio.voiceCaptureCancel(id: next)
    }

    func testCameraClipReportsAttachedAudioAndReleasesAfterSuccess() async throws {
        let device = FakeVoiceDevice()
        let audio = AudioSession(requestVoicePermission: { $0(true) }, openVoiceDevice: { _, _ in device })
        let camera = CameraSession(audioSession: audio) {
            FakeCameraClipCapture(addAudio: { true }, recordClip: { _ in "with-audio.mov" })
        }
        let clip = try await camera.clip(duration: 1, withAudio: true)
        XCTAssertEqual(clip.path, "with-audio.mov")
        XCTAssertTrue(clip.hasAudio)
        let id = UUID()
        try await audio.voiceCaptureStart(id: id, clipLimit: 320_000, ingress: ContinuousClock().now)
        try await audio.voiceCaptureCancel(id: id)
    }

    func testCameraClipWithUnavailableOrDisabledAudioDoesNotHoldMicrophoneWhileRecording() async throws {
        for withAudio in [true, false] {
            let recording = expectation(description: "video-only recording started (withAudio: \(withAudio))")
            let audioAttempt = withAudio ? expectation(description: "camera tried to attach audio") : nil
            let gate = CameraClipGate(entered: recording)
            let device = FakeVoiceDevice()
            let audio = AudioSession(requestVoicePermission: { $0(true) }, openVoiceDevice: { _, _ in device })
            let camera = CameraSession(audioSession: audio) {
                FakeCameraClipCapture(addAudio: {
                    XCTAssertTrue(withAudio, "camera tried to attach audio on a video-only request")
                    audioAttempt?.fulfill()
                    return false
                }, recordClip: { _ in
                    await gate.wait()
                    return "video-only.mov"
                })
            }
            let pending = Task { try await camera.clip(duration: 1, withAudio: withAudio) }
            await fulfillment(of: [recording] + (audioAttempt.map { [$0] } ?? []), timeout: 2)
            let id = UUID()
            do {
                try await audio.voiceCaptureStart(id: id, clipLimit: 320_000, ingress: ContinuousClock().now)
            } catch {
                XCTFail("video-only clip held the microphone: \(error)")
            }
            await gate.release()
            let clip = try await pending.value
            XCTAssertFalse(clip.hasAudio)
            let observations = try await audio.voiceCapturePoll(id: id, after: 0)
            XCTAssertEqual(observations, [])
            try await audio.voiceCaptureCancel(id: id)
        }
    }
}

private enum FakeCameraClipFailure: Error { case recording }

private struct FakeCameraClipCapture: CameraClipCapture {
    let addAudio: @Sendable () -> Bool
    let recordClip: @Sendable (Double) async throws -> String

    func addAudioInput() -> Bool { addAudio() }
    func record(duration: Double, isolation: isolated CameraSession) async throws -> String {
        try await recordClip(duration)
    }
}

private actor CameraClipGate {
    private let entered: XCTestExpectation
    private var waiter: CheckedContinuation<Void, Never>?
    private var released = false

    init(entered: XCTestExpectation) { self.entered = entered }

    func wait() async {
        entered.fulfill()
        guard !released else { return }
        await withCheckedContinuation { waiter = $0 }
    }

    func release() {
        released = true
        waiter?.resume()
        waiter = nil
    }
}

private final class PermissionAnswers: @unchecked Sendable {
    typealias Reply = @Sendable (Bool) -> Void
    private let lock = NSLock()
    private let firstRequest: XCTestExpectation
    private var reply: Reply?
    private var autoAllow = false

    init(firstRequest: XCTestExpectation) { self.firstRequest = firstRequest }

    func request(_ callback: @escaping Reply) {
        let allowed = lock.withLock {
            if autoAllow { return true }
            reply = callback
            return false
        }
        if allowed { callback(true) } else { firstRequest.fulfill() }
    }

    func takeFirst() -> Reply? { lock.withLock { reply } }
    func enableAutoAllow() { lock.withLock { autoAllow = true } }
}

private final class FakeVoiceDevice: VoiceCaptureDevice, @unchecked Sendable {
    private let lock = NSLock()
    private var buffer = VoiceCaptureBuffer(clipLimit: 320_000)
    private var openCount = 0
    private var isStopped = false

    var opens: Int { lock.withLock { openCount } }
    var stopped: Bool { lock.withLock { isStopped } }
    func didOpen() { lock.withLock { openCount += 1 } }
    func feedFrame() throws {
        try lock.withLock {
            try Array(repeating: Int16(16_384), count: 800).withUnsafeBufferPointer { try buffer.append($0) }
        }
    }
    func checkHealth() throws { try lock.withLock { try buffer.checkHealth() } }
    func poll(after cursor: Int) throws -> [VoiceCaptureObservation] {
        try lock.withLock { try buffer.poll(after: cursor) }
    }
    func seal(first: Int, end: Int) throws -> Data {
        stop()
        return try lock.withLock { try buffer.wav(first: first, end: end) }
    }
    func stop() { lock.withLock { isStopped = true } }
}
