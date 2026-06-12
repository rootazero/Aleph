//! Audio I/O: anything-in → 16 kHz mono f32 PCM; PCM → wav / ogg-opus out.
//!
//! Formats: wav/mp3/m4a-aac/flac via symphonia; ogg-opus + webm-opus via the
//! opus decoder (symphonia demuxes mkv/webm but has no opus codec).

pub mod ogg_opus;

use anyhow::{bail, Context};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::CODEC_TYPE_OPUS;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::probe::Hint;

pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Decode arbitrary input audio to 16 kHz mono f32 PCM.
/// `name_hint` is a filename or mime used to seed format probing.
pub fn decode_to_pcm_mono_16k(bytes: &[u8], name_hint: &str) -> anyhow::Result<Vec<f32>> {
    if bytes.len() >= 36 && bytes.starts_with(b"OggS") && bytes[28..].windows(8).take(64).any(|w| w == b"OpusHead") {
        return ogg_opus::decode_to_16k(bytes);
    }
    decode_via_symphonia(bytes, name_hint)
}

fn decode_via_symphonia(bytes: &[u8], name_hint: &str) -> anyhow::Result<Vec<f32>> {
    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes.to_vec())), Default::default());
    let mut hint = Hint::new();
    let lower = name_hint.to_ascii_lowercase();
    for (needle, ext) in [
        ("wav", "wav"), ("mp3", "mp3"), ("m4a", "m4a"), ("mp4", "mp4"),
        ("aac", "aac"), ("flac", "flac"), ("webm", "webm"), ("ogg", "ogg"),
    ] {
        if lower.contains(needle) {
            hint.with_extension(ext);
            break;
        }
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &Default::default(), &Default::default())
        .context("unrecognized audio container")?;
    let mut format = probed.format;
    let track = format.default_track().context("no audio track")?.clone();
    let src_rate = track.codec_params.sample_rate.unwrap_or(48_000);
    let channels = track.codec_params.channels.map_or(1, |c| c.count().max(1));

    // webm/mkv carrying opus: symphonia demuxes, opus crate decodes (to 16k directly).
    if track.codec_params.codec == CODEC_TYPE_OPUS {
        let mut dec = opus::Decoder::new(TARGET_SAMPLE_RATE, opus::Channels::Mono)?;
        let mut pcm = Vec::new();
        let mut buf = vec![0f32; 16_000];
        while let Ok(packet) = format.next_packet() {
            if packet.track_id() != track.id {
                continue;
            }
            match dec.decode_float(packet.buf(), &mut buf, false) {
                Ok(n) => pcm.extend_from_slice(&buf[..n]),
                Err(e) => tracing::warn!("skipping undecodable opus packet: {e}"),
            }
        }
        if pcm.is_empty() {
            bail!("opus stream decoded to zero samples");
        }
        return Ok(pcm);
    }

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &Default::default())
        .context("unsupported audio codec")?;
    let mut mono: Vec<f32> = Vec::new();
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track.id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // Tolerate locally corrupt packets / trailing junk, but propagate
            // structural failures (unsupported features, reset-required, etc.).
            Err(symphonia::core::errors::Error::DecodeError(e)) => {
                tracing::warn!("skipping undecodable packet: {e}");
                continue;
            }
            Err(symphonia::core::errors::Error::IoError(e)) => {
                tracing::warn!("skipping packet with I/O error: {e}");
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        let mut sbuf = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
        sbuf.copy_interleaved_ref(decoded);
        let frames = sbuf.samples().chunks_exact(channels);
        mono.extend(frames.map(|f| f.iter().sum::<f32>() / channels as f32));
    }
    if mono.is_empty() {
        bail!("audio decoded to zero samples");
    }
    resample_to_16k(&mono, src_rate)
}

/// High-quality sinc resample to 16 kHz (no-op when already there).
pub fn resample_to_16k(samples: &[f32], src_rate: u32) -> anyhow::Result<Vec<f32>> {
    if src_rate == TARGET_SAMPLE_RATE {
        return Ok(samples.to_vec());
    }
    use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
    let params = SincInterpolationParameters {
        sinc_len: 128,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 128,
        window: WindowFunction::BlackmanHarris2,
    };
    let chunk = 1024usize;
    let mut rs = SincFixedIn::<f32>::new(
        f64::from(TARGET_SAMPLE_RATE) / f64::from(src_rate),
        2.0,
        params,
        chunk,
        1,
    )?;
    let delay = rs.output_delay();
    let ideal_len = (samples.len() as f64 * f64::from(TARGET_SAMPLE_RATE) / f64::from(src_rate))
        .round() as usize;
    let mut out = Vec::with_capacity(delay + ideal_len + chunk);
    let mut pos = 0usize;
    while pos + chunk <= samples.len() {
        let processed = rs.process(&[&samples[pos..pos + chunk]], None)?;
        out.extend_from_slice(&processed[0]);
        pos += chunk;
    }
    // Flush: feed the final partial block (rubato zero-pads internally), then
    // drain the resampler's delay line until all real frames are out.
    let mut tail = &samples[pos..];
    while out.len() < delay + ideal_len {
        let processed = if tail.is_empty() {
            rs.process_partial::<&[f32]>(None, None)?
        } else {
            let p = rs.process_partial(Some(&[tail]), None)?;
            tail = &[];
            p
        };
        if processed[0].is_empty() {
            break; // normal drain completion; trim below handles shortfall
        }
        out.extend_from_slice(&processed[0]);
    }
    // Drop the sinc filter's group delay at the head and the padded silence tail.
    out.drain(..delay.min(out.len()));
    out.truncate(ideal_len);
    Ok(out)
}

/// Encode mono f32 PCM as 16-bit WAV.
pub fn encode_wav(samples: &[f32], sample_rate: u32) -> anyhow::Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut w = hound::WavWriter::new(&mut cursor, spec)?;
        for s in samples {
            w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)?;
        }
        w.finalize()?;
    }
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize)
            .map(|i| (i as f32 * 440.0 * 2.0 * std::f32::consts::PI / rate as f32).sin() * 0.5)
            .collect()
    }

    #[test]
    fn wav_roundtrip_through_decode_entry() {
        let pcm = sine(16_000, 0.5);
        let wav = encode_wav(&pcm, 16_000).unwrap();
        let back = decode_to_pcm_mono_16k(&wav, "x.wav").unwrap();
        assert!((back.len() as i64 - pcm.len() as i64).abs() < 32);
    }

    #[test]
    fn resamples_48k_wav_to_16k() {
        let pcm48 = sine(48_000, 0.5);
        let wav = encode_wav(&pcm48, 48_000).unwrap();
        let back = decode_to_pcm_mono_16k(&wav, "x.wav").unwrap();
        let dur = back.len() as f32 / 16_000.0;
        assert!((dur - 0.5).abs() < 0.005, "duration {dur}");
    }

    #[test]
    fn resample_output_length_is_ideal() {
        let pcm48 = sine(48_000, 0.5); // 24_000 samples
        let out = resample_to_16k(&pcm48, 48_000).unwrap();
        assert_eq!(out.len(), 8_000, "no padded-silence tail, no delay head");
    }

    #[test]
    fn resample_handles_input_shorter_than_one_chunk() {
        let pcm48 = sine(48_000, 500.0 / 48_000.0); // 500 samples < 1024 chunk
        let out = resample_to_16k(&pcm48, 48_000).unwrap();
        assert!(!out.is_empty());
        assert!(out.len().abs_diff(167) <= 2, "got {} samples", out.len());
    }

    #[test]
    fn ogg_opus_input_routes_to_opus_decoder() {
        let pcm = sine(16_000, 0.4);
        let encoded = ogg_opus::encode(&pcm, 16_000).unwrap();
        let back = decode_to_pcm_mono_16k(&encoded, "voice.ogg").unwrap();
        assert!(back.len() > 16_000 / 4);
    }

    #[test]
    fn mp3_fixture_decodes() {
        let bytes = include_bytes!("../../tests/fixtures/tone.mp3");
        let pcm = decode_to_pcm_mono_16k(bytes, "tone.mp3").unwrap();
        assert!(pcm.len() > 1_000, "mp3 decoded {} samples", pcm.len());
    }

    #[test]
    fn garbage_input_errors() {
        assert!(decode_to_pcm_mono_16k(b"definitely not audio", "x.bin").is_err());
    }
}
