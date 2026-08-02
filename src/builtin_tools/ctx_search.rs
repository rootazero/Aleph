//! `CtxSearchTool` — BM25 retrieval over offloaded tool output.
//!
//! Large tool results (build logs, big greps, web fetches) are evicted from
//! the context window to disk by [`ToolResultStore`](crate::tools::result_store)
//! and indexed into a per-session FTS5 store. This tool lets the model pull
//! back *only the relevant sections* via a keyword query, instead of
//! re-reading the whole file with `read_file` (which would defeat the offload).
//!
//! Read-only and side-effect-free, so it is safe under parallel dispatch.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{notify_tool_result, notify_tool_start};
use crate::error::Result;
use crate::tools::result_store::{global_tool_result_store, ToolResultStore};
use crate::tools::turn_context::current_session_key;
use crate::tools::AlephTool;

/// Default number of sections returned when `limit` is omitted.
const DEFAULT_LIMIT: usize = 5;
/// Hard cap on returned sections, to keep the result compact.
const MAX_LIMIT: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CtxSearchArgs {
    /// Keywords describing what to find in previously-offloaded tool output
    /// (e.g. "failing test assertion", "database timeout error",
    /// "function definition parseConfig"). Natural-language phrasing is fine;
    /// punctuation is ignored.
    pub query: String,
    /// Maximum number of matching sections to return. Defaults to 5, capped
    /// at 20.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One matching section from the offloaded-output index.
#[derive(Debug, Serialize)]
pub struct CtxSearchHit {
    /// The tool call the section came from (e.g. the originating tool name).
    pub source: String,
    /// Zero-based section ordinal within its source.
    pub section: i64,
    /// Section title (its first meaningful line).
    pub title: String,
    /// A short excerpt of the section body around the match.
    pub excerpt: String,
}

#[derive(Debug, Serialize)]
pub struct CtxSearchOutput {
    /// Echo of the query that was run.
    pub query: String,
    /// Number of sections returned in `hits`.
    pub matched: usize,
    /// Total sections currently indexed across all offloaded outputs.
    pub indexed_sections: usize,
    /// The matching sections, most relevant first.
    pub hits: Vec<CtxSearchHit>,
    /// Human-facing note when there are no hits (empty index vs no match).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Builtin tool exposing BM25 search over offloaded tool output.
#[derive(Clone, Default)]
pub struct CtxSearchTool;

impl CtxSearchTool {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AlephTool for CtxSearchTool {
    const NAME: &'static str = "ctx_search";
    const DESCRIPTION: &'static str = "Search large tool outputs that were offloaded out of the context window (build logs, big greps, web fetches). When a tool result shows '[Full output persisted: … Indexed N sections …]', call ctx_search with keywords to retrieve only the relevant sections instead of re-reading the whole file. Returns the best-matching sections ranked by relevance.";

    type Args = CtxSearchArgs;
    type Output = CtxSearchOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        notify_tool_start(Self::NAME, &args.query);
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

        let Some(store) = global_tool_result_store() else {
            let note = "No offloaded tool output is indexed in this session yet.".to_string();
            notify_tool_result(Self::NAME, &note, false);
            return Ok(CtxSearchOutput {
                query: args.query,
                matched: 0,
                indexed_sections: 0,
                hits: Vec::new(),
                note: Some(note),
            });
        };
        // The installed store is the process-wide, unscoped handle. Narrow it to
        // the session running this tool — the same key the two *write* seams
        // (`build_request_tool_service`, the harness bridge) scoped their handles
        // with, so the search looks where the offload actually landed. Reading
        // the raw handle here would search the empty unscoped scope and silently
        // return zero hits; searching it *unfiltered* would surface another
        // agent's tool output. `None` (direct call outside a dispatched turn:
        // tests, non-gateway paths) keeps the unscoped handle.
        let store = match current_session_key() {
            Some(session) => ToolResultStore::for_session(&store, session),
            None => store,
        };

        let indexed_sections = store.indexed_sections();
        let hits: Vec<CtxSearchHit> = store
            .search(&args.query, limit)
            .into_iter()
            .map(|h| CtxSearchHit {
                source: h.source,
                section: h.chunk_no,
                title: h.title,
                excerpt: h.snippet,
            })
            .collect();

        let note = if hits.is_empty() {
            Some(if indexed_sections == 0 {
                "No offloaded tool output is indexed yet — nothing to search.".to_string()
            } else {
                format!(
                    "No sections matched '{}' across {indexed_sections} indexed sections; \
                     try different keywords.",
                    args.query
                )
            })
        } else {
            None
        };

        notify_tool_result(
            Self::NAME,
            &format!("{} match(es) of {indexed_sections} indexed", hits.len()),
            true,
        );

        Ok(CtxSearchOutput {
            query: args.query,
            matched: hits.len(),
            indexed_sections,
            hits,
            note,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_note_when_no_store_installed() {
        // In unit-test context the global store is typically uninstalled.
        // The tool must degrade gracefully rather than error.
        let tool = CtxSearchTool::new();
        let out = tool
            .call(CtxSearchArgs {
                query: "anything".to_string(),
                limit: None,
            })
            .await
            .expect("call should not error");
        // Either no store (note set, 0 hits) or a store from another test —
        // in both cases the call must succeed and never panic.
        assert_eq!(out.query, "anything");
        assert!(out.matched <= DEFAULT_LIMIT);
    }

    #[test]
    fn limit_is_clamped() {
        assert_eq!(1usize.clamp(1, MAX_LIMIT), 1);
        assert_eq!(999usize.clamp(1, MAX_LIMIT), MAX_LIMIT);
    }
}
