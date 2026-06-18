//! `NoteStore` implementation for `SqliteMemoryBackend`.
//!
//! Stores note index entries, wikilink edges, and FTS content
//! in the `notes_index`, `notes_links`, and `notes_fts` tables.
//!
//! All data is scoped by `agent_id`. Notes are identified by
//! `path = "{category}/{title}"` within each agent.
//!
//! Split into:
//! - [`helpers`]: free helper functions (`row_to_entry`, hashing, provenance
//!   encoding, disk loading, edge collection).
//! - [`store_impl`]: the single indivisible `impl NoteStore for
//!   SqliteMemoryBackend` block (cannot be split across files).

mod helpers;
mod store_impl;

// `store_impl` imports helpers directly from `helpers`; the only consumer of a
// re-export at this module path is `tests.rs` (`super::super::body_text_sha256`),
// so re-export just that one, gated to test builds.
#[cfg(test)]
pub(crate) use helpers::body_text_sha256;

#[cfg(test)]
mod tests;
