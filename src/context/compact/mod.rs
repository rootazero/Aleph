//! Cross-turn context compaction for the live conversation.
//!
//! Houses the LLM-based `ContextCompactor` plus the semantic-unit chunker and
//! summary utilities it relies on.
//!
//! `session_summary_source` (cross-session artifact consumer) lives under
//! `crate::memory::session_compactor::summary_source`; it IS part of the
//! live-compaction path — the compactor's zero-cost `SessionMemoryReuse`
//! strategy reads it before paying a side-channel summarization call.

pub mod compactor;
pub mod directive;
/// Event-level cut-boundary guards shared by the drain sites that cut into
/// the persisted event log (`manual` / `session_split`) — the event-typed
/// mirror of the compactor's message-level `snap_boundary_forward`.
mod event_snap;
/// Cumulative "which files did this conversation read / change" ledger,
/// re-emitted below the summary at every compaction drain site (pi
/// `computeFileLists` parity). Private to the compaction module for the same
/// reason [`plan_carry`] is: the only legitimate producer is a drain.
mod file_carry;
pub mod fit;
/// User-driven `/compact`: summarize the conversation prefix and soft-retire it
/// from the event log. Orthogonal to the pressure-driven in-turn compaction in
/// [`compactor`] — that one produces a transient summary for one prompt, this
/// one edits what every future prompt is rebuilt from.
pub mod manual;
/// Re-injection of the model's own execution list below the summary at every
/// compaction drain site — private to the compaction module, which owns all of
/// them.
mod plan_carry;
mod preserve;
pub mod rescue;
pub mod session_split;
pub mod summary_utils;
pub mod tool_aware_chunker;
