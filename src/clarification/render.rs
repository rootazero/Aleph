//! The one place a [`ClarificationQuestion`] becomes bytes a human sees.
//!
//! This used to live inside `AskUserTool::build_request`, which was fine while
//! `ask_user` was the only producer and every question was answered in one
//! shot. Neither holds any more: the registry now advances a cursor through a
//! multi-question request, so **the surface that delivers question *k+1* is the
//! one that received the answer to question *k*** — the inbound router, not the
//! tool. Rendering therefore has to sit where both can reach it, on the side
//! everything else depends on (`ask_user` → `clarification`, router →
//! `clarification`), not inside one of the consumers.
//!
//! The text body needs no live identity. Only a registered request gets an
//! inline keyboard, with buttons addressed to an exact request and cursor.

use super::{ClarificationOption, ClarificationQuestion};
use crate::gateway::channel::{InlineButton, InlineKeyboard};
use crate::gateway::i18n::Msg;
use uuid::Uuid;

pub(crate) const CLARIFICATION_BUTTON_PREFIX: &str = "clarify:";
const MAX_CALLBACK_BYTES: usize = 64;

pub(crate) struct ClarificationButton {
    pub request_id: Uuid,
    pub cursor: usize,
    pub index: u32,
}

pub(crate) fn parse_button_callback(payload: &str) -> Option<ClarificationButton> {
    if payload.len() > MAX_CALLBACK_BYTES {
        return None;
    }
    let payload = payload.strip_prefix(CLARIFICATION_BUTTON_PREFIX)?;
    let (request_id, choice) = payload.split_once(':')?;
    let (cursor, ordinal) = choice.split_once(':')?;
    if request_id.len() != 32
        || !cursor.bytes().all(|b| b.is_ascii_digit())
        || !ordinal.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let request_id = Uuid::parse_str(request_id).ok()?;
    let cursor = cursor.parse().ok()?;
    let ordinal: u32 = ordinal.parse().ok()?;
    let index = ordinal.checked_sub(1)?;
    (index < MAX_CHOICE_BUTTONS as u32).then_some(ClarificationButton {
        request_id,
        cursor,
        index,
    })
}

/// Max button label length (chars) before truncation.
const MAX_LABEL_CHARS: usize = 32;

/// Max choices rendered as buttons; beyond this the menu is text-only.
///
/// The numbered text body is independently bounded to stay within channel
/// message-size limits.
const MAX_CHOICE_BUTTONS: usize = 12;

/// Bound text-only menus for channel message-size limits.
const MAX_MENU_OPTIONS: usize = 12;
const MAX_MENU_FIELD_CHARS: usize = 100;

/// One question, rendered for delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedQuestion {
    /// Complete message body — prompt, numbered menu, and the reply hint.
    pub text: String,
    /// Tappable shortcut, when the question and channel both allow one.
    pub keyboard: Option<InlineKeyboard>,
}

/// Render a compact button label `"<n>. <label>"`, truncated so a long choice
/// doesn't bloat the keyboard — the full text is always listed in the message
/// body, so the button only needs to be tappable, not complete.
fn button_label(index: usize, label: &str) -> String {
    let text = format!("{index}. {label}");
    if text.chars().count() > MAX_LABEL_CHARS {
        let truncated: String = text.chars().take(MAX_LABEL_CHARS - 1).collect();
        format!("{truncated}…")
    } else {
        text
    }
}

/// Collapse internal whitespace runs in `s` to a single space, so an option
/// description (or label) containing an embedded `\n` / `\r` / `\t` does not
/// shift the numbered prefix and make the user's `k+1`-th reply select the
/// wrong row.
fn collapse_whitespace(s: &str) -> String {
    let mut buf = String::with_capacity(s.len());
    let mut in_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !in_ws {
                buf.push(' ');
                in_ws = true;
            }
        } else {
            buf.push(ch);
            in_ws = false;
        }
    }
    buf.trim().to_string()
}

/// The numbered menu body for `options`.
///
/// Whitespace inside `label` and `description` is collapsed and both fields
/// are length-limited, so model output cannot break numbering or grow the body
/// without bound.
///
/// The label and the description are then escaped: they are the model's own
/// words, and a channel that renders this body as Markdown would otherwise
/// turn a label like `[Approve](https://evil.example)` into a live link. The
/// escaping happens AFTER the collapse and the truncation, so the length caps
/// still measure the raw model text.
fn menu(options: &[ClarificationOption]) -> String {
    let mut out = String::new();
    for (i, opt) in options.iter().take(MAX_MENU_OPTIONS).enumerate() {
        let label = escape_markdown(
            &collapse_whitespace(&opt.label)
                .chars()
                .take(MAX_MENU_FIELD_CHARS)
                .collect::<String>(),
        );
        let description = opt
            .description
            .as_deref()
            .map(collapse_whitespace)
            .filter(|d| !d.is_empty())
            .map(|desc| {
                escape_markdown(&desc.chars().take(MAX_MENU_FIELD_CHARS).collect::<String>())
            });
        match description {
            Some(desc) => out.push_str(&format!("{}. {} — {desc}\n", i + 1, label)),
            None => out.push_str(&format!("{}. {}\n", i + 1, label)),
        }
    }
    if options.len() > MAX_MENU_OPTIONS {
        out.push_str(&format!("…and {} more\n", options.len() - MAX_MENU_OPTIONS));
    }
    out
}

/// Build the inline keyboard for `question`, or `None` when one would be
/// misleading or oversized.
///
/// Suppressed for three distinct reasons, each load-bearing:
/// * **no options** — nothing to tap;
/// * **multi-select** — a tap carries exactly one index, so a keyboard would
///   offer a control that silently answers less than the question asks;
/// * **too many options** — the payload would outgrow the channel's limits.
fn keyboard_for(
    question: &ClarificationQuestion,
    request_id: Uuid,
    cursor: usize,
) -> Option<InlineKeyboard> {
    if question.secret
        || !question.has_options()
        || question.multi_select
        || question.options.len() > MAX_CHOICE_BUTTONS
    {
        return None;
    }
    let request_id = request_id.simple();
    let buttons: Vec<InlineButton> = question
        .options
        .iter()
        .enumerate()
        .map(|(i, opt)| InlineButton {
            text: button_label(i + 1, &opt.label),
            callback_data: format!(
                "{CLARIFICATION_BUTTON_PREFIX}{request_id}:{cursor}:{}",
                i + 1
            ),
        })
        .collect();
    if buttons
        .iter()
        .any(|b| b.callback_data.len() > MAX_CALLBACK_BYTES)
    {
        return None;
    }
    let mut keyboard = InlineKeyboard::new();
    for chunk in buttons.chunks(2) {
        keyboard.rows.push(chunk.to_vec());
    }
    Some(keyboard)
}

/// Render question `index` of `total` (both 0-based / count).
///
/// `total > 1` adds a `(2/3)` progress marker: on a sequential surface the user
/// is answering a queue and has no other way to know how long it is.
#[must_use]
pub fn render(question: &ClarificationQuestion, index: usize, total: usize) -> RenderedQuestion {
    let position = if total > 1 {
        format!(" ({}/{total})", index + 1)
    } else {
        String::new()
    };
    let header = question
        .header
        .as_deref()
        .map(|h| format!("[{}] ", escape_markdown(h)))
        .unwrap_or_default();

    let prompt = escape_markdown(&collapse_whitespace(&question.prompt));
    let mut text = format!("❓{position} {header}{prompt}");
    // The hint is read by a person, so it is translated (the prompt and the
    // option labels are the model's own words and are already in the user's
    // language). Everything `ask` produces for the MODEL — the headless denial,
    // the delivery failure, the withheld-secret reason — stays English on
    // purpose: see `gateway::i18n`.
    let hint = crate::gateway::i18n::t_ui(if question.has_options() {
        if question.multi_select {
            Msg::ClarifyReplyPickMany
        } else {
            Msg::ClarifyReplyPickOne
        }
    } else {
        Msg::ClarifyReplyFreeText
    });
    if question.has_options() {
        text.push_str("\n\n");
        text.push_str(&menu(&question.options));
        text.push('\n');
        text.push_str(&hint);
    } else {
        text.push_str("\n\n");
        text.push_str(&hint);
    }

    RenderedQuestion {
        text,
        keyboard: None,
    }
}

pub(crate) fn render_addressed(
    question: &ClarificationQuestion,
    cursor: usize,
    total: usize,
    request_id: &str,
) -> RenderedQuestion {
    let mut rendered = render(question, cursor, total);
    rendered.keyboard = Uuid::parse_str(request_id)
        .ok()
        .and_then(|id| keyboard_for(question, id, cursor));
    rendered
}

/// Escape Telegram-flavoured Markdown special characters in LLM-supplied text.
/// Applied to question headers, to prompts, and to the option labels and
/// descriptions of the menu body, so formatting control characters cannot
/// alter the downstream message. The menu's numbering prefix is generated, not
/// model-supplied, and the inline-keyboard button text is plain text on every
/// surface that carries a keyboard — neither is escaped.
fn escape_markdown(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('*', r"\*")
        .replace('_', r"\_")
        .replace('`', r"\`")
        .replace('[', r"\[")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Vec<ClarificationOption> {
        vec![
            ClarificationOption::new("in-place", "in-place").with_description("brief downtime"),
            ClarificationOption::new("blue-green", "blue-green"),
        ]
    }

    /// The hint the user reads comes from the catalog, so a translated install
    /// gets a translated hint. Compared against the catalog entry rather than a
    /// literal: the two used to be the same string in two places, and that is
    /// the shape that drifts.
    fn hint(msg: Msg<'static>) -> String {
        crate::gateway::i18n::t(msg, crate::gateway::i18n::Locale::En)
    }

    #[test]
    fn free_text_question_has_no_menu_and_no_keyboard() {
        let r = render(&ClarificationQuestion::text("q", "Pick?"), 0, 1);
        assert!(
            r.text.ends_with(&hint(Msg::ClarifyReplyFreeText)),
            "{}",
            r.text
        );
        assert!(r.keyboard.is_none());
        // Single question: no progress marker.
        assert!(!r.text.contains("(1/"));
    }

    /// The defect this module exists to make impossible: an option description
    /// was wired onto `ClarificationOption`, rendered by the channel path, and
    /// dropped by every other surface because each one re-rendered on its own.
    #[test]
    fn descriptions_reach_the_rendered_menu() {
        let r = render(
            &ClarificationQuestion::select("q", "Strategy?", opts()),
            0,
            1,
        );
        assert!(
            r.text.contains("1. in-place — brief downtime"),
            "{}",
            r.text
        );
        assert!(r.text.contains("2. blue-green\n"), "{}", r.text);
        assert!(
            r.text.ends_with(&hint(Msg::ClarifyReplyPickOne)),
            "{}",
            r.text
        );
    }

    #[test]
    fn multi_question_render_carries_its_position_and_header() {
        let q = ClarificationQuestion::select("q", "Where?", opts()).with_header("Env");
        let r = render(&q, 1, 3);
        assert!(r.text.starts_with("❓ (2/3) [Env] Where?"), "{}", r.text);
    }

    #[test]
    fn keyboard_callbacks_are_one_based_indices() {
        let id = Uuid::new_v4();
        let q = ClarificationQuestion::select("q", "?", opts());
        assert!(render(&q, 1, 3).keyboard.is_none());
        let kb = render_addressed(&q, 1, 3, &id.to_string())
            .keyboard
            .expect("2 options render a keyboard");
        let datas: Vec<&str> = kb
            .rows
            .iter()
            .flatten()
            .map(|b| b.callback_data.as_str())
            .collect();
        for (index, data) in datas.into_iter().enumerate() {
            assert_eq!(data, format!("clarify:{}:1:{}", id.simple(), index + 1));
            let parsed = parse_button_callback(data).unwrap();
            assert_eq!(
                (parsed.request_id, parsed.cursor, parsed.index),
                (id, 1, index as u32)
            );
        }
    }

    /// A tap can only ever carry ONE index, so offering buttons for a
    /// multi-select question would render a control that answers less than the
    /// question asks. Text menu still lists everything.
    #[test]
    fn multi_select_suppresses_the_keyboard_but_not_the_menu() {
        let q = ClarificationQuestion::select("q", "?", opts()).with_multi_select(true);
        assert!(keyboard_for(&q, Uuid::nil(), 0).is_none());
        let r = render(&q, 0, 1);
        assert!(r.text.contains("1. in-place"));
        assert!(
            r.text.ends_with(&hint(Msg::ClarifyReplyPickMany)),
            "{}",
            r.text
        );
    }

    #[test]
    fn keyboard_caps_long_choice_lists_to_text_only() {
        let many: Vec<ClarificationOption> = (0..20)
            .map(|i| ClarificationOption::new(&format!("opt{i}"), &format!("opt{i}")))
            .collect();
        assert!(
            keyboard_for(
                &ClarificationQuestion::select("q", "?", many),
                Uuid::nil(),
                0
            )
            .is_none(),
            "oversized choice lists must not render buttons"
        );

        let twelve: Vec<ClarificationOption> = (0..12)
            .map(|i| ClarificationOption::new(&format!("opt{i}"), &format!("opt{i}")))
            .collect();
        let kb = keyboard_for(
            &ClarificationQuestion::select("q", "?", twelve),
            Uuid::nil(),
            3,
        )
        .expect("12 choices render");
        assert_eq!(kb.rows.iter().flatten().count(), 12);
        let last = &kb.rows.last().unwrap().last().unwrap().callback_data;
        assert!(last.len() <= MAX_CALLBACK_BYTES);
        assert_eq!(parse_button_callback(last).unwrap().index, 11);
    }

    #[test]
    fn menu_bounds_option_count_and_field_lengths() {
        let many: Vec<ClarificationOption> = (0..MAX_MENU_OPTIONS + 2)
            .map(|i| {
                ClarificationOption::new(&format!("{i}"), &"x".repeat(MAX_MENU_FIELD_CHARS + 1))
            })
            .collect();
        let rendered = menu(&many);
        assert!(rendered.contains("…and 2 more"));
        assert!(!rendered.contains(&format!("{}. ", MAX_MENU_OPTIONS + 1)));
        assert!(rendered.lines().next().unwrap().len() < 250);
    }

    #[test]
    fn button_label_truncates_long_choice() {
        assert_eq!(button_label(1, "staging"), "1. staging");
        let long = button_label(2, &"x".repeat(80));
        assert!(
            long.chars().count() <= MAX_LABEL_CHARS,
            "label too long: {long}"
        );
        assert!(long.ends_with('…'));
    }
}
