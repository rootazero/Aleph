import AVFoundation
import Foundation

protocol VoiceCaptureDevice: AnyObject {
    func checkHealth() throws
    func poll(after cursor: Int) throws -> [VoiceCaptureObservation]
    func seal(first: Int, end: Int) throws -> Data
    func stop()
}

// Engine ownership stays on AudioSession. The tap converts and commits under
// one lock, without enqueuing tasks or retaining input buffers across callbacks.
final class EngineVoiceCaptureDevice: VoiceCaptureDevice, @unchecked Sendable {
    private var engine: AVAudioEngine?
    private var configurationObserver: NSObjectProtocol?
    private let source: Source

    init(clipLimit: Int, onFailure: @escaping @Sendable () -> Void) throws {
        let engine = AVAudioEngine()
        let input = engine.inputNode
        let inputFormat = input.outputFormat(forBus: 0)
        guard (8_000...192_000).contains(inputFormat.sampleRate),
              (1...32).contains(inputFormat.channelCount),
              let format = AVAudioFormat(commonFormat: .pcmFormatInt16,
                                         sampleRate: Double(VoiceCaptureBuffer.sampleRate),
                                         channels: 1, interleaved: false),
              let converter = AVAudioConverter(from: inputFormat, to: format) else {
            throw VoiceCaptureBuffer.Failure.invalidFormat
        }
        converter.primeMethod = .none
        converter.downmix = true
        let source = Source(clipLimit: clipLimit, converter: converter, onFailure: onFailure)
        self.source = source
        self.engine = engine
        configurationObserver = NotificationCenter.default.addObserver(
            forName: .AVAudioEngineConfigurationChange, object: engine, queue: nil
        ) { _ in source.invalidate(.deviceLoss) }
        input.installTap(onBus: 0, bufferSize: 1_024, format: inputFormat) { buffer, time in
            source.receive(buffer, time: time)
        }
        do {
            try engine.start()
        } catch {
            stop()
            throw error
        }
    }

    func checkHealth() throws {
        guard engine?.isRunning == true else { throw VoiceCaptureBuffer.Failure.deviceLoss }
        try source.checkHealth()
    }

    func poll(after cursor: Int) throws -> [VoiceCaptureObservation] {
        try checkHealth()
        return try source.poll(after: cursor)
    }

    func seal(first: Int, end: Int) throws -> Data {
        guard engine?.isRunning == true else { throw VoiceCaptureBuffer.Failure.deviceLoss }
        try source.closeForSeal()
        stop()
        return try source.wav(first: first, end: end)
    }

    func stop() {
        // Closing under Source's lock waits for any in-flight conversion and
        // prevents a callback racing the tap removal from touching the clip.
        source.close()
        if let configurationObserver {
            NotificationCenter.default.removeObserver(configurationObserver)
            self.configurationObserver = nil
        }
        if let engine {
            engine.stop()
            engine.inputNode.removeTap(onBus: 0)
            self.engine = nil
        }
    }

    private final class Source: @unchecked Sendable {
        private let lock = NSLock()
        private let converter: AVAudioConverter
        private let onFailure: @Sendable () -> Void
        private var buffer: VoiceCaptureBuffer
        private var accepting = true
        private var nextInputSample: AVAudioFramePosition?
        private var lastOutput = ContinuousClock().now

        init(clipLimit: Int, converter: AVAudioConverter, onFailure: @escaping @Sendable () -> Void) {
            buffer = VoiceCaptureBuffer(clipLimit: clipLimit)
            self.converter = converter
            self.onFailure = onFailure
        }

        func receive(_ input: AVAudioPCMBuffer, time: AVAudioTime) {
            let failed = lock.withLock {
                guard accepting, buffer.failure == nil else { return false }
                do {
                    try convert(input, time: time)
                    return false
                } catch {
                    buffer.invalidate(error as? VoiceCaptureBuffer.Failure ?? .invalidFormat)
                    return true
                }
            }
            if failed { onFailure() }
        }

        private func convert(_ input: AVAudioPCMBuffer, time: AVAudioTime) throws {
            let frames = input.frameLength
            guard input.format.isEqual(converter.inputFormat), (1...16_384).contains(frames) else {
                throw VoiceCaptureBuffer.Failure.invalidFormat
            }
            guard time.isSampleTimeValid, time.sampleRate == input.format.sampleRate,
                  nextInputSample.map({ $0 == time.sampleTime }) ?? true else {
                throw VoiceCaptureBuffer.Failure.clockLoss
            }
            let (next, overflow) = time.sampleTime.addingReportingOverflow(Int64(frames))
            guard !overflow else { throw VoiceCaptureBuffer.Failure.clockLoss }
            nextInputSample = next

            let estimate = ceil(Double(frames) * converter.outputFormat.sampleRate / input.format.sampleRate)
            let capacity = AVAudioFrameCount(estimate) + 1_024
            guard let output = AVAudioPCMBuffer(pcmFormat: converter.outputFormat, frameCapacity: capacity) else {
                throw VoiceCaptureBuffer.Failure.invalidFormat
            }
            var supplied = false
            var conversionError: NSError?
            let status = converter.convert(to: output, error: &conversionError) { _, inputStatus in
                if supplied {
                    inputStatus.pointee = .noDataNow
                    return nil
                }
                supplied = true
                inputStatus.pointee = .haveData
                return input
            }
            guard supplied, conversionError == nil, status == .haveData || status == .inputRanDry,
                  output.frameLength < capacity, let samples = output.int16ChannelData?[0] else {
                throw VoiceCaptureBuffer.Failure.invalidFormat
            }
            if output.frameLength != 0 {
                try buffer.append(UnsafeBufferPointer(start: samples, count: Int(output.frameLength)))
                lastOutput = ContinuousClock().now
            }
        }

        func invalidate(_ reason: VoiceCaptureBuffer.Failure) {
            let failed = lock.withLock {
                guard accepting, buffer.failure == nil else { return false }
                buffer.invalidate(reason)
                return true
            }
            if failed { onFailure() }
        }

        private func checkHealthLocked() throws {
            try buffer.checkHealth()
            guard lastOutput.duration(to: ContinuousClock().now) < .seconds(2) else {
                buffer.invalidate(.deviceLoss)
                throw VoiceCaptureBuffer.Failure.deviceLoss
            }
        }

        func checkHealth() throws { try lock.withLock { try checkHealthLocked() } }
        func close() { lock.withLock { accepting = false } }
        func closeForSeal() throws {
            try lock.withLock {
                try checkHealthLocked()
                accepting = false
            }
        }
        func poll(after cursor: Int) throws -> [VoiceCaptureObservation] {
            try lock.withLock { try buffer.poll(after: cursor) }
        }
        func wav(first: Int, end: Int) throws -> Data {
            try lock.withLock { try buffer.wav(first: first, end: end) }
        }
    }
}
