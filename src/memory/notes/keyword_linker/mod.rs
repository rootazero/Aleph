//! Keyword-based note linking: LLM extracts a keyword set per note, code pairs
//! notes by set overlap (see `overlap`). Edges carry a fixed `co_tag` relation
//! type; the connecting keyword is kept only as `via_keyword` diagnostics.

pub mod extract;
pub mod overlap;

pub use extract::{extract_keywords, NoteForExtraction};
pub use overlap::{pair_by_overlap, LinkTriple, NoteKeywords, CO_TAG_RELATION};
