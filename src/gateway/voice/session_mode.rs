//! Session → voice-mode pointer registry.
//!
//! Voice mode is decided in the gateway inbound router (from per-channel
//! [`VoiceState`](super::state::VoiceState) plus the inbound voice-reply hint),
//! but it must be *read* much later and in a different subsystem — during system
//! prompt assembly in the orchestrator's harness bridge — so that
//! [`VoiceModeLayer`](crate::thinker::layers) can inject the spoken-reply
//! guidelines. The two sides share only the canonical session key.
//!
//! This is the same shape as [`scratchpad_registry`](crate::builtin_tools::scratchpad_registry):
//! a process-global session-keyed pointer the producer writes and the consumer
//! reads back. Unlike the scratchpad binding it needs **no persistence** — voice
//! mode is recomputed by the router on every inbound message, so a restart that
//! drops the table simply re-derives it on the next turn.
//!
//! Each entry carries one extra fact beyond presence: whether the user's message
//! arrived as ASR-transcribed speech this turn. The inbound router already
//! computes that (`voice::inbound` transcription), but it used to be OR-collapsed
//! into the single "voice on" boolean and lost. Carrying it here lets
//! `VoiceModeLayer` adapt the prompt (invite transcription-artifact repair)
//! without any new classifier — the registry still only records gateway facts.
//!
//! R10 note: pure plumbing, not cognition. It records the mechanical answer to
//! "is voice mode on for this session right now, and was the input transcribed?"
//! and performs no judgment.
//!
//! Replaces the previously dead `metadata["voice_mode_active"]` stamp, which had
//! no reader: request metadata never reached `build_system_prompt`, so the
//! voice-mode prompt layer never fired in production.

use std::collections::HashMap;

use once_cell::sync::Lazy;

use crate::sync_primitives::Mutex;

/// session_key → `transcribed_input`. Presence in the map = voice mode active;
/// the value = whether this turn's user input was ASR-transcribed speech.
static ACTIVE: Lazy<Mutex<HashMap<String, bool>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn lock() -> crate::sync_primitives::MutexGuard<'static, HashMap<String, bool>> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Record voice mode for `session_key` this turn. Called by the inbound router
/// immediately before dispatching the run. `active=false` clears any stale
/// entry; `transcribed` marks that the user's message arrived as transcribed
/// speech (only meaningful when `active`).
pub fn set(session_key: &str, active: bool, transcribed: bool) {
    let mut guard = lock();
    if active {
        guard.insert(session_key.to_string(), transcribed);
    } else {
        guard.remove(session_key);
    }
}

/// Read the voice state for `session_key`. Called by the harness bridge during
/// prompt assembly. `None` = voice off; `Some(transcribed)` = voice on with the
/// ASR-input flag. Unknown session → `None`.
#[must_use]
pub fn get(session_key: &str) -> Option<bool> {
    lock().get(session_key).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_read_roundtrip() {
        let key = "voice-session-test-roundtrip";
        assert_eq!(get(key), None);
        set(key, true, false);
        assert_eq!(get(key), Some(false));
        set(key, true, true);
        assert_eq!(get(key), Some(true));
        set(key, false, false);
        assert_eq!(get(key), None);
    }

    #[test]
    fn unknown_session_is_inactive() {
        assert_eq!(get("voice-session-never-set"), None);
    }

    #[test]
    fn transcribed_flag_is_carried() {
        let key = "voice-session-transcribed";
        set(key, true, true);
        assert_eq!(
            get(key),
            Some(true),
            "transcribed input must survive the wire"
        );
        set(key, false, false);
    }

    #[test]
    fn sessions_are_isolated() {
        set("voice-session-a", true, false);
        set("voice-session-b", false, false);
        assert_eq!(get("voice-session-a"), Some(false));
        assert_eq!(get("voice-session-b"), None);
        set("voice-session-a", false, false);
    }
}
