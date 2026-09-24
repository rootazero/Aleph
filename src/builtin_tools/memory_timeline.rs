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
use crate::memory::events::UnpartitionedRows;
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
    /// The caller's read set, bound per call by the registry's dispatch arm
    /// ([`Self::for_caller`]). `None` on the boot-built instance: a timeline
    /// read that did not come through the arm has no partition to read, and
    /// says so rather than answering "no events" for a fact that has them.
    read_scope: Option<(Vec<String>, UnpartitionedRows)>,
}

impl MemoryTimelineTool {
    #[must_use]
    pub const fn new(traveler: Arc<MemoryTimeTraveler>) -> Self {
        Self {
            traveler,
            read_scope: None,
        }
    }

    /// Bind this call to the caller's partitions (the registry arm's
    /// `caller_memory_read_partitions` + `unattributed_memory_events_for`).
    #[must_use]
    pub fn for_caller(mut self, partitions: Vec<String>, unpartitioned: UnpartitionedRows) -> Self {
        self.read_scope = Some((partitions, unpartitioned));
        self
    }

    /// Internal implementation
    async fn call_impl(
        &self,
        args: MemoryTimelineArgs,
    ) -> std::result::Result<MemoryTimelineOutput, ToolError> {
        use super::{notify_tool_result, notify_tool_start};

        // Format validation bounds the input surface only. WHOSE fact this is
        // is decided by the partition filter the dispatch arm bound
        // (`for_caller`). `fact_id` is only ever a bound SQL parameter, never a
        // path, so `/` guards nothing — and every stream the note tools write
        // is keyed `category/filename`, so refusing it made this face unable to
        // read any of them.
        let Some((partitions, unpartitioned)) = self.read_scope.as_ref() else {
            return Err(ToolError::Execution(
                "memory_timeline is not bound to a caller's partitions; it must be dispatched \
                 through the builtin tool registry"
                    .to_string(),
            ));
        };
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
        if fact_id.chars().any(char::is_control) {
            return Err(ToolError::InvalidArgs(
                "fact_id contains a control character".to_string(),
            ));
        }

        let args_summary = format!("fact timeline: {}", &fact_id);
        notify_tool_start(Self::NAME, &args_summary);

        let explanation = self
            .traveler
            .explain_fact(fact_id, partitions, *unpartitioned)
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
            read_scope: self.read_scope.clone(),
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
        .in_partition(Some("main".into()))
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
        let tool = MemoryTimelineTool::new(traveler)
            .for_caller(vec!["main".into()], UnpartitionedRows::Refuse);

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
        let tool = MemoryTimelineTool::new(traveler)
            .for_caller(vec!["main".into()], UnpartitionedRows::Refuse);

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

    /// A note-path id (`category/filename`, the key every `note_manage`
    /// stream carries) reaches the partition filter: the caller whose
    /// partition holds it reads it, another caller gets exactly the answer a
    /// never-written id gets. Control characters are still refused.
    #[tokio::test]
    async fn a_note_path_fact_id_is_read_through_the_partition_filter() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        let id = "learning/rust-pref";
        db.append_memory_event(&created_event(id).in_partition(Some("main__u-alice".into())))
            .await
            .unwrap();
        let traveler = Arc::new(MemoryTimeTraveler::new(db));
        let read_as = |who: &str, fact_id: &str| {
            let tool = MemoryTimelineTool::new(Arc::clone(&traveler)).for_caller(
                vec!["main".into(), format!("main__{who}")],
                UnpartitionedRows::Refuse,
            );
            let args = MemoryTimelineArgs {
                fact_id: fact_id.to_string(),
            };
            async move { tool.call(args).await }
        };

        let alice = read_as("u-alice", id)
            .await
            .expect("Alice reads her own note");
        assert_eq!(alice.explanation.events.len(), 1);

        let bob = read_as("u-bob", id)
            .await
            .expect_err("Bob must not read Alice's note")
            .to_string();
        let never = read_as("u-bob", "learning/never-written")
            .await
            .expect_err("a never-written id has no history")
            .to_string();
        assert_eq!(
            bob.replace(id, "<id>"),
            never.replace("learning/never-written", "<id>")
        );

        let control = read_as("u-alice", "learning/rust\u{7}pref")
            .await
            .expect_err("a control character is refused")
            .to_string();
        assert!(control.contains("control character"), "{control}");
    }

    /// The boot-built instance has no caller: it must say so, not answer
    /// "No events found" for a fact that has events (判据 §8).
    #[tokio::test]
    async fn an_unbound_timeline_refuses_instead_of_reporting_no_history() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        db.append_memory_event(&created_event("fact-unbound"))
            .await
            .unwrap();
        let tool = MemoryTimelineTool::new(Arc::new(MemoryTimeTraveler::new(db)));
        let text = tool
            .call(MemoryTimelineArgs {
                fact_id: "fact-unbound".into(),
            })
            .await
            .expect_err("an unbound tool has no partition to read")
            .to_string();
        assert!(!text.contains("No events found"), "{text}");
        assert!(text.contains("not bound to a caller"), "{text}");
    }
}
