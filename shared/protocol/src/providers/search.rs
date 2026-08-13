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
//! # Why a trait, and not one function per list
//!
//! Chat providers are not the only catalogue that outgrew scrolling: the
//! generation family ships 44 presets across five modalities and had no filter
//! at all. The cheap fix — a `.contains()` per page — is how a codebase ends up
//! with four answers to "which row did the user mean", three of which nobody
//! ever compares. [`Searchable`] is the row-level half of the rule, so a list
//! that has only an id and a name gets the *same* tiering as the catalogue by
//! construction rather than by remembering to copy it.
//!
//! Only [`CatalogEntry`] carries children (its roster), so only it can reach
//! [`MatchRank::ChildOnly`]. The trait deliberately does **not** model children:
//! a list without them would otherwise have to answer a question it has no
//! vocabulary for, and "no children" would become indistinguishable from "no
//! matching children".
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

/// A row any preset picker can rank.
///
/// The three facts every catalogue in the product happens to have. A list with
/// no alternative ids simply does not override [`Searchable::search_aliases`].
pub trait Searchable {
    /// The stable id the operator types and the config keys on.
    fn search_id(&self) -> &str;
    /// The human-facing name. Ranks below every id-shaped match, because a
    /// display name *mentioning* a word is weaker evidence than an id *being*
    /// it.
    fn search_display_name(&self) -> &str;
    /// Alternative ids that resolve to this row (`kimi` → `moonshot`).
    fn search_aliases(&self) -> &[String] {
        &[]
    }
}

impl Searchable for CatalogEntry {
    fn search_id(&self) -> &str {
        &self.id
    }
    fn search_display_name(&self) -> &str {
        &self.display_name
    }
    fn search_aliases(&self) -> &[String] {
        &self.aliases
    }
}

/// How well a row matched, best first. Lower sorts earlier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchRank {
    /// The query is exactly the row id (or one of its aliases).
    ExactId = 0,
    /// The row id starts with the query.
    IdPrefix = 1,
    /// An alias starts with, or the id contains, the query.
    IdSubstring = 2,
    /// The human-facing display name matched.
    DisplayName = 3,
    /// Only a child — a model in the roster — matched; the row itself did not.
    /// Structurally unreachable for lists that have no children.
    ChildOnly = 4,
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

/// A matched row from a list that has no children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowMatch {
    /// Position in the input slice.
    pub index: usize,
    pub rank: MatchRank,
}

/// Rank a row on its own identity alone. `None` when nothing in it matched.
///
/// The whole of the tiering rule, in one place: everything else in this module
/// either calls it, or adds the children tier underneath it.
fn rank_identity<T: Searchable + ?Sized>(row: &T, q: &str) -> Option<MatchRank> {
    let id = row.search_id().to_lowercase();
    let aliases: Vec<String> = row
        .search_aliases()
        .iter()
        .map(|a| a.to_lowercase())
        .collect();

    if id == q || aliases.iter().any(|a| a == q) {
        return Some(MatchRank::ExactId);
    }
    if id.starts_with(q) {
        return Some(MatchRank::IdPrefix);
    }
    if id.contains(q) || aliases.iter().any(|a| a.starts_with(q)) {
        return Some(MatchRank::IdSubstring);
    }
    if row.search_display_name().to_lowercase().contains(q) {
        return Some(MatchRank::DisplayName);
    }
    None
}

/// Normalise a query. Empty means "no filter", which every entry point below
/// treats as a no-op rather than as a match-nothing.
fn normalise(query: &str) -> String {
    query.trim().to_lowercase()
}

/// Rank one entry, falling through to its roster when the row itself missed.
fn rank_one(entry: &CatalogEntry, q: &str) -> Option<(MatchRank, Vec<RosterModel>)> {
    if let Some(rank) = rank_identity(entry, q) {
        return Some((rank, entry.roster.clone()));
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
        Some((MatchRank::ChildOnly, matched))
    }
}

/// Filter and rank a catalogue.
///
/// An empty or whitespace-only query keeps every row, in the order given, with
/// every roster intact — so "no filter" is behaviourally identical to not
/// calling this at all.
#[must_use]
pub fn rank_entries(entries: &[CatalogEntry], query: &str) -> Vec<EntryMatch> {
    let q = normalise(query);
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

/// Filter and rank any list of [`Searchable`] rows.
///
/// The children-less half of [`rank_entries`], and the one a preset grid with
/// no roster wants. Same tiering, same stable sort, same empty-query no-op.
#[must_use]
pub fn rank_rows<T: Searchable>(rows: &[T], query: &str) -> Vec<RowMatch> {
    let q = normalise(query);
    if q.is_empty() {
        return rows
            .iter()
            .enumerate()
            .map(|(index, _)| RowMatch {
                index,
                rank: MatchRank::ExactId,
            })
            .collect();
    }

    let mut matches: Vec<RowMatch> = rows
        .iter()
        .enumerate()
        .filter_map(|(index, r)| rank_identity(r, &q).map(|rank| RowMatch { index, rank }))
        .collect();
    matches.sort_by_key(|m| m.rank);
    matches
}

/// Filter any [`Searchable`] list into owned rows, best match first.
#[must_use]
pub fn filter_rows<T: Searchable + Clone>(rows: &[T], query: &str) -> Vec<T> {
    rank_rows(rows, query)
        .into_iter()
        .map(|m| rows[m.index].clone())
        .collect()
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

    /// A children-less list, standing in for the generation preset grid.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Preset {
        id: String,
        name: String,
    }

    impl Searchable for Preset {
        fn search_id(&self) -> &str {
            &self.id
        }
        fn search_display_name(&self) -> &str {
            &self.name
        }
    }

    fn preset(id: &str, name: &str) -> Preset {
        Preset {
            id: id.into(),
            name: name.into(),
        }
    }

    fn presets() -> Vec<Preset> {
        vec![
            preset("openai-dalle", "OpenAI DALL-E"),
            preset("google-veo", "Google Veo"),
            preset("openai-tts", "OpenAI TTS"),
            preset("suno", "Suno"),
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
        assert_eq!(ranked[1].rank, MatchRank::ChildOnly);
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
        assert_eq!(ranked[0].rank, MatchRank::ChildOnly);
        assert_eq!(
            ranked[0]
                .models
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
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

    // --- the children-less half ------------------------------------------

    #[test]
    fn a_childless_list_gets_the_same_tiering() {
        let p = presets();
        let ranked = rank_rows(&p, "openai");
        // Both OpenAI rows match by id prefix; `suno` and `google-veo` do not.
        let ids: Vec<&str> = ranked.iter().map(|m| p[m.index].id.as_str()).collect();
        assert_eq!(ids, vec!["openai-dalle", "openai-tts"]);
        assert!(ranked.iter().all(|m| m.rank == MatchRank::IdPrefix));
    }

    #[test]
    fn a_childless_list_can_be_found_by_display_name() {
        let p = presets();
        // The id is `openai-dalle`; only the display name carries the hyphen
        // in "DALL-E". The weaker tier still has to match.
        let ranked = rank_rows(&p, "dall-e");
        assert_eq!(ranked.len(), 1);
        assert_eq!(p[ranked[0].index].id, "openai-dalle");
        assert_eq!(ranked[0].rank, MatchRank::DisplayName);
    }

    #[test]
    fn a_childless_list_never_reaches_the_children_tier() {
        // The tier exists on the enum for the catalogue's benefit; a list with
        // nothing under it must not be able to claim it.
        let p = presets();
        for query in ["openai", "veo", "s", "OPENAI", "  suno  "] {
            assert!(
                rank_rows(&p, query)
                    .iter()
                    .all(|m| m.rank != MatchRank::ChildOnly),
                "query {query:?} reached ChildOnly on a list with no children"
            );
        }
    }

    #[test]
    fn an_empty_query_is_a_no_op_for_childless_lists_too() {
        let p = presets();
        assert_eq!(filter_rows(&p, "  "), p);
    }

    #[test]
    fn both_halves_agree_on_identity_ranking() {
        // The whole reason for the trait: a row's own identity must rank the
        // same whether or not the list it sits in has children. Asserted over
        // the catalogue's own rows, ranked both ways.
        let c = catalog();
        for query in ["openai", "open", "kimi", "moonshot", "OpenRouter", "zzz"] {
            let via_entries: Vec<(usize, MatchRank)> = rank_entries(&c, query)
                .into_iter()
                // ChildOnly is the tier `rank_rows` structurally cannot reach,
                // so it is the one row shape excluded from the comparison.
                .filter(|m| m.rank != MatchRank::ChildOnly)
                .map(|m| (m.index, m.rank))
                .collect();
            let via_rows: Vec<(usize, MatchRank)> = rank_rows(&c, query)
                .into_iter()
                .map(|m| (m.index, m.rank))
                .collect();
            assert_eq!(via_entries, via_rows, "disagreement on query {query:?}");
        }
    }
}
