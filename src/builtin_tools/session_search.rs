//! Cross-session search tool using summary-driven retrieval (Spec B).
//!
//! Primary path: query `HybridAssembler` with `FactSourceFilter::Only(SessionCompressed)`
//! to get structured session summary facts, deduplicate per session, and enrich
//! each hit with 0-2 raw FTS5 evidence quotes.
//!
//! Lazy fallback: for sessions that appear in raw FTS5 but have no summary yet,
//! call `SummarySynthesizer::lazy_for` to synthesize on demand.
//!
//! Access-controlled: results are filtered by the caller's A2A policy so that
//! an agent can only see transcripts from sessions it is allowed to reach.

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

    async fn call_impl(
        &self,
        args: SessionSearchArgs,
    ) -> std::result::Result<SessionSearchOutput, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        notify_tool_start("session_search", &format!("搜索历史对话: {}", &args.query));

        let mut hits: Vec<SessionSearchHit> = Vec::new();

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
            let survivors = top_per_session(candidates, args.max_results.max(1));

            // ③ Build hits, fetching evidence_quotes per surviving session.
            for c in &survivors {
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
            .search_messages(&args.query, args.max_results.saturating_mul(4))
            .await
            .map_err(|e| ToolError::Execution(format!("session_store fallback: {e}")))?;

        for raw in raw_hits {
            if already_covered.contains(&raw.session_key) {
                continue;
            }
            if !self.is_accessible(&raw.agent_id) {
                continue;
            }
            if hits.len() >= args.max_results {
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
            requested = args.max_results,
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

/// UTF-8-safe truncation at `max_chars` characters, appending `…` when trimmed.
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
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
