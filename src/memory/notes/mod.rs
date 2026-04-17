//! Knowledge Notes — markdown-first memory units.
//!
//! Each note is a markdown file with YAML frontmatter containing category,
//! tags, bullet-point facts, and `[[wikilinks]]` to other notes.
//! SQLite is a rebuildable index; the markdown files are the source of truth.

pub mod extractor;
pub mod indexer;
mod note;
pub mod orientation;
pub mod retrieval;
pub mod search_result;
pub mod store;
mod wikilink;

pub use indexer::{IndexStats, NoteIndexer, CATEGORY_DIRS};
pub use note::{sanitize_title, KnowledgeNote};
pub use retrieval::{NoteContent, NoteRetrieval};
pub use search_result::NoteSearchResult;
pub use wikilink::{extract_wikilinks, resolve_wikilink, rewrite_wikilinks};

pub mod ingest;
pub mod profile;
pub mod query_filer;
