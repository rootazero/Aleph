//! Migration logic to convert existing `MemoryFact`s into markdown knowledge notes.

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::AlephError;
use crate::memory::context::NoteType;
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

        let title = derive_note_title(&fact.path, &fact.note_type);
        let category = note_type_to_category(&fact.note_type);

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

        let path = indexer.write_note("default", &category, &note).await?;
        indexer.index_file("default", &category, &path).await?;

        stats.notes_created += 1;
    }

    Ok(stats)
}

/// Derive a human-readable note title from a VFS path and fact type.
///
/// Path format: `"aleph://user/preferences/coding"` → `"coding"`.
fn derive_note_title(path: &str, note_type: &NoteType) -> String {
    let segments: Vec<&str> = path
        .strip_prefix("aleph://")
        .unwrap_or(path)
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    match segments.as_slice() {
        [_, _, topic, ..] => topic.replace('_', " "),
        [_, topic] => topic.replace('_', " "),
        _ => format!("{:?}", note_type).to_lowercase(),
    }
}

/// Map a `NoteType` enum variant to a note category string.
fn note_type_to_category(ft: &NoteType) -> String {
    match ft {
        NoteType::Preference => "preference",
        NoteType::Plan => "plan",
        NoteType::Learning => "learning",
        NoteType::Project => "project",
        NoteType::Personal => "personal",
        NoteType::Tool => "tool",
        NoteType::Lesson => "lesson",
        NoteType::Skill => "skill",
        NoteType::Wiki => "wiki",
        _ => "other",
    }
    .to_string()
}

/// Migrate wiki files from old location (`~/.aleph/data/wiki/{agent_id}/`)
/// to the new memory/notes structure (`~/.aleph/data/memory/{agent_id}/wiki/`).
///
/// Only copies `.md` files that do not already exist at the destination,
/// making this function safe to call repeatedly (idempotent).
pub async fn migrate_wiki_files(
    old_wiki_dir: &std::path::Path,
    memory_dir: &std::path::Path,
    agent_id: &str,
) -> Result<usize, AlephError> {
    let dest_dir = memory_dir.join(agent_id).join("wiki");
    tokio::fs::create_dir_all(&dest_dir).await.map_err(|e| {
        AlephError::other(format!("Failed to create wiki dest dir: {}", e))
    })?;

    let mut entries = match tokio::fs::read_dir(old_wiki_dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => {
            return Err(AlephError::other(format!(
                "Failed to read old wiki dir: {}",
                e
            )))
        }
    };

    let mut migrated = 0;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if let Some(filename) = path.file_name() {
            let dest = dest_dir.join(filename);
            if !dest.exists() {
                tokio::fs::copy(&path, &dest).await.map_err(|e| {
                    AlephError::other(format!(
                        "Failed to copy wiki file {:?}: {}",
                        filename, e
                    ))
                })?;
                migrated += 1;
            }
        }
    }
    Ok(migrated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_title_from_three_segment_path() {
        let title = derive_note_title("aleph://user/preferences/coding", &NoteType::Preference);
        assert_eq!(title, "coding");
    }

    #[test]
    fn derives_title_from_two_segment_path() {
        let title = derive_note_title("aleph://user/hobbies", &NoteType::Personal);
        assert_eq!(title, "hobbies");
    }

    #[test]
    fn replaces_underscores_with_spaces() {
        let title =
            derive_note_title("aleph://user/preferences/dark_mode", &NoteType::Preference);
        assert_eq!(title, "dark mode");
    }

    #[test]
    fn falls_back_to_note_type_for_empty_path() {
        let title = derive_note_title("", &NoteType::Learning);
        assert_eq!(title, "learning");
    }

    #[test]
    fn maps_note_types_to_categories() {
        assert_eq!(note_type_to_category(&NoteType::Preference), "preference");
        assert_eq!(note_type_to_category(&NoteType::Plan), "plan");
        assert_eq!(note_type_to_category(&NoteType::Learning), "learning");
        assert_eq!(note_type_to_category(&NoteType::Project), "project");
        assert_eq!(note_type_to_category(&NoteType::Personal), "personal");
        assert_eq!(note_type_to_category(&NoteType::Tool), "tool");
        assert_eq!(note_type_to_category(&NoteType::Lesson), "lesson");
        assert_eq!(note_type_to_category(&NoteType::Skill), "skill");
        assert_eq!(note_type_to_category(&NoteType::Wiki), "wiki");
        assert_eq!(note_type_to_category(&NoteType::Other), "other");
        assert_eq!(note_type_to_category(&NoteType::Transcript), "other");
    }

    #[tokio::test]
    async fn migrate_wiki_files_copies_md_files() {
        let tmp = tempfile::tempdir().unwrap();
        let old_wiki = tmp.path().join("wiki").join("default");
        std::fs::create_dir_all(&old_wiki).unwrap();
        std::fs::write(old_wiki.join("rust-ownership.md"), "# Rust Ownership").unwrap();
        std::fs::write(old_wiki.join("llm-prompts.md"), "# LLM Prompts").unwrap();
        std::fs::write(old_wiki.join("notes.txt"), "not a markdown file").unwrap();

        let memory_dir = tmp.path().join("memory");
        let count = migrate_wiki_files(&old_wiki, &memory_dir, "default")
            .await
            .unwrap();
        assert_eq!(count, 2);

        let dest = memory_dir.join("default").join("wiki");
        assert!(dest.join("rust-ownership.md").exists());
        assert!(dest.join("llm-prompts.md").exists());
        assert!(!dest.join("notes.txt").exists());
    }

    #[tokio::test]
    async fn migrate_wiki_files_skips_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let old_wiki = tmp.path().join("wiki").join("default");
        std::fs::create_dir_all(&old_wiki).unwrap();
        std::fs::write(old_wiki.join("page.md"), "old content").unwrap();

        let memory_dir = tmp.path().join("memory");
        let dest = memory_dir.join("default").join("wiki");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("page.md"), "new content").unwrap();

        let count = migrate_wiki_files(&old_wiki, &memory_dir, "default")
            .await
            .unwrap();
        assert_eq!(count, 0);

        // Existing content should not be overwritten
        let content = std::fs::read_to_string(dest.join("page.md")).unwrap();
        assert_eq!(content, "new content");
    }

    #[tokio::test]
    async fn migrate_wiki_files_handles_missing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let nonexistent = tmp.path().join("no-such-dir");
        let memory_dir = tmp.path().join("memory");

        let count = migrate_wiki_files(&nonexistent, &memory_dir, "default")
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
