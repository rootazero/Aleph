//! Pure helpers for surfacing note relationships at retrieval time:
//! force-surface structural-strong targets and render a compact backlink
//! footer. No IO — the caller fetches relations/backlinks and applies these.

use std::collections::HashSet;

use crate::memory::notes::is_structural_strong;

/// Structural-strong targets (target_path, rel_type) of one note, excluding any
/// path already present in the result set. Order preserved, deduped by path.
#[must_use]
pub fn structural_targets(
    relations: &[(String, String)],
    already: &HashSet<String>,
) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    relations
        .iter()
        .filter(|(to, rel)| is_structural_strong(rel) && !already.contains(to))
        .filter(|(to, _)| seen.insert(to.clone()))
        .cloned()
        .collect()
}

/// Compact one-line relationship footer for a surfaced note, or None when there
/// is nothing to add. `strong_outs` is (target, rel_type) of this note's
/// structural-strong out-edges; `backlink_count` is how many notes link to it.
#[must_use]
pub fn backlink_footer(strong_outs: &[(String, String)], backlink_count: usize) -> Option<String> {
    if strong_outs.is_empty() && backlink_count == 0 {
        return None;
    }
    let mut parts = Vec::new();
    if backlink_count > 0 {
        parts.push(format!("← {backlink_count} backlinks"));
    }
    for (to, rel) in strong_outs {
        parts.push(format!("⚠ {rel} → {to}"));
    }
    Some(format!("[relations] {}", parts.join(" · ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_targets_filters_to_strong_and_unseen() {
        let rels = vec![
            ("plan/old".to_string(), "superseded_by".to_string()),
            ("entity/acme".to_string(), "works_at".to_string()),
            ("plan/dup".to_string(), "contradicts".to_string()),
        ];
        let already: HashSet<String> = ["plan/dup".to_string()].into_iter().collect();
        let got = structural_targets(&rels, &already);
        assert_eq!(
            got,
            vec![("plan/old".to_string(), "superseded_by".to_string())]
        );
    }

    #[test]
    fn footer_none_when_nothing() {
        assert!(backlink_footer(&[], 0).is_none());
    }

    #[test]
    fn footer_renders_backlinks_and_strong() {
        let outs = vec![("plan/old".to_string(), "supersedes".to_string())];
        let f = backlink_footer(&outs, 3).unwrap();
        assert!(f.contains("← 3 backlinks"));
        assert!(f.contains("⚠ supersedes → plan/old"));
    }
}
