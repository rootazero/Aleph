//! One tool call as a header line plus a folded `⎿` output slot.
//!
//! ```text
//! ⏺ Read(src/gateway/mod.rs:1-120)
//!   ⎿ Read 120 lines
//! ⏺ Bash(cargo test -p aleph-tui)  ✗ 1.2s
//!   ⎿ error[E0433]: failed to resolve …
//!     … +41 lines (ctrl+o to expand)
//! ⏺ Edit(src/tui/theme.rs)  +12 -3
//!   ⎿  30 │ - pub const DEFAULT_THEME
//!      30 │ + pub fn resolve(role: SemanticColor)
//!     … +2 hunks (ctrl+o to expand)
//! ```
//!
//! Replaces the 3–5-line bordered box this crate drew per call, which spent
//! most of a tool-heavy transcript on border glyphs.
//!
//! # What this file decides, and what it does not
//!
//! Nothing here knows how many rows to show, which end of the output to keep,
//! how to word a hint, or what a diff row contains: `fold`, `FoldPolicy`,
//! `diff_rows`, `stats_label` and `expand_hint` all live in
//! `shared_ui_logic::transcript` so the Panel reaches the same answers. This
//! file turns those answers into `ratatui` spans and decides the column
//! arithmetic, which is the only part a terminal has that a DOM does not.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use shared_ui_logic::transcript::{
    diff_rows, expand_hint, fmt_duration_ms, fold, spinner_frame, stats_label, DiffView, FoldBody,
    FoldPolicy, Modality, RowBody, RowStatus, SemanticColor, ToolGroup, ToolRow,
};

use crate::tui::theme::palette;

/// First body row's prefix (`  ⎿ `).
const BODY_FIRST: &str = "  \u{23bf} ";
/// Every body row after the first, and the hint row.
const BODY_CONT: &str = "    ";
/// Columns [`BODY_FIRST`] and [`BODY_CONT`] both occupy.
const BODY_INDENT: u16 = 4;

/// A successful call shows its duration only past this, so a transcript of
/// fast reads stays quiet while a 40-second build is labelled.
///
/// A failure always shows one: when something went wrong, how long it took
/// before it did is part of the report.
const SLOW_OK_MS: u64 = 1_000;

/// How a row says "there is more" — the keyboard wording until mouse capture
/// is on, at which point the caller passes [`Modality::Mouse`].
///
/// Threaded rather than assumed because the hint must describe an affordance
/// that actually exists: a terminal that refused mouse capture, or a build
/// that has not enabled it, would otherwise tell the user to click something
/// nothing is listening for (spec §8).
pub const KEY_MODALITY: Modality = Modality::Key("ctrl+o");

fn style(role: SemanticColor) -> Style {
    Style::default().fg(palette().color(role))
}

/// Status glyph and its role.
///
/// # Every glyph here must be ONE column
///
/// The name after the glyph is not re-aligned per state, so a status whose
/// glyph is two columns wide moves that row's text sideways the moment the
/// call starts running and back again when it settles — a whole transcript
/// walking as calls complete. Padding does not buy the alignment back:
/// `ratatui` budgets cells with the same `unicode-width` any padding would be
/// computed from, so a wide glyph stays wide however many spaces follow it.
/// The glyph itself has to be narrow.
///
/// `tests::the_status_glyphs_are_all_one_column` measures every glyph this
/// function can return, plus the group headline's `●`.
///
/// `Pending` is a filled circle in the pending colour rather than a spinner:
/// a row restored from a log without a start event settles to `Pending`
/// (`ToolRow::settle_resumed`), and a spinner there would be the "turns
/// forever" bug that rule exists to prevent.
fn status_glyph(status: &RowStatus, now_ms: u64) -> (String, SemanticColor) {
    match status {
        RowStatus::Pending => ("\u{23fa}".to_string(), SemanticColor::ToolPending),
        RowStatus::Running { .. } => (
            spinner_frame(now_ms).to_string(),
            SemanticColor::ToolRunning,
        ),
        RowStatus::Ok { .. } => ("\u{23fa}".to_string(), SemanticColor::ToolOk),
        RowStatus::Err { .. } => ("\u{23fa}".to_string(), SemanticColor::ToolErr),
    }
}

/// The right-hand side of the header: diff stats, a failure mark, or nothing.
fn header_suffix(row: &ToolRow) -> Option<(String, SemanticColor)> {
    // Stats win over duration: `+12 -3` is what an edit is, and the reason the
    // server computed a `FileChange` at all. One file's label; several files
    // are summed by the body, which lists them.
    if let RowBody::FileChanges(changes) = &row.body {
        if let [only] = changes.as_slice() {
            return Some((stats_label(only), SemanticColor::Dim));
        }
        return Some((format!("{} files", changes.len()), SemanticColor::Dim));
    }
    match &row.status {
        RowStatus::Err { duration_ms, .. } => Some((
            format!("\u{2717} {}", fmt_duration_ms(*duration_ms)),
            SemanticColor::ToolErr,
        )),
        RowStatus::Ok { duration_ms } if *duration_ms >= SLOW_OK_MS => {
            Some((fmt_duration_ms(*duration_ms), SemanticColor::Dim))
        }
        _ => None,
    }
}

/// `⏺ Read(src/gateway/mod.rs:1-120)  +12 -3`
///
/// One column of glyph plus one space, then the name — see [`status_glyph`]
/// for why that first cell is not allowed to change width.
fn header_line(row: &ToolRow, now_ms: u64) -> Line<'static> {
    let (glyph, glyph_role) = status_glyph(&row.status, now_ms);
    let mut spans = vec![
        Span::styled(format!("{glyph} "), style(glyph_role)),
        Span::styled(
            row.summary.display_name.clone(),
            style(SemanticColor::Accent).add_modifier(Modifier::BOLD),
        ),
    ];
    if !row.summary.args_text.is_empty() {
        spans.push(Span::styled(
            format!("({})", row.summary.args_text),
            style(SemanticColor::Dim),
        ));
    }
    if let Some((text, role)) = header_suffix(row) {
        spans.push(Span::styled(format!("  {text}"), style(role)));
    }
    Line::from(spans)
}

/// Prefix for body row `i`, so the `⎿` appears once.
fn body_prefix(i: usize) -> Span<'static> {
    let text = if i == 0 { BODY_FIRST } else { BODY_CONT };
    Span::styled(text.to_string(), style(SemanticColor::ToolRail))
}

fn hint_line(hidden: usize, modality: Modality) -> Line<'static> {
    Line::from(vec![
        Span::styled(BODY_CONT.to_string(), style(SemanticColor::ToolRail)),
        Span::styled(expand_hint(modality, hidden), style(SemanticColor::Dim)),
    ])
}

/// The `⎿` slot for a text body.
fn text_body(
    text: &str,
    display_name: &str,
    width: u16,
    expanded: bool,
    modality: Modality,
    out: &mut Vec<Line<'static>>,
) {
    let body_width = width.saturating_sub(BODY_INDENT);
    let lines: Vec<&str> = text.lines().collect();
    let policy = if expanded {
        // Expanded means "show it all", which is a policy with no cap rather
        // than a different anchor: keeping `for_display_name`'s anchor here
        // would make an expanded Bash show its TAIL and call it everything.
        FoldPolicy {
            collapsed_rows: u16::MAX,
            anchor: shared_ui_logic::transcript::FoldAnchor::Head,
            body: FoldBody::Show,
        }
    } else {
        FoldPolicy::for_display_name(display_name)
    };
    let folded = fold(&lines, body_width, policy);

    // A policy that hides the body has nothing to show and something to say:
    // `Read 120 lines`. The count is logical lines, which is what a reader
    // means by "lines" for a file — the *hint* below counts physical rows,
    // because that is what expanding would actually add to the screen.
    if policy.body == FoldBody::Hide && !expanded {
        if folded.hidden_lines > 0 {
            let unit = if folded.hidden_lines == 1 {
                "line"
            } else {
                "lines"
            };
            out.push(Line::from(vec![
                body_prefix(0),
                Span::styled(
                    format!("{display_name} {} {unit}", folded.hidden_lines),
                    style(SemanticColor::Dim),
                ),
            ]));
        }
        return;
    }

    for (i, row) in folded.rows.iter().enumerate() {
        out.push(Line::from(vec![
            body_prefix(i),
            Span::styled(row.clone(), style(SemanticColor::Fg)),
        ]));
    }
    if folded.hidden_rows > 0 {
        out.push(hint_line(folded.hidden_rows, modality));
    }
}

/// One diff row's gutter number: the side that line exists on.
fn gutter(row: &shared_ui_logic::transcript::DiffRow) -> String {
    use aleph_protocol::file_change::LineTag;
    let n = match row.tag {
        LineTag::Del => row.old_no,
        LineTag::Add | LineTag::Ctx => row.new_no.or(row.old_no),
    };
    match n {
        Some(n) => format!("{n:>4}"),
        None => "    ".to_string(),
    }
}

fn tag_mark(tag: aleph_protocol::file_change::LineTag) -> (&'static str, SemanticColor) {
    use aleph_protocol::file_change::LineTag;
    match tag {
        LineTag::Add => ("+", SemanticColor::DiffAdd),
        LineTag::Del => ("-", SemanticColor::DiffDel),
        LineTag::Ctx => (" ", SemanticColor::DiffCtx),
    }
}

/// The `⎿` slot for a structured change set.
fn file_changes_body(
    changes: &[aleph_protocol::file_change::FileChange],
    expanded: bool,
    modality: Modality,
    out: &mut Vec<Line<'static>>,
) {
    let mut emitted = 0usize;
    for change in changes {
        // Several files in one call (`apply_patch`) need their names; a single
        // file already has it in the header's `args_text`.
        if changes.len() > 1 {
            out.push(Line::from(vec![
                body_prefix(emitted),
                Span::styled(
                    format!("{}  {}", change.path, stats_label(change)),
                    style(SemanticColor::Dim),
                ),
            ]));
            emitted += 1;
        }
        match diff_rows(change, expanded) {
            // Says WHY, and never falls back to rendering the change as if it
            // had simply touched nothing (spec §8).
            DiffView::Unavailable(_) => {
                out.push(Line::from(vec![
                    body_prefix(emitted),
                    Span::styled(stats_label(change), style(SemanticColor::Dim)),
                ]));
                emitted += 1;
            }
            DiffView::Rows(rows) => {
                for row in &rows.rows {
                    let (mark, role) = tag_mark(row.tag);
                    let mut spans = vec![
                        body_prefix(emitted),
                        Span::styled(
                            format!("{} \u{2502} {mark}", gutter(row)),
                            style(SemanticColor::DiffGutter),
                        ),
                    ];
                    for s in &row.spans {
                        let mut st = style(role);
                        if s.emphasis {
                            st = st.add_modifier(Modifier::REVERSED);
                        }
                        spans.push(Span::styled(s.text.clone(), st));
                    }
                    out.push(Line::from(spans));
                    emitted += 1;
                }
                if rows.hidden_hunks > 0 {
                    let unit = if rows.hidden_hunks == 1 {
                        "hunk"
                    } else {
                        "hunks"
                    };
                    out.push(Line::from(vec![
                        Span::styled(BODY_CONT.to_string(), style(SemanticColor::ToolRail)),
                        Span::styled(
                            format!(
                                "\u{2026} +{} {unit} ({} to expand)",
                                rows.hidden_hunks,
                                match modality {
                                    Modality::Mouse => "click",
                                    Modality::Key(k) => k,
                                }
                            ),
                            style(SemanticColor::Dim),
                        ),
                    ]));
                    emitted += 1;
                } else if rows.hidden_rows > 0 {
                    out.push(hint_line(rows.hidden_rows, modality));
                    emitted += 1;
                }
            }
        }
    }
}

/// Render one tool row.
#[must_use]
pub fn render_tool_row(
    row: &ToolRow,
    now_ms: u64,
    width: u16,
    modality: Modality,
) -> Vec<Line<'static>> {
    let mut out = vec![header_line(row, now_ms)];
    if width <= BODY_INDENT {
        return out;
    }
    match &row.body {
        RowBody::None => {}
        RowBody::Text(text) if !text.trim().is_empty() => text_body(
            text,
            &row.summary.display_name,
            width,
            row.expanded,
            modality,
            &mut out,
        ),
        RowBody::Text(_) => {}
        RowBody::FileChanges(changes) => {
            file_changes_body(changes, row.expanded, modality, &mut out);
        }
    }
    // An error that printed nothing still has to say what went wrong —
    // otherwise a failed call renders as a bare header and reads like a
    // success with a cross next to it.
    //
    // Keyed on "nothing was drawn" rather than on `RowBody::None`: a body of
    // whitespace is skipped above and is just as empty on screen, and keying
    // on the variant would leave exactly that case silent.
    if let RowStatus::Err { message, .. } = &row.status {
        if out.len() == 1 && !message.is_empty() {
            out.push(Line::from(vec![
                body_prefix(0),
                Span::styled(message.clone(), style(SemanticColor::ToolErr)),
            ]));
        }
    }
    out
}

/// `● Explored 4 calls · 0.8s`, with the member rows when expanded.
#[must_use]
pub fn render_tool_group(
    group: &ToolGroup,
    now_ms: u64,
    width: u16,
    modality: Modality,
) -> Vec<Line<'static>> {
    // `●` U+25CF, one column like every row glyph, so a group headline and the
    // rows under it start their text in the same column.
    let mut out = vec![Line::from(vec![
        Span::styled("\u{25cf} ", style(SemanticColor::Dim)),
        Span::styled(group.headline(), style(SemanticColor::Dim)),
    ])];
    if group.expanded {
        for row in &group.rows {
            out.extend(render_tool_row(row, now_ms, width, modality));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::file_change::{
        FileChange, FileChangeKind, Hunk, HunkLine, LineTag, Unavailable,
    };
    use aleph_protocol::ToolResult;
    use serde_json::json;

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn finished(tool: &str, args: serde_json::Value, output: &str) -> ToolRow {
        let mut row = ToolRow::new("c1", tool, &args);
        row.start(0);
        row.finish(&ToolResult::success(output), 10, 10);
        row
    }

    #[test]
    fn a_read_shows_a_count_and_no_body() {
        let row = finished(
            "file_read",
            json!({ "path": "src/lib.rs" }),
            &"line\n".repeat(120),
        );
        let out = text(&render_tool_row(&row, 0, 80, KEY_MODALITY));
        assert_eq!(out.len(), 2, "header + one caption: {out:?}");
        assert!(out[0].starts_with("\u{23fa} Read(src/lib.rs)"), "{out:?}");
        assert!(out[1].contains("Read 120 lines"), "{out:?}");
    }

    /// The hint counts PHYSICAL rows, so one very long logical line still
    /// reports what expanding would add to the screen.
    ///
    /// # When this goes red
    ///
    /// Swap `fold` for anything that counts `text.lines()` and this drops to
    /// `+0`, because the body is a single logical line.
    #[test]
    fn the_hint_counts_physical_rows_not_logical_lines() {
        let blob = "x".repeat(6_000);
        let row = finished("bash", json!({ "command": "cat big.json" }), &blob);
        let out = text(&render_tool_row(&row, 0, 80, KEY_MODALITY));
        let hint = out.last().expect("a hint row");
        assert!(hint.contains("ctrl+o to expand"), "{hint}");
        // 6000 columns of body at 76 usable columns ≈ 79 rows, of which the
        // policy shows 2.
        let hidden: usize = hint
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .expect("the hint names a number");
        assert!(hidden > 70, "physical rows, got {hidden}: {hint}");
    }

    /// A shell failure keeps the END of the output, which is where the error
    /// is, and says so with a cross.
    #[test]
    fn a_failed_shell_keeps_the_tail() {
        let mut row = ToolRow::new("c1", "bash", &json!({ "command": "cargo test" }));
        row.start(0);
        let mut body = String::new();
        for i in 0..40 {
            body.push_str(&format!("line {i}\n"));
        }
        body.push_str("error[E0433]: failed to resolve\n");
        // Built field-wise: a failing tool has BOTH an error and the output it
        // produced before failing, and no constructor spells that pair.
        let result = ToolResult {
            success: false,
            output: Some(body),
            error: Some("boom".into()),
            presentation: None,
        };
        row.finish(&result, 1_200, 1_200);
        let out = text(&render_tool_row(&row, 0, 80, KEY_MODALITY));
        assert!(out[0].contains("\u{2717} 1.2s"), "{out:?}");
        assert!(
            out.iter().any(|l| l.contains("error[E0433]")),
            "the tail must survive: {out:?}"
        );
    }

    #[test]
    fn an_edit_shows_stats_and_gutter_rows() {
        let change = FileChange {
            path: "src/tui/theme.rs".into(),
            kind: FileChangeKind::Modified,
            hunks: vec![Hunk {
                old_start: 30,
                new_start: 30,
                lines: vec![
                    HunkLine {
                        tag: LineTag::Del,
                        text: "pub const DEFAULT_THEME".into(),
                    },
                    HunkLine {
                        tag: LineTag::Add,
                        text: "pub fn resolve(role: SemanticColor)".into(),
                    },
                ],
            }],
            added: 12,
            removed: 3,
            unavailable: None,
        };
        let mut row = ToolRow::new(
            "c1",
            "file_edit",
            &json!({ "file_path": "src/tui/theme.rs" }),
        );
        row.start(0);
        row.finish(
            &ToolResult::success("ok").with_presentation(Some(
                aleph_protocol::file_change::Presentation::FileChanges {
                    changes: vec![change],
                },
            )),
            10,
            10,
        );
        let out = text(&render_tool_row(&row, 0, 80, KEY_MODALITY));
        assert!(out[0].contains("+12 -3"), "{out:?}");
        assert!(out.iter().any(|l| l.contains("30 \u{2502} -")), "{out:?}");
        assert!(out.iter().any(|l| l.contains("30 \u{2502} +")), "{out:?}");
    }

    /// A withheld diff renders the REASON. The failure this guards against is
    /// rendering it as a change that touched nothing.
    #[test]
    fn an_unavailable_diff_says_why() {
        let change =
            FileChange::unavailable("big.bin", FileChangeKind::Modified, Unavailable::Binary);
        let mut row = ToolRow::new("c1", "file_write", &json!({ "path": "big.bin" }));
        row.start(0);
        row.finish(
            &ToolResult::success("ok").with_presentation(Some(
                aleph_protocol::file_change::Presentation::FileChanges {
                    changes: vec![change],
                },
            )),
            10,
            10,
        );
        let out = text(&render_tool_row(&row, 0, 80, KEY_MODALITY));
        assert!(
            out.iter().any(|l| l.contains("binary")),
            "the reason must be on screen: {out:?}"
        );
        assert!(
            !out.iter().any(|l| l.contains("+0 -0")),
            "an unavailable diff must not read as an empty one: {out:?}"
        );
    }

    /// A row restored without a start event must not spin.
    #[test]
    fn a_resumed_row_shows_a_dot_not_a_spinner() {
        let mut row = ToolRow::new("c1", "grep", &json!({ "pattern": "x" }));
        row.status = RowStatus::Running { since_ms: 0 };
        row.settle_resumed();
        let out = text(&render_tool_row(&row, 0, 80, KEY_MODALITY));
        assert!(out[0].starts_with("\u{23fa} "), "{out:?}");
        for frame in shared_ui_logic::transcript::SPINNER_FRAMES {
            assert!(!out[0].starts_with(frame), "spinning after resume: {out:?}");
        }
    }

    /// Expanding must show the WHOLE body from the top, not a larger tail.
    #[test]
    fn expanding_a_shell_row_shows_the_head_too() {
        let mut row = ToolRow::new("c1", "bash", &json!({ "command": "ls" }));
        row.start(0);
        let body: String = (0..20).map(|i| format!("line {i}\n")).collect();
        row.finish(&ToolResult::success(&body), 10, 10);
        row.expanded = true;
        let out = text(&render_tool_row(&row, 0, 80, KEY_MODALITY));
        assert!(out.iter().any(|l| l.contains("line 0")), "{out:?}");
        assert!(out.iter().any(|l| l.contains("line 19")), "{out:?}");
    }

    /// Every glyph that can open a row is exactly one column, so the name
    /// after it does not walk sideways as a call runs and settles.
    ///
    /// # When this goes red
    ///
    /// Swap any status glyph for a two-column one — an emoji, `🔴`, a boxed
    /// mark — and this fails. That is the whole point: the alignment cannot be
    /// bought back with padding, because `ratatui` budgets cells with the same
    /// `unicode-width` the padding would be computed from. The glyph itself has
    /// to be narrow.
    ///
    /// Measured through `unicode-width` rather than `chars().count()`, which
    /// reports one for a wide glyph too and would agree with the bug.
    ///
    /// What this CANNOT see: a terminal that paints a glyph wider than
    /// `unicode-width` claims. That disagreement is invisible to every test in
    /// this repo and is on the real-machine list.
    #[test]
    fn the_status_glyphs_are_all_one_column() {
        use unicode_width::UnicodeWidthStr;
        let mut seen = vec![
            status_glyph(&RowStatus::Pending, 0).0,
            status_glyph(&RowStatus::Ok { duration_ms: 1 }, 0).0,
            status_glyph(
                &RowStatus::Err {
                    duration_ms: 1,
                    message: "boom".into(),
                },
                0,
            )
            .0,
            // The group headline's own glyph, which shares the column.
            "\u{25cf}".to_string(),
        ];
        // Every frame, not whichever one `now_ms = 0` happens to select.
        for frame in shared_ui_logic::transcript::SPINNER_FRAMES {
            seen.push(frame.to_string());
        }
        for glyph in seen {
            assert_eq!(
                UnicodeWidthStr::width(glyph.as_str()),
                1,
                "{glyph:?} is not one column wide"
            );
        }
    }

    /// The hint has to describe an affordance that exists.
    #[test]
    fn the_hint_follows_the_modality_it_is_given() {
        let row = finished("bash", json!({ "command": "ls" }), &"x\n".repeat(50));
        let keyed = text(&render_tool_row(&row, 0, 80, KEY_MODALITY));
        assert!(keyed.last().unwrap().contains("ctrl+o to expand"));
        let moused = text(&render_tool_row(&row, 0, 80, Modality::Mouse));
        assert!(moused.last().unwrap().contains("click to expand"));
    }
}
