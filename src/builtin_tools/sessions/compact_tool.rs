//! Session compact tool — summarize this conversation's older turns and drop
//! them from the live context.
//!
//! Tool form of the `/compact` (alias `/compress`) slash command and the
//! `session.compact` RPC; all three now drive the same
//! [`compact_session`](crate::context::compact::manual::compact_session), so a
//! `/compact` typed in the Panel, sent through a channel, issued from the TUI,
//! or called by the model itself (R8) does exactly one thing.
//!
//! Not a deletion: the summarized turns are soft-retired from the event log the
//! prompt is rebuilt from, while the rows themselves — and their BM25 index —
//! survive, so `recall_events` can still surface a detail the summary
//! abstracted away, `/undo` and export still see the full history, and the
//! Panel keeps its scrollback.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::context::compact::manual::{ManualCompactOptions, ManualCompactOutcome};
use crate::error::Result;
use crate::tools::AlephTool;

/// Arguments for the `session_compact` tool
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
pub struct SessionCompactArgs {
    /// Injected by registry — serialized session key (internal, hidden from LLM schema)
    #[serde(default)]
    #[schemars(skip)]
    pub __session_key: String,

    /// Optional focus for the summary, e.g. "keep every detail about the
    /// migration script, drop the debugging tangents". Free text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// Output from `session_compact` tool
#[derive(Debug, Clone, Serialize)]
pub struct SessionCompactOutput {
    /// Conversational events folded into the summary
    pub compacted: usize,
    /// Events still replayed verbatim
    pub kept: usize,
    /// Estimated prompt tokens removed from every future turn
    pub tokens_saved: usize,
    /// Human-readable status message
    pub message: String,
}

/// Tool that compacts the current conversation's live context.
#[derive(Clone, Default)]
pub struct SessionCompactTool;

impl SessionCompactTool {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AlephTool for SessionCompactTool {
    const NAME: &'static str = "session_compact";
    const DESCRIPTION: &'static str =
        "Compact this conversation: summarize the older turns into a single summary and stop \
         replaying them in full, keeping the most recent turns verbatim. Frees context without \
         losing the thread — the summarized turns stay searchable and stay in the user's \
         transcript. Pass `instructions` to steer what the summary must preserve. Use when the \
         user asks to compact, compress, or shorten the conversation.";

    type Args = SessionCompactArgs;
    type Output = SessionCompactOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "session_compact()".to_string(),
            "session_compact(instructions=\"keep the API design decisions, drop the build errors\")"
                .to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let outcome = run_manual_compaction(
            &args.__session_key,
            ManualCompactOptions {
                instructions: args.instructions,
                keep_tokens: None,
            },
        )
        .await?;
        Ok(render(&outcome))
    }
}

/// Drive a manual compaction for `session_key_str`, resolving the process-wide
/// session service / event store / summarizer.
///
/// Shared by this tool and the `session.compact` RPC handler so the two
/// surfaces cannot drift (R6): both resolve the same collaborators, apply the
/// same clamps, and report the same numbers.
pub async fn run_manual_compaction(
    session_key_str: &str,
    opts: ManualCompactOptions,
) -> Result<ManualCompactOutcome> {
    use crate::error::AlephError;

    if session_key_str.is_empty() {
        return Err(AlephError::tool(
            "session_compact: no session context available (session key not injected)",
        ));
    }
    let session_id = crate::gateway::router::SessionKey::from_key_string(session_key_str)
        .ok_or_else(|| {
            AlephError::tool(format!(
                "session_compact: failed to parse session key '{session_key_str}'"
            ))
        })?;

    let service = crate::session::service::global_session_service().ok_or_else(|| {
        AlephError::tool("session_compact: session service unavailable (daemon not initialised)")
    })?;
    let store = crate::session::store::global_session_event_store()
        .ok_or_else(|| AlephError::tool("session_compact: session event store unavailable"))?;

    // The summarizer is optional by design: without a provider the compaction
    // still runs, falling back to the same deterministic truncation the
    // automatic path uses when its LLM call fails.
    let compactor = crate::context::compact::manual::manual_summarizer().map(|provider| {
        crate::context::compact::compactor::ContextCompactor::new(
            provider,
            crate::context::compact::compactor::CompactorConfig::default(),
        )
    });

    crate::context::compact::manual::compact_session(
        service.as_ref(),
        store.as_ref(),
        compactor.as_ref(),
        &session_id,
        &opts,
    )
    .await
    .map_err(|e| AlephError::tool(format!("session_compact: compaction failed: {e}")))
}

/// Render an outcome into the tool's user-facing output shape.
#[must_use]
pub fn render(outcome: &ManualCompactOutcome) -> SessionCompactOutput {
    let message = if outcome.compacted {
        format!(
            "🗜 Compacted {} earlier message(s) into a summary; kept the {} most recent verbatim (~{} tokens freed per turn).",
            outcome.events_compacted,
            outcome.events_kept,
            outcome.tokens_saved()
        )
    } else {
        outcome.skipped_reason.clone().map_or_else(
            || "Nothing to compact.".to_string(),
            |reason| format!("Nothing to compact — {reason}."),
        )
    };
    SessionCompactOutput {
        compacted: outcome.events_compacted,
        kept: outcome.events_kept,
        tokens_saved: outcome.tokens_saved(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::AlephTool;

    #[test]
    fn test_tool_definition() {
        let tool = SessionCompactTool::new();
        let def = AlephTool::definition(&tool);

        assert_eq!(def.name, "session_compact");
        assert!(!def.requires_confirmation);
    }

    #[test]
    fn instructions_are_part_of_the_model_facing_schema() {
        // `/compact <instructions>` is only reachable if the model (and the
        // slash arg mapper) can see the field.
        let tool = SessionCompactTool::new();
        let def = AlephTool::definition(&tool);
        let schema = serde_json::to_string(&def.parameters).unwrap();
        assert!(
            schema.contains("instructions"),
            "instructions must be exposed: {schema}"
        );
        assert!(
            !schema.contains("__session_key"),
            "the injected session key must stay hidden: {schema}"
        );
    }

    #[tokio::test]
    async fn test_empty_session_key_errors() {
        let tool = SessionCompactTool::new();
        let result = tool.call(SessionCompactArgs::default()).await;
        assert!(result.is_err());
    }

    #[test]
    fn skipped_outcome_renders_its_reason_and_zero_saving() {
        let out = render(&ManualCompactOutcome {
            compacted: false,
            events_compacted: 0,
            events_kept: 4,
            tokens_before: 0,
            tokens_after: 0,
            summary: String::new(),
            skipped_reason: Some("conversation already fits the verbatim tail budget".into()),
        });
        assert_eq!(out.tokens_saved, 0);
        assert!(out.message.contains("Nothing to compact"));
        assert!(out.message.contains("verbatim tail budget"));
    }

    #[test]
    fn compacted_outcome_reports_the_measured_saving_not_a_per_message_guess() {
        // The old RPC reported `deleted * 50`. This asserts the number comes
        // from the measured before/after, so that fabrication cannot come back.
        let out = render(&ManualCompactOutcome {
            compacted: true,
            events_compacted: 12,
            events_kept: 6,
            tokens_before: 9_000,
            tokens_after: 700,
            summary: "…".into(),
            skipped_reason: None,
        });
        assert_eq!(out.tokens_saved, 8_300);
        assert_eq!(out.compacted, 12);
        assert_eq!(out.kept, 6);
        assert!(out.message.contains("8300"));
    }
}
