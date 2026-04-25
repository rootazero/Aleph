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
