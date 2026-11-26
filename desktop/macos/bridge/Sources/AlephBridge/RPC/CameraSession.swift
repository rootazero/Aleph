import AVFoundation
import AppKit
import Foundation

protocol CameraClipCapture {
    func addAudioInput() -> Bool
    // Returns or throws only after capture has stopped and its audio input is detached.
    func record(duration: Double, isolation: isolated CameraSession) async throws -> String
}

/// Isolates camera setup; each request keeps its own capture session across awaits.
actor CameraSession {
    private let audioSession: AudioSession
    private let makeClip: @Sendable () throws -> any CameraClipCapture

    init(
        audioSession: AudioSession,
        makeClip: @escaping @Sendable () throws -> any CameraClipCapture = { try AVCameraClipCapture() }
    ) {
        self.audioSession = audioSession
        self.makeClip = makeClip
    }

    func snap(quality: Float) async throws -> (jpeg: Data, width: Int, height: Int) {
        guard let device = AVCaptureDevice.default(for: .video) else {
            throw RpcError(code: -32003, message: "No camera device found", data: nil)
        }
        let input: AVCaptureDeviceInput
        do {
            input = try AVCaptureDeviceInput(device: device)
        } catch {
            throw RpcError(
                code: -32003,
                message: "Failed to create camera input: \(error)",
                data: nil
            )
        }

        let session = AVCaptureSession()
        session.beginConfiguration()
        guard session.canAddInput(input) else {
            session.commitConfiguration()
            throw RpcError(code: -32003, message: "Cannot add camera input to session", data: nil)
        }
        session.addInput(input)

        let output = AVCapturePhotoOutput()
        guard session.canAddOutput(output) else {
            session.commitConfiguration()
            throw RpcError(code: -32003, message: "Cannot add photo output to session", data: nil)
        }
        session.addOutput(output)
        session.commitConfiguration()
        session.startRunning()

        // Brief warm-up so auto-exposure settles before the shutter fires.
        try? await Task.sleep(nanoseconds: 500_000_000)

        let settings = AVCapturePhotoSettings()
        // AVFoundation does its own codec choice; bias via quality prioritization.
        settings.photoQualityPrioritization = quality > 0.7
            ? .quality
            : (quality > 0.4 ? .balanced : .speed)

        let delegate = PhotoCaptureDelegate()
        output.capturePhoto(with: settings, delegate: delegate)

        let jpeg: Data
        do {
            jpeg = try await delegate.waitForPhoto(timeout: 10)
        } catch {
            session.stopRunning()
            throw error
        }
        session.stopRunning()

        // Extract dimensions from the in-memory JPEG.
        var width = 0
        var height = 0
        if let image = NSImage(data: jpeg), let rep = image.representations.first {
            width = rep.pixelsWide
            height = rep.pixelsHigh
        }
        return (jpeg, width, height)
    }

    func clip(duration: Double, withAudio: Bool) async throws -> (path: String, duration: Double, hasAudio: Bool) {
        let capture = try makeClip()
        var lease: MicrophoneReservation.Ticket?
        if withAudio, let ticket = await audioSession.reserveCameraMicrophone() {
            if capture.addAudioInput() {
                lease = ticket
            } else {
                await audioSession.releaseCameraMicrophone(ticket)
            }
        }

        // Keep the exact ticket local across actor suspension and release only
        // after the hardware boundary has completed cleanup (including errors).
        let result: Result<String, Error>
        do {
            result = .success(try await capture.record(duration: duration, isolation: self))
        } catch {
            result = .failure(error)
        }
        if let lease { await audioSession.releaseCameraMicrophone(lease) }
        return (try result.get(), duration, lease != nil)
    }
}

private final class AVCameraClipCapture: CameraClipCapture {
    private let session = AVCaptureSession()
    private let movie = AVCaptureMovieFileOutput()
    private let dir = mediaDir()
    private var audioInput: AVCaptureDeviceInput?

    init() throws {
        guard let video = AVCaptureDevice.default(for: .video) else {
            throw RpcError(code: -32003, message: "No camera device found", data: nil)
        }
        let videoInput: AVCaptureDeviceInput
        do {
            videoInput = try AVCaptureDeviceInput(device: video)
        } catch {
            throw RpcError(
                code: -32003,
                message: "Failed to create video input: \(error)",
                data: nil
            )
        }

        session.beginConfiguration()
        guard session.canAddInput(videoInput) else {
            session.commitConfiguration()
            throw RpcError(code: -32003, message: "Cannot add video input", data: nil)
        }
        session.addInput(videoInput)

        guard session.canAddOutput(movie) else {
            session.commitConfiguration()
            throw RpcError(code: -32003, message: "Cannot add movie output", data: nil)
        }
        session.addOutput(movie)
        session.commitConfiguration()

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
    }

    func addAudioInput() -> Bool {
        guard let device = AVCaptureDevice.default(for: .audio),
              let input = try? AVCaptureDeviceInput(device: device) else { return false }

        session.beginConfiguration()
        defer { session.commitConfiguration() }
        guard session.canAddInput(input) else { return false }
        session.addInput(input)
        audioInput = input
        return true
    }

    func record(duration: Double, isolation: isolated CameraSession) async throws -> String {
        let url = dir.appendingPathComponent("camera_clip_\(timestampSuffix()).mov")

        let delegate = MovieRecordingDelegate()
        do {
            session.startRunning()
            defer {
                withExtendedLifetime(delegate) {
                    session.stopRunning()
                    if let audioInput {
                        session.beginConfiguration()
                        session.removeInput(audioInput)
                        session.commitConfiguration()
                        self.audioInput = nil
                    }
                }
            }
            // Brief warm-up for auto-exposure (mirrors legacy Rust 300ms sleep).
            try? await Task.sleep(nanoseconds: 300_000_000)
            movie.startRecording(to: url, recordingDelegate: delegate)

            try? await Task.sleep(nanoseconds: UInt64(duration * 1_000_000_000))
            movie.stopRecording()
            try await delegate.waitForFinish(timeout: duration + 10)
        }

        if !FileManager.default.fileExists(atPath: url.path) {
            throw RpcError(
                code: -32003,
                message: "Movie recording produced no output file",
                data: nil
            )
        }

        return url.path
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

// MARK: - AVCapture delegates

/// Captures a single photo and exposes it via an async `waitForPhoto(timeout:)`.
/// Thread-safety: the delegate callback arrives on an AVFoundation internal
/// queue, we guard state with a POSIX lock and hand off to a continuation.
final class PhotoCaptureDelegate: NSObject, AVCapturePhotoCaptureDelegate, @unchecked Sendable {
    private let lock = NSLock()
    private var result: Result<Data, RpcError>?
    private var continuation: CheckedContinuation<Data, Error>?
    private var timedOut = false

    func photoOutput(_ output: AVCapturePhotoOutput,
                     didFinishProcessingPhoto photo: AVCapturePhoto,
                     error: Error?) {
        lock.lock()
        if let error = error {
            result = .failure(RpcError(
                code: -32003,
                message: "Photo capture failed: \(error)",
                data: nil
            ))
        } else if let data = photo.fileDataRepresentation() {
            result = .success(data)
        } else {
            result = .failure(RpcError(
                code: -32003,
                message: "Photo capture returned no data",
                data: nil
            ))
        }
        let cont = continuation
        continuation = nil
        let r = result
        lock.unlock()
        if let cont, let r {
            Self.deliver(r, to: cont)
        }
    }

    func waitForPhoto(timeout: TimeInterval) async throws -> Data {
        try await withCheckedThrowingContinuation { cont in
            lock.lock()
            if let r = result {
                lock.unlock()
                Self.deliver(r, to: cont)
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
                        message: "Photo capture timed out",
                        data: nil
                    ))
                }
            }
        }
    }

    /// Synchronous scoped-locked helper — Swift 6 disallows `NSLock.lock/unlock`
    /// in async contexts, so we funnel the timeout path through a plain method.
    private func takePendingOnTimeout() -> CheckedContinuation<Data, Error>? {
        lock.lock()
        defer { lock.unlock() }
        let pending = continuation
        if pending != nil {
            continuation = nil
            timedOut = true
        }
        return pending
    }

    private static func deliver(
        _ r: Result<Data, RpcError>,
        to cont: CheckedContinuation<Data, Error>
    ) {
        switch r {
        case .success(let d): cont.resume(returning: d)
        case .failure(let e): cont.resume(throwing: e)
        }
    }
}

/// Waits for an `AVCaptureMovieFileOutput` recording to finish.
final class MovieRecordingDelegate: NSObject, AVCaptureFileOutputRecordingDelegate, @unchecked Sendable {
    private let lock = NSLock()
    private var finished = false
    private var failure: RpcError?
    private var continuation: CheckedContinuation<Void, Error>?

    func fileOutput(_ output: AVCaptureFileOutput,
                    didFinishRecordingTo outputFileURL: URL,
                    from connections: [AVCaptureConnection],
                    error: Error?) {
        lock.lock()
        finished = true
        // AVFoundation reports "stopped normally" via error code -11806.
        // Treat that as success so the clip is returned to the caller.
        if let error = error, (error as NSError).code != -11806 {
            failure = RpcError(
                code: -32003,
                message: "Movie recording failed: \(error)",
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
                        message: "Movie recording timed out",
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
