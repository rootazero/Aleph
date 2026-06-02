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

    /// Begin an open-ended push-to-talk recording. Triggers the native
    /// microphone TCC prompt on first use (direct AVFoundation access works on
    /// unsigned/ad-hoc builds, unlike WKWebView `getUserMedia`). Replaces any
    /// recording already in progress.
    func recordStart() async throws {
        // No input device at all (e.g. a Mac mini with no built-in mic and none
        // attached). Detect this BEFORE prompting for permission so we surface a
        // distinct "no microphone" signal the core can turn into a clear message,
        // rather than a cryptic `record() failed`. The token is matched on the
        // Rust side (`handle_record_start`).
        guard hasAudioInputDevice() else {
            throw RpcError(
                code: -32004,
                message: "audio.record_start: NO_AUDIO_INPUT_DEVICE",
                data: nil
            )
        }

        // Proactively request mic access so the system prompt appears up front
        // and a denial surfaces as a clean error rather than an empty file.
        let granted = await AVCaptureDevice.requestAccess(for: .audio)
        guard granted else {
            throw RpcError(
                code: -32004,
                message: "audio.record_start: microphone permission denied",
                data: nil
            )
        }

        // Reentrancy guard. `await requestAccess` above is a suspension point:
        // while the TCC dialog is up the actor is free, so a duplicate
        // `record_start` (e.g. a second click before the UI left Idle) can race
        // in. If a recording is already live, return success WITHOUT touching
        // its AudioQueue — calling `stop()` here while the queue's input
        // callback is in flight is a use-after-free that crashes the helper
        // (AudioRecorderAQInputCallback → objc_loadWeak EXC_BAD_ACCESS).
        if let existing = activeRecorder {
            if existing.isRecording {
                return
            }
            // Stale, no longer recording — finalise it safely (let the queue
            // drain via the delegate) before starting a fresh recording.
            existing.stop()
            if let delegate = activeDelegate {
                try? await delegate.waitForFinish(timeout: 2.0)
            }
            activeRecorder = nil
            activeURL = nil
            activeDelegate = nil
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
            try? await Task.sleep(nanoseconds: 250_000_000)
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
    }

    /// Stop the active push-to-talk recording and return the captured file.
    func recordStop() async throws -> (filePath: String, duration: Double, format: String) {
        guard let recorder = activeRecorder, let url = activeURL else {
            throw RpcError(
                code: -32004,
                message: "audio.record_stop: no active recording",
                data: nil
            )
        }
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
