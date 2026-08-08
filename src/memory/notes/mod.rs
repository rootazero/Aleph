//! Knowledge Notes — markdown-first memory units.
//!
//! Each note is a markdown file with YAML frontmatter containing category,
//! tags, bullet-point facts, and `[[wikilinks]]` to other notes.
//! `SQLite` is a rebuildable index; the markdown files are the source of truth.

pub mod dedup;
pub mod governance;
pub mod graph;
pub mod indexer;
pub mod keyword_linker;
pub mod links;
mod note;
pub mod orientation;
pub mod search_result;
pub mod store;
pub mod watcher;
mod wikilink;

pub use dedup::find_similar_notes;
pub use governance::gate::{
    CandidateNote, DefaultNoteWriteGate, GateOutcome, GateThresholds, NoteWriteAction,
    NoteWriteGate,
};
pub use indexer::{canonicalize_category, IndexStats, NoteIndexer, RebuildAllStats, CATEGORY_DIRS};
pub use note::{
    is_structural_strong, sanitize_note_path, sanitize_title, tags_mark_permanent, FactProvenance,
    KnowledgeNote, ProvenanceOrigin, Relation, Severity, STRUCTURAL_STRONG,
};
pub use search_result::NoteSearchResult;
pub use wikilink::{extract_wikilinks, extract_wikilinks_with_alias, rewrite_wikilinks};

pub mod ingest;
pub mod profile;
pub mod query_filer;
