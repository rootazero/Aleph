//! Unlinked-mention scanner (Task 15). Constant lives here from day one so
//! store/stage code can reference the relation label before the scanner lands.

/// Relation label for auto-detected unlinked mentions in `notes_links`.
pub const MENTION_RELATION: &str = "mention";
/// Confidence for mention soft edges (spec §2.3).
pub const MENTION_CONFIDENCE: f32 = 0.35;
