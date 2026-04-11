//! Knowledge Notes — markdown-first memory units.
//!
//! Each note is a markdown file with YAML frontmatter containing category,
//! tags, bullet-point facts, and `[[wikilinks]]` to other notes.
//! SQLite is a rebuildable index; the markdown files are the source of truth.

mod note;
pub mod store;
mod wikilink;

pub use note::KnowledgeNote;
pub use wikilink::{extract_wikilinks, rewrite_wikilinks};
