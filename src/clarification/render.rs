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
//! Two products, always built together so they cannot disagree:
//! * the **text body**, which every channel can carry and which always lists
//!   every option — this is the plain-text fallback, not a degraded mode; and
//! * an optional **inline keyboard**, a tappable shortcut for channels that
//!   render one. The callback payload is `clarify:<1-based index>`, byte-identical
//!   to what a user typing that number produces, so
//!   [`crate::clarification::session::interpret_reply`] stays the single
//!   interpreter for both.

use super::{ClarificationOption, ClarificationQuestion};
use crate::gateway::channel::{InlineButton, InlineKeyboard};
use crate::gateway::i18n::Msg;

/// Max button label length (chars) before truncation.
const MAX_LABEL_CHARS: usize = 32;

/// Max choices rendered as buttons; beyond this the menu is text-only.
///
/// The numbered text body always lists every choice, so a long list simply
/// falls back to typed selection and the callback payload stays well under the
/// channel's per-message limits.
const MAX_CHOICE_BUTTONS: usize = 12;

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
/// Whitespace inside `label` and `description` is collapsed so a model-supplied
/// newline / ANSI byte in either field cannot break the numbering — every
/// numbered row is one line, so the user's `k+1` reply selects the matching
/// option regardless of model output quirks.
fn menu(options: &[ClarificationOption]) -> String {
    let mut out = String::new();
    for (i, opt) in options.iter().enumerate() {
        let label = collapse_whitespace(&opt.label);
        match opt
            .description
            .as_deref()
            .map(collapse_whitespace)
            .filter(|d| !d.is_empty())
        {
            Some(desc) => out.push_str(&format!("{}. {} — {desc}\n", i + 1, label)),
            None => out.push_str(&format!("{}. {}\n", i + 1, label)),
        }
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
fn keyboard_for(question: &ClarificationQuestion) -> Option<InlineKeyboard> {
    if !question.has_options()
        || question.multi_select
        || question.options.len() > MAX_CHOICE_BUTTONS
    {
        return None;
    }
    let buttons: Vec<InlineButton> = question
        .options
        .iter()
        .enumerate()
        .map(|(i, opt)| InlineButton {
            text: button_label(i + 1, &opt.label),
            // 1-based index; the router strips `clarify:` and resolves the
            // pending clarification with the bare number (see
            // `try_intercept_hitl`).
            callback_data: format!("clarify:{}", i + 1),
        })
        .collect();
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
        .map(|h| format!("[{h}] "))
        .unwrap_or_default();

    let mut text = format!("❓{position} {header}{}", question.prompt);
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
        keyboard: keyboard_for(question),
    }
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
        let kb = keyboard_for(&ClarificationQuestion::select("q", "?", opts()))
            .expect("2 options render a keyboard");
        let datas: Vec<&str> = kb
            .rows
            .iter()
            .flatten()
            .map(|b| b.callback_data.as_str())
            .collect();
        assert_eq!(datas, vec!["clarify:1", "clarify:2"]);
    }

    /// A tap can only ever carry ONE index, so offering buttons for a
    /// multi-select question would render a control that answers less than the
    /// question asks. Text menu still lists everything.
    #[test]
    fn multi_select_suppresses_the_keyboard_but_not_the_menu() {
        let q = ClarificationQuestion::select("q", "?", opts()).with_multi_select(true);
        assert!(keyboard_for(&q).is_none());
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
            keyboard_for(&ClarificationQuestion::select("q", "?", many)).is_none(),
            "oversized choice lists must not render buttons"
        );

        let twelve: Vec<ClarificationOption> = (0..12)
            .map(|i| ClarificationOption::new(&format!("opt{i}"), &format!("opt{i}")))
            .collect();
        let kb = keyboard_for(&ClarificationQuestion::select("q", "?", twelve))
            .expect("12 choices render");
        assert_eq!(kb.rows.iter().flatten().count(), 12);
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
