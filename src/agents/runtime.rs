//! Sub-agent runtime — canonical home for `AgentRuntime` and the types it
//! exposes to `SubagentTool` and callers.
//!
//! Relocated from `agent_loop/agent_runtime.rs` during Phase 6c deletion.
//! `AgentLoop` itself (in `agent_loop/loop_core.rs`) is retained; this module
//! wraps it with lifecycle tracing and transcript persistence.

use std::time::Instant;

use tokio_util::sync::CancellationToken;

use crate::agent_loop::subagent_runner::{run_subagent, SubagentRunConfig};
use crate::agents::AgentDef;
use crate::harness::chain_context::ChainContext;
use crate::providers::AiProvider;
use crate::session::ingress_safety::SafetyGuard;
use crate::sync_primitives::Arc;
use crate::thinker::prompt_builder::PromptSnapshot;
use crate::tools::runtime::LoopToolRegistry;

// =============================================================================
// LoopRunResult
// =============================================================================

/// Outcome of a completed sub-agent run. Mirrors the legacy
/// `agent_loop::loop_core::LoopRunResult` field-for-field so that
/// `SubagentTool` and downstream consumers see zero behavior change.
#[derive(Debug, Clone)]
pub struct LoopRunResult {
    pub final_text: Option<String>,
    pub iterations: usize,
    pub tool_calls_made: usize,
    pub total_tokens: usize,
    pub hit_limit: bool,
    pub cancelled: bool,
    /// Chain ID shared across all depths in a subagent call chain.
    pub chain_id: String,
    /// Nesting depth (0 = root agent).
    pub depth: u32,
}

// =============================================================================
// Type aliases
// =============================================================================

/// Factory that builds a fresh LoopToolRegistry for the sub-agent.
///
/// The factory is responsible for providing the parent's tools minus
/// the "subagent" tool itself (to prevent infinite recursion).
pub type ToolRegistryFactory = Arc<dyn Fn() -> LoopToolRegistry + Send + Sync>;

/// Factory that builds a SafetyGuard for the sub-agent.
///
/// SafetyGuard is not Clone, so we use a factory to produce a fresh instance
/// each time a sub-agent is spawned.
pub type SafetyGuardFactory = Arc<dyn Fn() -> SafetyGuard + Send + Sync>;

// SharedSnapshot is re-exported from crate::agent_loop (defined in agent_loop/mod.rs)
// to avoid a circular dependency between this module and loop_core.

// =============================================================================
// AgentRuntimeConfig
// =============================================================================

/// Configuration for launching a sub-agent.
pub struct AgentRuntimeConfig {
    /// The agent definition describing role, tools, and limits.
    pub agent_def: AgentDef,
    /// The task to execute.
    pub task: String,
    /// Optional context summary from the parent agent.
    pub context_summary: Option<String>,
    /// Explicit model override (highest priority).
    pub model: Option<String>,
    /// Timeout in seconds for the entire run.
    pub timeout_secs: u64,
    /// Optional prompt snapshot from the parent (used by fork path).
    pub prompt_snapshot: Option<PromptSnapshot>,
}

// =============================================================================
// Transcript types
// =============================================================================

/// Outcome classification for a sub-agent execution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum TranscriptOutcome {
    /// The sub-agent completed successfully.
    Success,
    /// The sub-agent encountered an error.
    Error(String),
    /// The sub-agent was terminated due to timeout.
    Timeout,
}

/// Structured transcript of a sub-agent execution for observability.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubagentTranscript {
    /// Unique identifier for the agent instance.
    pub agent_id: String,
    /// Agent type name (from agent_def).
    pub agent_type: String,
    /// Summary of the task that was executed.
    pub task_summary: String,
    /// How the execution ended.
    pub outcome: TranscriptOutcome,
    /// Number of think-act iterations completed.
    pub iterations: usize,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// Total tokens consumed.
    pub tokens_used: usize,
    /// Key findings extracted from the agent's final response (first 200 chars).
    pub key_findings: String,
}

// =============================================================================
// AgentRuntime
// =============================================================================

/// Middle layer that manages sub-agent lifecycle: setup, execution, and transcript.
pub struct AgentRuntime {
    provider: Arc<dyn AiProvider>,
    tool_registry_factory: ToolRegistryFactory,
    safety_guard_factory: SafetyGuardFactory,
    child_chain: ChainContext,
    cancel_token: CancellationToken,
}

impl AgentRuntime {
    /// Create a new runtime with the shared infrastructure needed to spawn agents.
    pub fn new(
        provider: Arc<dyn AiProvider>,
        tool_registry_factory: ToolRegistryFactory,
        safety_guard_factory: SafetyGuardFactory,
        child_chain: ChainContext,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            provider,
            tool_registry_factory,
            safety_guard_factory,
            child_chain,
            cancel_token,
        }
    }

    /// Execute a sub-agent to completion with lifecycle tracing.
    pub async fn run(&self, config: AgentRuntimeConfig) -> Result<LoopRunResult, String> {
        let start = Instant::now();
        let agent_id = format!(
            "{}-{}",
            config.agent_def.id,
            start.elapsed().as_nanos() % 100_000
        );
        let agent_type = config.agent_def.id.clone();
        let task_summary = truncate_for_log(&config.task, 120);

        tracing::info!(
            agent_id = %agent_id,
            agent_type = %agent_type,
            task = %task_summary,
            "SubagentStart: launching sub-agent"
        );

        let result = self.execute_fresh_path(&config).await;

        let duration_ms = start.elapsed().as_millis() as u64;
        let key_findings = match &result {
            Ok(run_result) => run_result
                .final_text
                .as_deref()
                .unwrap_or("")
                .chars()
                .take(200)
                .collect::<String>(),
            Err(_) => String::new(),
        };
        let transcript = match &result {
            Ok(run_result) => SubagentTranscript {
                agent_id: agent_id.clone(),
                agent_type: agent_type.clone(),
                task_summary: task_summary.clone(),
                outcome: TranscriptOutcome::Success,
                iterations: run_result.iterations,
                duration_ms,
                tokens_used: run_result.total_tokens,
                key_findings: key_findings.clone(),
            },
            Err(e) => {
                let outcome = if e.contains("timed out") {
                    TranscriptOutcome::Timeout
                } else {
                    TranscriptOutcome::Error(e.clone())
                };
                SubagentTranscript {
                    agent_id: agent_id.clone(),
                    agent_type: agent_type.clone(),
                    task_summary: task_summary.clone(),
                    outcome,
                    iterations: 0,
                    duration_ms,
                    tokens_used: 0,
                    key_findings: key_findings.clone(),
                }
            }
        };

        tracing::info!(
            agent_id = %transcript.agent_id,
            agent_type = %transcript.agent_type,
            outcome = ?format_outcome(&transcript.outcome),
            iterations = transcript.iterations,
            duration_ms = transcript.duration_ms,
            tokens_used = transcript.tokens_used,
            "SubagentEnd: sub-agent completed"
        );

        persist_transcript(&transcript, &self.child_chain.chain_id);

        result
    }

    async fn execute_fresh_path(
        &self,
        config: &AgentRuntimeConfig,
    ) -> Result<LoopRunResult, String> {
        let mut registry = (self.tool_registry_factory)();
        registry.retain(|name| config.agent_def.is_tool_allowed(name));

        run_subagent(SubagentRunConfig {
            provider: &self.provider,
            registry,
            safety_guard: (self.safety_guard_factory)(),
            agent_def: &config.agent_def,
            model: config.model.clone(),
            task: &config.task,
            context_summary: config.context_summary.as_deref(),
            timeout_secs: config.timeout_secs,
            child_chain: self.child_chain.clone(),
            cancel_token: self.cancel_token.clone(),
        })
        .await
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Truncate a string for log output, appending "..." if truncated.
pub fn truncate_for_log(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        match s.char_indices().nth(max_len) {
            Some((idx, _)) => format!("{}...", &s[..idx]),
            None => s.to_string(),
        }
    }
}

/// Format a TranscriptOutcome for log output.
pub fn format_outcome(outcome: &TranscriptOutcome) -> &str {
    match outcome {
        TranscriptOutcome::Success => "success",
        TranscriptOutcome::Error(_) => "error",
        TranscriptOutcome::Timeout => "timeout",
    }
}

/// Maximum transcript directories to retain per session.
const MAX_TRANSCRIPT_DIRS: usize = 50;

fn cleanup_old_transcripts(base_dir: &std::path::Path) {
    let parent = match base_dir.parent() {
        Some(p) => p,
        None => return,
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };

    let mut dirs: Vec<(std::path::PathBuf, std::time::SystemTime)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((e.path(), modified))
        })
        .collect();

    if dirs.len() <= MAX_TRANSCRIPT_DIRS {
        return;
    }

    dirs.sort_by_key(|(_, t)| *t);
    let to_remove = dirs.len() - MAX_TRANSCRIPT_DIRS;
    for (path, _) in dirs.into_iter().take(to_remove) {
        let _ = std::fs::remove_dir_all(path);
    }
}

/// Persist a subagent transcript to disk for future retrieval.
/// Best-effort: errors are logged but not propagated.
pub fn persist_transcript(transcript: &SubagentTranscript, session_id: &str) {
    let base = match dirs::home_dir() {
        Some(h) => h.join(".aleph/data/transcripts").join(session_id),
        None => {
            tracing::warn!("Cannot resolve home dir for transcript persistence");
            return;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&base) {
        tracing::warn!(error = %e, "Failed to create transcript directory");
        return;
    }
    let path = base.join(format!("{}.json", transcript.agent_id));
    match serde_json::to_string_pretty(transcript) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::warn!(path = %path.display(), error = %e, "Failed to write transcript");
            } else {
                tracing::debug!(path = %path.display(), "Transcript persisted");
                cleanup_old_transcripts(&base);
            }
        }
        Err(e) => tracing::warn!(error = %e, "Failed to serialize transcript"),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_agent_def() -> AgentDef {
        AgentDef::new("test-agent", crate::agents::AgentMode::SubAgent)
    }

    #[test]
    fn agent_runtime_config_construction() {
        let config = AgentRuntimeConfig {
            agent_def: make_agent_def(),
            task: "Do something".to_string(),
            context_summary: Some("Parent context".to_string()),
            model: Some("claude-sonnet".to_string()),
            timeout_secs: 60,
            prompt_snapshot: None,
        };

        assert_eq!(config.task, "Do something");
        assert_eq!(config.timeout_secs, 60);
        assert!(config.context_summary.is_some());
        assert!(config.model.is_some());
        assert!(config.prompt_snapshot.is_none());
    }

    #[test]
    fn transcript_outcome_variants() {
        let success = TranscriptOutcome::Success;
        assert_eq!(format_outcome(&success), "success");

        let error = TranscriptOutcome::Error("something broke".to_string());
        assert_eq!(format_outcome(&error), "error");

        let timeout = TranscriptOutcome::Timeout;
        assert_eq!(format_outcome(&timeout), "timeout");
    }

    #[test]
    fn subagent_transcript_field_access() {
        let transcript = SubagentTranscript {
            agent_id: "test-agent-123".to_string(),
            agent_type: "test-agent".to_string(),
            task_summary: "test task".to_string(),
            outcome: TranscriptOutcome::Success,
            iterations: 5,
            duration_ms: 1200,
            tokens_used: 500,
            key_findings: "some findings".to_string(),
        };

        assert_eq!(transcript.agent_id, "test-agent-123");
        assert_eq!(transcript.agent_type, "test-agent");
        assert_eq!(transcript.task_summary, "test task");
        assert_eq!(transcript.iterations, 5);
        assert_eq!(transcript.duration_ms, 1200);
        assert_eq!(transcript.tokens_used, 500);
    }

    #[test]
    fn truncate_for_log_short_string() {
        assert_eq!(truncate_for_log("hello", 10), "hello");
    }

    #[test]
    fn truncate_for_log_long_string() {
        let s = "a".repeat(200);
        let result = truncate_for_log(&s, 50);
        assert!(result.ends_with("..."));
        assert_eq!(result.len(), 53);
    }

    #[test]
    fn truncate_for_log_unicode_safe() {
        let s = "你好世界这是一个很长的字符串用于测试截断功能";
        let result = truncate_for_log(s, 5);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn transcript_serialization_roundtrip() {
        let transcript = SubagentTranscript {
            agent_id: "test-123".to_string(),
            agent_type: "explorer".to_string(),
            task_summary: "Find all Rust files".to_string(),
            outcome: TranscriptOutcome::Success,
            iterations: 5,
            duration_ms: 1200,
            tokens_used: 3000,
            key_findings: "Found 42 Rust files in src/".to_string(),
        };

        let json = serde_json::to_string(&transcript).unwrap();
        let deserialized: SubagentTranscript = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.agent_id, "test-123");
        assert_eq!(deserialized.iterations, 5);
        assert_eq!(deserialized.key_findings, "Found 42 Rust files in src/");
        assert!(matches!(deserialized.outcome, TranscriptOutcome::Success));
    }

    #[test]
    fn transcript_error_outcome_roundtrip() {
        let transcript = SubagentTranscript {
            agent_id: "err-1".to_string(),
            agent_type: "planner".to_string(),
            task_summary: "Plan feature".to_string(),
            outcome: TranscriptOutcome::Error("timeout".to_string()),
            iterations: 0,
            duration_ms: 5000,
            tokens_used: 0,
            key_findings: String::new(),
        };

        let json = serde_json::to_string(&transcript).unwrap();
        assert!(json.contains("\"timeout\""));
        let deserialized: SubagentTranscript = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized.outcome, TranscriptOutcome::Error(ref e) if e == "timeout"));
    }
}
