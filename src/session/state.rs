//! `SessionState` — in-memory reducer over `SessionEvent`.
//!
//! Pure function of the event stream. Used by `SessionActor` during replay
//! and after each emitted event. Never persisted; always rebuilt from the
//! event log.

use std::collections::HashMap;

use crate::gateway::session_manager::SessionIdentityMeta;
use crate::session::events::{ApprovalSource, SessionEvent, TurnId, TurnOutcome};

#[derive(Debug, Default, Clone)]
pub struct SessionState {
    pub identity: Option<SessionIdentityMeta>,
    pub current_turn: Option<TurnState>,
    pub completed_turns: usize,
    pub tokens_used: u32,
    pub tokens_budget: u32,
    pub wake_count: u32,
}

#[derive(Debug, Clone)]
pub struct TurnState {
    pub id: TurnId,
    pub pending_tool_calls: HashMap<String, PendingToolCall>,
}

#[derive(Debug, Clone)]
pub struct PendingToolCall {
    pub name: String,
    pub approved: Option<ApprovalSource>,
}

impl SessionState {
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    pub fn apply(&mut self, event: &SessionEvent) {
        match event {
            SessionEvent::SessionCreated { identity, .. } => {
                // rust-doctor-disable-next-line excessive-clone
                self.identity = Some(identity.clone());
            }
            SessionEvent::SessionWoken { .. } => {
                self.wake_count += 1;
            }
            SessionEvent::SessionDetached { .. } => {
                // Detach is observational; no state mutation.
            }
            SessionEvent::SessionForked { .. } => {
                // Lineage metadata; no state mutation in parent.
            }
            SessionEvent::RunStarted { .. } => {
                // Run markers are observational; resume detection scans the
                // event log directly. No state mutation.
            }
            SessionEvent::RunFinished { .. } => {
                // See RunStarted.
            }

            SessionEvent::TurnStarted { turn_id, .. } => {
                self.current_turn = Some(TurnState {
                    id: *turn_id,
                    pending_tool_calls: HashMap::new(),
                });
            }
            SessionEvent::TurnEnded {
                turn_id, outcome, ..
            } => {
                if self
                    .current_turn
                    .as_ref()
                    .is_some_and(|turn| turn.id == *turn_id)
                {
                    if matches!(outcome, TurnOutcome::Completed) {
                        self.completed_turns += 1;
                    }
                    self.current_turn = None;
                }
            }

            SessionEvent::UserMessage { .. } => {
                // Messages are preserved in the event log and materialized
                // for UI via the projection layer — no state mutation.
            }
            SessionEvent::AssistantMessage { .. } => {
                // See UserMessage.
            }
            SessionEvent::AssistantRunMeta { .. } => {
                // Observational; metadata persistence is handled by the projector.
            }
            SessionEvent::SystemMessage { .. } => {
                // See UserMessage.
            }

            SessionEvent::ToolCallRequested {
                turn_id,
                call_id,
                name,
                ..
            } => {
                if let Some(turn) = self
                    .current_turn
                    .as_mut()
                    .filter(|turn| turn.id == *turn_id)
                {
                    turn.pending_tool_calls.insert(
                        call_id.clone(),
                        PendingToolCall {
                            name: name.clone(),
                            approved: None,
                        },
                    );
                }
            }
            SessionEvent::ToolCallApproved {
                turn_id,
                call_id,
                by,
                ..
            } => {
                if let Some(turn) = self
                    .current_turn
                    .as_mut()
                    .filter(|turn| turn.id == *turn_id)
                {
                    if let Some(pc) = turn.pending_tool_calls.get_mut(call_id) {
                        pc.approved = Some(*by);
                    }
                }
            }
            SessionEvent::ToolCallDenied {
                turn_id, call_id, ..
            } => {
                if let Some(turn) = self
                    .current_turn
                    .as_mut()
                    .filter(|turn| turn.id == *turn_id)
                {
                    turn.pending_tool_calls.remove(call_id);
                }
            }
            SessionEvent::ToolResult {
                turn_id, call_id, ..
            } => {
                if let Some(turn) = self
                    .current_turn
                    .as_mut()
                    .filter(|turn| turn.id == *turn_id)
                {
                    turn.pending_tool_calls.remove(call_id);
                }
            }
            SessionEvent::ToolError {
                turn_id, call_id, ..
            } => {
                if let Some(turn) = self
                    .current_turn
                    .as_mut()
                    .filter(|turn| turn.id == *turn_id)
                {
                    turn.pending_tool_calls.remove(call_id);
                }
            }

            SessionEvent::SubagentSpawned { .. } => {
                // Tracked via events; no parent-state mutation needed in Phase 1.
            }
            SessionEvent::SubagentReturned { .. } => {
                // Tracked via events; no parent-state mutation needed in Phase 1.
            }

            SessionEvent::BudgetUpdated {
                tokens_used,
                tokens_budget,
                ..
            } => {
                self.tokens_used = self.tokens_used.max(*tokens_used);
                self.tokens_budget = *tokens_budget;
            }
            SessionEvent::CompactionPerformed { .. } => {
                // Compaction's effect on state is encoded in the summary; state itself
                // is not truncated because replay is still from event seq 0.
            }

            SessionEvent::Error { .. } => {
                // Errors are observational; recovery logic is at the Harness layer.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::{now_ms, MessageContent, ToolOutput, TurnTrigger};

    fn turn_started(tid: TurnId) -> SessionEvent {
        SessionEvent::TurnStarted {
            turn_id: tid,
            trigger: TurnTrigger::UserMessage,
            at: now_ms(),
        }
    }

    fn turn_ended_completed(tid: TurnId) -> SessionEvent {
        SessionEvent::TurnEnded {
            turn_id: tid,
            outcome: TurnOutcome::Completed,
            at: now_ms(),
        }
    }

    #[test]
    fn fresh_state_has_no_turn() {
        let s = SessionState::default();
        assert!(s.current_turn.is_none());
        assert_eq!(s.completed_turns, 0);
    }

    #[test]
    fn turn_started_sets_current_turn() {
        let mut s = SessionState::default();
        let tid = uuid::Uuid::new_v4();
        s.apply(&turn_started(tid));
        assert_eq!(s.current_turn.as_ref().unwrap().id, tid);
    }

    #[test]
    fn turn_ended_completed_increments_counter_and_clears_current() {
        let mut s = SessionState::default();
        let tid = uuid::Uuid::new_v4();
        s.apply(&turn_started(tid));
        s.apply(&turn_ended_completed(tid));
        assert!(s.current_turn.is_none());
        assert_eq!(s.completed_turns, 1);
    }

    #[test]
    fn stale_turn_end_does_not_clear_current_turn_or_count() {
        let mut s = SessionState::default();
        let first = uuid::Uuid::new_v4();
        let current = uuid::Uuid::new_v4();
        s.apply(&turn_started(first));
        s.apply(&turn_started(current));
        s.apply(&turn_ended_completed(first));
        assert_eq!(s.current_turn.as_ref().map(|turn| turn.id), Some(current));
        assert_eq!(s.completed_turns, 0);
    }

    #[test]
    fn duplicate_turn_end_does_not_count_twice() {
        let mut s = SessionState::default();
        let tid = uuid::Uuid::new_v4();
        let event = turn_ended_completed(tid);
        s.apply(&turn_started(tid));
        s.apply(&event);
        s.apply(&event);
        assert_eq!(s.completed_turns, 1);
    }

    #[test]
    fn tool_events_are_isolated_by_turn_id() {
        let mut s = SessionState::default();
        let first = uuid::Uuid::new_v4();
        let current = uuid::Uuid::new_v4();
        s.apply(&turn_started(first));
        s.apply(&SessionEvent::ToolCallRequested {
            turn_id: first,
            call_id: "c1".into(),
            name: "bash_exec".into(),
            input: serde_json::json!({}),
            at: now_ms(),
        });
        s.apply(&turn_started(current));
        s.apply(&SessionEvent::ToolResult {
            turn_id: first,
            call_id: "c1".into(),
            output: ToolOutput {
                value: serde_json::json!("ok"),
                metadata: Default::default(),
            },
            at: now_ms(),
        });
        assert!(s
            .current_turn
            .as_ref()
            .is_some_and(|turn| turn.id == current));
        assert!(s
            .current_turn
            .as_ref()
            .is_some_and(|turn| turn.pending_tool_calls.is_empty()));
    }

    #[test]
    fn turn_ended_cancelled_does_not_increment_completed_turns() {
        let mut s = SessionState::default();
        let tid = uuid::Uuid::new_v4();
        s.apply(&turn_started(tid));
        s.apply(&SessionEvent::TurnEnded {
            turn_id: tid,
            outcome: TurnOutcome::Cancelled,
            at: now_ms(),
        });
        assert!(s.current_turn.is_none());
        assert_eq!(s.completed_turns, 0);
    }

    #[test]
    fn turn_ended_errored_does_not_increment_completed_turns() {
        let mut s = SessionState::default();
        let tid = uuid::Uuid::new_v4();
        s.apply(&turn_started(tid));
        s.apply(&SessionEvent::TurnEnded {
            turn_id: tid,
            outcome: TurnOutcome::Errored {
                kind: crate::session::events::ErrorKind::Tool,
            },
            at: now_ms(),
        });
        assert!(s.current_turn.is_none());
        assert_eq!(s.completed_turns, 0);
    }

    #[test]
    fn tool_call_lifecycle_tracks_pending() {
        let mut s = SessionState::default();
        let tid = uuid::Uuid::new_v4();
        s.apply(&turn_started(tid));
        s.apply(&SessionEvent::ToolCallRequested {
            turn_id: tid,
            call_id: "c1".into(),
            name: "bash_exec".into(),
            input: serde_json::json!({}),
            at: now_ms(),
        });
        assert_eq!(s.current_turn.as_ref().unwrap().pending_tool_calls.len(), 1);

        s.apply(&SessionEvent::ToolResult {
            turn_id: tid,
            call_id: "c1".into(),
            output: ToolOutput {
                value: serde_json::json!("ok"),
                metadata: Default::default(),
            },
            at: now_ms(),
        });
        assert_eq!(s.current_turn.as_ref().unwrap().pending_tool_calls.len(), 0);
    }

    #[test]
    fn replay_is_deterministic() {
        let mut s1 = SessionState::default();
        let mut s2 = SessionState::default();
        let tid = uuid::Uuid::new_v4();
        let events = vec![
            turn_started(tid),
            SessionEvent::UserMessage {
                turn_id: tid,
                content: MessageContent {
                    text: "hi".into(),
                    blocks: vec![],
                    thinking: None,
                    thinking_signature: None,
                },
                at: now_ms(),
                synthetic: false,
            },
            turn_ended_completed(tid),
        ];
        for ev in &events {
            s1.apply(ev);
        }
        for ev in &events {
            s2.apply(ev);
        }
        assert_eq!(s1.completed_turns, s2.completed_turns);
        assert_eq!(s1.current_turn.is_none(), s2.current_turn.is_none());
    }

    #[test]
    fn wake_count_increments() {
        let mut s = SessionState::default();
        s.apply(&SessionEvent::SessionWoken {
            at: now_ms(),
            prior_head: 10,
        });
        s.apply(&SessionEvent::SessionWoken {
            at: now_ms(),
            prior_head: 20,
        });
        assert_eq!(s.wake_count, 2);
    }

    #[test]
    fn budget_updated_is_absolute() {
        let mut s = SessionState::default();
        let tid = uuid::Uuid::new_v4();
        s.apply(&SessionEvent::BudgetUpdated {
            turn_id: tid,
            tokens_used: 100,
            tokens_budget: 4000,
            at: now_ms(),
        });
        assert_eq!(s.tokens_used, 100);
        s.apply(&SessionEvent::BudgetUpdated {
            turn_id: tid,
            tokens_used: 250,
            tokens_budget: 4000,
            at: now_ms(),
        });
        assert_eq!(s.tokens_used, 250);
    }

    #[test]
    fn budget_updated_is_monotonic() {
        let mut s = SessionState::default();
        let tid = uuid::Uuid::new_v4();
        s.apply(&SessionEvent::BudgetUpdated {
            turn_id: tid,
            tokens_used: 100,
            tokens_budget: 4000,
            at: now_ms(),
        });
        s.apply(&SessionEvent::BudgetUpdated {
            turn_id: tid,
            tokens_used: 20,
            tokens_budget: 4000,
            at: now_ms(),
        });
        assert_eq!(s.tokens_used, 100);
    }

    #[test]
    fn run_markers_are_no_op_projections() {
        use crate::session::events::RunOutcome;
        let mut s = SessionState::default();
        let before_turns = s.completed_turns;
        s.apply(&SessionEvent::RunStarted {
            run_id: "run-1".into(),
            at: now_ms(),
            project_root: None,
        });
        s.apply(&SessionEvent::RunFinished {
            run_id: "run-1".into(),
            outcome: RunOutcome::Completed,
            at: now_ms(),
        });
        assert!(s.current_turn.is_none());
        assert_eq!(s.completed_turns, before_turns);
        assert_eq!(s.wake_count, 0);
    }
}
