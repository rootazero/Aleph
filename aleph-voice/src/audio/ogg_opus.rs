//! Minimal OGG-Opus mux/demux. Encode targets Telegram voice notes
//! (mono, VoIP-tuned); decode covers inbound TG voice messages.

use anyhow::{bail, Context};

const OGG_OPUS_SERIAL: u32 = 0x416c6570; // "Alep"

/// Encode mono f32 PCM into an OGG-Opus byte stream.
/// `sample_rate` must be one of 8/12/16/24/48 kHz (Kokoro 24k, inbound 16k both fit).
pub fn encode(samples: &[f32], sample_rate: u32) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(sample_rate > 0, "sample_rate must be non-zero");
    anyhow::ensure!(!samples.is_empty(), "cannot encode empty sample buffer");
    let mut enc = opus::Encoder::new(sample_rate, opus::Channels::Mono, opus::Application::Voip)
        .context("create opus encoder")?;
    let pre_skip = enc.get_lookahead().unwrap_or(0) as u16;

    let mut out = Vec::new();
    {
        let mut writer = ogg::PacketWriter::new(&mut out);
        // OpusHead (RFC 7845 §5.1)
        let mut head = Vec::with_capacity(19);
        head.extend_from_slice(b"OpusHead");
        head.push(1); // version
        head.push(1); // channels
        head.extend_from_slice(&pre_skip.to_le_bytes());
        head.extend_from_slice(&sample_rate.to_le_bytes()); // input rate (informational)
        head.extend_from_slice(&0i16.to_le_bytes()); // output gain
        head.push(0); // channel mapping family
        writer.write_packet(head, OGG_OPUS_SERIAL, ogg::PacketWriteEndInfo::EndPage, 0)?;
        // OpusTags
        let mut tags = Vec::new();
        tags.extend_from_slice(b"OpusTags");
        tags.extend_from_slice(&(11u32).to_le_bytes());
        tags.extend_from_slice(b"aleph-voice");
        tags.extend_from_slice(&0u32.to_le_bytes());
        writer.write_packet(tags, OGG_OPUS_SERIAL, ogg::PacketWriteEndInfo::EndPage, 0)?;

        // 20 ms frames; granule position counts 48 kHz samples (RFC 7845 §4).
        let frame = (sample_rate / 50) as usize;
        let granule_per_frame = 960u64; // 20ms @ 48k
        let mut granule = u64::from(pre_skip);
        let mut buf = vec![0u8; 4000];
        let total_frames = samples.len().div_ceil(frame);
        for (i, chunk) in samples.chunks(frame).enumerate() {
            let mut padded;
            let input = if chunk.len() == frame {
                chunk
            } else {
                padded = chunk.to_vec();
                padded.resize(frame, 0.0);
                &padded[..]
            };
            let n = enc.encode_float(input, &mut buf).context("opus encode")?;
            granule += granule_per_frame;
            let end = if i + 1 == total_frames {
                ogg::PacketWriteEndInfo::EndStream
            } else {
                ogg::PacketWriteEndInfo::NormalPacket
            };
            writer.write_packet(buf[..n].to_vec(), OGG_OPUS_SERIAL, end, granule)?;
        }
    }
    Ok(out)
}

/// Decode an OGG-Opus stream to 16 kHz mono f32 PCM (opus decoder resamples natively).
pub fn decode_to_16k(bytes: &[u8]) -> anyhow::Result<Vec<f32>> {
    let mut reader = ogg::PacketReader::new(std::io::Cursor::new(bytes));
    let mut dec = opus::Decoder::new(16_000, opus::Channels::Mono).context("create opus decoder")?;
    let mut pcm = Vec::new();
    let mut header_packets = 0u8;
    let mut buf = vec![0f32; 16_000]; // 1s max frame headroom
    while let Some(packet) = reader.read_packet()? {
        if header_packets < 2 {
            if header_packets == 0 && !packet.data.starts_with(b"OpusHead") {
                bail!("not an OGG-Opus stream");
            }
            if header_packets == 1 && !packet.data.starts_with(b"OpusTags") {
                bail!("missing OpusTags packet");
            }
            header_packets += 1;
            continue;
        }
        let n = dec.decode_float(&packet.data, &mut buf, false).context("opus decode")?;
        pcm.extend_from_slice(&buf[..n]);
    }
    Ok(pcm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_duration_and_energy() {
        let rate = 16_000u32;
        let samples: Vec<f32> = (0..rate) // 1s 440Hz
            .map(|i| (i as f32 * 440.0 * 2.0 * std::f32::consts::PI / rate as f32).sin() * 0.5)
            .collect();
        let encoded = encode(&samples, rate).unwrap();
        assert!(encoded.starts_with(b"OggS"));
        let decoded = decode_to_16k(&encoded).unwrap();
        let dur = decoded.len() as f32 / 16_000.0;
        assert!((dur - 1.0).abs() < 0.15, "duration {dur}s drifted");
        let energy: f32 = decoded.iter().map(|s| s * s).sum::<f32>() / decoded.len() as f32;
        assert!(energy > 0.01, "decoded audio is near-silent: {energy}");
    }

    #[test]
    fn rejects_non_opus() {
        assert!(decode_to_16k(b"OggS....garbage").is_err());
        assert!(decode_to_16k(b"plainly not ogg").is_err());
    }
}
