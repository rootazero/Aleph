//! One Think→Act iteration as data. The unit the transcript folds by
//! (spec §4): thinking + interstitial text + the tool rows it issued + the
//! loop's own notes about that iteration. Both surfaces paint a settled step
//! as one line (`step_headline`) and expand it on demand; the reducer
//! (`reducer.rs`) is the only writer.

use unicode_width::UnicodeWidthStr;

use super::turn_summary::summarize_rows;
use super::view_model::{ToolGroup, ToolRow, TurnSummaryEntry};

/// Columns a headline may occupy before it is clipped with `…`.
pub const HEADLINE_MAX_COLS: u16 = 80;

/// The provider's thinking for one iteration. `streaming` is true while
/// `Reasoning` deltas are still arriving; the authoritative
/// `ReasoningEmitted` record replaces the text and clears it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingBlock {
    pub text: String,
    pub streaming: bool,
}

/// Step-scoped narration from the loop itself — never the model's thinking,
/// never shown unless the step is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    ToolSummary,
    VerifierVeto,
    ReactiveCompaction,
    MoaAdvisor,
    MoaAggregating,
    MoaAdvisorSpend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub kind: NoteKind,
    pub text: String,
}

/// `Live` while it is the run's current iteration; `Settled` once the next
/// `TurnStarted` or the run's end closed it; `Pending` when restored from a
/// replay that never saw it end — "unknown", never a fabricated success and
/// never a spinner (the `settle_resumed` rule, one level up).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Live,
    Settled,
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepEntry {
    pub id: String,
    /// `None` = the server never said which iteration this is (a frame that
    /// arrived before any `TurnStarted`, or a path that emits no trace). Such
    /// a step is never renumbered by a later `TurnStarted` — that opens a new
    /// step (判据 §8).
    pub iteration: Option<u32>,
    pub thinking: Option<ThinkingBlock>,
    pub text: Option<String>,
    /// Raw rows in issue order. Grouping (`Explored N calls`) is a paint-time
    /// projection — see `group_tool_rows` — so a row is always reachable by
    /// id here.
    pub tools: Vec<ToolRow>,
    pub notes: Vec<Note>,
    pub status: StepStatus,
    pub started_ms: Option<u64>,
    pub ended_ms: Option<u64>,
}

impl StepEntry {
    /// Nothing to show and nothing to count: such a step is removed at run
    /// end rather than rendered as "Step N: (nothing)" (spec §9).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.thinking
            .as_ref()
            .is_none_or(|t| t.text.trim().is_empty())
            && self.text.as_deref().is_none_or(|t| t.trim().is_empty())
            && self.tools.is_empty()
            && self.notes.is_empty()
    }

    pub fn find_tool_mut(&mut self, tool_id: &str) -> Option<&mut ToolRow> {
        self.tools.iter_mut().find(|r| r.id == tool_id)
    }
}

/// What a step's tool list becomes at paint time: consecutive read-only rows
/// fold into one `Explored N calls` group (same rule as `group_entries`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepTool {
    Row(ToolRow),
    Group(ToolGroup),
}

/// Where a headline's text came from — the surface may style them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlineSource {
    Thinking,
    Text,
    Tally,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Headline {
    pub text: String,
    pub source: HeadlineSource,
}

/// The first sentence of `s`, clipped to `max_cols` columns.
///
/// A sentence ends at the fullwidth `。！？` (always a boundary), at a `.`,
/// `!` or `?` that is followed by whitespace or the end (so `src/parse.rs`
/// and `a != b` are not sentence ends — the ASCII trio shares one rule),
/// or at a newline or a lone `\r` (old Mac line endings). "Followed by
/// whitespace" means `char::is_whitespace` — not just `' '`/`'\t'`/`'\n'`
/// — so a fullwidth space (U+3000) or an NBSP after the terminator also
/// counts. `None` for blank input — a blank headline would read as
/// "nothing was thought", which is not known.
#[must_use]
pub fn first_sentence(s: &str, max_cols: u16) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut end = s.len();
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '\n' | '\r' => {
                end = i;
                break;
            }
            '。' | '！' | '？' => {
                end = i + c.len_utf8();
                break;
            }
            '.' | '!' | '?' => {
                let next_is_break = match chars.peek() {
                    None => true,
                    Some((_, next)) => next.is_whitespace(),
                };
                if next_is_break {
                    end = i + c.len_utf8();
                    break;
                }
            }
            _ => {}
        }
    }
    let sentence = s.get(..end).unwrap_or(s).trim_end();
    if sentence.is_empty() {
        return None;
    }
    Some(clip_cols(sentence, max_cols))
}

/// Clip to `max_cols` display columns (clamped to at least 1), ending in
/// `…` (one column) when anything was removed. Never splits a character.
///
/// Width is measured with [`UnicodeWidthStr::width`] on each candidate
/// PREFIX as a whole, not as a sum of each character's own width:
/// unicode-width reports some multi-codepoint sequences (an emoji base
/// character plus its U+FE0F variation selector, for example) as wider
/// than the sum of their parts, matching how a string-width renderer such
/// as ratatui lays them out. A per-character sum would under-count such a
/// sequence and fail to clip a headline the renderer lays out over budget.
fn clip_cols(s: &str, max_cols: u16) -> String {
    let max = usize::from(max_cols.max(1));
    if s.width() <= max {
        return s.to_string();
    }
    let budget = max.saturating_sub(1);
    let mut cut = 0usize;
    for (i, c) in s.char_indices() {
        let candidate_end = i + c.len_utf8();
        let candidate = s.get(..candidate_end).unwrap_or("");
        if candidate.width() > budget {
            break;
        }
        cut = candidate_end;
    }
    format!("{}…", s.get(..cut).unwrap_or(""))
}

/// `Read 2 files, ran 1 command · 8s` for ONE step — no minimum-tool gate,
/// unlike the run-level `summarize_turn`. `None` while no row is terminal
/// (unknown, not "nothing ran").
#[must_use]
pub fn step_tally(step: &StepEntry) -> Option<TurnSummaryEntry> {
    summarize_rows(&step.tools)
}

/// Ruling R1: thinking's first sentence → text's first sentence → the tool
/// tally. `None` when the step has no thinking/text sentence and no
/// terminal tool row to tally — not only for an empty step: a step with
/// only notes, or only running/pending tools, also returns `None` here.
/// There is no fourth fallback in Phase S; how such a step is rendered is
/// a Phase T decision (recorded in the spec).
#[must_use]
pub fn step_headline(step: &StepEntry, max_cols: u16) -> Option<Headline> {
    if let Some(text) = step
        .thinking
        .as_ref()
        .and_then(|t| first_sentence(&t.text, max_cols))
    {
        return Some(Headline {
            text,
            source: HeadlineSource::Thinking,
        });
    }
    if let Some(text) = step
        .text
        .as_deref()
        .and_then(|t| first_sentence(t, max_cols))
    {
        return Some(Headline {
            text,
            source: HeadlineSource::Text,
        });
    }
    step_tally(step).map(|t| Headline {
        text: clip_cols(&super::turn_summary::turn_summary_text(&t), max_cols),
        source: HeadlineSource::Tally,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{RowStatus, ToolRow};
    use aleph_protocol::ToolResult;
    use proptest::prelude::*;
    use serde_json::json;
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

    fn finished(id: &str, tool: &str, args: serde_json::Value, ok: bool, ms: u64) -> ToolRow {
        let mut r = ToolRow::new(id, tool, &args);
        r.start(1_000);
        let res = if ok {
            ToolResult::success("out")
        } else {
            ToolResult::error("boom")
        };
        r.finish(&res, ms, 1_000 + ms);
        r
    }

    fn step(thinking: Option<&str>, text: Option<&str>, tools: Vec<ToolRow>) -> StepEntry {
        StepEntry {
            id: "step-1".into(),
            iteration: Some(1),
            thinking: thinking.map(|t| ThinkingBlock {
                text: t.into(),
                streaming: false,
            }),
            text: text.map(str::to_string),
            tools,
            notes: Vec::new(),
            status: StepStatus::Settled,
            started_ms: None,
            ended_ms: None,
        }
    }

    #[test]
    fn first_sentence_stops_at_cjk_and_ascii_terminators_and_newlines() {
        assert_eq!(
            first_sentence("先看 CI 日志定位失败的测试。然后修。", 80).as_deref(),
            Some("先看 CI 日志定位失败的测试。")
        );
        assert_eq!(
            first_sentence("Reading src/parse.rs to find the bug. Then fixing it.", 80).as_deref(),
            Some("Reading src/parse.rs to find the bug."),
            "a dot inside a path is not a sentence end"
        );
        assert_eq!(
            first_sentence("first line\nsecond line", 80).as_deref(),
            Some("first line")
        );
        assert_eq!(first_sentence("   \n  ", 80), None);
    }

    #[test]
    fn first_sentence_treats_ascii_bang_and_question_like_the_dot_rule() {
        // `!`/`?` are boundaries only when followed by whitespace or the
        // end, the same rule `.` already gets — so `!=` and a `?` inside a
        // URL query string are not sentence ends.
        assert_eq!(
            first_sentence("Check a != b first. Then go.", 80).as_deref(),
            Some("Check a != b first."),
            "the `!` in `!=` is not followed by whitespace"
        );
        assert_eq!(
            first_sentence("Fetch https://x.io/a?b=1 now. Done.", 80).as_deref(),
            Some("Fetch https://x.io/a?b=1 now."),
            "the `?` inside the URL query string is not followed by whitespace"
        );
        assert_eq!(
            first_sentence("Really? Yes.", 80).as_deref(),
            Some("Really?"),
            "a `?` followed by whitespace is still a boundary"
        );
    }

    #[test]
    fn first_sentence_treats_full_width_space_as_whitespace_after_a_terminator() {
        // `char::is_whitespace` covers U+3000 (IDEOGRAPHIC SPACE), not just
        // `' '`/`'\t'`/`'\n'` — so a `.` before it is still a boundary.
        assert_eq!(
            first_sentence("完成.\u{3000}下一步", 80).as_deref(),
            Some("完成.")
        );
    }

    #[test]
    fn first_sentence_treats_a_lone_carriage_return_as_a_line_break() {
        // Old Mac line endings use a lone `\r` with no `\n`; it must break
        // the sentence the same way `\n` does, not reach the headline.
        assert_eq!(
            first_sentence("first line\rsecond line", 80).as_deref(),
            Some("first line")
        );
    }

    #[test]
    fn first_sentence_clips_to_the_column_budget_with_an_ellipsis() {
        let h = first_sentence(&"很长的一句话".repeat(20), 12).unwrap();
        assert!(h.ends_with('…'), "{h}");
        let cols: usize = h.chars().map(|c| c.width().unwrap_or(0)).sum();
        assert!(cols <= 12, "{cols} columns: {h}");
    }

    #[test]
    fn clipping_measures_string_width_not_a_per_char_sum() {
        // U+2764 HEAVY BLACK HEART + U+FE0F VARIATION SELECTOR-16: as a
        // whole "emoji presentation sequence" unicode-width reports this
        // pair as 2 columns, while summing each char's own width gives 1
        // (1 + 0). A per-character sum would under-count a headline of
        // these and fail to clip it to a string-width renderer's layout
        // (ratatui measures spans by string width).
        let hearts = "\u{2764}\u{FE0F}".repeat(50); // 50 sequences, 100 columns by string width
        let clipped = first_sentence(&hearts, 12).unwrap();
        assert!(clipped.ends_with('…'), "{clipped}");
        let cols = clipped.width();
        assert!(cols <= 12, "{cols} columns: {clipped}");
    }

    proptest! {
        /// Any input: no panic, never over budget, never a half character,
        /// `None` exactly when the input is blank, and the returned text
        /// (with any clipping `…` stripped) is always a genuine prefix of
        /// the trimmed input — never a fabricated string.
        #[test]
        fn first_sentence_never_panics_and_respects_the_width(
            s in "[\\PC\\n\\t\\r ]*",
            cols in 1u16..120,
        ) {
            match first_sentence(&s, cols) {
                Some(h) => {
                    let w = h.width();
                    prop_assert!(w <= usize::from(cols), "{w} > {cols}: {h:?}");
                    prop_assert!(!h.trim().is_empty());
                    let prefix = h.strip_suffix('…').unwrap_or(&h);
                    prop_assert!(
                        s.trim().starts_with(prefix),
                        "{h:?} is not a prefix of {:?}", s.trim()
                    );
                }
                None => prop_assert!(s.trim().is_empty()),
            }
        }
    }

    #[test]
    fn headline_prefers_thinking_then_text_then_tally() {
        let tools = vec![finished(
            "t1",
            "file_read",
            json!({"path": "a.rs"}),
            true,
            5,
        )];
        let with_thinking = step(
            Some("Checking the tz code. More."),
            Some("Let me look."),
            tools.clone(),
        );
        let h = step_headline(&with_thinking, 80).unwrap();
        assert_eq!(h.source, HeadlineSource::Thinking);
        assert_eq!(h.text, "Checking the tz code.");

        let text_only = step(None, Some("Let me look at it. Now."), tools.clone());
        let h = step_headline(&text_only, 80).unwrap();
        assert_eq!(h.source, HeadlineSource::Text);
        assert_eq!(h.text, "Let me look at it.");

        let tally_only = step(None, None, tools);
        let h = step_headline(&tally_only, 80).unwrap();
        assert_eq!(h.source, HeadlineSource::Tally);
        // A 5 ms read: `Read 1 file · 0.0s` (`fmt_duration_ms(5)` is "0.0s").
        assert_eq!(h.text, "Read 1 file · 0.0s");
    }

    #[test]
    fn a_blank_thinking_block_falls_through_to_text() {
        let s = step(Some("   \n"), Some("Actually here."), Vec::new());
        let h = step_headline(&s, 80).unwrap();
        assert_eq!(h.source, HeadlineSource::Text);
    }

    #[test]
    fn an_empty_step_has_no_headline_and_reports_empty() {
        let s = step(None, None, Vec::new());
        assert!(s.is_empty());
        assert_eq!(step_headline(&s, 80), None);
    }

    #[test]
    fn a_step_with_only_a_note_is_not_empty() {
        let mut s = step(None, None, Vec::new());
        s.notes.push(Note {
            kind: NoteKind::VerifierVeto,
            text: "checklist incomplete".into(),
        });
        assert!(!s.is_empty());
    }

    #[test]
    fn is_empty_is_false_when_only_tools_are_present() {
        let s = step(
            None,
            None,
            vec![finished("t1", "bash", json!({"command": "ls"}), true, 1)],
        );
        assert!(
            !s.is_empty(),
            "a step with a tool row has something to show"
        );
    }

    #[test]
    fn is_empty_is_false_when_only_text_is_present() {
        let s = step(None, Some("Just a note."), Vec::new());
        assert!(
            !s.is_empty(),
            "a step with interstitial text has something to show"
        );
    }

    #[test]
    fn is_empty_is_false_when_thinking_has_real_text() {
        let s = step(Some("Actually thinking here."), None, Vec::new());
        assert!(
            !s.is_empty(),
            "non-blank thinking is not the same as no thinking at all"
        );
    }

    #[test]
    fn step_tally_counts_one_tool_where_the_turn_summary_would_not() {
        // The clocks deliberately disagree with the status here (started at
        // 1_000, "finished" at a clock 9_000 ms later than a consistent
        // fixture would use, while the wire's duration_ms stays 40): if
        // summarize_rows ever reverted to ended_ms - started_ms, this
        // assertion would see 9_040, not 40, and go red.
        let mut r = ToolRow::new("t1", "bash", &json!({"command": "ls"}));
        r.start(1_000);
        r.finish(&ToolResult::success("out"), 40, 1_000 + 40 + 9_000);
        let s = step(None, None, vec![r]);
        let t = step_tally(&s).expect("one tool is enough for a step tally");
        assert_eq!(t.commands, 1);
        assert_eq!(
            t.duration_ms, 40,
            "duration comes from RowStatus, not the clocks (which disagree with it here on purpose)"
        );
    }

    #[test]
    fn step_tally_is_unknown_while_no_row_is_terminal() {
        let mut r = ToolRow::new("t1", "bash", &json!({"command": "ls"}));
        r.start(5);
        let s = step(None, None, vec![r]);
        assert_eq!(step_tally(&s), None);
    }

    #[test]
    fn find_tool_mut_reaches_a_row_by_id() {
        let mut s = step(
            None,
            None,
            vec![finished("t9", "bash", json!({}), false, 1)],
        );
        assert!(matches!(
            s.find_tool_mut("t9").map(|r| &r.status),
            Some(RowStatus::Err { .. })
        ));
        assert!(s.find_tool_mut("nope").is_none());
    }
}
