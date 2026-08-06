//! Consolidated unit tests for the harness_bridge module.
#![allow(unused_imports)]

use crate::sync_primitives::Arc;
use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;

use crate::agents::AgentRegistry;
use crate::context::budget::{ContextBudget, ContextBudgetConfig};
use crate::context::compact::compactor::{CompactorConfig, ContextCompactor};
use crate::harness::agent::AgentHarness;
use crate::harness::callback::HarnessCallback;
use crate::harness::deps::HarnessDeps;
use crate::mcp::manager::McpManagerHandle;
use crate::memory::store::MemoryBackend;
use crate::orchestrator::dispatch::{FlowOutcome, FlowStreamEvent, HarnessRunner};
use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::{FlowInput, FlowSpec};
use crate::providers::{AiProvider, DefaultProviderHandle};
use crate::routing::session_key::SessionKey;
use crate::sandbox::Sandbox;
use crate::session::events::SessionEvent;
use crate::session::service::{SessionId, SessionService};
use crate::tools::service::ToolService;
use crate::verification::VerifierChain;

use super::context_blocks::*;
use super::prompt_build::*;
use super::runner_impl::*;
use super::*;

use crate::orchestrator::flow_spec::FlowHistoryTurn;
use crate::session::events::MessageContent;
use session_seed::seed_session;

#[test]
fn broadcast_callback_fans_lifecycle_events() {
    let (tx, mut rx) = broadcast::channel::<FlowStreamEvent>(16);
    let mut cb = super::callback::BroadcastCallback::new(tx, 200_000);

    cb.on_delta("hello ");
    cb.on_delta("world");
    // Exactly one tool-start signal now exists. Its name-only twin
    // (`on_tool_call`) was deleted (D4): it had no call id, so the synthetic
    // `id: "legacy"` it used to invent produced a second, undeadable tool row
    // (`ToolCallDone` only ever carries the real id) that nothing keyed by call
    // id — the inline approval card — could pair against.
    cb.on_tool_call_start(
        "call-1",
        "read_file",
        &serde_json::json!({ "path": "a.rs" }),
    );

    let mut received = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        received.push(ev);
    }

    // 3 events: two Deltas + exactly ONE ToolCallStart for the one tool call.
    assert_eq!(
        received.len(),
        3,
        "one tool call must produce exactly one ToolCallStart; got {received:?}"
    );
    match &received[0] {
        FlowStreamEvent::Delta(s) => assert_eq!(s, "hello "),
        other => panic!("expected Delta(\"hello \"), got {other:?}"),
    }
    match &received[1] {
        FlowStreamEvent::Delta(s) => assert_eq!(s, "world"),
        other => panic!("expected Delta(\"world\"), got {other:?}"),
    }
    match &received[2] {
        FlowStreamEvent::ToolCallStart { id, name, args } => {
            assert_eq!(name, "read_file");
            assert_eq!(id, "call-1", "the emitted id must be the harness call id");
            assert_eq!(args, &serde_json::json!({ "path": "a.rs" }));
        }
        other => panic!("expected ToolCallStart, got {other:?}"),
    }
}

/// P4: `on_complete_with_outcome` is the single emitter of the terminal
/// `Complete(outcome)` event. The outcome payload survives the
/// callback → broadcast hop unchanged.
#[test]
fn broadcast_callback_on_complete_with_outcome_emits_terminal_event() {
    use crate::orchestrator::dispatch::{FlowOutcome, TerminateReason, TokenBreakdown};

    let (tx, mut rx) = broadcast::channel::<FlowStreamEvent>(16);
    let mut cb = super::callback::BroadcastCallback::new(tx, 200_000);

    let outcome = FlowOutcome {
        final_text: "all done".into(),
        iterations: 4,
        tool_calls_made: 2,
        total_tokens: 1500,
        hit_limit: true,
        terminate_reason: TerminateReason::HitMaxIterations { used: 4 },
        duration_ms: 1234,
        token_breakdown: TokenBreakdown {
            input: 800,
            output: 600,
            ..Default::default()
        },
        tool_timeline: Vec::new(),
        estimated_cost: None,
        context_tokens: 0,
        context_window: 0,
        serving_model: None,
        serving_provider: None,
    };
    cb.on_complete_with_outcome(&outcome);

    let received: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert_eq!(received.len(), 1, "exactly one Complete event");
    match &received[0] {
        FlowStreamEvent::Complete(o) => {
            assert_eq!(o.final_text, "all done");
            assert_eq!(o.iterations, 4);
            assert_eq!(o.tool_calls_made, 2);
            assert_eq!(o.total_tokens, 1500);
            assert!(o.hit_limit);
            assert_eq!(
                o.terminate_reason,
                TerminateReason::HitMaxIterations { used: 4 }
            );
            assert_eq!(o.duration_ms, 1234);
            assert_eq!(o.token_breakdown.input, 800);
            assert_eq!(o.token_breakdown.output, 600);
        }
        other => panic!("expected Complete, got {other:?}"),
    }
}

#[test]
fn classify_harness_error_network_is_transient() {
    let err = crate::harness::trait_def::HarnessError::Llm(crate::error::AlephError::network(
        "connection reset mid-stream",
    ));
    let out = super::error::classify_harness_error(err, "anthropic");
    assert!(matches!(out, FlowError::Transient { .. }));
}

#[test]
fn classify_harness_error_http_500_is_transient() {
    let err = crate::harness::trait_def::HarnessError::Llm(crate::error::AlephError::network(
        "upstream returned 500",
    ));
    let out = super::error::classify_harness_error(err, "anthropic");
    assert!(matches!(out, FlowError::Transient { .. }));
}

#[test]
fn classify_harness_error_generic_is_internal() {
    let err = crate::harness::trait_def::HarnessError::Llm(crate::error::AlephError::Other {
        message: "opaque failure".into(),
        suggestion: None,
    });
    let out = super::error::classify_harness_error(err, "anthropic");
    assert!(matches!(out, FlowError::Internal(_)));
}

#[test]
fn classify_harness_error_4500_is_not_server_transient() {
    // Word-boundary check: "4500" contains "500" substring but is not status 500.
    let err = crate::harness::trait_def::HarnessError::Llm(crate::error::AlephError::Other {
        message: "processed 4500 items then gave up".into(),
        suggestion: None,
    });
    let out = super::error::classify_harness_error(err, "anthropic");
    assert!(matches!(out, FlowError::Internal(_)));
}

/// Mid-run gauge cadence: `on_context_usage` fans a self-contained
/// `ContextGauge` event carrying the construction-time window, and stays
/// silent when either side is 0 (unknown window / no billed tokens).
#[test]
fn broadcast_callback_emits_context_gauge_with_prewired_window() {
    let (tx, mut rx) = broadcast::channel::<FlowStreamEvent>(16);
    let mut cb = super::callback::BroadcastCallback::new(tx, 200_000);

    cb.on_context_usage(0, 10); // no billed tokens → suppressed
    cb.on_context_usage(42_000, 55_000);

    let received: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert_eq!(received.len(), 1, "zero-occupancy call must be suppressed");
    match &received[0] {
        FlowStreamEvent::ContextGauge {
            context_tokens,
            context_window,
            total_tokens,
        } => {
            assert_eq!(*context_tokens, 42_000);
            assert_eq!(*context_window, 200_000);
            assert_eq!(*total_tokens, 55_000);
        }
        other => panic!("expected ContextGauge, got {other:?}"),
    }
}

#[test]
fn broadcast_callback_suppresses_context_gauge_without_window() {
    let (tx, mut rx) = broadcast::channel::<FlowStreamEvent>(16);
    let mut cb = super::callback::BroadcastCallback::new(tx, 0);
    cb.on_context_usage(42_000, 55_000);
    assert!(
        rx.try_recv().is_err(),
        "unknown window (0) must not emit a gauge event"
    );
}

#[test]
fn broadcast_callback_is_silent_when_no_receivers() {
    // No active receiver — `send` returns Err(SendError) but
    // BroadcastCallback swallows it so the harness loop is unaffected.
    let (tx, _rx) = broadcast::channel::<FlowStreamEvent>(1);
    drop(_rx);
    let mut cb = super::callback::BroadcastCallback::new(tx, 200_000);
    cb.on_delta("nobody is listening");
    cb.on_tool_call_start("call-1", "read_file", &serde_json::Value::Null);
    // No panic = pass.
}

// -- seed_session tests --------------------------------------------------

use crate::session::in_process::InProcessActorSessionService;
use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};

fn fresh_service() -> std::sync::Arc<dyn SessionService> {
    // rust-doctor-disable-next-line unwrap-in-production
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    migrate_add_session_events(&conn).unwrap();
    let store: std::sync::Arc<dyn SessionEventStore> =
        std::sync::Arc::new(SqliteEventStore::new(conn));
    std::sync::Arc::new(InProcessActorSessionService::new(store))
}

#[tokio::test]
async fn seed_session_prompt_emits_one_user_message() {
    let service = fresh_service();
    let sid = SessionKey::ephemeral("seed-prompt");
    super::session_seed::seed_session(service.as_ref(), &sid, FlowInput::Prompt("hello".into()))
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .expect("seed Prompt");

    // rust-doctor-disable-next-line unwrap-in-production
    let events = service.get_events(&sid, None, None).await.unwrap();
    let user_count = events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::UserMessage { .. }))
        .count();
    assert_eq!(user_count, 1);
}

#[tokio::test]
async fn seed_session_history_replays_turns_and_adds_prompt() {
    let service = fresh_service();
    let sid = SessionKey::ephemeral("seed-history");
    let turns = vec![
        FlowHistoryTurn::User(MessageContent {
            text: "q1".into(),
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        }),
        FlowHistoryTurn::Assistant(MessageContent {
            text: "a1".into(),
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        }),
        FlowHistoryTurn::User(MessageContent {
            text: "q2".into(),
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        }),
        FlowHistoryTurn::Assistant(MessageContent {
            text: "a2".into(),
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        }),
    ];
    seed_session(
        service.as_ref(),
        &sid,
        FlowInput::History {
            turns,
            prompt: "q3".into(),
        },
    )
    .await
    // rust-doctor-disable-next-line unwrap-in-production
    .expect("seed History");

    // rust-doctor-disable-next-line unwrap-in-production
    let events = service.get_events(&sid, None, None).await.unwrap();
    let users: Vec<String> = events
        .iter()
        .filter_map(|r| match &r.event {
            // rust-doctor-disable-next-line excessive-clone
            SessionEvent::UserMessage { content, .. } => Some(content.text.clone()),
            _ => None,
        })
        .collect();
    let assistants: Vec<String> = events
        .iter()
        .filter_map(|r| match &r.event {
            // rust-doctor-disable-next-line excessive-clone
            SessionEvent::AssistantMessage { content, .. } => Some(content.text.clone()),
            _ => None,
        })
        .collect();
    let turn_started_count = events
        .iter()
        .filter(|r| matches!(r.event, SessionEvent::TurnStarted { .. }))
        .count();

    assert_eq!(users, vec!["q1", "q2", "q3"]);
    assert_eq!(assistants, vec!["a1", "a2"]);
    assert_eq!(
        turn_started_count, 1,
        "exactly one TurnStarted for the trailing prompt"
    );
}

#[tokio::test]
async fn history_input_does_not_reseed_when_log_nonempty() {
    let service = fresh_service();
    let sid = SessionKey::ephemeral("seed-noreseed");

    // Pre-seed ONE user message so the log is non-empty
    seed_session(service.as_ref(), &sid, FlowInput::Prompt("earlier".into()))
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();

    // Then seed History with the SAME history turn + a new prompt
    let turns = vec![FlowHistoryTurn::User(MessageContent {
        text: "earlier".into(),
        blocks: Vec::new(),
        thinking: None,
        thinking_signature: None,
    })];
    seed_session(
        service.as_ref(),
        &sid,
        FlowInput::History {
            turns,
            prompt: "new".into(),
        },
    )
    .await
    // rust-doctor-disable-next-line unwrap-in-production
    .unwrap();

    // Collect user texts from get_events
    // rust-doctor-disable-next-line unwrap-in-production
    let events = service.get_events(&sid, None, None).await.unwrap();
    let user_texts: Vec<String> = events
        .iter()
        .filter_map(|r| match &r.event {
            // rust-doctor-disable-next-line excessive-clone
            SessionEvent::UserMessage { content, .. } => Some(content.text.clone()),
            _ => None,
        })
        .collect();

    // Assert: "earlier" is NOT re-seeded (would appear twice without the guard),
    // only the new "new" is added
    assert_eq!(user_texts, vec!["earlier".to_string(), "new".to_string()]);
}

#[tokio::test]
async fn seed_session_multimodal_emits_one_user_per_entry() {
    let service = fresh_service();
    let sid = SessionKey::ephemeral("seed-multimodal");
    let msgs = vec![
        MessageContent {
            text: "m1".into(),
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        },
        MessageContent {
            text: "m2".into(),
            blocks: Vec::new(),
            thinking: None,
            thinking_signature: None,
        },
    ];
    super::session_seed::seed_session(service.as_ref(), &sid, FlowInput::Multimodal(msgs))
        .await
        // rust-doctor-disable-next-line unwrap-in-production
        .expect("seed Multimodal");

    // rust-doctor-disable-next-line unwrap-in-production
    let events = service.get_events(&sid, None, None).await.unwrap();
    let users: Vec<String> = events
        .iter()
        .filter_map(|r| match &r.event {
            // rust-doctor-disable-next-line excessive-clone
            SessionEvent::UserMessage { content, .. } => Some(content.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(users, vec!["m1", "m2"]);
}

// BUG-2/BUG-3 regression coverage — `last_user_query` must round-trip the
// most recent user-side text out of every `FlowInput` variant so the
// gateway path can hand it to `MemoryContextProvider::build_memory_user_message`.
// Empty strings degrade cleanly to "" so callers can short-circuit
// retrieval without a panic.

fn msg(text: &str) -> crate::session::events::MessageContent {
    crate::session::events::MessageContent {
        text: text.to_string(),
        blocks: Vec::new(),
        thinking: None,
        thinking_signature: None,
    }
}

#[test]
fn last_user_query_extracts_prompt() {
    let q = super::last_user_query(&FlowInput::Prompt("hello world".into()));
    assert_eq!(q, "hello world");
}

#[test]
fn last_user_query_extracts_history_prompt() {
    let input = FlowInput::History {
        turns: vec![],
        prompt: "next turn please".into(),
    };
    assert_eq!(super::last_user_query(&input), "next turn please");
}

#[test]
fn last_user_query_extracts_last_non_empty_message() {
    let input = FlowInput::Messages(vec![msg("first"), msg("second")]);
    assert_eq!(super::last_user_query(&input), "second");
}

#[test]
fn last_user_query_skips_trailing_empty_messages() {
    let input = FlowInput::Messages(vec![msg("real query"), msg(""), msg("")]);
    assert_eq!(super::last_user_query(&input), "real query");
}

#[test]
fn last_user_query_handles_multimodal() {
    let input = FlowInput::Multimodal(vec![msg("first"), msg("multimodal-tail")]);
    assert_eq!(super::last_user_query(&input), "multimodal-tail");
}

#[test]
fn last_user_query_returns_empty_for_empty_messages() {
    let input = FlowInput::Messages(vec![]);
    assert_eq!(super::last_user_query(&input), "");
}

// Note on build_system_prompt coverage: the prompt assembly path itself
// requires a wired `MemoryContextProvider` (LLM-backed reranker, embedder,
// hybrid assembler, FactSourceFilter pipeline). That is exercised in the
// P0 joint e2e validation step against a live aleph-server, where curated
// markers and retrieval markers can be planted in a known-state fixture
// and the resulting RequestPayload.system_prompt asserted via TraceSink.
// Adding a unit test here would require a heavy fixture stack (provider,
// session_service, tool_service, agent registry) that the surrounding
// file already builds via `fresh_service` for `seed_session` only.

/// Regression: the assembled prompt must not claim "No tools available" nor
/// mandate the legacy `{reasoning, action}` JSON envelope. Tool schemas reach
/// the model through native `tool_use`, so either string would be a lie the
/// model then acts on.
///
/// Both producers are now gone — `ResponseFormatLayer` was unregistered
/// 2026-05-10, `ToolsLayer` deleted 2026-07-26 — so this is a tripwire against
/// re-introducing prompt-injected tool listings without the parser half.
#[test]
fn harness_bridge_prompt_config_skips_tools_and_response_format_layers() {
    use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};

    let prompt = PromptBuilder::new(PromptConfig::default()).build_system_prompt(&[]);

    assert!(
        !prompt.contains("No tools available"),
        "ToolsLayer leaked the empty-tools sentinel into a native-tool-use prompt:\n{prompt}"
    );
    assert!(
        !prompt.contains("## Response Format"),
        "ResponseFormatLayer leaked the JSON-envelope mandate into a native-tool-use prompt:\n{prompt}"
    );
    assert!(
        !prompt.contains("\"reasoning\""),
        "Prompt still references the {{reasoning, action}} envelope schema:\n{prompt}"
    );
}

// (A "companion check" doc comment for the `native_tools_enabled = false`
// branch used to sit here with no test body under it — the body had already
// been deleted. Both the flag and `ToolsLayer` are gone as of 2026-07-26.)

// -- resolve_max_iterations tests (H1: cap the Think→Act loop) ----------
//
// The harness loop must always be capped. Before this wiring the
// orchestrator passed `max_iterations: None`, so a model that kept
// emitting tool calls looped forever. These tests pin the resolution
// rules: per-flow override wins, zero means "unset", and a misconfigured
// default still yields a non-zero cap.

#[test]
fn resolve_max_iterations_uses_default_when_no_override() {
    assert_eq!(super::resolve_max_iterations(None, None, 200), 200);
}

#[test]
fn resolve_max_iterations_flow_override_wins() {
    assert_eq!(super::resolve_max_iterations(None, Some(50), 200), 50);
}

#[test]
fn resolve_max_iterations_treats_zero_override_as_unset() {
    assert_eq!(super::resolve_max_iterations(None, Some(0), 200), 200);
}

#[test]
fn resolve_max_iterations_falls_back_when_default_is_zero() {
    // Misconfigured `[execution] max_iterations = 0` must still cap.
    assert_eq!(
        super::resolve_max_iterations(None, None, 0),
        super::FALLBACK_MAX_ITERATIONS
    );
}

#[test]
fn resolve_max_iterations_never_returns_zero() {
    assert_eq!(
        super::resolve_max_iterations(None, Some(0), 0),
        super::FALLBACK_MAX_ITERATIONS
    );
}

/// D2: runtime override is the highest-priority layer — beats both
/// flow override and default. Zero on runtime layer falls through.
#[test]
fn resolve_max_iterations_runtime_override_wins_over_flow_override() {
    assert_eq!(super::resolve_max_iterations(Some(20), Some(50), 200), 20);
}

#[test]
fn resolve_max_iterations_runtime_override_wins_over_default() {
    assert_eq!(super::resolve_max_iterations(Some(20), None, 200), 20);
}

#[test]
fn resolve_max_iterations_zero_runtime_falls_through_to_flow() {
    assert_eq!(
        super::resolve_max_iterations(Some(0), Some(50), 200),
        50,
        "0 runtime override is 'unset' — flow override applies"
    );
}

// (`legacy_prompt_config_still_emits_tools_layer` was deleted with `ToolsLayer`
// on 2026-07-26. It pinned the "legacy path" — `native_tools_enabled = false` —
// which no production writer ever selected, and whose consumer, the
// `{reasoning, action}` text-envelope parser, had been gone since 2026-05-10.
// The surviving guard is the negative one above: no prompt may claim "No tools
// available" while schemas travel by native tool_use.)

fn goal_for_summary() -> crate::goal::Goal {
    // Passive, no caps → the byte-identical baseline shape.
    crate::goal::Goal::new("sess", "Ship the deadline feature", 0, 1_000)
}

#[test]
fn goal_summary_no_caps_is_bare_objective() {
    let g = goal_for_summary();
    assert_eq!(
        render_goal_summary(&g),
        "Ship the deadline feature (status=active)"
    );
}

#[test]
fn goal_summary_surfaces_budget_and_iteration() {
    let g = goal_for_summary()
        .with_budget(Some(5_000))
        .with_pursuit(crate::goal::PursuitMode::Active { max_iterations: 8 })
        .spent_continuation(2_000);
    let out = render_goal_summary(&g);
    assert!(out.contains(", budget=5000"), "got: {out}");
    assert!(out.contains(", autonomous iteration 1/8"), "got: {out}");
}

/// The goal summary enters the SYSTEM PROMPT, which is the prefix of every
/// message-level prompt-cache breakpoint. It must therefore be a pure function
/// of the goal — no wall clock. It still tells the model a deadline EXISTS; the
/// remaining time is delivered every turn on the transient tail message
/// (`live_deadline_status`), where changing bytes cost only themselves.
///
/// This test used to assert the opposite (`", deadline in ~1h30m"` rendered into
/// the prompt), which is precisely the bug: a countdown in the cached prefix
/// re-keys the entire conversation history on EVERY turn — cache write (1.25x)
/// instead of cache read (0.1x), for as long as the goal is alive.
#[test]
fn goal_summary_is_clock_free_so_the_cache_prefix_survives() {
    let now_ms = 1_000_000;
    let g = goal_for_summary().with_deadline_ms(Some(now_ms + 90 * 60 * 1_000));

    let out = render_goal_summary(&g);
    assert!(out.contains(", deadline set"), "got: {out}");
    assert!(
        !out.contains("~1h30m") && !out.contains("in ~"),
        "no countdown may reach the cached prefix; got: {out}"
    );

    // The load-bearing property, stated directly: two builds an hour apart are
    // byte-identical, so the prefix hash — and the whole conversation cache —
    // survives. Nothing about `render_goal_summary` can reintroduce a clock
    // without failing here.
    assert_eq!(render_goal_summary(&g), out);
}

#[test]
fn goal_summary_says_when_the_pursuit_is_parked() {
    use crate::goal::{Goal, PursuitMode};
    let task_parked = Goal::new("s", "obj", 0, 1_000)
        .with_pursuit(PursuitMode::Active { max_iterations: 5 })
        .with_wait_on_task("task-7".into(), Some("waiting on the build".into()), 1_000);
    let out = render_goal_summary(&task_parked);
    assert!(out.contains("parked"), "got: {out}");
    assert!(out.contains("task-7"), "got: {out}");

    let timer_parked = Goal::new("s", "obj", 0, 1_000)
        .with_pursuit(PursuitMode::Active { max_iterations: 5 })
        .with_wait_until(999_000, None, 1_000);
    let out = render_goal_summary(&timer_parked);
    assert!(out.contains("parked"), "got: {out}");
    // The remaining wait is clock-derived and belongs on the far side of the
    // prompt-cache breakpoint (`live_deadline_status`), never here: a byte
    // that changes every run would re-key the whole conversation prefix.
    assert!(!out.contains("999"), "no clock-derived bytes here: {out}");
}

#[test]
fn an_unparked_goal_summary_is_byte_identical_to_before() {
    use crate::goal::{Goal, PursuitMode};
    let g = Goal::new("s", "Migrate auth", 0, 1_000)
        .with_pursuit(PursuitMode::Active { max_iterations: 5 });
    assert_eq!(
        render_goal_summary(&g),
        "Migrate auth (status=active, autonomous iteration 0/5)"
    );
}

#[test]
fn deadline_render_buckets_and_edges() {
    let now = 1_000_000_u64;
    // sub-minute → seconds
    assert_eq!(render_deadline(now + 30_000, now), "deadline in ~30s");
    // minutes bucket
    assert_eq!(render_deadline(now + 5 * 60_000, now), "deadline in ~5m");
    // hours+minutes bucket
    assert_eq!(
        render_deadline(now + (2 * 3600 + 15 * 60) * 1000, now),
        "deadline in ~2h15m"
    );
    // already past → blocked next hook
    assert_eq!(render_deadline(now - 1, now), "deadline passed");
    // exactly now → passed (>= guard)
    assert_eq!(render_deadline(now, now), "deadline passed");
    // no clock available → existence only, no misleading countdown
    assert_eq!(render_deadline(now + 60_000, 0), "deadline set");
}

#[test]
fn parked_countdown_is_rendered_relative_to_now() {
    // Same convention as `render_deadline`: no clock (0) degrades instead of
    // lying, and an elapsed park reads as due rather than negative.
    assert_eq!(render_park_wait(600_000, 0), "parked, wake time unknown");
    assert_eq!(render_park_wait(600_000, 300_000), "parked, ~5m left");
    assert_eq!(render_park_wait(600_000, 600_001), "parked, wake due");
}
