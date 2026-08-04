//! Rewriting text that carries an untrusted-content boundary.
//!
//! Every ingress stage that shortens a tool's text field can be handed a fenced
//! payload — `web_fetch` fences the page it returns, the browser tools fence
//! scraped content, and the MCP adapter fences each text block a server sends.
//! The boundary markers are **structure, not content**: they are what tells the
//! model where the untrusted region starts and stops, and a rewrite that drops
//! them (or keeps only the opening one) is strictly worse than no rewrite at
//! all.
//!
//! Both shortening stages therefore go through [`rewrite_interior`] rather than
//! replacing a field wholesale.

use crate::security::content_sanitizer::split_external_fence;

/// Apply `rewrite` to `text`, or — when `text` is a well-formed fenced payload —
/// to its interior only, re-emitting the markers verbatim.
///
/// `rewrite` returning `None` means "I declined", and propagates: the caller
/// leaves the field alone. Unbalanced or multi-fence text is not split (see
/// [`split_external_fence`]), so it is passed through as ordinary content —
/// which is safe, because nothing there is a boundary this function could break.
pub(super) fn rewrite_interior<F>(text: &str, rewrite: F) -> Option<String>
where
    F: FnOnce(&str) -> Option<String>,
{
    match split_external_fence(text) {
        Some(fence) => rewrite(fence.interior).map(|body| fence.rewrap(&body)),
        None => rewrite(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::content_sanitizer::{wrap_external_content, ContentSource};

    fn fenced(body: &str) -> String {
        wrap_external_content(body, ContentSource::BrowserContent)
    }

    #[test]
    fn the_rewrite_sees_the_interior_and_the_markers_come_back() {
        let text = fenced("line one\nline two\nline three");
        let out = rewrite_interior(&text, |interior| {
            assert_eq!(interior, "line one\nline two\nline three");
            Some("line one\n… (2 lines omitted) …".to_string())
        })
        .expect("rewrite accepted");

        let split = split_external_fence(&out).expect("the fence must be intact");
        assert_eq!(split.interior, "line one\n… (2 lines omitted) …");
    }

    #[test]
    fn declining_leaves_the_caller_nothing_to_write() {
        let text = fenced("body");
        assert!(rewrite_interior(&text, |_| None).is_none());
    }

    /// `web_fetch` prepends its `[fetch_focus: …]` line ahead of the fence when
    /// the caller asked a question about the page. Insisting the fence start at
    /// byte 0 would decline exactly those results — and go back to destroying
    /// their markers.
    #[test]
    fn text_outside_the_fence_is_preserved_and_not_rewritten() {
        let text = format!("[fetch_focus: pricing]\n\n{}\ntrailer", fenced("a\nb\nc"));
        let out = rewrite_interior(&text, |interior| {
            assert_eq!(
                interior, "a\nb\nc",
                "only the untrusted region is rewritten"
            );
            Some("a\n… (2 lines omitted) …".to_string())
        })
        .expect("rewrite accepted");

        assert!(out.starts_with("[fetch_focus: pricing]\n\n"), "got: {out}");
        assert!(out.ends_with("\ntrailer"), "got: {out}");
        let split = split_external_fence(&out).expect("the fence must be intact");
        assert_eq!(split.interior, "a\n… (2 lines omitted) …");
    }

    /// An unchanged interior must round-trip byte-for-byte, or every no-op pass
    /// silently rewrites the field and re-keys the provider's prefix cache.
    #[test]
    fn an_unchanged_interior_round_trips_exactly() {
        for text in [
            fenced("body"),
            format!("[fetch_focus: q]\n\n{}", fenced("body")),
            format!("{}\ntrailer", fenced("body")),
            fenced(""),
        ] {
            let split = split_external_fence(&text).expect("well-formed");
            assert_eq!(split.rewrap(split.interior), text);
        }
    }

    #[test]
    fn unfenced_text_is_rewritten_whole() {
        let out =
            rewrite_interior("plain body", |t| Some(format!("[{t}]"))).expect("rewrite accepted");
        assert_eq!(out, "[plain body]");
    }

    /// Two concatenated fences are not one fence. Re-stitching a single pair of
    /// markers around a rewritten blob would silently move the boundary.
    #[test]
    fn two_fences_are_treated_as_ordinary_content() {
        let text = format!("{}\n{}", fenced("a"), fenced("b"));
        let out = rewrite_interior(&text, |t| {
            assert!(t.contains("<<<END_EXTERNAL_UNTRUSTED_CONTENT"));
            Some(t.to_string())
        })
        .expect("rewrite accepted");
        assert_eq!(out, text);
    }
}
