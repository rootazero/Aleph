//! The pieces both exported documents are built from.
//!
//! Two documents leave this module's parent: a conversation transcript
//! ([`super::session_html`]) and a published deliverable
//! ([`super::document`]). They differ only in what goes in the middle — the
//! shell, the page header, the attachment grid and the byte budget are the
//! same, and must stay the same. The CSP in particular is a guarantee about
//! *every* document Aleph serves from its own origin, so it exists once here
//! rather than once per renderer where a later edit could quietly weaken one
//! of them.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;

use super::markdown::{escape_attr, escape_text};
use super::{ExportArtifact, MAX_INLINE_ARTIFACT_BYTES, MAX_INLINE_TOTAL_BYTES};

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
/// `img-src data:` and not `https:` is the other half: an exported document is
/// meant to be readable offline, and a remote image would both break that and
/// tell whoever the URL names when and from where the document was opened.
const CSP: &str = "default-src 'none'; img-src data:; style-src 'unsafe-inline'";

/// Wrap a rendered `<body>` fragment in the shared document shell.
pub(super) fn wrap_document(title: &str, body: &str) -> String {
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

/// The page header: document title over one line of provenance.
pub(super) fn render_page_head(title: &str, meta: &str) -> String {
    format!(
        "<header class=\"page-head\">\n<h1>{title}</h1>\n<p class=\"page-meta\">{meta}</p>\n</header>\n",
        title = escape_text(title),
        meta = escape_text(meta),
    )
}

/// Render the attachment grid, inlining what fits the byte budget.
///
/// The budget is enforced here rather than trusted from the caller: an artifact
/// over [`MAX_INLINE_ARTIFACT_BYTES`], or one that would push the running total
/// past [`MAX_INLINE_TOTAL_BYTES`], is listed without its payload instead of
/// being inlined. `heading` names the section, because "Attachments" is right
/// for a transcript and wrong for a report's own figures.
pub(super) fn render_artifacts(heading: &str, artifacts: &[ExportArtifact]) -> String {
    if artifacts.is_empty() {
        return String::new();
    }

    let mut out = format!(
        "<section class=\"artifacts\">\n<h2>{}</h2>\n<div class=\"grid\">\n",
        escape_text(heading)
    );
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

/// Byte count as the rest of the product spells it (KB/MB, one decimal).
pub(super) fn human_size(bytes: u64) -> String {
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

    fn art(name: &str, mime: &str, len: usize) -> ExportArtifact {
        ExportArtifact {
            filename: name.to_string(),
            mime_type: mime.to_string(),
            bytes: Some(vec![0u8; len]),
            size: len as u64,
        }
    }

    #[test]
    fn the_shell_declares_a_script_free_policy() {
        let html = wrap_document("s", "<p>x</p>");
        assert!(html.starts_with("<!DOCTYPE html>"), "{html}");
        assert!(html.contains("Content-Security-Policy"), "{html}");
        assert!(html.contains("default-src &#39;none&#39;"), "{html}");
        assert!(!html.contains("script-src"), "script-src leaked in: {html}");
        assert!(html.ends_with("</html>\n"), "{html}");
    }

    #[test]
    fn the_shell_escapes_the_title_in_both_places_it_appears() {
        let html = wrap_document("<b>x</b>", "");
        assert!(!html.contains("<b>x</b>"), "{html}");
        assert_eq!(html.matches("&lt;b&gt;x&lt;/b&gt;").count(), 1, "{html}");
    }

    #[test]
    fn an_empty_artifact_list_renders_no_section() {
        assert!(render_artifacts("Attachments", &[]).is_empty());
    }

    #[test]
    fn image_is_inlined_as_a_data_uri() {
        let html = render_artifacts("Attachments", &[art("shot.png", "image/png", 16)]);
        assert!(html.contains("<img src=\"data:image/png;base64,"), "{html}");
    }

    #[test]
    fn non_image_is_served_as_an_opaque_download() {
        let html = render_artifacts("Attachments", &[art("r.pdf", "application/pdf", 16)]);
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
        let html = render_artifacts("Attachments", &[art("page.html", "text/html", 16)]);
        assert!(!html.contains("data:text/html"), "{html}");
        assert!(html.contains("data:application/octet-stream"), "{html}");
    }

    /// The filename reaches a `download=` attribute, so an unescaped quote
    /// there would be an attribute breakout. It ALSO reaches the visible label,
    /// where a raw quote is ordinary text and harmless — so the assertion has
    /// to name the attribute rather than scan the whole document.
    #[test]
    fn quote_in_filename_cannot_break_out_of_the_download_attribute() {
        let html = render_artifacts(
            "Attachments",
            &[art("evil\" onclick=\"alert(1).bin", "application/zip", 4)],
        );
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
    fn oversize_artifact_is_listed_but_not_embedded() {
        let big = ExportArtifact {
            filename: "huge.bin".to_string(),
            mime_type: "application/octet-stream".to_string(),
            bytes: Some(vec![0u8; (MAX_INLINE_ARTIFACT_BYTES + 1) as usize]),
            size: MAX_INLINE_ARTIFACT_BYTES + 1,
        };
        let html = render_artifacts("Attachments", &[big]);
        assert!(html.contains("huge.bin"), "artifact vanished: {html}");
        assert!(html.contains("not embedded"), "{html}");
        assert!(!html.contains("base64,"), "payload was inlined anyway");
    }

    #[test]
    fn total_budget_stops_inlining_after_the_ceiling() {
        let each = MAX_INLINE_ARTIFACT_BYTES as usize;
        let count = (MAX_INLINE_TOTAL_BYTES / MAX_INLINE_ARTIFACT_BYTES) as usize + 1;
        let arts: Vec<_> = (0..count)
            .map(|i| art(&format!("f{i}.bin"), "application/octet-stream", each))
            .collect();
        let html = render_artifacts("Attachments", &arts);
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
        let html = render_artifacts("Attachments", &[a]);
        assert!(html.contains("gone.png"), "{html}");
        assert!(html.contains("1.2 KB"), "{html}");
    }

    #[test]
    fn the_section_heading_is_escaped() {
        let html = render_artifacts("<b>Figures</b>", &[art("a.png", "image/png", 4)]);
        assert!(html.contains("&lt;b&gt;Figures&lt;/b&gt;"), "{html}");
        assert!(!html.contains("<b>Figures</b>"), "{html}");
    }

    #[test]
    fn human_size_formats_each_magnitude() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 MB");
    }
}
