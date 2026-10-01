//! Cross-turn context compaction for the live conversation.
//!
//! Houses the LLM-based `ContextCompactor` plus the semantic-unit chunker and
//! summary utilities it relies on.
//!
//! `session_summary_source` (cross-session artifact consumer) lives under
//! `crate::memory::session_compactor::summary_source`; it IS part of the
//! live-compaction path — the compactor's zero-cost `SessionMemoryReuse`
//! strategy reads it before paying a side-channel summarization call.

mod cheap_poison;
/// Cross-run fingerprint-cache carry-over for [`ContextCompactor`](compactor) —
/// the process-wide per-session slot that survives a run boundary, plus the
/// zero-cost session-memory reuse wiring. Private to the compaction module
/// because both are loaded into `ContextCompactor` at construction time and
/// nothing outside this directory constructs or inspects either.
mod compaction_cache;
/// Pure window-construction helpers shared by every compaction path (window
/// selection, fingerprint hashing, transcript serialization). Private to the
/// compaction module because each helper is an internal step of one of the
/// drain sites the compactor orchestrates.
pub(crate) mod compaction_window;
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
/// The fold ledger: lazy per-fold payback verdicts (`judge_fold`) over the
/// fold registry's token accounting plus the `compactor:<agent>` metering
/// channel. Pure functions only — no background sweep, no hot-path cost; the
/// `core/fold-economics` doctor check calls in when someone asks whether a
/// fold net-saved tokens or cost more than it will ever repay.
pub mod fold_ledger;
/// The fold registry: a derived, read-only view over `FoldRecorded` /
/// `CompactionPerformed` events that makes every compaction's retired span
/// addressable. No parallel table — `session_events` stays the single source
/// of truth.
pub mod folds;
/// Re-emit the newest screenshot below the summary, so the image the preflight
/// image-stripping stage deliberately protects survives the drain that runs
/// immediately after it on the same vector.
mod image_carry;
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
