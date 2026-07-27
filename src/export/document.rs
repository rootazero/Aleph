//! The published deliverable — the finished work product, on its own page.
//!
//! # Why this is not the transcript export
//!
//! [`super::session_html`] renders the whole conversation: every turn, in
//! order, with its attachments. That is the right artifact for "show me how we
//! got here" and the wrong one for "here is the thing you asked me to make".
//! Handing someone a chat log when they asked for a report buries the answer
//! in the process that produced it, and the two documents want opposite
//! things — the transcript wants completeness, the deliverable wants exactly
//! the finished text and nothing else.
//!
//! So a deliverable is one title, one body of Markdown the agent wrote on
//! purpose, and the figures it chose to include. No roles, no timestamps, no
//! tool calls.
//!
//! Everything about *how* it is rendered — the shell, the CSP, the typography,
//! the attachment budget — comes from [`super::page`], so the security
//! properties argued for the transcript hold here verbatim: no `<script>`, no
//! remote fetch, readable offline from a `file://` path.

use super::markdown::render_markdown;
use super::page::{render_artifacts, render_page_head, wrap_document};
use super::{ExportArtifact, ExportDocument};

/// Heading over the figures a deliverable carries.
///
/// Not "Attachments": in a report these are the charts and tables the text
/// refers to, not incidental files that rode along with a conversation.
const FIGURES_HEADING: &str = "Figures";

/// Render a published deliverable as a self-contained HTML document.
///
/// `figures` are artifacts the agent named; they are inlined subject to the
/// same byte budget as any export and listed rather than dropped when they do
/// not fit.
#[must_use]
pub fn render_document_html(doc: &ExportDocument, figures: &[ExportArtifact]) -> String {
    let mut body = String::with_capacity(2048 + doc.markdown.len() * 2);

    body.push_str(&render_page_head(&doc.title, &doc.provenance));
    body.push_str("<main>\n<article class=\"doc prose\">\n");
    body.push_str(&render_markdown(&doc.markdown));
    body.push_str("\n</article>\n");
    body.push_str(&render_artifacts(FIGURES_HEADING, figures));
    body.push_str("</main>\n");

    wrap_document(&doc.title, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(title: &str, markdown: &str) -> ExportDocument {
        ExportDocument {
            title: title.to_string(),
            provenance: "Aleph · 2026-07-26 19:35".to_string(),
            markdown: markdown.to_string(),
        }
    }

    #[test]
    fn the_body_is_the_markdown_and_nothing_else() {
        let html = render_document_html(&doc("Q3 review", "# Findings\n\n- one\n- two"), &[]);
        assert!(html.contains("<h1>Findings</h1>"), "{html}");
        assert!(html.contains("<li>one</li>"), "{html}");
        // None of the transcript's furniture leaks into a deliverable. The
        // assertions name the *markup*, not the string: the shared stylesheet
        // is embedded in every document and legitimately mentions `.msg-body`.
        assert!(
            !html.contains("class=\"msg-body"),
            "transcript chrome leaked: {html}"
        );
        assert!(!html.contains("class=\"role\""), "{html}");
        assert!(!html.contains("No messages."), "{html}");
    }

    #[test]
    fn the_body_carries_the_shared_prose_class() {
        let html = render_document_html(&doc("t", "text"), &[]);
        assert!(html.contains("class=\"doc prose\""), "{html}");
    }

    #[test]
    fn a_deliverable_never_contains_a_script_tag() {
        let html = render_document_html(
            &doc("t", "<script>alert(1)</script>\n\n[x](javascript:alert(1))"),
            &[],
        );
        assert!(!html.contains("<script"), "script tag present: {html}");
        assert!(html.contains("&lt;script&gt;"), "not escaped: {html}");
        assert!(!html.contains("javascript:alert"), "{html}");
    }

    #[test]
    fn the_title_and_provenance_are_escaped() {
        let mut d = doc("<b>t</b>", "body");
        d.provenance = "<i>when</i>".to_string();
        let html = render_document_html(&d, &[]);
        assert!(!html.contains("<b>t</b>"), "{html}");
        assert!(!html.contains("<i>when</i>"), "{html}");
        assert!(html.contains("&lt;i&gt;when&lt;/i&gt;"), "{html}");
    }

    #[test]
    fn figures_are_inlined_under_their_own_heading() {
        let fig = ExportArtifact {
            filename: "chart.png".to_string(),
            mime_type: "image/png".to_string(),
            bytes: Some(vec![0u8; 8]),
            size: 8,
        };
        let html = render_document_html(&doc("t", "See the chart."), &[fig]);
        assert!(
            html.contains(&format!("<h2>{FIGURES_HEADING}</h2>")),
            "{html}"
        );
        assert!(html.contains("<img src=\"data:image/png;base64,"), "{html}");
        // "Attachments" is the transcript's word, not this document's. Again
        // the assertion names the heading markup: the shared stylesheet carries
        // an `── Attachments ──` section comment.
        assert!(!html.contains("<h2>Attachments</h2>"), "{html}");
    }

    #[test]
    fn a_deliverable_with_no_figures_has_no_figure_section() {
        let html = render_document_html(&doc("t", "just text"), &[]);
        assert!(
            !html.contains(&format!("<h2>{FIGURES_HEADING}</h2>")),
            "{html}"
        );
        assert!(!html.contains("class=\"artifacts\""), "{html}");
    }

    #[test]
    fn an_empty_body_still_produces_a_valid_document() {
        let html = render_document_html(&doc("t", ""), &[]);
        assert!(html.starts_with("<!DOCTYPE html>"), "{html}");
        assert!(html.ends_with("</html>\n"), "{html}");
    }
}
