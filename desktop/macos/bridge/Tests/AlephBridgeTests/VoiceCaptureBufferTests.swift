import XCTest
@testable import AlephBridge

final class VoiceCaptureBufferTests: XCTestCase {
    func testReadinessAndObservationsUseOnlyCompleteRealFrames() throws {
        var buffer = VoiceCaptureBuffer(clipLimit: 320_000)
        XCTAssertEqual(try buffer.poll(after: 0), [])
        try Array(repeating: Int16(-16_384), count: 799).withUnsafeBufferPointer { try buffer.append($0) }
        XCTAssertEqual(try buffer.poll(after: 0), [])
        try [Int16(-16_384)].withUnsafeBufferPointer { try buffer.append($0) }
        XCTAssertEqual(try buffer.poll(after: 0), [.init(endSample: 800, rms: 0.5)])
        XCTAssertEqual(try buffer.poll(after: 800), [])
        XCTAssertThrowsError(try buffer.poll(after: 0))
    }

    func testDelayedPollBatchesAreContiguousAndBoundedIndependentlyOfPCM() throws {
        var buffer = VoiceCaptureBuffer(clipLimit: 320_000)
        try Array(repeating: Int16(1), count: 32 * 800).withUnsafeBufferPointer { try buffer.append($0) }
        let first = try buffer.poll(after: 0)
        XCTAssertEqual(first.map(\.endSample), (1...16).map { $0 * 800 })
        XCTAssertEqual(try buffer.poll(after: 0), first)
        XCTAssertEqual(try buffer.poll(after: 12_800).map(\.endSample), (17...32).map { $0 * 800 })
        try Array(repeating: Int16(1), count: 16 * 800).withUnsafeBufferPointer { try buffer.append($0) }
        XCTAssertThrowsError(try Array(repeating: Int16(1), count: 800).withUnsafeBufferPointer { try buffer.append($0) }) {
            XCTAssertEqual($0 as? VoiceCaptureBuffer.Failure, .observationOverflow)
        }
        XCTAssertThrowsError(try buffer.wav(first: 0, end: 800))
    }

    func testWrappedPCMStillSealsExactSamplesAfterFortySeconds() throws {
        var buffer = VoiceCaptureBuffer(clipLimit: 320_000)
        var cursor = 0
        for frame in 0..<800 {
            try Array(repeating: Int16(frame), count: 800).withUnsafeBufferPointer { try buffer.append($0) }
            cursor = try XCTUnwrap(buffer.poll(after: cursor).last).endSample
        }
        XCTAssertEqual(cursor, 640_000)
        let clip = try buffer.wav(first: 320_000, end: cursor)
        XCTAssertEqual(clip.count, 640_044)
        XCTAssertEqual(Array(clip[44..<46]), [144, 1])
        XCTAssertEqual(Array(clip.suffix(2)), [31, 3])
        XCTAssertThrowsError(try buffer.wav(first: 0, end: 800))
    }

    func testNormalizedStreamCannotExceedFortyFiveSeconds() throws {
        var buffer = VoiceCaptureBuffer(clipLimit: 320_000)
        var cursor = 0
        let frame = Array(repeating: Int16(1), count: 800)
        for _ in 0..<900 {
            try frame.withUnsafeBufferPointer { try buffer.append($0) }
            cursor = try XCTUnwrap(buffer.poll(after: cursor).last).endSample
        }
        XCTAssertEqual(cursor, 720_000)
        XCTAssertThrowsError(try [Int16(1)].withUnsafeBufferPointer { try buffer.append($0) }) {
            XCTAssertEqual($0 as? VoiceCaptureBuffer.Failure, .sampleOverflow)
        }
    }

    func testRingNeverOverwritesUnacknowledgedClipHorizon() throws {
        var buffer = VoiceCaptureBuffer(clipLimit: 320_000)
        var cursor = 0
        let frame = Array(repeating: Int16(1), count: 800)
        for _ in 0..<400 {
            try frame.withUnsafeBufferPointer { try buffer.append($0) }
            cursor = try XCTUnwrap(buffer.poll(after: cursor).last).endSample
        }
        XCTAssertEqual(cursor, 320_000)
        XCTAssertEqual(try buffer.poll(after: cursor), [])
        for _ in 0..<16 { try frame.withUnsafeBufferPointer { try buffer.append($0) } }
        XCTAssertThrowsError(try frame.withUnsafeBufferPointer { try buffer.append($0) }) {
            XCTAssertEqual($0 as? VoiceCaptureBuffer.Failure, .sampleOverflow)
        }
        XCTAssertThrowsError(try buffer.wav(first: 0, end: cursor))
    }

    func testWAVUsesExactHalfOpenRangeAnd320000SampleCap() throws {
        var buffer = VoiceCaptureBuffer(clipLimit: 320_000)
        var cursor = 0
        for sample in 0..<403 {
            try Array(repeating: Int16(sample), count: 800).withUnsafeBufferPointer { try buffer.append($0) }
            cursor = try XCTUnwrap(buffer.poll(after: cursor).last).endSample
        }
        // The production pre-roll controller selects this range without rounding
        // either endpoint to the 800-sample observation cadence.
        let wav = try buffer.wav(first: 2_080, end: 322_080)
        XCTAssertEqual(wav.count, 640_044)
        XCTAssertEqual(Array(wav[44..<46]), [2, 0])
        XCTAssertEqual(Array(wav.suffix(2)), [146, 1])
        XCTAssertThrowsError(try buffer.wav(first: 2_079, end: 322_080)) {
            XCTAssertEqual($0 as? VoiceCaptureBuffer.Failure, .invalidRange)
        }
        XCTAssertThrowsError(try buffer.wav(first: 2_401, end: cursor + 1)) {
            XCTAssertEqual($0 as? VoiceCaptureBuffer.Failure, .invalidRange)
        }
    }
}
