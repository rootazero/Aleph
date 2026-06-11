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
//! R10 note: pure plumbing, not cognition. It records the mechanical answer to
//! "is voice mode on for this session right now?" and performs no judgment.
//!
//! Replaces the previously dead `metadata["voice_mode_active"]` stamp, which had
//! no reader: request metadata never reached `build_system_prompt`, so the
//! voice-mode prompt layer never fired in production.

use std::collections::HashSet;

use once_cell::sync::Lazy;

use crate::sync_primitives::Mutex;

static ACTIVE: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));

fn lock() -> crate::sync_primitives::MutexGuard<'static, HashSet<String>> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Record whether voice mode is active for `session_key` this turn. Called by
/// the inbound router immediately before dispatching the run.
pub fn set(session_key: &str, active: bool) {
    let mut guard = lock();
    if active {
        guard.insert(session_key.to_string());
    } else {
        guard.remove(session_key);
    }
}

/// Read whether voice mode is active for `session_key`. Called by the harness
/// bridge during prompt assembly. Unknown session → `false`.
#[must_use]
pub fn is_active(session_key: &str) -> bool {
    lock().contains(session_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_read_roundtrip() {
        let key = "voice-session-test-roundtrip";
        assert!(!is_active(key));
        set(key, true);
        assert!(is_active(key));
        set(key, false);
        assert!(!is_active(key));
    }

    #[test]
    fn unknown_session_is_inactive() {
        assert!(!is_active("voice-session-never-set"));
    }

    #[test]
    fn sessions_are_isolated() {
        set("voice-session-a", true);
        set("voice-session-b", false);
        assert!(is_active("voice-session-a"));
        assert!(!is_active("voice-session-b"));
        set("voice-session-a", false);
    }
}
