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
pub use store::{BatchOp, CuratedMemoryStore, WriteOutcome};

/// Configuration for the curated hot memory zone.
///
/// Defaults align with Hermes (`memory_tool.py` lines 116-119):
/// - `MEMORY.md` agent notes: 2,200 chars
/// - `USER.md` user profile: 1,375 chars
///
/// The envelope has a THIRD block — `OPEN_LOOPS.md` — and its render policy
/// belongs here beside its two siblings, even though the flag that decides
/// whether it is produced at all lives in `[memory.reflection]`. The split is
/// by owner, not by feature: reflection owns *whether loops are written*,
/// this config owns *how the curated envelope renders what it finds*.
#[derive(Debug, Clone, Copy)]
pub struct CuratedConfig {
    pub memory_char_limit: usize,
    pub user_char_limit: usize,
    pub legacy_warn_threshold: f32,
    /// Char budget for the `<OpenLoops>` block. Was a hardcoded `2000` at the
    /// single call site while both sibling blocks took theirs from config.
    pub open_loops_char_limit: usize,
    /// Days after which a persisted open-loops capture stops being injected.
    /// `0` disables the ceiling (inject at any age — the pre-ceiling
    /// behaviour).
    ///
    /// `OPEN_LOOPS.md` is rewritten ONLY when a reflection runs to completion,
    /// so every early return (disabled, cooldown, `min_turns`,
    /// `min_user_chars`, LLM error) leaves the previous capture in place. A
    /// month of sub-threshold sessions therefore kept re-injecting month-old
    /// follow-ups into the always-on Stable zone, and R5's "proactively pick
    /// them back up" had the agent chasing items the user had long since
    /// resolved. Naming the capture date (which the block already does) tells
    /// the model how old they are; it does not stop them from arriving.
    pub open_loops_max_age_days: u32,
}

impl Default for CuratedConfig {
    fn default() -> Self {
        Self {
            memory_char_limit: 2_200,
            user_char_limit: 1_375,
            legacy_warn_threshold: 0.95,
            open_loops_char_limit: 2_000,
            open_loops_max_age_days: 14,
        }
    }
}
