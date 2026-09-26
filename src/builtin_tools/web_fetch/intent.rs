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

/// A page persisted (and, when the index took it, indexed) by [`index_page`].
pub(super) struct IndexedPage {
    /// The offload footer: the persisted path, and how to read the rest back.
    pub(super) footer: String,
    /// Whether the page's sections are in the index. `false`: a search over
    /// them has nothing to find, which is not the same as nothing matching.
    pub(super) indexed: bool,
}

/// Persist and index `page` — the whole extracted page, raw — under this call.
/// Done once per call: the blob is named by the call id, so the page is written
/// exactly once however many times [`sections_for_prompt`] is asked to fit a
/// smaller cap.
///
/// `None` when the store cannot take the page (a write failure, or no retrieval
/// tool callable to read it back); the caller then truncates the page as it
/// did before.
pub(super) fn index_page(
    store: &ToolResultStore,
    call_id: &str,
    url: &str,
    page: &str,
) -> Option<IndexedPage> {
    // Stored fenced, like any offloaded web page: the footer then carries no
    // preview of the untrusted text outside a fence.
    // The footer names only the retrieval tools the dispatch can call; outside
    // a dispatch nothing is known about the gates, and both are assumed.
    let recovery = crate::tools::result_processing::dispatch_recovery_tools()
        .unwrap_or(crate::tools::result_processing::RecoveryTools::ALL);
    let offloaded = crate::tools::result_processing::offload_indexed(
        Some(store),
        call_id,
        super::WebFetchTool::NAME,
        &wrap_external_content(page, source_of(url)),
        0,
        recovery,
    )?;
    Some(IndexedPage {
        footer: offloaded.footer,
        indexed: offloaded.sections.is_some_and(|n| n > 0),
    })
}

/// The sections of `page` (persisted by [`index_page`] under `call_id`) that
/// match `prompt`, in page order, at most `cap_chars` of them, fenced as
/// external content and followed by the footer. With no section matching, the
/// head of the page stands in, so the model still sees what the page is — and
/// the note says which of two things happened: nothing matched, or the page
/// could not be searched at all.
pub(super) fn sections_for_prompt(
    store: &ToolResultStore,
    call_id: &str,
    url: &str,
    prompt: &str,
    page: &str,
    indexed: &IndexedPage,
    cap_chars: usize,
) -> String {
    let label = source_label(super::WebFetchTool::NAME, call_id);
    // `None`: the page is not in the index, or the index could not answer.
    let hits = if indexed.indexed {
        store.search_source(&label, prompt, CANDIDATE_SECTIONS)
    } else {
        None
    };
    let searchable = hits.is_some();
    let mut picked: Vec<(i64, String)> = Vec::new();
    let mut used = 0usize;
    for hit in hits.unwrap_or_default() {
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
    let chars = page.chars().count();
    let note = if !searchable {
        format!(
            "[Page too large to return whole ({chars} chars), and it could not be searched \
             here (the index did not take it); showing its beginning. The whole page is \
             saved at the path below.]"
        )
    } else if matched == 0 {
        format!(
            "[Page too large to return whole ({chars} chars) and no section matched the focus; \
             showing its beginning.]"
        )
    } else {
        format!(
            "[Page too large to return whole ({chars} chars): showing the {matched} section(s) \
             matching the focus, in page order.]"
        )
    };
    format!(
        "{note}\n{}\n{}",
        wrap_external_content(&body, source_of(url)),
        indexed.footer
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
        let indexed = index_page(store, call_id, url, page)?;
        Some(sections_for_prompt(
            store, call_id, url, prompt, page, &indexed, cap_chars,
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

    /// The index cannot take the page (its database cannot open): the page is
    /// still persisted, and the note says it could not be searched — not that
    /// nothing matched, which only a working index can say.
    ///
    /// Mutation-checked: reading an unavailable index as an empty result
    /// ("no section matched") turns this red.
    #[test]
    fn an_unavailable_index_is_not_reported_as_no_match() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("results");
        // A directory where the index database file should be: it cannot open.
        std::fs::create_dir_all(root.join("index.db")).unwrap();
        let store = std::sync::Arc::new(ToolResultStore::with_dir_for_tests(root));
        let out = by_intent(
            &store,
            "call_intent_5",
            "flux capacitor",
            &long_page(),
            2_000,
        )
        .expect("the page is still persisted");
        assert!(out.contains("could not be searched"), "{out}");
        assert!(!out.contains("no section matched"), "{out}");
        assert!(out.contains("[Full output persisted: "), "{out}");
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
