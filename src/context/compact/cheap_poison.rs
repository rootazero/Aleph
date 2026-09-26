//! Process-wide memory of cheap-tier summarizers that failed with a
//! model-class error.
//!
//! The compactor is rebuilt per run, so a per-compactor flag meant every run
//! that compacted paid one failed call to an aux model the endpoint does not
//! serve (the canonical case: a preset key pointed at a relay that has no
//! `default_aux_model`) before retrying on the main provider. The cheap
//! provider itself is built once at boot and shared by every run, so the fact
//! "this (provider, model) cannot summarize here" belongs to the process.
//!
//! Bounded both ways: an entry expires after [`POISON_TTL`] (a model-scoped
//! quota can heal), and at most [`MAX_ENTRIES`] are kept (oldest evicted).
//! Keyed by the provider's serving identity, so tests stay isolated by giving
//! their providers distinct names.

use std::time::{Duration, Instant};

use crate::providers::AiProvider;
use crate::sync_primitives::Mutex;

/// How long a poisoned cheap summarizer stays skipped.
const POISON_TTL: Duration = Duration::from_secs(60 * 60);

/// Upper bound on remembered entries. A process has one cheap summarizer per
/// primary provider, so this is never reached in practice.
const MAX_ENTRIES: usize = 64;

/// `(provider, model)` identity of a summarizer, from the serving hints the
/// provider (and every wrapper around it) reports, falling back to its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PoisonKey {
    provider: String,
    model: String,
}

impl PoisonKey {
    pub(crate) fn of(provider: &dyn AiProvider) -> Self {
        Self {
            provider: provider
                .serving_provider_hint()
                .map_or_else(|| provider.name().to_string(), |p| p.into_owned()),
            model: provider
                .serving_model_hint()
                .map(std::borrow::Cow::into_owned)
                .unwrap_or_default(),
        }
    }
}

/// The remembered failures, oldest first. A type rather than bare functions
/// over the static so tests exercise their own instance.
struct PoisonTable {
    entries: Vec<(PoisonKey, Instant)>,
}

impl PoisonTable {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    fn expire(&mut self, now: Instant) {
        self.entries
            .retain(|(_, at)| now.saturating_duration_since(*at) < POISON_TTL);
    }

    fn contains(&mut self, key: &PoisonKey, now: Instant) -> bool {
        self.expire(now);
        self.entries.iter().any(|(k, _)| k == key)
    }

    fn insert(&mut self, key: PoisonKey, now: Instant) -> bool {
        if self.contains(&key, now) {
            return false;
        }
        if self.entries.len() >= MAX_ENTRIES {
            self.entries.remove(0);
        }
        self.entries.push((key, now));
        true
    }
}

static POISONED: Mutex<PoisonTable> = Mutex::new(PoisonTable::new());

/// Whether `key` failed with a model-class error within [`POISON_TTL`].
pub(crate) fn is_poisoned(key: &PoisonKey) -> bool {
    POISONED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(key, Instant::now())
}

/// Record a model-class failure for `key`. Returns `true` when this call
/// poisoned it (so the caller logs the transition once), `false` when it was
/// already poisoned.
pub(crate) fn poison(key: PoisonKey) -> bool {
    POISONED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> PoisonKey {
        PoisonKey {
            provider: name.to_string(),
            model: "aux".to_string(),
        }
    }

    #[test]
    fn poison_is_reported_once_and_then_remembered() {
        let k = key(&format!("poison-once-{}", uuid::Uuid::new_v4()));
        assert!(!is_poisoned(&k));
        assert!(poison(k.clone()), "the first failure poisons");
        assert!(!poison(k.clone()), "the second is already known");
        assert!(is_poisoned(&k));
    }

    #[test]
    fn the_model_is_part_of_the_key() {
        let mut table = PoisonTable::new();
        let now = Instant::now();
        table.insert(key("p"), now);
        let other_model = PoisonKey {
            provider: "p".to_string(),
            model: "another-aux".to_string(),
        };
        assert!(!table.contains(&other_model, now));
    }

    #[test]
    fn an_entry_expires_after_the_ttl() {
        let mut table = PoisonTable::new();
        let then = Instant::now();
        table.insert(key("p"), then);
        assert!(table.contains(&key("p"), then + POISON_TTL / 2));
        assert!(!table.contains(&key("p"), then + POISON_TTL));
        assert!(
            table.insert(key("p"), then + POISON_TTL),
            "re-poisonable once expired"
        );
    }

    #[test]
    fn the_table_stays_bounded_and_evicts_the_oldest() {
        let mut table = PoisonTable::new();
        let now = Instant::now();
        for i in 0..=MAX_ENTRIES {
            table.insert(key(&format!("p{i}")), now);
        }
        assert_eq!(table.entries.len(), MAX_ENTRIES);
        assert!(
            !table.contains(&key("p0"), now),
            "the oldest entry went first"
        );
        assert!(table.contains(&key(&format!("p{MAX_ENTRIES}")), now));
    }
}
