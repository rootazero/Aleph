use aleph_protocol::framing::DEFAULT_MAX_PAYLOAD_BYTES;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use serde_json::Value;

// Synthesized bytes originate in a bounded IPC reply. Apply its entire
// payload budget as a ceiling to the inline field before Base64 decoding.
const MAX_BASE64_BYTES: usize = DEFAULT_MAX_PAYLOAD_BYTES as usize;
const MAX_ID_BYTES: usize = 128;
const INVALID_AUDIO: &str = "invalid inline voice playback audio";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Play {
    pub(super) playback_id: String,
    audio_base64: String,
    mime_type: Mime,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Stop {
    pub(super) playback_id: String,
}

#[derive(Clone, Copy, Deserialize)]
pub(super) enum Mime {
    #[serde(rename = "audio/mpeg")]
    Mp3,
    #[serde(rename = "audio/wav")]
    Wav,
}

fn validate_id(id: &str) -> Result<(), &'static str> {
    if id.is_empty() || id.len() > MAX_ID_BYTES {
        return Err("invalid voice playback ID");
    }
    Ok(())
}

impl Play {
    pub(super) fn parse(value: Value) -> Result<Self, &'static str> {
        let play: Self = serde_json::from_value(value).map_err(|_| INVALID_AUDIO)?;
        validate_id(&play.playback_id)?;
        let len = play.audio_base64.len();
        if len == 0 || len > MAX_BASE64_BYTES || !len.is_multiple_of(4) {
            return Err(INVALID_AUDIO);
        }
        Ok(play)
    }

    pub(super) fn decode(self) -> Result<(Mime, Vec<u8>), &'static str> {
        let bytes = STANDARD
            .decode(self.audio_base64)
            .map_err(|_| INVALID_AUDIO)?;
        if !self.mime_type.matches(&bytes) {
            return Err(INVALID_AUDIO);
        }
        Ok((self.mime_type, bytes))
    }
}

impl Stop {
    pub(super) fn parse(value: Value) -> Result<Self, &'static str> {
        let stop: Self =
            serde_json::from_value(value).map_err(|_| "invalid voice playback stop")?;
        validate_id(&stop.playback_id)?;
        Ok(stop)
    }
}

impl Mime {
    // AVAudioPlayer's file type hint is not validation; only give it inline
    // data with a header matching the canonical MIME returned by synthesis.
    fn matches(self, bytes: &[u8]) -> bool {
        match self {
            Self::Wav => {
                let Some(header) = bytes.get(..12) else {
                    return false;
                };
                &header[..4] == b"RIFF"
                    && &header[8..12] == b"WAVE"
                    && u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize
                        == bytes.len() - 8
            }
            Self::Mp3 => {
                let Some(frame) = after_id3(bytes).and_then(|bytes| bytes.get(..4)) else {
                    return false;
                };
                frame[0] == 0xff
                    && frame[1] & 0xe0 == 0xe0
                    && frame[1] & 0x18 != 0x08
                    && frame[1] & 0x06 == 0x02
                    && frame[2] >> 4 != 0x0f
                    && frame[2] & 0x0c != 0x0c
            }
        }
    }
}

fn after_id3(bytes: &[u8]) -> Option<&[u8]> {
    if !bytes.starts_with(b"ID3") {
        return Some(bytes);
    }
    let header = bytes.get(..10)?;
    if !(2..=4).contains(&header[3]) || header[6..10].iter().any(|b| b & 0x80 != 0) {
        return None;
    }
    let size = header[6..10]
        .iter()
        .fold(0, |size, &byte| (size << 7) | usize::from(byte));
    let footer = if header[3] == 4 && header[5] & 0x10 != 0 {
        10
    } else {
        0
    };
    bytes.get(10 + size + footer..)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn playback_inline_rejects_paths_urls_unknown_mime_mismatch_and_oversize() {
        let wav = STANDARD.encode(b"RIFF\x04\0\0\0WAVE");
        let mp3 = STANDARD.encode(b"ID3\x04\0\0\0\0\0\0\xff\xfb\x90\xc0");
        let play = |mime: &str, audio: &str| {
            Play::parse(json!({"playback_id": "attempt", "audio_base64": audio, "mime_type": mime}))
                .and_then(Play::decode)
        };
        assert!(play("audio/wav", &wav).is_ok());
        assert!(play("audio/mpeg", &mp3).is_ok());
        assert!(play("audio/wav", &mp3).is_err());
        assert!(play("audio/mpeg", &wav).is_err());
        assert!(play("audio/mp3", &mp3).is_err());
        assert!(play("audio/wav", "https://example.com/audio").is_err());
        assert!(play("audio/wav", "/private/audio.wav").is_err());
        assert!(play("audio/wav", "TQ=!").is_err());
        assert!(Play::parse(json!({
            "playback_id": "attempt", "audio_base64": "A".repeat(MAX_BASE64_BYTES + 4),
            "mime_type": "audio/wav"
        }))
        .is_err());
        assert!(Play::parse(json!({
            "playback_id": "attempt", "audio_base64": wav, "mime_type": "audio/wav",
            "path": "/private/audio.wav"
        }))
        .is_err());
        assert!(Stop::parse(json!({"playback_id": ""})).is_err());
        assert!(
            Stop::parse(json!({"playback_id": "attempt", "url": "https://example.com"})).is_err()
        );
        assert_eq!(
            Stop::parse(json!({"playback_id": "Attempt"}))
                .unwrap()
                .playback_id,
            "Attempt"
        );
    }
}
