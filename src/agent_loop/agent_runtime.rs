//! AgentRuntime — lifecycle layer between SubagentTool (dispatch) and AgentLoop (execution).
//!
//! Responsibilities:
//! - Resolve model override chain (explicit > agent hint > default)
//! - Build tool registry filtered by agent permissions
//! - Construct PromptBuilder with agent definition
//! - Execute AgentLoop with timeout and cancellation
//! - Record structured transcript via tracing

use std::time::Instant;

use tokio_util::sync::CancellationToken;

use super::chain_context::ChainContext;
use super::loop_core::{AgentLoop, LoopConfig, LoopRunResult, NoopCallback};
use super::provider_bridge::AiProviderBridge;
use super::safety::SafetyGuard;
use super::tool::LoopToolRegistry;
use crate::agents::AgentDef;
use crate::providers::AiProvider;
use crate::sync_primitives::{Arc, RwLock};
use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig, PromptSnapshot};

// ─────────────────────────────────────────────────────────────────────────────
// Type aliases (migrated from subagent_runner.rs)
// ─────────────────────────────────────────────────────────────────────────────

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

/// Shared snapshot of the parent's prompt state, used by the fork path (Task 7).
pub type SharedSnapshot = Arc<RwLock<Option<PromptSnapshot>>>;

// ─────────────────────────────────────────────────────────────────────────────
// AgentRuntimeConfig
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration for launching a sub-agent via `AgentRuntime`.
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

// ─────────────────────────────────────────────────────────────────────────────
// SubagentTranscript / TranscriptOutcome
// ─────────────────────────────────────────────────────────────────────────────

/// Outcome classification for a sub-agent execution.
#[derive(Debug, Clone)]
pub enum TranscriptOutcome {
    /// The sub-agent completed successfully.
    Success,
    /// The sub-agent encountered an error.
    Error(String),
    /// The sub-agent was terminated due to timeout.
    Timeout,
}

/// Structured transcript of a sub-agent execution for observability.
#[derive(Debug, Clone)]
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
}

// ─────────────────────────────────────────────────────────────────────────────
// AgentRuntime
// ─────────────────────────────────────────────────────────────────────────────

/// Middle layer that manages sub-agent lifecycle: setup, execution, and transcript.
///
/// Replaces the free function `run_subagent()` from `subagent_runner.rs` with a
/// struct-based API that adds structured tracing and transcript recording.
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
    ///
    /// This is the fresh-path execution that builds everything from scratch
    /// (model, registry, prompt builder, loop config) from the agent definition.
    /// The fork path (reusing a parent snapshot) will be added in Task 7.
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

        // Phase 1: Resolve model, build registry, prompt builder, loop config
        let result = self.execute_fresh_path(&config).await;

        // Phase 4: Record transcript
        let duration_ms = start.elapsed().as_millis() as u64;
        let transcript = match &result {
            Ok(run_result) => SubagentTranscript {
                agent_id: agent_id.clone(),
                agent_type: agent_type.clone(),
                task_summary: task_summary.clone(),
                outcome: TranscriptOutcome::Success,
                iterations: run_result.iterations,
                duration_ms,
                tokens_used: run_result.total_tokens,
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

        result
    }

    /// Fresh-path execution: build everything from the agent definition.
    async fn execute_fresh_path(&self, config: &AgentRuntimeConfig) -> Result<LoopRunResult, String> {
        // Resolve model: explicit arg > agent_def.model_hint > default
        let resolved_model = config
            .model
            .clone()
            .or_else(|| config.agent_def.model_hint.clone());
        let bridge = if let Some(m) = resolved_model {
            AiProviderBridge::new(self.provider.clone()).with_model(m)
        } else {
            AiProviderBridge::new(self.provider.clone())
        };

        // Build tool registry, then filter to agent's allowed tools
        let mut registry = (self.tool_registry_factory)();
        registry.retain(|name| config.agent_def.is_tool_allowed(name));

        // Build prompt builder for sub-agent via thinker pipeline
        let prompt_builder = PromptBuilder::new(PromptConfig::default())
            .with_agent(config.agent_def.clone());

        // Build loop config from agent definition
        let loop_config = LoopConfig {
            max_iterations: config.agent_def.max_iterations.unwrap_or(25) as usize,
            token_budget: config.agent_def.token_budget.unwrap_or(100_000) as usize,
        };

        // Create and run the agent loop
        let mut agent_loop = AgentLoop::new(
            bridge,
            registry,
            prompt_builder,
            (self.safety_guard_factory)(),
            loop_config,
            self.cancel_token.clone(),
        )
        .with_chain(self.child_chain.clone());

        // Prepend parent context to the task message if provided
        let effective_task = match &config.context_summary {
            Some(summary) => format!(
                "## Context from parent agent\n\n{}\n\n---\n\n{}",
                summary, config.task
            ),
            None => config.task.clone(),
        };

        let mut callback = NoopCallback;
        let timeout_duration = std::time::Duration::from_secs(config.timeout_secs);
        let run_result = tokio::time::timeout(
            timeout_duration,
            agent_loop.run(&effective_task, &mut callback),
        )
        .await;

        match run_result {
            Err(_elapsed) => Err(format!(
                "Sub-agent timed out after {}s",
                config.timeout_secs
            )),
            Ok(Ok(result)) => Ok(result),
            Ok(Err(e)) => Err(format!("sub-agent failed: {}", e)),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Truncate a string for log output, appending "..." if truncated.
fn truncate_for_log(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        // Use char_indices for UTF-8 safety (P7 defensive design)
        match s.char_indices().nth(max_len) {
            Some((idx, _)) => format!("{}...", &s[..idx]),
            None => s.to_string(),
        }
    }
}

/// Format a TranscriptOutcome for log output.
fn format_outcome(outcome: &TranscriptOutcome) -> &str {
    match outcome {
        TranscriptOutcome::Success => "success",
        TranscriptOutcome::Error(_) => "error",
        TranscriptOutcome::Timeout => "timeout",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

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
        let s = "hello";
        assert_eq!(truncate_for_log(s, 10), "hello");
    }

    #[test]
    fn truncate_for_log_long_string() {
        let s = "a".repeat(200);
        let result = truncate_for_log(&s, 50);
        assert!(result.ends_with("..."));
        // 50 chars + "..."
        assert_eq!(result.len(), 53);
    }

    #[test]
    fn truncate_for_log_unicode_safe() {
        // Chinese characters are multi-byte; ensure no panic on boundary
        let s = "你好世界这是一个很长的字符串用于测试截断功能";
        let result = truncate_for_log(s, 5);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn fork_path_uses_snapshot_prefix() {
        use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};

        let config = PromptConfig::default();
        let builder = PromptBuilder::new(config);
        let tools: Vec<crate::agent_loop::ToolInfo> = vec![];
        let snapshot = builder.capture_snapshot(&tools);

        // Build a minimal sub-agent AgentDef for the fork path
        let agent_def = AgentDef::new("fork-test-agent", crate::agents::AgentMode::SubAgent);

        let forked = builder.build_from_snapshot(&snapshot, &agent_def, &tools);

        // Fork result must start with the snapshot's stable prefix
        assert!(
            forked.starts_with(&snapshot.stable_prefix),
            "forked prompt must start with the snapshot's stable prefix"
        );
        // Fork result should include content beyond just the prefix
        assert!(forked.len() >= snapshot.stable_prefix.len());
    }
}
