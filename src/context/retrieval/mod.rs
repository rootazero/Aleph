//! Retrieval layer for offloaded tool output.
//!
//! When a tool result is too large for the context window it is evicted to
//! disk by [`crate::tools::result_store::ToolResultStore`]. This module adds
//! a searchable [`ContentIndex`] alongside that blob so the model can pull
//! back only the relevant slices via BM25 search instead of re-reading the
//! whole file — turning "offloaded but un-retrievable" into "offloaded and
//! precisely queryable".
//!
//! See [`content_index`] for the FTS5 implementation.

mod content_index;

pub(crate) use content_index::sanitize_fts_query;
pub use content_index::{ContentIndex, IndexError, IndexOutcome, SearchHit};
