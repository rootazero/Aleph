//! Pure data logic for the memory console — facet classification, category
//! bucketing, and client-side pagination. No Leptos runtime dependency, so it
//! is unit-tested directly.

use crate::api::CompressedFact;

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
        !matches!(self, MemoryFacet::Raw)
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
        MemoryFacet::Raw | MemoryFacet::SearchHits => Vec::new(),
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
    pub user_input: String,
    pub ai_output: String,
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
        if !item.user_input.trim().is_empty() {
            out.push_str(&format!("**Q** {}\n\n", item.user_input.trim()));
        }
        if !item.ai_output.trim().is_empty() {
            out.push_str(&format!("**A** {}\n\n", item.ai_output.trim()));
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
    fn raws_export_labels_both_halves_and_keeps_session() {
        let items = vec![RawExport {
            id: "raw-1".into(),
            agent_id: "main".into(),
            session_id: Some("s-77".into()),
            created_at: "2026-07-24 14:02".into(),
            user_input: "why phantom pages?".into(),
            ai_output: "the total was global".into(),
        }];
        let md = raws_to_markdown(&items);
        assert!(md.contains("2026-07-24 14:02"));
        assert!(md.contains("main"));
        assert!(md.contains("s-77"));
        assert!(md.contains("**Q** why phantom pages?"));
        assert!(md.contains("**A** the total was global"));
    }

    #[test]
    fn raws_export_omits_an_empty_half() {
        // Raw rows built from a single-sided record must not emit a bare "**A**".
        let items = vec![RawExport {
            id: "raw-2".into(),
            agent_id: "main".into(),
            session_id: None,
            created_at: "2026-07-24 14:03".into(),
            user_input: "only a question".into(),
            ai_output: String::new(),
        }];
        let md = raws_to_markdown(&items);
        assert!(md.contains("**Q** only a question"));
        assert!(!md.contains("**A**"));
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
}
