//! The one predicate deciding whether a URL from data may become an `href`.
//!
//! # Why this is not a helper each page keeps
//!
//! Three surfaces render vendor-supplied links: the skills page (a skill's
//! homepage), the generation preset setup panel (`signup_url` / `homepage`) and
//! the chat provider detail panel. The skills page has always screened the
//! scheme — `javascript:` in an `href` is script execution — and wrote the
//! screen inline; the provider panel rendered its "Get a key" link with no
//! screen at all. Two answers to "is this safe to click" is one too many, and
//! the unscreened one is the answer that matters.
//!
//! Today every value reaching here is a `&'static str` from a preset table, so
//! the missing screen was not exploitable. That is a fact about the callers,
//! not about the function — and a preset table is exactly the kind of thing
//! that grows an operator-writable override.
//!
//! Non-http(s) values are not dropped: they are rendered as plain text, because
//! a vendor that published a `mailto:` support address should still be readable.

use leptos::prelude::*;

/// True when this URL may be used as an `href`.
///
/// `http` / `https` only. Everything else — `javascript:`, `data:`, `file:`,
/// and every scheme not yet invented — is text.
#[must_use]
pub fn is_safe_external_url(url: &str) -> bool {
    let lower = url.trim().to_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Render `label` as a link to `href`, or as plain text when the scheme is not
/// one a click may follow.
///
/// `class` is the caller's: a footnote link and a call-to-action are the same
/// safety question and different typography (R4 — clients choose wording and
/// colour, never whether something is allowed).
#[must_use]
pub fn safe_external_link(href: &str, class: &'static str, label: impl IntoView) -> AnyView {
    if is_safe_external_url(href) {
        view! {
            <a href=href.to_string() target="_blank" rel="noopener noreferrer" class=class>
                {label}
            </a>
        }
        .into_any()
    } else {
        view! { <span class=class>{label}</span> }.into_any()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_clickable_schemes_become_links() {
        assert!(is_safe_external_url("https://platform.openai.com/api-keys"));
        assert!(is_safe_external_url("http://localhost:11434"));
        // Case and surrounding space are the two cheap ways past a naive check.
        assert!(is_safe_external_url("  HTTPS://example.com  "));
    }

    #[test]
    fn a_script_url_is_not_a_link() {
        for hostile in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "  javascript:alert(1)",
            "data:text/html;base64,PHNjcmlwdD4=",
            "file:///etc/passwd",
            "vbscript:msgbox(1)",
        ] {
            assert!(
                !is_safe_external_url(hostile),
                "{hostile} must not become an href"
            );
        }
    }

    /// A vendor's `mailto:` support address is still worth reading, so the
    /// rejection is "not clickable", not "not shown".
    #[test]
    fn an_unclickable_url_is_still_readable() {
        assert!(!is_safe_external_url("mailto:support@example.com"));
    }
}
