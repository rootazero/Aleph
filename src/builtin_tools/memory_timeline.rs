//! `memory_timeline` tool — view the complete lifecycle of a memory fact.
//!
//! Wraps [`MemoryTimeTraveler::explain_fact`] to provide a human-readable
//! timeline of creation, modification, decay, and invalidation events.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::error::ToolError;
use crate::error::Result;
use crate::memory::events::traveler::MemoryTimeTraveler;
use crate::memory::explain::FactExplanation;
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

// ── Args / Output ───────────────────────────────────────────────────────────

/// Arguments for the `memory_timeline` tool
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct MemoryTimelineArgs {
    /// The fact ID to inspect
    pub fact_id: String,
}

/// Output from the `memory_timeline` tool
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryTimelineOutput {
    /// The full lifecycle explanation of the fact
    pub explanation: FactExplanation,
}

// ── Tool struct ─────────────────────────────────────────────────────────────

/// View the complete lifecycle of a memory fact
pub struct MemoryTimelineTool {
    traveler: Arc<MemoryTimeTraveler>,
}

impl MemoryTimelineTool {
    #[must_use]
    pub const fn new(traveler: Arc<MemoryTimeTraveler>) -> Self {
        Self { traveler }
    }

    /// Internal implementation
    async fn call_impl(
        &self,
        args: MemoryTimelineArgs,
    ) -> std::result::Result<MemoryTimelineOutput, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        // BT-D-R4-05 (partial fix): validate the fact_id format up-front.
        // `memory_events` has no agent/partition column at all, so there is
        // no per-agent authorization to add at the traveler — any caller
        // that learns or guesses another corpus's fact id can read its
        // lifecycle and current content; this validation bounds the input
        // surface but does not close that gap. Closing it needs a real
        // schema-level partition column and is tracked as a separate
        // change, not something fixable at this call site.
        let fact_id = args.fact_id.trim();
        if fact_id.is_empty() {
            return Err(ToolError::InvalidArgs(
                "memory_timeline requires a non-empty fact_id".to_string(),
            ));
        }
        if fact_id.len() > 256 {
            return Err(ToolError::InvalidArgs(format!(
                "fact_id is {} bytes; max 256",
                fact_id.len()
            )));
        }
        if fact_id.chars().any(|c| {
            c.is_whitespace() || c.is_control() || c == '/' || c == '\\' || c == '`' || c == '$'
        }) {
            return Err(ToolError::InvalidArgs(
                "fact_id contains an invalid character (whitespace, control, /, \\, `, or $)"
                    .to_string(),
            ));
        }

        let args_summary = format!("fact timeline: {}", &fact_id);
        notify_tool_start(Self::NAME, &args_summary);

        let explanation = self
            .traveler
            .explain_fact(fact_id)
            .await
            .map_err(|e| ToolError::Execution(format!("Failed to explain fact: {e}")))?;

        notify_tool_result(
            Self::NAME,
            &format!("fact_id={}, valid={}", fact_id, explanation.is_valid),
            true,
        );

        Ok(MemoryTimelineOutput { explanation })
    }
}

impl Clone for MemoryTimelineTool {
    fn clone(&self) -> Self {
        Self {
            traveler: self.traveler.clone(),
        }
    }
}

// ── AlephTool impl ──────────────────────────────────────────────────────────

#[async_trait]
impl AlephTool for MemoryTimelineTool {
    const NAME: &'static str = "memory_timeline";
    const DESCRIPTION: &'static str =
        "View the complete lifecycle of a memory fact — creation, modification, \
         decay, invalidation timeline. Use when you need to understand why a \
         fact changed or was invalidated.";

    type Args = MemoryTimelineArgs;
    type Output = MemoryTimelineOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.call_impl(args).await.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactSource, NoteType};
    use crate::memory::events::{EventActor, MemoryEvent, MemoryEventEnvelope};
    use crate::resilience::database::StateDatabase;
    use crate::routing::session_key::SessionKey;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};

    #[test]
    fn test_args_deserialization() {
        let json = r#"{"fact_id": "abc-123"}"#;
        let args: MemoryTimelineArgs = serde_json::from_str(json).unwrap();
        assert_eq!(args.fact_id, "abc-123");
    }

    /// A turn scope with a real (non-empty) agent id, the shape
    /// `ScopedToolService::execute` builds for every in-turn tool call.
    fn turn(agent: &str) -> TurnContext {
        TurnContext {
            session_key: SessionKey::Main {
                agent_id: agent.to_string(),
                main_key: crate::routing::session_key::DEFAULT_MAIN_KEY.to_string(),
                epoch: 0,
            },
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
        }
    }

    fn created_event(fact_id: &str) -> MemoryEventEnvelope {
        MemoryEventEnvelope::new(
            fact_id.into(),
            1,
            MemoryEvent::NoteCreated {
                note_path: fact_id.into(),
                content: "User prefers Rust".into(),
                note_type: NoteType::Preference,
                path: "aleph://user/preferences/language".into(),
                namespace: "owner".into(),
                agent: "default".into(),
                source: FactSource::Extracted,
                source_memory_ids: vec![],
            },
            EventActor::Agent,
            None,
        )
    }

    /// Reachability: `memory_timeline` is called from inside a turn, and
    /// every in-turn call is scoped by `ScopedToolService::execute` — so a
    /// fact with real event history must come back, not read as "no
    /// history". Before the fix, `explain_fact` was called with
    /// `acting_agent_id("")`, which resolves to the turn's real agent id
    /// (e.g. "main") from inside a scope — never the empty-string wildcard
    /// — while the `actor` column only ever holds
    /// `{agent,user,system,decay,migration}`. `actor = 'main'` matched
    /// nothing and the tool reported "No events found" for a fact that had
    /// events all along.
    #[tokio::test]
    async fn timeline_is_reachable_from_inside_a_scoped_turn() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let fact_id = "fact-scoped-1";
        db.append_memory_event(&created_event(fact_id))
            .await
            .unwrap();

        let traveler = Arc::new(MemoryTimeTraveler::new(db));
        let tool = MemoryTimelineTool::new(traveler);

        let result = TURN_CONTEXT
            .scope(turn("main"), async {
                tool.call(MemoryTimelineArgs {
                    fact_id: fact_id.to_string(),
                })
                .await
            })
            .await;

        let output = result.unwrap_or_else(|e| {
            panic!("a fact with events must produce a timeline, not an error: {e}")
        });
        assert_eq!(output.explanation.fact_id, fact_id);
        assert_eq!(output.explanation.events.len(), 1);
    }

    /// Same reachability failure, pinned to the exact misleading error text
    /// a caller would have seen: the wildcard-vs-actor mismatch surfaced as
    /// "No events found for fact X", which reads as "this fact has no
    /// history" rather than "the filter excluded every row".
    #[tokio::test]
    async fn no_events_found_error_does_not_leak_for_a_fact_with_events() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let fact_id = "fact-scoped-2";
        db.append_memory_event(&created_event(fact_id))
            .await
            .unwrap();

        let traveler = Arc::new(MemoryTimeTraveler::new(db));
        let tool = MemoryTimelineTool::new(traveler);

        let result = TURN_CONTEXT
            .scope(turn("main"), async {
                tool.call(MemoryTimelineArgs {
                    fact_id: fact_id.to_string(),
                })
                .await
            })
            .await;

        if let Err(e) = result {
            assert!(
                !e.to_string().contains("No events found"),
                "a fact that has events must not surface the empty-history error: {e}"
            );
        }
    }
}
