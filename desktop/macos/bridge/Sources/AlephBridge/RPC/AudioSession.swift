import AVFoundation
import Foundation

/// Internal device descriptor for the list_devices handler. Kept file-local so
/// the actor API does not leak `JSONValue` into its public surface.
struct AudioDeviceInfo {
    let uid: String
    let name: String
    let isInput: Bool
    let isDefault: Bool
}

/// Serializes microphone access. macOS permits concurrent `AVAudioRecorder`
/// instances in principle but the hardware input path is single-tenant in
/// practice; we gate record() calls through an actor to match camera.
actor AudioSession {
    typealias RequestVoicePermission = @Sendable (@escaping @Sendable (Bool) -> Void) -> Void
    typealias OpenVoiceDevice = @Sendable (Int, @escaping @Sendable () -> Void) throws -> any VoiceCaptureDevice

    private struct VoiceLease {
        let id: UUID
        let ticket: MicrophoneReservation.Ticket
        let deadline: ContinuousClock.Instant
        let cancellation: VoiceCaptureStartCancellation
        let input: Server.InputLifetime?
        var startup: CheckedContinuation<Void, Error>?
        var watchdog: Task<Void, Never>?
        var device: (any VoiceCaptureDevice)?
    }

    private let requestVoicePermission: RequestVoicePermission
    private let openVoiceDevice: OpenVoiceDevice
    private var microphone = MicrophoneReservation()
    private var voiceAdmission = VoiceCaptureAdmission()
    private var voiceLease: VoiceLease?

    init(
        requestVoicePermission: @escaping RequestVoicePermission = { reply in
            AVCaptureDevice.requestAccess(for: .audio, completionHandler: reply)
        },
        openVoiceDevice: @escaping OpenVoiceDevice = { limit, onFailure in
            try EngineVoiceCaptureDevice(clipLimit: limit, onFailure: onFailure)
        }
    ) {
        self.requestVoicePermission = requestVoicePermission
        self.openVoiceDevice = openVoiceDevice
    }

    func reserveCameraMicrophone() -> MicrophoneReservation.Ticket? {
        microphone.reserve(.camera)
    }

    func releaseCameraMicrophone(_ ticket: MicrophoneReservation.Ticket) {
        guard ticket.kind == .camera else { return }
        microphone.release(ticket)
    }

    private func microphoneBusy() -> RpcError {
        RpcError(code: -32004, message: "audio: microphone is already reserved", data: nil)
    }

    private func voiceError(_ message: String) -> RpcError {
        RpcError(code: -32004, message: "media.audio.voice_capture: \(message)", data: nil)
    }

    func voiceCaptureStart(
        id: UUID, clipLimit: Int, ingress: ContinuousClock.Instant, input: Server.InputLifetime? = nil
    ) async throws {
        guard input?.isClosed != true, voiceAdmission.allowsStart(id, ingress: ingress, now: ContinuousClock().now) else {
            throw voiceError("start expired, cancelled or helper generation retired")
        }
        try Task.checkCancellation()
        guard let ticket = microphone.reserve(.voice) else {
            if let owner = microphone.current, owner.kind != .voice {
                // This request cannot have reached either permission or device open.
                let ownerName = owner.kind == .camera ? "camera clip" : "legacy recording"
                throw RpcError(code: -32004, message: "audio: \(ownerName) owns microphone",
                               data: .object(["reason": .string("voice_capture_legacy_busy")]))
            }
            throw microphoneBusy()
        }
        let deadline = ingress.advanced(by: VoiceCaptureAdmission.leaseBudget)
        let cancellation = VoiceCaptureStartCancellation()
        voiceLease = VoiceLease(id: id, ticket: ticket, deadline: deadline, cancellation: cancellation, input: input)

        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (startup: CheckedContinuation<Void, Error>) in
                guard voiceLease?.ticket == ticket else {
                    startup.resume(throwing: CancellationError())
                    return
                }
                voiceLease?.startup = startup
                guard !Task.isCancelled else {
                    releaseVoice(ticket, failing: CancellationError())
                    return
                }
                voiceLease?.watchdog = Task { [weak self] in
                    while !Task.isCancelled {
                        let next = min(ContinuousClock().now.advanced(by: .milliseconds(200)), deadline)
                        do { try await Task.sleep(until: next, clock: ContinuousClock()) } catch { return }
                        guard await self?.checkVoiceLease(ticket) == true else { return }
                    }
                }
                requestVoicePermission { [weak self] granted in
                    Task { await self?.voicePermissionResolved(ticket, granted: granted, clipLimit: clipLimit) }
                }
            }
        } onCancel: {
            cancellation.cancel()
            Task { await self.releaseVoice(ticket, failing: CancellationError()) }
        }
    }

    private func voicePermissionResolved(_ ticket: MicrophoneReservation.Ticket, granted: Bool, clipLimit: Int) {
        guard let lease = voiceLease, lease.ticket == ticket, let startup = lease.startup else { return }
        guard granted, lease.input?.isClosed != true, !lease.cancellation.isCancelled,
              ContinuousClock().now < lease.deadline else {
            releaseVoice(ticket, failing: voiceError("permission denied, cancelled or lease expired"))
            return
        }
        do {
            voiceLease?.device = try openVoiceDevice(clipLimit) { [weak self] in
                Task { await self?.captureDeviceFailed(ticket) }
            }
            guard lease.input?.isClosed != true, !lease.cancellation.isCancelled,
                  ContinuousClock().now < lease.deadline else {
                releaseVoice(ticket, failing: voiceError("cancelled or lease expired while opening device"))
                return
            }
            voiceLease?.startup = nil
            startup.resume()
        } catch {
            releaseVoice(ticket, failing: error)
        }
    }

    private func captureDeviceFailed(_ ticket: MicrophoneReservation.Ticket) {
        releaseVoice(ticket, failing: voiceError("device, samples or sample clock lost"))
    }

    private func checkVoiceLease(_ ticket: MicrophoneReservation.Ticket) -> Bool {
        guard let lease = voiceLease, lease.ticket == ticket else { return false }
        do {
            guard lease.input?.isClosed != true, !lease.cancellation.isCancelled,
                  ContinuousClock().now < lease.deadline else { throw voiceError("lease closed or expired") }
            try lease.device?.checkHealth()
            return true
        } catch {
            releaseVoice(ticket, failing: error)
            return false
        }
    }

    private func releaseVoice(_ ticket: MicrophoneReservation.Ticket, failing error: (any Error)? = nil) {
        guard let lease = voiceLease, lease.ticket == ticket else { return }
        lease.device?.stop()
        lease.watchdog?.cancel()
        voiceLease = nil
        voiceAdmission.finished(lease.id)
        microphone.release(ticket)
        if let startup = lease.startup {
            startup.resume(throwing: error ?? CancellationError())
        }
    }

    func voiceCapturePoll(id: UUID, after cursor: Int) throws -> [VoiceCaptureObservation] {
        guard let lease = voiceLease, lease.id == id, let device = lease.device,
              checkVoiceLease(lease.ticket) else { throw voiceError("capture not active") }
        do {
            return try device.poll(after: cursor)
        } catch {
            releaseVoice(lease.ticket, failing: error)
            throw error
        }
    }

    func voiceCaptureSeal(id: UUID, first: Int, end: Int) throws -> Data {
        guard let lease = voiceLease, lease.id == id, let device = lease.device,
              checkVoiceLease(lease.ticket) else { throw voiceError("capture not active") }
        defer { releaseVoice(lease.ticket) }
        let wav = try device.seal(first: first, end: end)
        guard ContinuousClock().now < lease.deadline else { throw voiceError("lease expired") }
        return wav
    }

    func voiceCaptureCancel(id: UUID) throws {
        if let lease = voiceLease, lease.id == id {
            releaseVoice(lease.ticket)
        } else if !voiceAdmission.cancelUnknown(id) {
            throw voiceError("unmatched cancel history saturated; retire helper generation")
        }
    }

    func closeVoiceCaptureInput() {
        voiceAdmission.retire()
        if let lease = voiceLease { releaseVoice(lease.ticket) }
    }

    /// Active push-to-talk recorder, held between `recordStart` and `recordStop`.
    /// The delegate is retained too so its continuation/encode callbacks fire.
    private var activeRecorder: AVAudioRecorder?
    private var activeURL: URL?
    private var activeDelegate: AudioRecordingDelegate?

    /// Whether the host has any audio input device (built-in or attached).
    /// A Mac mini with no microphone returns false — used to fail
    /// `recordStart` early with a clear "no microphone" signal.
    private func hasAudioInputDevice() -> Bool {
        if AVCaptureDevice.default(for: .audio) != nil {
            return true
        }
        let discovery = AVCaptureDevice.DiscoverySession(
            deviceTypes: [.builtInMicrophone, .externalUnknown],
            mediaType: .audio,
            position: .unspecified
        )
        return !discovery.devices.isEmpty
    }

    func listDevices() async throws -> [AudioDeviceInfo] {
        do {
            let discovery = AVCaptureDevice.DiscoverySession(
                deviceTypes: [.builtInMicrophone, .externalUnknown],
                mediaType: .audio,
                position: .unspecified
            )
            let defaultUID = AVCaptureDevice.default(for: .audio)?.uniqueID
            return discovery.devices.map { device in
                AudioDeviceInfo(
                    uid: device.uniqueID,
                    name: device.localizedName,
                    isInput: true,
                    isDefault: device.uniqueID == defaultUID
                )
            }
        }
    }

    func record(durationSecs: Double) async throws -> (filePath: String, duration: Double, format: String) {
        guard let ticket = microphone.reserve(.timed) else { throw microphoneBusy() }
        defer { microphone.release(ticket) }
        let dir = mediaDir()
        do {
            try FileManager.default.createDirectory(
                at: dir, withIntermediateDirectories: true
            )
        } catch {
            throw RpcError(
                code: -32003,
                message: "Cannot create media directory \(dir.path): \(error)",
                data: nil
            )
        }
        let url = dir.appendingPathComponent("audio_record_\(timestampSuffix()).m4a")

        let settings: [String: Any] = [
            AVFormatIDKey: kAudioFormatMPEG4AAC,
            AVSampleRateKey: 44100,
            AVNumberOfChannelsKey: 1,
            AVEncoderAudioQualityKey: AVAudioQuality.medium.rawValue,
        ]

        let recorder: AVAudioRecorder
        do {
            recorder = try AVAudioRecorder(url: url, settings: settings)
        } catch {
            throw RpcError(
                code: -32003,
                message: "audio.record: failed to initialise recorder: \(error)",
                data: nil
            )
        }

        let delegate = AudioRecordingDelegate()
        recorder.delegate = delegate

        guard recorder.prepareToRecord() else {
            throw RpcError(
                code: -32004,
                message: "audio.record: prepareToRecord failed (permission denied or microphone in use)",
                data: nil
            )
        }

        guard recorder.record(forDuration: durationSecs) else {
            throw RpcError(
                code: -32004,
                message: "audio.record: record() failed (permission denied or microphone in use)",
                data: nil
            )
        }

        do {
            try await delegate.waitForFinish(timeout: durationSecs + 5.0)
        } catch {
            recorder.stop()
            throw error
        }

        if !FileManager.default.fileExists(atPath: url.path) {
            throw RpcError(
                code: -32003,
                message: "audio.record: recorder produced no output file",
                data: nil
            )
        }

        return (url.path, durationSecs, "m4a")
    }

    /// Begin an open-ended push-to-talk recording. A duplicate is idempotent
    /// only after the existing AudioQueue has actually started.
    func recordStart() async throws {
        if activeRecorder?.isRecording == true { return }
        let existing = activeRecorder
        let ticket: MicrophoneReservation.Ticket
        if existing != nil, let current = microphone.current, current.kind == .open {
            ticket = current
        } else {
            guard let reserved = microphone.reserve(.open) else { throw microphoneBusy() }
            ticket = reserved
        }
        var startedRecording = false
        defer { if !startedRecording { microphone.release(ticket) } }

        if let existing {
            // Clear before the await so recordStop cannot wait on the same
            // encoder continuation; the reservation still excludes new input.
            let delegate = activeDelegate
            activeRecorder = nil
            activeURL = nil
            activeDelegate = nil
            existing.stop()
            if let delegate { try? await delegate.waitForFinish(timeout: 2.0) }
        }

        // Preserve the no-device token expected by the Rust legacy caller.
        guard hasAudioInputDevice() else {
            throw RpcError(
                code: -32004,
                message: "audio.record_start: NO_AUDIO_INPUT_DEVICE",
                data: nil
            )
        }
        let granted = await AVCaptureDevice.requestAccess(for: .audio)
        try Task.checkCancellation()
        guard granted else {
            throw RpcError(
                code: -32004,
                message: "audio.record_start: microphone permission denied",
                data: nil
            )
        }

        let dir = mediaDir()
        do {
            try FileManager.default.createDirectory(
                at: dir, withIntermediateDirectories: true
            )
        } catch {
            throw RpcError(
                code: -32003,
                message: "audio.record_start: cannot create media directory \(dir.path): \(error)",
                data: nil
            )
        }
        let url = dir.appendingPathComponent("audio_record_\(timestampSuffix()).m4a")

        let settings: [String: Any] = [
            AVFormatIDKey: kAudioFormatMPEG4AAC,
            AVSampleRateKey: 44100,
            AVNumberOfChannelsKey: 1,
            AVEncoderAudioQualityKey: AVAudioQuality.medium.rawValue,
        ]

        let recorder: AVAudioRecorder
        do {
            recorder = try AVAudioRecorder(url: url, settings: settings)
        } catch {
            throw RpcError(
                code: -32003,
                message: "audio.record_start: failed to initialise recorder: \(error)",
                data: nil
            )
        }

        let delegate = AudioRecordingDelegate()
        recorder.delegate = delegate

        guard recorder.prepareToRecord() else {
            throw RpcError(
                code: -32004,
                message: "audio.record_start: prepareToRecord failed (permission denied or microphone in use)",
                data: nil
            )
        }

        // `record()` starts the CoreAudio input queue, which can transiently
        // fail to acquire the device — e.g. a just-released reservation that
        // coreaudiod hasn't fully reaped, or another client mid-handoff. TCC
        // permission is already confirmed above, so a failure here is device
        // contention, not denial: retry a few times with short backoff before
        // surfacing it.
        var started = recorder.record()
        var attempt = 0
        while !started, attempt < 4 {
            attempt += 1
            try await Task.sleep(for: .milliseconds(250))
            started = recorder.record()
        }
        guard started else {
            throw RpcError(
                code: -32004,
                message: "audio.record_start: record() failed after \(attempt) retries (microphone in use or unavailable)",
                data: nil
            )
        }

        activeRecorder = recorder
        activeURL = url
        activeDelegate = delegate
        startedRecording = true
    }

    /// Stop the active push-to-talk recording and return the captured file.
    func recordStop() async throws -> (filePath: String, duration: Double, format: String) {
        guard let recorder = activeRecorder, let url = activeURL,
              let ticket = microphone.current, ticket.kind == .open else {
            throw RpcError(
                code: -32004,
                message: "audio.record_stop: no active recording",
                data: nil
            )
        }
        defer { microphone.release(ticket) }
        // `currentTime` resets to 0 on stop(), so capture elapsed time first.
        let elapsed = recorder.currentTime
        let delegate = activeDelegate
        recorder.stop()
        activeRecorder = nil
        activeURL = nil
        activeDelegate = nil

        // Let the encoder finalise the file (delegate fires didFinishRecording).
        if let delegate {
            try? await delegate.waitForFinish(timeout: 5.0)
        }

        if !FileManager.default.fileExists(atPath: url.path) {
            throw RpcError(
                code: -32003,
                message: "audio.record_stop: recorder produced no output file",
                data: nil
            )
        }
        return (url.path, elapsed, "m4a")
    }
}

/// $HOME/.aleph/data/_media — mirrors `desktop/macos/src/media.rs::media_dir`.
private func mediaDir() -> URL {
    let home = FileManager.default.homeDirectoryForCurrentUser
    return home
        .appendingPathComponent(".aleph")
        .appendingPathComponent("data")
        .appendingPathComponent("_media")
}

private func timestampSuffix() -> String {
    let ms = Int(Date().timeIntervalSince1970 * 1000)
    return String(ms)
}

// MARK: - AVAudioRecorder delegate

/// Waits for an `AVAudioRecorder` recording to finish, mirroring the lock +
/// continuation pattern used by `MovieRecordingDelegate` in `CameraSession`.
final class AudioRecordingDelegate: NSObject, AVAudioRecorderDelegate, @unchecked Sendable {
    private let lock = NSLock()
    private var finished = false
    private var failure: RpcError?
    private var continuation: CheckedContinuation<Void, Error>?

    func audioRecorderDidFinishRecording(_ recorder: AVAudioRecorder, successfully flag: Bool) {
        lock.lock()
        finished = true
        if !flag {
            failure = RpcError(
                code: -32003,
                message: "audio.record: recording did not finish successfully",
                data: nil
            )
        }
        let cont = continuation
        continuation = nil
        let err = failure
        lock.unlock()
        if let cont {
            if let err {
                cont.resume(throwing: err)
            } else {
                cont.resume(returning: ())
            }
        }
    }

    func audioRecorderEncodeErrorDidOccur(_ recorder: AVAudioRecorder, error: Error?) {
        lock.lock()
        finished = true
        failure = RpcError(
            code: -32003,
            message: "audio.record: encode error: \(error.map { "\($0)" } ?? "unknown")",
            data: nil
        )
        let cont = continuation
        continuation = nil
        let err = failure
        lock.unlock()
        if let cont, let err {
            cont.resume(throwing: err)
        }
    }

    func waitForFinish(timeout: TimeInterval) async throws {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            lock.lock()
            if finished {
                let err = failure
                lock.unlock()
                if let err {
                    cont.resume(throwing: err)
                } else {
                    cont.resume(returning: ())
                }
                return
            }
            continuation = cont
            lock.unlock()
            Task {
                try? await Task.sleep(nanoseconds: UInt64(timeout * 1_000_000_000))
                let pending = self.takePendingOnTimeout()
                if let pending {
                    pending.resume(throwing: RpcError(
                        code: -32003,
                        message: "audio.record: recorder timed out",
                        data: nil
                    ))
                }
            }
        }
    }

    private func takePendingOnTimeout() -> CheckedContinuation<Void, Error>? {
        lock.lock()
        defer { lock.unlock() }
        let pending = continuation
        continuation = nil
        return pending
    }
}
