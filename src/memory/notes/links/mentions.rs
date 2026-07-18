//! Unlinked-mention scanner (spec M1, D2): a note body mentioning another
//! note's filename/alias — without a `[[wikilink]]` — earns a low-confidence
//! `mention` soft edge. Deterministic exact matching, zero LLM (R7-clean:
//! same class as FTS). Bodies are never modified (D2: only humans/LLMs write
//! real `[[links]]`).

use std::collections::HashMap;

use super::resolve::normalize_link_key;

/// Relation label for auto-detected unlinked mentions in `notes_links`.
pub const MENTION_RELATION: &str = "mention";
/// Confidence for mention soft edges (spec §2.3).
pub const MENTION_CONFIDENCE: f32 = 0.35;
/// Per-note cap on emitted mentions (spec M1 guard).
pub const MAX_MENTIONS_PER_NOTE: usize = 5;

/// One note's scan input.
pub struct MentionDoc {
    pub path: String,
    /// filename + frontmatter aliases (the names other bodies may mention).
    pub names: Vec<String>,
    /// frontmatter-stripped body text.
    pub body: String,
    /// raw wikilink targets already present in the body (skip re-linking).
    pub linked_raw: Vec<String>,
}

/// A name qualifies when: ASCII-only names have ≥4 chars; names containing
/// CJK have ≥2 chars. Short names ("app", "图") produce only noise.
fn name_qualifies(name: &str) -> bool {
    let has_cjk = name.chars().any(is_cjk);
    let n = name.chars().count();
    if has_cjk {
        n >= 2
    } else {
        n >= 4
    }
}

pub(crate) const fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x3040..=0x30FF | 0xAC00..=0xD7AF)
}

/// ASCII names need word boundaries (both neighbours non-alphanumeric);
/// CJK names match as substrings (no word boundaries in CJK text).
fn body_mentions(body_norm: &str, name_norm: &str, cjk: bool) -> bool {
    // An empty needle makes `find` return `Some(0)` on every iteration while
    // `from` never advances (end == start == from), hanging the scan. A name
    // that normalizes to empty (e.g. an all-ideographic-space alias) carries no
    // signal, so it mentions nothing.
    if name_norm.is_empty() {
        return false;
    }
    if cjk {
        return body_norm.contains(name_norm);
    }
    let mut from = 0;
    while let Some(rel) = body_norm[from..].find(name_norm) {
        let start = from + rel;
        let end = start + name_norm.len();
        // ASCII-only boundary check: a CJK neighbour must NOT block a match
        // ("使用Rust开发" does mention "Rust"), so is_ascii_alphanumeric — the
        // Unicode is_alphanumeric would count ideographs as alphabetic.
        let before_ok = start == 0
            || !body_norm[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        let after_ok = end >= body_norm.len()
            || !body_norm[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

/// Deterministic unlinked-mention scan across the corpus.
/// Returns (from_path, to_path) pairs, ≤ MAX_MENTIONS_PER_NOTE per from-note,
/// deterministic order (sorted by (from, to)).
#[must_use]
pub fn scan_mentions(docs: &[MentionDoc]) -> Vec<(String, String)> {
    // Dictionary: normalized name → owning paths. Ambiguous names (owned by
    // >1 note) are dropped wholesale — mirror the resolver's never-guess rule.
    let mut dict: HashMap<String, Vec<&str>> = HashMap::new();
    for d in docs {
        for name in &d.names {
            if name_qualifies(name) {
                dict.entry(normalize_link_key(name))
                    .or_default()
                    .push(d.path.as_str());
            }
        }
    }
    dict.retain(|_, owners| {
        // sort first: dedup only removes consecutive duplicates, and owner
        // ordering must not decide whether a name looks unique.
        owners.sort();
        owners.dedup();
        owners.len() == 1
    });

    let mut out: Vec<(String, String)> = Vec::new();
    let mut hits: Vec<(String, String)> = Vec::new();
    for d in docs {
        let body_norm = normalize_link_key(&d.body);
        let linked: Vec<String> = d.linked_raw.iter().map(|s| normalize_link_key(s)).collect();
        hits.clear();
        for (name_norm, owners) in &dict {
            let target = owners[0];
            if target == d.path {
                continue; // self
            }
            // Already a real [[link]] — in EITHER wikilink form.
            //
            // This used to compare only against `name_norm`, the bare note name.
            // But Aleph writes wikilinks in FULL-PATH form (`add_links` renders
            // `[[category/filename]]`, and `NoteWeave` passes the resolved path), so
            // `[[personal/news-monitoring]]` never matched the key `news-monitoring`
            // and the guard did not fire. `body_mentions` then matched the bare name
            // INSIDE the wikilink markup itself — '/' before it and ']' after it are
            // both non-alphanumeric, so the word-boundary test passes — yielding a
            // phantom "unlinked mention" for a pair that already has a real link edge.
            //
            // The resulting INSERT was a harmless no-op (`ON CONFLICT DO NOTHING`),
            // but the hit was COUNTED first: it burned a slot in
            // `MAX_MENTIONS_PER_NOTE` and in the per-cycle cap. A note with 5+
            // path-form outgoing links therefore emitted five no-op hits and zero
            // genuine unlinked-mention edges, every cycle, forever.
            let target_norm = normalize_link_key(target);
            if linked.iter().any(|l| l == name_norm || *l == target_norm) {
                continue;
            }
            let cjk = name_norm.chars().any(is_cjk);
            if body_mentions(&body_norm, name_norm, cjk) {
                hits.push((d.path.clone(), target.to_string()));
            }
        }
        hits.sort();
        hits.dedup();
        hits.truncate(MAX_MENTIONS_PER_NOTE);
        out.extend(std::mem::take(&mut hits));
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(path: &str, names: &[&str], body: &str, linked: &[&str]) -> MentionDoc {
        MentionDoc {
            path: path.into(),
            names: names.iter().map(|s| s.to_string()).collect(),
            body: body.into(),
            linked_raw: linked.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn detects_ascii_mention_with_word_boundary() {
        let docs = vec![
            doc("a/rust-notes", &["rust-notes"], "body of target", &[]),
            doc(
                "b/diary",
                &["diary"],
                "today I reread rust-notes again",
                &[],
            ),
        ];
        assert_eq!(
            scan_mentions(&docs),
            vec![("b/diary".to_string(), "a/rust-notes".to_string())]
        );
    }

    /// A note that ALREADY links to the target in full-path form must not also
    /// report an "unlinked mention" of it.
    ///
    /// Aleph writes wikilinks in path form (`[[category/filename]]`), so the old
    /// bare-name-only skip guard never fired, and the bare name was then matched
    /// inside the wikilink markup itself. The phantom row was a no-op insert, but
    /// it was counted first and burned a slot in the per-note mention cap — a note
    /// with 5+ path-form links produced five no-op hits and zero real mention edges.
    #[test]
    fn path_form_wikilink_counts_as_already_linked() {
        let docs = vec![
            doc("personal/news-monitoring", &["news-monitoring"], "x", &[]),
            doc(
                "b/diary",
                &["diary"],
                "Related: [[personal/news-monitoring]]",
                &["personal/news-monitoring"],
            ),
        ];
        assert!(
            scan_mentions(&docs).is_empty(),
            "a path-form [[link]] is a real link — it must not also count as an \
             unlinked mention"
        );
    }

    /// The bare-name link form still skips too (the original guard's job).
    #[test]
    fn bare_name_wikilink_still_counts_as_already_linked() {
        let docs = vec![
            doc("personal/news-monitoring", &["news-monitoring"], "x", &[]),
            doc(
                "b/diary",
                &["diary"],
                "Related: [[news-monitoring]]",
                &["news-monitoring"],
            ),
        ];
        assert!(scan_mentions(&docs).is_empty());
    }

    /// ...and a genuine unlinked mention is still detected when there is no link.
    #[test]
    fn genuine_unlinked_mention_survives_the_stricter_guard() {
        let docs = vec![
            doc("personal/news-monitoring", &["news-monitoring"], "x", &[]),
            doc("b/diary", &["diary"], "I read news-monitoring today", &[]),
        ];
        assert_eq!(
            scan_mentions(&docs),
            vec![(
                "b/diary".to_string(),
                "personal/news-monitoring".to_string()
            )]
        );
    }

    #[test]
    fn word_boundary_rejects_substring_for_ascii() {
        let docs = vec![
            doc("a/rust", &["rust"], "x", &[]),
            doc("b/d", &["d"], "we trust the process", &[]), // "rust" inside "trust"
        ];
        assert!(scan_mentions(&docs).is_empty());
    }

    #[test]
    fn cjk_mention_matches_substring() {
        let docs = vec![
            doc("a/记忆系统", &["记忆系统"], "x", &[]),
            doc("b/日记", &["日记"], "今天研究了记忆系统的检索", &[]),
        ];
        assert_eq!(
            scan_mentions(&docs),
            vec![("b/日记".into(), "a/记忆系统".into())]
        );
    }

    #[test]
    fn ascii_name_adjacent_to_cjk_matches() {
        // CJK neighbours are not ASCII word characters: "使用Rust开发" (no
        // spaces) must still count as mentioning the note named "Rust".
        let docs = vec![
            doc("a/rust", &["Rust"], "x", &[]),
            doc("b/日记", &["日记"], "使用Rust开发", &[]),
        ];
        assert_eq!(
            scan_mentions(&docs),
            vec![("b/日记".into(), "a/rust".into())]
        );
    }

    #[test]
    fn short_names_never_match() {
        let docs = vec![
            doc("a/app", &["app"], "x", &[]), // ASCII len 3 < 4
            doc("a/图", &["图"], "y", &[]),   // CJK len 1 < 2
            doc("b/d", &["dddd"], "the app draws a 图", &[]),
        ];
        assert!(scan_mentions(&docs).is_empty());
    }

    #[test]
    fn skips_already_linked_and_self() {
        let docs = vec![
            doc(
                "a/rust-notes",
                &["rust-notes"],
                "rust-notes mentions itself",
                &[],
            ),
            doc(
                "b/diary",
                &["diary"],
                "see [[rust-notes]] and rust-notes prose",
                &["rust-notes"],
            ),
        ];
        assert!(
            scan_mentions(&docs).is_empty(),
            "self + already-linked must not edge"
        );
    }

    #[test]
    fn ambiguous_name_is_skipped_entirely() {
        let docs = vec![
            doc("a/notes", &["notes"], "x", &[]),
            doc("b/notes", &["notes"], "y", &[]),
            doc("c/diary", &["diary"], "my notes about things", &[]),
        ];
        assert!(
            scan_mentions(&docs).is_empty(),
            "duplicate name must never guess"
        );
    }

    #[test]
    fn per_note_cap_applies() {
        let mut docs: Vec<MentionDoc> = (0..8)
            .map(|i| {
                doc(
                    &format!("t/target-{i:02}"),
                    &[&format!("target-{i:02}")],
                    "x",
                    &[],
                )
            })
            .collect();
        let body: String = (0..8).map(|i| format!("target-{i:02} ")).collect();
        docs.push(doc("s/spammy", &["spammy"], &body, &[]));
        let hits = scan_mentions(&docs);
        assert_eq!(hits.len(), MAX_MENTIONS_PER_NOTE);
        assert!(hits.iter().all(|(f, _)| f == "s/spammy"));
    }
}
