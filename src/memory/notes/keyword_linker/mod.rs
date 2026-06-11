//! Keyword-based note linking: LLM extracts a keyword set per note, code pairs
//! notes by set overlap (see `overlap`), links carry the connecting keyword.

pub mod overlap;

pub use overlap::{pair_by_overlap, LinkTriple, NoteKeywords};
