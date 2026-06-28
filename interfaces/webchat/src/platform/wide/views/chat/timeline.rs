//! Timeline derivation — folds the flat chat message vector into a structured
//! render model with calendar-day separators and per-message clock labels.
//!
//! This is the *presentation-structure* layer for the message list: it keeps
//! [`super::messages`] free of date-bucketing logic and concentrates every
//! `js_sys::Date` touch in a few small bridge fns. The folding core
//! ([`build_rows`]) is pure — it receives the day-ordinal / label / clock
//! mappers as closures — so it is exercised by host unit tests without a WASM
//! runtime.
//!
//! Why a derived row model instead of a flat `<For>` over messages? Inserting
//! separators inline would scatter "is this a new day?" state across the
//! render closure. Modelling the list as an explicit `Vec<TimelineRow>` makes
//! the segmentation a single, memoizable transform: Leptos recomputes it only
//! when `messages` changes, not on every reactive read.

use super::state::ChatMessage;

/// A single render row in the message timeline.
#[derive(Debug, Clone, PartialEq)]
pub enum TimelineRow {
    /// Calendar-day separator anchoring the run of messages that follow it.
    /// `key` is a stable per-day identifier (the local day ordinal as a
    /// string) used for `<For>` keying; `label` is the rendered text
    /// ("Today" / "Yesterday" / "YYYY-MM-DD").
    DaySeparator { key: String, label: String },
    /// A message plus its resolved clock label ("HH:MM", or empty when the
    /// message carries no timestamp — e.g. legacy history rows).
    Message { message: ChatMessage, clock: String },
    /// A run's consecutive intermediate step bubbles, folded into one bounded
    /// scrolling strip (keeps the chat column short). `completed` is true when
    /// no step is still streaming → render auto-collapsed to a summary line.
    StepStrip {
        run_id: String,
        steps: Vec<ChatMessage>,
        completed: bool,
    },
}

/// Fold `messages` into timeline rows, inserting a [`TimelineRow::DaySeparator`]
/// before the first message of each new calendar day.
///
/// All three closures are injected so this fn stays pure / host-testable:
/// - `day_ordinal`: epoch-millis → local-day integer (days since some epoch).
/// - `label_for`: epoch-millis → separator label for that message's day.
/// - `clock_for`: epoch-millis → per-message time-of-day label.
///
/// Messages without a timestamp never emit a separator and carry an empty
/// clock, so a mixed history (legacy rows + freshly stamped rows) degrades
/// gracefully instead of forcing a bogus "unknown day" bucket.
pub fn build_rows(
    messages: &[ChatMessage],
    day_ordinal: impl Fn(i64) -> i64,
    label_for: impl Fn(i64) -> String,
    clock_for: impl Fn(i64) -> String,
) -> Vec<TimelineRow> {
    let mut rows = Vec::with_capacity(messages.len() + 2);
    let mut last_day: Option<i64> = None;
    let mut pending: Vec<ChatMessage> = Vec::new();

    for m in messages {
        // Fold every Think→Act *step* bubble of one run into a single StepStrip
        // so a long run doesn't stretch the column. A step is any iteration-
        // tagged assistant bubble that is NOT the run's final answer — this
        // includes the still-streaming current turn (which `begin_step` leaves
        // as the non-intermediate `assistant-{run}` placeholder), so it folds
        // into the strip instead of dangling below it as a bare bubble.
        if is_step(m) {
            if pending
                .first()
                .is_some_and(|p| run_id_of(p) != run_id_of(m))
            {
                flush_strip(&mut rows, &mut pending);
            }
            pending.push(m.clone());
            continue;
        }

        // A non-step row closes any open strip before it renders.
        flush_strip(&mut rows, &mut pending);

        let clock = match m.timestamp {
            Some(ts) => {
                let day = day_ordinal(ts);
                if last_day != Some(day) {
                    rows.push(TimelineRow::DaySeparator {
                        key: day.to_string(),
                        label: label_for(ts),
                    });
                    last_day = Some(day);
                }
                clock_for(ts)
            }
            None => String::new(),
        };
        rows.push(TimelineRow::Message {
            message: m.clone(),
            clock,
        });
    }
    flush_strip(&mut rows, &mut pending);
    rows
}

/// Run id behind a message id (`intermediate-{run}-{n}`, `assistant-{run}`, or
/// a bare user id).
fn run_id_of(m: &ChatMessage) -> String {
    crate::views::chat::messages::run_id_from_message_id(&m.id)
}

/// A trailing assistant bubble is the run's *final answer* — rendered as its
/// own message, not folded into the step strip — when it carries real text and
/// either issued no tool call (a pure reply that ends the run) or has been
/// flagged `is_final` by `run_complete`/`replay_run` from the harness's
/// authoritative `summary.final_response`. The `is_final` escape hatch covers a
/// run whose last turn emitted the answer *and* a tool call (e.g. a closing
/// `web_fetch`): the answer renders as a bubble with its tool card inline,
/// instead of staying trapped in the strip. A plain mid-run tool turn (no
/// `is_final`) still folds into the strip, so a 200-iteration tool run collapses
/// entirely while a normal one-shot reply renders as a plain bubble.
fn is_final_answer(m: &ChatMessage) -> bool {
    m.role == "assistant"
        && !m.is_intermediate
        && !m.content.trim().is_empty()
        && (m.tool_calls.is_empty() || m.is_final)
}

/// Whether this bubble folds into the step strip: any iteration-tagged
/// assistant turn that isn't the final answer (finalized intermediates, the
/// streaming current step, tool-only or empty placeholder turns).
fn is_step(m: &ChatMessage) -> bool {
    m.role == "assistant" && m.iteration.is_some() && !is_final_answer(m)
}

/// Flush accumulated intermediate steps into one `StepStrip` row. No-op when
/// empty. `completed` is true when no step is still streaming.
fn flush_strip(rows: &mut Vec<TimelineRow>, pending: &mut Vec<ChatMessage>) {
    if pending.is_empty() {
        return;
    }
    let run_id = run_id_of(&pending[0]);
    let completed = pending.iter().all(|m| !m.is_streaming);
    rows.push(TimelineRow::StepStrip {
        run_id,
        steps: std::mem::take(pending),
        completed,
    });
}

/// Stable `<For>` key for a timeline row.
///
/// Mirrors the composite key the flat list used (id + volatile fields) so a
/// streaming bubble still re-renders per token; separators key on their day.
#[must_use]
pub fn row_key(row: &TimelineRow) -> String {
    match row {
        TimelineRow::DaySeparator { key, .. } => format!("sep:{key}"),
        TimelineRow::Message { message: m, clock } => format!(
            "{}:{}:{}:{}:{}:{}:{}",
            m.id,
            m.content.len(),
            m.is_streaming,
            m.is_intermediate,
            m.tool_calls.len(),
            m.model_info.is_some(),
            clock,
        ),
        TimelineRow::StepStrip {
            run_id,
            steps,
            completed,
        } => {
            // Include aggregate volatile state (content length + tool counts) so
            // a late `text_emitted` updating a folded step changes the <For> key
            // and forces a fresh render — mirroring the Message arm which keys on
            // content.len(). Without this, a streaming/late step update would
            // reuse the DOM node and show a stale snapshot.
            let content_len: usize = steps.iter().map(|s| s.content.len()).sum();
            let tools: usize = steps.iter().map(|s| s.tool_calls.len()).sum();
            format!(
                "strip:{run_id}:{}:{completed}:{content_len}:{tools}",
                steps.len()
            )
        }
    }
}

/// Convenience wrapper used by the render path: derive rows straight from
/// `messages`, resolving day ordinals / labels / clocks against the local
/// timezone via `js_sys::Date`. `today_label` / `yesterday_label` come from
/// i18n so the relative day names follow the UI locale.
///
/// Kept arch-agnostic (the date helpers are dual-armed) so the panel crate
/// still compiles under a host `cargo check`; only the WASM build ever renders.
pub fn derive_timeline(
    messages: &[ChatMessage],
    today_label: &str,
    yesterday_label: &str,
) -> Vec<TimelineRow> {
    let today = local_day_ordinal(now_millis());
    build_rows(
        messages,
        local_day_ordinal,
        |ts| {
            let day = local_day_ordinal(ts);
            if day == today {
                today_label.to_string()
            } else if day == today - 1 {
                yesterday_label.to_string()
            } else {
                format_date(ts)
            }
        },
        format_clock,
    )
}

// ---- JS bridges (timezone-aware) ----------------------------------------
// Dual-armed so the module also compiles for host unit tests; the host arm is
// inert (the pure `build_rows` path is what tests exercise).

/// Current wall-clock time in epoch milliseconds.
#[cfg(target_arch = "wasm32")]
pub fn now_millis() -> i64 {
    js_sys::Date::now() as i64
}

/// Current wall-clock time in epoch milliseconds.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub const fn now_millis() -> i64 {
    0
}

/// Local-timezone day ordinal (days since the Unix epoch in the user's tz).
/// `local = utc - getTimezoneOffset()` (offset is minutes east-of-UTC negated,
/// per the JS convention), then floor-divide by one day.
#[cfg(target_arch = "wasm32")]
fn local_day_ordinal(ts_millis: i64) -> i64 {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts_millis as f64));
    let offset_ms = d.get_timezone_offset() * 60_000.0;
    (((ts_millis as f64) - offset_ms) / 86_400_000.0).floor() as i64
}
#[cfg(not(target_arch = "wasm32"))]
const fn local_day_ordinal(_ts_millis: i64) -> i64 {
    0
}

/// Local "HH:MM" for a message's timestamp.
#[cfg(target_arch = "wasm32")]
fn format_clock(ts_millis: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts_millis as f64));
    format!("{:02}:{:02}", d.get_hours(), d.get_minutes())
}
#[cfg(not(target_arch = "wasm32"))]
const fn format_clock(_ts_millis: i64) -> String {
    String::new()
}

/// Local "YYYY-MM-DD" for a separator more than a day in the past.
#[cfg(target_arch = "wasm32")]
fn format_date(ts_millis: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts_millis as f64));
    format!(
        "{:04}-{:02}-{:02}",
        d.get_full_year(),
        d.get_month() + 1,
        d.get_date()
    )
}
#[cfg(not(target_arch = "wasm32"))]
const fn format_date(_ts_millis: i64) -> String {
    String::new()
}

/// Parse a `chat.history` wire timestamp (RFC3339 / ISO-8601, or a bare
/// epoch-seconds/millis string) into epoch milliseconds. Returns `None` when
/// the string is empty or unparseable so legacy rows render clock-free instead
/// of anchoring a bogus 1970 separator.
#[cfg(target_arch = "wasm32")]
pub fn parse_wire_timestamp(raw: &str) -> Option<i64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    // Fast path: a bare integer is epoch seconds (10 digits) or millis (13).
    if let Ok(n) = s.parse::<i64>() {
        return Some(if s.len() <= 11 { n * 1000 } else { n });
    }
    // Otherwise let the JS engine parse ISO/RFC3339; NaN ⇒ unparseable.
    let ms = js_sys::Date::parse(s);
    if ms.is_nan() {
        None
    } else {
        Some(ms as i64)
    }
}

/// Host stub — the hydration path that calls this is WASM-only.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub const fn parse_wire_timestamp(_raw: &str) -> Option<i64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, ts: Option<i64>) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            role: "user".into(),
            content: "hi".into(),
            tool_calls: vec![],
            is_streaming: false,
            is_intermediate: false,
            error: None,
            model_info: None,
            is_final: false,
            text_finalized: false,
            timestamp: ts,
            iteration: None,
            agent_id: None,
            plan_archive: None,
        }
    }

    fn msg_user(id: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            role: "user".into(),
            content: content.into(),
            tool_calls: vec![],
            is_streaming: false,
            is_intermediate: false,
            error: None,
            model_info: None,
            is_final: false,
            text_finalized: false,
            iteration: None,
            timestamp: None,
            agent_id: None,
            plan_archive: None,
        }
    }

    fn msg_step(id: &str, it: usize, content: &str, streaming: bool) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            role: "assistant".into(),
            content: content.into(),
            tool_calls: vec![],
            is_streaming: streaming,
            is_intermediate: true,
            error: None,
            model_info: None,
            is_final: false,
            text_finalized: false,
            iteration: Some(it),
            timestamp: None,
            agent_id: None,
            plan_archive: None,
        }
    }

    fn msg_final(run: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: run.into(),
            role: "assistant".into(),
            content: content.into(),
            tool_calls: vec![],
            is_streaming: false,
            is_intermediate: false,
            error: None,
            model_info: None,
            is_final: false,
            text_finalized: false,
            iteration: None,
            timestamp: None,
            agent_id: None,
            plan_archive: None,
        }
    }

    #[test]
    fn consecutive_intermediates_fold_into_one_strip() {
        let msgs = vec![
            msg_user("u1", "hi"),
            msg_step("intermediate-run-a-1", 1, "s1", false),
            msg_step("intermediate-run-a-2", 2, "s2", false),
            msg_final("run-a", "answer"),
        ];
        let rows = derive_timeline(&msgs, "Today", "Yesterday");
        let strips: Vec<&TimelineRow> = rows
            .iter()
            .filter(|r| matches!(r, TimelineRow::StepStrip { .. }))
            .collect();
        assert_eq!(strips.len(), 1);
        if let TimelineRow::StepStrip {
            run_id,
            steps,
            completed,
        } = strips[0]
        {
            assert_eq!(run_id, "run-a");
            assert_eq!(steps.len(), 2);
            assert!(*completed, "no streaming step → completed");
        } else {
            panic!("expected StepStrip");
        }
        assert!(rows
            .iter()
            .any(|r| matches!(r, TimelineRow::Message { message, .. } if message.id == "run-a")));
    }

    #[test]
    fn streaming_step_marks_strip_incomplete() {
        let msgs = vec![
            msg_step("intermediate-run-b-1", 1, "s1", false),
            msg_step("intermediate-run-b-2", 2, "s2", true),
        ];
        let rows = derive_timeline(&msgs, "Today", "Yesterday");
        let strip = rows.iter().find_map(|r| match r {
            TimelineRow::StepStrip { completed, .. } => Some(*completed),
            _ => None,
        });
        assert_eq!(strip, Some(false));
    }

    /// A trailing `assistant-{run}` step bubble (NOT intermediate) carrying a
    /// tool call — the shape `begin_step` leaves the current turn in. Must fold
    /// into the strip, not dangle below it.
    fn msg_tool_step(run: &str, it: usize, content: &str, streaming: bool) -> ChatMessage {
        ChatMessage {
            id: format!("assistant-{run}"),
            role: "assistant".into(),
            content: content.into(),
            tool_calls: vec![crate::views::chat::state::ToolCallEntry {
                tool_id: "t1".into(),
                tool_name: "code_exec".into(),
                status: "completed".into(),
                duration_ms: Some(3),
            }],
            is_streaming: streaming,
            is_intermediate: false,
            error: None,
            model_info: None,
            is_final: false,
            text_finalized: false,
            iteration: Some(it),
            timestamp: None,
            agent_id: None,
            plan_archive: None,
        }
    }

    /// Empty streaming placeholder for the just-started current turn: tagged,
    /// non-intermediate, no content, no tools. The dangling "#N + cursor" the
    /// user saw — must fold into the strip (keeping it open) rather than render
    /// as a bare bubble.
    fn msg_empty_step(run: &str, it: usize) -> ChatMessage {
        ChatMessage {
            id: format!("assistant-{run}"),
            role: "assistant".into(),
            content: String::new(),
            tool_calls: vec![],
            is_streaming: true,
            is_intermediate: false,
            error: None,
            model_info: None,
            is_final: false,
            text_finalized: false,
            iteration: Some(it),
            timestamp: None,
            agent_id: None,
            plan_archive: None,
        }
    }

    #[test]
    fn trailing_tool_step_folds_into_strip_not_dangling() {
        // A tool-only run that ended without a text reply (the 200-iteration
        // convergence case): every turn, including the trailing one, is a step.
        let msgs = vec![
            msg_user("u1", "hi"),
            msg_step("intermediate-run-z-1", 1, "trying api", false),
            msg_tool_step("run-z", 2, "让我尝试使用东方财富的API获取数据", false),
        ];
        let rows = derive_timeline(&msgs, "Today", "Yesterday");
        // No standalone assistant Message row (the trailing step folded in).
        assert!(
            !rows.iter().any(|r| matches!(
                r,
                TimelineRow::Message { message, .. } if message.role == "assistant"
            )),
            "trailing tool step must not render as a dangling bubble"
        );
        let strip = rows
            .iter()
            .find_map(|r| match r {
                TimelineRow::StepStrip {
                    steps, completed, ..
                } => Some((steps.len(), *completed)),
                _ => None,
            })
            .expect("a strip");
        assert_eq!(strip.0, 2, "both steps fold into one strip");
        assert!(strip.1, "all steps done → strip collapses");
    }

    #[test]
    fn empty_placeholder_step_folds_and_keeps_strip_open() {
        let msgs = vec![
            msg_step("intermediate-run-y-1", 1, "step one", false),
            msg_empty_step("run-y", 2),
        ];
        let rows = derive_timeline(&msgs, "Today", "Yesterday");
        assert!(
            !rows.iter().any(
                |r| matches!(r, TimelineRow::Message { message, .. } if message.role == "assistant")
            ),
            "empty placeholder must fold, not dangle"
        );
        let completed = rows.iter().find_map(|r| match r {
            TimelineRow::StepStrip { completed, .. } => Some(*completed),
            _ => None,
        });
        assert_eq!(
            completed,
            Some(false),
            "streaming placeholder keeps strip open"
        );
    }

    #[test]
    fn pure_text_final_answer_stays_standalone() {
        // Normal multi-step run that DID end with a text reply: steps fold, the
        // reply renders as its own bubble.
        let msgs = vec![
            msg_step("intermediate-run-w-1", 1, "searching", false),
            msg_final("run-w", "Here is your answer."),
        ];
        let rows = derive_timeline(&msgs, "Today", "Yesterday");
        assert!(
            rows.iter().any(|r| matches!(
                r,
                TimelineRow::Message { message, .. } if message.id == "run-w"
            )),
            "final text answer renders as a standalone bubble"
        );
        assert!(
            rows.iter()
                .any(|r| matches!(r, TimelineRow::StepStrip { steps, .. } if steps.len() == 1)),
            "the one tool step still folds into a strip"
        );
    }

    #[test]
    fn final_answer_with_tool_call_escapes_the_strip() {
        // The real bug: the terminating turn emitted the answer text AND a
        // closing tool call (e.g. a `web_fetch`), then the run ended. Without
        // the `is_final` flag this trailing bubble — having a non-empty
        // `tool_calls` — was folded into the step strip instead of rendering as
        // the conversational reply. `run_complete`/`replay_run` flag it
        // `is_final`, so it must render as a standalone Message (tool card
        // inline) while the earlier steps still fold.
        let mut answer = msg_tool_step("run-r", 2, "根据我的搜索，我为您整理了以下报告……", false);
        answer.is_final = true;
        let msgs = vec![
            msg_user("u1", "今天美股发生了什么"),
            msg_step("intermediate-run-r-1", 1, "searching", false),
            answer,
        ];
        let rows = derive_timeline(&msgs, "Today", "Yesterday");
        assert!(
            rows.iter().any(|r| matches!(
                r,
                TimelineRow::Message { message, .. }
                    if message.id == "assistant-run-r" && !message.tool_calls.is_empty()
            )),
            "is_final answer renders as a standalone bubble even with a tool call"
        );
        assert!(
            rows.iter()
                .any(|r| matches!(r, TimelineRow::StepStrip { steps, .. } if steps.len() == 1)),
            "the earlier tool step still folds into a strip"
        );
    }

    #[test]
    fn single_turn_reply_has_no_strip() {
        // A one-shot answer (no tools) is the final answer — plain bubble, no strip.
        let mut answer = msg_final("run-s", "hello there");
        answer.iteration = Some(1); // begin_step stamps even single turns
        let rows = derive_timeline(&[msg_user("u1", "hi"), answer], "Today", "Yesterday");
        assert!(
            !rows
                .iter()
                .any(|r| matches!(r, TimelineRow::StepStrip { .. })),
            "a tool-less reply must not be folded into a strip"
        );
    }

    #[test]
    fn row_key_strip_changes_on_content_update() {
        let s1 = TimelineRow::StepStrip {
            run_id: "r1".into(),
            steps: vec![msg_step("intermediate-r1-1", 1, "partial", true)],
            completed: false,
        };
        let s2 = TimelineRow::StepStrip {
            run_id: "r1".into(),
            steps: vec![msg_step("intermediate-r1-1", 1, "partial content", true)],
            completed: false,
        };
        assert_ne!(
            row_key(&s1),
            row_key(&s2),
            "key must change when content grows"
        );
    }

    // Fake mappers: treat each whole "1000ms" bucket as a day.
    fn day(ts: i64) -> i64 {
        ts / 1000
    }
    fn label(ts: i64) -> String {
        format!("D{}", ts / 1000)
    }
    fn clock(ts: i64) -> String {
        format!("T{ts}")
    }

    #[test]
    fn empty_input_yields_no_rows() {
        let rows = build_rows(&[], day, label, clock);
        assert!(rows.is_empty());
    }

    #[test]
    fn first_dated_message_gets_a_separator() {
        let rows = build_rows(&[msg("a", Some(1500))], day, label, clock);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            TimelineRow::DaySeparator {
                key: "1".into(),
                label: "D1".into()
            }
        );
        match &rows[1] {
            TimelineRow::Message { message, clock } => {
                assert_eq!(message.id, "a");
                assert_eq!(clock, "T1500");
            }
            _ => panic!("expected message row"),
        }
    }

    #[test]
    fn consecutive_same_day_messages_share_one_separator() {
        let rows = build_rows(
            &[msg("a", Some(1100)), msg("b", Some(1900))],
            day,
            label,
            clock,
        );
        // sep, a, b — only one separator for the shared day "1".
        assert_eq!(rows.len(), 3);
        assert!(matches!(rows[0], TimelineRow::DaySeparator { .. }));
        assert!(matches!(rows[1], TimelineRow::Message { .. }));
        assert!(matches!(rows[2], TimelineRow::Message { .. }));
    }

    #[test]
    fn day_change_inserts_a_new_separator() {
        let rows = build_rows(
            &[msg("a", Some(1500)), msg("b", Some(2500))],
            day,
            label,
            clock,
        );
        // sep(D1), a, sep(D2), b
        assert_eq!(rows.len(), 4);
        assert_eq!(
            rows[2],
            TimelineRow::DaySeparator {
                key: "2".into(),
                label: "D2".into()
            }
        );
    }

    #[test]
    fn undated_messages_emit_no_separator_and_empty_clock() {
        let rows = build_rows(&[msg("a", None), msg("b", None)], day, label, clock);
        assert_eq!(rows.len(), 2);
        for r in &rows {
            match r {
                TimelineRow::Message { clock, .. } => assert!(clock.is_empty()),
                _ => panic!("undated input must not produce separators"),
            }
        }
    }

    #[test]
    fn mixed_dated_and_undated_degrades_gracefully() {
        // undated row carries no separator; the later dated row still anchors.
        let rows = build_rows(&[msg("a", None), msg("b", Some(1500))], day, label, clock);
        // a(msg), sep(D1), b(msg)
        assert_eq!(rows.len(), 3);
        assert!(matches!(rows[0], TimelineRow::Message { .. }));
        assert!(matches!(rows[1], TimelineRow::DaySeparator { .. }));
        assert!(matches!(rows[2], TimelineRow::Message { .. }));
    }

    #[test]
    fn row_key_distinguishes_separators_from_messages() {
        let sep = TimelineRow::DaySeparator {
            key: "5".into(),
            label: "D5".into(),
        };
        assert_eq!(row_key(&sep), "sep:5");
        let m = TimelineRow::Message {
            message: msg("x", Some(1000)),
            clock: "T1000".into(),
        };
        assert!(row_key(&m).starts_with("x:"));
    }
}
