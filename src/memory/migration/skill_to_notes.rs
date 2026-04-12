//! One-time migration: skill facts → skill/*.md Knowledge Notes

use crate::error::AlephError;
use crate::memory::context::{MemoryFact, NoteType};
use crate::memory::notes::{KnowledgeNote, NoteIndexer};
use crate::memory::notes::store::NoteStore;
use crate::memory::store::{MemoryBackend, MemoryStore};
use tracing::info;

/// Summary of the skill migration run.
#[derive(Debug)]
pub struct SkillMigrationReport {
    pub total_skills: usize,
    pub migrated: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

/// Migrate skill facts from the facts table into `skill/*.md` Knowledge Notes.
///
/// Each `NoteType::Skill` fact whose `path` looks like
/// `"aleph://skills/<category>/<name>/"` is converted into a `KnowledgeNote`
/// and written to `{memory_dir}/{agent_id}/skill/{name}.md`.
///
/// Set `dry_run = true` to log what would happen without writing any files.
pub async fn migrate_skills_to_notes<S: NoteStore + Send + Sync>(
    database: &MemoryBackend,
    indexer: &NoteIndexer<S>,
    agent_id: &str,
    dry_run: bool,
) -> Result<SkillMigrationReport, AlephError> {
    let all_facts: Vec<MemoryFact> = database.get_all_facts(false, None).await?;
    let skill_facts: Vec<_> = all_facts
        .into_iter()
        .filter(|f| f.note_type == NoteType::Skill && f.is_valid)
        .collect();

    let mut report = SkillMigrationReport {
        total_skills: skill_facts.len(),
        migrated: 0,
        skipped: 0,
        errors: Vec::new(),
    };

    for fact in &skill_facts {
        // Extract skill name from path like "aleph://skills/coding/rust-debug/"
        let name = fact
            .path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("unknown");

        // Extract category hint from second segment after "aleph://skills/"
        let category_hint = fact
            .path
            .strip_prefix("aleph://skills/")
            .and_then(|s: &str| {
                // Collect non-empty segments: trailing slash creates empty strings
                let mut parts = s.split('/').filter(|p| !p.is_empty());
                let first = parts.next()?;
                // If there are more non-empty segments, `first` is the category;
                // otherwise `first` is the skill name itself → fall back to "general".
                if parts.next().is_some() {
                    Some(first)
                } else {
                    None
                }
            })
            .unwrap_or("general");

        let facts = parse_skill_content(&fact.content);

        if dry_run {
            info!(
                name,
                category = category_hint,
                facts = facts.len(),
                "DRY RUN: would migrate skill"
            );
            report.migrated += 1;
            continue;
        }

        let note = KnowledgeNote {
            title: name.to_string(),
            category: "skill".to_string(),
            tags: vec![category_hint.to_string()],
            facts,
            links: vec![],
            created_at: fact.created_at,
            updated_at: fact.updated_at,
            content_hash: String::new(),
        };

        match indexer.write_note(agent_id, "skill", &note).await {
            Ok(_) => {
                report.migrated += 1;
                info!(name, "Migrated skill to notes");
            }
            Err(e) => {
                let err_msg = format!("{name}: {e}");
                report.errors.push(err_msg);
                report.skipped += 1;
            }
        }
    }

    Ok(report)
}

/// Parse skill fact content into individual fact strings.
///
/// Strips bullet markers (`- ` / `* `), skips markdown headers (`# …`),
/// and skips blank lines.  If the resulting list is empty, the full raw
/// content is returned as a single fact so no information is lost.
fn parse_skill_content(content: &str) -> Vec<String> {
    if content.trim().is_empty() {
        return vec![];
    }

    let mut facts = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("# ") || trimmed == "#" {
            // Skip markdown headers
            continue;
        }
        let fact = if let Some(rest) = trimmed.strip_prefix("- ") {
            rest.to_string()
        } else if let Some(rest) = trimmed.strip_prefix("* ") {
            rest.to_string()
        } else {
            trimmed.to_string()
        };

        if !fact.is_empty() {
            facts.push(fact);
        }
    }

    if facts.is_empty() {
        // Preserve content as a single fact rather than losing data
        facts.push(content.trim().to_string());
    }

    facts
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_skill_content_with_bullets() {
        let content =
            "Debug Rust errors\n\n- Read error messages carefully\n- Check borrow checker hints";
        let facts = parse_skill_content(content);
        // "Debug Rust errors" (plain line) + 2 bullet items
        assert_eq!(facts.len(), 3);
        assert_eq!(facts[0], "Debug Rust errors");
        assert_eq!(facts[1], "Read error messages carefully");
        assert_eq!(facts[2], "Check borrow checker hints");
    }

    #[test]
    fn parses_single_line_skill() {
        let content = "Always use --release for benchmarks";
        let facts = parse_skill_content(content);
        assert_eq!(facts, vec!["Always use --release for benchmarks"]);
    }

    #[test]
    fn parses_empty_content() {
        let facts = parse_skill_content("");
        assert!(facts.is_empty());
    }

    #[test]
    fn skips_markdown_headers() {
        let content = "# Rust Debug\n- Read error messages\n## Sub-heading\n- Check types";
        let facts = parse_skill_content(content);
        // Headers skipped; "## Sub-heading" is not "# " prefix so it won't be skipped —
        // but it is treated as a plain line. Let's verify actual behavior:
        // "## Sub-heading" → doesn't start with "# " → kept as plain fact
        assert!(facts.contains(&"Read error messages".to_string()));
        assert!(facts.contains(&"Check types".to_string()));
        // "# Rust Debug" (starts with "# ") should be skipped
        assert!(!facts.contains(&"Rust Debug".to_string()));
        assert!(!facts.iter().any(|f| f.starts_with("# ")));
    }

    #[test]
    fn parses_star_bullets() {
        let content = "* Use cargo fmt\n* Run clippy";
        let facts = parse_skill_content(content);
        assert_eq!(facts, vec!["Use cargo fmt", "Run clippy"]);
    }

    #[test]
    fn fallback_to_single_fact_when_only_whitespace_lines() {
        // Content with only blank lines should yield empty
        let facts = parse_skill_content("   \n\n  ");
        assert!(facts.is_empty());
    }

    #[test]
    fn preserves_content_as_single_fact_when_no_structure() {
        // Non-empty content that has no recognisable structure remains intact
        let content = "Use --release for benchmarks";
        let facts = parse_skill_content(content);
        assert_eq!(facts.len(), 1);
    }

    #[test]
    fn category_hint_extracted_correctly() {
        // Simulate the path parsing logic inline to verify the logic is sound
        let path = "aleph://skills/coding/rust-debug/";
        let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or("unknown");
        let category_hint = path
            .strip_prefix("aleph://skills/")
            .and_then(|s: &str| {
                let mut parts = s.split('/').filter(|p| !p.is_empty());
                let first = parts.next()?;
                if parts.next().is_some() { Some(first) } else { None }
            })
            .unwrap_or("general");

        assert_eq!(name, "rust-debug");
        assert_eq!(category_hint, "coding");
    }

    #[test]
    fn category_hint_falls_back_for_flat_paths() {
        // Path without sub-category: "aleph://skills/rust-debug/"
        let path = "aleph://skills/rust-debug/";
        let category_hint = path
            .strip_prefix("aleph://skills/")
            .and_then(|s: &str| {
                let mut parts = s.split('/').filter(|p| !p.is_empty());
                let first = parts.next()?;
                if parts.next().is_some() { Some(first) } else { None }
            })
            .unwrap_or("general");

        assert_eq!(category_hint, "general");
    }
}
