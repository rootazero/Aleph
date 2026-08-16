//! Wire-format conversions: loop trace events → the gateway's `AgentTrace*`
//! event protocol.
//!
//! These `From` impls used to sit at the bottom of `src/harness/trace.rs`, which
//! made the Think→Act loop a consumer of `aleph_protocol` — serialisation for a
//! transport the loop knows nothing about. R10 caps the harness at scaffolding
//! for the loop; wire format is not that. Every consumer of these impls is in
//! this module tree (`execution_engine::callback`, `event_emitter`), so they
//! live next to their callers, and `src/harness/` no longer references
//! `aleph_protocol` at all.
//!
//! The orphan rule is satisfied crate-wide, not module-wide: `LoopTrace*` are
//! local to `alephcore` and appear as the trait's type parameter, so these impls
//! on the foreign `aleph_protocol` types are legal from any module in the crate.

use crate::harness::trace::{
    LoopTraceEvent, LoopTraceSessionOutcome, LoopTraceState, LoopTraceTextKind,
    LoopTraceTurnMetrics, LoopTraceTurnOutcome,
};

impl From<LoopTraceEvent> for aleph_protocol::AgentTraceEvent {
    fn from(event: LoopTraceEvent) -> Self {
        match event {
            LoopTraceEvent::TextEmitted {
                iteration,
                stream,
                text,
            } => Self::TextEmitted {
                iteration,
                stream: stream.into(),
                text,
            },
            LoopTraceEvent::ToolCallStarted { iteration, call } => Self::ToolCallStarted {
                iteration,
                call: aleph_protocol::AgentTraceToolCallStart {
                    tool_id: call.tool_id,
                    tool_name: call.tool_name,
                    input: call.input,
                },
            },
            LoopTraceEvent::ToolCallCompleted {
                iteration,
                call,
                result,
            } => Self::ToolCallCompleted {
                iteration,
                call: aleph_protocol::AgentTraceToolCallEnd {
                    tool_id: call.tool_id,
                    tool_name: call.tool_name,
                    input: call.input,
                    duration_ms: call.duration_ms,
                },
                result: match result {
                    crate::tools::runtime::ToolResult::Success { output } => {
                        aleph_protocol::AgentTraceToolResult::Success { output }
                    }
                    crate::tools::runtime::ToolResult::Error { error, retryable } => {
                        aleph_protocol::AgentTraceToolResult::Error { error, retryable }
                    }
                },
            },
            LoopTraceEvent::TurnStarted { iteration } => Self::TurnStarted { iteration },
            LoopTraceEvent::TurnStateEntered { iteration, state } => Self::TurnStateEntered {
                iteration,
                state: state.into(),
            },
            LoopTraceEvent::TurnCompleted {
                iteration,
                outcome,
                metrics,
            } => Self::TurnCompleted {
                iteration,
                outcome: outcome.into(),
                metrics: metrics.into(),
            },
            LoopTraceEvent::SessionCompleted {
                outcome,
                iterations,
                tool_calls_made,
                total_tokens,
                hit_limit,
                final_text,
                terminate_reason,
                duration_ms,
                token_breakdown,
                tool_timeline,
            } => Self::SessionCompleted {
                outcome: outcome.into(),
                iterations,
                tool_calls_made,
                total_tokens,
                hit_limit,
                final_text,
                // Re-serialize through JSON so the protocol crate stays
                // independent of `alephcore` types — keeps the crate-edge
                // boundary clean (R3 core minimalism).
                terminate_reason: terminate_reason
                    .as_ref()
                    .and_then(|r| serde_json::to_value(r).ok()),
                duration_ms,
                token_breakdown: token_breakdown
                    .as_ref()
                    .and_then(|b| serde_json::to_value(b).ok()),
                tool_timeline: tool_timeline
                    .iter()
                    .filter_map(|inv| serde_json::to_value(inv).ok())
                    .collect(),
            },
            LoopTraceEvent::WorktreeCreated { path } => Self::WorktreeCreated { path },
            LoopTraceEvent::WorktreeCleanedUp { path, leaked } => {
                Self::WorktreeCleanedUp { path, leaked }
            }
            LoopTraceEvent::McpScopeAttached {
                agent_id,
                references,
                inline_count,
            } => Self::McpScopeAttached {
                agent_id,
                references,
                inline_count,
            },
            LoopTraceEvent::McpScopeCleaned { agent_id, leaked } => {
                Self::McpScopeCleaned { agent_id, leaked }
            }
            LoopTraceEvent::ProviderUsage {
                agent_id,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                thinking_tokens,
            } => Self::ProviderUsage {
                agent_id,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                thinking_tokens,
            },
            LoopTraceEvent::ReactiveCompactionAttempted {
                token_gap,
                succeeded,
            } => Self::ReactiveCompactionAttempted {
                token_gap,
                succeeded,
            },
            LoopTraceEvent::VerifierVeto { iteration, reason } => {
                Self::VerifierVeto { iteration, reason }
            }
            LoopTraceEvent::MoaAdvisor {
                index,
                count,
                label,
                text,
                error,
            } => Self::MoaAdvisor {
                index,
                count,
                label,
                text,
                error,
            },
            LoopTraceEvent::MoaAggregating {
                aggregator,
                advisor_count,
                cached,
            } => Self::MoaAggregating {
                aggregator,
                advisor_count,
                cached,
            },
            LoopTraceEvent::MoaAdvisorSpend {
                advisor_count,
                billed_count,
                input_tokens,
                output_tokens,
                cost_usd,
            } => Self::MoaAdvisorSpend {
                advisor_count,
                billed_count,
                input_tokens,
                output_tokens,
                cost_usd,
            },
            LoopTraceEvent::MoaTurnTrace { preset, payload } => {
                Self::MoaTurnTrace { preset, payload }
            }
            LoopTraceEvent::CacheHealthDegraded {
                scope,
                streak,
                reads,
                writes,
                prefix_changed,
            } => Self::CacheHealthDegraded {
                scope,
                streak,
                reads,
                writes,
                prefix_changed,
            },
        }
    }
}

impl From<LoopTraceTextKind> for aleph_protocol::AgentTraceTextKind {
    fn from(kind: LoopTraceTextKind) -> Self {
        match kind {
            LoopTraceTextKind::Final => Self::Final,
        }
    }
}

impl From<LoopTraceState> for aleph_protocol::AgentTraceState {
    fn from(state: LoopTraceState) -> Self {
        match state {
            LoopTraceState::Think => Self::Think,
            LoopTraceState::Act => Self::Act,
        }
    }
}

impl From<LoopTraceTurnOutcome> for aleph_protocol::AgentTraceTurnOutcome {
    fn from(outcome: LoopTraceTurnOutcome) -> Self {
        match outcome {
            // The loop only ever produces these two — caps and cancellation are
            // session-level exits. `AgentTraceTurnOutcome::{HitLimit,Cancelled}`
            // stay on the protocol side for stored legacy blobs (same as
            // `AgentTraceTextKind::Intermediate`); nothing produces them now.
            LoopTraceTurnOutcome::Continue => Self::Continue,
            LoopTraceTurnOutcome::Stop => Self::Stop,
        }
    }
}

impl From<LoopTraceSessionOutcome> for aleph_protocol::AgentTraceSessionOutcome {
    fn from(outcome: LoopTraceSessionOutcome) -> Self {
        match outcome {
            LoopTraceSessionOutcome::Completed => Self::Completed,
            LoopTraceSessionOutcome::HitLimit => Self::HitLimit,
            LoopTraceSessionOutcome::Cancelled => Self::Cancelled,
        }
    }
}

impl From<LoopTraceTurnMetrics> for aleph_protocol::AgentTraceTurnMetrics {
    fn from(metrics: LoopTraceTurnMetrics) -> Self {
        Self {
            requested_tool_calls: metrics.requested_tool_calls,
            executed_tool_calls: metrics.executed_tool_calls,
            productive: metrics.productive,
            total_tokens: metrics.total_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::trace::{ToolCallEndEvent, ToolCallStartEvent};
    use crate::orchestrator::dispatch::TerminateReason;

    #[test]
    fn session_completed_encodes_alephcore_types_as_opaque_json() {
        let event = LoopTraceEvent::SessionCompleted {
            outcome: LoopTraceSessionOutcome::HitLimit,
            iterations: 4,
            tool_calls_made: 2,
            total_tokens: 900,
            hit_limit: true,
            final_text: Some("done".into()),
            terminate_reason: Some(TerminateReason::HitMaxIterations { used: 4 }),
            duration_ms: Some(1_234),
            token_breakdown: None,
            tool_timeline: Vec::new(),
        };

        let wire: aleph_protocol::AgentTraceEvent = event.into();
        let aleph_protocol::AgentTraceEvent::SessionCompleted {
            outcome,
            hit_limit,
            terminate_reason,
            duration_ms,
            ..
        } = wire
        else {
            panic!("expected SessionCompleted");
        };

        assert!(matches!(
            outcome,
            aleph_protocol::AgentTraceSessionOutcome::HitLimit
        ));
        assert!(hit_limit);
        assert_eq!(duration_ms, Some(1_234));
        assert_eq!(
            terminate_reason,
            Some(serde_json::json!({"kind": "hit_max_iterations", "used": 4}))
        );
    }

    #[test]
    fn tool_call_completed_maps_error_result_with_retryable_flag() {
        let event = LoopTraceEvent::ToolCallCompleted {
            iteration: 1,
            call: ToolCallEndEvent {
                tool_id: "call-1".into(),
                tool_name: "bash".into(),
                input: serde_json::json!({"command": "ls"}),
                duration_ms: 12,
            },
            result: crate::tools::runtime::ToolResult::Error {
                error: "boom".into(),
                retryable: true,
            },
        };

        let wire: aleph_protocol::AgentTraceEvent = event.into();
        let aleph_protocol::AgentTraceEvent::ToolCallCompleted { call, result, .. } = wire else {
            panic!("expected ToolCallCompleted");
        };

        assert_eq!(call.tool_id, "call-1");
        assert_eq!(call.duration_ms, 12);
        assert!(matches!(
            result,
            aleph_protocol::AgentTraceToolResult::Error {
                retryable: true,
                ..
            }
        ));
    }

    #[test]
    fn tool_call_started_carries_id_name_and_input() {
        let event = LoopTraceEvent::ToolCallStarted {
            iteration: 0,
            call: ToolCallStartEvent {
                tool_id: "call-0".into(),
                tool_name: "read".into(),
                input: serde_json::json!({"path": "/tmp/x"}),
            },
        };

        let wire: aleph_protocol::AgentTraceEvent = event.into();
        let aleph_protocol::AgentTraceEvent::ToolCallStarted { iteration, call } = wire else {
            panic!("expected ToolCallStarted");
        };

        assert_eq!(iteration, 0);
        assert_eq!(call.tool_name, "read");
        assert_eq!(call.input, serde_json::json!({"path": "/tmp/x"}));
    }

    #[test]
    fn scalar_enums_map_one_to_one() {
        assert!(matches!(
            aleph_protocol::AgentTraceTextKind::from(LoopTraceTextKind::Final),
            aleph_protocol::AgentTraceTextKind::Final
        ));
        assert!(matches!(
            aleph_protocol::AgentTraceState::from(LoopTraceState::Act),
            aleph_protocol::AgentTraceState::Act
        ));
        assert!(matches!(
            aleph_protocol::AgentTraceTurnOutcome::from(LoopTraceTurnOutcome::Stop),
            aleph_protocol::AgentTraceTurnOutcome::Stop
        ));
        assert!(matches!(
            aleph_protocol::AgentTraceSessionOutcome::from(LoopTraceSessionOutcome::Completed),
            aleph_protocol::AgentTraceSessionOutcome::Completed
        ));
    }

    #[test]
    fn turn_metrics_fields_survive_the_crate_edge() {
        let metrics = LoopTraceTurnMetrics {
            requested_tool_calls: 3,
            executed_tool_calls: 2,
            productive: true,
            total_tokens: 42,
        };

        let wire = aleph_protocol::AgentTraceTurnMetrics::from(metrics);

        assert_eq!(wire.requested_tool_calls, 3);
        assert_eq!(wire.executed_tool_calls, 2);
        assert!(wire.productive);
        assert_eq!(wire.total_tokens, 42);
    }
}
