//! The one matcher every provider/model picker filters through.
//!
//! # Why a shared function and not one per client
//!
//! The Panel and the TUI both need "find me the provider I mean" over the same
//! rows, and a picker's *first* row is the one a bare Enter selects. Two
//! independently written matchers therefore do not merely look different — they
//! disagree about what confirming does, which is the difference between opening
//! the provider you typed and opening a different one whose description happens
//! to mention the word.
//!
//! # Two decisions this deliberately keeps
//!
//! **Substring, not fuzzy.** `providers.catalog` returns rows in a curated
//! order (default first, verified first) and each row's roster in ladder order.
//! Scoring by subsequence quality shuffles both into near-alphabetical noise.
//! The reference implementation this was compared against (pi's `fuzzyFilter`)
//! needs a second, differently-ordered search string per item purely to undo
//! its own positional penalty; with substring matching that problem does not
//! exist.
//!
//! **Tiered, not flat.** An exact id has to outrank a mention. This is the same
//! lesson the TUI command palette already paid for: its filter matched an
//! entry's description as well as its name, so typing `mode` selected `/tools`
//! — whose hint reads "Tool progress mode: …" — and confirming ran the wrong
//! command with the wrong argument. Ranking is a stable sort, so the curated
//! order survives inside each tier.

use super::catalog::RosterModel;
use super::wire::CatalogEntry;

/// How well a row matched, best first. Lower sorts earlier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchRank {
    /// The query is exactly the provider id (or one of its aliases).
    ExactId = 0,
    /// The provider id starts with the query.
    IdPrefix = 1,
    /// An alias starts with, or the id contains, the query.
    IdSubstring = 2,
    /// The human-facing display name matched.
    DisplayName = 3,
    /// Only a model id in the roster matched — the provider itself did not.
    ModelOnly = 4,
}

/// A matched catalogue row.
#[derive(Debug, Clone, PartialEq)]
pub struct EntryMatch {
    /// Position in the input slice, so a caller can map back without cloning.
    pub index: usize,
    pub rank: MatchRank,
    /// The roster to show. Identical to the entry's roster when the provider
    /// itself matched; narrowed to the matching ids when it did not — a
    /// provider surfaced *because* one model matched should not then hide
    /// which one.
    pub models: Vec<RosterModel>,
}

/// Rank one entry against a lowercased, trimmed query.
///
/// Returns `None` when nothing in the row matches.
fn rank_one(entry: &CatalogEntry, q: &str) -> Option<(MatchRank, Vec<RosterModel>)> {
    let id = entry.id.to_lowercase();
    let aliases: Vec<String> = entry.aliases.iter().map(|a| a.to_lowercase()).collect();

    if id == q || aliases.iter().any(|a| a == q) {
        return Some((MatchRank::ExactId, entry.roster.clone()));
    }
    if id.starts_with(q) {
        return Some((MatchRank::IdPrefix, entry.roster.clone()));
    }
    if id.contains(q) || aliases.iter().any(|a| a.starts_with(q)) {
        return Some((MatchRank::IdSubstring, entry.roster.clone()));
    }
    if entry.display_name.to_lowercase().contains(q) {
        return Some((MatchRank::DisplayName, entry.roster.clone()));
    }

    let matched: Vec<RosterModel> = entry
        .roster
        .iter()
        .filter(|m| m.id.to_lowercase().contains(q))
        .cloned()
        .collect();
    if matched.is_empty() {
        None
    } else {
        Some((MatchRank::ModelOnly, matched))
    }
}

/// Filter and rank a catalogue.
///
/// An empty or whitespace-only query keeps every row, in the order given, with
/// every roster intact — so "no filter" is behaviourally identical to not
/// calling this at all.
#[must_use]
pub fn rank_entries(entries: &[CatalogEntry], query: &str) -> Vec<EntryMatch> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return entries
            .iter()
            .enumerate()
            .map(|(index, e)| EntryMatch {
                index,
                rank: MatchRank::ExactId,
                models: e.roster.clone(),
            })
            .collect();
    }

    let mut matches: Vec<EntryMatch> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, e)| {
            rank_one(e, &q).map(|(rank, models)| EntryMatch {
                index,
                rank,
                models,
            })
        })
        .collect();

    // Stable: the curated order survives within each tier.
    matches.sort_by_key(|m| m.rank);
    matches
}

/// Filter a catalogue into owned rows, rosters narrowed to the matching ids.
///
/// The convenience form for renderers that hold owned rows anyway.
#[must_use]
pub fn filter_catalog(entries: &[CatalogEntry], query: &str) -> Vec<CatalogEntry> {
    rank_entries(entries, query)
        .into_iter()
        .map(|m| CatalogEntry {
            roster: m.models,
            ..entries[m.index].clone()
        })
        .collect()
}

/// Flatten a catalogue into `(provider_id, model)` pairs matching a query.
///
/// A model picker searches across providers, so `gpt-5` must find OpenAI's row
/// *and* the relay that resells it. When the provider itself matched, every one
/// of its models is a candidate; when only a model matched, only that model is.
#[must_use]
pub fn rank_models<'a>(
    entries: &'a [CatalogEntry],
    query: &str,
) -> Vec<(&'a CatalogEntry, RosterModel)> {
    rank_entries(entries, query)
        .into_iter()
        .flat_map(|m| {
            let entry = &entries[m.index];
            m.models.into_iter().map(move |model| (entry, model))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::catalog::{ModelSource, RosterModel};
    use super::*;

    fn entry(id: &str, display: &str, aliases: &[&str], models: &[&str]) -> CatalogEntry {
        CatalogEntry {
            id: id.into(),
            display_name: display.into(),
            default_model: models.first().copied().unwrap_or_default().into(),
            base_url: String::new(),
            protocol: "openai".into(),
            color: String::new(),
            homepage: None,
            notes: None,
            signup_url: None,
            fallback_models: Vec::new(),
            default_aux_model: None,
            aliases: aliases.iter().map(|s| (*s).to_string()).collect(),
            modalities: Vec::new(),
            models: Vec::new(),
            has_api_key: false,
            verified: false,
            enabled: false,
            is_default: false,
            auth_kind: super::super::wire::AuthKind::ApiKey,
            capabilities: None,
            cost: None,
            endpoint: "cloud".into(),
            lifecycle: super::super::catalog::ModelLifecycle::ACTIVE,
            requires_explicit_model: false,
            discoverable: true,
            roster: models
                .iter()
                .map(|m| RosterModel::new(*m, ModelSource::PresetFallback))
                .collect(),
        }
    }

    fn catalog() -> Vec<CatalogEntry> {
        vec![
            // Deliberately NOT alphabetical — this is the curated order the
            // ranker must not shuffle within a tier.
            entry("openrouter", "OpenRouter", &[], &["openai/gpt-5.6"]),
            entry("openai", "OpenAI", &[], &["gpt-5.6", "gpt-5.6-luna"]),
            entry("moonshot", "Moonshot", &["kimi"], &["kimi-k2.6"]),
        ]
    }

    #[test]
    fn an_exact_id_outranks_a_provider_that_merely_contains_it() {
        let c = catalog();
        let ranked = rank_entries(&c, "openai");
        // `openrouter` is listed first and its model id contains "openai",
        // but the exact id has to win — a bare Enter selects row 0.
        assert_eq!(c[ranked[0].index].id, "openai");
        assert_eq!(ranked[0].rank, MatchRank::ExactId);
        assert_eq!(c[ranked[1].index].id, "openrouter");
        assert_eq!(ranked[1].rank, MatchRank::ModelOnly);
    }

    #[test]
    fn an_alias_finds_the_canonical_row() {
        let c = catalog();
        let ranked = rank_entries(&c, "kimi");
        assert_eq!(c[ranked[0].index].id, "moonshot");
        assert_eq!(ranked[0].rank, MatchRank::ExactId);
    }

    #[test]
    fn curated_order_survives_inside_a_tier() {
        let c = catalog();
        // Both `openrouter` and `openai` start with "open"; the input order
        // must be preserved rather than sorted.
        let ranked = rank_entries(&c, "open");
        let ids: Vec<&str> = ranked
            .iter()
            .map(|m| c[m.index].id.as_str())
            .take(2)
            .collect();
        assert_eq!(ids, vec!["openrouter", "openai"]);
    }

    #[test]
    fn a_model_only_match_narrows_the_roster() {
        let c = catalog();
        let ranked = rank_entries(&c, "luna");
        assert_eq!(ranked.len(), 1);
        assert_eq!(c[ranked[0].index].id, "openai");
        assert_eq!(ranked[0].rank, MatchRank::ModelOnly);
        assert_eq!(
            ranked[0].models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["gpt-5.6-luna"]
        );
    }

    #[test]
    fn a_provider_match_keeps_every_model() {
        let c = catalog();
        let filtered = filter_catalog(&c, "openai");
        assert_eq!(filtered[0].roster.len(), 2);
    }

    #[test]
    fn an_empty_query_is_a_no_op() {
        let c = catalog();
        assert_eq!(filter_catalog(&c, "   "), c);
    }

    #[test]
    fn model_search_spans_providers() {
        let c = catalog();
        let hits = rank_models(&c, "gpt-5.6");
        let pairs: Vec<(&str, &str)> = hits
            .iter()
            .map(|(e, m)| (e.id.as_str(), m.id.as_str()))
            .collect();
        assert!(pairs.contains(&("openai", "gpt-5.6")));
        assert!(pairs.contains(&("openrouter", "openai/gpt-5.6")));
    }
}
