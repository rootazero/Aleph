//! Voice-related built-in tools.
//!
//! Exposes voice configuration operations as LLM-callable tools (R9 compliance).
//! The LLM can toggle voice mode, adjust provider/voice settings, and query
//! voice state purely through natural language.

pub mod local_voice;
pub mod voice_mode_set;

pub use local_voice::{LocalVoiceArgs, LocalVoiceOutput, LocalVoiceTool};
pub use voice_mode_set::{VoiceModeSetArgs, VoiceModeSetOutput, VoiceModeSetTool};
