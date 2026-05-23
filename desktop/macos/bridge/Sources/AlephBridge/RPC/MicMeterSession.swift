import AVFoundation
import Foundation

/// Long-lived microphone meter session.
///
/// Lazily installs an `AVAudioEngine` tap on the default input on first poll
/// and keeps it running across calls (so the orange "mic in use" indicator
/// stays on while the caller is interested). The tap callback updates an
/// EMA-smoothed RMS level which `poll()` returns synchronously.
///
/// The session shuts itself down when no poll arrives for
/// `idleTimeoutSeconds` — protects against a daemon-side crash leaving the
/// mic indicator on indefinitely.
actor MicMeterSession {
    /// Idle timeout. Must mirror `MIC_METER_IDLE_TIMEOUT_SECS` in
    /// `shared/protocol/src/desktop_bridge/methods/media.rs`.
    private let idleTimeoutSeconds: TimeInterval = 60

    /// Smoothing factor for the EMA. 0.3 ≈ ~3 sample half-life — responsive
    /// enough for voice detection without flicker on quiet samples.
    private let smoothing: Float = 0.3

    private var engine: AVAudioEngine?
    private var lastError: String?
    private let levelStore = MicMeterLevelStore()
    private var lastPolledAt: Date = .distantPast
    private var idleSweepStarted = false

    /// Poll the current level. Starts the tap on first call, then returns the
    /// most recent EMA-smoothed RMS sample.
    func poll() async -> MicMeterPollResult {
        lastPolledAt = Date()
        startIdleSweepIfNeeded()

        if engine == nil {
            if let err = startEngine() {
                lastError = err
                return .init(level: nil, active: false, reason: err)
            }
            lastError = nil
        }

        let level = await levelStore.current()
        return .init(level: level, active: true, reason: nil)
    }

    /// Stop the tap and release the engine. Safe to call repeatedly.
    func stop() {
        if let eng = engine {
            eng.inputNode.removeTap(onBus: 0)
            eng.stop()
            engine = nil
        }
    }

    // MARK: - private

    private func startEngine() -> String? {
        let eng = AVAudioEngine()
        let input = eng.inputNode
        let format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0, format.channelCount > 0 else {
            return "no input device"
        }

        let store = self.levelStore
        let smoothing = self.smoothing
        input.installTap(onBus: 0, bufferSize: 1024, format: format) { buffer, _ in
            guard let rms = computeRms(buffer: buffer) else { return }
            Task { await store.push(sample: rms, smoothing: smoothing) }
        }

        eng.prepare()
        do {
            try eng.start()
        } catch {
            input.removeTap(onBus: 0)
            return "audio engine start failed: \(error)"
        }
        engine = eng
        return nil
    }

    private func startIdleSweepIfNeeded() {
        if idleSweepStarted { return }
        idleSweepStarted = true
        Task { [weak self] in
            while let self {
                try? await Task.sleep(nanoseconds: UInt64(5 * 1_000_000_000))
                await self.sweepIdle()
            }
        }
    }

    private func sweepIdle() {
        guard engine != nil else { return }
        if Date().timeIntervalSince(lastPolledAt) >= idleTimeoutSeconds {
            stop()
        }
    }
}

/// Internal carrier — mirrors `MicMeterResult` on the wire.
struct MicMeterPollResult {
    let level: Float?
    let active: Bool
    let reason: String?
}

/// Thread-safe holder for the EMA-smoothed RMS level. Tap callback writes;
/// `poll()` reads.
actor MicMeterLevelStore {
    private var ema: Float = 0.0
    private var samples: UInt = 0

    func push(sample: Float, smoothing: Float) {
        if samples == 0 {
            ema = sample
        } else {
            ema = ema + smoothing * (sample - ema)
        }
        samples &+= 1
    }

    func current() -> Float? {
        samples == 0 ? nil : min(max(ema, 0.0), 1.0)
    }
}

/// RMS of a single audio buffer, normalised to 0.0..1.0 (assumes Float32 PCM,
/// which `AVAudioEngine.inputNode` produces by default on macOS).
func computeRms(buffer: AVAudioPCMBuffer) -> Float? {
    guard let data = buffer.floatChannelData else { return nil }
    let frameLength = Int(buffer.frameLength)
    if frameLength == 0 { return nil }
    let channelCount = Int(buffer.format.channelCount)
    var sumSquares: Float = 0
    var totalFrames: Int = 0
    for ch in 0..<channelCount {
        let ptr = data[ch]
        for i in 0..<frameLength {
            let v = ptr[i]
            sumSquares += v * v
        }
        totalFrames += frameLength
    }
    if totalFrames == 0 { return nil }
    let mean = sumSquares / Float(totalFrames)
    return mean.squareRoot()
}
