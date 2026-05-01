//! SessionState — in-memory reducer over SessionEvent.
//!
//! Pure function of the event stream. Used by SessionActor during replay
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
    pub fn apply(&mut self, event: &SessionEvent) {
        match event {
            SessionEvent::SessionCreated { identity, .. } => {
                self.identity = Some(identity.clone());
            }
            SessionEvent::SessionWoken { .. } => {
                self.wake_count += 1;
            }
            SessionEvent::SessionDetached { .. } => {
                // Detach is observational; no state mutation.
            }

            SessionEvent::TurnStarted { turn_id, .. } => {
                self.current_turn = Some(TurnState {
                    id: *turn_id,
                    pending_tool_calls: HashMap::new(),
                });
            }
            SessionEvent::TurnEnded { outcome, .. } => {
                if matches!(outcome, TurnOutcome::Completed) {
                    self.completed_turns += 1;
                }
                self.current_turn = None;
            }

            SessionEvent::UserMessage { .. } => {
                // Messages are preserved in the event log and materialized
                // for UI via the projection layer — no state mutation.
            }
            SessionEvent::AssistantMessage { .. } => {
                // See UserMessage.
            }
            SessionEvent::SystemMessage { .. } => {
                // See UserMessage.
            }

            SessionEvent::LlmCallStarted { .. } => {
                // Observational; budget tracking happens via BudgetUpdated.
            }
            SessionEvent::LlmCallEnded { .. } => {
                // Observational; budget tracking happens via BudgetUpdated.
            }

            SessionEvent::ToolCallRequested { call_id, name, .. } => {
                if let Some(turn) = self.current_turn.as_mut() {
                    turn.pending_tool_calls.insert(
                        call_id.clone(),
                        PendingToolCall {
                            name: name.clone(),
                            approved: None,
                        },
                    );
                }
            }
            SessionEvent::ToolCallApproved { call_id, by, .. } => {
                if let Some(turn) = self.current_turn.as_mut() {
                    if let Some(pc) = turn.pending_tool_calls.get_mut(call_id) {
                        pc.approved = Some(by.clone());
                    }
                }
            }
            SessionEvent::ToolCallDenied { call_id, .. } => {
                if let Some(turn) = self.current_turn.as_mut() {
                    turn.pending_tool_calls.remove(call_id);
                }
            }
            SessionEvent::ToolResult { call_id, .. } => {
                if let Some(turn) = self.current_turn.as_mut() {
                    turn.pending_tool_calls.remove(call_id);
                }
            }
            SessionEvent::ToolError { call_id, .. } => {
                if let Some(turn) = self.current_turn.as_mut() {
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
                self.tokens_used = *tokens_used;
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
                },
                at: now_ms(),
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
}
