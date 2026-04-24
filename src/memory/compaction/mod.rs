//! Compaction — TRANSITIONAL. The framework modules have moved to
//! `crate::context::compact`. Only `session_summary_source` remains
//! here temporarily; it moves to `memory::session_compactor::summary_source`
//! in Commit 3 of the P1 migration.

pub mod session_summary_source;
pub use session_summary_source::SessionSummarySource;
