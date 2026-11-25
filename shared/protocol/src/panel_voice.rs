//! Whole-turn voice RPC wire limits and typed failures shared by Core and clients.

use serde::{Deserialize, Serialize};

pub const SAMPLE_RATE_HZ: u32 = 16_000;
pub const MAX_UTTERANCE_SAMPLES: usize = SAMPLE_RATE_HZ as usize * 20;
pub const MAX_PCM_BYTES: usize = MAX_UTTERANCE_SAMPLES * std::mem::size_of::<i16>();
pub const WAV_HEADER_BYTES: usize = 44;
pub const MAX_WAV_BYTES: usize = WAV_HEADER_BYTES + MAX_PCM_BYTES;
pub const MAX_WAV_BASE64_BYTES: usize = MAX_WAV_BYTES.div_ceil(3) * 4;
pub const VOICE_TAB_ID: &str = "voice";
pub const VOICE_TURN_TRANSCRIBE_METHOD: &str = "voice.turn.transcribe";
pub const VOICE_TURN_SYNTHESIZE_METHOD: &str = "voice.turn.synthesize";
pub const VOICE_TURN_CANCEL_METHOD: &str = "voice.turn.cancel";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceErrorKind {
    InvalidAudio,
    NoSpeech,
    TooLarge,
    NotConfigured,
    OfflineUnavailable,
    UnsupportedLanguage,
    PermissionDenied,
    ProviderFailed,
    UnsupportedOutput,
    CaptureUnavailable,
    Cancelled,
}
