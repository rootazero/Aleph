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

/// Sections of the page ranked against the prompt: the candidates the cap
/// then picks from. The search is over this page's own sections only.
const CANDIDATE_SECTIONS: usize = 40;

/// Between two non-adjacent sections, so a reader does not take the text as
/// continuous.
const GAP: &str = "\n[…]\n";

/// Persist and index `page` — the whole extracted page, raw — under this call,
/// returning the offload footer (the persisted path, and how to search the
/// rest). Done once per call: the blob is named by the call id, so the page is
/// written exactly once however many times [`sections_for_prompt`] is asked to
/// fit a smaller cap.
///
/// `None` when the store cannot take the page (a write failure); the caller
/// then truncates the page as it did before.
pub(super) fn index_page(
    store: &ToolResultStore,
    call_id: &str,
    url: &str,
    page: &str,
) -> Option<String> {
    // Stored fenced, like any offloaded web page: the footer then carries no
    // preview of the untrusted text outside a fence.
    // The footer names only the retrieval tools the dispatch can call; outside
    // a dispatch nothing is known about the gates, and both are assumed.
    let recovery = crate::tools::result_processing::dispatch_recovery_tools()
        .unwrap_or(crate::tools::result_processing::RecoveryTools::ALL);
    let (footer, _) = crate::tools::result_processing::recovery_footer_for(
        Some(store),
        call_id,
        super::WebFetchTool::NAME,
        &wrap_external_content(page, source_of(url)),
        0,
        recovery,
    )?;
    Some(footer)
}

/// The sections of `page` (indexed by [`index_page`] under `call_id`) that
/// match `prompt`, in page order, at most `cap_chars` of them, fenced as
/// external content and followed by `footer`. With no section matching, the
/// head of the page stands in, so the model still sees what the page is.
pub(super) fn sections_for_prompt(
    store: &ToolResultStore,
    call_id: &str,
    url: &str,
    prompt: &str,
    page: &str,
    footer: &str,
    cap_chars: usize,
) -> String {
    let label = source_label(super::WebFetchTool::NAME, call_id);
    let mut picked: Vec<(i64, String)> = Vec::new();
    let mut used = 0usize;
    for hit in store.search_source(&label, prompt, CANDIDATE_SECTIONS) {
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
    format!(
        "{note}\n{}\n{footer}",
        wrap_external_content(&body, source_of(url))
    )
}

fn source_of(url: &str) -> ContentSource {
    ContentSource::WebFetch {
        url: url.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, std::sync::Arc<ToolResultStore>) {
        let dir = tempfile::tempdir().unwrap();
        // Created up front: the index opens once, on first use, and a missing
        // root disables it for the store's lifetime.
        std::fs::create_dir_all(dir.path().join("results")).unwrap();
        let store = ToolResultStore::with_dir_for_tests(dir.path().join("results"));
        let store = ToolResultStore::for_session(&std::sync::Arc::new(store), "test:intent");
        (dir, store)
    }

    /// Index the page under `call_id`, then pick its sections for `prompt`.
    fn by_intent(
        store: &ToolResultStore,
        call_id: &str,
        prompt: &str,
        page: &str,
        cap_chars: usize,
    ) -> Option<String> {
        let url = "https://example.com/p";
        let footer = index_page(store, call_id, url, page)?;
        Some(sections_for_prompt(
            store, call_id, url, prompt, page, &footer, cap_chars,
        ))
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
        let out = by_intent(
            &store,
            "call_intent_1",
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

    /// The page's own sections are ranked among themselves: a session whose
    /// other outputs match the prompt better still gets the page's match.
    ///
    /// Mutation-checked: searching the pooled session index and keeping the
    /// page's hits afterwards (the old shape) turns this red.
    #[test]
    fn the_pages_own_match_survives_better_matches_elsewhere_in_the_session() {
        let (_dir, store) = store();
        let noisy: String = (0..2_000)
            .map(|i| format!("flux capacitor flux capacitor {i}\n"))
            .collect();
        store
            .index_output("call_other_output", "bash", &noisy)
            .expect("the other output is indexed");
        let out = by_intent(
            &store,
            "call_intent_4",
            "flux capacitor",
            &long_page(),
            4_000,
        )
        .expect("the store takes the page");
        assert!(out.contains("1.21 gigawatts"), "{out}");
    }

    /// Nothing matches: the head stands in, and says so.
    #[test]
    fn no_match_shows_the_beginning_and_says_so() {
        let (_dir, store) = store();
        let out = by_intent(&store, "call_intent_2", "zyxwvut", &long_page(), 2_000)
            .expect("the store takes the page");
        assert!(out.contains("no section matched"), "{out}");
        assert!(out.contains("Paragraph 0 is"), "{out}");
    }

    /// The footer names only what the dispatch can call: here `file_read`,
    /// not `ctx_search` — a subagent given `web_fetch` without `ctx_search`
    /// must not be handed a search it cannot run.
    ///
    /// Mutation-checked: footing with `RecoveryTools::ALL` regardless turns
    /// this red.
    #[tokio::test]
    async fn the_footer_names_only_the_retrieval_tools_the_dispatch_can_call() {
        let (_dir, store) = store();
        let only_read = crate::tools::result_processing::RecoveryTools {
            ctx_search: false,
            file_read: true,
        };
        let out = crate::tools::result_processing::with_recovery_tools(only_read, async {
            by_intent(
                &store,
                "call_intent_3",
                "flux capacitor",
                &long_page(),
                2_000,
            )
        })
        .await
        .expect("the store takes the page");
        let footer = out
            .split("[Full output persisted: ")
            .nth(1)
            .expect("a footer");
        assert!(!footer.contains("ctx_search"), "{footer}");
        assert!(footer.contains("file_read"), "{footer}");
    }
}
