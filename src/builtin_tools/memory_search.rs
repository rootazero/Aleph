//! Memory search tool with hybrid retrieval and post-retrieval arbitration
//!
//! Implements `AlephTool` trait for AI agent integration.

use crate::sync_primitives::Arc;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::error::ToolError;
use crate::config::types::profile::SmartRecallConfig;
use crate::error::Result;
use crate::memory::context_comptroller::RetrievalResult;
use crate::memory::note_retrieval::NoteFactRetrieval;
use crate::memory::notes::NoteIndexer;
use crate::memory::store::raw_memory::RawMemoryStore;
use crate::memory::store::MemoryBackend;
use crate::memory::{
    ComptrollerConfig, ContextComptroller, EmbeddingProvider, SqliteMemoryBackend, TokenBudget,
    TranscriptIndexer, DEFAULT_AGENT,
};
use crate::tools::AlephTool;

/// Arguments for `memory_search` tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MemorySearchArgs {
    /// Search query
    pub query: String,
    /// Max results to return (default 10)
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    /// Workspace to search in. If omitted, uses the active workspace from execution context.
    #[serde(default)]
    pub workspace: Option<String>,
    /// Search across multiple specific workspaces (e.g., ["crypto", "health"]).
    /// Takes priority over `workspace` when set.
    #[serde(default)]
    pub workspaces: Option<Vec<String>>,
    /// If true, search across ALL workspaces. Takes highest priority.
    #[serde(default)]
    pub cross_workspace: Option<bool>,
    /// Search scope: "all" (default) searches long-term memory only,
    /// "`current_session`" searches only the current session's compressed summaries,
    /// "both" searches long-term memory and the current session summaries together.
    #[serde(default)]
    pub scope: Option<String>,
}

const fn default_max_results() -> usize {
    10
}

/// A single memory fact result
#[derive(Debug, Clone, Serialize)]
pub struct FactResult {
    pub content: String,
    pub note_type: String,
    pub confidence: f32,
    pub similarity_score: f32,
    pub path: String,
}

/// A single transcript result
#[derive(Debug, Clone, Serialize)]
pub struct TranscriptResult {
    pub user_input: String,
    pub ai_output: String,
    pub context: String,
    pub similarity_score: f32,
}

/// Output from `memory_search` tool
#[derive(Debug, Clone, Serialize)]
pub struct MemorySearchOutput {
    pub facts: Vec<FactResult>,
    pub transcripts: Vec<TranscriptResult>,
    pub query: String,
    pub tokens_saved: usize,
    pub path_clusters: Vec<PathCluster>,
    /// Cross-workspace results from Smart Recall Phase 2 (empty if not triggered)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cross_workspace: Vec<CrossWorkspaceFact>,
    /// Whether Smart Recall Phase 2 was triggered
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub smart_recall_triggered: bool,
}

/// A cross-workspace fact returned by Smart Recall Phase 2.
#[derive(Debug, Clone, Serialize)]
pub struct CrossWorkspaceFact {
    pub content: String,
    pub source_workspace: String,
    pub relevance_score: f32,
}

/// A cluster of facts under the same VFS path
#[derive(Debug, Clone, Serialize)]
pub struct PathCluster {
    pub path: String,
    pub fact_count: usize,
    pub top_score: f32,
}

/// Group facts by path, returning clusters where count >= threshold
fn cluster_facts_by_path(facts: &[FactResult], threshold: usize) -> Vec<PathCluster> {
    use std::collections::HashMap;

    let mut groups: HashMap<&str, (usize, f32)> = HashMap::new();
    for fact in facts {
        if fact.path.is_empty() {
            continue;
        }
        let entry = groups.entry(&fact.path).or_insert((0, 0.0));
        entry.0 += 1;
        if fact.similarity_score > entry.1 {
            entry.1 = fact.similarity_score;
        }
    }

    groups
        .into_iter()
        .filter(|(_, (count, _))| *count >= threshold)
        .map(|(path, (count, top_score))| PathCluster {
            path: path.to_string(),
            fact_count: count,
            top_score,
        })
        .collect()
}

/// Memory search tool with hybrid retrieval
pub struct MemorySearchTool {
    database: MemoryBackend,
    note_retrieval: Arc<NoteFactRetrieval<SqliteMemoryBackend>>,
    comptroller: Arc<ContextComptroller>,
    _indexer: Arc<TranscriptIndexer>,
    /// Shared default workspace ID, written per-request by the execution engine
    /// (`execute.rs`) from the session's agent id.
    /// Falls back to `DEFAULT_AGENT` ("main") when not set.
    default_workspace: Arc<RwLock<String>>,
    /// Shared session key for the current session, set by the execution engine.
    /// Used to scope "`current_session`" searches to the active session's summaries.
    default_session_key: Arc<RwLock<String>>,
    /// Per-agent smart recall configs, keyed by agent id and written by the
    /// execution engine at run start. A map (not a single slot) so concurrent
    /// runs of different agents each write their own entry instead of racing
    /// on one process-global value.
    smart_recall_config: Arc<RwLock<std::collections::HashMap<String, SmartRecallConfig>>>,
    /// Mirror of `MemoryConfig.project_scoped` — session raw chunks are written
    /// under the resolved (optionally project-scoped) agent id, so the
    /// `current_session` scope must derive the same id when reading.
    project_scoped: bool,
}

impl MemorySearchTool {
    /// Tool identifier
    pub const NAME: &'static str = "memory_search";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str =
        "Search personal memory for relevant facts and conversation history. \
        Returns both compressed facts and raw transcripts with redundancy elimination. \
        By default searches the active workspace. Use 'workspaces' to search specific workspaces, \
        or 'cross_workspace: true' to search all workspaces. \
        Use 'scope' to control what is searched: 'all' (default, long-term memory only), \
        'current_session' (only this session's compressed summaries), \
        or 'both' (long-term memory plus current session summaries).";

    /// Default similarity threshold when not specified by config.
    ///
    /// L2 distance in high-dimensional space (1536-dim) produces lower similarity
    /// scores than one might expect. 0.3 is a pragmatic floor that balances
    /// recall (not missing relevant memories) with precision (not returning noise).
    const DEFAULT_SIMILARITY_THRESHOLD: f32 = 0.3;

    /// Create a new `MemorySearchTool` instance.
    ///
    /// `similarity_threshold`: if `Some`, overrides the default (from config.toml).
    pub fn new_with_embedder(
        database: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
    ) -> Self {
        Self::new_with_config(database, embedder, None, None, None, None)
    }

    /// Create a new `MemorySearchTool` with explicit similarity threshold from config.
    ///
    /// `rerank_config`: when `Some` and enabled, attaches a cross-encoder
    /// reranker to the long-term recall path. `None` / disabled leaves recall
    /// behaviour unchanged.
    ///
    /// `scoring_config`: when `Some` and active, applies retrieval-time recency
    /// decay + MMR diversity. `None` / inactive leaves ranking unchanged.
    pub fn new_with_config(
        database: MemoryBackend,
        embedder: Arc<dyn EmbeddingProvider>,
        similarity_threshold: Option<f32>,
        rerank_config: Option<&crate::memory::rerank::RerankConfig>,
        scoring_config: Option<&crate::config::types::memory::RetrievalScoringConfig>,
        expansion_config: Option<&crate::config::types::memory::ExpansionConfig>,
    ) -> Self {
        let _threshold = similarity_threshold.unwrap_or(Self::DEFAULT_SIMILARITY_THRESHOLD);

        // NoteFactRetrieval: used for the primary long-term recall path.
        let memory_dir = crate::utils::paths::get_note_memory_dir().unwrap_or_else(|_| {
            std::env::temp_dir()
                .join("aleph")
                .join("memory")
                .join("note")
        });
        let note_indexer = Arc::new(NoteIndexer::new(memory_dir, database.clone()));
        let mut retrieval = NoteFactRetrieval::new(note_indexer, Arc::clone(&embedder));
        if let Some(cfg) = rerank_config {
            retrieval = retrieval.with_rerank_config(cfg);
        }
        if let Some(cfg) = scoring_config {
            retrieval = retrieval.with_scoring_config(cfg);
        }
        if let Some(cfg) = expansion_config {
            retrieval = retrieval.with_expansion_config(cfg);
        }
        let note_retrieval = Arc::new(retrieval);

        let comptroller_config = ComptrollerConfig::default();
        let comptroller = Arc::new(ContextComptroller::new(comptroller_config));

        let indexer = Arc::new(TranscriptIndexer::new(database.clone(), embedder.clone()));

        Self {
            database,
            note_retrieval,
            comptroller,
            _indexer: indexer,
            default_workspace: Arc::new(RwLock::new(DEFAULT_AGENT.to_string())),
            default_session_key: Arc::new(RwLock::new(String::new())),
            smart_recall_config: Arc::new(RwLock::new(std::collections::HashMap::new())),
            project_scoped: false,
        }
    }

    /// Enable per-project scoping for the `current_session` raw-chunk reads,
    /// mirroring `MemoryConfig.project_scoped` (same convention as
    /// `note_manage`'s `with_project_scoping`).
    #[must_use]
    pub const fn with_project_scoping(mut self, enabled: bool) -> Self {
        self.project_scoped = enabled;
        self
    }

    /// Get a shared handle to the default workspace setting.
    ///
    /// The execution engine can update this value when the active workspace changes,
    /// so that tool calls without an explicit `workspace` arg use the correct workspace.
    #[must_use]
    pub fn default_workspace_handle(&self) -> Arc<RwLock<String>> {
        Arc::clone(&self.default_workspace)
    }

    /// Get a shared handle to the current session key.
    ///
    /// The execution engine writes the active session's key string here after
    /// session resolution. Used by `scope="current_session`" to filter `SQLite`
    /// facts under `aleph://session/{session_key}/`.
    #[must_use]
    pub fn default_session_key_handle(&self) -> Arc<RwLock<String>> {
        Arc::clone(&self.default_session_key)
    }

    /// Get a shared handle to the per-agent smart recall configs.
    ///
    /// The execution engine writes the running agent's profile
    /// `SmartRecallConfig` under that agent's id after workspace resolution
    /// (absent key = smart recall disabled for that agent).
    #[must_use]
    pub fn smart_recall_config_handle(
        &self,
    ) -> Arc<RwLock<std::collections::HashMap<String, SmartRecallConfig>>> {
        Arc::clone(&self.smart_recall_config)
    }

    /// Execute memory search (internal implementation)
    async fn call_impl(
        &self,
        args: MemorySearchArgs,
    ) -> std::result::Result<MemorySearchOutput, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        use crate::gateway::agent_env::AgentEnvFilter;

        // Resolve search scope: "all" (default), "current_session", or "both"
        let scope = args.scope.as_deref().unwrap_or("all");

        // Resolve workspace filter with priority:
        // cross_workspace: true → All
        // workspaces: [...] → Multiple
        // workspace: "x" → Single
        // default → Single (active workspace)
        //
        // Per-run truth first: the shared workspace handle is process-global
        // and rewritten at every run start, so a concurrent run of another
        // agent can overwrite it mid-turn and this search would read the
        // wrong agent's workspace. The task-local is scoped per tool call by
        // the dispatch chokepoint and cannot race.
        let default_ws = match crate::tools::turn_context::current_agent_id() {
            Some(agent_id) => agent_id,
            None => self.default_workspace.read().await.clone(),
        };
        let workspace_filter = if args.cross_workspace.unwrap_or(false) {
            AgentEnvFilter::All
        } else if let Some(ref wss) = args.workspaces {
            AgentEnvFilter::Multiple(wss.clone())
        } else {
            let ws = args.workspace.as_deref().unwrap_or(&default_ws);
            AgentEnvFilter::Single(ws.to_string())
        };

        // For logging and path lookups, extract a primary workspace name
        let workspace_label = match &workspace_filter {
            AgentEnvFilter::Single(ws) => ws.clone(),
            AgentEnvFilter::Multiple(wss) => format!("[{}]", wss.join(", ")),
            AgentEnvFilter::All => "ALL".to_string(),
        };

        // Notify tool start
        let args_summary = format!("记忆搜索: {}", &args.query);
        notify_tool_start(Self::NAME, &args_summary);

        info!(query = %args.query, max_results = args.max_results, workspace = %workspace_label, scope = %scope, "Executing memory search");

        // Step 1: Session-local search.
        //
        // When scope is "current_session" or "both", query raw_memories for
        // records stored under the active session's path prefix.
        // This restores the search path that was gutted when the facts table
        // was removed (Task 24); session data now lives in raw_memories.
        let session_facts: Vec<FactResult> = if scope == "current_session" || scope == "both" {
            // Per-run truth first: the shared handle is process-global and
            // rewritten at every run start, so a concurrent run of another
            // agent can overwrite it mid-turn and this search would read the
            // wrong session. The task-local is scoped per tool call by the
            // dispatch chokepoint and cannot race.
            let session_key = match crate::tools::turn_context::current_session_key() {
                Some(sk) => sk,
                None => self.default_session_key.read().await.clone(),
            };
            if session_key.is_empty() {
                Vec::new()
            } else {
                // Session-local raw chunks are written under the session's
                // *resolved* agent id (session_compactor::store_raw_chunk —
                // the id post_turn_compress resolves via project_scope). The
                // session key encodes the owning agent, so parse it rather
                // than trusting the workspace handle (never populated at
                // runtime) or the process-global agent handle (racy across
                // concurrent runs).
                let base_agent = crate::routing::session_key::SessionKey::from_key_string(
                    &session_key,
                )
                .map_or_else(|| "default".to_string(), |k| k.agent_id().to_string());
                let agent_id = crate::memory::project_scope::scoped_or_base(
                    &base_agent,
                    self.project_scoped,
                    crate::projects::current_project_root().as_deref(),
                );
                let agent_id = agent_id.as_str();
                let path_prefix = format!("aleph://session/{session_key}/");
                // Saturate: max_results is LLM-supplied and unclamped, so a
                // huge value must not overflow-panic in debug builds.
                let fetch_limit = args.max_results.saturating_mul(2);
                let raws = self
                    .database
                    .get_raw_by_path_prefix(&path_prefix, agent_id, fetch_limit)
                    .await
                    .map_err(|e| {
                        ToolError::Execution(format!("Session raw fact lookup failed: {e}"))
                    })?;

                let query_lower = args.query.to_lowercase();
                raws.into_iter()
                    .filter(|r| r.content.to_lowercase().contains(&query_lower))
                    .take(args.max_results)
                    .map(|raw| FactResult {
                        content: raw.content,
                        note_type: raw.source.as_str().to_string(),
                        confidence: 1.0,
                        similarity_score: 1.0,
                        path: raw.path.unwrap_or_default(),
                    })
                    .collect()
            }
        } else {
            Vec::new()
        };

        // Step 2: Long-term memory retrieval (when scope is "all" or "both")
        let (
            long_term_facts,
            long_term_transcripts,
            cross_workspace_results,
            recall_triggered,
            tokens_saved,
        ) = if scope == "all" || scope == "both" {
            // Determine if Smart Recall should be used:
            // Only for single-workspace queries where user didn't explicitly request cross-workspace.
            // Keyed by the running agent's id (default_ws, resolved above from
            // the per-run task-local) so a concurrent run of another agent —
            // which writes its own map entry — cannot swap the profile mid-turn.
            let smart_recall_cfg: Option<SmartRecallConfig> =
                self.smart_recall_config.read().await.get(&default_ws).cloned();
            let use_smart_recall = matches!(&workspace_filter, AgentEnvFilter::Single(_))
                && args.cross_workspace.is_none()
                && args.workspaces.is_none()
                && smart_recall_cfg.as_ref().is_some_and(|c| c.enabled);

            let (retrieval_result, cross_ws, triggered) = if use_smart_recall {
                // Smart recall: search primary workspace first, then expand to all agents
                // if primary results are sparse. Uses NoteFactRetrieval multi-agent path.
                let primary_ws = match &workspace_filter {
                    AgentEnvFilter::Single(ws) => ws.clone(),
                    _ => unreachable!(),
                };
                let config = smart_recall_cfg.as_ref().ok_or_else(|| {
                    ToolError::Execution("Smart recall config disappeared".into())
                })?;
                let threshold = config.min_primary_results;
                let max_cross = config.max_cross_results;
                debug!(workspace = %workspace_label, "Performing Smart Recall retrieval via NoteFactRetrieval");

                // Phase 1: primary workspace
                let primary_scored = self
                    .note_retrieval
                    .retrieve(&args.query, &primary_ws, args.max_results)
                    .await
                    .map_err(|e| {
                        ToolError::Execution(format!("Smart recall phase 1 failed: {e}"))
                    })?;

                let recall_triggered = primary_scored.len() < threshold;

                // Phase 2: cross-workspace expansion when primary results are sparse
                let cross_scored = if recall_triggered {
                    info!(
                        primary_count = primary_scored.len(),
                        threshold = threshold,
                        "Smart Recall Phase 2 triggered — expanding to all agents"
                    );
                    let memory_dir =
                        crate::utils::paths::get_note_memory_dir().unwrap_or_else(|_| {
                            std::env::temp_dir()
                                .join("aleph")
                                .join("memory")
                                .join("note")
                        });
                    self.note_retrieval
                        .retrieve_all_agents(&args.query, &memory_dir, args.max_results)
                        .await
                        .map_err(|e| {
                            ToolError::Execution(format!("Smart recall phase 2 failed: {e}"))
                        })?
                } else {
                    Vec::new()
                };

                // Merge primary + cross results, deduplicate by path
                let mut seen_paths = std::collections::HashSet::new();
                let mut merged_facts = Vec::new();
                for sf in primary_scored.iter().chain(cross_scored.iter()) {
                    if seen_paths.insert(sf.fact.path.clone()) {
                        let mut fact = sf.fact.clone();
                        fact.similarity_score = Some(sf.score);
                        merged_facts.push(fact);
                    }
                }

                // Build cross_workspace vec from the expansion results (capped at max_cross)
                let cross_ws: Vec<CrossWorkspaceFact> = if recall_triggered {
                    cross_scored
                        .iter()
                        .filter(|sf| !primary_scored.iter().any(|p| p.fact.path == sf.fact.path))
                        .take(max_cross)
                        .map(|sf| CrossWorkspaceFact {
                            content: sf.fact.content.clone(),
                            source_workspace: sf.fact.agent.clone(),
                            relevance_score: sf.score,
                        })
                        .collect()
                } else {
                    Vec::new()
                };

                if recall_triggered {
                    info!(
                        cross_count = cross_ws.len(),
                        "Smart Recall Phase 2 returned cross-workspace results"
                    );
                }

                let result = RetrievalResult {
                    facts: merged_facts,
                };
                (result, cross_ws, recall_triggered)
            } else {
                // Use NoteFactRetrieval for primary long-term recall, routing
                // each filter variant to its matching retrieval method. A
                // `Multiple`/`All` filter must fan out across agents — collapsing
                // it into the bracketed `workspace_label` (e.g. "[crypto, health]"
                // / "ALL") and calling single-agent `retrieve` searched a
                // non-existent agent and returned nothing.
                debug!(workspace = %workspace_label, "Performing note-based retrieval");
                let scored = match &workspace_filter {
                    AgentEnvFilter::Single(ws) => self
                        .note_retrieval
                        .retrieve(&args.query, ws, args.max_results)
                        .await
                        .map_err(|e| ToolError::Execution(format!("Note retrieval failed: {e}")))?,
                    AgentEnvFilter::Multiple(wss) => self
                        .note_retrieval
                        .retrieve_multi_agent(&args.query, wss, args.max_results)
                        .await
                        .map_err(|e| {
                            ToolError::Execution(format!("Multi-workspace retrieval failed: {e}"))
                        })?,
                    AgentEnvFilter::All => {
                        let memory_dir =
                            crate::utils::paths::get_note_memory_dir().unwrap_or_else(|_| {
                                std::env::temp_dir()
                                    .join("aleph")
                                    .join("memory")
                                    .join("note")
                            });
                        self.note_retrieval
                            .retrieve_all_agents(&args.query, &memory_dir, args.max_results)
                            .await
                            .map_err(|e| {
                                ToolError::Execution(format!(
                                    "Cross-workspace retrieval failed: {e}"
                                ))
                            })?
                    }
                };
                // Convert Vec<ScoredFact> → RetrievalResult for the comptroller pipeline.
                let facts = scored
                    .into_iter()
                    .map(|sf| {
                        let mut fact = sf.fact;
                        fact.similarity_score = Some(sf.score);
                        fact
                    })
                    .collect();
                let result = RetrievalResult { facts };
                (result, Vec::new(), false)
            };

            debug!(
                facts_count = retrieval_result.facts.len(),
                "Long-term retrieval completed"
            );

            // Post-retrieval arbitration
            debug!("Performing post-retrieval arbitration");
            let budget = TokenBudget::new(100000); // Large budget for MVP
            let arbitrated = self.comptroller.arbitrate(retrieval_result, budget);

            info!(
                facts = arbitrated.facts.len(),
                tokens_saved = arbitrated.tokens_saved,
                "Arbitration completed"
            );

            let facts: Vec<FactResult> = arbitrated
                .facts
                .into_iter()
                .map(|f| FactResult {
                    content: f.content,
                    note_type: format!("{:?}", f.note_type),
                    confidence: f.similarity_score.unwrap_or(0.0),
                    similarity_score: f.similarity_score.unwrap_or(0.0),
                    path: f.path,
                })
                .collect();

            let transcripts: Vec<TranscriptResult> = Vec::new();

            (
                facts,
                transcripts,
                cross_ws,
                triggered,
                arbitrated.tokens_saved,
            )
        } else {
            // scope == "current_session" — skip long-term retrieval entirely
            (Vec::new(), Vec::new(), Vec::new(), false, 0)
        };

        // Merge session facts with long-term facts
        let mut facts = session_facts;
        facts.extend(long_term_facts);
        let transcripts = long_term_transcripts;

        // Step 3: Compute path clusters.
        // L1 overview loading is DEPRECATED after the notes migration; the
        // facts-table `get_by_path` lookup has been removed.
        let path_clusters = cluster_facts_by_path(&facts, 3);

        // Notify success
        let cross_suffix = if !cross_workspace_results.is_empty() {
            format!(", {} 条跨域回忆", cross_workspace_results.len())
        } else {
            String::new()
        };
        let result_summary = format!(
            "找到 {} 条事实, {} 条对话记录{} (节省 {} tokens)",
            facts.len(),
            transcripts.len(),
            cross_suffix,
            tokens_saved
        );
        notify_tool_result(Self::NAME, &result_summary, true);

        Ok(MemorySearchOutput {
            facts,
            transcripts,
            query: args.query,
            tokens_saved,
            path_clusters,
            cross_workspace: cross_workspace_results,
            smart_recall_triggered: recall_triggered,
        })
    }
}

impl Clone for MemorySearchTool {
    fn clone(&self) -> Self {
        Self {
            database: self.database.clone(),
            note_retrieval: self.note_retrieval.clone(),
            comptroller: self.comptroller.clone(),
            _indexer: self._indexer.clone(),
            default_workspace: self.default_workspace.clone(),
            default_session_key: self.default_session_key.clone(),
            smart_recall_config: self.smart_recall_config.clone(),
            project_scoped: self.project_scoped,
        }
    }
}

/// Implementation of `AlephTool` trait for `MemorySearchTool`
#[async_trait]
impl AlephTool for MemorySearchTool {
    const NAME: &'static str = "memory_search";
    const DESCRIPTION: &'static str =
        "Search personal memory for relevant facts and conversation history. \
        Returns both compressed facts and raw transcripts with redundancy elimination. \
        By default searches the active workspace. Use 'workspaces' to search specific workspaces, \
        or 'cross_workspace: true' to search all workspaces. \
        Use 'scope' to control what is searched: 'all' (default, long-term memory only), \
        'current_session' (only this session's compressed summaries), \
        or 'both' (long-term memory plus current session summaries).";

    type Args = MemorySearchArgs;
    type Output = MemorySearchOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "memory_search(query='What are my coding preferences?', max_results=10)".to_string(),
            "memory_search(query='Previous discussions about Rust')".to_string(),
            "memory_search(query='My travel plans', max_results=5)".to_string(),
            "memory_search(query='What did we discuss earlier?', scope='current_session')"
                .to_string(),
            "memory_search(query='Rust async patterns', scope='both')".to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_search_args_serialization() {
        // Test that args can be serialized/deserialized
        let args = MemorySearchArgs {
            query: "test query".to_string(),
            max_results: 5,
            workspace: None,
            workspaces: None,
            cross_workspace: None,
            scope: None,
        };

        let json = serde_json::to_string(&args).unwrap();
        let deserialized: MemorySearchArgs = serde_json::from_str(&json).unwrap();

        assert_eq!(args.query, deserialized.query);
        assert_eq!(args.max_results, deserialized.max_results);
    }

    #[test]
    fn test_cross_workspace_args_deserialization() {
        // Test cross_workspace: true
        let json = r#"{"query": "exercise plan", "cross_workspace": true}"#;
        let args: MemorySearchArgs = serde_json::from_str(json).unwrap();
        assert!(args.cross_workspace.unwrap());
        assert!(args.workspaces.is_none());

        // Test workspaces: [...]
        let json = r#"{"query": "health tips", "workspaces": ["health", "fitness"]}"#;
        let args: MemorySearchArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.workspaces.as_ref().unwrap().len(), 2);
        assert!(args.cross_workspace.is_none());

        // Test backward compatibility: just query
        let json = r#"{"query": "hello"}"#;
        let args: MemorySearchArgs = serde_json::from_str(json).unwrap();
        assert!(args.workspace.is_none());
        assert!(args.workspaces.is_none());
        assert!(args.cross_workspace.is_none());
        assert_eq!(args.max_results, 10); // default
    }

    #[test]
    fn test_default_max_results() {
        assert_eq!(default_max_results(), 10);
    }

    #[test]
    fn test_path_cluster_serialization() {
        let cluster = PathCluster {
            path: "aleph://user/preferences/coding/".to_string(),
            fact_count: 5,
            top_score: 0.85,
        };
        let json = serde_json::to_string(&cluster).unwrap();
        assert!(json.contains("aleph://user/preferences/coding/"));
        assert!(json.contains("5"));
    }

    #[test]
    fn test_cluster_facts_by_path() {
        let facts = vec![
            FactResult {
                content: "Fact 1".into(),
                note_type: "Preference".into(),
                confidence: 0.9,
                similarity_score: 0.8,
                path: "aleph://user/preferences/coding/".into(),
            },
            FactResult {
                content: "Fact 2".into(),
                note_type: "Preference".into(),
                confidence: 0.85,
                similarity_score: 0.75,
                path: "aleph://user/preferences/coding/".into(),
            },
            FactResult {
                content: "Fact 3".into(),
                note_type: "Preference".into(),
                confidence: 0.8,
                similarity_score: 0.7,
                path: "aleph://user/preferences/coding/".into(),
            },
            FactResult {
                content: "Fact 4".into(),
                note_type: "Learning".into(),
                confidence: 0.9,
                similarity_score: 0.6,
                path: "aleph://knowledge/learning/".into(),
            },
        ];

        let clusters = cluster_facts_by_path(&facts, 3);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].path, "aleph://user/preferences/coding/");
        assert_eq!(clusters[0].fact_count, 3);
    }
}
