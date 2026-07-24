//! Shared component types for the agentic loop.
//!
//! Note: the legacy `EventHandler`-based component chain (`IntentAnalyzer`,
//! `TaskPlanner`, `ToolExecutor`, `LoopController`, `SessionRecorder`, `SessionCompactor`)
//! has been removed — it was superseded by the `src/harness/` Think→Act loop and
//! had no production consumers. Only the shared domain types remain; they are
//! consumed by the `event` system (session/part types).

mod types;

pub use types::*;
