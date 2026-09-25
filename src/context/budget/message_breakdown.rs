//! The last measured prompt's message tokens, per session, split by kind.
//!
//! `ContextBudget::before_turn` / `note_compaction_effect` measure every
//! prompt on its way to the provider — after the preflight passes and any
//! compaction, projected to the reasoning that provider is sent — with the one
//! message estimator ([`estimate_message_tokens_split`]). A budget that was
//! told its session publishes that split here, and `context.breakdown` reads
//! it back as the conversation half of the window.
//!
//! Recorded at send time rather than re-derived from the event log at read
//! time: a re-derivation would describe a prompt that was never sent (no
//! preflight trimming, no compaction, no projection). A separate store from
//! `thinker::prompt_size_registry` because the two are measured at different
//! moments — the prompt layers once per run, the messages every turn.
//!
//! Process memory; a restart resets it, and the breakdown then reports the
//! messages as unknown until the session's next turn.
//!
//! [`estimate_message_tokens_split`]: super::pressure::estimate_message_tokens_split

use std::num::NonZeroUsize;
use std::sync::Mutex;

use lru::LruCache;

use super::pressure::MessageTokenSplit;

/// Sessions tracked at once; the stalest is dropped past this.
const MAX_TRACKED_SESSIONS: usize = 256;

fn store() -> &'static Mutex<LruCache<String, MessageTokenSplit>> {
    static STORE: std::sync::OnceLock<Mutex<LruCache<String, MessageTokenSplit>>> =
        std::sync::OnceLock::new();
    STORE.get_or_init(|| {
        Mutex::new(LruCache::new(
            NonZeroUsize::new(MAX_TRACKED_SESSIONS)
                .unwrap_or_else(|| unreachable!("MAX_TRACKED_SESSIONS > 0")),
        ))
    })
}

/// Record `split` as `session_key`'s latest measured prompt.
pub(crate) fn publish(session_key: &str, split: MessageTokenSplit) {
    store()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .put(session_key.to_string(), split);
}

/// `session_key`'s latest measured prompt, or `None` when none has been
/// measured since the server started.
#[must_use]
pub fn latest(session_key: &str) -> Option<MessageTokenSplit> {
    store()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(session_key)
        .copied()
}
