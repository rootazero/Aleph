import AVFoundation
import Darwin

/// Detached samples from the Core's canonical mono 16 kHz PCM16 WAV.
struct OfflineSpeechAudio: Sendable {
    enum LoadError: Error, Equatable {
        case openFailed
        case statFailed
        case invalidFile
        case readFailed
        case closeFailed
        case invalidAudio
    }

    private static let sampleRate = 16_000
    private static let sampleBytes = 2
    private static let headerBytes = 44
    private static let maxFrames = sampleRate * 20
    private static let maxWAVBytes = headerBytes + maxFrames * sampleBytes

    let samples: [Int16]

    private init(samples: [Int16]) {
        self.samples = samples
    }

    /// The caller must establish trust in the absolute canonical path's parent.
    static func load(fromTrustedPath path: String) throws -> Self {
        let descriptor = path.withCString {
            Darwin.open($0, O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK)
        }
        guard descriptor >= 0 else { throw LoadError.openFailed }
        return try decodeWAV(readAndClose(descriptor))
    }

    /// Takes ownership of an opened descriptor. No bytes escape unless close succeeds.
    static func readAndClose(
        _ descriptor: Int32,
        closeDescriptor: (Int32) -> Int32 = Darwin.close
    ) throws -> [UInt8] {
        let readResult = Result { try readBounded(descriptor) }
        // An interrupted close leaves the descriptor's state uncertain. Never close it twice.
        guard closeDescriptor(descriptor) == 0 else { throw LoadError.closeFailed }
        return try readResult.get()
    }

    private static func readBounded(_ descriptor: Int32) throws -> [UInt8] {
        var status = stat()
        guard Darwin.fstat(descriptor, &status) == 0 else { throw LoadError.statFailed }
        guard (status.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG),
              status.st_size >= off_t(headerBytes + sampleBytes),
              status.st_size <= off_t(maxWAVBytes)
        else { throw LoadError.invalidFile }

        // The extra byte detects growth even if the original size was already at the cap.
        var bytes = [UInt8](repeating: 0, count: maxWAVBytes + 1)
        var offset = 0
        while offset < bytes.count {
            let count = bytes.withUnsafeMutableBytes { buffer -> Int in
                var count: Int
                repeat {
                    count = Darwin.read(
                        descriptor, buffer.baseAddress!.advanced(by: offset), buffer.count - offset
                    )
                } while count == -1 && errno == EINTR
                return count
            }
            guard count >= 0 else { throw LoadError.readFailed }
            if count == 0 { break }
            offset += count
        }
        guard offset < bytes.count, off_t(offset) == status.st_size else {
            throw LoadError.invalidFile
        }
        bytes.removeLast(bytes.count - offset)
        return bytes
    }

    private static func decodeWAV(_ bytes: [UInt8]) throws -> Self {
        func u16(_ offset: Int) -> UInt16 {
            UInt16(bytes[offset]) | (UInt16(bytes[offset + 1]) << 8)
        }
        func u32(_ offset: Int) -> UInt32 {
            UInt32(u16(offset)) | (UInt32(u16(offset + 2)) << 16)
        }

        let pcmBytes = bytes.count - headerBytes
        guard pcmBytes >= sampleBytes, pcmBytes <= maxFrames * sampleBytes,
              pcmBytes.isMultiple(of: sampleBytes),
              bytes[0..<4].elementsEqual("RIFF".utf8),
              u32(4) == UInt32(bytes.count - 8),
              bytes[8..<16].elementsEqual("WAVEfmt ".utf8),
              u32(16) == 16, u16(20) == 1, u16(22) == 1,
              u32(24) == UInt32(sampleRate), u32(28) == UInt32(sampleRate * sampleBytes),
              u16(32) == UInt16(sampleBytes), u16(34) == 16,
              bytes[36..<40].elementsEqual("data".utf8),
              u32(40) == UInt32(pcmBytes)
        else { throw LoadError.invalidAudio }

        let samples = stride(from: headerBytes, to: bytes.count, by: sampleBytes).map {
            Int16(bitPattern: u16($0))
        }
        return Self(samples: samples)
    }

    func makePCMBuffer() -> AVAudioPCMBuffer? {
        let frames = AVAudioFrameCount(samples.count)
        guard let format = AVAudioFormat(
            commonFormat: .pcmFormatInt16, sampleRate: Double(Self.sampleRate),
            channels: 1, interleaved: false
        ), let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frames),
           let destination = buffer.int16ChannelData?[0]
        else { return nil }

        buffer.frameLength = frames
        samples.withUnsafeBufferPointer { source in
            destination.update(from: source.baseAddress!, count: source.count)
        }
        return buffer
    }
}
