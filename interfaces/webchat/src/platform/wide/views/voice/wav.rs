//! Pure audio codec helpers for the immersive capture path — host-testable,
//! zero web_sys.
//!
//! The capture path buffers raw Float32 PCM (with a pre-roll lead-in so the
//! first syllable is never clipped) and must hand the core a self-contained
//! audio blob. WAV is chosen over the old MediaRecorder/webm route precisely
//! because webm chunks are not independently decodable — you cannot prepend a
//! pre-roll to a webm stream — whereas raw PCM → WAV is trivially sliceable.

/// Encode mono Float32 samples (nominally −1.0..1.0) as a 16-bit PCM WAV byte
/// buffer (44-byte header + little-endian samples). Out-of-range samples are
/// clamped, not wrapped.
pub(crate) fn encode_wav_mono(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    const CHANNELS: u16 = 1;
    const BITS: u16 = 16;
    let block_align = CHANNELS * (BITS / 8);
    let byte_rate = sample_rate * u32::from(block_align);
    let data_len = (samples.len() * 2) as u32;

    let mut buf = Vec::with_capacity(44 + data_len as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_len).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk body size
    buf.extend_from_slice(&1u16.to_le_bytes()); // audio format = PCM
    buf.extend_from_slice(&CHANNELS.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&BITS.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        buf.extend_from_slice(&v.to_le_bytes());
    }
    buf
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 (RFC 4648, `+/`, `=` padding). Pure so the WAV→RPC hop needs
/// no JS `btoa` (which would mangle the Latin-1 round-trip on bytes ≥ 0x80).
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(B64[(b0 >> 2) as usize] as char);
        out.push(B64[(((b0 & 0b11) << 4) | (b1 >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64[(((b1 & 0b1111) << 2) | (b2 >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[(b2 & 0b111111) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_well_formed() {
        let wav = encode_wav_mono(&[0.0, 0.0], 16_000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        // sample rate at offset 24 (LE u32)
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        // 2 samples * 2 bytes = 4 bytes of data
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 4);
        assert_eq!(wav.len(), 44 + 4);
    }

    #[test]
    fn samples_are_clamped_not_wrapped() {
        let wav = encode_wav_mono(&[2.0, -2.0], 8_000);
        let s0 = i16::from_le_bytes(wav[44..46].try_into().unwrap());
        let s1 = i16::from_le_bytes(wav[46..48].try_into().unwrap());
        assert_eq!(s0, 32_767);
        assert_eq!(s1, -32_767);
    }

    #[test]
    fn empty_samples_yield_bare_header() {
        let wav = encode_wav_mono(&[], 16_000);
        assert_eq!(wav.len(), 44);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 0);
    }

    #[test]
    fn base64_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_handles_high_bytes() {
        // bytes >= 0x80 must not be corrupted (the btoa-via-Latin1 trap)
        assert_eq!(base64_encode(&[0xff, 0x00, 0x80]), "/wCA");
    }
}
