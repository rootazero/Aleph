//! Wikilink extraction, rewriting, and resolution for `[[link]]` syntax.

use std::sync::LazyLock;

use regex::Regex;

use crate::memory::notes::store::NoteStore;

/// Regex matching `[[...]]` wikilinks (non-greedy, no nested brackets).
static WIKILINK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[([^\]]+)\]\]").unwrap());

/// Extract all wikilink targets from `text`.
///
/// ```text
/// "See [[Rust Learning]] and [[编辑器偏好]]"
/// → vec!["Rust Learning", "编辑器偏好"]
/// ```
pub fn extract_wikilinks(text: &str) -> Vec<String> {
    WIKILINK_RE
        .captures_iter(text)
        .map(|cap| cap[1].to_string())
        .collect()
}

/// Replace every `[[old_name]]` with `[[new_name]]`, leaving other links intact.
pub fn rewrite_wikilinks(text: &str, old_name: &str, new_name: &str) -> String {
    WIKILINK_RE
        .replace_all(text, |caps: &regex::Captures| {
            if &caps[1] == old_name {
                format!("[[{new_name}]]")
            } else {
                caps[0].to_string()
            }
        })
        .into_owned()
}

/// Delete every `[[name]]` occurrence from `text`, leaving other links intact.
///
/// Used by `NoteLintStage` (D4) to purge wikilinks pointing at notes that
/// no longer exist and have no fuzzy-match candidate. Whitespace around the
/// removed link is intentionally not collapsed — the original surrounding
/// text is preserved verbatim minus the `[[...]]` token.
pub fn remove_wikilink(text: &str, name: &str) -> String {
    WIKILINK_RE
        .replace_all(text, |caps: &regex::Captures| {
            if &caps[1] == name {
                String::new()
            } else {
                caps[0].to_string()
            }
        })
        .into_owned()
}

/// Resolve a wikilink target to a note path using Obsidian-compatible rules.
///
/// 1. If link contains '/' → exact path match
/// 2. If no '/' → global filename search, resolve if exactly one match
/// 3. Returns None if ambiguous or not found
pub async fn resolve_wikilink<S: NoteStore>(
    store: &S,
    link: &str,
    agent_id: &str,
) -> Option<String> {
    if link.contains('/') {
        if store.get_note_index(link, agent_id).await.ok()?.is_some() {
            return Some(link.to_string());
        }
        return None;
    }

    let matches = store.find_by_filename(link, agent_id).await.ok()?;
    if matches.len() == 1 {
        return Some(matches[0].clone());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_wikilinks_from_text() {
        let text = "See [[Rust Learning]] and [[编辑器偏好]]";
        let links = extract_wikilinks(text);
        assert_eq!(links, vec!["Rust Learning", "编辑器偏好"]);
    }

    #[test]
    fn extracts_no_links_from_plain_text() {
        let links = extract_wikilinks("No links here.");
        assert!(links.is_empty());
    }

    #[test]
    fn rewrites_wikilinks() {
        let text = "See [[Old Name]] and [[Keep This]].";
        let result = rewrite_wikilinks(text, "Old Name", "New Name");
        assert_eq!(result, "See [[New Name]] and [[Keep This]].");
    }

    #[test]
    fn rewrite_leaves_unmatched_links_intact() {
        let text = "[[Alpha]] [[Beta]] [[Gamma]]";
        let result = rewrite_wikilinks(text, "Beta", "Delta");
        assert_eq!(result, "[[Alpha]] [[Delta]] [[Gamma]]");
    }

    #[test]
    fn remove_wikilink_drops_named_target() {
        let text = "see [[stale]] and [[keep]]";
        assert_eq!(remove_wikilink(text, "stale"), "see  and [[keep]]");
    }

    #[test]
    fn remove_wikilink_drops_all_occurrences() {
        let text = "[[x]] x [[x]] [[y]]";
        assert_eq!(remove_wikilink(text, "x"), " x  [[y]]");
    }

    #[test]
    fn remove_wikilink_no_op_when_target_absent() {
        let text = "[[a]] [[b]]";
        assert_eq!(remove_wikilink(text, "z"), "[[a]] [[b]]");
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::*;
    use crate::memory::notes::KnowledgeNote;
    use crate::memory::store::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;
    use uuid::Uuid;

    fn create_test_db() -> Arc<SqliteMemoryBackend> {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_resolve_{}", Uuid::new_v4()));
        Arc::new(SqliteMemoryBackend::new(&db_path).unwrap())
    }

    fn make_note(title: &str) -> KnowledgeNote {
        KnowledgeNote {
            title: title.to_string(),
            category: "test".to_string(),
            tags: vec![],
            facts: vec!["fact".to_string()],
            links: vec![],
            created_at: 0,
            updated_at: 0,
            content_hash: format!("h_{title}"),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn resolves_exact_path() {
        let db = create_test_db();
        db.index_note(&make_note("rust"), "default", "reference")
            .await
            .unwrap();

        let result = resolve_wikilink(&*db, "reference/rust", "default").await;
        assert_eq!(result, Some("reference/rust".to_string()));
    }

    #[tokio::test]
    async fn resolves_unique_filename() {
        let db = create_test_db();
        db.index_note(&make_note("rust"), "default", "reference")
            .await
            .unwrap();

        let result = resolve_wikilink(&*db, "rust", "default").await;
        assert_eq!(result, Some("reference/rust".to_string()));
    }

    #[tokio::test]
    async fn returns_none_for_ambiguous() {
        let db = create_test_db();
        db.index_note(&make_note("rust"), "default", "reference")
            .await
            .unwrap();
        db.index_note(&make_note("rust"), "default", "learning")
            .await
            .unwrap();

        let result = resolve_wikilink(&*db, "rust", "default").await;
        assert_eq!(result, None); // ambiguous
    }

    #[tokio::test]
    async fn returns_none_for_not_found() {
        let db = create_test_db();
        let result = resolve_wikilink(&*db, "nonexistent", "default").await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn returns_none_for_wrong_path() {
        let db = create_test_db();
        db.index_note(&make_note("rust"), "default", "reference")
            .await
            .unwrap();

        let result = resolve_wikilink(&*db, "skill/rust", "default").await;
        assert_eq!(result, None); // no note at skill/rust
    }
}
