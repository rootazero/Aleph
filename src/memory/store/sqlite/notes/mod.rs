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

// Re-export the crate-visible helpers at the module path so existing callers
// (and `tests.rs` via `super::super::body_text_sha256`) keep their paths.
pub(crate) use helpers::{
    body_text_sha256, collect_edges_between, load_note_content_from_disk,
    provenance_origin_from_str, provenance_origin_to_str, row_to_entry,
};

#[cfg(test)]
mod tests;
