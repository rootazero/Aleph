//! Pure view-model for the Extensions store: functional-category facets, client-side
//! filtering, featured and per-category-shelf grouping, and trust/kind →
//! design-token class maps.
//!
//! Filtering is client-side because the browse view fetches the whole catalog
//! once and then filters instantly as chips change; the server has no trust
//! facet. Its free-text search (`hub::cache::matches_query`, which the
//! `hub_catalog_search` tool uses) covers the same fields as [`matches`] here —
//! keep the two in step if either grows a field.
use crate::api::extensions::ExtensionEntry;

pub struct CategoryFacet {
    pub value: &'static str,     // snake_case wire category
    pub label_key: &'static str, // i18n key under `extensions.cat`
    pub emoji: &'static str,
}

/// Primary browse taxonomy (spec §12). Order here is the shelf/chip display order.
pub const CATEGORIES: &[CategoryFacet] = &[
    CategoryFacet {
        value: "search",
        label_key: "extensions.cat.search",
        emoji: "🌐",
    },
    CategoryFacet {
        value: "developer",
        label_key: "extensions.cat.developer",
        emoji: "🛠",
    },
    CategoryFacet {
        value: "productivity",
        label_key: "extensions.cat.productivity",
        emoji: "⚡",
    },
    CategoryFacet {
        value: "writing",
        label_key: "extensions.cat.writing",
        emoji: "✍",
    },
    CategoryFacet {
        value: "communication",
        label_key: "extensions.cat.communication",
        emoji: "💬",
    },
    CategoryFacet {
        value: "knowledge",
        label_key: "extensions.cat.knowledge",
        emoji: "📚",
    },
    CategoryFacet {
        value: "files",
        label_key: "extensions.cat.files",
        emoji: "📁",
    },
    CategoryFacet {
        value: "design",
        label_key: "extensions.cat.design",
        emoji: "🎨",
    },
    CategoryFacet {
        value: "automation",
        label_key: "extensions.cat.automation",
        emoji: "🔁",
    },
    CategoryFacet {
        value: "finance",
        label_key: "extensions.cat.finance",
        emoji: "💰",
    },
    CategoryFacet {
        value: "utilities",
        label_key: "extensions.cat.utilities",
        emoji: "🧰",
    },
    CategoryFacet {
        value: "other",
        label_key: "extensions.cat.other",
        emoji: "•",
    },
];

#[derive(Debug, Clone, PartialEq)]
pub struct Filters {
    pub category: String, // "featured" | "all" | one of CATEGORIES.value
    pub kind: String,     // "all" | "skill" | "plugin" | "mcp"
    pub trust: String,    // "all" | "official" | "verified" | "community" | "unverified"
    pub query: String,
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            category: "featured".into(),
            kind: "all".into(),
            trust: "all".into(),
            query: String::new(),
        }
    }
}

#[must_use]
pub fn matches(e: &ExtensionEntry, f: &Filters) -> bool {
    let cat_ok = f.category == "featured" || f.category == "all" || e.category == f.category;
    let kind_ok = f.kind == "all" || e.kind == f.kind;
    let trust_ok = f.trust == "all" || e.trust_tier == f.trust;
    let query_ok = f.query.trim().is_empty() || {
        let q = f.query.to_lowercase();
        e.name.to_lowercase().contains(&q)
            || e.description.to_lowercase().contains(&q)
            || e.tags.iter().any(|t| t.to_lowercase().contains(&q))
    };
    cat_ok && kind_ok && trust_ok && query_ok
}

#[must_use]
pub fn apply_filters(entries: &[ExtensionEntry], f: &Filters) -> Vec<ExtensionEntry> {
    entries.iter().filter(|e| matches(e, f)).cloned().collect()
}

/// v1 deterministic stand-in for the Store Agent's editorial picks (P4 replaces this):
/// Official+Verified tiers, sorted by name, capped.
#[must_use]
pub fn featured_picks(entries: &[ExtensionEntry], max: usize) -> Vec<ExtensionEntry> {
    let mut v: Vec<ExtensionEntry> = entries
        .iter()
        .filter(|e| e.trust_tier == "official" || e.trust_tier == "verified")
        .cloned()
        .collect();
    v.sort_by_key(|a| a.name.to_lowercase());
    v.truncate(max);
    v
}

/// Group entries into per-category shelves in CATEGORIES order; skip empty categories.
#[must_use]
pub fn group_into_shelves(entries: &[ExtensionEntry]) -> Vec<(&'static str, Vec<ExtensionEntry>)> {
    CATEGORIES
        .iter()
        .filter_map(|c| {
            let items: Vec<ExtensionEntry> = entries
                .iter()
                .filter(|e| e.category == c.value)
                .cloned()
                .collect();
            (!items.is_empty()).then_some((c.value, items))
        })
        .collect()
}

#[must_use]
pub fn kind_badge_class(kind: &str) -> &'static str {
    match kind {
        "skill" => "bg-success-subtle text-success",
        "plugin" => "bg-primary-subtle text-primary",
        "mcp" => "bg-info-subtle text-info",
        _ => "bg-surface-sunken text-text-secondary",
    }
}

#[must_use]
pub fn trust_dot_class(tier: &str) -> &'static str {
    match tier {
        "official" => "bg-primary",
        "verified" => "bg-success",
        "community" => "bg-text-tertiary",
        "unverified" => "bg-warning",
        _ => "bg-text-tertiary",
    }
}

#[must_use]
pub fn risk_banner_class(risk: &str) -> &'static str {
    match risk {
        "runs_commands" => "bg-danger-subtle text-danger border-danger/30",
        "remote_endpoint" | "instructs_agent" => "bg-warning-subtle text-warning border-warning/30",
        _ => "bg-info-subtle text-info border-info/30",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::extensions::ExtensionEntry;

    fn e(
        id: &str,
        kind: &str,
        cat: &str,
        trust: &str,
        name: &str,
        desc: &str,
        tags: &[&str],
    ) -> ExtensionEntry {
        ExtensionEntry {
            id: id.into(),
            kind: kind.into(),
            category: cat.into(),
            name: name.into(),
            description: desc.into(),
            author: None,
            icon: None,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            version: None,
            source_id: "s".into(),
            trust_tier: trust.into(),
            repo_url: None,
            requires_config: false,
            config_schema: None,
            installed: false,
            enabled: false,
            update_available: false,
            source_label: String::new(),
        }
    }

    #[test]
    fn matches_category_kind_trust_query() {
        let item = e(
            "a",
            "mcp",
            "developer",
            "community",
            "GitHub",
            "Manage repos",
            &["git"],
        );
        // category facet
        assert!(matches(
            &item,
            &Filters {
                category: "developer".into(),
                ..Default::default()
            }
        ));
        assert!(!matches(
            &item,
            &Filters {
                category: "data".into(),
                ..Default::default()
            }
        ));
        // "featured"/"all" are pass-through category facets
        assert!(matches(
            &item,
            &Filters {
                category: "featured".into(),
                ..Default::default()
            }
        ));
        assert!(matches(
            &item,
            &Filters {
                category: "all".into(),
                ..Default::default()
            }
        ));
        // kind (secondary)
        assert!(matches(
            &item,
            &Filters {
                kind: "mcp".into(),
                ..Default::default()
            }
        ));
        assert!(!matches(
            &item,
            &Filters {
                kind: "skill".into(),
                ..Default::default()
            }
        ));
        // trust filtered CLIENT-SIDE (server has no trust filter)
        assert!(matches(
            &item,
            &Filters {
                trust: "community".into(),
                ..Default::default()
            }
        ));
        assert!(!matches(
            &item,
            &Filters {
                trust: "official".into(),
                ..Default::default()
            }
        ));
        // query over name OR description OR tags, case-insensitive
        assert!(matches(
            &item,
            &Filters {
                query: "github".into(),
                ..Default::default()
            }
        ));
        assert!(matches(
            &item,
            &Filters {
                query: "REPOS".into(),
                ..Default::default()
            }
        ));
        assert!(matches(
            &item,
            &Filters {
                query: "git".into(),
                ..Default::default()
            }
        ));
        assert!(!matches(
            &item,
            &Filters {
                query: "zzz".into(),
                ..Default::default()
            }
        ));
    }

    #[test]
    fn featured_prefers_official_verified_capped_sorted() {
        let items = vec![
            e("c", "mcp", "data", "community", "Zeta", "", &[]),
            e("a", "mcp", "data", "official", "Beta", "", &[]),
            e("b", "skill", "writing", "verified", "Alpha", "", &[]),
        ];
        let f = featured_picks(&items, 2);
        assert_eq!(f.len(), 2);
        // official+verified only, sorted by name → Alpha, Beta
        assert_eq!(f[0].name, "Alpha");
        assert_eq!(f[1].name, "Beta");
    }

    #[test]
    fn shelves_skip_empty_and_follow_category_order() {
        let items = vec![
            e("a", "mcp", "developer", "official", "A", "", &[]),
            e("b", "mcp", "search", "official", "B", "", &[]),
        ];
        let shelves = group_into_shelves(&items);
        // CATEGORIES order has search before developer → search shelf first
        assert_eq!(shelves[0].0, "search");
        assert_eq!(shelves[1].0, "developer");
        assert_eq!(shelves.len(), 2); // no empty shelves for the other 11 categories
    }

    #[test]
    fn class_maps_cover_all_known_values() {
        assert_eq!(trust_dot_class("official"), "bg-primary");
        assert_eq!(trust_dot_class("verified"), "bg-success");
        assert_eq!(trust_dot_class("community"), "bg-text-tertiary");
        assert_eq!(trust_dot_class("unverified"), "bg-warning");
        assert_eq!(kind_badge_class("skill"), "bg-success-subtle text-success");
        assert_eq!(kind_badge_class("plugin"), "bg-primary-subtle text-primary");
        assert_eq!(kind_badge_class("mcp"), "bg-info-subtle text-info");
        assert_eq!(
            risk_banner_class("runs_commands"),
            "bg-danger-subtle text-danger border-danger/30"
        );
    }
}
