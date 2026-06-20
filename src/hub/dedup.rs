//! Cross-source dedup: the same upstream extension surfaced by multiple hubs
//! (Aleph Hub, ClawHub, Hermes Atlas, …) collapses to the highest-priority
//! source. Keyed on the normalized upstream `repo_url` — the open-source
//! provenance the contract already mandates. Applied at read time so the cache
//! stays per-source and dedup always reflects the latest catalog.

use std::collections::HashMap;

use crate::hub::types::ExtensionEntry;

/// Default source priority (earlier = higher). Overridable via config later.
pub const DEFAULT_HUB_PRIORITY: &[&str] = &["aleph-hub", "clawhub", "hermes-atlas"];

/// Normalized upstream identity, or `None` when there is no resolvable upstream
/// (such entries are never cross-deduped).
#[must_use]
pub fn dedup_key(entry: &ExtensionEntry) -> Option<String> {
    let raw = entry.repo_url.as_deref()?.trim().to_ascii_lowercase();
    let s = raw
        .strip_prefix("https://")
        .or_else(|| raw.strip_prefix("http://"))
        .unwrap_or(&raw);
    let s = s.strip_suffix('/').unwrap_or(s);
    let s = s.strip_suffix(".git").unwrap_or(s);
    let s = s.strip_suffix('/').unwrap_or(s);
    Some(s.to_string())
}

#[must_use]
fn rank(source_id: &str, order: &[String]) -> usize {
    order.iter().position(|s| s == source_id).unwrap_or(usize::MAX)
}

/// Collapse cross-source duplicates, keeping the best-ranked source per
/// upstream. Entries without a dedup key pass through untouched. Stable: a
/// winner keeps its first-seen position; non-keyed entries are appended.
#[must_use]
pub fn dedup_by_priority(entries: Vec<ExtensionEntry>, order: &[String]) -> Vec<ExtensionEntry> {
    let mut idx_of: HashMap<String, usize> = HashMap::new();
    let mut winners: Vec<ExtensionEntry> = Vec::new();
    let mut passthrough: Vec<ExtensionEntry> = Vec::new();
    for e in entries {
        match dedup_key(&e) {
            None => passthrough.push(e),
            Some(k) => match idx_of.get(&k).copied() {
                Some(i) => {
                    let cur = rank(&winners[i].source_id, order);
                    let new = rank(&e.source_id, order);
                    // Lower rank wins; tie-break on source_id for determinism.
                    if new < cur || (new == cur && e.source_id < winners[i].source_id) {
                        winners[i] = e;
                    }
                }
                None => {
                    idx_of.insert(k, winners.len());
                    winners.push(e);
                }
            },
        }
    }
    winners.append(&mut passthrough);
    winners
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::types::{ExtensionCategory, ExtensionKind, TrustTier};

    fn e(id: &str, src: &str, repo: Option<&str>) -> ExtensionEntry {
        ExtensionEntry {
            id: id.into(),
            kind: ExtensionKind::Mcp,
            category: ExtensionCategory::Other,
            name: id.into(),
            description: String::new(),
            author: None,
            icon: None,
            tags: vec![],
            version: None,
            source_id: src.into(),
            repo_url: repo.map(Into::into),
            trust_tier: TrustTier::Community,
            requires_config: false,
            config_schema: None,
            installed: false,
            enabled: false,
            update_available: false,
            via: None,
            install_spec: None,
        }
    }

    fn order() -> Vec<String> {
        DEFAULT_HUB_PRIORITY.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn keeps_highest_priority_duplicate() {
        let input = vec![
            e("clawhub:foo", "clawhub", Some("https://github.com/acme/foo")),
            e("aleph-hub:foo", "aleph-hub", Some("https://github.com/acme/foo.git")),
            e("hermes-atlas:foo", "hermes-atlas", Some("https://github.com/acme/foo/")),
            e("docker-mcp:bar", "docker-mcp", None), // no repo → never deduped
        ];
        let out = dedup_by_priority(input, &order());
        assert_eq!(out.len(), 2);
        let foo = out.iter().find(|x| x.repo_url.is_some()).unwrap();
        // .git / trailing-slash variants normalize to the same key; aleph-hub wins.
        assert_eq!(foo.source_id, "aleph-hub");
        assert!(out.iter().any(|x| x.source_id == "docker-mcp"));
    }

    #[test]
    fn unlisted_sources_rank_below_listed() {
        let input = vec![
            e("mcp-official:x", "mcp-official", Some("https://github.com/a/x")),
            e("aleph-hub:x", "aleph-hub", Some("https://github.com/a/x")),
        ];
        let out = dedup_by_priority(input, &order());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source_id, "aleph-hub");
    }
}
