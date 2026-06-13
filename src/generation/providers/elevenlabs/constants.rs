//! Constants for `ElevenLabs` TTS provider.

/// Default API endpoint for `ElevenLabs`
pub const DEFAULT_ENDPOINT: &str = "https://api.elevenlabs.io";

/// Default model for TTS
pub const DEFAULT_MODEL: &str = "eleven_monolingual_v1";

/// Default voice ID (Rachel)
pub const DEFAULT_VOICE_ID: &str = "21m00Tcm4TlvDq8ikWAM";

/// Default timeout for TTS requests (60 seconds)
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Available voices (name -> `voice_id`)
pub const VOICES: &[(&str, &str)] = &[
    ("rachel", "21m00Tcm4TlvDq8ikWAM"),
    ("domi", "AZnzlk1XvdvUeBnXmlld"),
    ("bella", "EXAVITQu4vr4xnSDxMaL"),
    ("antoni", "ErXwobaYiN019PkySvjV"),
    ("elli", "MF3mGyEYCl7XYWbV9V6O"),
    ("josh", "TxGEqnHWrfWFTfGW9XjX"),
    ("arnold", "VR6AewLTigWG4xSOukaG"),
    ("adam", "pNInz6obpgDQGcFmaJgB"),
    ("sam", "yoZ06aMxZJJ28mfd3POQ"),
];

/// Known models. This list is advisory only — unrecognized models are passed
/// through with a warning (see `ElevenLabsProvider::new`) so newer ElevenLabs
/// models work without a code change. Ordered roughly newest/best first.
pub const MODELS: &[&str] = &[
    "eleven_v3",
    "eleven_multilingual_v2",
    "eleven_flash_v2_5",
    "eleven_flash_v2",
    "eleven_turbo_v2_5",
    "eleven_turbo_v2",
    "eleven_multilingual_v1",
    "eleven_monolingual_v1",
];

/// Output formats
pub const OUTPUT_FORMATS: &[&str] = &[
    "mp3_44100_128",
    "mp3_44100_192",
    "pcm_16000",
    "pcm_22050",
    "pcm_24000",
    "pcm_44100",
];
