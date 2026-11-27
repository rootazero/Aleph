import Foundation

struct VoiceCaptureObservation: Codable, Equatable, Sendable {
    let endSample: Int
    let rms: Double

    enum CodingKeys: String, CodingKey {
        case endSample = "end_sample"
        case rms
    }
}

// Start-relative offsets never wrap. Only history older than the last
// acknowledged cursor's possible clip may be overwritten.
struct VoiceCaptureBuffer {
    static let sampleRate = 16_000
    static let frameSamples = 800
    static let maxClipSamples = 320_000
    static let batchSize = 16
    static let backlogLimit = batchSize * 2
    static let ringHeadroom = 12_800
    static let streamLimit = 720_000

    enum Failure: Error, Equatable {
        case sampleOverflow, observationOverflow, invalidCursor, invalidRange
        case invalidFormat, clockLoss, deviceLoss
    }

    private let clipLimit: Int
    private var samples: [Int16]
    private var sampleCount = 0
    private var pending: [VoiceCaptureObservation] = []
    private var acknowledged = 0
    private var delivered = 0
    private var squareSum = 0.0
    private(set) var failure: Failure?

    init(clipLimit: Int) {
        precondition((1...Self.maxClipSamples).contains(clipLimit))
        self.clipLimit = clipLimit
        samples = Array(repeating: 0, count: clipLimit + Self.ringHeadroom)
    }

    mutating func invalidate(_ reason: Failure) {
        if failure == nil { failure = reason }
    }

    mutating func append(_ input: UnsafeBufferPointer<Int16>) throws {
        try checkHealth()
        guard input.count <= Self.streamLimit - sampleCount else {
            invalidate(.sampleOverflow)
            throw Failure.sampleOverflow
        }
        let frames = (sampleCount % Self.frameSamples + input.count) / Self.frameSamples
        if frames > Self.backlogLimit - pending.count { invalidate(.observationOverflow) }
        let overwritten = max(0, sampleCount + input.count - samples.count)
        if overwritten > max(0, acknowledged - clipLimit) { invalidate(.sampleOverflow) }
        try checkHealth()
        for sample in input {
            samples[sampleCount % samples.count] = sample
            sampleCount += 1
            let scaled = Double(sample) / 32_768
            squareSum += scaled * scaled
            if sampleCount.isMultiple(of: Self.frameSamples) {
                pending.append(.init(endSample: sampleCount, rms: sqrt(squareSum / Double(Self.frameSamples))))
                squareSum = 0
            }
        }
    }

    mutating func poll(after cursor: Int) throws -> [VoiceCaptureObservation] {
        try checkHealth()
        guard cursor >= acknowledged, cursor <= delivered, cursor.isMultiple(of: Self.frameSamples) else {
            throw Failure.invalidCursor
        }
        pending.removeFirst((cursor - acknowledged) / Self.frameSamples)
        acknowledged = cursor
        let batch = Array(pending.prefix(Self.batchSize))
        delivered = max(delivered, batch.last?.endSample ?? cursor)
        return batch
    }

    func wav(first: Int, end: Int) throws -> Data {
        try checkHealth()
        guard first >= max(0, sampleCount - samples.count), end > first, end <= delivered, end <= sampleCount,
              end - first <= clipLimit else {
            throw Failure.invalidRange
        }
        let pcmBytes = UInt32((end - first) * MemoryLayout<Int16>.size)
        var wav = Data(capacity: 44 + Int(pcmBytes))
        func appendLE<T: FixedWidthInteger>(_ value: T) {
            var little = value.littleEndian
            withUnsafeBytes(of: &little) { wav.append(contentsOf: $0) }
        }
        wav.append(contentsOf: "RIFF".utf8)
        appendLE(36 + pcmBytes)
        wav.append(contentsOf: "WAVEfmt ".utf8)
        appendLE(UInt32(16))
        appendLE(UInt16(1))
        appendLE(UInt16(1))
        appendLE(UInt32(Self.sampleRate))
        appendLE(UInt32(Self.sampleRate * 2))
        appendLE(UInt16(2))
        appendLE(UInt16(16))
        wav.append(contentsOf: "data".utf8)
        appendLE(pcmBytes)
        for index in first..<end { appendLE(samples[index % samples.count]) }
        return wav
    }

    func checkHealth() throws {
        if let failure { throw failure }
    }
}
