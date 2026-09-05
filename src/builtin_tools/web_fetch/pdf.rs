//! PDF special-case extraction for the web fetch tool.
//!
//! When a fetched response is a PDF, the HTML pipeline would reduce the
//! binary payload to lossy UTF-8 garbage. This module extracts the embedded
//! text layer with `lopdf` instead, one `[page N]` section per page, and
//! fails honestly when there is no text layer (scanned / image-only /
//! password-protected documents) — it never falls back to parsing the
//! binary as HTML.

use reqwest::header::HeaderMap;
use tracing::debug;

use super::super::error::ToolError;

/// Maximum PDF body size in bytes (20 MB).
///
/// Larger than the HTML response budget (10 MB) because PDFs routinely
/// exceed 10 MB while carrying only a few pages of extractable text. The
/// bound is enforced twice: the fetch stream aborts at this cap, and
/// `extract_pdf` re-checks before parsing so an oversized payload surfaces
/// as an honest size error instead of a truncated half-document handed to
/// the parser.
pub(crate) const MAX_PDF_BYTES: usize = 20 * 1024 * 1024;

/// Decide whether a fetched response should take the PDF path.
///
/// `Content-Type: application/pdf` is authoritative in BOTH directions: a
/// PDF content type dispatches even without a `.pdf` URL, and a non-PDF
/// content type (e.g. an HTML error page served under a `.pdf` URL) stays
/// on the HTML path. Only a generic binary content type
/// (`application/octet-stream`) or a missing header falls back to the
/// `.pdf` URL suffix hint.
pub(crate) fn is_pdf_response(headers: &HeaderMap, url: &str) -> bool {
    let content_type = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok());
    match content_type {
        Some(ct) => {
            let mime = ct
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            match mime.as_str() {
                "application/pdf" => true,
                // Generic binary: servers frequently mislabel PDFs this
                // way, so defer to the URL hint.
                "application/octet-stream" | "binary/octet-stream" => url_suggests_pdf(url),
                _ => false,
            }
        }
        None => url_suggests_pdf(url),
    }
}

/// `.pdf` URL suffix hint (query string and fragment stripped,
/// case-insensitive).
fn url_suggests_pdf(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.to_ascii_lowercase().ends_with(".pdf")
}

/// Extract a PDF's text layer, returning `(title, body)`.
///
/// The body is the per-page text joined with `[page N]` markers. The title
/// comes from the PDF Info dictionary (the analogue of HTML `<title>`) and
/// is `None` when absent. Errors are honest and final:
///
/// * over 20 MB → size error (no truncated parse),
/// * unparseable bytes → parse error,
/// * password-protected (empty-password decrypt failed) → password error,
/// * parseable but no text layer → "scanned document" error.
pub(crate) fn extract_pdf(
    bytes: &[u8],
) -> std::result::Result<(Option<String>, String), ToolError> {
    if bytes.len() > MAX_PDF_BYTES {
        return Err(ToolError::Execution(format!(
            "PDF too large: {} bytes (max {} bytes)",
            bytes.len(),
            MAX_PDF_BYTES,
        )));
    }

    let mut doc = lopdf::Document::load_mem(bytes)
        .map_err(|e| ToolError::Execution(format!("Failed to parse PDF: {e}")))?;

    if doc.is_encrypted() {
        // Many "encrypted" PDFs in the wild use an empty user password
        // (owner-password-only protection); try that before giving up.
        doc.decrypt("").map_err(|_| {
            ToolError::Execution(
                "PDF is password-protected; cannot extract text without the password".to_string(),
            )
        })?;
    }

    let title = extract_pdf_title(&doc);

    let page_numbers: Vec<u32> = doc.get_pages().keys().copied().collect();
    if page_numbers.is_empty() {
        return Err(ToolError::Execution(
            "Failed to parse PDF: document contains no pages".to_string(),
        ));
    }

    let mut body = String::new();
    let mut any_text = false;
    for n in page_numbers {
        // One bad page must not sink the whole document — skip it and
        // keep extracting the rest.
        let text = match doc.extract_text(&[n]) {
            Ok(t) => t,
            Err(e) => {
                debug!("lopdf failed to extract page {n}: {e}");
                String::new()
            }
        };
        body.push_str(&format!("[page {n}]\n"));
        let text = text.trim();
        if !text.is_empty() {
            any_text = true;
            body.push_str(text);
        }
        body.push_str("\n\n");
    }

    if !any_text {
        return Err(ToolError::Execution(
            "PDF has no extractable text layer (likely a scanned or image-only document)"
                .to_string(),
        ));
    }

    Ok((title, body.trim_end().to_string()))
}

/// Best-effort document title from the PDF Info dictionary. Missing,
/// non-string, or blank titles yield `None`.
fn extract_pdf_title(doc: &lopdf::Document) -> Option<String> {
    let info = doc.trailer.get(b"Info").ok()?;
    let dict = match info {
        lopdf::Object::Reference(id) => doc.get_object(*id).ok()?.as_dict().ok()?,
        lopdf::Object::Dictionary(d) => d,
        _ => return None,
    };
    let title = lopdf::decode_text_string(dict.get(b"Title").ok()?).ok()?;
    let title = title.trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Document, Object, Stream};
    use reqwest::header::HeaderValue;

    /// Build a minimal multi-page PDF with a real text layer, using lopdf
    /// itself so the fixture round-trips through the same parser the
    /// pipeline uses. `None` page text produces a page with graphics but
    /// no text operators (the "scanned page" shape).
    fn build_test_pdf(title: Option<&str>, pages: &[Option<&str>]) -> Vec<u8> {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Courier",
            "Encoding" => "WinAnsiEncoding",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });

        let mut kids = Vec::new();
        for text in pages {
            let content = match text {
                Some(t) => format!("BT /F1 24 Tf 72 720 Td ({t}) Tj ET").into_bytes(),
                // Graphics-only page: a stroked rectangle, no text ops.
                None => b"0 0 595 842 re S".to_vec(),
            };
            let content_id = doc.add_object(Stream::new(dictionary! {}, content));
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
                "Contents" => content_id,
                "Resources" => resources_id,
            });
            kids.push(page_id.into());
        }

        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids,
                "Count" => pages.len() as i64,
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        if let Some(t) = title {
            let info_id = doc.add_object(dictionary! {
                "Title" => Object::string_literal(t),
            });
            doc.trailer.set("Info", info_id);
        }
        // Encryption key derivation reads the trailer ID; harmless otherwise.
        doc.trailer.set(
            "ID",
            Object::Array(vec![
                Object::string_literal(b"ABC".to_vec()),
                Object::string_literal(b"DEF".to_vec()),
            ]),
        );

        let mut buf = Vec::new();
        doc.save_to(&mut buf).expect("test fixture must serialize");
        buf
    }

    /// Encrypt an in-memory document with a non-empty user password.
    fn encrypt_document(doc: &mut Document) {
        use lopdf::{EncryptionState, EncryptionVersion, Permissions};
        let version = EncryptionVersion::V2 {
            document: doc,
            owner_password: "owner",
            user_password: "user",
            key_length: 40,
            permissions: Permissions::all(),
        };
        let state = EncryptionState::try_from(version).expect("encryption state");
        doc.encrypt(&state).expect("test fixture must encrypt");
    }

    fn headers_with_content_type(ct: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_str(ct).expect("valid header value"),
        );
        h
    }

    // ─── Dispatch decisions ─────────────────────────────────────────

    #[test]
    fn content_type_pdf_dispatches_even_without_pdf_url() {
        let h = headers_with_content_type("application/pdf");
        assert!(is_pdf_response(&h, "https://example.com/article.html"));
    }

    #[test]
    fn content_type_pdf_with_parameters_dispatches() {
        let h = headers_with_content_type("application/pdf; charset=binary");
        assert!(is_pdf_response(&h, "https://example.com/x"));
    }

    #[test]
    fn non_pdf_content_type_wins_over_pdf_url_hint() {
        // An HTML error page served under a `.pdf` URL must stay on the
        // HTML path — Content-Type is authoritative.
        let h = headers_with_content_type("text/html; charset=utf-8");
        assert!(!is_pdf_response(&h, "https://example.com/paper.pdf"));
    }

    #[test]
    fn pdf_url_hint_dispatches_when_content_type_missing() {
        let h = HeaderMap::new();
        assert!(is_pdf_response(&h, "https://example.com/paper.pdf"));
        assert!(is_pdf_response(&h, "https://example.com/PAPER.PDF"));
        assert!(is_pdf_response(&h, "https://example.com/p.pdf?dl=1#frag"));
    }

    #[test]
    fn generic_binary_content_type_falls_back_to_url_hint() {
        let h = headers_with_content_type("application/octet-stream");
        assert!(is_pdf_response(&h, "https://example.com/paper.pdf"));
        assert!(!is_pdf_response(&h, "https://example.com/archive.zip"));
    }

    #[test]
    fn non_pdf_is_not_intercepted() {
        let h = headers_with_content_type("text/html");
        assert!(!is_pdf_response(&h, "https://example.com/page.html"));
        let h = HeaderMap::new();
        assert!(!is_pdf_response(&h, "https://example.com/page.html"));
    }

    // ─── Extraction pipeline ────────────────────────────────────────

    #[test]
    fn round_trip_extracts_text_with_page_markers() {
        let pdf = build_test_pdf(
            Some("Aleph PDF Test"),
            &[Some("Hello Aleph pipeline"), Some("Second page content")],
        );
        let (title, body) = extract_pdf(&pdf).expect("fixture must extract");
        assert_eq!(title.as_deref(), Some("Aleph PDF Test"));
        assert!(body.contains("[page 1]"), "missing page 1 marker: {body}");
        assert!(body.contains("[page 2]"), "missing page 2 marker: {body}");
        assert!(body.contains("Hello Aleph pipeline"), "page 1 text: {body}");
        assert!(body.contains("Second page content"), "page 2 text: {body}");
    }

    #[test]
    fn missing_title_yields_none_but_text_extracts() {
        let pdf = build_test_pdf(None, &[Some("Body text here")]);
        let (title, body) = extract_pdf(&pdf).expect("fixture must extract");
        assert_eq!(title, None);
        assert!(body.contains("Body text here"));
    }

    #[test]
    fn oversized_pdf_is_rejected_before_parsing() {
        let bytes = vec![0u8; MAX_PDF_BYTES + 1];
        let err = extract_pdf(&bytes).expect_err("oversized PDF must fail");
        let msg = err.to_string();
        assert!(msg.contains("PDF too large"), "unexpected error: {msg}");
        assert!(msg.contains("20971520"), "error must state the cap: {msg}");
    }

    #[test]
    fn scanned_pdf_without_text_layer_fails_honestly() {
        let pdf = build_test_pdf(None, &[None, None]);
        let err = extract_pdf(&pdf).expect_err("textless PDF must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("no extractable text layer"),
            "unexpected error: {msg}"
        );
        assert!(msg.contains("scanned"), "error should hint at scan: {msg}");
    }

    #[test]
    fn password_protected_pdf_fails_honestly() {
        let mut doc = Document::load_mem(&build_test_pdf(None, &[Some("secret text")]))
            .expect("fixture must parse");
        encrypt_document(&mut doc);
        let mut buf = Vec::new();
        doc.save_to(&mut buf)
            .expect("encrypted fixture must serialize");

        let err = extract_pdf(&buf).expect_err("encrypted PDF must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("password-protected"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn garbage_bytes_fail_with_parse_error() {
        let err = extract_pdf(b"this is not a pdf at all").expect_err("garbage must fail");
        assert!(
            err.to_string().contains("Failed to parse PDF"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn partially_textless_document_keeps_available_pages() {
        // Page 2 is graphics-only; page 1 text must still come through
        // (single bad page does not sink the document).
        let pdf = build_test_pdf(None, &[Some("kept text"), None]);
        let (_title, body) = extract_pdf(&pdf).expect("partial text must extract");
        assert!(body.contains("kept text"));
        assert!(body.contains("[page 2]"), "page marker kept: {body}");
    }
}
