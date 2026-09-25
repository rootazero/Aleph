//! Per-session tally of what tool output cost on its way into the context.
//!
//! Layer 2 (`result_processing::apply_result_budget`) knows, for every call,
//! how many tokens the tool produced and how many of them it admitted into the
//! conversation — `ProcessedResult::tokens_in_context` — and whether it
//! offloaded the rest. Nothing read that figure, so "how much does ingress
//! reduction actually save?" had no answer anywhere. The dispatcher records
//! each call here ([`record`]); `context.breakdown` reports the tally for the
//! session it is asked about.
//!
//! Process memory, like the prompt-size registry beside it: a restart resets
//! it, and the wire says "since the server started", not "ever".

use std::num::NonZeroUsize;

use std::sync::Mutex;

use lru::LruCache;

/// Sessions tracked at once; the stalest is dropped past this. An entry is
/// four counters and a key, so the whole map stays a few KB.
const MAX_TRACKED_SESSIONS: usize = 256;

/// One session's tool output, summed over every dispatched call since the
/// server started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IngressTally {
    /// Tool calls whose result went through Layer 2.
    pub calls: u64,
    /// Estimated tokens the tools produced (the flattened, pre-ingress output).
    pub produced_tokens: u64,
    /// Estimated tokens of those results that entered the conversation.
    pub in_context_tokens: u64,
    /// Results whose full output was offloaded to the result store.
    pub offloaded: u64,
}

fn tallies() -> &'static Mutex<LruCache<String, IngressTally>> {
    static TALLIES: std::sync::OnceLock<Mutex<LruCache<String, IngressTally>>> =
        std::sync::OnceLock::new();
    TALLIES.get_or_init(|| {
        Mutex::new(LruCache::new(
            NonZeroUsize::new(MAX_TRACKED_SESSIONS)
                .unwrap_or_else(|| unreachable!("MAX_TRACKED_SESSIONS > 0")),
        ))
    })
}

/// Add one call to `session_key`'s tally.
pub(crate) fn record(session_key: &str, produced: usize, in_context: usize, offloaded: bool) {
    let mut map = tallies()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let entry = map.get_or_insert_mut(session_key.to_string(), IngressTally::default);
    entry.calls = entry.calls.saturating_add(1);
    entry.produced_tokens = entry.produced_tokens.saturating_add(produced as u64);
    entry.in_context_tokens = entry.in_context_tokens.saturating_add(in_context as u64);
    entry.offloaded = entry.offloaded.saturating_add(u64::from(offloaded));
}

/// `session_key`'s tally, or `None` when no call of that session has been
/// recorded since the server started.
#[must_use]
pub fn tally(session_key: &str) -> Option<IngressTally> {
    tallies()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(session_key)
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_accumulate_per_session_and_sessions_stay_apart() {
        let a = format!("test:tally:a:{}", uuid::Uuid::new_v4());
        let b = format!("test:tally:b:{}", uuid::Uuid::new_v4());
        assert_eq!(tally(&a), None, "an unseen session has no tally, not zeros");

        record(&a, 10_000, 1_200, true);
        record(&a, 300, 300, false);
        record(&b, 50, 50, false);

        assert_eq!(
            tally(&a),
            Some(IngressTally {
                calls: 2,
                produced_tokens: 10_300,
                in_context_tokens: 1_500,
                offloaded: 1,
            })
        );
        assert_eq!(tally(&b).map(|t| t.calls), Some(1));
    }
}
