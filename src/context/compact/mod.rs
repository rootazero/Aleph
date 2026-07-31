//! Cross-turn context compaction for the live conversation.
//!
//! Houses the LLM-based `ContextCompactor` plus the semantic-unit chunker and
//! summary utilities it relies on.
//!
//! `session_summary_source` (cross-session artifact consumer) remains under
//! `crate::memory::session_compactor::summary_source` — it is not part of the
//! live-compaction path.

pub mod compactor;
pub mod directive;
pub mod fit;
/// User-driven `/compact`: summarize the conversation prefix and soft-retire it
/// from the event log. Orthogonal to the pressure-driven in-turn compaction in
/// [`compactor`] — that one produces a transient summary for one prompt, this
/// one edits what every future prompt is rebuilt from.
pub mod manual;
/// Verbatim re-attachment of the user's own turns at every compaction drain
/// site — private to the compaction module, which owns all four of them.
mod plan_carry;
mod preserve;
pub mod rescue;
pub mod session_split;
pub mod summary_utils;
pub mod tool_aware_chunker;

pub use summary_utils::{strip_analysis_block, IDENTIFIER_PRESERVATION};
pub use tool_aware_chunker::{parse_semantic_units, SemanticChunk, SemanticUnit, ToolAwareChunker};
