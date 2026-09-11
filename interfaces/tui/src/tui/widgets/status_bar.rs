// Status bar widget: a single line of conversation STATE — what is serving
// it, how full its context is, what it has cost, what it is doing right now.
//
// `● model │ ctx 23% (50k/200k) │ $0.042 │ tier:ask │ ⚡2 │ ⠹ Pondering… 12s`
//
// Keys live on the hint line (`widgets::hint_line`), not here.
//
// # Why the segments are a list and not a format string
//
// A terminal can be 40 columns wide. Concatenating everything and letting
// ratatui clip means the RIGHTMOST thing is what disappears, and the
// rightmost thing is the live working indicator — the one segment a reader is
// actually watching. So each segment declares what it is, the line drops
// segments by priority until it fits, and the priority is a `match` on the
// kind rather than declaration order: adding a segment is then a decision
// someone has to write down (same shape as `regions::RegionKind::priority`).

use std::time::Duration;

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use shared_ui_logic::transcript::{verb, Locale, VERB_REROLL_MS};
use unicode_width::UnicodeWidthStr;

use crate::tui::app::{CostView, SessionKnobs};
use crate::tui::slash::{SessionKnob, ToolProgressMode};
use crate::tui::theme::{spinner_at, theme};

/// What a segment is. Display order is the order they are built in `render`;
/// this enum's order means nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegKind {
    /// Connection dot + model name. One segment: a dot with no model names
    /// nothing, and a model with no dot hides whether it is reachable.
    Status,
    Ctx,
    Cost,
    /// One per set session knob, so a narrow line sheds them one at a time.
    Knob,
    Agents,
    Cache,
    ToolMode,
    Tokens,
    Session,
    /// Spinner + verb + elapsed, while a run is in flight. Absent when idle —
    /// the idle help hint lives on the hint line now.
    Activity,
}

impl SegKind {
    /// Lower survives longer. Written as a `match` so a new segment cannot
    /// inherit an answer from where it happened to be typed.
    ///
    /// The order says: what it is doing beats what is serving it, which beats
    /// how much room and money are left, which beats everything that is
    /// merely a number you can also get from `/usage`.
    const fn priority(self) -> u8 {
        match self {
            Self::Activity => 0,
            Self::Status => 1,
            Self::Ctx => 2,
            Self::Cost => 3,
            Self::Knob => 4,
            Self::Agents => 5,
            Self::Cache => 6,
            Self::ToolMode => 7,
            Self::Tokens => 8,
            Self::Session => 9,
        }
    }
}

struct Segment {
    spans: Vec<Span<'static>>,
    kind: SegKind,
}

impl Segment {
    fn one(kind: SegKind, text: String, style: Style) -> Self {
        Self {
            spans: vec![Span::styled(text, style)],
            kind,
        }
    }

    fn width(&self) -> usize {
        self.spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum()
    }
}

/// The `│` between segments.
const SEP: &str = "\u{2502}";

fn total_width(segments: &[Segment]) -> usize {
    let bars = segments.len().saturating_sub(1);
    segments.iter().map(Segment::width).sum::<usize>() + bars
}

/// Drop segments until the line fits, highest priority number first, and
/// within one priority the rightmost first.
///
/// Never returns empty: when even the single most important segment is wider
/// than the terminal, it is kept and the paint clips it — a blank status bar
/// is not a better answer than a truncated one.
fn fit(mut segments: Vec<Segment>, width: usize) -> Vec<Segment> {
    while segments.len() > 1 && total_width(&segments) > width {
        let Some(worst) = segments.iter().map(|s| s.kind.priority()).max() else {
            break;
        };
        let Some(idx) = segments.iter().rposition(|s| s.kind.priority() == worst) else {
            break;
        };
        segments.remove(idx);
    }
    segments
}

pub struct StatusBar<'a> {
    pub model: &'a str,
    pub session: &'a str,
    pub tokens: u64,
    /// Live context-window occupancy `(used, window)` from the latest gauge
    /// event, or `None` when unknown. Rendered as `ctx 23% (12.3k/200.0k)`,
    /// tinted by fill ratio.
    pub context_gauge: Option<(u32, u32)>,
    /// Priced spend on this conversation — see [`crate::tui::app::CostTally`].
    /// Renders `$?` rather than `$0.000` when nothing priced is known.
    pub cost: CostView,
    /// Last-call prompt-cache hit rate as a rounded percentage (0–100), or
    /// `None` when no call has reported cache activity. Rendered as a
    /// `cache N%` segment — a sudden drop is the live signal that a prefix
    /// bust just happened.
    pub cache_stat: Option<u64>,
    /// Agent id behind `cache_stat` when it is not the session root's, so a
    /// delegated sub-agent's cold start is labelled instead of being read as
    /// the root agent's prefix breaking.
    pub cache_stat_agent: Option<&'a str>,
    /// This session's background sub-agents still running. Rendered as a
    /// `⚡N agents` segment only when non-zero — pi's "3 running agents"
    /// footer, in Aleph's status-bar vocabulary.
    pub running_agents: usize,
    pub is_connected: bool,
    pub tool_progress_mode: ToolProgressMode,
    /// Advances the working-indicator spinner (shared 50ms tick counter).
    pub spinner_frame: usize,
    /// Elapsed time of the active run, or `None` when idle.
    pub run_elapsed: Option<Duration>,
    /// The active run's id, used only to salt the working verb so two runs do
    /// not open with the same word. `None` when idle.
    pub run_id: Option<&'a str>,
    /// Which language the working verb is drawn in.
    pub locale: Locale,
    /// This conversation's persisted knobs, as the server last reported them.
    ///
    /// Each is `None` when the session carries no override — rendered as
    /// *nothing*, not as a guessed value: the TUI does not read the server's
    /// config, so printing "auto" for an unset tier would be this client
    /// inventing a fact. An unset knob is invisible; a set one is named.
    pub knobs: SessionKnobs<'a>,
}

impl StatusBar<'_> {
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let text_style = Style::default().fg(theme().status_fg).bg(theme().status_bg);
        let segments = fit(self.segments(), area.width as usize);

        let sep_style = Style::default().fg(theme().muted).bg(theme().status_bg);
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (i, seg) in segments.into_iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(SEP.to_string(), sep_style));
            }
            spans.extend(seg.spans);
        }
        if spans.is_empty() {
            spans.push(Span::styled(String::new(), text_style));
        }

        let paragraph =
            Paragraph::new(Line::from(spans)).style(Style::default().bg(theme().status_bg));
        frame.render_widget(paragraph, area);
    }

    /// Every segment this state has to show, in display order.
    fn segments(&self) -> Vec<Segment> {
        let text_style = Style::default().fg(theme().status_fg).bg(theme().status_bg);
        let mut out: Vec<Segment> = Vec::new();

        let (dot, dot_color) = if self.is_connected {
            ("\u{25cf}", theme().connected) // ●
        } else {
            ("\u{25cb}", theme().disconnected) // ○
        };
        out.push(Segment {
            spans: vec![
                Span::styled(" ".to_string(), text_style),
                Span::styled(
                    dot.to_string(),
                    Style::default().fg(dot_color).bg(theme().status_bg),
                ),
                Span::styled(format!(" {} ", self.model), text_style),
            ],
            kind: SegKind::Status,
        });

        // Context-window gauge, tinted by fill ratio so a run approaching the
        // window edge reads at a glance. Only shown once a `ContextGauge`
        // event has supplied a real denominator.
        if let Some((used, window)) = self.context_gauge.filter(|&(_, w)| w > 0) {
            out.push(Segment::one(
                SegKind::Ctx,
                format!(" ctx {} ", format_context_gauge(used, window)),
                Style::default()
                    .fg(context_gauge_color(used, window))
                    .bg(theme().status_bg),
            ));
        }

        out.push(Segment::one(
            SegKind::Cost,
            format!(" {} ", self.cost.label()),
            text_style,
        ));

        // The conversation's own settings — the reason reopening a terminal
        // mid-task now lands you back where you were rather than on the
        // install defaults. Enumerated from `SessionKnob::ALL` so a knob added
        // to the parser cannot quietly fail to appear here.
        for knob in SessionKnob::ALL {
            let Some(value) = knob_value(&self.knobs, knob) else {
                continue;
            };
            out.push(Segment::one(
                SegKind::Knob,
                format!(" {}:{value} ", knob.command()),
                text_style,
            ));
        }

        // Background sub-agents still working for this session. Absent at
        // zero — an idle session does not need a zero.
        if self.running_agents > 0 {
            out.push(Segment::one(
                SegKind::Agents,
                format!(
                    " \u{26a1}{} agent{} ",
                    self.running_agents,
                    if self.running_agents == 1 { "" } else { "s" }
                ),
                Style::default()
                    .fg(theme().tool_running)
                    .bg(theme().status_bg),
            ));
        }

        // Last-call prompt-cache hit rate, shown only once a provider call has
        // reported cache activity. Dimmed to a warning tint under 50% — a low
        // last-call rate right after a healthy streak is the live symptom of a
        // stable-prefix bust.
        if let Some(pct) = self.cache_stat {
            let label = match self.cache_stat_agent {
                Some(agent) => format!(" cache {pct}% \u{b7}{agent} "),
                None => format!(" cache {pct}% "),
            };
            out.push(Segment::one(
                SegKind::Cache,
                label,
                Style::default()
                    .fg(cache_stat_color(pct))
                    .bg(theme().status_bg),
            ));
        }

        out.push(Segment::one(
            SegKind::ToolMode,
            format!(" T:{} ", self.tool_progress_mode.glyph()),
            text_style,
        ));
        out.push(Segment::one(
            SegKind::Tokens,
            format!(" {} ", format_tokens(self.tokens)),
            text_style,
        ));
        out.push(Segment::one(
            SegKind::Session,
            format!(" {} ", self.session),
            text_style,
        ));

        if let Some(elapsed) = self.run_elapsed {
            out.push(Segment::one(
                SegKind::Activity,
                format!(
                    " {} {}\u{2026} {}s ",
                    spinner_at(self.spinner_frame),
                    working_verb(self.locale, self.run_id.unwrap_or_default(), elapsed),
                    elapsed.as_secs()
                ),
                Style::default()
                    .fg(theme().tool_running)
                    .bg(theme().status_bg),
            ));
        }

        out
    }
}

/// The word in `⠹ Pondering… 12s`.
///
/// Two inputs, both already on hand, and no clock of its own:
///
/// * the elapsed time picks the 7-second window (`VERB_REROLL_MS`), so the
///   word changes while you wait rather than sitting there for a minute;
/// * the run id salts it, so two runs do not both open on `Thinking` — which
///   is what seeding on elapsed time alone gives you, every time.
///
/// The multiply is a cheap scramble: without it, consecutive windows walk the
/// verb list in order and a long run reads like a list being recited.
fn working_verb(locale: Locale, run_id: &str, elapsed: Duration) -> &'static str {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    run_id.hash(&mut h);
    let window = elapsed.as_millis() as u64 / VERB_REROLL_MS;
    let seed = h
        .finish()
        .wrapping_add(window.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    verb(locale, seed)
}

/// One knob's value, or `None` when the session follows the global default.
///
/// The exhaustive `match` is the point: it is what turns "a knob was added to
/// the command parser but never shown" into a compile error.
const fn knob_value<'a>(knobs: &SessionKnobs<'a>, knob: SessionKnob) -> Option<&'a str> {
    match knob {
        SessionKnob::ExecTier => knobs.exec_tier,
        SessionKnob::Mode => knobs.mode,
        SessionKnob::Think => knobs.think_level,
        SessionKnob::Memory => knobs.memory_mode,
    }
}

/// Format a token count as a human-readable string.
/// 0-999 -> "N tok", 1000-999999 -> "N.Nk tok", 1000000+ -> "N.NM tok"
fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        let millions = tokens as f64 / 1_000_000.0;
        format!("{millions:.1}M tok")
    } else if tokens >= 1_000 {
        let thousands = tokens as f64 / 1_000.0;
        format!("{thousands:.1}k tok")
    } else {
        format!("{tokens} tok")
    }
}

/// Compact token count without the ` tok` suffix, for the context-gauge
/// numerator/denominator (e.g. `12.3k`, `200.0k`, `1.0M`, `847`).
///
/// Shared with the `/context` overlay, which paints the same quantity from the
/// same gauge one keystroke away: two spellings of a token count on one screen
/// is the same fact told twice (判据 §1).
pub(super) fn compact_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Format context-window occupancy as `23% (12.3k/200.0k)`.
///
/// The percentage leads because it is the number a reader acts on; the raw
/// pair stays because it is the one that says *how much room is left in
/// tokens*, which a percentage of an unstated window cannot.
fn format_context_gauge(used: u32, window: u32) -> String {
    let pct = (u64::from(used) * 100) / u64::from(window.max(1));
    format!(
        "{pct}% ({}/{})",
        compact_tokens(u64::from(used)),
        compact_tokens(u64::from(window))
    )
}

/// Tint the cache stat: normal at or above 50%, warning below — cold starts
/// are expected (first call is always a write), so no red/error tier.
fn cache_stat_color(pct: u64) -> Color {
    if pct >= 50 {
        theme().status_fg
    } else {
        theme().warning
    }
}

/// Tint the gauge by fill ratio: normal under 70%, amber 70–90%, red at or
/// above 90% so an imminent context overflow is legible before it truncates.
fn context_gauge_color(used: u32, window: u32) -> Color {
    let ratio = f64::from(used) / f64::from(window);
    if ratio >= 0.9 {
        theme().error
    } else if ratio >= 0.7 {
        theme().warning
    } else {
        theme().status_fg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_bar() -> StatusBar<'static> {
        StatusBar {
            model: "claude-opus-5",
            session: "agent:main:main:s3",
            tokens: 12_345,
            context_gauge: Some((46_000, 200_000)),
            cost: CostView::Exact(0.042),
            cache_stat: Some(87),
            cache_stat_agent: None,
            running_agents: 2,
            is_connected: true,
            tool_progress_mode: ToolProgressMode::default(),
            spinner_frame: 0,
            run_elapsed: Some(Duration::from_secs(12)),
            run_id: Some("run-1"),
            locale: Locale::En,
            knobs: SessionKnobs {
                mode: Some("code"),
                exec_tier: Some("ask"),
                think_level: None,
                memory_mode: None,
            },
        }
    }

    fn painted(bar: &StatusBar<'_>, width: u16) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut term = Terminal::new(TestBackend::new(width, 1)).expect("backend");
        term.draw(|f| bar.render(f, f.area())).expect("draw");
        let buf = term.backend().buffer().clone();
        (0..width)
            .map(|x| buf.cell((x, 0)).map_or(" ", |c| c.symbol()).to_string())
            .collect()
    }

    /// The wire, end to end: what `fit` keeps is what reaches the cells.
    /// Without this the priority logic could be perfect and unpainted
    /// (判据 §4 — assert the effect arrived, not that the call happened).
    ///
    /// # Why the narrow half asserts on the working indicator
    ///
    /// "The session key is absent at 40 columns" is NOT evidence of dropping:
    /// a 40-column buffer cannot hold it either way, so that assertion stays
    /// green with `fit` disabled entirely (判据 §10 — it was written that way
    /// first, and the mutation run is what said so). What only dropping can
    /// produce is the segment *to the right* of the shed ones becoming
    /// visible: the working indicator is built last, so at 40 columns it is on
    /// screen if and only if the lower-priority segments made way for it.
    #[test]
    fn what_survives_the_fit_is_what_reaches_the_screen() {
        let wide = painted(&full_bar(), 200);
        assert!(wide.contains("claude-opus-5"), "{wide}");
        assert!(wide.contains("agent:main:main:s3"), "{wide}");
        assert!(wide.contains("$0.042"), "{wide}");
        assert!(wide.contains("ctx 23%"), "{wide}");

        let narrow = painted(&full_bar(), 40);
        assert!(narrow.contains("claude-opus-5"), "{narrow}");
        let word = working_verb(Locale::En, "run-1", Duration::from_secs(12));
        assert!(
            narrow.contains(word) && narrow.contains("12s"),
            "the working indicator must survive to 40 columns, got: {narrow}"
        );
        assert!(
            !narrow.contains("agent:main:main:s3"),
            "the session key must be the first thing to go: {narrow}"
        );
    }

    /// 判据 §8, on the surface a reader actually sees: an unknown cost paints
    /// `$?`. `$0.000` would read as "this conversation was free", which is
    /// the one wrong answer that looks like a fact.
    #[test]
    fn an_unknown_cost_paints_a_question_mark_not_a_zero() {
        let mut bar = full_bar();
        bar.cost = CostView::Unknown;
        let line = painted(&bar, 200);
        assert!(line.contains("$?"), "{line}");
        assert!(!line.contains("$0.000"), "{line}");

        bar.cost = CostView::AtLeast(0.1);
        let line = painted(&bar, 200);
        assert!(line.contains("$0.100+?"), "{line}");
    }

    #[test]
    fn format_tokens_small() {
        assert_eq!(format_tokens(0), "0 tok");
        assert_eq!(format_tokens(42), "42 tok");
        assert_eq!(format_tokens(999), "999 tok");
    }

    #[test]
    fn format_tokens_thousands() {
        assert_eq!(format_tokens(1000), "1.0k tok");
        assert_eq!(format_tokens(1234), "1.2k tok");
        assert_eq!(format_tokens(3200), "3.2k tok");
        assert_eq!(format_tokens(999_999), "1000.0k tok");
    }

    #[test]
    fn format_tokens_millions() {
        assert_eq!(format_tokens(1_000_000), "1.0M tok");
        assert_eq!(format_tokens(1_234_567), "1.2M tok");
        assert_eq!(format_tokens(42_500_000), "42.5M tok");
    }

    #[test]
    fn context_gauge_leads_with_the_percentage() {
        assert_eq!(format_context_gauge(12_345, 200_000), "6% (12.3k/200.0k)");
        assert_eq!(format_context_gauge(847, 8_000), "10% (847/8.0k)");
        assert_eq!(
            format_context_gauge(1_500_000, 1_000_000),
            "150% (1.5M/1.0M)"
        );
    }

    #[test]
    fn context_gauge_color_bands() {
        // < 70% normal, 70–90% warning, >= 90% error.
        assert_eq!(context_gauge_color(10, 100), theme().status_fg);
        assert_eq!(context_gauge_color(75, 100), theme().warning);
        assert_eq!(context_gauge_color(95, 100), theme().error);
    }

    #[test]
    fn cache_stat_color_bands() {
        // >= 50% normal, below warning — no error tier (cold starts are
        // expected, not alarming).
        assert_eq!(cache_stat_color(87), theme().status_fg);
        assert_eq!(cache_stat_color(50), theme().status_fg);
        assert_eq!(cache_stat_color(12), theme().warning);
    }

    /// **The B6 guard.** A format string gets this wrong silently: it keeps
    /// concatenating and lets the terminal clip, which throws away the
    /// RIGHTMOST segment — the live working indicator — and keeps the
    /// session key nobody was reading.
    ///
    /// Two properties, at every width from "everything fits" down to "only
    /// one survivor":
    ///
    /// 1. the line fits (or a single over-wide segment is all that is left);
    /// 2. nothing was dropped while something less important was kept.
    ///
    /// # When this goes red
    ///
    /// Deleting the drop loop in `fit`; reversing `max`/`min` on the
    /// priority; giving two kinds the same number so the order stops being
    /// total. All three were run.
    #[test]
    fn a_narrow_line_sheds_segments_in_the_declared_order() {
        // Counted per priority rather than per kind: `Knob` is emitted once
        // per set knob, so "is this kind still present" cannot tell a
        // partially-shed level from an untouched one.
        let census = |segs: &[Segment]| {
            let mut by_priority = [0usize; 16];
            for s in segs {
                by_priority[s.kind.priority() as usize] += 1;
            }
            by_priority
        };

        for width in 8..=120 {
            let all = census(&full_bar().segments());
            let kept = fit(full_bar().segments(), width);
            let kept_census = census(&kept);

            assert!(!kept.is_empty(), "width {width}: nothing left to read");
            assert!(
                total_width(&kept) <= width || kept.len() == 1,
                "width {width}: {} columns of content, with {} segments left",
                total_width(&kept),
                kept.len()
            );

            // The dropped set must be an upper tail of the priority order:
            // once a level has lost anything, no less important level may
            // still have survivors.
            for p in 0..all.len() {
                if kept_census[p] == all[p] {
                    continue;
                }
                for (q, kept_at_q) in kept_census.iter().enumerate().skip(p + 1) {
                    assert_eq!(
                        *kept_at_q, 0,
                        "width {width}: priority {p} was shed while priority {q} survived"
                    );
                }
            }
        }
    }

    /// The concrete consequence of the ordering, stated once so a reader can
    /// see what it buys: at 40 columns you still know what it is doing and
    /// what is serving it, and you have lost the session key.
    #[test]
    fn at_forty_columns_the_working_indicator_outlives_the_session_key() {
        let kinds: Vec<SegKind> = fit(full_bar().segments(), 40)
            .iter()
            .map(|s| s.kind)
            .collect();
        assert!(kinds.contains(&SegKind::Activity), "{kinds:?}");
        assert!(kinds.contains(&SegKind::Status), "{kinds:?}");
        assert!(!kinds.contains(&SegKind::Session), "{kinds:?}");
    }

    /// Every kind has its own rung. A tie would make `fit`'s choice depend on
    /// build order rather than on a decision anyone wrote down.
    #[test]
    fn the_priority_order_is_total() {
        let kinds = [
            SegKind::Status,
            SegKind::Ctx,
            SegKind::Cost,
            SegKind::Knob,
            SegKind::Agents,
            SegKind::Cache,
            SegKind::ToolMode,
            SegKind::Tokens,
            SegKind::Session,
            SegKind::Activity,
        ];
        let mut seen: Vec<u8> = kinds.iter().map(|k| k.priority()).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), kinds.len(), "two kinds share a priority");
    }

    /// An idle session shows no working indicator at all — the help hint it
    /// used to carry moved to the hint line, and a segment that said
    /// "0s" would be describing a run that is not happening.
    #[test]
    fn an_idle_bar_has_no_activity_segment() {
        let mut bar = full_bar();
        bar.run_elapsed = None;
        bar.run_id = None;
        assert!(!bar.segments().iter().any(|s| s.kind == SegKind::Activity));
    }

    /// The verb re-rolls on the 7-second boundary and not before, and two
    /// runs do not open on the same word.
    #[test]
    fn the_verb_rerolls_every_seven_seconds_and_is_salted_by_the_run() {
        let at = |ms: u64| working_verb(Locale::En, "run-1", Duration::from_millis(ms));
        assert_eq!(at(0), at(VERB_REROLL_MS - 1));
        assert_ne!(at(0), at(VERB_REROLL_MS));
        assert_eq!(at(VERB_REROLL_MS), at(VERB_REROLL_MS + 100));

        // Different runs, same instant: the salt is what separates them.
        // (Not every pair of ids can differ — there are 30 verbs — so this
        // asserts over a handful, which is what "salted" has to mean.)
        let zero = Duration::ZERO;
        let words: std::collections::HashSet<&str> = (0..8)
            .map(|i| working_verb(Locale::En, &format!("run-{i}"), zero))
            .collect();
        assert!(
            words.len() > 1,
            "every run opened on the same word: {words:?}"
        );
    }

    #[test]
    fn the_verb_follows_the_locale() {
        let en = working_verb(Locale::En, "r", Duration::ZERO);
        let zh = working_verb(Locale::Zh, "r", Duration::ZERO);
        assert!(en.is_ascii(), "{en}");
        assert!(!zh.is_ascii(), "{zh}");
    }
}
