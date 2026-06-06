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
        // Fold consecutive intermediate (Think→Act) step bubbles of one run
        // into a single StepStrip so a long run doesn't stretch the column.
        if m.role == "assistant" && m.is_intermediate {
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

/// Flush accumulated intermediate steps into one StepStrip row. No-op when
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
        TimelineRow::StepStrip { run_id, steps, completed } => {
            format!("strip:{run_id}:{}:{completed}", steps.len())
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
pub fn now_millis() -> i64 {
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
fn local_day_ordinal(_ts_millis: i64) -> i64 {
    0
}

/// Local "HH:MM" for a message's timestamp.
#[cfg(target_arch = "wasm32")]
fn format_clock(ts_millis: i64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts_millis as f64));
    format!("{:02}:{:02}", d.get_hours(), d.get_minutes())
}
#[cfg(not(target_arch = "wasm32"))]
fn format_clock(_ts_millis: i64) -> String {
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
fn format_date(_ts_millis: i64) -> String {
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
pub fn parse_wire_timestamp(_raw: &str) -> Option<i64> {
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
            timestamp: ts,
            iteration: None,
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
            iteration: None,
            timestamp: None,
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
            iteration: Some(it),
            timestamp: None,
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
            iteration: None,
            timestamp: None,
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
        if let TimelineRow::StepStrip { run_id, steps, completed } = strips[0] {
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
