import AVFoundation
import Darwin
import Foundation
import XCTest
@testable import AlephBridge

final class OfflineSpeechAudioTests: XCTestCase {
    func testLoadOwnsSignedSamplesAndMakesAnIndependentPCMBuffer() throws {
        let expected: [Int16] = [.min, -1234, 0, 0x1234, .max]
        try withTemporaryDirectory { directory in
            let file = directory.appendingPathComponent("voice.wav")
            try Data(canonicalWAV(expected)).write(to: file)
            let audio = try OfflineSpeechAudio.load(fromTrustedPath: file.path)
            try FileManager.default.removeItem(at: file)

            XCTAssertEqual(audio.samples, expected)
            let buffer = try XCTUnwrap(audio.makePCMBuffer())
            XCTAssertEqual(buffer.format.commonFormat, .pcmFormatInt16)
            XCTAssertEqual(buffer.format.sampleRate, 16_000)
            XCTAssertEqual(buffer.format.channelCount, 1)
            XCTAssertFalse(buffer.format.isInterleaved)
            XCTAssertEqual(buffer.frameLength, AVAudioFrameCount(expected.count))
            let channel = try XCTUnwrap(buffer.int16ChannelData?[0])
            XCTAssertEqual(Array(UnsafeBufferPointer(start: channel, count: expected.count)), expected)
            channel[0] = 42
            XCTAssertEqual(audio.samples, expected)
        }
    }

    func testLoadAcceptsOneFrame() throws {
        try withTemporaryDirectory { directory in
            let file = directory.appendingPathComponent("voice.wav")
            try Data(canonicalWAV([0x1234])).write(to: file)
            XCTAssertEqual(try OfflineSpeechAudio.load(fromTrustedPath: file.path).samples, [0x1234])
        }
    }

    func testLoadAcceptsExactUpperBoundAndRejectsOneExtraByteOrFrame() throws {
        var samples = [Int16](repeating: -1234, count: 320_000)
        samples[samples.count - 1] = 5678
        let wav = canonicalWAV(samples)
        XCTAssertEqual(wav.count, 640_044)
        try withTemporaryDirectory { directory in
            let file = directory.appendingPathComponent("voice.wav")
            try Data(wav).write(to: file)
            let audio = try OfflineSpeechAudio.load(fromTrustedPath: file.path)
            XCTAssertEqual(audio.samples, samples)
            let buffer = try XCTUnwrap(audio.makePCMBuffer())
            XCTAssertEqual(buffer.frameLength, 320_000)
            XCTAssertEqual(buffer.int16ChannelData?[0][319_999], 5678)

            try Data(wav + [0]).write(to: file)
            XCTAssertThrowsError(try OfflineSpeechAudio.load(fromTrustedPath: file.path)) {
                XCTAssertEqual($0 as? OfflineSpeechAudio.LoadError, .invalidFile)
            }
            try Data(canonicalWAV(samples + [0])).write(to: file)
            XCTAssertThrowsError(try OfflineSpeechAudio.load(fromTrustedPath: file.path)) {
                XCTAssertEqual($0 as? OfflineSpeechAudio.LoadError, .invalidFile)
            }
        }
    }

    func testLoadRejectsNonCanonicalHeaders() throws {
        let valid = canonicalWAV([1])
        let fields: [(String, Int)] = [
            ("RIFF", 0), ("RIFF size", 4), ("WAVE", 8), ("fmt ", 12), ("fmt size", 16),
            ("encoding", 20), ("channels", 22), ("sample rate", 24), ("byte rate", 28),
            ("block alignment", 32), ("bit depth", 34), ("data", 36), ("data size", 40),
        ]
        try withTemporaryDirectory { directory in
            let file = directory.appendingPathComponent("malformed.wav")
            for (field, offset) in fields {
                var malformed = valid
                malformed[offset] ^= 1
                try Data(malformed).write(to: file)
                XCTAssertThrowsError(try OfflineSpeechAudio.load(fromTrustedPath: file.path), field) {
                    XCTAssertEqual($0 as? OfflineSpeechAudio.LoadError, .invalidAudio, field)
                }
            }
        }
    }

    func testLoadRejectsEmptyTruncatedTrailingAndUnalignedData() throws {
        let valid = canonicalWAV([1, 2, 3])
        var oddLength = canonicalWAV([1]) + [0]
        oddLength[4] = 39
        oddLength[40] = 3
        let fixtures: [(String, [UInt8], OfflineSpeechAudio.LoadError)] = [
            ("truncated header", Array(valid.prefix(43)), .invalidFile),
            ("zero frames", canonicalWAV([]), .invalidFile),
            ("truncated PCM", Array(valid.dropLast(2)), .invalidAudio),
            ("trailing bytes", valid + [0, 0], .invalidAudio),
            ("odd PCM length", oddLength, .invalidAudio),
        ]
        try withTemporaryDirectory { directory in
            let file = directory.appendingPathComponent("malformed.wav")
            for (label, wav, error) in fixtures {
                try Data(wav).write(to: file)
                XCTAssertThrowsError(try OfflineSpeechAudio.load(fromTrustedPath: file.path), label) {
                    XCTAssertEqual($0 as? OfflineSpeechAudio.LoadError, error, label)
                }
            }
        }
    }

    func testLoadRejectsFinalSymlinkAndNonRegularDescriptor() throws {
        try withTemporaryDirectory { directory in
            let file = directory.appendingPathComponent("voice.wav")
            let link = directory.appendingPathComponent("link.wav")
            try Data(canonicalWAV([1])).write(to: file)
            try FileManager.default.createSymbolicLink(at: link, withDestinationURL: file)

            XCTAssertThrowsError(try OfflineSpeechAudio.load(fromTrustedPath: link.path)) {
                XCTAssertEqual($0 as? OfflineSpeechAudio.LoadError, .openFailed)
            }
            XCTAssertThrowsError(try OfflineSpeechAudio.load(fromTrustedPath: directory.path)) {
                XCTAssertEqual($0 as? OfflineSpeechAudio.LoadError, .invalidFile)
            }
        }
    }

    func testReadAndCloseClosesOnceBeforeReturningBytes() throws {
        try withTemporaryDirectory { directory in
            let file = directory.appendingPathComponent("voice.wav")
            let wav = canonicalWAV([0x1234])
            try Data(wav).write(to: file)
            let descriptor = file.path.withCString { Darwin.open($0, O_RDONLY) }
            guard descriptor >= 0 else { return XCTFail("Could not open the fixture") }

            var closeCalls = 0
            var closed = false
            let bytes = try OfflineSpeechAudio.readAndClose(descriptor) { fd in
                closeCalls += 1
                guard closeCalls == 1 else { return -1 }
                XCTAssertEqual(fd, descriptor)
                XCTAssertEqual(Darwin.lseek(descriptor, 0, SEEK_CUR), off_t(wav.count))
                let result = Darwin.close(descriptor)
                closed = result == 0
                return result
            }
            XCTAssertTrue(closed)
            XCTAssertEqual(closeCalls, 1)
            XCTAssertEqual(bytes, wav)
        }
    }

    func testReadAndCloseDoesNotReturnBytesOrRetryAfterUncertainClose() throws {
        try withTemporaryDirectory { directory in
            let file = directory.appendingPathComponent("voice.wav")
            try Data(canonicalWAV([1])).write(to: file)
            let descriptor = file.path.withCString { Darwin.open($0, O_RDONLY) }
            guard descriptor >= 0 else { return XCTFail("Could not open the fixture") }

            var closeCalls = 0
            XCTAssertThrowsError(try OfflineSpeechAudio.readAndClose(descriptor) { fd in
                closeCalls += 1
                XCTAssertEqual(fd, descriptor)
                if closeCalls == 1 {
                    // Model a close that reports failure even though the descriptor is gone.
                    XCTAssertEqual(Darwin.close(descriptor), 0)
                }
                return -1
            }) {
                XCTAssertEqual($0 as? OfflineSpeechAudio.LoadError, .closeFailed)
            }
            XCTAssertEqual(closeCalls, 1)
        }
    }

    private func withTemporaryDirectory(_ body: (URL) throws -> Void) throws {
        let manager = FileManager.default
        let directory = manager.temporaryDirectory.resolvingSymlinksInPath()
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try manager.createDirectory(at: directory, withIntermediateDirectories: false)
        defer { try? manager.removeItem(at: directory) }
        try body(directory)
    }

    private func canonicalWAV(_ samples: [Int16]) -> [UInt8] {
        func le32(_ value: UInt32) -> [UInt8] {
            (0..<4).map { UInt8(truncatingIfNeeded: value >> ($0 * 8)) }
        }
        let pcmSize = UInt32(samples.count * 2)
        var bytes = Array("RIFF".utf8)
        bytes += le32(36 + pcmSize)
        bytes += Array("WAVEfmt ".utf8)
        bytes += le32(16)
        bytes += [1, 0, 1, 0]
        bytes += le32(16_000)
        bytes += le32(32_000)
        bytes += [2, 0, 16, 0]
        bytes += Array("data".utf8)
        bytes += le32(pcmSize)
        bytes.reserveCapacity(44 + Int(pcmSize))
        for sample in samples {
            let bits = UInt16(bitPattern: sample)
            bytes.append(UInt8(truncatingIfNeeded: bits))
            bytes.append(UInt8(truncatingIfNeeded: bits >> 8))
        }
        return bytes
    }
}
