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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryFacet {
    AllNotes,
    Facts,
    Feedback,
    Lessons,
    Raw,
}

impl MemoryFacet {
    /// True for every facet backed by the note layer (i.e. not `Raw`).
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
        MemoryFacet::Raw => Vec::new(),
        other => facts
            .iter()
            .filter(|f| fact_facet(&f.category) == other)
            .cloned()
            .collect(),
    }
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
#[cfg(target_arch = "wasm32")]
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
/// holds it within that facet's slice. `None` when the path is not in the
/// window (e.g. it falls outside the NOTE_WINDOW cap) — callers surface a notice.
#[must_use]
pub fn locate_note(window: &[CompressedFact], path: &str) -> Option<(MemoryFacet, u32)> {
    let note = window.iter().find(|f| f.path == path)?;
    let facet = fact_facet(&note.category);
    let slice = facet_slice(window, facet);
    let pos = slice.iter().position(|f| f.path == path)?;
    Some((facet, (pos as u32) / PAGE_SIZE))
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
            category: cat.into(),
            path: format!("{cat}/x"),
        }
    }

    fn fact_p(cat: &str, p: &str) -> CompressedFact {
        CompressedFact {
            id: p.into(),
            agent_id: "main".into(),
            content: "c".into(),
            fact_type: cat.into(),
            created_at: 0,
            category: cat.into(),
            path: p.into(),
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
        let mut window: Vec<CompressedFact> =
            (0..60).map(|i| fact_p("preference", &format!("f{i}"))).collect();
        window.push(fact_p("feedback", "fb0"));

        // 56th Facts note (index 55) lands on page 1 (55 / 50).
        assert_eq!(locate_note(&window, "f55"), Some((MemoryFacet::Facts, 1)));
        // First Facts note is on page 0.
        assert_eq!(locate_note(&window, "f0"), Some((MemoryFacet::Facts, 0)));
        // Feedback note maps to the Feedback facet, page 0.
        assert_eq!(locate_note(&window, "fb0"), Some((MemoryFacet::Feedback, 0)));
        // Unknown path → None.
        assert_eq!(locate_note(&window, "missing"), None);
    }
}
