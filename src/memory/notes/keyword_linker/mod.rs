//! Keyword-based note linking: LLM extracts a keyword set per note, code pairs
//! notes by set overlap (see `overlap`), links carry the connecting keyword.

pub mod extract;
pub mod overlap;

pub use extract::{extract_keywords, NoteForExtraction};
pub use overlap::{pair_by_overlap, LinkTriple, NoteKeywords};
