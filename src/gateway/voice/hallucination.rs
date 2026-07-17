//! Whisper hallucination filter for inbound STT — re-export of the shared
//! transcript-hygiene pass.
//!
//! The implementation lives in [`aleph_protocol::voice_text`] so the Panel can
//! apply the *identical* whole-utterance filter at the streaming lock point
//! (per-segment filtering inside the streaming decoders would eat legitimate
//! short segments of longer speech). This path is kept stable for the batch
//! STT caller ([`super::inbound::stt`]).

pub use aleph_protocol::voice_text::filter_transcript;
