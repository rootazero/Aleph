//! Fetch by intent: a page too large to return whole comes back as the
//! sections that match the caller's `prompt`, plus a handle to the rest.
//!
//! Pure retrieval (R7): the page is indexed into the store `ctx_search` reads,
//! and the sections are ranked by BM25 against the words the model wrote —
//! nothing here decides what the model needs beyond that ranking. When the
//! ranking misses, the model searches the same index with other words through
//! `ctx_search`, so there is one index, not a second one owned by this tool.

use crate::security::content_sanitizer::{
    sanitize_external_text, wrap_external_content, ContentSource,
};
use crate::tools::result_store::{source_label, ToolResultStore};

/// Hits fetched before keeping only this page's own: the search spans every
/// output the session has indexed.
const OVERFETCH_HITS: usize = 40;

/// Between two non-adjacent sections, so a reader does not take the text as
/// continuous.
const GAP: &str = "\n[…]\n";

/// `page` — the whole extracted page, raw — as the sections matching `prompt`
/// in page order, at most `cap_chars` of them, fenced as external content and
/// followed by the offload footer (the persisted path, and how to search the
/// rest). With no section matching, the head of the page stands in, so the
/// model still sees what the page is.
///
/// `None` when the store cannot take the page (no store, a write failure); the
/// caller then truncates the page as it did before.
pub(super) fn sections_for_prompt(
    store: &ToolResultStore,
    call_id: &str,
    url: &str,
    prompt: &str,
    page: &str,
    cap_chars: usize,
) -> Option<String> {
    let source = || ContentSource::WebFetch {
        url: url.to_string(),
    };
    // Stored fenced, like any offloaded web page: the footer then carries no
    // preview of the untrusted text outside a fence.
    let (footer, _) = crate::tools::result_processing::recovery_footer(
        Some(store),
        call_id,
        super::WebFetchTool::NAME,
        &wrap_external_content(page, source()),
        0,
    )?;
    let label = source_label(super::WebFetchTool::NAME, call_id);
    let mut picked: Vec<(i64, String)> = Vec::new();
    let mut used = 0usize;
    for hit in store.search(prompt, OVERFETCH_HITS) {
        if hit.source != label {
            continue;
        }
        let text = sanitize_external_text(&hit.body);
        let len = text.chars().count() + GAP.len();
        if used + len > cap_chars {
            continue;
        }
        used += len;
        picked.push((hit.chunk_no, text));
    }
    let matched = picked.len();
    let body = if picked.is_empty() {
        crate::utils::text_format::truncate_chars(&sanitize_external_text(page), cap_chars)
            .to_string()
    } else {
        picked.sort_by_key(|(chunk, _)| *chunk);
        picked
            .into_iter()
            .map(|(_, text)| text)
            .collect::<Vec<_>>()
            .join(GAP)
    };
    let note = if matched == 0 {
        format!(
            "[Page too large to return whole ({} chars) and no section matched the focus; \
             showing its beginning.]",
            page.chars().count()
        )
    } else {
        format!(
            "[Page too large to return whole ({} chars): showing the {matched} section(s) \
             matching the focus, in page order.]",
            page.chars().count()
        )
    };
    Some(format!(
        "{note}\n{}\n{footer}",
        wrap_external_content(&body, source())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, std::sync::Arc<ToolResultStore>) {
        let dir = tempfile::tempdir().unwrap();
        let store = ToolResultStore::with_dir_for_tests(dir.path().join("results"));
        let store = ToolResultStore::for_session(&std::sync::Arc::new(store), "test:intent");
        (dir, store)
    }

    fn long_page() -> String {
        let mut page = String::new();
        for i in 0..400 {
            page.push_str(&format!(
                "Paragraph {i} is about the weather in general and nothing else.\n"
            ));
            if i == 311 {
                page.push_str("The flux capacitor requires 1.21 gigawatts to operate.\n");
            }
        }
        page
    }

    /// The matching section comes back, not the page's head, within the cap,
    /// fenced, with the handle to the whole page.
    ///
    /// Mutation-checked: dropping the search (always the head) turns this red.
    #[test]
    fn the_section_matching_the_focus_comes_back_with_a_handle() {
        let (_dir, store) = store();
        let page = long_page();
        let out = sections_for_prompt(
            &store,
            "call_intent_1",
            "https://example.com/p",
            "flux capacitor gigawatts",
            &page,
            4_000,
        )
        .expect("the store takes the page");
        assert!(out.contains("1.21 gigawatts"), "{out}");
        assert!(
            !out.contains("Paragraph 0 is"),
            "the head is not the match: {out}"
        );
        assert!(out.contains("[Full output persisted: "), "{out}");
        assert!(out.contains("matching the focus"), "{out}");
        let fenced = out
            .split("[Full output persisted: ")
            .next()
            .expect("a head");
        assert!(
            fenced.chars().count() < 4_000 + 400,
            "{}",
            fenced.chars().count()
        );
    }

    /// Nothing matches: the head stands in, and says so.
    #[test]
    fn no_match_shows_the_beginning_and_says_so() {
        let (_dir, store) = store();
        let out = sections_for_prompt(
            &store,
            "call_intent_2",
            "https://example.com/p",
            "zyxwvut",
            &long_page(),
            2_000,
        )
        .expect("the store takes the page");
        assert!(out.contains("no section matched"), "{out}");
        assert!(out.contains("Paragraph 0 is"), "{out}");
    }
}
