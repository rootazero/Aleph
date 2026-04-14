//! LLM-facing orientation layer: SCHEMA.md + index.md + log.md.
//!
//! The three markdown files under `~/.aleph/memory/note/{agent_id}/` give the
//! LLM a global map each session. SQLite remains a rebuildable index; this
//! module owns the human-and-LLM-readable projection.

pub mod types;

pub use types::{IndexStats, LogAction, LogEntry, OrientationSnapshot, TokenBudget};
