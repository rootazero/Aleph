//! Pure data logic for the memory console — facet classification, category
//! bucketing, and client-side pagination. No Leptos runtime dependency, so it
//! is unit-tested directly.

use std::collections::HashSet;

use super::selection::RowRef;

use crate::api::{CompressedFact, RawMemory};

/// Window of notes pulled in one `list_facts` call, then faceted/paginated
/// client-side. When `list_facts` returns exactly this many, the store may be
/// larger than the window — surface a truncation notice (no silent caps).
pub const NOTE_WINDOW: usize = 1000;

/// Fixed number of entries shown per page.
pub const PAGE_SIZE: u32 = 50;

/// Page sizes offered by the pager's selector. `PAGE_SIZE` must appear here so
/// the default is reachable after the user changes it.
pub const PAGE_SIZES: [u32; 3] = [25, 50, 100];

/// The state of one fetch.
///
/// Replaces the `(loaded: bool, data: T)` pair the memory console used to carry
/// alongside `if let Ok(..)` loaders. Under that shape an RPC failure produced
/// an empty `data` with `loaded = true` — indistinguishable from an empty
/// store, so every gateway error rendered as "no memories yet". Making failure
/// its own variant means a renderer cannot match exhaustively without drawing
/// the error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loadable<T> {
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Loadable<T> {
    /// Lift an RPC result into a load state, keeping the error text so the UI
    /// can show *what* went wrong rather than an empty list.
    #[must_use]
    pub fn from_rpc(res: Result<T, String>) -> Self {
        match res {
            Ok(v) => Self::Ready(v),
            Err(e) => Self::Failed(e),
        }
    }

    #[must_use]
    pub fn as_ready(&self) -> Option<&T> {
        match self {
            Self::Ready(v) => Some(v),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryFacet {
    /// Curated hot memory (`MEMORY.md`) — the block injected into every turn.
    ///
    /// Owns its own fetch and its own renderer (`curated::CuratedPanel`): its
    /// rows are neither notes nor raw turns, so none of the window slicing,
    /// paging or batch machinery below applies to it.
    Curated,
    AllNotes,
    Facts,
    Feedback,
    Lessons,
    /// Server-side full-text hits from `graph.search`.
    ///
    /// Note-shaped like the four buckets above, but its rows do NOT come from
    /// the loaded window — they arrive on their own signal, so `facet_slice`
    /// returns empty here and `bucket_counts` ignores it.
    SearchHits,
    Raw,
}

impl MemoryFacet {
    /// True for every note-shaped facet (i.e. not `Raw`). Drives which table /
    /// drawer / delete verb applies: note facets use `graph.delete_note`, `Raw`
    /// uses `memory.delete`. Mixing those two is what made search hits
    /// undeletable.
    #[must_use]
    pub fn is_notes(&self) -> bool {
        // Exhaustive on purpose. This used to be `!matches!(self, Raw)`,
        // which silently classified any NEW facet as note-shaped — and
        // "note-shaped" decides which delete verb runs against the row.
        // A new variant should be a compile error here, not a wrong default.
        match self {
            Self::AllNotes | Self::Facts | Self::Feedback | Self::Lessons | Self::SearchHits => {
                true
            }
            Self::Raw | Self::Curated => false,
        }
    }
}

/// Map a backend note category to its display facet. Backend categories are
/// lowercase (src/memory/context/enums.rs). `feedback` and `goal-lessons` are
/// the third memory pillar; everything else is a general fact.
#[must_use]
pub fn fact_facet(category: &str) -> MemoryFacet {
    match category {
        "feedback" => MemoryFacet::Feedback,
        "goal-lessons" => MemoryFacet::Lessons,
        _ => MemoryFacet::Facts,
    }
}

/// Counts for `[AllNotes, Facts, Feedback, Lessons]` over a note window.
#[must_use]
pub fn bucket_counts(facts: &[CompressedFact]) -> [usize; 4] {
    let mut out = [facts.len(), 0, 0, 0];
    for f in facts {
        match fact_facet(&f.category) {
            MemoryFacet::Feedback => out[2] += 1,
            MemoryFacet::Lessons => out[3] += 1,
            _ => out[1] += 1,
        }
    }
    out
}

/// Filter a note window down to one notes facet (`AllNotes` = passthrough).
#[must_use]
pub fn facet_slice(facts: &[CompressedFact], facet: MemoryFacet) -> Vec<CompressedFact> {
    match facet {
        MemoryFacet::AllNotes => facts.to_vec(),
        MemoryFacet::Raw | MemoryFacet::SearchHits | MemoryFacet::Curated => Vec::new(),
        other => facts
            .iter()
            .filter(|f| fact_facet(&f.category) == other)
            .cloned()
            .collect(),
    }
}

/// Case-insensitive substring filter over a note window by `content`.
/// An empty or whitespace-only query is a passthrough (full clone).
#[must_use]
pub fn filter_notes(window: &[CompressedFact], query: &str) -> Vec<CompressedFact> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return window.to_vec();
    }
    window
        .iter()
        .filter(|f| f.content.to_lowercase().contains(&q))
        .cloned()
        .collect()
}

/// Zero-indexed client-side page slice; out-of-range pages yield empty.
#[must_use]
pub fn page_slice<T: Clone>(items: &[T], page: u32, page_size: u32) -> Vec<T> {
    let start = (page as usize) * (page_size as usize);
    items
        .iter()
        .skip(start)
        .take(page_size as usize)
        .cloned()
        .collect()
}

/// Total page count (>=1).
#[must_use]
pub fn page_count(total: usize, page_size: u32) -> u32 {
    (total as u64).div_ceil(page_size as u64).max(1) as u32
}

/// Whether the pager's "next" control should be enabled.
///
/// `total_pages` is `None` when the true total is unknown — a version-skewed
/// core that never sent a `total` field, surfaced all the way through
/// `RawWindow`/`NotesWindow` as `Option<u64>`. In that case, fall back to
/// "this page came back full, so there is probably more" rather than hiding
/// the control: reading an absent total as "no more rows" is what made the
/// raw pager vanish entirely against an un-upgraded core.
#[must_use]
pub fn has_next_page(
    page: u32,
    total_pages: Option<u32>,
    current_len: usize,
    page_size: u32,
) -> bool {
    match total_pages {
        Some(tp) => page + 1 < tp,
        None => current_len as u32 >= page_size,
    }
}

/// Format a unix-seconds timestamp for display (`YYYY-MM-DD HH:MM`); `—` for
/// non-positive. Single source of truth for both memory tabs (replaces the
/// former duplicate in `views/memory` and mirrors `api/memory::format_timestamp_secs`).
pub fn format_ts(ts: i64) -> String {
    if ts <= 0 {
        return "\u{2014}".to_string();
    }
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64((ts * 1000) as f64));
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        date.get_full_year(),
        date.get_month() + 1,
        date.get_date(),
        date.get_hours(),
        date.get_minutes(),
    )
}

/// Whether the loaded notes window is truncated relative to the true store
/// size.
///
/// Prefers the server-reported `total` when the core sent one (`None` means
/// an un-upgraded core didn't send the field at all, not that the count is
/// zero — see [`crate::api::MemoryApi::list_facts`]). Without it, fall back
/// to the same cap heuristic the phone list already uses
/// (`loaded >= NOTE_WINDOW`): a version-skewed core that can't report the
/// true total must not read as "definitely not truncated" just because the
/// precise comparison is unavailable — that silently re-opens the exact
/// trailing-page bug this notice exists to prevent.
#[must_use]
pub fn notes_truncated(total: Option<u64>, loaded: usize) -> bool {
    match total {
        Some(t) => (t as usize) > loaded,
        None => loaded >= NOTE_WINDOW,
    }
}

/// Locate a note by its `path` within the loaded window. Returns the facet to
/// switch to (mapped from the note's category) and the zero-indexed page that
/// holds it within that facet's slice, computed against the caller's current
/// `page_size` (the pager's page-size selector can change this at runtime, so
/// a baked-in constant here would point at the wrong page). `None` when the
/// path is not in the window (e.g. it falls outside the NOTE_WINDOW cap) —
/// callers surface a notice.
#[must_use]
pub fn locate_note(
    window: &[CompressedFact],
    path: &str,
    page_size: u32,
) -> Option<(MemoryFacet, u32)> {
    let note = window.iter().find(|f| f.path == path)?;
    let facet = fact_facet(&note.category);
    let slice = facet_slice(window, facet);
    let pos = slice.iter().position(|f| f.path == path)?;
    Some((facet, (pos as u32) / page_size))
}

// ─── Markdown export ────────────────────────────────────────────────────────

/// Maximum entries one clipboard export may carry.
///
/// Each note needs its own `graph.node_detail` round trip, so an unbounded
/// "select all → copy" would fan out arbitrarily. The batch bar disables the
/// button above this and says the limit out loud — a silent truncation would
/// hand the user a partial export that looks complete.
pub const EXPORT_MAX: usize = 50;

/// One note staged for export. `body` is `Err` when its full text could not be
/// fetched; the renderer keeps the entry and records the reason rather than
/// dropping it.
#[derive(Debug, Clone)]
pub struct NoteExport {
    pub title: String,
    pub path: String,
    pub body: Result<String, String>,
}

/// One raw conversation row staged for export.
#[derive(Debug, Clone)]
pub struct RawExport {
    pub id: String,
    pub agent_id: String,
    pub session_id: Option<String>,
    /// Already-formatted display timestamp (see [`format_ts`]).
    pub created_at: String,
    /// The recorded turn text. One body: `raw_memories` has a single
    /// `content` column, so the `**Q**` / `**A**` split this used to emit
    /// labelled a distinction the store never made — every export marked the
    /// whole row as the question and never wrote an answer.
    pub content: String,
}

/// Which of `selected` are present on `page_rows`, and how many are not.
///
/// Raw rows are server-paginated and `selected` is never cleared on page
/// change, so a selection built across two pages can outlive the page it was
/// made on. Silently exporting only the ids still on the current page would
/// hand the user a partial export under an unqualified "Copied to
/// clipboard" — this reports the drop count so the caller can disclose it,
/// the same way `batch_export_partial` already discloses a failed
/// `node_detail` fetch on the notes side.
#[must_use]
pub fn stage_raw_export(
    selected: &HashSet<RowRef>,
    page_rows: &[RawMemory],
) -> (Vec<RawMemory>, usize) {
    // Matched on (partition, id), not id alone: the list spans the partition
    // union the gateway resolves, and a row is only the row the user ticked if
    // both halves agree.
    let staged: Vec<RawMemory> = page_rows
        .iter()
        .filter(|r| selected.contains(&RowRef::new(&r.agent_id, &r.id)))
        .cloned()
        .collect();
    let dropped = selected.len().saturating_sub(staged.len());
    (staged, dropped)
}

/// Render staged notes as a markdown document, one `#` section per note.
#[must_use]
pub fn notes_to_markdown(items: &[NoteExport]) -> String {
    let mut out = String::new();
    for item in items {
        out.push_str(&format!("# {}\n\n`{}`\n\n", item.title, item.path));
        match &item.body {
            Ok(body) => out.push_str(body.trim_end()),
            Err(e) => out.push_str(&format!("<!-- body unavailable: {e} -->")),
        }
        out.push_str("\n\n");
    }
    // One trailing newline, not two, so round-tripping the text is stable.
    out.truncate(out.trim_end().len());
    out.push('\n');
    out
}

/// Render staged raw rows as a markdown document, one `#` section per turn.
#[must_use]
pub fn raws_to_markdown(items: &[RawExport]) -> String {
    let mut out = String::new();
    for item in items {
        let session = item
            .session_id
            .as_deref()
            .map(|s| format!(" · session {s}"))
            .unwrap_or_default();
        out.push_str(&format!(
            "# {}\n\n`{}` · {}{}\n\n",
            item.created_at, item.id, item.agent_id, session
        ));
        if !item.content.trim().is_empty() {
            out.push_str(item.content.trim());
            out.push_str("\n\n");
        }
    }
    out.truncate(out.trim_end().len());
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::CompressedFact;

    fn fact(cat: &str) -> CompressedFact {
        CompressedFact {
            id: cat.into(),
            agent_id: "main".into(),
            content: "c".into(),
            fact_type: cat.into(),
            created_at: 0,
            updated_at: 0,
            category: cat.into(),
            path: format!("{cat}/x"),
            tags: Vec::new(),
            link_count: 0,
            match_field: None,
        }
    }

    fn fact_p(cat: &str, p: &str) -> CompressedFact {
        CompressedFact {
            id: p.into(),
            agent_id: "main".into(),
            content: "c".into(),
            fact_type: cat.into(),
            created_at: 0,
            updated_at: 0,
            category: cat.into(),
            path: p.into(),
            tags: Vec::new(),
            link_count: 0,
            match_field: None,
        }
    }

    #[test]
    fn fact_facet_maps_categories() {
        assert!(matches!(fact_facet("feedback"), MemoryFacet::Feedback));
        assert!(matches!(fact_facet("goal-lessons"), MemoryFacet::Lessons));
        assert!(matches!(fact_facet("preference"), MemoryFacet::Facts));
        assert!(matches!(fact_facet("project"), MemoryFacet::Facts));
    }

    #[test]
    fn bucket_counts_partition() {
        let facts = vec![
            fact("feedback"),
            fact("goal-lessons"),
            fact("preference"),
            fact("plan"),
        ];
        // [AllNotes, Facts, Feedback, Lessons]
        assert_eq!(bucket_counts(&facts), [4, 2, 1, 1]);
    }

    #[test]
    fn facet_slice_filters() {
        let facts = vec![fact("feedback"), fact("preference")];
        assert_eq!(facet_slice(&facts, MemoryFacet::Feedback).len(), 1);
        assert_eq!(facet_slice(&facts, MemoryFacet::AllNotes).len(), 2);
        assert_eq!(facet_slice(&facts, MemoryFacet::Facts).len(), 1);
        assert_eq!(facet_slice(&facts, MemoryFacet::Raw).len(), 0);
    }

    #[test]
    fn pagination_helpers() {
        let v: Vec<u32> = (0..120).collect();
        assert_eq!(page_count(120, 50), 3);
        assert_eq!(page_count(0, 50), 1);
        assert_eq!(page_slice(&v, 0, 50).len(), 50);
        assert_eq!(page_slice(&v, 2, 50), (100..120).collect::<Vec<_>>());
        assert_eq!(page_slice(&v, 9, 50).len(), 0); // out-of-range page
    }

    #[test]
    fn locate_note_finds_facet_and_page() {
        let mut window: Vec<CompressedFact> = (0..60)
            .map(|i| fact_p("preference", &format!("f{i}")))
            .collect();
        window.push(fact_p("feedback", "fb0"));

        // 56th Facts note (index 55) lands on page 1 (55 / 50).
        assert_eq!(
            locate_note(&window, "f55", 50),
            Some((MemoryFacet::Facts, 1))
        );
        // First Facts note is on page 0.
        assert_eq!(
            locate_note(&window, "f0", 50),
            Some((MemoryFacet::Facts, 0))
        );
        // Feedback note maps to the Feedback facet, page 0.
        assert_eq!(
            locate_note(&window, "fb0", 50),
            Some((MemoryFacet::Feedback, 0))
        );
        // Unknown path → None.
        assert_eq!(locate_note(&window, "missing", 50), None);
    }

    #[test]
    fn locate_note_uses_the_caller_s_page_size_not_a_baked_in_one() {
        // The pager's page-size selector can change page_size at runtime; if
        // this used a fixed constant instead of the parameter, the returned
        // page would point at the wrong page as soon as the user picked a
        // different size, and the reverse-link jump would silently land on
        // an empty or mismatched page.
        let window: Vec<CompressedFact> = (0..60)
            .map(|i| fact_p("preference", &format!("f{i}")))
            .collect();

        // Index 55: page 1 at page_size 50, but page 2 at page_size 25.
        assert_eq!(
            locate_note(&window, "f55", 50),
            Some((MemoryFacet::Facts, 1))
        );
        assert_eq!(
            locate_note(&window, "f55", 25),
            Some((MemoryFacet::Facts, 2))
        );
    }

    fn fact_content(content: &str) -> CompressedFact {
        CompressedFact {
            id: "i".into(),
            agent_id: "main".into(),
            content: content.into(),
            fact_type: "preference".into(),
            created_at: 0,
            updated_at: 0,
            category: "preference".into(),
            path: content.into(),
            tags: Vec::new(),
            link_count: 0,
            match_field: None,
        }
    }

    #[test]
    fn filter_notes_empty_query_passthrough() {
        let w = vec![fact_content("Alpha"), fact_content("Beta")];
        assert_eq!(filter_notes(&w, "").len(), 2);
        assert_eq!(filter_notes(&w, "   ").len(), 2);
    }

    #[test]
    fn filter_notes_case_insensitive_substring() {
        let w = vec![
            fact_content("Deploy on 18790"),
            fact_content("Smoke test first"),
        ];
        let r = filter_notes(&w, "SMOKE");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].content, "Smoke test first");
    }

    #[test]
    fn filter_notes_no_match_is_empty() {
        let w = vec![fact_content("Alpha")];
        assert!(filter_notes(&w, "zzz").is_empty());
    }

    // ── Loadable ────────────────────────────────────────────────────────────

    #[test]
    fn from_rpc_preserves_the_error_message() {
        // This is the whole point: the old loaders mapped Err to "no data", so
        // an RPC failure and an empty store rendered identically.
        let failed: Loadable<Vec<u32>> = Loadable::from_rpc(Err("gateway timeout".into()));
        assert_eq!(failed, Loadable::Failed("gateway timeout".to_string()));
        assert!(failed.as_ready().is_none());
    }

    #[test]
    fn from_rpc_wraps_ok_as_ready() {
        let ready: Loadable<Vec<u32>> = Loadable::from_rpc(Ok(vec![1, 2]));
        assert_eq!(ready.as_ready(), Some(&vec![1, 2]));
        assert!(!ready.is_loading());
    }

    #[test]
    fn an_empty_ok_is_ready_not_failed() {
        // An empty store is a legitimate Ready state, distinct from Failed.
        let empty: Loadable<Vec<u32>> = Loadable::from_rpc(Ok(vec![]));
        assert_eq!(empty.as_ready(), Some(&vec![]));
        assert!(matches!(empty, Loadable::Ready(_)));
    }

    #[test]
    fn loading_is_neither_ready_nor_failed() {
        let l: Loadable<Vec<u32>> = Loadable::Loading;
        assert!(l.is_loading());
        assert!(l.as_ready().is_none());
    }

    // ── SearchHits facet ────────────────────────────────────────────────────

    #[test]
    fn search_hits_is_note_shaped() {
        // Hits are notes, so every note-shaped affordance (drawer, locate-in-
        // graph, note delete path) applies to them.
        assert!(MemoryFacet::SearchHits.is_notes());
        assert!(!MemoryFacet::Raw.is_notes());
    }

    #[test]
    fn search_hits_never_slices_the_window() {
        // Hit rows arrive from graph.search on their own signal; slicing the
        // loaded window for this facet would silently show stale local rows.
        let facts = vec![fact("preference"), fact("feedback")];
        assert!(facet_slice(&facts, MemoryFacet::SearchHits).is_empty());
    }

    #[test]
    fn bucket_counts_ignores_search_hits() {
        // The chip badges describe the loaded window's four note buckets; the
        // hit count is reported separately by the hits signal.
        let facts = vec![fact("feedback"), fact("preference")];
        assert_eq!(bucket_counts(&facts), [2, 1, 1, 0]);
    }

    // ── Markdown export ─────────────────────────────────────────────────────

    #[test]
    fn notes_export_writes_title_path_and_body() {
        let items = vec![NoteExport {
            title: "deploy-notes".into(),
            path: "facts/deploy-notes".into(),
            body: Ok("- smoke test first\n".into()),
        }];
        let md = notes_to_markdown(&items);
        assert!(md.contains("# deploy-notes"));
        assert!(md.contains("`facts/deploy-notes`"));
        assert!(md.contains("- smoke test first"));
    }

    #[test]
    fn notes_export_marks_unfetchable_bodies_instead_of_dropping_them() {
        // A note whose body failed to load must still appear, with the reason
        // visible. Silently omitting it would make the export look complete.
        let items = vec![NoteExport {
            title: "broken".into(),
            path: "facts/broken".into(),
            body: Err("node_detail: timeout".into()),
        }];
        let md = notes_to_markdown(&items);
        assert!(md.contains("# broken"));
        assert!(md.contains("<!-- body unavailable: node_detail: timeout -->"));
    }

    #[test]
    fn notes_export_separates_entries_with_a_blank_line() {
        let items = vec![
            NoteExport {
                title: "a".into(),
                path: "facts/a".into(),
                body: Ok("x".into()),
            },
            NoteExport {
                title: "b".into(),
                path: "facts/b".into(),
                body: Ok("y".into()),
            },
        ];
        let md = notes_to_markdown(&items);
        assert_eq!(md.matches("# ").count(), 2);
        assert!(
            md.contains("x\n\n# b"),
            "entries must be blank-line separated: {md:?}"
        );
    }

    #[test]
    fn raws_export_writes_the_body_and_keeps_the_provenance_header() {
        let items = vec![RawExport {
            id: "raw-1".into(),
            agent_id: "main".into(),
            session_id: Some("s-77".into()),
            created_at: "2026-07-24 14:02".into(),
            content: "why phantom pages?".into(),
        }];
        let md = raws_to_markdown(&items);
        assert!(md.contains("2026-07-24 14:02"));
        assert!(md.contains("main"));
        assert!(md.contains("s-77"));
        assert!(md.contains("why phantom pages?"));
    }

    /// The export used to label every row `**Q**` and, when it found a
    /// second half, `**A**`. There is no second half — `raw_memories` has one
    /// `content` column — so every exported row claimed to be a question with
    /// no answer. Labels that can only ever say one thing are not labels.
    #[test]
    fn raws_export_does_not_label_the_body_as_a_question() {
        let items = vec![RawExport {
            id: "raw-2".into(),
            agent_id: "main".into(),
            session_id: None,
            created_at: "2026-07-24 14:03".into(),
            content: "only a question".into(),
        }];
        let md = raws_to_markdown(&items);
        assert!(md.contains("only a question"));
        assert!(!md.contains("**Q**"), "{md:?}");
        assert!(!md.contains("**A**"), "{md:?}");
    }

    #[test]
    fn raws_export_skips_a_row_with_an_empty_body() {
        let items = vec![RawExport {
            id: "raw-3".into(),
            agent_id: "main".into(),
            session_id: None,
            created_at: "2026-07-24 14:04".into(),
            content: "   ".into(),
        }];
        let md = raws_to_markdown(&items);
        // Header still present (the row exists), body section absent.
        assert!(md.contains("raw-3"));
        assert_eq!(
            md.trim_end().lines().last().unwrap().trim(),
            "`raw-3` · main"
        );
    }

    #[test]
    fn export_cap_is_fifty() {
        // The batch bar disables itself above this and says so; the constant is
        // the single source both the guard and the message read.
        assert_eq!(EXPORT_MAX, 50);
    }

    // ── Pager ────────────────────────────────────────────────────────────────

    #[test]
    fn page_sizes_start_at_the_current_default() {
        // 50 was the hardcoded page size; it stays the middle option so the
        // default view is unchanged for existing users.
        assert!(PAGE_SIZES.contains(&PAGE_SIZE));
        assert_eq!(PAGE_SIZES, [25, 50, 100]);
    }

    #[test]
    fn page_count_tracks_the_chosen_page_size() {
        assert_eq!(page_count(120, 25), 5);
        assert_eq!(page_count(120, 50), 3);
        assert_eq!(page_count(120, 100), 2);
    }

    #[test]
    fn page_count_exact_multiple_has_no_phantom_trailing_page() {
        // 100 rows at 50/page is exactly 2 full pages — div_ceil must not
        // round an exact multiple up to a 3rd, empty page.
        assert_eq!(page_count(100, 50), 2);
    }

    #[test]
    fn page_count_zero_total_is_still_one_page_at_any_page_size() {
        // An empty store reports one (empty) page regardless of which size
        // the user picked — this is the input where a phantom page would
        // reappear if the `.max(1)` floor were ever lost.
        assert_eq!(page_count(0, 25), 1);
        assert_eq!(page_count(0, 100), 1);
    }

    #[test]
    fn has_next_page_uses_the_precise_total_when_known() {
        assert!(has_next_page(0, Some(3), 50, 50));
        assert!(!has_next_page(2, Some(3), 20, 50));
    }

    #[test]
    fn has_next_page_falls_back_to_the_full_page_heuristic_when_total_is_unknown() {
        // Version skew: `total_pages` is `None` because the core never sent
        // a total at all. A full page must still offer "next" — this is the
        // exact case where the raw pager used to vanish entirely once `total`
        // silently defaulted to `0` instead of `None`.
        assert!(has_next_page(0, None, 50, 50));
        assert!(!has_next_page(0, None, 12, 50));
    }

    // ── notes_truncated ──────────────────────────────────────────────────────

    #[test]
    fn notes_truncated_uses_the_precise_total_when_known() {
        assert!(notes_truncated(Some(1200), 1000));
        assert!(!notes_truncated(Some(1000), 1000));
        assert!(!notes_truncated(Some(3), 3));
    }

    #[test]
    fn notes_truncated_falls_back_to_the_window_cap_when_total_is_unknown() {
        // An un-upgraded core doesn't send `total` at all (`None`, not `0`).
        // Loading exactly NOTE_WINDOW rows is the ambiguous case: the true
        // store could be bigger. Silently reading `None` as "not truncated"
        // is exactly the bug this notice exists to prevent, so the unknown
        // case must still warn once the window cap is hit.
        assert!(notes_truncated(None, NOTE_WINDOW));
        assert!(!notes_truncated(None, NOTE_WINDOW - 1));
    }

    // ── stage_raw_export ─────────────────────────────────────────────────────

    fn raw_row(id: &str) -> RawMemory {
        raw_row_in("main", id)
    }

    fn raw_row_in(partition: &str, id: &str) -> RawMemory {
        RawMemory {
            id: id.into(),
            agent_id: partition.into(),
            content: "q".into(),
            session_id: None,
            created_at: None,
        }
    }

    #[test]
    fn stage_raw_export_drops_ids_not_on_the_current_page() {
        // 50 ids selected across two pages of raw rows, but only the second
        // page's 25 rows are loaded right now (raw is server-paginated and
        // `selected` is never cleared on page change) -- the other 25 must be
        // reported as dropped, not silently omitted from an export that still
        // claims success.
        let page: Vec<RawMemory> = (25..50).map(|i| raw_row(&format!("r{i}"))).collect();
        let selected: HashSet<RowRef> = (0..50)
            .map(|i| RowRef::new("main", format!("r{i}")))
            .collect();
        let (staged, dropped) = stage_raw_export(&selected, &page);
        assert_eq!(staged.len(), 25);
        assert_eq!(dropped, 25);
    }

    /// A row is staged only when BOTH halves of its identity match. Ticking
    /// `r0` in one partition must not export the `r0` sitting in the other —
    /// which is what an id-only match did once the list started spanning the
    /// partition union.
    #[test]
    fn stage_raw_export_matches_on_the_partition_too_not_the_id_alone() {
        let page = vec![raw_row_in("main", "r0"), raw_row_in("main__u-owner", "r0")];
        let selected: HashSet<RowRef> = [RowRef::new("main__u-owner", "r0")].into_iter().collect();
        let (staged, dropped) = stage_raw_export(&selected, &page);
        assert_eq!(staged.len(), 1, "exactly the ticked row, not its namesake");
        assert_eq!(staged[0].agent_id, "main__u-owner");
        assert_eq!(dropped, 0);
    }

    #[test]
    fn stage_raw_export_no_drop_when_the_whole_selection_is_on_page() {
        let page: Vec<RawMemory> = (0..10).map(|i| raw_row(&format!("r{i}"))).collect();
        let selected: HashSet<RowRef> = (0..5)
            .map(|i| RowRef::new("main", format!("r{i}")))
            .collect();
        let (staged, dropped) = stage_raw_export(&selected, &page);
        assert_eq!(staged.len(), 5);
        assert_eq!(dropped, 0);
    }
}
