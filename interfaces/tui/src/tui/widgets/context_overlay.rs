// `/context` overlay: the measured layout of the last prompt this session
// sent, as proportion bars over the live window.
//
// # The one rule this surface exists to keep
//
// Every figure here is either measured or absent. `context.breakdown` reports
// `None` for what it cannot know, `reconcile` propagates that as
// `total: None`, and this widget paints `?`. A `0` in any of those places
// would read as "this costs nothing", which is the opposite of "we do not
// know" — and it is the reading a reader acts on (判据 §8, spec §8).

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use shared_ui_logic::transcript::ContextRow;

use crate::tui::app::{AppState, ContextView};
use crate::tui::theme::theme;

use super::status_bar::compact_tokens;

/// Cells of bar for a row at 100% of the window. Fixed rather than
/// width-derived so the bars of two runs at different terminal sizes mean the
/// same thing.
const BAR_CELLS: usize = 24;

/// Filled and empty bar cells. Half-blocks would imply a precision these
/// numbers do not have (the tool rows are `bytes / 4`).
const BAR_FULL: char = '\u{2588}';
const BAR_EMPTY: char = '\u{2591}';

/// What a number nobody measured is spelled as. One constant because it is
/// the same claim everywhere it appears.
const UNKNOWN: &str = "?";

/// Rows the overlay spends on something other than a measured row: two
/// borders, the headline, the blank under it, and the key hint. The footer is
/// counted separately because it is one or two lines depending on what there
/// is to say.
const CHROME_ROWS: u16 = 5;

/// `1.2 kB` / `847 B` / `3.4 MB` — byte sizes, distinct from the `k`/`M`
/// token spelling so the two cannot be confused at a glance.
fn compact_bytes(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1} MB", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1} kB", n as f64 / 1_000.0)
    } else {
        format!("{n} B")
    }
}

/// The bar for `tokens` out of `total`, or `None` when there is no total to
/// take a proportion of.
///
/// A bar drawn against Σrows instead would still be a bar — and it would read
/// as window occupancy, which is the number the reader came for. Absent is the
/// honest answer (判据 §17).
fn bar(tokens: u64, total: Option<u64>) -> Option<String> {
    let total = total?;
    if total == 0 {
        return None;
    }
    let filled = ((tokens as f64 / total as f64) * BAR_CELLS as f64).round() as usize;
    // A non-zero row always shows at least one cell: rounding a 0.4% row to
    // an empty bar says "nothing", which is a different claim from "a little".
    let filled = filled.clamp(usize::from(tokens > 0), BAR_CELLS);
    Some(
        std::iter::repeat_n(BAR_FULL, filled)
            .chain(std::iter::repeat_n(BAR_EMPTY, BAR_CELLS - filled))
            .collect(),
    )
}

/// `23%`, or `?` when there is no total.
///
/// A row that is present but rounds below one percent reads `<1%`: a bare `0%`
/// beside a visible sliver of bar says the row costs nothing, which is a
/// different claim from "less than a percent" (判据 §17).
fn share(tokens: u64, total: Option<u64>) -> String {
    match total {
        Some(t) if t > 0 => match (tokens * 100) / t {
            0 if tokens > 0 => "<1%".to_string(),
            pct => format!("{pct}%"),
        },
        _ => UNKNOWN.to_string(),
    }
}

/// The header: occupancy over the window, both of which can be unknown.
///
/// `?` on either side rather than a zero, and the two are independent — a
/// session with a known window and no gauge reads `? of 200.0k`, which is
/// exactly what it is.
#[must_use]
fn headline(view: &ContextView) -> String {
    let used = view
        .rows
        .total
        .map_or_else(|| UNKNOWN.to_string(), compact_tokens);
    let window = view
        .rows
        .window
        .map_or_else(|| UNKNOWN.to_string(), |w| compact_tokens(u64::from(w)));
    let pct = view
        .rows
        .percent
        .map_or_else(|| UNKNOWN.to_string(), |p| format!("{}%", p.round() as u64));
    format!("{pct} \u{00b7} {used} of {window}")
}

/// The footer: what the system prompt actually occupied, what the budget trim
/// cut from it, and what the rows do and do not cover.
///
/// The trim figure is the one thing this view knows that no other surface
/// does: the layer rows describe the prompt as ASSEMBLED, and for a session
/// over the system-prompt budget they overstate what the model received.
///
/// Two short lines rather than one long one, because the overlay does not wrap
/// — a row that wraps stops being a table — and a clipped footer is a wrong
/// label, not a shorter one (判据 §17).
#[must_use]
fn footer(view: &ContextView) -> Vec<String> {
    let mut first = match view.dynamic_sent_bytes {
        None => "no prompt measurement on this turn".to_string(),
        Some(_) => format!("prompt {} sent", compact_bytes(view.prompt_bytes_sent())),
    };
    if let Some(cut) = view.trimmed_bytes() {
        first.push_str(&format!(" \u{00b7} {} trimmed", compact_bytes(cut)));
    }
    let mut out = vec![first];
    // Only when there IS a remainder: naming `Other` when the row is not on
    // screen would describe something the reader cannot see.
    if view.rows.other > 0 {
        out.push(
            "rows: system prompt, pre-trim \u{00b7} Other: the rest of the window".to_string(),
        );
    }
    out
}

/// Render the overlay, centred over the transcript like the `/agents` detail
/// view — this is a report to read, not a picker to stand next to the
/// composer.
pub fn render_context_overlay(frame: &mut Frame, state: &AppState, area: Rect) {
    let Some(view) = &state.context_overlay else {
        return;
    };
    let footer = footer(view);
    // Height: everything the overlay wants to show, capped at the frame. A
    // fixed fraction would leave a tall empty box under a three-layer prompt
    // and clip a thirty-layer one just the same.
    let chrome = CHROME_ROWS + u16::try_from(footer.len()).unwrap_or(1);
    let wanted = u16::try_from(view.rows.rows.len())
        .unwrap_or(u16::MAX)
        .saturating_add(chrome);
    let margin_x = area.width / 10;
    let height = wanted.min(area.height);
    let rect = Rect::new(
        area.x + margin_x,
        area.y + (area.height.saturating_sub(height)) / 2,
        area.width.saturating_sub(margin_x * 2).max(20),
        height,
    );
    frame.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme().border_focused))
        .title(format!(" Context \u{00b7} turn {} ", view.turn));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    if inner.height < 3 {
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        headline(view),
        Style::default()
            .fg(theme().heading)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::default());

    // Rows: label, bar, tokens, share. The label column is sized to the
    // widest label present so the bars line up without a second pass.
    let label_width = view
        .rows
        .rows
        .iter()
        .map(|r| r.label.chars().count())
        .max()
        .unwrap_or(0)
        .min(28);
    // Every line that is not a measured row: the headline, the blank under
    // it, the hint, and one per footer line. The list gets what is left.
    let body_rows =
        usize::from(inner.height).saturating_sub(usize::from(CHROME_ROWS) - 2 + footer.len());
    let start = view.scroll.min(view.rows.rows.len().saturating_sub(1));
    for row in view.rows.rows.iter().skip(start).take(body_rows) {
        lines.push(row_line(row, view.rows.total, label_width));
    }

    for line in footer {
        lines.push(Line::from(Span::styled(
            line,
            Style::default().fg(theme().muted),
        )));
    }
    lines.push(Line::from(Span::styled(
        " \u{2191}\u{2193}/PgUp/PgDn scroll \u{00b7} esc close",
        Style::default().fg(theme().muted),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
}

/// One measured row. `Other` is dimmed: it is the part nobody attributed, and
/// it must not read like a component with a name.
fn row_line(row: &ContextRow, total: Option<u64>, label_width: usize) -> Line<'static> {
    let is_other = row.label == "Other";
    let label = if row.label.chars().count() > label_width {
        row.label.chars().take(label_width).collect::<String>()
    } else {
        format!("{:width$}", row.label, width = label_width)
    };
    let mut spans = vec![if is_other {
        Span::styled(format!("{label} "), Style::default().fg(theme().muted))
    } else {
        Span::raw(format!("{label} "))
    }];
    if let Some(bar) = bar(row.tokens, total) {
        spans.push(Span::styled(
            bar,
            Style::default().fg(if is_other {
                theme().muted
            } else {
                theme().primary
            }),
        ));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::raw(format!("{:>7}", compact_tokens(row.tokens))));
    spans.push(Span::styled(
        format!(" {:>4}", share(row.tokens, total)),
        Style::default().fg(theme().muted),
    ));
    if let Some(bytes) = row.bytes {
        spans.push(Span::styled(
            format!("  {}", compact_bytes(bytes)),
            Style::default().fg(theme().muted),
        ));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::context_breakdown::{ContextBreakdown, LayerSizeView, ToolSchemaSize};
    use ratatui::{backend::TestBackend, Terminal};

    fn breakdown() -> ContextBreakdown {
        ContextBreakdown {
            session_key: "agent:main".into(),
            turn: 4,
            layers: vec![
                LayerSizeView {
                    name: "identity".into(),
                    bytes: 4_000,
                    tokens: 1_000,
                    zone: "stable".into(),
                },
                LayerSizeView {
                    name: "memory".into(),
                    bytes: 8_000,
                    tokens: 2_000,
                    zone: "dynamic".into(),
                },
            ],
            tools: vec![ToolSchemaSize {
                name: "grep".into(),
                schema_bytes: 400,
                description_bytes: 400,
            }],
            messages_tokens: None,
            provider_reported: None,
            context_window: Some(200_000),
            dynamic_bytes_sent: Some(6_000),
        }
    }

    fn painted(view: ContextView, w: u16, h: u16) -> String {
        let mut state = AppState::new("s".into(), "m".into());
        state.context_overlay = Some(view);
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| render_context_overlay(f, &state, f.area()))
            .unwrap();
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<Vec<_>>()
            .chunks(w as usize)
            .map(|row| row.concat())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// **B7's guard.** A breakdown whose `provider_reported` is unknown — the
    /// state the gateway ALWAYS sends, and which stays unknown while this
    /// session has no gauge — paints `?`, never a zero.
    ///
    /// Asserting `0% ` would pass on a broken path: `total: None` collapsed to
    /// `unwrap_or(0)` renders `0% · 0 of 200.0k`, which reads as an empty
    /// window rather than an unmeasured one.
    ///
    /// Mutation-checked: `total.map_or_else(UNKNOWN, ..)` → `unwrap_or(0)` in
    /// either `headline` or `share` turns this red.
    #[test]
    fn an_unmeasured_total_paints_a_question_mark_not_a_zero() {
        let view = ContextView::new(&breakdown(), None);
        let head = headline(&view);
        assert_eq!(head, "? \u{00b7} ? of 200.0k", "got {head}");
        assert_eq!(share(1_000, None), UNKNOWN);
        let screen = painted(view, 80, 20);
        assert!(screen.contains('?'), "no ? on screen:\n{screen}");
        assert!(
            !screen.contains("0%"),
            "an unknown total must not paint a percentage:\n{screen}"
        );
    }

    /// With no total there is nothing to take a proportion of, so no bar is
    /// drawn at all — a bar against Σrows would read as window occupancy.
    #[test]
    fn no_total_means_no_bar_rather_than_a_bar_of_something_else() {
        assert_eq!(bar(1_000, None), None);
        assert_eq!(bar(1_000, Some(0)), None);
        let screen = painted(ContextView::new(&breakdown(), None), 80, 20);
        assert!(
            !screen.contains(BAR_FULL),
            "bars must not appear without a total:\n{screen}"
        );
    }

    /// A live gauge is what turns the rows into bars, and the header is the
    /// gauge's own percentage — the same number the status bar is painting.
    #[test]
    fn a_gauge_gives_every_row_its_share_of_the_window() {
        let view = ContextView::new(&breakdown(), Some((50_000, 200_000)));
        assert_eq!(headline(&view), "25% \u{00b7} 50.0k of 200.0k");
        let screen = painted(view, 80, 20);
        assert!(screen.contains(BAR_FULL), "expected bars:\n{screen}");
        assert!(screen.contains("identity"), "expected rows:\n{screen}");
        assert!(
            screen.contains("Other"),
            "the unattributed remainder must be named, not hidden:\n{screen}"
        );
    }

    /// A row with tokens always gets at least one cell: rounding a small row
    /// down to an empty bar claims it costs nothing.
    #[test]
    fn a_small_row_is_a_sliver_not_an_empty_bar() {
        let b = bar(1, Some(1_000_000)).expect("a total means a bar");
        assert_eq!(b.chars().filter(|c| *c == BAR_FULL).count(), 1);
        assert_eq!(b.chars().count(), BAR_CELLS);
        assert_eq!(bar(0, Some(1_000)).unwrap().chars().next(), Some(BAR_EMPTY));
        let full = bar(2_000, Some(1_000)).expect("bar");
        assert_eq!(
            full.chars().filter(|c| *c == BAR_FULL).count(),
            BAR_CELLS,
            "a row larger than the total must clamp, not overflow the bar"
        );
    }

    /// The trim is the fact this view uniquely holds: the rows describe the
    /// prompt BEFORE the system-prompt budget cut the dynamic suffix, and a
    /// reader comparing rows against the window would otherwise never learn
    /// that the model received less than they say.
    #[test]
    fn the_footer_says_the_rows_overstate_what_was_sent() {
        let view = ContextView::new(&breakdown(), None);
        let f = footer(&view).join("\n");
        assert!(f.contains("2.0 kB trimmed"), "got {f}");
        assert!(f.contains("prompt 10.0 kB sent"), "got {f}");
        assert!(
            !f.contains("Other"),
            "with no total there is no remainder row to explain: {f}"
        );
        let with_total = ContextView::new(&breakdown(), Some((50_000, 200_000)));
        assert!(
            footer(&with_total).join("\n").contains("Other"),
            "a remainder on screen must be explained"
        );
    }

    /// `dynamic_bytes_sent: None` is "no prompt was measured", and the footer
    /// must say that rather than reporting a trim of zero.
    #[test]
    fn an_unmeasured_prompt_says_so_instead_of_reporting_no_trim() {
        let mut b = breakdown();
        b.dynamic_bytes_sent = None;
        let f = footer(&ContextView::new(&b, None)).join("\n");
        assert!(f.contains("no prompt measurement"), "got {f}");
        assert!(!f.contains("trimmed"), "got {f}");
        assert!(
            !f.contains("sent"),
            "an unmeasured prompt has no sent size to report: {f}"
        );
    }

    /// A terminal too short for the list still paints the header and the
    /// footer — the numbers a reader came for — instead of letting the rows
    /// push them off the bottom.
    ///
    /// The fixture has far more layers than the overlay can show on purpose:
    /// with only the two-layer one, the list is short enough that a wrong row
    /// budget still leaves the footer on screen, and the test passes without
    /// measuring anything (判据 §10). Mutation-checked: `inner.height - 4` →
    /// `- 2` (forgetting the two lines below the list) turns this red.
    #[test]
    fn a_short_overlay_keeps_the_headline_and_the_footer() {
        let mut b = breakdown();
        b.layers = (0..12)
            .map(|i| LayerSizeView {
                name: format!("layer{i}"),
                bytes: 100,
                tokens: 100,
                zone: "stable".into(),
            })
            .collect();
        let screen = painted(ContextView::new(&b, Some((50_000, 200_000))), 80, 9);
        assert!(screen.contains("25%"), "headline missing:\n{screen}");
        assert!(screen.contains("sent"), "footer missing:\n{screen}");
        assert!(screen.contains("esc close"), "hint missing:\n{screen}");
    }
}
