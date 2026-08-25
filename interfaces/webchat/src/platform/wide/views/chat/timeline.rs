//! Timeline derivation — folds the flat chat message vector into a structured
//! render model with calendar-day separators, per-message clock labels, and
//! a shape-per-row breakdown of a run's intermediate turns.
//!
//! This is the *presentation-structure* layer for the message list: it keeps
//! [`super::messages`] free of date-bucketing and step-shape logic and
//! concentrates every `js_sys::Date` touch in a few small bridge fns. The
//! folding core ([`build_rows`]) is pure — it receives the day-ordinal /
//! label / clock mappers as closures — so it is exercised by host unit tests
//! without a WASM runtime.
//!
//! Why a derived row model instead of a flat `<For>` over messages? A run's
//! intermediate turns interleave narration text with tool calls; deriving
//! [`TimelineRow::Narration`] / [`TimelineRow::ToolLine`] /
//! [`TimelineRow::ExploreGroup`] rows up front lets the render layer draw
//! each shape with a dedicated component instead of branching on message
//! internals per item. Modelling the list as an explicit `Vec<TimelineRow>`
//! also makes day-separator insertion a single, memoizable transform: Leptos
//! recomputes it only when `messages` changes, not on every reactive read.

use super::state::{ChatMessage, ToolCallEntry};

/// A single render row in the message timeline.
#[derive(Debug, Clone, PartialEq)]
pub enum TimelineRow {
    /// Calendar-day separator anchoring the run of messages that follow it.
    /// `key` is a stable per-day identifier (the local day ordinal as a
    /// string) used for `<For>` keying; `label` is the rendered text
    /// ("Today" / "Yesterday" / "YYYY-MM-DD").
    DaySeparator { key: String, label: String },
    /// A message plus its resolved clock label and the small structural
    /// facts a row needs before it can render — NOT the growing `content`.
    /// `<For>`'s children closure runs once per stable key (see `row_key`);
    /// content itself is fetched reactively inside the rendered component so
    /// it keeps updating without a remount. `has_plan_archive`/`role` gate
    /// which component this row renders as (`PlanArchiveCell` /
    /// `SystemNoticeRow` / `ToolFallbackRow` / `MessageBubble`) — a decision
    /// made once, at closure-run time, since a message's role/archive-ness
    /// doesn't change after creation.
    Message {
        id: String,
        role: String,
        has_plan_archive: bool,
        is_streaming: bool,
        is_intermediate: bool,
        tool_call_count: usize,
        has_model_info: bool,
        clock: String,
    },
    /// An intermediate turn's narration text: no bubble, no strip — rendered
    /// inline, no-frame. Also covers the streaming cursor placeholder (empty
    /// content, still streaming, no tool calls yet).
    ///
    /// Carries only what `row_key` and the rendered `NarrationRow` need
    /// reactively (just `id`); content is fetched the same way as `Message`
    /// rows.
    Narration { id: String, is_streaming: bool },
    /// A single non-readonly tool call from an intermediate turn, rendered as
    /// one line item.
    ToolLine { run_id: String, tool: ToolCallEntry },
    /// A run of consecutive read-only tool calls (file reads / searches),
    /// collapsed into one block (mirrors Codex's "Exploring" grouping).
    ExploreGroup {
        /// Stable key: `explore:{run_id}:{first_tool_id}`.
        key: String,
        run_id: String,
        tools: Vec<ToolCallEntry>,
        /// True when no tool in the group is still `running` and none of the
        /// source messages are still streaming.
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
    let mut acc: Option<ExploreAcc> = None;

    for m in messages {
        // A step is any iteration-tagged assistant bubble that is NOT the
        // run's final answer — this includes the still-streaming current turn
        // (`assistant-{run}` is stamped as step 1 in `start_assistant_message`).
        // Its narration text and tool calls are derived into separate rows
        // below instead of folding the whole bubble into one strip.
        if is_step(m) {
            let run = run_id_of(m);
            if acc.as_ref().is_some_and(|a| a.run_id != run) {
                flush_explore(&mut rows, &mut acc);
            }
            let has_narration =
                !m.content.trim().is_empty() || (m.is_streaming && m.tool_calls.is_empty());
            if has_narration {
                flush_explore(&mut rows, &mut acc);
                rows.push(TimelineRow::Narration {
                    id: m.id.clone(),
                    is_streaming: m.is_streaming,
                });
            }
            for t in &m.tool_calls {
                if is_explore_tool(&t.tool_name) {
                    let a = acc.get_or_insert_with(|| ExploreAcc {
                        key: format!("explore:{run}:{}", t.tool_id),
                        run_id: run.clone(),
                        tools: Vec::with_capacity(m.tool_calls.len()),
                        streaming: false,
                    });
                    a.tools.push(t.clone());
                    a.streaming |= m.is_streaming;
                } else {
                    flush_explore(&mut rows, &mut acc);
                    rows.push(TimelineRow::ToolLine {
                        run_id: run.clone(),
                        tool: t.clone(),
                    });
                }
            }
            // Tool-carrying streaming step: keep the group "open" even if
            // narration was consumed above — streaming flag already folded
            // per-push.
            if m.is_streaming && !m.tool_calls.is_empty() {
                if let Some(a) = acc.as_mut() {
                    a.streaming = true;
                }
            }
            continue;
        }

        // A non-step row closes any open explore group before it renders.
        flush_explore(&mut rows, &mut acc);

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
            id: m.id.clone(),
            role: m.role.clone(),
            has_plan_archive: m.plan_archive.is_some(),
            is_streaming: m.is_streaming,
            is_intermediate: m.is_intermediate,
            tool_call_count: m.tool_calls.len(),
            has_model_info: m.model_info.is_some(),
            clock,
        });
    }
    flush_explore(&mut rows, &mut acc);
    rows
}

/// Only-read exploration tools (file reads / searches) → collapsed into an
/// [`TimelineRow::ExploreGroup`] (Codex "Exploring" grouping analog).
#[must_use]
pub fn is_explore_tool(tool_name: &str) -> bool {
    use crate::components::tool_card::ToolKind;
    matches!(
        ToolKind::from_name(tool_name),
        ToolKind::FileRead | ToolKind::Search
    )
}

/// Open explore-group accumulator (flushed on narration / action tool /
/// non-step row / end of input).
struct ExploreAcc {
    key: String,
    run_id: String,
    tools: Vec<ToolCallEntry>,
    streaming: bool,
}

/// Flush the open explore-group accumulator into one `ExploreGroup` row.
/// No-op when no group is open. `completed` is true when no tool in the
/// group is still `running` and none of its source messages are streaming.
///
/// A group of **one** degrades to a plain [`TimelineRow::ToolLine`] instead.
/// Grouping exists to stop a burst of twelve reads from filling the transcript;
/// applied to a single read it only subtracts. The collapsed header says
/// "Explored 1 items" — an ungrammatical count in place of the filename, with
/// the one fact the row exists to convey (which file? which query?) hidden
/// behind a disclosure triangle, while the equivalent non-read tool one line
/// above shows its argument inline. Below the threshold the row is exactly what
/// it was before grouping was introduced.
fn flush_explore(rows: &mut Vec<TimelineRow>, acc: &mut Option<ExploreAcc>) {
    let Some(mut a) = acc.take() else { return };
    if a.tools.len() == 1 {
        rows.push(TimelineRow::ToolLine {
            run_id: a.run_id,
            tool: a.tools.remove(0),
        });
        return;
    }
    let completed = !a.streaming && a.tools.iter().all(|t| t.status != "running");
    rows.push(TimelineRow::ExploreGroup {
        key: a.key,
        run_id: a.run_id,
        tools: a.tools,
        completed,
    });
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

/// Stable `<For>` key for a timeline row.
///
/// A streaming `Message`/`Narration` row's key does NOT include content
/// length — it stays stable while content grows so the row's DOM subtree is
/// never unmounted/remounted per token (see `messages.rs`'s per-row `Memo`
/// lookup for how the rendered content still updates without a remount).
/// The key changes only on a structural transition (streaming ends, a tool
/// call is added, etc.); separators key on their day.
#[must_use]
pub fn row_key(row: &TimelineRow) -> String {
    match row {
        TimelineRow::DaySeparator { key, .. } => format!("sep:{key}"),
        TimelineRow::Message {
            id,
            is_streaming,
            is_intermediate,
            tool_call_count,
            has_model_info,
            clock,
            ..
        } => format!(
            "{id}:{is_streaming}:{is_intermediate}:{tool_call_count}:{has_model_info}:{clock}",
        ),
        TimelineRow::Narration { id, is_streaming } => format!("narr:{id}:{is_streaming}"),
        TimelineRow::ToolLine { run_id, tool } => format!(
            "tool:{run_id}:{}:{}:{:?}",
            tool.tool_id, tool.status, tool.duration_ms
        ),
        TimelineRow::ExploreGroup {
            key,
            tools,
            completed,
            ..
        } => {
            let running = tools.iter().filter(|t| t.status == "running").count();
            format!("{key}:{}:{completed}:{running}", tools.len())
        }
    }
}

/// Identity of a bubble for the typewriter reveal cursor
/// ([`crate::state::typewriter::TypewriterClock`]).
///
/// **Not** just the message id: `ChatState::begin_step` finalizes the trailing
/// bubble by *renaming* it to `intermediate-{run}-{n}` and then pushes a brand
/// new bubble that **re-uses the id `assistant-{run}`**. A cursor keyed on the
/// bare id therefore leaks from step N onto step N+1 — and since the sweep is
/// only pruned on the "finished and caught up" path (which a renamed bubble
/// never reaches), step N+1 inherited a cursor whose `revealed` was already
/// step N's length. `advance_reveal` then saw `revealed >= total` and revealed
/// everything at once: **only the first step of a run ever animated**.
///
/// Folding the iteration in makes the id re-use visible, so each step gets its
/// own cursor. Non-step bubbles (user rows, the final answer) carry no
/// iteration and fold in `0`, which is stable across the
/// streaming → `finalize_answer` transition so the answer's sweep is not
/// restarted at the finish line.
#[must_use]
pub fn reveal_key(message: &ChatMessage) -> String {
    format!("{}@{}", message.id, message.iteration.unwrap_or(0))
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
            author_user_id: None,
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
            author_user_id: None,
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
            author_user_id: None,
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
            author_user_id: None,
        }
    }

    fn tool(id: &str, name: &str, status: &str) -> crate::views::chat::state::ToolCallEntry {
        crate::views::chat::state::ToolCallEntry {
            tool_id: id.into(),
            tool_name: name.into(),
            status: status.into(),
            duration_ms: None,
            started_at_ms: None,
        }
    }

    fn msg_step_tools(
        id: &str,
        it: usize,
        content: &str,
        streaming: bool,
        tools: Vec<crate::views::chat::state::ToolCallEntry>,
    ) -> ChatMessage {
        let mut m = msg_step(id, it, content, streaming);
        m.tool_calls = tools;
        m
    }

    /// A trailing `assistant-{run}` step bubble (NOT intermediate) carrying a
    /// tool call — the shape `begin_step` leaves the current turn in.
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
                started_at_ms: None,
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
            author_user_id: None,
        }
    }

    /// Empty streaming placeholder for the just-started current turn: tagged,
    /// non-intermediate, no content, no tools — emits the cursor `Narration`
    /// row rather than rendering as a bare bubble.
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
            author_user_id: None,
        }
    }

    #[test]
    fn narration_then_tools_emit_in_order() {
        // One step: narration + edit tool -> Narration row first, ToolLine after
        let msgs = vec![
            msg_user("u1", "hi"),
            msg_step_tools(
                "intermediate-r1-1",
                1,
                "我先改配置",
                false,
                vec![tool("t1", "file_edit", "completed")],
            ),
            msg_final("r1", "done"),
        ];
        let rows = derive_timeline(&msgs, "Today", "Yesterday");
        let kinds: Vec<&str> = rows
            .iter()
            .map(|r| match r {
                TimelineRow::Message { .. } => "msg",
                TimelineRow::Narration { .. } => "narr",
                TimelineRow::ToolLine { .. } => "tool",
                TimelineRow::ExploreGroup { .. } => "explore",
                TimelineRow::DaySeparator { .. } => "sep",
            })
            .collect();
        assert_eq!(kinds, vec!["msg", "narr", "tool", "msg"]);
    }

    #[test]
    fn consecutive_readonly_tools_merge_across_steps() {
        // step1: read+search (no narration text, empty non-streaming content); step2: another read
        // -> three read-only tools merged into one ExploreGroup (across messages, no narration in between)
        let msgs = vec![
            msg_step_tools(
                "intermediate-r1-1",
                1,
                "",
                false,
                vec![
                    tool("t1", "file_read", "completed"),
                    tool("t2", "web_search", "completed"),
                ],
            ),
            msg_step_tools(
                "intermediate-r1-2",
                2,
                "",
                false,
                vec![tool("t3", "file_read", "completed")],
            ),
        ];
        let rows = derive_timeline(&msgs, "Today", "Yesterday");
        let group = rows
            .iter()
            .find_map(|r| match r {
                TimelineRow::ExploreGroup {
                    key,
                    tools,
                    completed,
                    ..
                } => Some((key.clone(), tools.len(), *completed)),
                _ => None,
            })
            .expect("one explore group");
        assert_eq!(group.1, 3);
        assert!(group.2, "all terminal → completed");
        assert_eq!(group.0, "explore:r1:t1", "key anchors to first tool id");
    }

    /// A lone read is a tool line, not a one-item "group".
    ///
    /// The header it used to get read "Explored 1 items" and hid the filename
    /// behind a triangle — strictly less than the `ToolLine` the same call got
    /// before grouping existed, and less than the non-read tool rendered
    /// directly above it.
    #[test]
    fn a_lone_read_renders_as_a_tool_line_not_a_group() {
        let msgs = vec![msg_step_tools(
            "intermediate-r1-1",
            1,
            "",
            false,
            vec![tool("t1", "file_read", "completed")],
        )];
        let rows = derive_timeline(&msgs, "Today", "Yesterday");
        assert!(
            matches!(rows.as_slice(), [TimelineRow::ToolLine { tool, .. }] if tool.tool_id == "t1"),
            "expected a single ToolLine, got {rows:?}"
        );
    }

    /// …and two of them still collapse, so the threshold is the only thing
    /// that changed.
    #[test]
    fn two_reads_still_collapse_into_a_group() {
        let msgs = vec![msg_step_tools(
            "intermediate-r1-1",
            1,
            "",
            false,
            vec![
                tool("t1", "file_read", "completed"),
                tool("t2", "file_read", "completed"),
            ],
        )];
        let rows = derive_timeline(&msgs, "Today", "Yesterday");
        assert!(
            matches!(rows.as_slice(), [TimelineRow::ExploreGroup { tools, .. }] if tools.len() == 2),
            "expected one 2-item ExploreGroup, got {rows:?}"
        );
    }

    #[test]
    fn narration_flushes_explore_group() {
        // reads -> narration -> reads => two ExploreGroups, narration row
        // sandwiched in between. Two reads per side, not one: a lone read
        // degrades to a `ToolLine` (see `flush_explore`), which would make this
        // a test of the degradation rather than of the flush boundary.
        let msgs = vec![
            msg_step_tools(
                "intermediate-r1-1",
                1,
                "",
                false,
                vec![
                    tool("t1", "file_read", "completed"),
                    tool("t1b", "file_read", "completed"),
                ],
            ),
            msg_step_tools(
                "intermediate-r1-2",
                2,
                "找到了，接着看第二处",
                false,
                vec![
                    tool("t2", "file_read", "completed"),
                    tool("t2b", "file_read", "completed"),
                ],
            ),
        ];
        let rows = derive_timeline(&msgs, "Today", "Yesterday");
        let kinds: Vec<&str> = rows
            .iter()
            .map(|r| match r {
                TimelineRow::Narration { .. } => "narr",
                TimelineRow::ExploreGroup { .. } => "explore",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, vec!["explore", "narr", "explore"]);
    }

    #[test]
    fn action_tool_flushes_explore_group() {
        let msgs = vec![msg_step_tools(
            "intermediate-r1-1",
            1,
            "",
            false,
            vec![
                tool("t1", "file_read", "completed"),
                tool("t1b", "file_read", "completed"),
                tool("t2", "file_edit", "completed"),
                tool("t3", "file_read", "completed"),
                tool("t3b", "file_read", "completed"),
            ],
        )];
        let rows = derive_timeline(&msgs, "Today", "Yesterday");
        let kinds: Vec<&str> = rows
            .iter()
            .map(|r| match r {
                TimelineRow::ExploreGroup { .. } => "explore",
                TimelineRow::ToolLine { .. } => "tool",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, vec!["explore", "tool", "explore"]);
    }

    #[test]
    fn running_or_streaming_group_not_completed() {
        let msgs = vec![msg_step_tools(
            "intermediate-r1-1",
            1,
            "",
            true,
            vec![
                tool("t1", "file_read", "running"),
                tool("t2", "file_read", "completed"),
            ],
        )];
        let rows = derive_timeline(&msgs, "Today", "Yesterday");
        let completed = rows.iter().find_map(|r| match r {
            TimelineRow::ExploreGroup { completed, .. } => Some(*completed),
            _ => None,
        });
        assert_eq!(completed, Some(false));
    }

    #[test]
    fn empty_streaming_placeholder_emits_cursor_narration() {
        let msgs = vec![msg_empty_step("r1", 1)];
        let rows = derive_timeline(&msgs, "Today", "Yesterday");
        assert!(matches!(rows.as_slice(),
            [TimelineRow::Narration { is_streaming, .. }] if *is_streaming));
    }

    #[test]
    fn empty_finished_step_emits_nothing() {
        let mut m = msg_empty_step("r1", 1);
        m.is_streaming = false;
        let rows = derive_timeline(&[m], "Today", "Yesterday");
        assert!(rows.is_empty());
    }

    #[test]
    fn final_answer_and_user_stay_message_rows() {
        // Original pure_text_final_answer_stays_standalone / final_answer_with_tool_call_escapes_the_strip
        // semantics preserved under the new model: final answer is a Message row
        let mut answer = msg_tool_step("r-r", 2, "最终报告……", false);
        answer.is_final = true;
        let msgs = vec![
            msg_user("u1", "q"),
            msg_step("intermediate-r-r-1", 1, "searching", false),
            answer,
        ];
        let rows = derive_timeline(&msgs, "Today", "Yesterday");
        assert!(rows.iter().any(|r| matches!(r,
            TimelineRow::Message { id, tool_call_count, .. }
                if id == "assistant-r-r" && *tool_call_count > 0)));
    }

    #[test]
    fn row_key_narration_is_stable_across_content_growth() {
        // Content growth alone (the common case: a token arriving mid-stream)
        // must NOT change the key — that's what let the DOM subtree survive
        // across tokens instead of remounting every one.
        let m1 = msg_step("intermediate-r1-1", 1, "partial", true);
        let m2 = msg_step("intermediate-r1-1", 1, "partial more", true);
        let rows1 = vec![TimelineRow::Narration {
            id: m1.id.clone(),
            is_streaming: m1.is_streaming,
        }];
        let rows2 = vec![TimelineRow::Narration {
            id: m2.id.clone(),
            is_streaming: m2.is_streaming,
        }];
        assert_eq!(row_key(&rows1[0]), row_key(&rows2[0]));
    }

    #[test]
    fn row_key_narration_changes_when_streaming_ends() {
        let m1 = msg_step("intermediate-r1-1", 1, "text", true);
        let mut m2 = m1.clone();
        m2.is_streaming = false;
        assert_ne!(
            row_key(&TimelineRow::Narration {
                id: m1.id.clone(),
                is_streaming: m1.is_streaming
            }),
            row_key(&TimelineRow::Narration {
                id: m2.id.clone(),
                is_streaming: m2.is_streaming
            })
        );
    }

    #[test]
    fn row_key_explore_changes_on_status_transition() {
        let g = |status: &str| TimelineRow::ExploreGroup {
            key: "explore:r1:t1".into(),
            run_id: "r1".into(),
            tools: vec![tool("t1", "file_read", status)],
            completed: status != "running",
        };
        assert_ne!(row_key(&g("running")), row_key(&g("completed")));
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
            TimelineRow::Message { id, clock, .. } => {
                assert_eq!(id, "a");
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

    /// Regression for the "only the first step animates" bug: `begin_step`
    /// renames the finished bubble and re-uses `assistant-{run}` for the next
    /// one, so a reveal cursor keyed on the bare id leaked across steps and the
    /// inherited `revealed` immediately satisfied `revealed >= total`.
    #[test]
    fn reveal_key_separates_two_steps_that_share_the_assistant_id() {
        let step1 = msg_step("assistant-r1", 1, "first step narration", true);
        let step2 = msg_step("assistant-r1", 2, "second", true);
        assert_eq!(step1.id, step2.id, "precondition: the id really is re-used");
        assert_ne!(reveal_key(&step1), reveal_key(&step2));
    }

    /// The same bubble streaming then being promoted by `finalize_answer` keeps
    /// one key, so the answer's sweep is not restarted at the finish line.
    #[test]
    fn reveal_key_is_stable_across_finalize() {
        let mut m = msg_step("assistant-r1", 3, "partial", true);
        let before = reveal_key(&m);
        m.content = "the whole authoritative answer".into();
        m.is_streaming = false;
        m.is_final = true;
        m.is_intermediate = false;
        assert_eq!(reveal_key(&m), before);
    }

    /// A tool settled to `unknown` (its outcome frame was dropped and the run
    /// ended) is terminal: the explore block must collapse instead of pulsing
    /// "Exploring…" for the rest of the session.
    #[test]
    fn settled_unknown_tool_completes_its_explore_group() {
        let msgs = vec![msg_step_tools(
            "intermediate-r1-1",
            1,
            "",
            false,
            vec![
                tool(
                    "t1",
                    "file_read",
                    crate::views::chat::state::TOOL_STATUS_UNKNOWN,
                ),
                tool("t2", "file_read", "completed"),
            ],
        )];
        let rows = derive_timeline(&msgs, "Today", "Yesterday");
        let completed = rows.iter().find_map(|r| match r {
            TimelineRow::ExploreGroup { completed, .. } => Some(*completed),
            _ => None,
        });
        assert_eq!(completed, Some(true));
    }

    #[test]
    fn row_key_distinguishes_separators_from_messages() {
        let sep = TimelineRow::DaySeparator {
            key: "5".into(),
            label: "D5".into(),
        };
        assert_eq!(row_key(&sep), "sep:5");
        let m = TimelineRow::Message {
            id: "x".into(),
            role: "assistant".into(),
            has_plan_archive: false,
            is_streaming: false,
            is_intermediate: false,
            tool_call_count: 0,
            has_model_info: false,
            clock: "T1000".into(),
        };
        assert!(row_key(&m).starts_with("x:"));
    }
}
