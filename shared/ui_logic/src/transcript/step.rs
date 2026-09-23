//! One Think→Act iteration as data. The unit the transcript folds by
//! (spec §4): thinking + interstitial text + the tool rows it issued + the
//! loop's own notes about that iteration. Both surfaces paint a settled step
//! as one line (`step_headline`) and expand it on demand; the reducer
//! (`reducer.rs`) is the only writer.

use unicode_width::UnicodeWidthChar;

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
/// A sentence ends at `。！？!?`, at a newline, or at a `.` that is followed
/// by whitespace or the end (so `src/parse.rs` is not a sentence end).
/// `None` for blank input — a blank headline would read as "nothing was
/// thought", which is not known.
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
            '\n' => {
                end = i;
                break;
            }
            '。' | '！' | '？' | '!' | '?' => {
                end = i + c.len_utf8();
                break;
            }
            '.' => {
                let next_is_break = matches!(chars.peek(), None | Some((_, ' ' | '\t' | '\n')));
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

/// Clip to `max_cols` display columns, ending in `…` (one column) when
/// anything was removed. Never splits a character.
fn clip_cols(s: &str, max_cols: u16) -> String {
    let max = usize::from(max_cols.max(1));
    let total: usize = s.chars().map(|c| c.width().unwrap_or(0)).sum();
    if total <= max {
        return s.to_string();
    }
    let budget = max.saturating_sub(1);
    let mut width = 0usize;
    let mut cut = 0usize;
    for (i, c) in s.char_indices() {
        let w = c.width().unwrap_or(0);
        if width + w > budget {
            break;
        }
        width += w;
        cut = i + c.len_utf8();
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
/// tally. `None` only for an empty step.
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
    use unicode_width::UnicodeWidthChar;

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
    fn first_sentence_clips_to_the_column_budget_with_an_ellipsis() {
        let h = first_sentence(&"很长的一句话".repeat(20), 12).unwrap();
        assert!(h.ends_with('…'), "{h}");
        let cols: usize = h.chars().map(|c| c.width().unwrap_or(0)).sum();
        assert!(cols <= 12, "{cols} columns: {h}");
    }

    proptest! {
        /// Any input: no panic, never over budget, never a half character,
        /// `None` exactly when the input is blank.
        #[test]
        fn first_sentence_never_panics_and_respects_the_width(s in "\\PC*", cols in 1u16..120) {
            match first_sentence(&s, cols) {
                Some(h) => {
                    let w: usize = h.chars().map(|c| c.width().unwrap_or(0)).sum();
                    prop_assert!(w <= usize::from(cols), "{w} > {cols}: {h:?}");
                    prop_assert!(!h.trim().is_empty());
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
    fn step_tally_counts_one_tool_where_the_turn_summary_would_not() {
        let s = step(
            None,
            None,
            vec![finished("t1", "bash", json!({"command": "ls"}), true, 40)],
        );
        let t = step_tally(&s).expect("one tool is enough for a step tally");
        assert_eq!(t.commands, 1);
        assert_eq!(
            t.duration_ms, 40,
            "duration comes from RowStatus, not the clocks"
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
