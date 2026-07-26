//! Assembly of the single-file export document.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;

use super::markdown::{escape_attr, escape_text, render_markdown};
use super::{ExportArtifact, ExportMessage, MAX_INLINE_ARTIFACT_BYTES, MAX_INLINE_TOTAL_BYTES};

/// The stylesheet is embedded at COMPILE TIME.
///
/// A running `aleph-server` does not read `dist/*` — the Panel is baked in via
/// `rust_embed` for exactly this reason. A renderer that read its template off
/// disk would be the same "changed the file, nothing happened" footgun one
/// layer down.
const EXPORT_CSS: &str = include_str!("export.css");

/// Content-Security-Policy carried by the document itself, so the guarantee
/// travels with the file even when it is opened from disk rather than served.
///
/// `default-src 'none'` with no `script-src` means no script can execute —
/// see the module docs for why that is load-bearing rather than decorative.
const CSP: &str = "default-src 'none'; img-src data:; style-src 'unsafe-inline'";

/// Render one session — transcript plus artifacts — as a self-contained HTML
/// document.
///
/// The byte budget is enforced here rather than trusted from the caller: an
/// artifact over [`MAX_INLINE_ARTIFACT_BYTES`], or one that would push the
/// running total past [`MAX_INLINE_TOTAL_BYTES`], is listed without its
/// payload instead of being inlined.
#[must_use]
pub fn render_session_html(
    title: &str,
    messages: &[ExportMessage],
    artifacts: &[ExportArtifact],
) -> String {
    let mut body = String::with_capacity(4096 + EXPORT_CSS.len());

    body.push_str("<header class=\"page-head\">\n<h1>");
    body.push_str(&escape_text(title));
    body.push_str("</h1>\n<p class=\"page-meta\">");
    body.push_str(&escape_text(&summary_line(messages.len(), artifacts.len())));
    body.push_str("</p>\n</header>\n<main>\n");

    body.push_str(&render_transcript(messages));
    body.push_str(&render_artifacts(artifacts));

    body.push_str("</main>\n");

    format!(
        "<!DOCTYPE html>\n\
         <html lang=\"en\">\n\
         <head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"{csp}\">\n\
         <title>{title}</title>\n\
         <style>\n{css}</style>\n\
         </head>\n\
         <body>\n{body}</body>\n\
         </html>\n",
        csp = escape_attr(CSP),
        title = escape_text(title),
        css = EXPORT_CSS,
        body = body,
    )
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
        out.push_str("</span></div>\n<div class=\"msg-body\">\n");
        out.push_str(&render_markdown(&msg.text));
        out.push_str("\n</div>\n</article>\n");
    }
    out.push_str("</section>\n");
    out
}

fn render_artifacts(artifacts: &[ExportArtifact]) -> String {
    if artifacts.is_empty() {
        return String::new();
    }

    let mut out =
        String::from("<section class=\"artifacts\">\n<h2>Attachments</h2>\n<div class=\"grid\">\n");
    let mut inlined_total: u64 = 0;

    for art in artifacts {
        let name = escape_attr(&art.filename);
        let size = escape_text(&human_size(art.size));

        // Re-check the budget rather than trusting the caller: `bytes` may be
        // present and oversized, and the running total is only knowable here.
        let payload = art.bytes.as_ref().filter(|bytes| {
            let len = bytes.len() as u64;
            len <= MAX_INLINE_ARTIFACT_BYTES
                && inlined_total.saturating_add(len) <= MAX_INLINE_TOTAL_BYTES
        });

        match payload {
            Some(bytes) => {
                inlined_total = inlined_total.saturating_add(bytes.len() as u64);
                let encoded = BASE64.encode(bytes);
                if is_image(&art.mime_type) {
                    // `<img>` cannot execute its payload, so the real MIME is
                    // safe to keep — the browser needs it to decode.
                    out.push_str(&format!(
                        "<figure class=\"art art--image\">\
                         <img src=\"data:{mime};base64,{data}\" alt=\"{name}\">\
                         <figcaption>{label} <span class=\"size\">{size}</span></figcaption>\
                         </figure>\n",
                        mime = escape_attr(&art.mime_type),
                        data = escape_attr(&encoded),
                        name = name,
                        label = escape_text(&art.filename),
                        size = size,
                    ));
                } else {
                    // Non-image payloads are handed out as an opaque download.
                    // The declared MIME is deliberately dropped in favour of
                    // `application/octet-stream`: it removes the whole class of
                    // `data:text/html` navigation tricks in one move, and the
                    // saved file still carries its real extension.
                    out.push_str(&format!(
                        "<a class=\"art art--file\" download=\"{name}\" \
                         href=\"data:application/octet-stream;base64,{data}\">\
                         <span class=\"icon\" aria-hidden=\"true\">↓</span>\
                         <span class=\"label\">{label}</span>\
                         <span class=\"size\">{size}</span>\
                         </a>\n",
                        name = name,
                        data = escape_attr(&encoded),
                        label = escape_text(&art.filename),
                        size = size,
                    ));
                }
            }
            None => {
                // Listed, not embedded. A silently dropped attachment is worse
                // than a visible placeholder.
                out.push_str(&format!(
                    "<div class=\"art art--omitted\">\
                     <span class=\"label\">{label}</span>\
                     <span class=\"size\">{size}</span>\
                     <span class=\"note\">not embedded</span>\
                     </div>\n",
                    label = escape_text(&art.filename),
                    size = size,
                ));
            }
        }
    }

    out.push_str("</div>\n</section>\n");
    out
}

fn is_image(mime: &str) -> bool {
    mime.to_ascii_lowercase().starts_with("image/")
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

fn human_size(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let n = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", n / 1024.0)
    } else {
        format!("{:.1} MB", n / (1024.0 * 1024.0))
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
    fn csp_meta_is_present_and_forbids_script() {
        let html = render_session_html("s", &[], &[]);
        assert!(html.contains("Content-Security-Policy"), "{html}");
        assert!(html.contains("default-src &#39;none&#39;"), "{html}");
        assert!(!html.contains("script-src"), "script-src leaked in: {html}");
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

    /// The filename reaches a `download=` attribute, so an unescaped quote
    /// there would be an attribute breakout. It ALSO reaches the visible label,
    /// where a raw quote is ordinary text and harmless — so the assertion has
    /// to name the attribute rather than scan the whole document.
    #[test]
    fn quote_in_filename_cannot_break_out_of_the_download_attribute() {
        let a = art("evil\" onclick=\"alert(1).bin", "application/zip", 4);
        let html = render_session_html("s", &[], &[a]);
        assert!(
            html.contains(r#"download="evil&quot; onclick=&quot;alert(1).bin""#),
            "attribute not escaped: {html}"
        );
        assert!(
            !html.contains(r#"download="evil" "#),
            "attribute breakout: {html}"
        );
    }

    #[test]
    fn image_is_inlined_as_a_data_uri() {
        let html = render_session_html("s", &[], &[art("shot.png", "image/png", 16)]);
        assert!(html.contains("<img src=\"data:image/png;base64,"), "{html}");
    }

    #[test]
    fn non_image_is_served_as_an_opaque_download() {
        let html = render_session_html("s", &[], &[art("r.pdf", "application/pdf", 16)]);
        assert!(html.contains("download=\"r.pdf\""), "{html}");
        assert!(
            html.contains("href=\"data:application/octet-stream;base64,"),
            "declared mime should be dropped for non-images: {html}"
        );
        assert!(!html.contains("data:application/pdf"), "{html}");
    }

    /// A `data:text/html` payload must never become a navigable destination.
    #[test]
    fn html_artifact_is_not_navigable() {
        let html = render_session_html("s", &[], &[art("page.html", "text/html", 16)]);
        assert!(!html.contains("data:text/html"), "{html}");
        assert!(html.contains("data:application/octet-stream"), "{html}");
    }

    #[test]
    fn oversize_artifact_is_listed_but_not_embedded() {
        let big = ExportArtifact {
            filename: "huge.bin".to_string(),
            mime_type: "application/octet-stream".to_string(),
            bytes: Some(vec![0u8; (MAX_INLINE_ARTIFACT_BYTES + 1) as usize]),
            size: MAX_INLINE_ARTIFACT_BYTES + 1,
        };
        let html = render_session_html("s", &[], &[big]);
        assert!(html.contains("huge.bin"), "artifact vanished: {html}");
        assert!(html.contains("not embedded"), "{html}");
        assert!(!html.contains("base64,"), "payload was inlined anyway");
    }

    #[test]
    fn total_budget_stops_inlining_after_the_ceiling() {
        let each = (MAX_INLINE_ARTIFACT_BYTES) as usize;
        let count = (MAX_INLINE_TOTAL_BYTES / MAX_INLINE_ARTIFACT_BYTES) as usize + 1;
        let arts: Vec<_> = (0..count)
            .map(|i| art(&format!("f{i}.bin"), "application/octet-stream", each))
            .collect();
        let html = render_session_html("s", &[], &arts);
        assert!(html.contains("not embedded"), "ceiling not enforced");
        // Everything is still listed, embedded or not.
        for i in 0..count {
            assert!(html.contains(&format!("f{i}.bin")), "f{i}.bin missing");
        }
    }

    #[test]
    fn artifact_with_no_bytes_is_still_listed() {
        let a = ExportArtifact {
            filename: "gone.png".to_string(),
            mime_type: "image/png".to_string(),
            bytes: None,
            size: 1234,
        };
        let html = render_session_html("s", &[], &[a]);
        assert!(html.contains("gone.png"), "{html}");
        assert!(html.contains("1.2 KB"), "{html}");
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
    fn human_size_formats_each_magnitude() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
    }
}
