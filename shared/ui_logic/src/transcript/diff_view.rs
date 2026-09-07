//! From a wire `FileChange` to paintable rows. Unified layout only (spec
//! R-3 / §5): every row has an old/new line number, a tag, and spans where
//! `emphasis` marks the changed words of a paired Del/Add line (a second
//! LCS over tokens — pi-cc-extensions `diff-inline.ts`), budgeted so a
//! minified file never costs a quadratic table.

use aleph_protocol::file_change::{FileChange, Hunk, LineTag, Unavailable};

pub const COLLAPSED_DIFF_ROWS: usize = 2;
pub const EXPANDED_DIFF_ROWS: usize = 400;
pub const LCS_CELL_BUDGET_COLLAPSED: usize = 200_000;
pub const LCS_CELL_BUDGET_EXPANDED: usize = 1_000_000;
/// Lines longer than this get no inline spans (guards minified files).
pub const MAX_INLINE_LINE_CHARS: usize = 700;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub emphasis: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub tag: LineTag,
    pub spans: Vec<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiffRows {
    pub rows: Vec<DiffRow>,
    pub hidden_rows: usize,
    pub hidden_hunks: usize,
}

/// What [`diff_rows`] answers with — a **sum**, not a struct with an ignorable
/// field, on purpose.
///
/// A withheld diff (`Redacted`, `TooLarge`, …) and a genuinely no-op edit both
/// arrive here with zero hunks. If this returned rows either way they would be
/// byte-identical values, and the reason would live only in [`stats_label`] —
/// a *convention between two functions*, which is exactly the arrangement that
/// let "an empty hunk list reads as 'this change touched nothing'" happen on
/// the server side (the emitter's `Redacted` ruling exists because of it).
/// Here the caller cannot reach `rows` without matching the other arm, so a
/// renderer that forgets does not compile rather than showing nothing and
/// saying nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffView {
    /// Hunks were computed. Empty `rows` here means what it says: the change
    /// touched no lines.
    Rows(DiffRows),
    /// The server withheld the hunks and said why. Render the reason —
    /// [`stats_label`] turns it into the sentence, and its counts are still
    /// exact for `Redacted` and for the `MAX_HUNK_LINES` flavour of
    /// `TooLarge`.
    Unavailable(Unavailable),
}

/// `+12 -3`, or the unavailable reason when there are no hunks to show.
///
/// A `TooLarge` change with `added == 0 && removed == 0` is one whose stats
/// were never computed at all (the server's byte-size cap on a `Modified`
/// change learns no per-side line counts) — never a diff with zero real
/// changes, since the server's hunk-count cap only ever trips on a diff
/// that has changes. So that specific zero is rendered as unavailable, not
/// as a lying `+0 -0`; any other `TooLarge` (non-zero stats, from the
/// hunk-count cap) still shows its exact counts.
#[must_use]
pub fn stats_label(change: &FileChange) -> String {
    match change.unavailable {
        Some(Unavailable::TooLarge) if change.added == 0 && change.removed == 0 => {
            "diff unavailable: too large to compute".into()
        }
        Some(Unavailable::TooLarge) | None => format!("+{} -{}", change.added, change.removed),
        Some(Unavailable::Binary) => "diff unavailable: binary".into(),
        Some(Unavailable::PreImageUnavailable) => "diff unavailable: previous content unreadable".into(),
        Some(Unavailable::ToolFailed) => "diff unavailable: tool failed".into(),
        // Says WHY, not just that it is gone: the stats beside it are exact,
        // and a reader who is told only "unavailable" would reasonably wonder
        // whether the write itself went wrong. Nothing went wrong — the server
        // withheld a diff whose lines could not be masked without corrupting
        // the line counts.
        Some(Unavailable::Redacted) => "diff unavailable: contained a secret".into(),
    }
}

/// Tokens: identifier runs, whitespace runs, single punctuation chars.
fn tokenize(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let c = s[i..].chars().next().unwrap();
        let class = if c.is_alphanumeric() || c == '_' { 0 } else if c.is_whitespace() { 1 } else { 2 };
        let start = i;
        i += c.len_utf8();
        if class != 2 {
            while i < s.len() {
                let d = s[i..].chars().next().unwrap();
                let dc = if d.is_alphanumeric() || d == '_' { 0 } else if d.is_whitespace() { 1 } else { 2 };
                if dc != class { break; }
                i += d.len_utf8();
            }
        }
        out.push(&s[start..i]);
    }
    out
}

/// LCS over tokens; returns per-side `changed` flags. `None` when over budget.
fn token_lcs_changed(a: &[&str], b: &[&str], cell_budget: usize) -> Option<(Vec<bool>, Vec<bool>)> {
    let (n, m) = (a.len(), b.len());
    if n.saturating_mul(m) > cell_budget {
        return None;
    }
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
        }
    }
    let mut ca = vec![true; n];
    let mut cb = vec![true; m];
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            ca[i] = false;
            cb[j] = false;
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    Some((ca, cb))
}

fn spans_from(tokens: &[&str], changed: &[bool]) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    for (t, &c) in tokens.iter().zip(changed) {
        // Whitespace never carries emphasis on its own (pi trims spans).
        let c = c && !t.trim().is_empty();
        match out.last_mut() {
            Some(last) if last.emphasis == c => last.text.push_str(t),
            _ => out.push(Span { text: (*t).to_string(), emphasis: c }),
        }
    }
    out
}

fn plain(text: &str) -> Vec<Span> {
    vec![Span { text: text.to_string(), emphasis: false }]
}

/// Word-level spans for a paired removed/added line. Falls back to plain
/// spans when either line is too long or the token table is over budget.
#[must_use]
pub fn word_spans(del: &str, add: &str, cell_budget: usize) -> (Vec<Span>, Vec<Span>) {
    if del == add || del.chars().count() > MAX_INLINE_LINE_CHARS || add.chars().count() > MAX_INLINE_LINE_CHARS {
        return (plain(del), plain(add));
    }
    let (ta, tb) = (tokenize(del), tokenize(add));
    match token_lcs_changed(&ta, &tb, cell_budget) {
        Some((ca, cb)) => (spans_from(&ta, &ca), spans_from(&tb, &cb)),
        None => (plain(del), plain(add)),
    }
}

fn hunk_rows(h: &Hunk, budget: usize, out: &mut Vec<DiffRow>) {
    let (mut old_no, mut new_no) = (h.old_start, h.new_start);
    let mut i = 0;
    while i < h.lines.len() {
        let l = &h.lines[i];
        match l.tag {
            LineTag::Ctx => {
                out.push(DiffRow { old_no: Some(old_no), new_no: Some(new_no), tag: LineTag::Ctx, spans: plain(&l.text) });
                old_no += 1;
                new_no += 1;
                i += 1;
            }
            LineTag::Del => {
                // Pair a Del run with the Add run that immediately follows it.
                let del_start = i;
                while i < h.lines.len() && h.lines[i].tag == LineTag::Del { i += 1; }
                let add_start = i;
                while i < h.lines.len() && h.lines[i].tag == LineTag::Add { i += 1; }
                let dels = &h.lines[del_start..add_start];
                let adds = &h.lines[add_start..i];
                let pairs = dels.len().max(adds.len());
                let mut pending_adds: Vec<Vec<Span>> = Vec::with_capacity(adds.len());
                for k in 0..pairs {
                    match (dels.get(k), adds.get(k)) {
                        (Some(d), Some(a)) => {
                            let (ds, as_) = word_spans(&d.text, &a.text, budget);
                            out.push(DiffRow { old_no: Some(old_no), new_no: None, tag: LineTag::Del, spans: ds });
                            old_no += 1;
                            pending_adds.push(as_);
                        }
                        (Some(d), None) => {
                            out.push(DiffRow { old_no: Some(old_no), new_no: None, tag: LineTag::Del, spans: plain(&d.text) });
                            old_no += 1;
                        }
                        (None, Some(a)) => pending_adds.push(plain(&a.text)),
                        (None, None) => {}
                    }
                }
                for spans in pending_adds {
                    out.push(DiffRow { old_no: None, new_no: Some(new_no), tag: LineTag::Add, spans });
                    new_no += 1;
                }
            }
            LineTag::Add => {
                out.push(DiffRow { old_no: None, new_no: Some(new_no), tag: LineTag::Add, spans: plain(&l.text) });
                new_no += 1;
                i += 1;
            }
        }
    }
}

/// All rows (expanded) or the first `COLLAPSED_DIFF_ROWS` (collapsed), with
/// exact hidden counts so the hint can say `… +N rows · M hunks`.
///
/// A change that carries an `unavailable` reason returns
/// [`DiffView::Unavailable`] and its hunks are **not** painted, even in the
/// (today unreachable) state where a reason arrives alongside hunks: a reason
/// means the server chose to withhold, and `Redacted` — the one reason that
/// exists precisely because showing the lines was unsafe — makes that the
/// fail-closed direction. See [`DiffView`] for why this is a sum type.
#[must_use]
pub fn diff_rows(change: &FileChange, expanded: bool) -> DiffView {
    if let Some(why) = change.unavailable {
        return DiffView::Unavailable(why);
    }
    let budget = if expanded { LCS_CELL_BUDGET_EXPANDED } else { LCS_CELL_BUDGET_COLLAPSED };
    let limit = if expanded { EXPANDED_DIFF_ROWS } else { COLLAPSED_DIFF_ROWS };
    let mut all: Vec<DiffRow> = Vec::new();
    let mut hunk_starts: Vec<usize> = Vec::with_capacity(change.hunks.len());
    for h in &change.hunks {
        hunk_starts.push(all.len());
        hunk_rows(h, budget, &mut all);
    }
    let total = all.len();
    if total <= limit {
        return DiffView::Rows(DiffRows { rows: all, hidden_rows: 0, hidden_hunks: 0 });
    }
    // A hunk is hidden only when its FIRST row falls at or past the cut —
    // one whose start is still before `limit` has at least one visible row,
    // even if the cut lands exactly on the next hunk's boundary.
    let hidden_hunks = hunk_starts.iter().filter(|&&start| start >= limit).count();
    all.truncate(limit);
    DiffView::Rows(DiffRows {
        rows: all,
        hidden_rows: total - limit,
        hidden_hunks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::file_change::{FileChangeKind, HunkLine};

    fn line(tag: LineTag, s: &str) -> HunkLine { HunkLine { tag, text: s.into() } }

    fn change(hunks: Vec<Hunk>) -> FileChange {
        FileChange { path: "a.rs".into(), kind: FileChangeKind::Modified, hunks, added: 0, removed: 0, unavailable: None }
    }

    /// Unwrap the paintable arm; every caller below asserts on rows.
    fn rows(view: DiffView) -> DiffRows {
        match view {
            DiffView::Rows(r) => r,
            DiffView::Unavailable(why) => panic!("expected rows, got unavailable: {why:?}"),
        }
    }

    #[test]
    fn paired_del_add_get_word_emphasis_only_on_the_changed_tokens() {
        let (d, a) = word_spans("let x = foo(1);", "let x = bar(1);", LCS_CELL_BUDGET_EXPANDED);
        let em_d: Vec<&str> = d.iter().filter(|s| s.emphasis).map(|s| s.text.as_str()).collect();
        let em_a: Vec<&str> = a.iter().filter(|s| s.emphasis).map(|s| s.text.as_str()).collect();
        assert_eq!(em_d, vec!["foo"]);
        assert_eq!(em_a, vec!["bar"]);
    }

    #[test]
    fn line_numbers_advance_per_side_and_dels_precede_adds() {
        let c = change(vec![Hunk { old_start: 10, new_start: 10, lines: vec![
            line(LineTag::Ctx, "a"), line(LineTag::Del, "b"), line(LineTag::Del, "c"),
            line(LineTag::Add, "B"), line(LineTag::Ctx, "d"),
        ]}]);
        let r = rows(diff_rows(&c, true));
        let nums: Vec<(Option<u32>, Option<u32>, LineTag)> = r.rows.iter().map(|x| (x.old_no, x.new_no, x.tag)).collect();
        assert_eq!(nums, vec![
            (Some(10), Some(10), LineTag::Ctx),
            (Some(11), None, LineTag::Del),
            (Some(12), None, LineTag::Del),
            (None, Some(11), LineTag::Add),
            (Some(13), Some(12), LineTag::Ctx),
        ]);
    }

    #[test]
    fn collapsed_shows_two_rows_and_counts_hidden_rows_and_hunks() {
        let h = |start: u32| Hunk { old_start: start, new_start: start, lines: vec![line(LineTag::Ctx, "x"), line(LineTag::Add, "y"), line(LineTag::Ctx, "z")] };
        let c = change(vec![h(1), h(50), h(90)]);
        let r = rows(diff_rows(&c, false));
        assert_eq!(r.rows.len(), 2);
        assert_eq!(r.hidden_rows, 7);
        assert_eq!(r.hidden_hunks, 2);
    }

    #[test]
    fn over_budget_or_overlong_lines_fall_back_to_plain_spans() {
        let long = "a ".repeat(400); // 800 chars > MAX_INLINE_LINE_CHARS
        let (d, a) = word_spans(&long, &format!("{long}b"), LCS_CELL_BUDGET_EXPANDED);
        assert!(d.iter().all(|s| !s.emphasis) && a.iter().all(|s| !s.emphasis));
        let (d2, _) = word_spans("x y z", "x q z", 2); // budget too small for 5x5 tokens
        assert!(d2.iter().all(|s| !s.emphasis));
    }

    #[test]
    fn a_hunk_ending_exactly_at_the_limit_leaves_the_next_hunk_fully_hidden() {
        // hunk A is exactly COLLAPSED_DIFF_ROWS rows; hunk B is a distant,
        // unrelated hunk that must be entirely invisible after truncation.
        let a = Hunk { old_start: 1, new_start: 1, lines: vec![line(LineTag::Ctx, "a"), line(LineTag::Ctx, "b")] };
        let b = Hunk { old_start: 50, new_start: 50, lines: vec![line(LineTag::Ctx, "c"), line(LineTag::Ctx, "d"), line(LineTag::Ctx, "e")] };
        let c = change(vec![a, b]);
        let r = rows(diff_rows(&c, false));
        assert_eq!(r.rows.len(), 2);
        assert_eq!(r.hidden_rows, 3);
        assert_eq!(r.hidden_hunks, 1, "hunk B has zero visible rows and must count as hidden");
    }

    #[test]
    fn stats_label_reports_counts_or_the_unavailable_reason() {
        let mut c = change(vec![]);
        c.added = 12; c.removed = 3;
        assert_eq!(stats_label(&c), "+12 -3");
        c.unavailable = Some(Unavailable::TooLarge);
        assert_eq!(stats_label(&c), "+12 -3", "TooLarge keeps exact stats");
        c.unavailable = Some(Unavailable::PreImageUnavailable);
        assert!(stats_label(&c).starts_with("diff unavailable"));
    }

    #[test]
    fn stats_label_does_not_render_a_too_large_diffs_unknown_stats_as_plus_zero() {
        // `change()` defaults added/removed to 0 — the byte-size-cap
        // Modified case, where the stats were never computed.
        let mut c = change(vec![]);
        c.unavailable = Some(Unavailable::TooLarge);
        let label = stats_label(&c);
        assert!(!label.contains("+0"), "zero stats under TooLarge must not read as +0 -0: {label}");
        assert!(label.starts_with("diff unavailable"));
    }

    #[test]
    fn a_withheld_diff_and_an_empty_one_are_not_the_same_answer() {
        // The defect this guards: both used to return
        // `DiffRows { rows: [], hidden_rows: 0, hidden_hunks: 0 }` — byte for
        // byte the same value — so a renderer that painted rows and never
        // called `stats_label` showed nothing and said nothing about a diff
        // the server had deliberately withheld.
        let mut withheld = change(vec![]);
        withheld.unavailable = Some(Unavailable::Redacted);
        withheld.added = 4;
        withheld.removed = 1;
        let no_op = change(vec![]);

        assert_eq!(
            diff_rows(&withheld, true),
            DiffView::Unavailable(Unavailable::Redacted)
        );
        assert_eq!(diff_rows(&no_op, true), DiffView::Rows(DiffRows::default()));
        assert_ne!(diff_rows(&withheld, true), diff_rows(&no_op, true));
        // …and the reason still renders its sentence beside exact stats.
        assert_eq!(
            stats_label(&withheld),
            "diff unavailable: contained a secret"
        );
    }

    #[test]
    fn a_reason_wins_over_hunks_that_arrived_with_it() {
        // No producer emits this pair today (every server-side site clears
        // hunks in the same breath as it sets a reason), so this pins the
        // fail-closed direction rather than a live case: `Redacted` means
        // "these lines could not be shown safely", and painting them because
        // they happened to be present would be the one unrecoverable answer.
        let mut c = change(vec![Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![line(LineTag::Add, "AKIAIOSFODNN7EXAMPLE")],
        }]);
        c.unavailable = Some(Unavailable::Redacted);
        assert_eq!(
            diff_rows(&c, true),
            DiffView::Unavailable(Unavailable::Redacted)
        );
    }
}
