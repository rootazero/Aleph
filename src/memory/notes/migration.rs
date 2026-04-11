//! Migration logic to convert existing `MemoryFact`s into markdown knowledge notes.

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::AlephError;
use crate::memory::context::FactType;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::{KnowledgeNote, NoteIndexer};
use crate::memory::store::MemoryStore;

/// Statistics from a facts-to-notes migration run.
#[derive(Debug, Default)]
pub struct MigrationStats {
    pub facts_processed: usize,
    pub notes_created: usize,
    pub links_created: usize,
}

/// Migrate all valid `MemoryFact`s into `KnowledgeNote` markdown files.
///
/// Groups facts by a title derived from their VFS path, creates one note per
/// group, writes each note to disk, and indexes it in the `NoteStore`.
pub async fn migrate_facts_to_notes<S: NoteStore + MemoryStore>(
    store: &Arc<S>,
    indexer: &NoteIndexer<S>,
) -> Result<MigrationStats, AlephError> {
    let mut stats = MigrationStats::default();

    let facts = store.get_all_facts(false, None).await?;

    // Group facts by derived note title.
    // Key: (title, category), Value: (contents, created_at_min, updated_at_max)
    let mut groups: HashMap<String, (String, Vec<String>, i64, i64)> = HashMap::new();

    for fact in &facts {
        // Skip raw session data
        if fact.path.starts_with("aleph://session/") {
            continue;
        }

        stats.facts_processed += 1;

        let title = derive_note_title(&fact.path, &fact.fact_type);
        let category = fact_type_to_category(&fact.fact_type);

        let entry = groups
            .entry(title)
            .or_insert_with(|| (category, Vec::new(), i64::MAX, i64::MIN));

        entry.1.push(fact.content.clone());

        if fact.created_at < entry.2 {
            entry.2 = fact.created_at;
        }
        if fact.updated_at > entry.3 {
            entry.3 = fact.updated_at;
        }
    }

    // Create a note for each group, write to disk, and index.
    for (title, (category, contents, created_at, updated_at)) in &groups {
        let note = KnowledgeNote {
            title: title.clone(),
            category: category.clone(),
            tags: vec![],
            facts: contents.clone(),
            links: vec![],
            created_at: *created_at,
            updated_at: *updated_at,
            content_hash: String::new(), // will be recomputed on write/index
        };

        let path = indexer.write_note(&note).await?;
        indexer.index_file(&path).await?;

        stats.notes_created += 1;
    }

    Ok(stats)
}

/// Derive a human-readable note title from a VFS path and fact type.
///
/// Path format: `"aleph://user/preferences/coding"` → `"coding"`.
fn derive_note_title(path: &str, fact_type: &FactType) -> String {
    let segments: Vec<&str> = path
        .strip_prefix("aleph://")
        .unwrap_or(path)
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    match segments.as_slice() {
        [_, _, topic, ..] => topic.replace('_', " "),
        [_, topic] => topic.replace('_', " "),
        _ => format!("{:?}", fact_type).to_lowercase(),
    }
}

/// Map a `FactType` enum variant to a note category string.
fn fact_type_to_category(ft: &FactType) -> String {
    match ft {
        FactType::Preference => "preference",
        FactType::Plan => "plan",
        FactType::Learning => "learning",
        FactType::Project => "project",
        FactType::Personal => "personal",
        FactType::Tool => "tool",
        FactType::Lesson => "lesson",
        FactType::Skill => "skill",
        FactType::Wiki => "wiki",
        _ => "other",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_title_from_three_segment_path() {
        let title = derive_note_title("aleph://user/preferences/coding", &FactType::Preference);
        assert_eq!(title, "coding");
    }

    #[test]
    fn derives_title_from_two_segment_path() {
        let title = derive_note_title("aleph://user/hobbies", &FactType::Personal);
        assert_eq!(title, "hobbies");
    }

    #[test]
    fn replaces_underscores_with_spaces() {
        let title =
            derive_note_title("aleph://user/preferences/dark_mode", &FactType::Preference);
        assert_eq!(title, "dark mode");
    }

    #[test]
    fn falls_back_to_fact_type_for_empty_path() {
        let title = derive_note_title("", &FactType::Learning);
        assert_eq!(title, "learning");
    }

    #[test]
    fn maps_fact_types_to_categories() {
        assert_eq!(fact_type_to_category(&FactType::Preference), "preference");
        assert_eq!(fact_type_to_category(&FactType::Plan), "plan");
        assert_eq!(fact_type_to_category(&FactType::Learning), "learning");
        assert_eq!(fact_type_to_category(&FactType::Project), "project");
        assert_eq!(fact_type_to_category(&FactType::Personal), "personal");
        assert_eq!(fact_type_to_category(&FactType::Tool), "tool");
        assert_eq!(fact_type_to_category(&FactType::Lesson), "lesson");
        assert_eq!(fact_type_to_category(&FactType::Skill), "skill");
        assert_eq!(fact_type_to_category(&FactType::Wiki), "wiki");
        assert_eq!(fact_type_to_category(&FactType::Other), "other");
        assert_eq!(fact_type_to_category(&FactType::Transcript), "other");
    }
}
