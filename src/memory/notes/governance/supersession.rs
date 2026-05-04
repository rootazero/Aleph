//! Frontmatter ↔ body bidirectional supersession sync.
//!
//! The `superseded_by` frontmatter list is the source of truth for query-time
//! filtering. Authors and earlier prose-style notes may instead express the
//! same relationship as a `## Superseded by [[target]]` body section. This
//! module keeps the two representations in lockstep so both authoring styles
//! produce equivalent stored state.

use crate::memory::notes::KnowledgeNote;

static SUPERSEDED_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?m)^## Superseded by \[\[([^\]]+)\]\]\s*$").unwrap()
});

/// Read the rendered markdown for `## Superseded by [[X]]` headings and merge
/// the targets into `note.superseded_by` (set-union semantics, then sorted for
/// deterministic order).
pub fn sync_body_to_frontmatter(note: &mut KnowledgeNote, full_markdown: &str) {
    let mut existing: std::collections::HashSet<String> =
        note.superseded_by.iter().cloned().collect();
    for cap in SUPERSEDED_RE.captures_iter(full_markdown) {
        existing.insert(cap[1].to_string());
    }
    note.superseded_by = existing.into_iter().collect();
    note.superseded_by.sort();
}

/// Append `## Superseded by [[X]]` sections to the markdown for any
/// `superseded_by` entry not already present. Idempotent: re-running the
/// function on already-synced markdown does not duplicate sections.
pub fn ensure_supersession_section(markdown: &str, note: &KnowledgeNote) -> String {
    let mut out = markdown.to_string();
    for target in &note.superseded_by {
        let line = format!("## Superseded by [[{target}]]");
        if !markdown.contains(&line) {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::KnowledgeNote;

    #[test]
    fn body_section_promotes_to_frontmatter() {
        let md = "---\ncategory: preference\ntags: []\n---\n\n- claim\n\n## Superseded by [[preference/new]]\n";
        let mut n = KnowledgeNote::from_markdown("old", md).unwrap();
        sync_body_to_frontmatter(&mut n, md);
        assert_eq!(n.superseded_by, vec!["preference/new".to_string()]);
    }

    #[test]
    fn frontmatter_emits_body_section_on_write() {
        let n = KnowledgeNote {
            title: "old".into(),
            category: "preference".into(),
            facts: vec!["claim".into()],
            superseded_by: vec!["preference/new".into()],
            ..Default::default()
        };
        let md = n.to_markdown();
        let with_section = ensure_supersession_section(&md, &n);
        assert!(with_section.contains("## Superseded by [[preference/new]]"));
    }

    #[test]
    fn idempotent_when_already_synced() {
        let md = "---\ncategory: preference\ntags: []\nsuperseded_by: [preference/new]\n---\n\n- x\n\n## Superseded by [[preference/new]]\n";
        let n = KnowledgeNote::from_markdown("old", md).unwrap();
        let again = ensure_supersession_section(&md, &n);
        let count = again.matches("## Superseded by [[preference/new]]").count();
        assert_eq!(count, 1);
    }

    #[test]
    fn body_section_sync_unions_with_existing_frontmatter() {
        let md = "---\ncategory: preference\ntags: []\nsuperseded_by: [preference/a]\n---\n\n- x\n\n## Superseded by [[preference/b]]\n";
        let mut n = KnowledgeNote::from_markdown("old", md).unwrap();
        sync_body_to_frontmatter(&mut n, md);
        // Both 'a' (from frontmatter) and 'b' (from body section) must be present.
        assert!(n.superseded_by.contains(&"preference/a".to_string()));
        assert!(n.superseded_by.contains(&"preference/b".to_string()));
    }
}
