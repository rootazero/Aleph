//! Curated hot memory zone — Hermes-inspired bounded MEMORY.md per agent.
//!
//! See `docs/superpowers/specs/2026-05-01-memory-evolution-spec-a-curated-hot-snapshot-design.md`.

pub mod budget;
pub mod format;
pub mod legacy;
pub mod snapshot;
pub mod store;

#[cfg(test)]
mod tests;

pub use snapshot::CuratedSnapshot;
pub use store::{CuratedMemoryStore, WriteOutcome};

/// Configuration for the curated hot memory zone.
///
/// Defaults align with Hermes (`memory_tool.py` lines 116-119):
/// - `MEMORY.md` agent notes: 2,200 chars
/// - `USER.md` user profile: 1,375 chars
#[derive(Debug, Clone, Copy)]
pub struct CuratedConfig {
    pub memory_char_limit: usize,
    pub user_char_limit: usize,
    pub legacy_warn_threshold: f32,
}

impl Default for CuratedConfig {
    fn default() -> Self {
        Self {
            memory_char_limit: 2_200,
            user_char_limit: 1_375,
            legacy_warn_threshold: 0.95,
        }
    }
}

#[cfg(test)]
#[path = "."]
mod default_test {
    #[test]
    fn defaults_match_hermes_values() {
        let c = super::CuratedConfig::default();
        assert_eq!(c.memory_char_limit, 2_200);
        assert_eq!(c.user_char_limit, 1_375);
        assert!((c.legacy_warn_threshold - 0.95).abs() < 1e-6);
    }
}
