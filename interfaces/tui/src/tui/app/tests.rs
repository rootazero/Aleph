use super::*;
use aleph_protocol::{
    AgentTraceEvent, AgentTraceReplay, AgentTraceSessionOutcome, AgentTraceTextKind,
    SessionSnapshot, StreamEvent,
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

/// A menu question, with the two labels this fixture reuses.
fn menu_view(question: &str) -> AskDialogView {
    AskDialogView {
        question: question.to_string(),
        options: vec!["Yes".into(), "No".into()],
        multi_select: false,
        secret: false,
    }
}

#[test]
fn show_dialog_sets_focus() {
    let mut state = AppState::new("s".into(), "m".into());
    state.show_dialog("telegram:bot:1:u1".into(), menu_view("Approve?"));
    assert_eq!(state.focus, Focus::Dialog);
    let dialog = state.dialog.as_ref().unwrap();
    assert_eq!(dialog.session_key, "telegram:bot:1:u1");
    assert_eq!(dialog.question, "Approve?");
    assert_eq!(dialog.options.len(), 2);
    assert_eq!(dialog.selected, 0);
    // A menu question opens on the menu.
    assert!(!dialog.typing);
    assert!(dialog.has_quick_pick());
}

/// The defect the answer buffer exists to close: a question with no choices had
/// no answerable key at all, and `Esc` is swallowed for this overlay, so the
/// TUI was held by a modal nothing could dismiss. Such a question must open
/// straight into text mode.
#[test]
fn a_free_text_question_opens_ready_to_type() {
    let mut state = AppState::new("s".into(), "m".into());
    state.show_dialog(
        "telegram:bot:1:u1".into(),
        AskDialogView {
            question: "Which language?".into(),
            options: vec![],
            multi_select: false,
            secret: false,
        },
    );
    let dialog = state.dialog.as_ref().unwrap();
    assert!(dialog.typing, "a question with no menu must accept typing");
    assert!(!dialog.has_quick_pick());
    // Nothing typed yet ⇒ Enter has nothing to send (rather than sending "").
    assert_eq!(dialog.pending_reply(), None);
}

/// A single index cannot express a multi-select answer, so there is nothing to
/// quick-pick — the same reason the server suppresses a channel's inline
/// keyboard for these (`clarification::render::keyboard_for`).
#[test]
fn a_multi_select_question_opens_ready_to_type() {
    let mut state = AppState::new("s".into(), "m".into());
    state.show_dialog(
        "telegram:bot:1:u1".into(),
        AskDialogView {
            multi_select: true,
            ..menu_view("Which ones?")
        },
    );
    let dialog = state.dialog.as_ref().unwrap();
    assert!(dialog.typing);
    assert!(!dialog.has_quick_pick());
}

/// A pick sends the 1-BASED INDEX. Labels carry a `— description` suffix and
/// core matches labels exactly, so replying with the label would arrive as free
/// text with no selected index — right to a human, `custom` to the model.
#[test]
fn a_pick_replies_with_the_index_not_the_label() {
    let mut state = AppState::new("s".into(), "m".into());
    state.show_dialog(
        "k".into(),
        AskDialogView {
            options: vec!["staging — shared QA".into(), "prod — live".into()],
            ..menu_view("Deploy where?")
        },
    );
    let dialog = state.dialog.as_mut().unwrap();
    assert_eq!(dialog.pending_reply().as_deref(), Some("1"));
    dialog.selected = 1;
    assert_eq!(dialog.pending_reply().as_deref(), Some("2"));
}

/// Free text always beats the menu in text mode — a menu never forbids it,
/// which is why `ask_user` tells the model never to add an "other" choice.
#[test]
fn a_typed_answer_is_sent_verbatim() {
    let mut state = AppState::new("s".into(), "m".into());
    state.show_dialog("k".into(), menu_view("Approve?"));
    let dialog = state.dialog.as_mut().unwrap();
    dialog.typing = true;
    dialog.input = "  neither, wait for Ana  ".into();
    assert_eq!(
        dialog.pending_reply().as_deref(),
        Some("neither, wait for Ana")
    );
    // Tabbing back to the list means the list, buffer or no buffer.
    dialog.typing = false;
    assert_eq!(dialog.pending_reply().as_deref(), Some("1"));
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
fn switch_session_drops_stale_cache_stat() {
    // The old session's cache hit% is meaningless for a different prefix; a
    // cache-less provider in the new session would otherwise display it
    // forever (the stat only updates when a call reports cache activity).
    let mut state = AppState::new("s1".into(), "m".into());
    state.cache_stat = Some(87);

    state.switch_session("s2");
    assert_eq!(state.cache_stat, None, "stale cache stat must not survive");
}

#[test]
fn provider_usage_cache_stat_matches_canonical_formula() {
    // The status bar percentage must be the canonical `read / (input + read)`
    // from `aleph_protocol::cache_hit_ratio` — the same number the core DB
    // rollup and Panel Usage show for the same call. Regression guard for the
    // old local recompute (`input + creation + read` denominator), which read
    // systematically lower on cold starts and prefix rewrites.
    let mut state = AppState::new("s1".into(), "m".into());
    state.apply_agent_trace_event(&AgentTraceEvent::ProviderUsage {
        agent_id: "root".into(),
        input_tokens: 100,
        output_tokens: 10,
        cache_read_tokens: Some(870),
        cache_creation_tokens: Some(30),
        thinking_tokens: None,
    });
    // 870 / (100 + 870) ≈ 89.7% → 90, NOT the 87% the old denominator gave.
    let expected =
        (aleph_protocol::cache_hit_ratio(100, Some(870)).unwrap() * 100.0).round() as u64;
    assert_eq!(state.cache_stat, Some(expected));
    assert_eq!(state.cache_stat, Some(90));

    // A pure cold write (creation only) reads as 0%, not "unknown".
    state.apply_agent_trace_event(&AgentTraceEvent::ProviderUsage {
        agent_id: "root".into(),
        input_tokens: 500,
        output_tokens: 10,
        cache_read_tokens: Some(0),
        cache_creation_tokens: Some(500),
        thinking_tokens: None,
    });
    assert_eq!(state.cache_stat, Some(0));

    // A call with no cache activity at all must not clobber the stat with a
    // misleading 0% (providers without prompt caching).
    state.cache_stat = Some(90);
    state.apply_agent_trace_event(&AgentTraceEvent::ProviderUsage {
        agent_id: "root".into(),
        input_tokens: 500,
        output_tokens: 10,
        cache_read_tokens: None,
        cache_creation_tokens: None,
        thinking_tokens: None,
    });
    assert_eq!(state.cache_stat, Some(90));
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
    state.open_approval(
        "ap-1".into(),
        "rm -rf /tmp/x".into(),
        Some("destructive".into()),
        DEFAULT_APPROVAL_DECISIONS.to_vec(),
    );
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
    state.open_approval(
        "ap-1".into(),
        "cmd".into(),
        None,
        DEFAULT_APPROVAL_DECISIONS.to_vec(),
    );
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
        // A core that predates the structured view sends neither; the overlay
        // must still render from the flat pair.
        questions: vec![],
        answered: 0,
    };
    state.handle_gateway_event(event);

    assert_eq!(state.focus, Focus::Dialog);
    let dialog = state.dialog.as_ref().unwrap();
    // The dialog keeps the clarification key so the answer can resolve it.
    assert_eq!(dialog.session_key, "telegram:bot:1:u1");
    assert_eq!(dialog.question, "Allow file write?");
    assert_eq!(dialog.options, vec!["Allow", "Deny"]);
}

/// With the structured view the overlay must show what the flat pair
/// structurally cannot: the per-option description, the short header, and the
/// position within a multi-question request. Indices stay 1-based and
/// unchanged, which is what lets the answer keep being a bare number.
#[test]
fn handle_ask_user_renders_the_structured_question() {
    use aleph_protocol::{AskUserOption, AskUserQuestion};

    let mut state = AppState::new("s".into(), "m".into());
    state.handle_gateway_event(StreamEvent::AskUser {
        run_id: "run-1".into(),
        seq: 3,
        session_key: "telegram:bot:1:u1".into(),
        question: "Ticket id?".into(),
        options: vec![],
        questions: vec![
            AskUserQuestion {
                id: "env".into(),
                header: Some("Env".into()),
                prompt: "Deploy where?".into(),
                options: vec![AskUserOption {
                    label: "staging".into(),
                    description: Some("shared QA".into()),
                }],
                multi_select: false,
                secret: false,
            },
            AskUserQuestion {
                id: "ticket".into(),
                header: None,
                prompt: "Ticket id?".into(),
                options: vec![],
                multi_select: false,
                secret: false,
            },
        ],
        answered: 1,
    });

    let dialog = state.dialog.as_ref().unwrap();
    // Cursor at 1 ⇒ the SECOND question, not the first.
    assert!(
        dialog.question.contains("Ticket id?"),
        "{}",
        dialog.question
    );
    assert!(dialog.question.contains("(2/2)"), "{}", dialog.question);
}

/// The description reaches this surface at all — the defect the structured
/// view exists to close (a channel rendered `label — description` while every
/// other face rendered a bare label).
#[test]
fn handle_ask_user_shows_option_descriptions() {
    use aleph_protocol::{AskUserOption, AskUserQuestion};

    let mut state = AppState::new("s".into(), "m".into());
    state.handle_gateway_event(StreamEvent::AskUser {
        run_id: "run-1".into(),
        seq: 1,
        session_key: "telegram:bot:1:u1".into(),
        question: "Deploy where?".into(),
        options: vec!["staging".into()],
        questions: vec![AskUserQuestion {
            id: "env".into(),
            header: Some("Env".into()),
            prompt: "Deploy where?".into(),
            options: vec![AskUserOption {
                label: "staging".into(),
                description: Some("shared QA".into()),
            }],
            multi_select: false,
            secret: false,
        }],
        answered: 0,
    });

    let dialog = state.dialog.as_ref().unwrap();
    assert_eq!(dialog.options, vec!["staging — shared QA"]);
    assert!(dialog.question.starts_with("[Env] "), "{}", dialog.question);
    // Single question ⇒ no position marker.
    assert!(!dialog.question.contains("(1/"), "{}", dialog.question);
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

/// A streaming turn puts the same text on the wire twice - as `ResponseChunk`
/// deltas and again in full as `AgentTrace{TextEmitted{Final}}`. The TUI
/// subscribes to no topics, so it receives both, and both used to append.
///
/// The existing single-projection tests stayed green because each feeds only
/// one of the two streams; this one interleaves them in production order.
/// `think.rs` emits `TurnStarted` on every turn, so the "no agent_trace" branch
/// those tests exercise is unreachable in a real run.
#[test]
fn a_streamed_turn_appends_its_text_exactly_once() {
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
    for (i, piece) in ["Hel", "lo, ", "世界!"].iter().enumerate() {
        state.handle_gateway_event(StreamEvent::ResponseChunk {
            run_id: "run-1".into(),
            seq: 2 + i as u64,
            content: (*piece).to_string(),
            chunk_index: u32::try_from(i).unwrap(),
            is_final: false,
            is_intermediate: false,
        });
    }
    state.handle_gateway_event(StreamEvent::AgentTrace {
        run_id: "run-1".into(),
        seq: 9,
        event: AgentTraceEvent::TextEmitted {
            iteration: 1,
            stream: AgentTraceTextKind::Final,
            text: "Hello, 世界!".into(),
        },
    });

    match &state.messages[1] {
        ChatMessage::Assistant { content, .. } => assert_eq!(content, "Hello, 世界!"),
        other => panic!("Expected Assistant message, got: {other:?}"),
    }
}

/// A turn whose text never streamed (mock provider, or an output guardrail
/// holding the full text back) must still land in full from the trace event.
#[test]
fn an_unstreamed_turn_still_takes_its_full_final_text() {
    let mut state = AppState::new("s".into(), "m".into());

    state.handle_gateway_event(StreamEvent::AgentTrace {
        run_id: "run-1".into(),
        seq: 1,
        event: AgentTraceEvent::TurnStarted { iteration: 1 },
    });
    state.handle_gateway_event(StreamEvent::AgentTrace {
        run_id: "run-1".into(),
        seq: 2,
        event: AgentTraceEvent::TextEmitted {
            iteration: 1,
            stream: AgentTraceTextKind::Final,
            text: "no deltas for this one".into(),
        },
    });

    match &state.messages[1] {
        ChatMessage::Assistant { content, .. } => assert_eq!(content, "no deltas for this one"),
        other => panic!("Expected Assistant message, got: {other:?}"),
    }
}

/// Every turn restarts the watermark, so turn 2's text is not clipped by how
/// much turn 1 streamed. Both turns land in the same bubble
/// (`ensure_assistant_message` reuses the trailing Assistant message), which is
/// what made the original doubling accumulate rather than show up as a stray
/// second message.
#[test]
fn a_second_turn_is_not_clipped_by_the_first_turns_watermark() {
    let mut state = AppState::new("s".into(), "m".into());

    for iteration in 1..=2usize {
        let base = iteration as u64 * 10;
        state.handle_gateway_event(StreamEvent::AgentTrace {
            run_id: "run-1".into(),
            seq: base,
            event: AgentTraceEvent::TurnStarted { iteration },
        });
        state.handle_gateway_event(StreamEvent::ResponseChunk {
            run_id: "run-1".into(),
            seq: base + 1,
            content: format!("turn{iteration} "),
            chunk_index: 0,
            is_final: false,
            is_intermediate: false,
        });
        state.handle_gateway_event(StreamEvent::AgentTrace {
            run_id: "run-1".into(),
            seq: base + 2,
            event: AgentTraceEvent::TextEmitted {
                iteration,
                stream: AgentTraceTextKind::Final,
                text: format!("turn{iteration} "),
            },
        });
    }

    match &state.messages[1] {
        ChatMessage::Assistant { content, .. } => assert_eq!(content, "turn1 turn2 "),
        other => panic!("Expected Assistant message, got: {other:?}"),
    }
}

/// `agent_trace` is a deliberately-lossy mirror (bounded mpsc + `try_send`), so
/// a tool-heavy run can drop a `ToolCallCompleted` and leave the row spinning
/// forever. `RunComplete` must reconcile against the authoritative
/// `summary.tool_summaries` - the invariant the protocol documents and the
/// Panel already implements, and which the TUI was never wired to.
#[test]
fn a_dropped_completion_is_repaired_by_the_run_summary() {
    use aleph_protocol::events::{ToolErrorItem, ToolSummaryItem};

    let mut state = AppState::new("s".into(), "m".into());
    state.handle_gateway_event(StreamEvent::AgentTrace {
        run_id: "run-1".into(),
        seq: 1,
        event: AgentTraceEvent::ToolCallStarted {
            iteration: 1,
            call: aleph_protocol::AgentTraceToolCallStart {
                tool_id: "t1".into(),
                tool_name: "bash".into(),
                input: serde_json::json!({"command": "ls"}),
            },
        },
    });
    // ToolCallCompleted for t1 is DROPPED here - that is the whole point.
    assert_eq!(
        state.find_tool_mut("t1").unwrap().status,
        ToolStatus::Running
    );

    state.handle_gateway_event(StreamEvent::RunComplete {
        run_id: "run-1".into(),
        seq: 2,
        summary: RunSummary {
            tool_calls: 2,
            tool_summaries: vec![
                ToolSummaryItem {
                    tool_id: "t1".into(),
                    tool_name: "bash".into(),
                    emoji: "\u{1f4bb}".into(),
                    duration_ms: 120,
                    success: true,
                },
                // This one's *start* frame was dropped too: reconstruct the
                // row, do not skip it.
                ToolSummaryItem {
                    tool_id: "t2".into(),
                    tool_name: "file_read".into(),
                    emoji: "\u{1f4c4}".into(),
                    duration_ms: 8,
                    success: false,
                },
            ],
            errors: vec![ToolErrorItem {
                tool_name: "file_read".into(),
                error: "no such file".into(),
                tool_id: "t2".into(),
            }],
            ..Default::default()
        },
        total_duration_ms: 500,
    });

    let t1 = state.find_tool_mut("t1").unwrap();
    assert_eq!(t1.status, ToolStatus::Success);
    assert_eq!(t1.duration, Some(Duration::from_millis(120)));
    let t2 = state.find_tool_mut("t2").unwrap();
    assert_eq!(t2.status, ToolStatus::Failed);
    assert_eq!(t2.error.as_deref(), Some("no such file"));
}

/// A row the authoritative record does not mention either must still stop
/// spinning: the run is over, so `Running` is the one thing it cannot be.
/// `Unknown`, never `Success` - do not guess a terminal state.
#[test]
fn a_row_absent_from_the_summary_settles_to_unknown() {
    let mut state = AppState::new("s".into(), "m".into());
    state.handle_gateway_event(StreamEvent::AgentTrace {
        run_id: "run-1".into(),
        seq: 1,
        event: AgentTraceEvent::ToolCallStarted {
            iteration: 1,
            call: aleph_protocol::AgentTraceToolCallStart {
                tool_id: "ghost".into(),
                tool_name: "bash".into(),
                input: serde_json::json!({}),
            },
        },
    });

    state.handle_gateway_event(StreamEvent::RunError {
        run_id: "run-1".into(),
        seq: 2,
        error: "provider exploded".into(),
        error_code: None,
    });

    assert_eq!(
        state.find_tool_mut("ghost").unwrap().status,
        ToolStatus::Unknown
    );
}

// ---------------------------------------------------------------------------
// Thread persistence: the settings a reopened terminal must come back with
// ---------------------------------------------------------------------------

fn snapshot(key: &str) -> SessionSnapshot {
    SessionSnapshot {
        session_key: key.to_string(),
        agent_id: "main".into(),
        mode: Some("code".into()),
        exec_tier: Some("ask".into()),
        think_level: Some("high".into()),
        memory_mode: Some("off".into()),
        model_pin: Some("claude-opus-5".into()),
        model_pin_provider: Some("anthropic".into()),
        model: Some("gpt-5".into()),
        model_provider: Some("openai".into()),
        input_tokens: 900,
        output_tokens: 340,
        total_tokens: 1_240,
        estimated_cost_usd: 0.12,
        message_count: 8,
        compaction_count: 1,
        project_root: Some("/tmp/proj".into()),
        label: None,
    }
}

/// The headline behaviour: reopening a conversation restores its own settings,
/// not the install defaults the client happened to launch with.
#[test]
fn attaching_restores_the_conversations_settings_and_token_count() {
    let mut state = AppState::new(String::new(), "install-default-model".into());
    assert_eq!(state.total_tokens, 0);

    state.apply_session_snapshot(snapshot("agent:main:main:s3"));

    assert_eq!(state.session_key, "agent:main:main:s3");
    assert_eq!(
        state.total_tokens, 1_240,
        "the counter must not restart at 0"
    );
    let knobs = state.session_knobs();
    assert_eq!(knobs.mode, Some("code"));
    assert_eq!(knobs.exec_tier, Some("ask"));
    assert_eq!(knobs.think_level, Some("high"));
    assert_eq!(knobs.memory_mode, Some("off"));
}

/// A pinned model wins over the model that last served: the pick applies from
/// the next run, so showing `model` alone names the model the user just left.
#[test]
fn the_caption_shows_the_pin_not_the_model_it_replaced() {
    let mut state = AppState::new(String::new(), "install-default-model".into());
    state.apply_session_snapshot(snapshot("k"));
    assert_eq!(state.model_name, "claude-opus-5");
}

/// A conversation that has never run names no model. The caption must fall back
/// to the install default rather than keeping whatever the previous session had.
#[test]
fn a_conversation_with_no_model_falls_back_to_the_install_default() {
    let mut state = AppState::new(String::new(), "install-default-model".into());
    state.apply_session_snapshot(snapshot("first"));
    assert_eq!(state.model_name, "claude-opus-5");

    let fresh = SessionSnapshot {
        session_key: "second".into(),
        ..SessionSnapshot::default()
    };
    state.apply_session_snapshot(fresh);
    assert_eq!(state.model_name, "install-default-model");
}

/// Per-conversation state in a singleton component: switching must not carry
/// the previous conversation's spend or settings into the new one's status bar.
#[test]
fn switching_sessions_clears_the_previous_conversations_state() {
    let mut state = AppState::new("old".into(), "install-default-model".into());
    state.apply_session_snapshot(snapshot("old"));
    assert_eq!(state.total_tokens, 1_240);

    state.switch_session("agent:main:main:s9");

    assert_eq!(state.session_key, "agent:main:main:s9");
    assert_eq!(state.total_tokens, 0, "token count bled across the switch");
    assert_eq!(state.session_knobs(), SessionKnobs::default());
    assert_eq!(state.model_name, "install-default-model");
}

/// The gateway is the only authority on which key a run was routed to: an
/// unparseable key does not fail the call, it makes the router mint a fresh
/// epoch. A client that kept its own guess would address nothing.
#[test]
fn the_canonical_key_from_the_server_is_adopted() {
    let mut state = AppState::new(String::new(), "m".into());
    state.adopt_canonical_session_key("agent:main:main:s4");
    assert_eq!(state.session_key, "agent:main:main:s4");
}

/// …but an empty or unchanged key is a no-op: a server that omits the field
/// must not be able to blank the key the client is using.
#[test]
fn an_absent_canonical_key_does_not_blank_the_session() {
    let mut state = AppState::new("agent:main:main".into(), "m".into());
    state.adopt_canonical_session_key("");
    assert_eq!(state.session_key, "agent:main:main");
}

/// Adopting a *different* key drops the settings we hold: they describe the key
/// we just replaced, and a status bar confidently describing someone else's
/// conversation is worse than one that says nothing.
#[test]
fn adopting_a_different_key_drops_the_stale_settings() {
    let mut state = AppState::new("old".into(), "m".into());
    state.apply_session_snapshot(snapshot("old"));
    state.adopt_canonical_session_key("agent:main:main:s7");
    assert_eq!(state.session_knobs(), SessionKnobs::default());
}

/// A locally-set knob shows immediately, without waiting for the next attach —
/// but only through `record_local_knob`, which callers reach only after the
/// server has accepted the write.
#[test]
fn a_locally_recorded_knob_is_visible_before_the_next_attach() {
    let mut state = AppState::new("agent:main:main".into(), "m".into());
    assert_eq!(state.session_knobs().exec_tier, None);

    state.record_local_knob(SessionKnob::ExecTier, Some("full".into()));
    assert_eq!(state.session_knobs().exec_tier, Some("full"));

    // Clearing back to "follow global" is `None`, not a literal.
    state.record_local_knob(SessionKnob::ExecTier, None);
    assert_eq!(state.session_knobs().exec_tier, None);
}

/// A question that ends without this client answering it must take its card
/// with it. Before the protocol carried the terminal frame, an expired or
/// cancelled clarification left the overlay holding focus and claiming the
/// agent was waiting — for up to the 600 s timeout.
#[test]
fn a_clarification_that_ends_elsewhere_retires_the_card() {
    let mut state = AppState::new("s".into(), "m".into());
    state.show_dialog("telegram:bot:1:u1".into(), menu_view("Approve?"));
    assert_eq!(state.focus, Focus::Dialog);

    state.handle_gateway_event(StreamEvent::ClarificationEnded {
        session_key: "telegram:bot:1:u1".into(),
        outcome: "expired".into(),
    });

    assert!(state.dialog.is_none(), "the card must go with the question");
    assert_eq!(state.focus, Focus::Input);
    // The user is told WHICH ending it was: vanishing silently leaves them
    // unable to tell "answered" from "timed out while I was reading".
    match state.messages.last() {
        Some(ChatMessage::System { content }) => {
            assert!(content.contains("expired"), "{content}");
        }
        other => panic!("expected a system line naming the outcome, got {other:?}"),
    }
}

/// …but only its own. Two sessions are live in one TUI whenever a background
/// run answers elsewhere; closing on a foreign frame would yank a card the user
/// is mid-answer on.
#[test]
fn a_clarification_ending_in_another_session_leaves_this_card_alone() {
    let mut state = AppState::new("s".into(), "m".into());
    state.show_dialog("telegram:bot:1:u1".into(), menu_view("Approve?"));

    state.handle_gateway_event(StreamEvent::ClarificationEnded {
        session_key: "telegram:bot:1:SOMEONE-ELSE".into(),
        outcome: "cancelled".into(),
    });

    assert!(
        state.dialog.is_some(),
        "a foreign frame must not close this card"
    );
    assert_eq!(state.focus, Focus::Dialog);
}

// ---------------------------------------------------------------------------
// Provider / model picker
// ---------------------------------------------------------------------------

/// A catalogue in a deliberately non-alphabetical, curated order — the order
/// the server sends and the ranker must not shuffle within a tier.
fn catalog() -> Vec<CatalogEntry> {
    vec![
        sample_catalog_entry("openrouter", &["openai/gpt-5.6"]),
        sample_catalog_entry("openai", &["gpt-5.6", "gpt-5.6-luna"]),
    ]
}

/// The two levels share one matcher, so the ranker's rules hold at both.
///
/// A provider named by the query keeps its whole roster; a provider surfaced
/// only because one of its models matched shows that model and says so. A
/// second matcher for the model level is what would let the row a bare Enter
/// selects disagree with the level above it.
#[test]
fn both_picker_levels_rank_through_the_shared_matcher() {
    let c = catalog();

    // Provider level: an exact id outranks a provider that merely resells it,
    // even though the reseller is listed first.
    let rows = provider_picker_rows(&c, None, "openai");
    assert_eq!(
        rows[0],
        PickerRow::Provider {
            index: 1,
            matched: 2
        },
        "the exact id must be the row a bare Enter selects"
    );
    assert_eq!(
        rows[1],
        PickerRow::Provider {
            index: 0,
            matched: 1
        },
        "the reseller surfaced through one model, and reports one match"
    );

    // Model level, same query: it names the provider, so every model stays.
    let ids = model_ids(provider_picker_rows(&c, Some(1), "openai"));
    assert_eq!(ids, vec!["gpt-5.6", "gpt-5.6-luna"]);

    // Model level, a query naming one model: narrowed to it.
    assert_eq!(
        model_ids(provider_picker_rows(&c, Some(1), "luna")),
        vec!["gpt-5.6-luna"]
    );
}

fn model_ids(rows: Vec<PickerRow>) -> Vec<String> {
    rows.into_iter()
        .map(|r| match r {
            PickerRow::Model { model } => model.id,
            other => panic!("expected a model row, got {other:?}"),
        })
        .collect()
}

/// The filter survives a descent.
///
/// Typing a model id and pressing Enter twice has to land on that model. Were
/// the descent to clear the filter, the query that found the provider would
/// have to be typed again against its full roster — and a user has no reason to
/// expect their own keystrokes to be dropped halfway through one act.
#[test]
fn a_descent_keeps_the_query_that_found_the_provider() {
    let mut state = AppState::new("s".into(), "m".into());
    state.open_provider_picker(catalog(), "luna".into());
    assert_eq!(state.focus, Focus::ProviderPicker);

    // One provider matched, and through one of its models.
    assert_eq!(state.provider_picker.as_ref().unwrap().rows.len(), 1);
    let Some(ProviderPick::Provider(index)) = state.selected_provider_pick() else {
        panic!("the top level offers providers");
    };
    state.enter_provider(index);

    assert_eq!(
        state.selected_provider_pick(),
        Some(ProviderPick::Model("gpt-5.6-luna".into())),
        "the query has to still be narrowing after the descent"
    );
}

/// Backspace climbs before it closes, and only from a level that has one above.
#[test]
fn going_back_is_only_possible_from_the_model_level() {
    let mut state = AppState::new("s".into(), "m".into());
    state.open_provider_picker(catalog(), String::new());

    assert!(
        !state.provider_picker_go_back(),
        "the provider level has nowhere to climb to; the caller closes instead"
    );

    state.enter_provider(1);
    assert!(state.provider_picker_go_back());
    assert!(state.provider_picker.as_ref().unwrap().provider.is_none());
    assert_eq!(
        state.provider_picker.as_ref().unwrap().rows.len(),
        2,
        "climbing back restores the provider rows"
    );
}

/// An empty match set confirms to nothing, rather than to row zero of a list
/// that is not there.
#[test]
fn confirming_an_empty_filter_picks_nothing() {
    let mut state = AppState::new("s".into(), "m".into());
    state.open_provider_picker(catalog(), "no-such-vendor".into());
    assert!(state.provider_picker.as_ref().unwrap().rows.is_empty());
    assert_eq!(state.selected_provider_pick(), None);
}

/// Switching conversations drops the picker.
///
/// It is a singleton overlay in a client that holds one conversation at a time:
/// a pick left mid-flight would be confirmed against the session just left.
#[test]
fn switching_session_retires_the_provider_picker() {
    let mut state = AppState::new("s".into(), "m".into());
    state.open_provider_picker(catalog(), String::new());
    state.switch_session("other");
    assert!(state.provider_picker.is_none());
    assert_eq!(state.focus, Focus::Input);
}

// ---------------------------------------------------------------------------
// Cross-session frame routing: the `frame_belongs_here` guard
// ---------------------------------------------------------------------------
//
// Background runs, cron runs and delegated subagents all publish frames
// carrying a `run_id` but, with three exceptions, no `session_key` at all —
// see `StreamEvent::run_id()`'s doc in shared/protocol. The gateway's own
// visibility filter is owner-scoped, not session-scoped, so on a single-user
// box every one of those frames reaches every open TUI connection.
// `RunAccepted` is the one frame that names both `run_id` and `session_key`;
// these tests drive it as the teaching signal `frame_belongs_here` learns
// from, per Controller Ruling R7 (2026-08-20-btw-side-questions).

#[test]
fn frame_belongs_here_learns_from_run_accepted() {
    let mut state = AppState::new("s".into(), "m".into());

    // A run whose OWN `RunAccepted` named a different session is proven
    // foreign: dropped.
    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-foreign".into(),
        session_key: "agent:main:ephemeral:btw-abc".into(),
        accepted_at: "2026-03-04T00:00:00Z".into(),
    });
    assert!(!state.frame_belongs_here("run-foreign"));

    // A run whose `RunAccepted` named THIS session: kept.
    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-mine".into(),
        session_key: "s".into(),
        accepted_at: "2026-03-04T00:00:01Z".into(),
    });
    assert!(state.frame_belongs_here("run-mine"));

    // A run id this screen has never seen a `RunAccepted` for at all: "I
    // cannot tell" must not become "drop" — a new session's first turn, and
    // every frame from a core too old to send `RunAccepted`, must not go
    // silently missing.
    assert!(state.frame_belongs_here("run-never-announced"));
}

#[test]
fn a_response_chunk_from_another_sessions_run_is_not_appended() {
    let mut state = AppState::new("s".into(), "m".into());
    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-elsewhere".into(),
        session_key: "agent:main:ephemeral:btw-abc".into(),
        accepted_at: "2026-03-04T00:00:00Z".into(),
    });
    state.handle_gateway_event(StreamEvent::ResponseChunk {
        run_id: "run-elsewhere".into(),
        seq: 1,
        content: "not for you".into(),
        chunk_index: 0,
        is_final: false,
        is_intermediate: false,
    });
    // Only the welcome message — nothing was appended.
    assert_eq!(state.messages.len(), 1);
}

/// The same text a streaming turn produces twice on the wire (see
/// `turn_streamed_len`'s doc: `ResponseChunk` deltas AND, in full,
/// `AgentTrace{TextEmitted{Final}}`, because the TUI subscribes to no
/// topics). Both carriers must be guarded, not just the more obvious one.
#[test]
fn an_agent_trace_text_event_from_another_sessions_run_is_not_appended() {
    let mut state = AppState::new("s".into(), "m".into());
    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-elsewhere".into(),
        session_key: "agent:main:ephemeral:btw-abc".into(),
        accepted_at: "2026-03-04T00:00:00Z".into(),
    });
    state.handle_gateway_event(StreamEvent::AgentTrace {
        run_id: "run-elsewhere".into(),
        seq: 1,
        event: AgentTraceEvent::TextEmitted {
            iteration: 1,
            stream: AgentTraceTextKind::Final,
            text: "not for you either".into(),
        },
    });
    assert_eq!(state.messages.len(), 1);
}

#[test]
fn a_response_chunk_from_this_sessions_run_is_still_appended() {
    let mut state = AppState::new("s".into(), "m".into());
    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-mine".into(),
        session_key: "s".into(),
        accepted_at: "2026-03-04T00:00:00Z".into(),
    });
    state.handle_gateway_event(StreamEvent::ResponseChunk {
        run_id: "run-mine".into(),
        seq: 1,
        content: "hello".into(),
        chunk_index: 0,
        is_final: false,
        is_intermediate: false,
    });
    assert_eq!(state.messages.len(), 2);
    match &state.messages[1] {
        ChatMessage::Assistant { content, .. } => assert_eq!(content, "hello"),
        other => panic!("Expected Assistant message, got: {other:?}"),
    }
}

#[test]
fn a_response_chunk_from_a_run_never_announced_is_still_appended() {
    // No `RunAccepted` was ever seen for this run_id — a race that lost it,
    // or a core too old to send one at all.
    let mut state = AppState::new("s".into(), "m".into());
    state.handle_gateway_event(StreamEvent::ResponseChunk {
        run_id: "run-never-announced".into(),
        seq: 1,
        content: "hello".into(),
        chunk_index: 0,
        is_final: false,
        is_intermediate: false,
    });
    assert_eq!(state.messages.len(), 2);
}

/// `ModelResolved` and `ContextGauge` do not append visible chat text, but a
/// foreign run's status repainting this screen's gauge/model indicator is
/// the same defect wearing a different hat — the guard covers both, not just
/// the transcript-text arms.
#[test]
fn a_context_gauge_from_another_sessions_run_is_not_applied() {
    let mut state = AppState::new("s".into(), "m".into());
    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-elsewhere".into(),
        session_key: "agent:main:ephemeral:btw-abc".into(),
        accepted_at: "2026-03-04T00:00:00Z".into(),
    });
    state.handle_gateway_event(StreamEvent::ContextGauge {
        run_id: "run-elsewhere".into(),
        seq: 1,
        context_tokens: 500,
        context_window: 8000,
        total_tokens: 500,
    });
    assert!(state.context_gauge.is_none());
}

/// `AskUser` is deliberately exempt from this guard (see `run_scoped_id`'s
/// doc comment; the original task brief called this out of scope). This test
/// locks the decision in so a future change to the guard's shape does not
/// silently start hiding clarification cards for background/delegated runs.
#[test]
fn an_ask_user_from_another_sessions_run_is_still_shown() {
    let mut state = AppState::new("s".into(), "m".into());
    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-elsewhere".into(),
        session_key: "agent:main:ephemeral:btw-abc".into(),
        accepted_at: "2026-03-04T00:00:00Z".into(),
    });
    state.handle_gateway_event(StreamEvent::AskUser {
        run_id: "run-elsewhere".into(),
        seq: 1,
        session_key: "agent:main:ephemeral:btw-abc".into(),
        question: "Proceed?".into(),
        options: vec!["Yes".into(), "No".into()],
        questions: vec![],
        answered: 0,
    });
    assert!(state.dialog.is_some());
}

/// A conversation switch is the same defect from a different trigger: the
/// run left behind in the old session keeps streaming, and its frames must
/// not leak into the new one.
#[test]
fn a_run_left_behind_by_a_session_switch_does_not_leak_into_the_new_session() {
    let mut state = AppState::new("s1".into(), "m".into());
    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-1".into(),
        session_key: "s1".into(),
        accepted_at: "2026-03-04T00:00:00Z".into(),
    });
    state.switch_session("s2");

    state.handle_gateway_event(StreamEvent::ResponseChunk {
        run_id: "run-1".into(),
        seq: 2,
        content: "late arrival".into(),
        chunk_index: 0,
        is_final: false,
        is_intermediate: false,
    });

    assert!(
        state
            .messages
            .iter()
            .all(|m| !matches!(m, ChatMessage::Assistant { .. })),
        "the abandoned run's text must not land in the new session's transcript"
    );
}

/// The other half of the same property: a run's home session, once learned,
/// does not get baked in as "foreign forever" — switching away and back
/// must let its frames resume landing where they actually belong. Without
/// this, `mark_run_session`'s "own or foreign" recording would be pointless:
/// a set that only ever grew stale foreign entries would trap this exact
/// case just as badly as the leak it replaced.
#[test]
fn a_run_resumes_appearing_after_switching_back_to_its_own_session() {
    let mut state = AppState::new("s1".into(), "m".into());
    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-1".into(),
        session_key: "s1".into(),
        accepted_at: "2026-03-04T00:00:00Z".into(),
    });
    state.switch_session("s2");
    state.switch_session("s1");

    state.handle_gateway_event(StreamEvent::ResponseChunk {
        run_id: "run-1".into(),
        seq: 2,
        content: "still mine".into(),
        chunk_index: 0,
        is_final: false,
        is_intermediate: false,
    });

    match state.messages.last() {
        Some(ChatMessage::Assistant { content, .. }) => assert_eq!(content, "still mine"),
        other => panic!("Expected Assistant message, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// `/btw` side questions
// ---------------------------------------------------------------------------

/// Drive a whole side question through the frame path exactly as the gateway
/// would, and prove **nothing about it** reaches the main conversation.
///
/// This is the assertion Step 5 of this task makes on a real machine, made
/// here where it can be run on every commit. It is also the one that fails if
/// the intercept is removed or moved after the cross-session guard: the answer
/// would land in `messages` as an assistant bubble in a conversation the side
/// question was deliberately kept out of.
///
/// The session key on every side frame is the DERIVED one — that is what the
/// engine emits (`execute.rs` redirects before it emits `RunAccepted`), and it
/// is why keying the overlay on the key was never an option from a client that
/// cannot compute it.
#[test]
fn no_side_question_frame_reaches_the_main_transcript() {
    let mut state = AppState::new("agent:main:main".into(), "m".into());
    let before = state.messages.len();

    // A main run is in flight — a side question exists to be asked *while*
    // one is.
    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-main".into(),
        session_key: "agent:main:main".into(),
        accepted_at: "2026-08-20T00:00:00Z".into(),
    });

    // The user asks a side question; the send site claims the run it got back.
    state.open_btw("what files are in src?".into());
    state.btw.claim_pending_run("run-side".into());

    for event in [
        StreamEvent::RunAccepted {
            run_id: "run-side".into(),
            session_key: "agent:main:ephemeral:btw-abc123".into(),
            accepted_at: "2026-08-20T00:00:01Z".into(),
        },
        StreamEvent::AgentTrace {
            run_id: "run-side".into(),
            seq: 1,
            event: AgentTraceEvent::ToolCallStarted {
                iteration: 1,
                call: aleph_protocol::AgentTraceToolCallStart {
                    tool_id: "t1".into(),
                    tool_name: "file_read".into(),
                    input: serde_json::json!({}),
                },
            },
        },
        StreamEvent::ResponseChunk {
            run_id: "run-side".into(),
            seq: 2,
            content: "main.rs, lib.rs".into(),
            chunk_index: 0,
            is_final: false,
            is_intermediate: false,
        },
        StreamEvent::RunComplete {
            run_id: "run-side".into(),
            seq: 3,
            summary: RunSummary::default(),
            total_duration_ms: 12,
        },
    ] {
        state.handle_gateway_event(event);
    }

    // The overlay has the whole exchange…
    let exchange = state.btw.current().expect("the side question settled");
    assert_eq!(exchange.question, "what files are in src?");
    assert_eq!(exchange.answer, "main.rs, lib.rs");

    // …and the main conversation has none of it.
    assert_eq!(
        state.messages.len(),
        before,
        "a side question must add nothing to the main transcript, got: {:?}",
        &state.messages[before..]
    );
    for message in &state.messages {
        let text = match message {
            ChatMessage::User { content, .. }
            | ChatMessage::System { content }
            | ChatMessage::Assistant { content, .. } => content,
        };
        assert!(
            !text.contains("main.rs, lib.rs"),
            "the side answer leaked into the transcript: {text}"
        );
    }

    // The main run is untouched: the side question neither claimed the run
    // slot nor cleared it on its own completion.
    assert_eq!(
        state.current_run,
        Some("run-main".into()),
        "a side question's RunComplete must not end the main run"
    );
}

/// The side thread does not survive a `/session` switch.
///
/// `AppState` is a singleton — one conversation's worth of everything — so
/// per-conversation state that outlives a switch reports the previous
/// conversation under the new one's name (`switch_session` wipes `messages`,
/// `total_tokens`, the gauge and the cache stat for exactly this reason). The
/// overlay is per-conversation twice over: its exchanges are that
/// conversation's, and its run claims would route the OLD conversation's side
/// frames into the NEW screen's overlay.
///
/// It is cleared rather than keyed per conversation — see `switch_session` for
/// why, the short version being that the side key embeds the old key's epoch,
/// so after a switch the retained thread is one the server can no longer
/// address at all.
#[test]
fn switching_sessions_takes_the_side_thread_with_it() {
    let mut state = AppState::new("agent:main:main".into(), "m".into());
    state.open_btw("what files are in src?".into());
    state.btw.claim_pending_run("run-side".into());
    state.btw.push_delta("run-side", "main.rs");
    state.btw.finish_active("run-side", None);
    state.open_btw("and tests?".into());
    state.btw.claim_pending_run("run-side-2".into());
    assert_eq!(state.btw.exchanges.len(), 1);
    assert!(state.btw.open);

    state.switch_session("agent:main:main:s2");

    assert!(
        state.btw.exchanges.is_empty(),
        "the old conversation's side questions are still pageable: {:?}",
        state.btw.exchanges
    );
    assert!(
        state.btw.active.is_none(),
        "an old side run is still active"
    );
    assert!(!state.btw.open, "the overlay is still on screen");
    // The run CLAIMS survive on purpose — they are per-run, not
    // per-conversation, and they are the only thing that can still tell a
    // side frame apart from an ordinary one when that run's `RunAccepted`
    // never arrived. Dropping them is what let the old side answer render
    // into the new conversation.
    assert!(
        state.btw.accepts_frame(Some("run-side-2")),
        "the old conversation's side run is no longer recognised, so its \
         frames will be treated as this conversation's"
    );

    // …and a frame from it now lands nowhere: intercepted, but matching no
    // active question.
    let before = state.messages.len();
    state.handle_gateway_event(StreamEvent::ResponseChunk {
        run_id: "run-side-2".into(),
        seq: 1,
        content: "the old side answer".into(),
        chunk_index: 0,
        is_final: false,
        is_intermediate: false,
    });
    assert!(state.btw.active.is_none());
    assert_eq!(
        state.messages.len(),
        before,
        "the old side answer reached the new conversation: {:?}",
        &state.messages[before..]
    );
}

/// A modal raised over the side-question overlay must hand focus back to it,
/// not to the composer.
///
/// A clarification and an approval card are both raised by *frames*, so
/// neither needs the user to have left the overlay first. Returning focus to
/// `Input` left the side question painted over the transcript with every key
/// going to a textarea the user could not see — and `Esc` at `Focus::Input` is
/// a no-op, so there was no way out but `/stop` or quitting.
#[test]
fn answering_a_modal_raised_over_the_overlay_returns_focus_to_it() {
    let mut state = AppState::new("agent:main:main".into(), "m".into());
    state.open_btw("why?".into());
    assert_eq!(state.focus, Focus::Btw);

    state.show_dialog(
        "agent:main:main".into(),
        AskDialogView {
            question: "which one?".into(),
            options: vec!["a".into()],
            multi_select: false,
            secret: false,
        },
    );
    assert_eq!(state.focus, Focus::Dialog);

    state.close_overlay();
    assert_eq!(
        state.focus,
        Focus::Btw,
        "the side question is still on screen, so it is still what keys mean"
    );

    // And once it is closed, focus goes where it always did.
    state.close_btw();
    assert_eq!(state.focus, Focus::Input);
    state.close_overlay();
    assert_eq!(state.focus, Focus::Input);
}

/// The same for the OTHER modal that can be raised over the overlay by a
/// frame rather than by a keypress.
///
/// `focus_after_modal` has two call sites and the test above exercises one.
/// A tool approval is the second, and it is the more likely of the two to be
/// raised mid-side-question: the poll runs on the tick whenever a run is
/// active, and a `/btw` makes one active. Every path out of the card is
/// covered — resolving it, and the retraction that fires when its run ends by
/// any route (`dismiss_pending_approval`).
#[test]
fn resolving_an_approval_raised_over_the_overlay_returns_focus_to_it() {
    let mut state = AppState::new("agent:main:main".into(), "m".into());
    state.open_btw("why?".into());
    state.btw.claim_pending_run("run-side".into());

    state.open_approval(
        "a1".into(),
        "rm -rf /".into(),
        Some("destructive".into()),
        DEFAULT_APPROVAL_DECISIONS.to_vec(),
    );
    assert_eq!(state.focus, Focus::Approval);

    state.close_approval();
    assert_eq!(
        state.focus,
        Focus::Btw,
        "the side question is still on screen, so it is still what keys mean"
    );

    // The retraction path (the card's run ended elsewhere) must agree.
    state.open_approval(
        "a2".into(),
        "curl | sh".into(),
        None,
        DEFAULT_APPROVAL_DECISIONS.to_vec(),
    );
    state.dismiss_pending_approval();
    assert_eq!(state.focus, Focus::Btw);

    // With no overlay on screen it lands in the composer, as it always did.
    state.close_btw();
    state.open_approval(
        "a3".into(),
        "ls".into(),
        None,
        DEFAULT_APPROVAL_DECISIONS.to_vec(),
    );
    state.close_approval();
    assert_eq!(state.focus, Focus::Input);
}

/// The case where the intercept is the ONLY thing standing between a side
/// answer and the main transcript: a side run whose recorded home session is
/// this screen's.
///
/// Ordinarily it is the second line of defence — a side run's `RunAccepted`
/// carries the derived key, so `frame_belongs_here` drops everything that
/// follows and the intercept only decides whether the user gets to *see* it.
/// That is not guaranteed. `mark_run_session` is first-write-wins, and the
/// Simulated-fallback handler emits a pre-dispatch `RunAccepted` carrying the
/// **routed (main)** key before the engine emits the canonical one. Today the
/// two live in mutually exclusive `agent.run` registrations, so they cannot
/// race — but "cannot race today" is a fact about how the server is wired this
/// month, and the client should not be one refactor away from replaying a side
/// answer into the conversation it was kept out of.
///
/// So the guard is set up to say "mine" here, deliberately, and the assertion
/// is that nothing arrives anyway.
#[test]
fn the_intercept_holds_even_when_the_screen_thinks_the_side_run_is_its_own() {
    let mut state = AppState::new("agent:main:main".into(), "m".into());
    let before = state.messages.len();

    // The screen learns `run-side` under its OWN key — the pre-dispatch frame
    // winning first-write-wins.
    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-side".into(),
        session_key: "agent:main:main".into(),
        accepted_at: "2026-08-20T00:00:00Z".into(),
    });
    assert!(
        state.frame_belongs_here("run-side"),
        "precondition: the cross-session guard now believes this run is ours, \
         so it will NOT drop what follows"
    );

    state.open_btw("what files are in src?".into());
    state.btw.claim_pending_run("run-side".into());

    state.handle_gateway_event(StreamEvent::ResponseChunk {
        run_id: "run-side".into(),
        seq: 1,
        content: "main.rs, lib.rs".into(),
        chunk_index: 0,
        is_final: false,
        is_intermediate: false,
    });

    assert_eq!(
        state.btw.active.as_ref().expect("still answering").answer,
        "main.rs, lib.rs"
    );
    assert_eq!(
        state.messages.len(),
        before,
        "the side answer reached the main transcript: {:?}",
        &state.messages[before..]
    );
}

/// The intercept must be exactly as wide as the runs the overlay asked for.
/// A frame from the main run has to reach the transcript even while a side
/// question is streaming, or `/btw` would silently freeze the conversation it
/// was supposed to run beside.
#[test]
fn a_side_question_in_flight_does_not_swallow_the_main_run() {
    let mut state = AppState::new("agent:main:main".into(), "m".into());
    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-main".into(),
        session_key: "agent:main:main".into(),
        accepted_at: "2026-08-20T00:00:00Z".into(),
    });
    state.open_btw("side".into());
    state.btw.claim_pending_run("run-side".into());

    state.handle_gateway_event(StreamEvent::ResponseChunk {
        run_id: "run-main".into(),
        seq: 1,
        content: "the main answer".into(),
        chunk_index: 0,
        is_final: false,
        is_intermediate: false,
    });

    match state.messages.last().expect("a message") {
        ChatMessage::Assistant { content, .. } => assert_eq!(content, "the main answer"),
        other => panic!("expected the main run's assistant bubble, got: {other:?}"),
    }
    assert!(
        state
            .btw
            .active
            .as_ref()
            .expect("still answering")
            .answer
            .is_empty(),
        "the main run's text must not be routed into the side question"
    );
}

/// An `AskUser` frame carries no run id, so it is exempt from the screen's
/// cross-session guard by design. The overlay must not swallow it — a parked
/// question hidden behind an overlay that cannot answer it is a 600-second
/// stall with no visible cause.
#[test]
fn the_overlay_does_not_swallow_a_run_less_frame() {
    let mut state = AppState::new("agent:main:main".into(), "m".into());
    state.open_btw("side".into());
    state.btw.claim_pending_run("run-side".into());

    state.handle_gateway_event(StreamEvent::AskUser {
        run_id: String::new(),
        seq: 1,
        session_key: "agent:main:main".into(),
        question: "which one?".into(),
        options: vec!["a".into(), "b".into()],
        questions: Vec::new(),
        answered: 0,
    });

    assert!(
        state.dialog.is_some(),
        "a session-keyed clarification must still reach its overlay"
    );
}

// ---------------------------------------------------------------------------
// The terminal answer of a run that produced no other copy of it
// ---------------------------------------------------------------------------

/// A run the gateway SERVED rather than dispatched — `/btw promote` is the
/// first — never emits a chunk or a trace, so `summary.final_response` is the
/// only carrier its answer ever gets.
///
/// This is the surface that owns the `p` key, and before this it showed a
/// spinner and then nothing: for the success receipt AND for "nothing to
/// promote", while the failure receipt (`RunError`) spoke. The asymmetry fell
/// exactly the wrong way round, and it compounded — a user who presses `p`,
/// sees nothing, and presses it again carries a second full-size answer into
/// the main conversation with no signal either time.
#[test]
fn a_run_that_streamed_nothing_still_renders_its_terminal_answer() {
    let mut state = AppState::new("s".into(), "m".into());
    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-promote".into(),
        session_key: "s".into(),
        accepted_at: "2026-03-04T00:00:00Z".into(),
    });
    state.handle_gateway_event(StreamEvent::RunComplete {
        run_id: "run-promote".into(),
        seq: 1,
        summary: RunSummary {
            final_response: Some("Promoted the side answer into this conversation: why?".into()),
            ..Default::default()
        },
        total_duration_ms: 0,
    });

    let rendered: Vec<&String> = state
        .messages
        .iter()
        .filter_map(|m| match m {
            ChatMessage::Assistant { content, .. } => Some(content),
            _ => None,
        })
        .collect();
    assert_eq!(
        rendered.len(),
        1,
        "the receipt must reach the transcript: {:?}",
        state.messages
    );
    assert_eq!(
        rendered[0], "Promoted the side answer into this conversation: why?",
        "the terminal answer is the only copy a served run ever sends"
    );
}

/// The other half, and the reason this is a per-run flag rather than an
/// unconditional render: an ordinary streamed turn puts the same text on the
/// wire twice (deltas, then `final_response`), so rendering the summary
/// unconditionally would double every reply in the transcript.
#[test]
fn a_streamed_answer_is_not_doubled_by_its_own_summary() {
    let mut state = AppState::new("s".into(), "m".into());
    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-ordinary".into(),
        session_key: "s".into(),
        accepted_at: "2026-03-04T00:00:00Z".into(),
    });
    state.handle_gateway_event(StreamEvent::ResponseChunk {
        run_id: "run-ordinary".into(),
        seq: 1,
        content: "the answer".into(),
        chunk_index: 0,
        is_final: false,
        is_intermediate: false,
    });
    state.handle_gateway_event(StreamEvent::RunComplete {
        run_id: "run-ordinary".into(),
        seq: 2,
        summary: RunSummary {
            final_response: Some("the answer".into()),
            ..Default::default()
        },
        total_duration_ms: 0,
    });

    match state.messages.last().unwrap() {
        ChatMessage::Assistant { content, .. } => assert_eq!(
            content, "the answer",
            "the summary is a duplicate of what streaming already rendered"
        ),
        other => panic!("expected an assistant message, got {other:?}"),
    }
}

/// The flag belongs to the run, not to the screen: a second served run in the
/// same session must render its own answer even though the first one rendered
/// text a moment earlier.
#[test]
fn the_streamed_flag_is_cleared_when_the_next_run_is_accepted() {
    let mut state = AppState::new("s".into(), "m".into());
    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-one".into(),
        session_key: "s".into(),
        accepted_at: "2026-03-04T00:00:00Z".into(),
    });
    state.handle_gateway_event(StreamEvent::ResponseChunk {
        run_id: "run-one".into(),
        seq: 1,
        content: "first".into(),
        chunk_index: 0,
        is_final: false,
        is_intermediate: false,
    });
    state.handle_gateway_event(StreamEvent::RunComplete {
        run_id: "run-one".into(),
        seq: 2,
        summary: RunSummary::default(),
        total_duration_ms: 0,
    });

    state.handle_gateway_event(StreamEvent::RunAccepted {
        run_id: "run-two".into(),
        session_key: "s".into(),
        accepted_at: "2026-03-04T00:00:01Z".into(),
    });
    state.handle_gateway_event(StreamEvent::RunComplete {
        run_id: "run-two".into(),
        seq: 3,
        summary: RunSummary {
            final_response: Some("there is nothing to promote".into()),
            ..Default::default()
        },
        total_duration_ms: 0,
    });

    let joined: String = state
        .messages
        .iter()
        .filter_map(|m| match m {
            ChatMessage::Assistant { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("|");
    assert!(
        joined.contains("there is nothing to promote"),
        "the second run's receipt was suppressed by the first run's text: {joined:?}"
    );
}
