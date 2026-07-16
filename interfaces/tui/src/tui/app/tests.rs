use super::*;
use aleph_protocol::{
    AgentTraceEvent, AgentTraceReplay, AgentTraceSessionOutcome, AgentTraceTextKind, StreamEvent,
};

#[test]
fn new_state_has_welcome_message() {
    let state = AppState::new("test-session".into(), "claude-3".into());
    assert_eq!(state.messages.len(), 1);
    match &state.messages[0] {
        ChatMessage::System { content } => {
            assert!(content.contains("test-session"));
            assert!(content.contains("claude-3"));
        }
        other => panic!("Expected System message, got: {other:?}"),
    }
    assert!(state.auto_scroll);
    assert_eq!(state.focus, Focus::Input);
    assert!(!state.should_quit);
}

#[test]
fn scroll_up_disables_auto_scroll() {
    let mut state = AppState::new("s".into(), "m".into());
    assert!(state.auto_scroll);

    state.scroll_up(5);
    assert_eq!(state.scroll_offset, 5);
    assert!(!state.auto_scroll);

    // Scrolling up more adds to offset
    state.scroll_up(3);
    assert_eq!(state.scroll_offset, 8);
    assert!(!state.auto_scroll);
}

#[test]
fn scroll_to_bottom_re_enables_auto_scroll() {
    let mut state = AppState::new("s".into(), "m".into());
    state.scroll_up(10);
    assert!(!state.auto_scroll);
    assert_eq!(state.scroll_offset, 10);

    state.scroll_to_bottom();
    assert!(state.auto_scroll);
    assert_eq!(state.scroll_offset, 0);
}

#[test]
fn scroll_down_to_zero_re_enables_auto_scroll() {
    let mut state = AppState::new("s".into(), "m".into());
    state.scroll_up(3);
    assert!(!state.auto_scroll);

    state.scroll_down(3);
    assert_eq!(state.scroll_offset, 0);
    assert!(state.auto_scroll);
}

#[test]
fn toggle_verbose() {
    let mut state = AppState::new("s".into(), "m".into());
    assert!(!state.verbose);

    state.toggle_verbose();
    assert!(state.verbose);

    state.toggle_verbose();
    assert!(!state.verbose);
}

#[test]
fn ensure_assistant_message_creates_one() {
    let mut state = AppState::new("s".into(), "m".into());
    // Only has system message
    assert_eq!(state.messages.len(), 1);

    state.ensure_assistant_message();
    assert_eq!(state.messages.len(), 2);
    assert!(matches!(
        state.messages[1],
        ChatMessage::Assistant {
            is_streaming: true,
            ..
        }
    ));
}

#[test]
fn ensure_assistant_message_idempotent() {
    let mut state = AppState::new("s".into(), "m".into());
    state.ensure_assistant_message();
    assert_eq!(state.messages.len(), 2);

    // Calling again should not create another
    state.ensure_assistant_message();
    assert_eq!(state.messages.len(), 2);
}

#[test]
fn add_user_message_appended() {
    let mut state = AppState::new("s".into(), "m".into());
    state.add_user_message("hello".into());
    assert_eq!(state.messages.len(), 2);
    match &state.messages[1] {
        ChatMessage::User { content, .. } => assert_eq!(content, "hello"),
        other => panic!("Expected User message, got: {other:?}"),
    }
}

#[test]
fn find_tool_mut_returns_correct_tool() {
    let mut state = AppState::new("s".into(), "m".into());
    state.ensure_assistant_message();
    if let ChatMessage::Assistant { tools, .. } = state.current_assistant_mut() {
        tools.push(ToolExecution {
            id: "tool-1".into(),
            name: "bash".into(),
            params: "ls".into(),
            status: ToolStatus::Running,
            duration: None,
            progress: None,
            error: None,
        });
        tools.push(ToolExecution {
            id: "tool-2".into(),
            name: "read".into(),
            params: "file.txt".into(),
            status: ToolStatus::Running,
            duration: None,
            progress: None,
            error: None,
        });
    }

    let tool = state.find_tool_mut("tool-2");
    assert!(tool.is_some());
    assert_eq!(tool.unwrap().name, "read");

    let missing = state.find_tool_mut("tool-999");
    assert!(missing.is_none());
}

#[test]
fn open_command_palette_sets_focus() {
    let mut state = AppState::new("s".into(), "m".into());
    state.open_command_palette();
    assert_eq!(state.focus, Focus::CommandPalette);
    assert!(state.palette.is_some());

    let palette = state.palette.as_ref().unwrap();
    assert!(palette.input.is_empty());
    assert!(!palette.filtered.is_empty());
    assert_eq!(palette.selected, 0);
}

#[test]
fn close_overlay_resets_focus() {
    let mut state = AppState::new("s".into(), "m".into());
    state.open_command_palette();
    assert_eq!(state.focus, Focus::CommandPalette);

    state.close_overlay();
    assert_eq!(state.focus, Focus::Input);
    assert!(state.palette.is_none());
    assert!(state.dialog.is_none());
}

#[test]
fn show_dialog_sets_focus() {
    let mut state = AppState::new("s".into(), "m".into());
    state.show_dialog(
        "telegram:bot:1:u1".into(),
        "Approve?".into(),
        vec!["Yes".into(), "No".into()],
    );
    assert_eq!(state.focus, Focus::Dialog);
    let dialog = state.dialog.as_ref().unwrap();
    assert_eq!(dialog.session_key, "telegram:bot:1:u1");
    assert_eq!(dialog.question, "Approve?");
    assert_eq!(dialog.options.len(), 2);
    assert_eq!(dialog.selected, 0);
}

#[test]
fn switch_session_clears_messages() {
    let mut state = AppState::new("s1".into(), "m".into());
    state.add_user_message("hello".into());
    assert_eq!(state.messages.len(), 2);

    state.switch_session("s2");
    assert_eq!(state.session_key, "s2");
    // Should have 1 message: the switch notification
    assert_eq!(state.messages.len(), 1);
    match &state.messages[0] {
        ChatMessage::System { content } => assert!(content.contains("s2")),
        other => panic!("Expected System message, got: {other:?}"),
    }
}

#[test]
fn clear_screen_keeps_session() {
    let mut state = AppState::new("s1".into(), "m".into());
    state.add_user_message("hello".into());
    state.total_tokens = 500;

    state.clear_screen();
    assert_eq!(state.session_key, "s1");
    assert_eq!(state.total_tokens, 500);
    assert_eq!(state.messages.len(), 1);
    match &state.messages[0] {
        ChatMessage::System { content } => assert!(content.contains("cleared")),
        other => panic!("Expected System message, got: {other:?}"),
    }
}

#[test]
fn update_token_usage_accumulates() {
    let mut state = AppState::new("s".into(), "m".into());
    let summary = RunSummary {
        total_tokens: 100,
        tool_calls: 2,
        loops: 1,
        final_response: None,
        ..Default::default()
    };
    state.update_token_usage(&summary);
    assert_eq!(state.total_tokens, 100);

    state.update_token_usage(&summary);
    assert_eq!(state.total_tokens, 200);
}

#[test]
fn request_quit_sets_flag() {
    let mut state = AppState::new("s".into(), "m".into());
    assert!(!state.should_quit);
    state.request_quit();
    assert!(state.should_quit);
}

#[test]
fn handle_run_accepted() {
    let mut state = AppState::new("s".into(), "m".into());
    let event = StreamEvent::RunAccepted {
        run_id: "run-1".into(),
        session_key: "s".into(),
        accepted_at: "2026-03-04T00:00:00Z".into(),
    };
    let action = state.handle_gateway_event(event);
    assert!(matches!(action, Action::None));
    assert_eq!(state.current_run, Some("run-1".into()));
    assert!(state.is_connected);
}

#[test]
fn handle_response_chunk_appends_content() {
    let mut state = AppState::new("s".into(), "m".into());

    let chunk1 = StreamEvent::ResponseChunk {
        run_id: "run-1".into(),
        seq: 1,
        content: "Hello".into(),
        chunk_index: 0,
        is_final: false,
        is_intermediate: false,
    };
    state.handle_gateway_event(chunk1);

    let chunk2 = StreamEvent::ResponseChunk {
        run_id: "run-1".into(),
        seq: 2,
        content: " World".into(),
        chunk_index: 1,
        is_final: false,
        is_intermediate: false,
    };
    state.handle_gateway_event(chunk2);

    // Should have: system welcome + assistant message
    assert_eq!(state.messages.len(), 2);
    match &state.messages[1] {
        ChatMessage::Assistant { content, .. } => {
            assert_eq!(content, "Hello World");
        }
        other => panic!("Expected Assistant message, got: {other:?}"),
    }
}

#[test]
fn handle_agent_trace_text_events_populate_assistant_content_and_reasoning() {
    let mut state = AppState::new("s".into(), "m".into());

    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-1".into(),
        session_key: "s".into(),
        accepted_at: "2026-03-04T00:00:00Z".into(),
    });

    state.handle_gateway_event(StreamEvent::AgentTrace {
        run_id: "run-1".into(),
        seq: 1,
        event: AgentTraceEvent::TextEmitted {
            iteration: 1,
            stream: AgentTraceTextKind::Intermediate,
            text: "Inspecting replay trace".into(),
        },
    });
    state.handle_gateway_event(StreamEvent::AgentTrace {
        run_id: "run-1".into(),
        seq: 2,
        event: AgentTraceEvent::TextEmitted {
            iteration: 1,
            stream: AgentTraceTextKind::Final,
            text: "Replay loaded".into(),
        },
    });

    match &state.messages[1] {
        ChatMessage::Assistant {
            content, reasoning, ..
        } => {
            assert_eq!(content, "Replay loaded");
            assert_eq!(reasoning.as_deref(), Some("Inspecting replay trace"));
        }
        other => panic!("Expected Assistant message, got: {other:?}"),
    }
}

#[test]
fn handle_agent_trace_session_completed_updates_totals_and_closes_stream() {
    let mut state = AppState::new("s".into(), "m".into());

    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-1".into(),
        session_key: "s".into(),
        accepted_at: "2026-03-04T00:00:00Z".into(),
    });
    state.handle_gateway_event(StreamEvent::AgentTrace {
        run_id: "run-1".into(),
        seq: 1,
        event: AgentTraceEvent::TextEmitted {
            iteration: 1,
            stream: AgentTraceTextKind::Final,
            text: "Replay loaded".into(),
        },
    });

    state.handle_gateway_event(StreamEvent::AgentTrace {
        run_id: "run-1".into(),
        seq: 2,
        event: AgentTraceEvent::SessionCompleted {
            outcome: AgentTraceSessionOutcome::Completed,
            iterations: 1,
            tool_calls_made: 0,
            total_tokens: 321,
            hit_limit: false,
            final_text: Some("Replay loaded".into()),
            terminate_reason: None,
            duration_ms: None,
            token_breakdown: None,
            tool_timeline: Vec::new(),
        },
    });

    assert_eq!(state.total_tokens, 321);
    assert!(state.current_run.is_none());
    assert!(!state.current_run_uses_agent_trace);
    match &state.messages[1] {
        ChatMessage::Assistant { is_streaming, .. } => assert!(!is_streaming),
        other => panic!("Expected Assistant message, got: {other:?}"),
    }
}

#[test]
fn handle_agent_trace_decision_events_append_shared_projection_reasoning() {
    let mut state = AppState::new("s".into(), "m".into());

    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-1".into(),
        session_key: "s".into(),
        accepted_at: "2026-03-04T00:00:00Z".into(),
    });

    state.handle_gateway_event(StreamEvent::AgentTrace {
        run_id: "run-1".into(),
        seq: 1,
        event: AgentTraceEvent::TurnStarted { iteration: 1 },
    });
    state.handle_gateway_event(StreamEvent::AgentTrace {
        run_id: "run-1".into(),
        seq: 2,
        event: AgentTraceEvent::TurnStateEntered {
            iteration: 1,
            state: aleph_protocol::AgentTraceState::Think,
        },
    });
    state.handle_gateway_event(StreamEvent::AgentTrace {
        run_id: "run-1".into(),
        seq: 3,
        event: AgentTraceEvent::TurnCompleted {
            iteration: 1,
            outcome: aleph_protocol::AgentTraceTurnOutcome::Continue,
            metrics: aleph_protocol::AgentTraceTurnMetrics {
                requested_tool_calls: 1,
                executed_tool_calls: 1,
                productive: true,
                consecutive_errors: 0,
                total_tokens: 64,
            },
        },
    });

    match &state.messages[1] {
        ChatMessage::Assistant { reasoning, .. } => {
            assert_eq!(
                reasoning.as_deref(),
                Some(
                    "Turn started (iteration 1)\nThinking (iteration 1)\nTurn completed (continue) — tools: 1/1, tokens: 64"
                )
            );
        }
        other => panic!("Expected Assistant message, got: {other:?}"),
    }
}

#[test]
fn load_trace_replay_records_session_summary_in_reasoning() {
    let mut state = AppState::new("s".into(), "m".into());

    let replay = AgentTraceReplay {
        task: aleph_protocol::AgentTraceTaskSummary {
            task_id: "task-1".into(),
            session_id: "session-1".into(),
            agent_id: "agent-1".into(),
            status: "completed".into(),
            prompt_preview: "Inspect replay".into(),
            created_at: 10,
            updated_at: 20,
            started_at: Some(11),
            completed_at: Some(19),
            trace_count: 2,
            last_event_kind: Some("session_completed".into()),
        },
        traces: vec![
            aleph_protocol::AgentTraceReplayEntry {
                step: 0,
                event: AgentTraceEvent::TurnStarted { iteration: 1 },
            },
            aleph_protocol::AgentTraceReplayEntry {
                step: 1,
                event: AgentTraceEvent::SessionCompleted {
                    outcome: AgentTraceSessionOutcome::Completed,
                    iterations: 1,
                    tool_calls_made: 0,
                    total_tokens: 33,
                    hit_limit: false,
                    final_text: Some("done".into()),
                    terminate_reason: None,
                    duration_ms: None,
                    token_breakdown: None,
                    tool_timeline: Vec::new(),
                },
            },
        ],
    };
    state.load_trace_replay(&replay);

    match &state.messages[1] {
        ChatMessage::Assistant {
            content, reasoning, ..
        } => {
            assert_eq!(content, "done");
            assert_eq!(
                reasoning.as_deref(),
                Some("Turn started (iteration 1)\nSession completed (completed) — iterations: 1, tools: 0, tokens: 33 — done")
            );
        }
        other => panic!("Expected Assistant message, got: {other:?}"),
    }
}

#[test]
fn handle_tool_lifecycle() {
    let mut state = AppState::new("s".into(), "m".into());

    // Tool start
    let start = StreamEvent::ToolStart {
        run_id: "run-1".into(),
        seq: 1,
        tool_name: "bash".into(),
        tool_id: "t1".into(),
        params: serde_json::json!({"command": "ls"}),
    };
    state.handle_gateway_event(start);

    // Tool update
    let update = StreamEvent::ToolUpdate {
        run_id: "run-1".into(),
        seq: 2,
        tool_id: "t1".into(),
        progress: "running...".into(),
    };
    state.handle_gateway_event(update);

    {
        let tool = state.find_tool_mut("t1").unwrap();
        assert_eq!(tool.status, ToolStatus::Running);
        assert_eq!(tool.progress, Some("running...".into()));
    }

    // Tool end
    let end = StreamEvent::ToolEnd {
        run_id: "run-1".into(),
        seq: 3,
        tool_id: "t1".into(),
        result: aleph_protocol::ToolResult::success("output"),
        duration_ms: 150,
    };
    state.handle_gateway_event(end);

    let tool = state.find_tool_mut("t1").unwrap();
    assert_eq!(tool.status, ToolStatus::Success);
    assert_eq!(tool.duration, Some(Duration::from_millis(150)));
    assert!(tool.progress.is_none()); // cleared on end
}

#[test]
fn handle_agent_trace_tool_lifecycle_takes_precedence() {
    let mut state = AppState::new("s".into(), "m".into());

    state.handle_gateway_event(StreamEvent::AgentTrace {
        run_id: "run-1".into(),
        seq: 1,
        event: aleph_protocol::AgentTraceEvent::ToolCallStarted {
            iteration: 1,
            call: aleph_protocol::AgentTraceToolCallStart {
                tool_id: "t1".into(),
                tool_name: "bash".into(),
                input: serde_json::json!({"command": "ls"}),
            },
        },
    });

    state.handle_gateway_event(StreamEvent::ToolStart {
        run_id: "run-1".into(),
        seq: 2,
        tool_name: "bash".into(),
        tool_id: "t1".into(),
        params: serde_json::json!({"command": "ls"}),
    });

    state.handle_gateway_event(StreamEvent::AgentTrace {
        run_id: "run-1".into(),
        seq: 3,
        event: aleph_protocol::AgentTraceEvent::ToolSummary {
            iteration: 1,
            summary: "Listed the current directory".into(),
        },
    });

    state.handle_gateway_event(StreamEvent::ReasoningBlock {
        run_id: "run-1".into(),
        seq: 4,
        step_type: aleph_protocol::ReasoningStepType::Observation,
        label: "Tool Summary".into(),
        content: "legacy summary".into(),
        confidence: None,
        is_final: false,
    });

    state.handle_gateway_event(StreamEvent::AgentTrace {
        run_id: "run-1".into(),
        seq: 5,
        event: aleph_protocol::AgentTraceEvent::ToolCallCompleted {
            iteration: 1,
            call: aleph_protocol::AgentTraceToolCallEnd {
                tool_id: "t1".into(),
                tool_name: "bash".into(),
                input: serde_json::json!({"command": "ls"}),
                duration_ms: 120,
            },
            result: aleph_protocol::AgentTraceToolResult::Success {
                output: serde_json::json!({"ok": true}),
            },
        },
    });

    match &state.messages[1] {
        ChatMessage::Assistant {
            tools, reasoning, ..
        } => {
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0].id, "t1");
            assert_eq!(tools[0].status, ToolStatus::Success);
            assert_eq!(tools[0].duration, Some(Duration::from_millis(120)));
            assert_eq!(reasoning.as_deref(), Some("Listed the current directory"));
        }
        other => panic!("Expected Assistant message, got: {other:?}"),
    }
}

#[test]
fn handle_run_complete_clears_run() {
    let mut state = AppState::new("s".into(), "m".into());
    state.current_run = Some("run-1".into());

    // Create an assistant message that's streaming
    state.ensure_assistant_message();

    let event = StreamEvent::RunComplete {
        run_id: "run-1".into(),
        seq: 10,
        summary: RunSummary {
            total_tokens: 500,
            tool_calls: 3,
            loops: 2,
            final_response: Some("Done".into()),
            ..Default::default()
        },
        total_duration_ms: 5000,
    };
    state.handle_gateway_event(event);

    assert!(state.current_run.is_none());
    assert_eq!(state.total_tokens, 500);
    assert_eq!(state.last_run_duration, Some(Duration::from_secs(5)));

    // Assistant message should no longer be streaming
    match &state.messages.last().unwrap() {
        ChatMessage::Assistant { is_streaming, .. } => assert!(!is_streaming),
        other => panic!("Expected Assistant message, got: {other:?}"),
    }
}

#[test]
fn run_complete_does_not_double_count_after_agent_trace_session_completed() {
    let mut state = AppState::new("s".into(), "m".into());

    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-1".into(),
        session_key: "s".into(),
        accepted_at: "2026-03-04T00:00:00Z".into(),
    });
    state.handle_gateway_event(StreamEvent::AgentTrace {
        run_id: "run-1".into(),
        seq: 1,
        event: AgentTraceEvent::SessionCompleted {
            outcome: AgentTraceSessionOutcome::Completed,
            iterations: 1,
            tool_calls_made: 0,
            total_tokens: 321,
            hit_limit: false,
            final_text: Some("done".into()),
            terminate_reason: None,
            duration_ms: None,
            token_breakdown: None,
            tool_timeline: Vec::new(),
        },
    });
    state.handle_gateway_event(StreamEvent::RunComplete {
        run_id: "run-1".into(),
        seq: 2,
        summary: RunSummary {
            total_tokens: 321,
            tool_calls: 0,
            loops: 1,
            final_response: Some("done".into()),
            ..Default::default()
        },
        total_duration_ms: 1500,
    });

    assert_eq!(state.total_tokens, 321);
    assert!(!state.current_run_trace_summary_applied);
}

#[test]
fn handle_run_error_adds_system_message() {
    let mut state = AppState::new("s".into(), "m".into());
    state.current_run = Some("run-1".into());

    let event = StreamEvent::RunError {
        run_id: "run-1".into(),
        seq: 5,
        error: "something went wrong".into(),
        error_code: Some("E001".into()),
    };
    state.handle_gateway_event(event);

    assert!(state.current_run.is_none());
    // Last message should be the error system message
    match state.messages.last().unwrap() {
        ChatMessage::System { content } => {
            assert!(content.contains("something went wrong"));
        }
        other => panic!("Expected System message, got: {other:?}"),
    }
}

#[test]
fn open_approval_sets_focus_and_state() {
    let mut state = AppState::new("s".into(), "m".into());
    state.open_approval("ap-1".into(), "rm -rf /tmp/x".into(), Some("destructive".into()));
    assert_eq!(state.focus, Focus::Approval);
    let approval = state.approval.as_ref().unwrap();
    assert_eq!(approval.id, "ap-1");
    assert_eq!(approval.command, "rm -rf /tmp/x");
    assert_eq!(approval.selected, 0);
}

#[test]
fn run_error_dismisses_pending_approval() {
    // A run parked on an approval that then errors must not strand the modal:
    // the poll stops once current_run clears, so run-end has to retract it.
    let mut state = AppState::new("s".into(), "m".into());
    state.current_run = Some("run-1".into());
    state.open_approval("ap-1".into(), "cmd".into(), None);
    assert_eq!(state.focus, Focus::Approval);

    state.handle_gateway_event(StreamEvent::RunError {
        run_id: "run-1".into(),
        seq: 1,
        error: "boom".into(),
        error_code: None,
    });

    assert!(state.approval.is_none());
    assert_eq!(state.focus, Focus::Input);
}

#[test]
fn dismiss_pending_approval_is_noop_without_overlay() {
    // The guard must not touch focus when no overlay is up (e.g. a run ending
    // while the user is scrolling chat).
    let mut state = AppState::new("s".into(), "m".into());
    state.focus = Focus::Chat;
    state.dismiss_pending_approval();
    assert_eq!(state.focus, Focus::Chat);
    assert!(state.approval.is_none());
}

#[test]
fn handle_ask_user_shows_dialog() {
    let mut state = AppState::new("s".into(), "m".into());
    let event = StreamEvent::AskUser {
        run_id: "run-1".into(),
        seq: 3,
        session_key: "telegram:bot:1:u1".into(),
        question: "Allow file write?".into(),
        options: vec!["Allow".into(), "Deny".into()],
    };
    state.handle_gateway_event(event);

    assert_eq!(state.focus, Focus::Dialog);
    let dialog = state.dialog.as_ref().unwrap();
    // The dialog keeps the clarification key so the answer can resolve it.
    assert_eq!(dialog.session_key, "telegram:bot:1:u1");
    assert_eq!(dialog.question, "Allow file write?");
}

#[test]
fn handle_reasoning_appends() {
    let mut state = AppState::new("s".into(), "m".into());

    let event1 = StreamEvent::Reasoning {
        run_id: "run-1".into(),
        seq: 1,
        content: "Let me think".into(),
        is_complete: false,
    };
    state.handle_gateway_event(event1);

    let event2 = StreamEvent::Reasoning {
        run_id: "run-1".into(),
        seq: 2,
        content: " about this...".into(),
        is_complete: true,
    };
    state.handle_gateway_event(event2);

    match &state.messages[1] {
        ChatMessage::Assistant { reasoning, .. } => {
            assert_eq!(reasoning.as_deref(), Some("Let me think about this..."));
        }
        other => panic!("Expected Assistant message, got: {other:?}"),
    }
}
