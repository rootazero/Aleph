//! Voice subsystem — three concepts, four files.
//!
//! ## Four files, three concepts
//!
//! | File | Concept | Keying |
//! |------|---------|--------|
//! | `voice_mode.rs` | **session-turn registry** | `session_key` → `VoiceTurnState{transcribed, vocabulary}` |
//! | `state.rs` | **channel state** | `channel_id` → `VoiceState{enabled, provider, voice, consecutive_failures}` |
//! | `voice_mode_set.rs` (in `builtin_tools/voice_tools/`) | **LLM-callable tool** | mutates `VoiceState` (channel) only |
//! | `thinker::layers::voice_mode.rs` | **prompt-layer** | reads `voice_mode` registry → "## Voice Mode" block |
//!
//! Read this first if you're searching for "voice mode" or "voice state":
//! - "voice mode" on a **session** → `voice_mode.rs` (registry) + `layers/voice_mode.rs` (layer)
//! - "voice mode" on a **channel** → `state.rs` + `voice_mode_set.rs` tool
//!
//! Don't confuse the two. A session has a per-turn `VoiceTurnState`; a channel
//! has a long-lived `VoiceState`. The names are confusingly similar by
//! design—the 2026-07-21 rename `session_mode.rs` → `voice_mode.rs` was the
//! last deliberate fix for a previous collision; the table above is the
//! canonical cross-reference. R10 note: this disambiguation lives in docs
//! rather than file names because rename risk (git history blow-up, public
//! type-name drift) outweighs the readability gain.
//!
//! Streaming STT (`streaming/`), batch STT (`inbound/`), and TTS (`outbound.rs`)
//! are the runtime layers; they read from these state modules but never write
//! to them directly. The single funnel for cross-process voice plumbing is
//! `voice_mode.rs` (registry) — every voice-active turn writes there before
//! dispatch, and `prompt_build.rs` reads it during prompt assembly.

pub mod format;
pub mod hallucination;
pub mod inbound;
pub mod local_provider;
pub mod outbound;
pub mod sanitize;
pub mod state;
pub mod streaming;
pub mod voice_mode;
pub use state::VoiceState;
