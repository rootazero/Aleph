//! `RecallEventsTool` — BM25 retrieval over the current session's event log.
//!
//! Aleph records every message, tool call, tool result, and error of a session
//! in the `session_events` `SQLite` log. Compaction evicts old turns from the
//! *context window* once it grows large, but those events remain on disk. This
//! tool makes them BM25-searchable, so after a compaction the model can recover
//! *what it did* earlier this session — the file it was editing, the command
//! that failed, the path it took — by retrieving only the relevant slices
//! instead of asking the user to repeat.
//!
//! # Relationship to the sibling search tools
//!
//! Aleph has three complementary retrieval tools; each searches a different
//! store, so their descriptions are kept sharp to avoid tool-choice confusion:
//!
//! - [`ctx_search`](crate::builtin_tools::ctx_search) — large **tool output**
//!   that was offloaded out of context (build logs, big greps).
//! - [`session_search`](crate::builtin_tools::session_search) — **past
//!   conversations across sessions** (memory summaries + transcripts).
//! - `recall_events` (this tool) — **this session's own activity timeline**
//!   (tool actions, results, errors) dropped from context by compaction.
//!
//! Read-only and side-effect-free, so it is safe under parallel dispatch.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{notify_tool_result, notify_tool_start};
use crate::error::Result;
use crate::session::store::global_session_event_store;
use crate::tools::AlephTool;

/// Default number of events returned when `limit` is omitted.
const DEFAULT_LIMIT: usize = 5;
/// Hard cap on returned events, to keep the result compact.
const MAX_LIMIT: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RecallEventsArgs {
    /// Keywords describing what to recover from earlier in THIS session
    /// (e.g. "edited auth middleware", "cargo build error", "ran git rebase").
    /// Natural-language phrasing is fine; punctuation is ignored.
    pub query: String,
    /// Maximum number of matching events to return. Defaults to 5, capped at
    /// 20.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One matching event from the session timeline.
#[derive(Debug, Serialize)]
pub struct RecallEventsHit {
    /// Monotonic sequence number of the event within the session — lets the
    /// model order hits and reference a specific point in the timeline.
    pub seq: u64,
    /// Event kind (e.g. `tool_result`, `tool_call_requested`, `tool_error`,
    /// `user_message`).
    pub kind: String,
    /// Wall-clock the event was recorded (unix milliseconds).
    pub at_ms: i64,
    /// A short excerpt of the event around the match.
    pub excerpt: String,
}

#[derive(Debug, Serialize)]
pub struct RecallEventsOutput {
    /// Echo of the query that was run.
    pub query: String,
    /// Number of events returned in `hits`.
    pub matched: usize,
    /// The matching events, most relevant first.
    pub hits: Vec<RecallEventsHit>,
    /// Human-facing note when there is nothing to return (no session, no
    /// store, or no match).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Builtin tool exposing BM25 search over the current session's event log.
#[derive(Clone, Default)]
pub struct RecallEventsTool;

impl RecallEventsTool {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AlephTool for RecallEventsTool {
    const NAME: &'static str = "recall_events";
    const DESCRIPTION: &'static str = "Recall THIS session's earlier activity (your own tool calls, results, errors, and messages) that compaction dropped from the context window. Use it to restore continuity — which file you were editing, the command that failed, the approach you took — by retrieving only the relevant past events instead of asking the user to repeat. (For large offloaded tool OUTPUT use ctx_search; for PAST conversations across other sessions use session_search.) Returns the best-matching events ranked by relevance.";

    type Args = RecallEventsArgs;
    type Output = RecallEventsOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        notify_tool_start(Self::NAME, &args.query);
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

        let empty = |note: &str| RecallEventsOutput {
            query: args.query.clone(),
            matched: 0,
            hits: Vec::new(),
            note: Some(note.to_string()),
        };

        let Some(session_id) = crate::sandbox::context::current_session() else {
            let note = "No active session context — recall_events is unavailable here.";
            notify_tool_result(Self::NAME, note, false);
            return Ok(empty(note));
        };
        let Some(store) = global_session_event_store() else {
            let note = "Session event log is not available in this deployment.";
            notify_tool_result(Self::NAME, note, false);
            return Ok(empty(note));
        };

        // BT-D-R4-10: distinguish DB / store failure from a clean "no match".
        // unwrap_or_default() silently turned an error into an empty
        // result, which the empty-match arm then reported as "No
        // earlier events matched" — the model (and the user) would
        // hear a confident "no match" when the truth was "the database
        // failed to answer". Return a tool error so the caller can
        // retry or surface a real failure.
        let raw_hits = match store
            .search_events(&session_id, &args.query, limit)
            .await
        {
            Ok(h) => h,
            Err(e) => {
                let note = format!(
                    "session event store search failed: {e}; recall_events is unavailable \
                     for this call. Retry or fall back to session_search / ctx_search."
                );
                notify_tool_result(Self::NAME, &note, false);
                return Err(crate::error::AlephError::tool(note));
            }
        };
        let hits: Vec<RecallEventsHit> = raw_hits
            .into_iter()
            .map(|h| RecallEventsHit {
                seq: h.seq,
                kind: h.event_type,
                at_ms: h.created_at_ms,
                excerpt: h.snippet,
            })
            .collect();

        let note = if hits.is_empty() {
            Some(format!(
                "No earlier events matched '{}' this session; try different keywords.",
                args.query
            ))
        } else {
            None
        };

        notify_tool_result(Self::NAME, &format!("{} match(es)", hits.len()), true);

        Ok(RecallEventsOutput {
            query: args.query,
            matched: hits.len(),
            hits,
            note,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_note_when_no_session_context() {
        // Outside any SESSION_ID scope, current_session() is None and the tool
        // must degrade gracefully rather than error.
        let tool = RecallEventsTool::new();
        let out = tool
            .call(RecallEventsArgs {
                query: "anything".to_string(),
                limit: None,
            })
            .await
            .expect("call should not error");
        assert_eq!(out.query, "anything");
        assert_eq!(out.matched, 0);
        assert!(out.note.is_some());
    }

    #[test]
    fn limit_is_clamped() {
        assert_eq!(1usize.clamp(1, MAX_LIMIT), 1);
        assert_eq!(999usize.clamp(1, MAX_LIMIT), MAX_LIMIT);
    }
}
