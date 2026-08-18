//! Cross-session search tool using summary-driven retrieval (Spec B).
//!
//! Primary path: query `HybridAssembler` with `FactSourceFilter::Only(SessionCompressed)`
//! to get structured session summary facts, deduplicate per session, and enrich
//! each hit with 0-2 raw FTS5 evidence quotes.
//!
//! Lazy fallback: for sessions that appear in raw FTS5 but have no summary yet,
//! call `SummarySynthesizer::lazy_for` to synthesize on demand.
//!
//! Access-controlled on two independent axes, and neither substitutes for the
//! other:
//!
//! - **Which agent** may reach which — the caller's A2A policy
//!   ([`SessionSearchTool::is_accessible`]).
//! - **Which user** a session belongs to — P1/P2 isolation, through
//!   [`crate::gateway::visibility::ambient_transcript_visible`]
//!   ([`SessionSearchTool::transcript_visible`]). `search_messages` is a global
//!   FTS5 sweep over every session on the install; the A2A policy is blind to
//!   ownership, so this axis is the only thing standing between the model and
//!   another person's private transcript.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::error::ToolError;
use crate::error::Result;
use crate::gateway::context::GatewayContext;
use crate::memory::assembler::envelope::ItemSource;
use crate::memory::assembler::{AssemblyBudget, WorkingMemoryAssembler};
use crate::memory::context::FactSource;
use crate::memory::session_search_summary::dedup::{top_per_session, ScoredCandidate};
use crate::memory::session_search_summary::synthesizer::SummarySynthesizer;
use crate::memory::session_search_summary::FactSourceFilter;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct SessionSearchArgs {
    /// Full-text search query to find in past conversations
    pub query: String,
    /// Maximum number of matching messages to return (default 5)
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

const fn default_max_results() -> usize {
    5
}

/// BT-D-R4-08: hard cap on max_results. The user-supplied value drives
/// the top-N selector (candidates x4 for the lazy fallback, plus one
/// LLM synthesis call per primary survivor, plus potentially one per
/// lazy hit). An unbounded value (a model passing usize::MAX, or
/// repeatedly retrying with the same value) is a slow-burn cost
/// amplifier — the LLM-synthesis path can fire dozens of times per
/// call. 100 is far above any sensible cross-session recall request
/// and well below the cost becoming a real concern.
const MAX_SESSION_SEARCH_RESULTS: usize = 100;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub enum SummarySource {
    /// Reused from the existing `session_compactor` d{depth}/{seq} facts.
    Compactor,
    /// Produced by the `on_session_end` hook backstop.
    SessionEnd,
    /// Synthesized at query time as a fallback for in-flight short sessions.
    Lazy,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionSearchHit {
    pub session_key: String,
    pub agent_id: String,
    pub topic: Option<String>,
    /// Synthesized excerpt of the matched session (≤ 1500 chars).
    pub summary: String,
    /// 0-2 raw FTS5 snippets from the session's transcript (≤ 200 chars each).
    pub evidence_quotes: Vec<String>,
    pub timestamp: i64,
    pub source: SummarySource,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionSearchOutput {
    pub query: String,
    pub hits: Vec<SessionSearchHit>,
    pub total_hits: usize,
}

#[derive(Clone)]
pub struct SessionSearchTool {
    context: Arc<GatewayContext>,
    caller_agent_id: String,
    /// `HybridAssembler` for summary-driven primary retrieval. `None` when MCP
    /// is not yet injected; `call_impl` falls through to the raw FTS5 fallback.
    assembler: Option<Arc<dyn WorkingMemoryAssembler>>,
    /// `SummarySynthesizer` for lazy on-demand fallback synthesis. `None` when
    /// no `AiProvider` is configured; fallback skips synthesis and marks hit as unavailable.
    synthesizer: Option<Arc<SummarySynthesizer>>,
}

impl SessionSearchTool {
    pub fn new(
        context: Arc<GatewayContext>,
        caller_agent_id: impl Into<String>,
        assembler: Option<Arc<dyn WorkingMemoryAssembler>>,
        synthesizer: Option<Arc<SummarySynthesizer>>,
    ) -> Self {
        Self {
            context,
            caller_agent_id: caller_agent_id.into(),
            assembler,
            synthesizer,
        }
    }

    /// Check if a session owned by `session_agent_id` is accessible to the caller.
    fn is_accessible(&self, session_agent_id: &str) -> bool {
        self.context
            .a2a_policy()
            .is_allowed(&self.caller_agent_id, session_agent_id)
    }

    /// Whether this run may read `session_key`'s transcript at all (P1/P2 user
    /// isolation), memoised per call.
    ///
    /// The A2A policy above answers a different question — which AGENT may
    /// reach which agent — and is blind to WHO the session belongs to, so on a
    /// multi-user install it let the model quote another person's private
    /// conversation verbatim through the raw-FTS paths. The rule itself is not
    /// re-derived here: `visibility::ambient_transcript_visible` is the one
    /// body, so a room's transcript follows the roster exactly as it does in
    /// `sessions.list`.
    async fn transcript_visible(
        &self,
        session_key: &str,
        memo: &mut std::collections::HashMap<String, bool>,
    ) -> bool {
        if let Some(known) = memo.get(session_key) {
            return *known;
        }
        let visible = crate::gateway::visibility::ambient_transcript_visible(
            self.context.session_store().as_ref(),
            session_key,
        )
        .await;
        memo.insert(session_key.to_string(), visible);
        visible
    }

    async fn call_impl(
        &self,
        args: SessionSearchArgs,
    ) -> std::result::Result<SessionSearchOutput, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        // BT-D-R4-08: clamp the user-supplied max_results. The previous
        // shape trusted args.max_results verbatim, so the top-N
        // selector and the lazy fallback (which fetches
        // max_results*4) both inherited any value the model passed.
        // Clamp to MAX_SESSION_SEARCH_RESULTS at the tool boundary so
        // the same value is used everywhere downstream.
        let max_results = args.max_results.clamp(1, MAX_SESSION_SEARCH_RESULTS);

        notify_tool_start("session_search", &format!("搜索历史对话: {}", &args.query));

        let mut hits: Vec<SessionSearchHit> = Vec::new();
        // Per-call memo for the P1/P2 transcript gate: both the summary path
        // and the raw-FTS fallback ask about the same session keys, and each
        // miss costs a `get_metadata` round trip.
        let mut visible_memo: std::collections::HashMap<String, bool> =
            std::collections::HashMap::new();

        // ① Primary retrieval — summaries only (skipped if assembler not available).
        if let Some(ref assembler) = self.assembler {
            let envelope = assembler
                .assemble(
                    &args.query,
                    &self.caller_agent_id,
                    None,
                    AssemblyBudget { total_tokens: 4000 },
                    FactSourceFilter::Only(FactSource::SessionCompressed),
                )
                .await
                .map_err(|e| ToolError::Execution(format!("assembler: {e}")))?;

            // Translate envelope items into ScoredCandidate.
            let candidates: Vec<ScoredCandidate> = envelope
                .slots
                .iter()
                .flat_map(|s| s.items.iter())
                .filter_map(|item| {
                    let (session_key, fact_path) = match &item.source {
                        ItemSource::Summary { session_id, layer } => {
                            let path = format!("aleph://session/{session_id}/{layer}");
                            (session_id.clone(), path)
                        }
                        ItemSource::Raw {
                            session_id,
                            raw_id,
                            path,
                        } => {
                            let fact_path = path.clone().unwrap_or_else(|| {
                                format!("aleph://session/{session_id}/raw/{raw_id}")
                            });
                            (session_id.clone(), fact_path)
                        }
                        ItemSource::Note { .. } => return None,
                    };
                    Some(ScoredCandidate {
                        session_key,
                        agent_id: envelope.agent_id.clone(),
                        fact_path,
                        summary_text: item.content.clone(),
                        topic: if item.title.is_empty() {
                            None
                        } else {
                            Some(item.title.clone())
                        },
                        timestamp: item.updated_at,
                        score: item.relevance,
                    })
                })
                .collect();

            // ② Per-session dedup + cap.
            let survivors = top_per_session(candidates, max_results);

            // ③ Build hits, fetching evidence_quotes per surviving session.
            for c in &survivors {
                // P1/P2 isolation. Checked BEFORE the evidence fetch: the
                // quotes are raw transcript bytes, so a denied session must
                // cost neither an FTS round trip nor a chance to leak one.
                if !self
                    .transcript_visible(&c.session_key, &mut visible_memo)
                    .await
                {
                    continue;
                }
                let evidence = self
                    .fetch_evidence_quotes(&args.query, &c.session_key, 2)
                    .await
                    .unwrap_or_default();
                hits.push(SessionSearchHit {
                    session_key: c.session_key.clone(),
                    agent_id: c.agent_id.clone(),
                    topic: c.topic.clone(),
                    summary: truncate(&c.summary_text, 1500),
                    evidence_quotes: evidence,
                    timestamp: c.timestamp,
                    source: source_from_path(&c.fact_path),
                });
            }

            // ④ A2A filter.
            hits.retain(|h| self.is_accessible(&h.agent_id));
        }

        // ⑤ Lazy fallback: raw FTS5 hits whose session has no summary fact yet.
        let already_covered: std::collections::HashSet<String> =
            hits.iter().map(|h| h.session_key.clone()).collect();

        let raw_hits = self
            .context
            .session_store()
            .search_messages(&args.query, max_results.saturating_mul(4))
            .await
            .map_err(|e| ToolError::Execution(format!("session_store fallback: {e}")))?;

        for raw in raw_hits {
            if already_covered.contains(&raw.session_key) {
                continue;
            }
            if !self.is_accessible(&raw.agent_id) {
                continue;
            }
            // P1/P2 isolation. `search_messages` is a global FTS5 sweep over
            // every session on the install and the A2A policy above is blind to
            // WHO a session belongs to, so without this the model could quote
            // another person's private conversation verbatim.
            if !self
                .transcript_visible(&raw.session_key, &mut visible_memo)
                .await
            {
                continue;
            }
            if hits.len() >= max_results {
                break;
            }

            let summary = if let Some(ref synth) = self.synthesizer {
                match synth.lazy_for(&raw.agent_id, &raw.session_key).await {
                    Ok(fact) => truncate(&fact.content, 1500),
                    Err(_) => "[summary unavailable]".to_string(),
                }
            } else {
                "[summary unavailable]".to_string()
            };

            hits.push(SessionSearchHit {
                session_key: raw.session_key,
                agent_id: raw.agent_id,
                topic: raw.topic,
                summary,
                evidence_quotes: vec![truncate(&raw.content, 200)],
                timestamp: raw.timestamp,
                source: SummarySource::Lazy,
            });
        }

        let total_hits = hits.len();

        debug!(
            caller = %self.caller_agent_id,
            returned = total_hits,
            requested = max_results,
            "session_search: summary-driven results"
        );

        notify_tool_result(
            "session_search",
            &format!("找到 {total_hits} 条历史会话摘要"),
            true,
        );
        Ok(SessionSearchOutput {
            query: args.query,
            hits,
            total_hits,
        })
    }

    /// Fetch up to `max_quotes` raw FTS5 snippets for the given session.
    async fn fetch_evidence_quotes(
        &self,
        query: &str,
        session_key: &str,
        max_quotes: usize,
    ) -> Result<Vec<String>> {
        let raw = self
            .context
            .session_store()
            .search_messages(query, max_quotes.saturating_mul(8))
            .await
            .map_err(|e| crate::error::AlephError::tool(format!("evidence search: {e}")))?;
        let mut quotes: Vec<String> = raw
            .into_iter()
            .filter(|r| r.session_key == session_key)
            .take(max_quotes)
            .map(|r| truncate(&r.content, 200))
            .collect();
        quotes.truncate(max_quotes);
        Ok(quotes)
    }
}

/// Determine `SummarySource` from the canonical fact path.
/// - Paths ending in `/end-summary` → `SessionEnd`
/// - All other session paths (d{depth}/{seq}) → `Compactor`
fn source_from_path(path: &str) -> SummarySource {
    if path.ends_with("/end-summary") {
        SummarySource::SessionEnd
    } else {
        SummarySource::Compactor
    }
}

/// Hard cap at `max_chars` INCLUDING the `…`, so a quote budget is a real
/// ceiling rather than a hint.
fn truncate(s: &str, max_chars: usize) -> String {
    crate::utils::text_format::truncate_reserving(s, max_chars, "…")
}

#[async_trait]
impl AlephTool for SessionSearchTool {
    const NAME: &'static str = "session_search";
    const DESCRIPTION: &'static str =
        "Search past conversation transcripts across all sessions and retrieve summarized \
        excerpts. Each hit is one past session, returned with `summary` (synthesized excerpt \
        of what that session was about), `evidence_quotes` (0-2 raw transcript snippets for \
        grounding), and `source` (Compactor | SessionEnd | Lazy — Compactor is the most \
        authoritative when available). Use `summary` first; only consult `evidence_quotes` \
        when the summary is too abstract to answer the question.";

    type Args = SessionSearchArgs;
    type Output = SessionSearchOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_deserialization() {
        let json = r#"{"query": "test search"}"#;
        let args: SessionSearchArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.query, "test search");
        assert_eq!(args.max_results, 5);
    }

    #[test]
    fn args_with_max_results() {
        let json = r#"{"query": "test", "max_results": 3}"#;
        let args: SessionSearchArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.max_results, 3);
    }
}
