//! Assembly of the conversation-transcript export.
//!
//! This is the *record of how the work was done*. What the work produced is
//! [`super::document`] — a different document with a different audience, and
//! keeping them apart is the point: an export of the whole conversation is
//! evidence, not a deliverable.

use super::markdown::{escape_text, render_markdown};
use super::page::{render_artifacts, render_page_head, wrap_document};
use super::{ExportArtifact, ExportMessage};

/// Render one session — transcript plus artifacts — as a self-contained HTML
/// document.
#[must_use]
pub fn render_session_html(
    title: &str,
    messages: &[ExportMessage],
    artifacts: &[ExportArtifact],
) -> String {
    let mut body = String::with_capacity(4096);

    body.push_str(&render_page_head(
        title,
        &summary_line(messages.len(), artifacts.len()),
    ));
    body.push_str("<main>\n");
    body.push_str(&render_transcript(messages));
    body.push_str(&render_artifacts("Attachments", artifacts));
    body.push_str("</main>\n");

    wrap_document(title, &body)
}

fn summary_line(messages: usize, artifacts: usize) -> String {
    let m = if messages == 1 { "message" } else { "messages" };
    let a = if artifacts == 1 {
        "attachment"
    } else {
        "attachments"
    };
    format!("{messages} {m} · {artifacts} {a}")
}

fn render_transcript(messages: &[ExportMessage]) -> String {
    if messages.is_empty() {
        return "<section class=\"transcript\">\n<p class=\"empty\">No messages.</p>\n</section>\n"
            .to_string();
    }

    let mut out = String::from("<section class=\"transcript\">\n");
    for msg in messages {
        out.push_str("<article class=\"msg msg--");
        out.push_str(&role_slug(&msg.role));
        out.push_str("\">\n<div class=\"msg-head\"><span class=\"role\">");
        out.push_str(&escape_text(&msg.role));
        out.push_str("</span><span class=\"ts\">");
        out.push_str(&escape_text(&msg.timestamp));
        out.push_str("</span></div>\n<div class=\"msg-body prose\">\n");
        out.push_str(&render_markdown(&msg.text));
        out.push_str("\n</div>\n</article>\n");
    }
    out.push_str("</section>\n");
    out
}

/// Reduce a free-form role to a CSS-class-safe slug.
///
/// The role reaches a `class=` attribute; rather than escape it, restrict it —
/// an unknown role becomes `other` and simply gets the default styling.
fn role_slug(role: &str) -> String {
    let slug: String = role
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .take(24)
        .collect();
    if slug.is_empty() {
        "other".to_string()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, text: &str) -> ExportMessage {
        ExportMessage {
            role: role.to_string(),
            text: text.to_string(),
            timestamp: "2026-07-26 12:00".to_string(),
        }
    }

    fn art(name: &str, mime: &str, len: usize) -> ExportArtifact {
        ExportArtifact {
            filename: name.to_string(),
            mime_type: mime.to_string(),
            bytes: Some(vec![0u8; len]),
            size: len as u64,
        }
    }

    #[test]
    fn document_never_contains_a_script_tag() {
        let html = render_session_html(
            "s",
            &[msg("user", "<script>alert(1)</script>")],
            &[art("a.png", "image/png", 8)],
        );
        assert!(!html.contains("<script"), "script tag present");
    }

    #[test]
    fn script_injection_in_message_text_renders_as_literal_text() {
        let html = render_session_html("s", &[msg("user", "<script>alert(1)</script>")], &[]);
        assert!(html.contains("&lt;script&gt;"), "{html}");
    }

    #[test]
    fn javascript_link_in_message_is_neutralised() {
        let html = render_session_html("s", &[msg("assistant", "[x](javascript:alert(1))")], &[]);
        assert!(!html.contains("javascript:alert"), "{html}");
    }

    /// The transcript body carries both classes: `msg-body` for the bubble
    /// chrome that is transcript-only, `prose` for the Markdown typography it
    /// shares with a published deliverable.
    #[test]
    fn a_message_body_is_styled_as_shared_prose() {
        let html = render_session_html("s", &[msg("user", "hi")], &[]);
        assert!(html.contains("class=\"msg-body prose\""), "{html}");
    }

    #[test]
    fn role_slug_cannot_inject_into_the_class_attribute() {
        let html = render_session_html("s", &[msg("user\" onmouseover=\"x", "hi")], &[]);
        assert!(!html.contains("onmouseover=\"x\""), "{html}");
        assert_eq!(role_slug("user\" onmouseover=\"x"), "useronmouseoverx");
        assert_eq!(role_slug("***"), "other");
    }

    #[test]
    fn empty_session_still_produces_a_valid_document() {
        let html = render_session_html("Empty", &[], &[]);
        assert!(html.starts_with("<!DOCTYPE html>"), "{html}");
        assert!(html.contains("No messages."), "{html}");
        assert!(html.ends_with("</html>\n"), "{html}");
    }

    #[test]
    fn title_is_escaped_in_both_places_it_appears() {
        let html = render_session_html("<b>x</b>", &[], &[]);
        assert!(!html.contains("<b>x</b>"), "{html}");
        assert_eq!(html.matches("&lt;b&gt;x&lt;/b&gt;").count(), 2, "{html}");
    }

    #[test]
    fn the_summary_line_counts_both_kinds() {
        assert_eq!(summary_line(1, 1), "1 message · 1 attachment");
        assert_eq!(summary_line(0, 3), "0 messages · 3 attachments");
    }
}
