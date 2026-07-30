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
//! Each entry carries two facts beyond presence (see [`VoiceTurnState`]):
//! whether the user's message arrived as ASR-transcribed speech this turn, and
//! the session's resolved domain-vocabulary hint. Both were previously
//! discarded: the transcribed bit was OR-collapsed into the single "voice on"
//! boolean, and the vocabulary stopped at the ASR bias even though the model
//! needs the same term list to repair residual misrecognitions (FluidVoice's
//! one-dictionary-two-consumers pattern — the dictionary drives both the ASR
//! boost and the post-ASR repair).
//!
//! Entries are bounded two ways (mirroring `streaming::relay`'s StreamRegistry
//! capacity+TTL hygiene): a lazy TTL sweep once the map grows past
//! [`SWEEP_THRESHOLD`], and a hard [`MAX_ENTRIES`] ceiling that evicts the
//! oldest entries. Live sessions re-write their entry on every inbound
//! message, so reaping a stale entry can never break a live voice turn — the
//! next dispatch simply re-inserts it before the harness bridge reads.
//!
//! R10 note: pure plumbing, not cognition. It records the mechanical answer to
//! "is voice mode on for this session right now, and with what turn facts?"
//! and performs no judgment.
//!
//! Replaces the previously dead `metadata["voice_mode_active"]` stamp, which had
//! no reader: request metadata never reached `build_system_prompt`, so the
//! voice-mode prompt layer never fired in production.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;

use crate::sync_primitives::Mutex;

/// Stale-entry lifetime. Live sessions re-write on every inbound message, so
/// only abandoned sessions ever age out; six hours is far beyond any read
/// window (the harness bridge reads within the same turn that wrote).
const ENTRY_TTL: Duration = Duration::from_secs(6 * 60 * 60);

/// The O(n) sweep runs only once the map grows past this many entries — below
/// it the residual leak is a few KB and not worth a per-write pass.
const SWEEP_THRESHOLD: usize = 64;

/// Hard ceiling after the TTL sweep: evict oldest-first until under the cap.
const MAX_ENTRIES: usize = 1024;

/// The per-turn voice facts the gateway records for a voice-active session.
///
/// Presence in the registry = voice mode active (the reply is read aloud);
/// the fields refine *why* and *with what vocabulary*. R7: a mechanical
/// record of gateway facts, no judgment — the [`VoiceModeLayer`](crate::thinker::layers)
/// decides what to render from them.
#[derive(Debug, Clone)]
pub struct VoiceTurnState {
    /// This turn's user input arrived as ASR-transcribed speech.
    pub transcribed: bool,
    /// Domain-vocabulary hint (`[voice] vocabulary`), resolved by the writer
    /// from config at dispatch time. This is the same hint that biases ASR
    /// (`vocabulary_hint()` feeds the batch `prompt` and the streaming
    /// handshake `hotwords`); carrying it here lets `VoiceModeLayer` show the
    /// model the exact term list when inviting transcription repair, so a
    /// misrecognized domain word is repaired toward the configured term.
    /// `None` when no vocabulary is configured.
    pub vocabulary: Option<String>,
    /// Last-write stamp driving the lazy TTL sweep. Never read by the
    /// consumer side.
    recorded_at: Instant,
}

impl VoiceTurnState {
    /// Record the facts for the turn being dispatched *now*.
    #[must_use]
    pub fn new(transcribed: bool, vocabulary: Option<String>) -> Self {
        Self {
            transcribed,
            vocabulary,
            recorded_at: Instant::now(),
        }
    }
}

/// session_key → turn state. Presence in the map = voice mode active.
static ACTIVE: Lazy<Mutex<HashMap<String, VoiceTurnState>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn lock() -> crate::sync_primitives::MutexGuard<'static, HashMap<String, VoiceTurnState>> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Record voice mode for `session_key` this turn. Called by the inbound router
/// (and the Panel run builder) immediately before dispatching the run.
/// `None` clears any stale entry (voice off this turn).
pub fn set(session_key: &str, state: Option<VoiceTurnState>) {
    let mut guard = lock();
    match state {
        Some(s) => {
            guard.insert(session_key.to_string(), s);
        }
        None => {
            guard.remove(session_key);
        }
    }
    // Hygiene: reap dead-session entries once the map is worth sweeping, then
    // enforce the hard ceiling oldest-first. Both passes are no-ops for the
    // steady-state sizes a voice deployment actually reaches.
    if guard.len() > SWEEP_THRESHOLD {
        let now = Instant::now();
        guard.retain(|_, v| now.duration_since(v.recorded_at) < ENTRY_TTL);
        while guard.len() > MAX_ENTRIES {
            let oldest = guard
                .iter()
                .min_by_key(|(_, v)| v.recorded_at)
                .map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    guard.remove(&k);
                }
                None => break,
            }
        }
    }
}

/// Read the voice state for `session_key`. Called by the harness bridge during
/// prompt assembly. `None` = voice off; `Some(state)` = voice on with the
/// turn facts. Unknown session → `None`.
#[must_use]
pub fn get(session_key: &str) -> Option<VoiceTurnState> {
    lock().get(session_key).cloned()
}

#[cfg(test)]
fn set_with_age(session_key: &str, state: VoiceTurnState, age: Duration) {
    let mut state = state;
    state.recorded_at = Instant::now() - age;
    lock().insert(session_key.to_string(), state);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(transcribed: bool) -> Option<VoiceTurnState> {
        Some(VoiceTurnState::new(transcribed, None))
    }

    #[test]
    fn set_and_read_roundtrip() {
        let key = "voice-session-test-roundtrip";
        assert!(get(key).is_none());
        set(key, on(false));
        assert!(get(key).is_some_and(|s| !s.transcribed));
        set(key, on(true));
        assert!(get(key).is_some_and(|s| s.transcribed));
        set(key, None);
        assert!(get(key).is_none());
    }

    #[test]
    fn unknown_session_is_inactive() {
        assert!(get("voice-session-never-set").is_none());
    }

    #[test]
    fn transcribed_flag_is_carried() {
        let key = "voice-session-transcribed";
        set(key, on(true));
        assert!(
            get(key).is_some_and(|s| s.transcribed),
            "transcribed input must survive the wire"
        );
        set(key, None);
    }

    #[test]
    fn vocabulary_hint_is_carried() {
        let key = "voice-session-vocabulary";
        set(
            key,
            Some(VoiceTurnState::new(true, Some("Aleph, Leptos".to_string()))),
        );
        let state = get(key).expect("entry present");
        assert_eq!(state.vocabulary.as_deref(), Some("Aleph, Leptos"));
        set(key, None);
    }

    #[test]
    fn sessions_are_isolated() {
        set("voice-session-a", on(false));
        set("voice-session-b", None);
        assert!(get("voice-session-a").is_some_and(|s| !s.transcribed));
        assert!(get("voice-session-b").is_none());
        set("voice-session-a", None);
    }

    #[test]
    fn stale_entries_are_swept_past_threshold() {
        // Fill past the sweep threshold with ancient entries plus one fresh
        // one; the next `set` must reap the ancients and keep the fresh.
        for i in 0..=SWEEP_THRESHOLD {
            let key = format!("voice-sweep-stale-{i}");
            set_with_age(
                &key,
                VoiceTurnState::new(false, None),
                ENTRY_TTL + Duration::from_secs(1),
            );
        }
        let fresh = "voice-sweep-fresh";
        set(fresh, on(true));
        // The `set(fresh, …)` call itself pushed len past the threshold and
        // swept: every ancient entry is gone, the fresh one survives.
        for i in 0..=SWEEP_THRESHOLD {
            assert!(get(&format!("voice-sweep-stale-{i}")).is_none());
        }
        assert!(get(fresh).is_some());
        set(fresh, None);
    }

    #[test]
    fn hard_cap_evicts_oldest_first() {
        // All entries under the TTL (the sweep cannot help): the hard ceiling
        // must evict oldest-first. Ages start at 5h so no concurrently-running
        // test's entry (this binary shares the process-global map) can be
        // older and steal an eviction slot, making the order deterministic.
        const BASE_AGE: Duration = Duration::from_secs(5 * 60 * 60);
        for i in 0..(MAX_ENTRIES + 2) {
            let key = format!("voice-cap-{i:05}");
            // Entry 0 is the oldest; all stay under ENTRY_TTL (6h).
            let age = BASE_AGE - Duration::from_secs(i as u64);
            set_with_age(&key, VoiceTurnState::new(false, None), age);
        }
        // Trigger the hygiene path with one more ordinary write.
        set("voice-cap-trigger", on(false));
        // The two oldest are gone; the newest write survives.
        assert!(get("voice-cap-00000").is_none());
        assert!(get("voice-cap-00001").is_none());
        assert!(get("voice-cap-trigger").is_some());
        // Clean up the block so other tests see an empty map.
        let mut guard = lock();
        guard.retain(|k, _| !k.starts_with("voice-cap-"));
    }
}
