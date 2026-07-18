# Harness Stability Rescue Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Patch four P0 stability defects in `AgentHarness` (tool-error abort, missing per-turn watchdog, dead `TraceSink`, no per-turn timeout) without introducing new modules or traits.

**Architecture:** Surgical in-place edits across `src/harness/{agent,stall,deps,trait_def,trace_sink}.rs`. Four independent commits, each revertable. New tests grouped in `src/harness/tests/stability.rs`. All new `HarnessDeps` fields are `Option<...>` with `None` default = legacy behavior. Mechanical `field: None` updates required at all 18 construction sites.

**Tech Stack:** Rust 2021, `tokio` (timeout, CancellationToken), `tokio_util::sync::CancellationToken`, `async_trait`, `serde_json`. Test helpers use `rusqlite` in-memory + `InProcessActorSessionService`.

**Spec:** `docs/superpowers/specs/2026-05-04-harness-stability-rescue-design.md`

---

## File Structure

| File | Mod kind | What changes |
|------|----------|--------------|
| `src/harness/agent.rs` | Modify | All 4 steps. emit() helper, fire points, act() rescue, timeout wraps, record_activity dispersion |
| `src/harness/stall.rs` | None | API unchanged; only call sites move (in agent.rs) |
| `src/harness/deps.rs` | Modify | +2 Option fields: `consecutive_failure_cap`, `turn_timeout` |
| `src/harness/trait_def.rs` | Modify | +`TurnPhase` enum, +`HarnessError::StalledTurn` variant |
| `src/harness/trace_sink.rs` | Modify | Doc comment regulation only |
| `src/harness/mod.rs` | Modify | Register `mod stability` under `#[cfg(test)] mod tests` |
| `src/harness/tests/stability.rs` | Create | New file for all 10 stability tests + helpers |

**Out-of-scope sites that need mechanical `field: None` adds (no behavior change):**

- `src/agents/subagent_spawner.rs:188`
- `src/orchestrator/harness_bridge.rs:145`
- `tests/harness_run_e2e.rs:141`

These three sites add 2 trivial fields (`consecutive_failure_cap: None`, `turn_timeout: None`). No logic change, just struct-literal exhaustivity.

**HarnessDeps construction sites (all 18 must update):**

```text
src/agents/subagent_spawner.rs:188
src/orchestrator/harness_bridge.rs:145
src/harness/agent.rs:827, 1016, 1066
src/harness/tests/act.rs:277, 342, 427, 587
src/harness/tests/driver.rs:104, 168
src/harness/tests/task10_wiring.rs:253, 312, 385
src/harness/tests/think.rs:237, 285, 361, 419, 477
tests/harness_run_e2e.rs:141
```

---

## Task 0: Bootstrap — Create stability.rs Test Module

**Files:**
- Modify: `src/harness/mod.rs`
- Create: `src/harness/tests/stability.rs`

This task adds the empty test module so subsequent tasks can append tests without touching mod.rs again.

- [ ] **Step 0.1: Register `stability` test module**

Edit `src/harness/mod.rs`. Replace lines 24-30:

```rust
#[cfg(test)]
mod tests {
    mod act;
    mod driver;
    mod stability;
    mod task10_wiring;
    mod think;
}
```

- [ ] **Step 0.2: Create empty `stability.rs` with shared helpers**

Create `src/harness/tests/stability.rs`:

```rust
//! Stability rescue test suite — covers TraceSink wiring, act() error
//! rescue, per-turn timeout, and StallTracker dispersion.

#![allow(dead_code)] // helpers grow as tasks land

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::error::Result as AlephResult;
use crate::harness::callback::NoopHarnessCallback;
use crate::harness::deps::HarnessDeps;
use crate::harness::trace::LoopTraceEvent;
use crate::harness::trace_sink::TraceSink;
use crate::providers::adapter::{NativeToolCall, ProviderResponse, RequestPayload, StopReason};
use crate::providers::AiProvider;
use crate::routing::session_key::SessionKey;
use crate::session::events::{
    now_ms, MessageContent, SessionEvent, ToolOutput, ToolOutputMetadata, TurnTrigger,
};
use crate::session::in_process::InProcessActorSessionService;
use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};

/// Captures every `LoopTraceEvent` for assertion.
pub(super) struct RecordingTraceSink {
    pub(super) events: Arc<Mutex<Vec<LoopTraceEvent>>>,
}

impl RecordingTraceSink {
    pub(super) fn new() -> (Arc<Self>, Arc<Mutex<Vec<LoopTraceEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::new(Self {
            events: events.clone(),
        });
        (sink, events)
    }
}

impl TraceSink for RecordingTraceSink {
    fn on_trace(&self, event: &LoopTraceEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
    fn flush(&self) {}
}

/// Provider whose `process` future never resolves. Used for timeout tests.
pub(super) struct HangingProvider;

impl AiProvider for HangingProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        Box::pin(std::future::pending())
    }
    fn name(&self) -> &str {
        "hanging"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// Provider that returns one tool_call (`name`) once, then text-only "done".
pub(super) struct OneShotToolProvider {
    pub(super) name: String,
    pub(super) calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl AiProvider for OneShotToolProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        let calls = self.calls.clone();
        let tool = self.name.clone();
        Box::pin(async move {
            let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Ok(ProviderResponse {
                    text: None,
                    tool_calls: vec![NativeToolCall {
                        id: format!("c-{n}"),
                        name: tool,
                        arguments: serde_json::json!({}),
                    }],
                    thinking: None,
                    thinking_signature: None,
                    stop_reason: StopReason::ToolUse,
                    usage: None,
                })
            } else {
                Ok(ProviderResponse::text_only("done".into()))
            }
        })
    }
    fn name(&self) -> &str {
        "oneshot"
    }
    fn color(&self) -> &str {
        "#000000"
    }
}

/// Tool service that always returns `Err(ToolError::Other { ... })`.
pub(super) struct AlwaysFailTools;

#[async_trait::async_trait]
impl crate::tools::service::ToolService for AlwaysFailTools {
    async fn execute(
        &self,
        name: &str,
        _input: serde_json::Value,
    ) -> Result<ToolOutput, crate::tools::service::ToolError> {
        Err(crate::tools::service::ToolError::Other {
            message: format!("forced fail for {name}"),
        })
    }
    async fn list(&self) -> Vec<crate::tools::service::ToolDefinition> {
        Vec::new()
    }
    async fn describe(&self, _name: &str) -> Option<crate::tools::service::ToolDefinition> {
        None
    }
}

/// Tool service that succeeds for tools whose name starts with "ok_" and
/// fails for tools whose name starts with "fail_".
pub(super) struct MixedTools;

#[async_trait::async_trait]
impl crate::tools::service::ToolService for MixedTools {
    async fn execute(
        &self,
        name: &str,
        _input: serde_json::Value,
    ) -> Result<ToolOutput, crate::tools::service::ToolError> {
        if name.starts_with("fail_") {
            Err(crate::tools::service::ToolError::Other {
                message: format!("mixed tool {name} forced fail"),
            })
        } else {
            Ok(ToolOutput {
                value: serde_json::json!({"name": name}),
                metadata: ToolOutputMetadata::default(),
            })
        }
    }
    async fn list(&self) -> Vec<crate::tools::service::ToolDefinition> {
        Vec::new()
    }
    async fn describe(&self, _name: &str) -> Option<crate::tools::service::ToolDefinition> {
        None
    }
}

/// Tool service whose `execute` blocks forever (for act-phase timeout tests).
pub(super) struct HangingTools;

#[async_trait::async_trait]
impl crate::tools::service::ToolService for HangingTools {
    async fn execute(
        &self,
        _name: &str,
        _input: serde_json::Value,
    ) -> Result<ToolOutput, crate::tools::service::ToolError> {
        std::future::pending().await
    }
    async fn list(&self) -> Vec<crate::tools::service::ToolDefinition> {
        Vec::new()
    }
    async fn describe(&self, _name: &str) -> Option<crate::tools::service::ToolDefinition> {
        None
    }
}

/// Build a fresh attached session with one `TurnStarted` + `UserMessage`
/// pair so `harness.run` has work on first call.
pub(super) async fn fresh_session(
    tag: &str,
) -> (
    Arc<dyn crate::session::service::SessionService>,
    crate::session::service::SessionId,
) {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate_add_session_events(&conn).unwrap();
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    let session: Arc<dyn crate::session::service::SessionService> =
        Arc::new(InProcessActorSessionService::new(store));

    let sid = SessionKey::ephemeral(tag);
    session.attach(sid.clone()).await.unwrap();
    let turn = uuid::Uuid::new_v4();
    session
        .emit_event(
            &sid,
            SessionEvent::TurnStarted {
                turn_id: turn,
                trigger: TurnTrigger::UserMessage,
                at: now_ms(),
            },
        )
        .await
        .unwrap();
    session
        .emit_event(
            &sid,
            SessionEvent::UserMessage {
                turn_id: turn,
                content: MessageContent {
                    text: "go".into(),
                    blocks: vec![],
                },
                at: now_ms(),
            },
        )
        .await
        .unwrap();
    (session, sid)
}

/// Minimal `HarnessDeps` builder used by stability tests. All `Option` fields
/// default to `None`. Trace sink is `None` unless the test injects one.
///
/// Tests that need a different LLM/tool/sandbox set construct deps directly.
pub(super) fn minimal_deps(
    session: Arc<dyn crate::session::service::SessionService>,
    tools: Arc<dyn crate::tools::service::ToolService>,
    llm: Arc<dyn AiProvider>,
) -> HarnessDeps {
    HarnessDeps {
        session,
        tools,
        sandbox: Arc::new(crate::sandbox::NoopSandbox),
        llm,
        stop_hooks: None,
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
        trace_sink: None,
        system_prompt: None,
        max_iterations: None,
        power: None,
        stall_config: None,
        // Will be filled in by Tasks 2 and 3:
        // consecutive_failure_cap: None,
        // turn_timeout: None,
    }
}
```

> **Note:** The `minimal_deps` helper omits `consecutive_failure_cap` and `turn_timeout` until Tasks 2 and 3 add those fields. After each task, this helper grows by one field. The plan reminds you to do that update.

- [ ] **Step 0.3: Run a sanity build**

Run: `cargo check -p alephcore`
Expected: PASS (no test code yet, just helpers)

- [ ] **Step 0.4: Commit**

```bash
git add src/harness/mod.rs src/harness/tests/stability.rs
git commit -m "test(harness): scaffold stability test module"
```

---

## Task 1: Step 1 — TraceSink Wiring

**Files:**
- Modify: `src/harness/agent.rs`
- Modify: `src/harness/trace_sink.rs`
- Modify: `src/harness/tests/stability.rs`

### 1A. Update TraceSink trait doc + add emit() helper

- [ ] **Step 1.1: Add MUST-NOT-block regulation to TraceSink trait**

Replace `src/harness/trace_sink.rs:8-11`:

```rust
/// Implementations MUST NOT block. The sink is invoked from `AgentHarness`
/// async tasks; blocking calls back-pressure the entire harness loop.
/// Production sinks should push events to an `mpsc` channel and drain
/// elsewhere. The Gateway path uses `GatewayTraceSink` which is mpsc-backed.
pub trait TraceSink: Send + Sync {
    fn on_trace(&self, event: &LoopTraceEvent);
    fn flush(&self);
}
```

- [ ] **Step 1.2: Add `emit()` helper to AgentHarness impl**

Insert after `agent.rs:81` (after `MAX_STOP_HOOK_VETOS`):

```rust
    /// Lazy-construct a `LoopTraceEvent` and forward to `trace_sink`.
    /// Returns immediately when no sink is wired — the closure is not invoked.
    fn emit<F>(&self, build: F)
    where
        F: FnOnce() -> crate::harness::trace::LoopTraceEvent,
    {
        if let Some(ref sink) = self.deps.trace_sink {
            sink.on_trace(&build());
        }
    }
```

- [ ] **Step 1.3: cargo check**

Run: `cargo check -p alephcore`
Expected: PASS (helper unused but compiles)

### 1B. Write failing test for full lifecycle

- [ ] **Step 1.4: Append test to `stability.rs`**

Append to `src/harness/tests/stability.rs`:

```rust
use crate::harness::agent::AgentHarness;
use crate::harness::trait_def::Harness;
use crate::session::store as _; // avoid unused warning until other tests use it

#[tokio::test]
async fn recording_sink_captures_full_lifecycle() {
    let (sink, events) = RecordingTraceSink::new();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn AiProvider> = Arc::new(OneShotToolProvider {
        name: "ok_tool".into(),
        calls,
    });
    let (session, sid) = fresh_session("trace-lifecycle").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let mut deps = minimal_deps(session, tools, provider);
    deps.trace_sink = Some(sink);
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    harness.run(&sid, &mut cb, &cancel).await.expect("run ok");

    let captured = events.lock().unwrap().clone();
    let names: Vec<&str> = captured
        .iter()
        .map(|e| match e {
            LoopTraceEvent::TurnStarted { .. } => "TurnStarted",
            LoopTraceEvent::TurnStateEntered { .. } => "TurnStateEntered",
            LoopTraceEvent::TextEmitted { .. } => "TextEmitted",
            LoopTraceEvent::ToolCallStarted { .. } => "ToolCallStarted",
            LoopTraceEvent::ToolCallCompleted { .. } => "ToolCallCompleted",
            LoopTraceEvent::ToolSummary { .. } => "ToolSummary",
            LoopTraceEvent::TurnCompleted { .. } => "TurnCompleted",
            LoopTraceEvent::SessionCompleted { .. } => "SessionCompleted",
        })
        .collect();
    // 2 turns: tool turn + final text turn. Then SessionCompleted.
    assert!(names.contains(&"TurnStarted"), "missing TurnStarted: {names:?}");
    assert!(
        names.iter().filter(|n| **n == "TurnStateEntered").count() >= 2,
        "expected at least 2 TurnStateEntered events: {names:?}",
    );
    assert!(
        names.contains(&"ToolCallStarted") && names.contains(&"ToolCallCompleted"),
        "missing tool lifecycle events: {names:?}",
    );
    assert!(
        names.last().copied() == Some("SessionCompleted"),
        "SessionCompleted should be last: {names:?}",
    );
}
```

- [ ] **Step 1.5: Run test (expect FAIL — fire points not yet wired)**

Run: `cargo test -p alephcore --lib harness::tests::stability::recording_sink_captures_full_lifecycle -- --nocapture`
Expected: FAIL (assertion: `events` is empty / missing TurnStarted)

### 1C. Wire fire points in agent.rs

- [ ] **Step 1.6: Fire `TurnStarted` at run_turn_internal entry**

Insert after `agent.rs:106` (after sleep_guard block) — at the very top of `run_turn_internal`:

```rust
        self.emit(|| crate::harness::trace::LoopTraceEvent::TurnStarted {
            iteration: iterations,
        });
```

- [ ] **Step 1.7: Fire `TurnStateEntered { Think }` before LLM call**

Insert before `agent.rs:186` (`let response = self.deps.llm.process(payload).await?;`):

```rust
        self.emit(|| crate::harness::trace::LoopTraceEvent::TurnStateEntered {
            iteration: iterations,
            state: crate::harness::trace::LoopTraceState::Think,
        });
```

- [ ] **Step 1.8: Fire `TextEmitted` after non-empty text**

Replace `agent.rs:191-195` (the `if !text.is_empty()` block):

```rust
        if !text.is_empty() {
            // Non-streaming LLM layer emits one chunk per turn; the callback
            // shape permits finer chunking once `process_stream` is wired.
            callback.on_delta(&text);
            self.emit(|| crate::harness::trace::LoopTraceEvent::TextEmitted {
                iteration: iterations,
                stream: crate::harness::trace::LoopTraceTextKind::Final,
                text: text.clone(),
            });
        }
```

- [ ] **Step 1.9: Fire `TurnStateEntered { Act }` + `ToolCallStarted/Completed` in act()**

Modify `agent.rs:243-247` (the else-branch of `if response.tool_calls.is_empty()`):

```rust
        } else {
            self.emit(|| crate::harness::trace::LoopTraceEvent::TurnStateEntered {
                iteration: iterations,
                state: crate::harness::trace::LoopTraceState::Act,
            });
            let executed = self
                .act(session_id, turn_id, response.tool_calls, callback, iterations)
                .await?;
            Ok((TurnState::Continue, executed, false))
        }
```

> Note: `act()` signature gains an `iterations: usize` param.

- [ ] **Step 1.10: Update `act()` signature + emit ToolCall events**

Replace `agent.rs:257-336` (entire `act` method) with:

```rust
    async fn act(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        tool_calls: Vec<NativeToolCall>,
        callback: &mut dyn HarnessCallback,
        iteration: usize,
    ) -> Result<usize, HarnessError> {
        let mut first_error: Option<ToolError> = None;
        let mut executed_count: usize = 0;

        for call in tool_calls {
            callback.on_tool_call(&call.name);
            let started = std::time::Instant::now();
            self.emit(|| crate::harness::trace::LoopTraceEvent::ToolCallStarted {
                iteration,
                call: crate::harness::trace::ToolCallStartEvent {
                    tool_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    input: call.arguments.clone(),
                },
            });
            let requested = SessionEvent::ToolCallRequested {
                turn_id,
                call_id: call.id.clone(),
                name: call.name.clone(),
                input: call.arguments.clone(),
                at: now_ms(),
            };
            self.deps.session.emit_event(session_id, requested).await?;

            if let Some(ref prior_err) = first_error {
                let skip_event = SessionEvent::ToolError {
                    turn_id,
                    call_id: call.id.clone(),
                    error: format!("Skipped: {}", prior_err),
                    at: now_ms(),
                };
                if let Err(emit_err) = self.deps.session.emit_event(session_id, skip_event).await {
                    tracing::warn!(
                        ?session_id,
                        call_id = %call.id,
                        ?emit_err,
                        "failed to persist skipped-tool ToolError event",
                    );
                }
                self.emit(|| crate::harness::trace::LoopTraceEvent::ToolCallCompleted {
                    iteration,
                    call: crate::harness::trace::ToolCallEndEvent {
                        tool_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        input: call.arguments.clone(),
                        duration_ms: started.elapsed().as_millis() as u64,
                    },
                    result: crate::tools::runtime::ToolResult::Error {
                        error: format!("Skipped: {}", prior_err),
                        retryable: false,
                    },
                });
                continue;
            }

            match self.deps.tools.execute(&call.name, call.arguments.clone()).await {
                Ok(output) => {
                    executed_count = executed_count.saturating_add(1);
                    let output_value = output.value.clone();
                    let result_event = SessionEvent::ToolResult {
                        turn_id,
                        call_id: call.id.clone(),
                        output,
                        at: now_ms(),
                    };
                    self.deps
                        .session
                        .emit_event(session_id, result_event)
                        .await?;
                    self.emit(|| crate::harness::trace::LoopTraceEvent::ToolCallCompleted {
                        iteration,
                        call: crate::harness::trace::ToolCallEndEvent {
                            tool_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            input: call.arguments.clone(),
                            duration_ms: started.elapsed().as_millis() as u64,
                        },
                        result: crate::tools::runtime::ToolResult::Success {
                            output: output_value,
                        },
                    });
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    let error_event = SessionEvent::ToolError {
                        turn_id,
                        call_id: call.id.clone(),
                        error: error_msg.clone(),
                        at: now_ms(),
                    };
                    if let Err(emit_err) =
                        self.deps.session.emit_event(session_id, error_event).await
                    {
                        tracing::warn!(
                            ?session_id,
                            call_id = %call.id,
                            ?emit_err,
                            "failed to persist ToolError event",
                        );
                    }
                    self.emit(|| crate::harness::trace::LoopTraceEvent::ToolCallCompleted {
                        iteration,
                        call: crate::harness::trace::ToolCallEndEvent {
                            tool_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            input: call.arguments.clone(),
                            duration_ms: started.elapsed().as_millis() as u64,
                        },
                        result: crate::tools::runtime::ToolResult::Error {
                            error: error_msg,
                            retryable: false,
                        },
                    });
                    first_error = Some(e);
                }
            }
        }

        if let Some(e) = first_error {
            return Err(HarnessError::Tool(e));
        }
        Ok(executed_count)
    }
```

> Note: The current first-error abort behavior is **preserved here** — it gets removed in Task 2. This task is purely additive (trace events).

- [ ] **Step 1.11: Fire `TurnCompleted` at all turn return paths**

Replace the entire return-path tail of `run_turn_internal` (`agent.rs:212-247`) with the unified version below. Replace lines starting at `if response.tool_calls.is_empty() {`:

```rust
        let outcome_for_trace;
        let metrics_for_trace;
        let result;

        if response.tool_calls.is_empty() {
            let block = self
                .evaluate_stop_hooks(iterations, tool_calls_made, Some(text.clone()))
                .await;
            if let Some(reason) = block {
                tracing::info!(
                    ?session_id,
                    reason = %reason,
                    "stop hook vetoed; forcing continue",
                );
                let new_turn = uuid::Uuid::new_v4();
                let block_event = SessionEvent::UserMessage {
                    turn_id: new_turn,
                    content: MessageContent {
                        text: format!("[stop-hook veto] {reason}"),
                        blocks: Vec::new(),
                    },
                    at: now_ms(),
                };
                self.deps
                    .session
                    .emit_event(session_id, block_event)
                    .await?;
                outcome_for_trace = crate::harness::trace::LoopTraceTurnOutcome::Continue;
                metrics_for_trace = crate::harness::trace::LoopTraceTurnMetrics {
                    requested_tool_calls: 0,
                    executed_tool_calls: 0,
                    productive: false,
                    consecutive_errors: 0,
                    total_tokens: 0,
                };
                result = Ok((TurnState::Continue, 0, true));
            } else {
                outcome_for_trace = crate::harness::trace::LoopTraceTurnOutcome::Stop;
                metrics_for_trace = crate::harness::trace::LoopTraceTurnMetrics {
                    requested_tool_calls: 0,
                    executed_tool_calls: 0,
                    productive: false,
                    consecutive_errors: 0,
                    total_tokens: 0,
                };
                result = Ok((TurnState::Done, 0, false));
            }
        } else {
            self.emit(|| crate::harness::trace::LoopTraceEvent::TurnStateEntered {
                iteration: iterations,
                state: crate::harness::trace::LoopTraceState::Act,
            });
            let requested = response.tool_calls.len();
            let executed = self
                .act(session_id, turn_id, response.tool_calls, callback, iterations)
                .await?;
            outcome_for_trace = crate::harness::trace::LoopTraceTurnOutcome::Continue;
            metrics_for_trace = crate::harness::trace::LoopTraceTurnMetrics {
                requested_tool_calls: requested,
                executed_tool_calls: executed,
                productive: executed > 0,
                consecutive_errors: 0,
                total_tokens: 0,
            };
            result = Ok((TurnState::Continue, executed, false));
        }

        self.emit(|| crate::harness::trace::LoopTraceEvent::TurnCompleted {
            iteration: iterations,
            outcome: outcome_for_trace,
            metrics: metrics_for_trace,
        });
        result
```

> Note: This block replaces both the if/else outcome computation and adds TurnCompleted as the trailing emit. It removes the duplicate `TurnStateEntered { Act }` at Step 1.9 — that was a draft. The single emit lives inside the else branch here.

- [ ] **Step 1.12: Revert the duplicate emit added in Step 1.9**

Re-edit `agent.rs` to ensure the `else { self.emit(... Act); ... }` block from Step 1.9 is **removed in favor of** the version in Step 1.11. Open agent.rs, search for `TurnStateEntered` followed by `state: ... Act`. There must be exactly **one** such line (inside the else-branch from Step 1.11). If two exist, delete the earlier one.

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 1.13: Fire `SessionCompleted` at run() exit paths**

Replace `agent.rs:429-487` (entire `Harness::run` impl). Find it via `impl Harness for AgentHarness {`:

```rust
    async fn run(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
        cancel: &CancellationToken,
    ) -> Result<(), HarnessError> {
        let cap = self.deps.max_iterations;
        let mut iterations: usize = 0;
        let mut tool_calls_made: usize = 0;
        let mut stop_hook_veto_count: usize = 0;
        let result: Result<crate::harness::trace::LoopTraceSessionOutcome, HarnessError> = loop {
            if cancel.is_cancelled() {
                break Err(HarnessError::Cancelled);
            }
            if let Some(ref tracker) = self.stall_tracker {
                if tracker.is_stalled() {
                    let elapsed = tracker.elapsed().await;
                    break Err(HarnessError::Stalled { elapsed });
                }
            }
            match self
                .run_turn_internal(session_id, callback, iterations, tool_calls_made)
                .await
            {
                Err(e) => break Err(e),
                Ok((TurnState::Continue, executed, is_veto)) => {
                    if let Some(ref tracker) = self.stall_tracker {
                        tracker.record_activity().await;
                    }
                    iterations = iterations.saturating_add(1);
                    tool_calls_made = tool_calls_made.saturating_add(executed);
                    if is_veto {
                        stop_hook_veto_count = stop_hook_veto_count.saturating_add(1);
                        if stop_hook_veto_count >= Self::MAX_STOP_HOOK_VETOS {
                            tracing::warn!(
                                ?session_id,
                                max_vetos = Self::MAX_STOP_HOOK_VETOS,
                                "stop-hook veto limit reached; forcing Done to prevent infinite loop",
                            );
                            callback.on_complete();
                            break Ok(crate::harness::trace::LoopTraceSessionOutcome::HitLimit);
                        }
                    } else {
                        stop_hook_veto_count = 0;
                    }
                    if let Some(limit) = cap {
                        if iterations >= limit {
                            self.hit_limit.store(true, Ordering::Relaxed);
                            callback.on_complete();
                            break Ok(crate::harness::trace::LoopTraceSessionOutcome::HitLimit);
                        }
                    }
                }
                Ok((TurnState::Done, _, _)) => {
                    callback.on_complete();
                    break Ok(crate::harness::trace::LoopTraceSessionOutcome::Completed);
                }
            }
        };

        match result {
            Ok(outcome) => {
                self.emit(|| crate::harness::trace::LoopTraceEvent::SessionCompleted {
                    outcome,
                    iterations,
                    tool_calls_made,
                    total_tokens: 0,
                    hit_limit: matches!(
                        outcome,
                        crate::harness::trace::LoopTraceSessionOutcome::HitLimit,
                    ),
                    final_text: None,
                });
                Ok(())
            }
            Err(e) => {
                let session_outcome = match &e {
                    HarnessError::Cancelled | HarnessError::Stalled { .. } => {
                        crate::harness::trace::LoopTraceSessionOutcome::Cancelled
                    }
                    _ => crate::harness::trace::LoopTraceSessionOutcome::Cancelled,
                };
                self.emit(|| crate::harness::trace::LoopTraceEvent::SessionCompleted {
                    outcome: session_outcome,
                    iterations,
                    tool_calls_made,
                    total_tokens: 0,
                    hit_limit: false,
                    final_text: None,
                });
                Err(e)
            }
        }
    }
```

- [ ] **Step 1.14: Run lifecycle test**

Run: `cargo test -p alephcore --lib harness::tests::stability::recording_sink_captures_full_lifecycle -- --nocapture`
Expected: PASS

- [ ] **Step 1.15: Append zero-overhead test**

Append to `stability.rs`:

```rust
/// Sink builder that panics when invoked. Confirms `emit()` skips construction
/// when `trace_sink` is `None`.
struct PanickingTraceSink;
impl TraceSink for PanickingTraceSink {
    fn on_trace(&self, _event: &LoopTraceEvent) {
        panic!("trace sink should not be invoked");
    }
    fn flush(&self) {
        panic!("trace sink flush should not be invoked");
    }
}

#[tokio::test]
async fn noop_sink_zero_overhead() {
    // No sink wired — the harness must complete without ever building events.
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn AiProvider> = Arc::new(OneShotToolProvider {
        name: "ok_tool".into(),
        calls,
    });
    let (session, sid) = fresh_session("trace-zero").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    // trace_sink stays None — the helper sets it that way.
    let deps = minimal_deps(session, tools, provider);
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    harness.run(&sid, &mut cb, &cancel).await.expect("ok");
}
```

- [ ] **Step 1.16: Run zero-overhead test**

Run: `cargo test -p alephcore --lib harness::tests::stability::noop_sink_zero_overhead -- --nocapture`
Expected: PASS

- [ ] **Step 1.17: Full harness test sweep**

Run: `cargo test -p alephcore --lib harness:: -- --nocapture`
Expected: ALL PASS (existing think/act/driver/task10 tests still green; new stability tests green)

- [ ] **Step 1.18: Commit Step 1**

```bash
git add src/harness/agent.rs src/harness/trace_sink.rs src/harness/tests/stability.rs
git commit -m "feat(harness): wire TraceSink fire points across full turn lifecycle"
```

---

## Task 2: Step 2 — act() Error Rescue + Consecutive Failure Cap

**Files:**
- Modify: `src/harness/deps.rs`
- Modify: `src/harness/agent.rs`
- Modify: `src/harness/tests/stability.rs`
- Modify: 18 HarnessDeps construction sites (mechanical `field: None`)

### 2A. Add the deps field

- [ ] **Step 2.1: Append `consecutive_failure_cap` to HarnessDeps**

Append before the closing brace of `HarnessDeps` in `src/harness/deps.rs:67`:

```rust
    /// Hard cap on consecutive turns where every tool call failed. When
    /// reached, the harness forces `TurnState::Done` with `hit_limit=true`
    /// to prevent the model from looping on permanently-failing tools.
    /// `None` disables the cap (legacy behavior). Recommended `Some(8)`.
    pub consecutive_failure_cap: Option<usize>,
```

- [ ] **Step 2.2: Update minimal_deps helper**

Edit `src/harness/tests/stability.rs` `minimal_deps` to add the new field:

```rust
        // ... existing fields ...
        stall_config: None,
        consecutive_failure_cap: None,
```

Remove the `// Will be filled in by Tasks 2 and 3` comment line referencing `consecutive_failure_cap`.

- [ ] **Step 2.3: Mechanical update — all 18 HarnessDeps construction sites**

Append `consecutive_failure_cap: None,` after `stall_config: ...,` (or wherever the last existing field is) in each of:

```text
src/agents/subagent_spawner.rs:188
src/orchestrator/harness_bridge.rs:145
src/harness/agent.rs:827
src/harness/agent.rs:1016
src/harness/agent.rs:1066
src/harness/tests/act.rs:277
src/harness/tests/act.rs:342
src/harness/tests/act.rs:427
src/harness/tests/act.rs:587
src/harness/tests/driver.rs:104
src/harness/tests/driver.rs:168
src/harness/tests/task10_wiring.rs:253
src/harness/tests/task10_wiring.rs:312
src/harness/tests/task10_wiring.rs:385
src/harness/tests/think.rs:237
src/harness/tests/think.rs:285
src/harness/tests/think.rs:361
src/harness/tests/think.rs:419
src/harness/tests/think.rs:477
tests/harness_run_e2e.rs:141
```

> Use grep to verify: `grep -rn "stall_config: None," src/ tests/ --include="*.rs"` — every match should be followed by `consecutive_failure_cap: None,` after this step.

- [ ] **Step 2.4: cargo check**

Run: `cargo check -p alephcore --tests`
Expected: PASS (no E0063 missing-field errors)

### 2B. TDD: tool_failure_recovers_in_next_think

- [ ] **Step 2.5: Append failing test**

Append to `stability.rs`:

```rust
/// After Task 2 lands, a tool failure becomes a tool_result(is_error=true) in
/// the session log and the model gets a chance to recover on the next Think.
/// Currently (pre-Task 2), the harness aborts via `HarnessError::Tool`.
#[tokio::test]
async fn tool_failure_recovers_in_next_think() {
    // Provider plays: turn 0 → emit fail_tool, turn 1 → final text "ok"
    struct RecoveryProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl AiProvider for RecoveryProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let calls = self.calls.clone();
            Box::pin(async move {
                let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n == 0 {
                    Ok(ProviderResponse {
                        text: None,
                        tool_calls: vec![NativeToolCall {
                            id: "c-0".into(),
                            name: "fail_one".into(),
                            arguments: serde_json::json!({}),
                        }],
                        thinking: None,
                        thinking_signature: None,
                        stop_reason: StopReason::ToolUse,
                        usage: None,
                    })
                } else {
                    Ok(ProviderResponse::text_only("recovered".into()))
                }
            })
        }
        fn name(&self) -> &str { "recovery" }
        fn color(&self) -> &str { "#000000" }
    }

    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn AiProvider> = Arc::new(RecoveryProvider { calls: calls.clone() });
    let (session, sid) = fresh_session("recover-tool-fail").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let deps = minimal_deps(session.clone(), tools, provider);
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    let outcome = harness.run(&sid, &mut cb, &cancel).await;

    assert!(outcome.is_ok(), "harness must not abort on tool error: {outcome:?}");
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "model should be called twice (tool turn + recovery turn)",
    );
    let events = session.get_events(&sid, None, None).await.unwrap();
    let has_tool_error = events.iter().any(|r| matches!(
        r.event,
        SessionEvent::ToolError { .. }
    ));
    assert!(has_tool_error, "session log must contain ToolError event");
}
```

- [ ] **Step 2.6: Run test (expect FAIL with HarnessError::Tool)**

Run: `cargo test -p alephcore --lib harness::tests::stability::tool_failure_recovers_in_next_think -- --nocapture`
Expected: FAIL — `harness must not abort on tool error: Err(...)`

### 2C. Implement act() rescue

- [ ] **Step 2.7: Strip first_error short-circuit from act()**

Replace the entire `act()` body (the version installed in Step 1.10) with this rescued version:

```rust
    async fn act(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        tool_calls: Vec<NativeToolCall>,
        callback: &mut dyn HarnessCallback,
        iteration: usize,
    ) -> Result<usize, HarnessError> {
        let mut executed_count: usize = 0;

        for call in tool_calls {
            callback.on_tool_call(&call.name);
            let started = std::time::Instant::now();
            self.emit(|| crate::harness::trace::LoopTraceEvent::ToolCallStarted {
                iteration,
                call: crate::harness::trace::ToolCallStartEvent {
                    tool_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    input: call.arguments.clone(),
                },
            });
            let requested = SessionEvent::ToolCallRequested {
                turn_id,
                call_id: call.id.clone(),
                name: call.name.clone(),
                input: call.arguments.clone(),
                at: now_ms(),
            };
            self.deps.session.emit_event(session_id, requested).await?;

            match self.deps.tools.execute(&call.name, call.arguments.clone()).await {
                Ok(output) => {
                    executed_count = executed_count.saturating_add(1);
                    let output_value = output.value.clone();
                    let result_event = SessionEvent::ToolResult {
                        turn_id,
                        call_id: call.id.clone(),
                        output,
                        at: now_ms(),
                    };
                    self.deps
                        .session
                        .emit_event(session_id, result_event)
                        .await?;
                    self.emit(|| crate::harness::trace::LoopTraceEvent::ToolCallCompleted {
                        iteration,
                        call: crate::harness::trace::ToolCallEndEvent {
                            tool_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            input: call.arguments.clone(),
                            duration_ms: started.elapsed().as_millis() as u64,
                        },
                        result: crate::tools::runtime::ToolResult::Success {
                            output: output_value,
                        },
                    });
                }
                Err(e) => {
                    // Tool failure is now model-recoverable: emit ToolError to
                    // session log and trace, but DO NOT propagate. The next
                    // Think will see `tool_result.is_error=true` via build_prompt.
                    let error_msg = e.to_string();
                    let error_event = SessionEvent::ToolError {
                        turn_id,
                        call_id: call.id.clone(),
                        error: error_msg.clone(),
                        at: now_ms(),
                    };
                    if let Err(emit_err) =
                        self.deps.session.emit_event(session_id, error_event).await
                    {
                        tracing::warn!(
                            ?session_id,
                            call_id = %call.id,
                            ?emit_err,
                            "failed to persist ToolError event",
                        );
                    }
                    self.emit(|| crate::harness::trace::LoopTraceEvent::ToolCallCompleted {
                        iteration,
                        call: crate::harness::trace::ToolCallEndEvent {
                            tool_id: call.id.clone(),
                            tool_name: call.name.clone(),
                            input: call.arguments.clone(),
                            duration_ms: started.elapsed().as_millis() as u64,
                        },
                        result: crate::tools::runtime::ToolResult::Error {
                            error: error_msg,
                            retryable: false,
                        },
                    });
                    // Continue the batch — DO NOT set a sticky error.
                }
            }
        }

        Ok(executed_count)
    }
```

> Removed: `first_error` variable, the `Skipped:` short-circuit branch, and the trailing `if let Some(e) = first_error { return Err(...) }`.

- [ ] **Step 2.8: Re-run recovery test**

Run: `cargo test -p alephcore --lib harness::tests::stability::tool_failure_recovers_in_next_think -- --nocapture`
Expected: PASS

### 2D. partial_batch_failure_continues

- [ ] **Step 2.9: Append test**

Append to `stability.rs`:

```rust
#[tokio::test]
async fn partial_batch_failure_continues() {
    // Provider emits 3 tool_calls in one turn: ok_a, fail_b, ok_c. Then text.
    struct BatchProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl AiProvider for BatchProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let calls = self.calls.clone();
            Box::pin(async move {
                let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n == 0 {
                    Ok(ProviderResponse {
                        text: None,
                        tool_calls: vec![
                            NativeToolCall { id: "a".into(), name: "ok_a".into(), arguments: serde_json::json!({}) },
                            NativeToolCall { id: "b".into(), name: "fail_b".into(), arguments: serde_json::json!({}) },
                            NativeToolCall { id: "c".into(), name: "ok_c".into(), arguments: serde_json::json!({}) },
                        ],
                        thinking: None,
                        thinking_signature: None,
                        stop_reason: StopReason::ToolUse,
                        usage: None,
                    })
                } else {
                    Ok(ProviderResponse::text_only("done".into()))
                }
            })
        }
        fn name(&self) -> &str { "batch" }
        fn color(&self) -> &str { "#000000" }
    }

    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn AiProvider> = Arc::new(BatchProvider { calls });
    let (session, sid) = fresh_session("partial-batch").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let deps = minimal_deps(session.clone(), tools, provider);
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    harness.run(&sid, &mut cb, &cancel).await.expect("ok");

    let events = session.get_events(&sid, None, None).await.unwrap();
    let n_results = events.iter().filter(|r| matches!(r.event, SessionEvent::ToolResult { .. })).count();
    let n_errors = events.iter().filter(|r| matches!(r.event, SessionEvent::ToolError { .. })).count();
    assert_eq!(n_results, 2, "expected 2 ToolResult (ok_a, ok_c): events={events:#?}");
    assert_eq!(n_errors, 1, "expected 1 ToolError (fail_b): events={events:#?}");
}
```

- [ ] **Step 2.10: Run test**

Run: `cargo test -p alephcore --lib harness::tests::stability::partial_batch_failure_continues -- --nocapture`
Expected: PASS

### 2E. Consecutive failure cap

- [ ] **Step 2.11: Add counter to run() loop**

In `agent.rs` `Harness::run` impl (the version from Step 1.13), insert the counter setup immediately after `let mut stop_hook_veto_count: usize = 0;`:

```rust
        let mut consecutive_failure_turns: usize = 0;
```

In the `Ok((TurnState::Continue, executed, is_veto))` arm, **before** the `is_veto` check, add:

```rust
                    // Consecutive-failure tracking: a turn is "all-failed" when
                    // it executed nothing yet emitted at least one ToolError.
                    // We approximate via `executed == 0 && there were tool calls`,
                    // which is equivalent because Step 2 act() never short-circuits.
                    if executed == 0 && !is_veto {
                        // Distinguish "no tools requested" (just text) from
                        // "all tools failed" by re-checking the session log
                        // tail for ToolError in the most recent turn segment.
                        let events = self.deps.session.get_events(session_id, None, None).await
                            .map_err(HarnessError::Session)?;
                        let last_assistant_idx = events.iter().rposition(|r| matches!(
                            r.event,
                            SessionEvent::AssistantMessage { .. }
                        )).unwrap_or(0);
                        let had_failure = events[last_assistant_idx..].iter().any(|r| matches!(
                            r.event,
                            SessionEvent::ToolError { .. }
                        ));
                        if had_failure {
                            consecutive_failure_turns = consecutive_failure_turns.saturating_add(1);
                            if let Some(cap) = self.deps.consecutive_failure_cap {
                                if consecutive_failure_turns >= cap {
                                    tracing::warn!(
                                        ?session_id,
                                        cap,
                                        "consecutive total-failure cap reached; forcing Done",
                                    );
                                    self.hit_limit.store(true, Ordering::Relaxed);
                                    callback.on_complete();
                                    break Ok(crate::harness::trace::LoopTraceSessionOutcome::HitLimit);
                                }
                            }
                        } else {
                            consecutive_failure_turns = 0;
                        }
                    } else if executed > 0 {
                        consecutive_failure_turns = 0;
                    }
```

- [ ] **Step 2.12: cargo check**

Run: `cargo check -p alephcore --tests`
Expected: PASS

- [ ] **Step 2.13: Append cap test**

Append to `stability.rs`:

```rust
#[tokio::test]
async fn consecutive_total_failure_caps_loop() {
    // Provider keeps emitting one fail_x tool forever. With cap=3, the
    // harness must terminate after 3 fully-failed turns and set hit_limit.
    struct AlwaysFailProvider;
    impl AiProvider for AlwaysFailProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            Box::pin(async move {
                Ok(ProviderResponse {
                    text: None,
                    tool_calls: vec![NativeToolCall {
                        id: format!("c-{}", uuid::Uuid::new_v4()),
                        name: "fail_x".into(),
                        arguments: serde_json::json!({}),
                    }],
                    thinking: None,
                    thinking_signature: None,
                    stop_reason: StopReason::ToolUse,
                    usage: None,
                })
            })
        }
        fn name(&self) -> &str { "always-fail" }
        fn color(&self) -> &str { "#000000" }
    }

    let provider: Arc<dyn AiProvider> = Arc::new(AlwaysFailProvider);
    let (session, sid) = fresh_session("cap-loop").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let mut deps = minimal_deps(session, tools, provider);
    deps.consecutive_failure_cap = Some(3);
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.run(&sid, &mut cb, &cancel),
    ).await;
    outcome.expect("must terminate within 2s").expect("Ok exit");
    assert!(harness.hit_limit(), "hit_limit should be true after cap");
}
```

- [ ] **Step 2.14: Run test**

Run: `cargo test -p alephcore --lib harness::tests::stability::consecutive_total_failure_caps_loop -- --nocapture`
Expected: PASS

- [ ] **Step 2.15: Full harness sweep**

Run: `cargo test -p alephcore --lib harness:: -- --nocapture`
Expected: ALL PASS. **If existing think/act/driver tests now fail because they expected `HarnessError::Tool` to surface, those tests are testing the old behavior and need updating.** Treat such failures as expected and update them to assert on session-log ToolError instead.

> Common failure pattern to expect and fix: an existing test asserts `harness.run(...).await.unwrap_err()` then matches on `HarnessError::Tool`. After Task 2, the same scenario returns `Ok(())` with the error in the session log. Update such tests by replacing the unwrap_err with `.expect("ok")` and asserting on `events.iter().any(|r| matches!(r.event, SessionEvent::ToolError { .. }))`.

- [ ] **Step 2.16: Commit Step 2**

```bash
git add src/harness/agent.rs src/harness/deps.rs src/harness/tests/ \
        src/agents/subagent_spawner.rs src/orchestrator/harness_bridge.rs \
        tests/harness_run_e2e.rs
git commit -m "feat(harness): rescue tool errors back to model + cap consecutive failures"
```

---

## Task 3: Step 3 — Per-turn Timeout + TurnPhase

**Files:**
- Modify: `src/harness/trait_def.rs`
- Modify: `src/harness/deps.rs`
- Modify: `src/harness/agent.rs`
- Modify: `src/harness/tests/stability.rs`
- Modify: 18 HarnessDeps construction sites (mechanical `turn_timeout: None`)

### 3A. New types

- [ ] **Step 3.1: Add TurnPhase enum to trait_def.rs**

Append to `src/harness/trait_def.rs` (before the `#[derive(Debug, thiserror::Error)] pub enum HarnessError`):

```rust
/// Identifies which sub-phase of a turn was hung when a per-turn timeout fired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnPhase {
    /// LLM `process()` call was hung.
    Think,
    /// A specific tool's `execute()` call was hung.
    Act { tool_name: String },
}

impl std::fmt::Display for TurnPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TurnPhase::Think => write!(f, "Think"),
            TurnPhase::Act { tool_name } => write!(f, "Act({tool_name})"),
        }
    }
}
```

- [ ] **Step 3.2: Add StalledTurn variant to HarnessError**

In `trait_def.rs`, append a variant inside `HarnessError`:

```rust
    /// A single Think or Act phase exceeded `turn_timeout`. Distinct from
    /// `Stalled` (which captures cross-turn idle): `StalledTurn` fires when
    /// an `await` itself does not return.
    #[error("turn stalled in {phase} after {elapsed:?}")]
    StalledTurn {
        phase: TurnPhase,
        elapsed: std::time::Duration,
    },
```

- [ ] **Step 3.3: Re-export TurnPhase from mod.rs**

In `src/harness/mod.rs:22`, add `TurnPhase` to the `pub use trait_def::{...}` line:

```rust
pub use trait_def::{Harness, HarnessError, TurnPhase, TurnState};
```

- [ ] **Step 3.4: Map StalledTurn in SessionDriver impl**

In `agent.rs` `impl SessionDriver`, find the `match e { ... }` block (currently lines 406-418) and add the new arm before the closing brace:

```rust
                HarnessError::StalledTurn { phase, elapsed } => {
                    crate::error::AlephError::provider(format!(
                        "agent turn stalled in {phase} after {elapsed:?}"
                    ))
                }
```

- [ ] **Step 3.5: Add `turn_timeout` to HarnessDeps**

Append to `src/harness/deps.rs`:

```rust
    /// Hard wall-clock budget for a single Think or Act phase. When set, the
    /// harness wraps each LLM call and each tool exec in `tokio::time::timeout`.
    /// Exceeding the budget yields `HarnessError::StalledTurn` with the
    /// hung phase. `None` disables (legacy behavior). Recommended `Some(300s)`.
    pub turn_timeout: Option<std::time::Duration>,
```

- [ ] **Step 3.6: Update minimal_deps helper + 18 sites**

Edit `src/harness/tests/stability.rs` `minimal_deps` to include `turn_timeout: None,`.

Mechanical: append `turn_timeout: None,` to all 18 sites listed in Task 2.3.

Verify: `grep -rn "consecutive_failure_cap: None," src/ tests/ --include="*.rs"` — every match should be followed by `turn_timeout: None,` in the next field row.

- [ ] **Step 3.7: cargo check**

Run: `cargo check -p alephcore --tests`
Expected: PASS

### 3B. TDD: think timeout

- [ ] **Step 3.8: Append failing test**

Append to `stability.rs`:

```rust
use crate::harness::trait_def::{HarnessError, TurnPhase};

#[tokio::test]
async fn think_timeout_fires_with_phase_think() {
    let provider: Arc<dyn AiProvider> = Arc::new(HangingProvider);
    let (session, sid) = fresh_session("think-timeout").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let mut deps = minimal_deps(session, tools, provider);
    deps.turn_timeout = Some(std::time::Duration::from_millis(200));
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.run(&sid, &mut cb, &cancel),
    ).await.expect("must return within 2s");

    match result {
        Err(HarnessError::StalledTurn { phase, elapsed }) => {
            assert_eq!(phase, TurnPhase::Think, "phase must be Think");
            assert!(elapsed >= std::time::Duration::from_millis(150), "elapsed {elapsed:?}");
        }
        other => panic!("expected StalledTurn(Think), got {other:?}"),
    }
    assert!(
        started.elapsed() < std::time::Duration::from_millis(800),
        "harness must abort within ~3× timeout, took {:?}",
        started.elapsed(),
    );
}
```

- [ ] **Step 3.9: Run test (expect FAIL — timeout not yet wired)**

Run: `cargo test -p alephcore --lib harness::tests::stability::think_timeout_fires_with_phase_think -- --nocapture`
Expected: FAIL — test times out at the outer 2s guard.

### 3C. Wire Think timeout

- [ ] **Step 3.10: Wrap LLM call in tokio::time::timeout**

In `agent.rs`, replace the line `let response = self.deps.llm.process(payload).await?;` (currently `agent.rs:186`) with:

```rust
        let response = match self.deps.turn_timeout {
            Some(budget) => {
                let started = std::time::Instant::now();
                match tokio::time::timeout(budget, self.deps.llm.process(payload)).await {
                    Ok(Ok(r)) => r,
                    Ok(Err(e)) => return Err(HarnessError::Llm(e)),
                    Err(_elapsed) => {
                        return Err(HarnessError::StalledTurn {
                            phase: TurnPhase::Think,
                            elapsed: started.elapsed(),
                        });
                    }
                }
            }
            None => self.deps.llm.process(payload).await?,
        };
```

> Add `use crate::harness::trait_def::TurnPhase;` to the top of agent.rs if not already present.

- [ ] **Step 3.11: Run test**

Run: `cargo test -p alephcore --lib harness::tests::stability::think_timeout_fires_with_phase_think -- --nocapture`
Expected: PASS

### 3D. TDD: act timeout

- [ ] **Step 3.12: Append failing test**

Append to `stability.rs`:

```rust
#[tokio::test]
async fn act_timeout_fires_with_phase_act_and_tool_name() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider: Arc<dyn AiProvider> = Arc::new(OneShotToolProvider {
        name: "slow_tool".into(),
        calls,
    });
    let (session, sid) = fresh_session("act-timeout").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(HangingTools);

    let mut deps = minimal_deps(session, tools, provider);
    deps.turn_timeout = Some(std::time::Duration::from_millis(200));
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.run(&sid, &mut cb, &cancel),
    ).await.expect("must return within 2s");

    match result {
        Err(HarnessError::StalledTurn { phase, .. }) => match phase {
            TurnPhase::Act { tool_name } => {
                assert_eq!(tool_name, "slow_tool");
            }
            other => panic!("expected Act phase, got {other:?}"),
        },
        other => panic!("expected StalledTurn(Act), got {other:?}"),
    }
}
```

- [ ] **Step 3.13: Run test (expect FAIL — act timeout not wired)**

Run: `cargo test -p alephcore --lib harness::tests::stability::act_timeout_fires_with_phase_act_and_tool_name -- --nocapture`
Expected: FAIL

### 3E. Wire Act timeout

- [ ] **Step 3.14: Wrap each tool execute in act() with timeout**

In `agent.rs` `act()` (the version from Step 2.7), replace the `match self.deps.tools.execute(...)` line with:

```rust
            let exec_fut = self.deps.tools.execute(&call.name, call.arguments.clone());
            let exec_result: Result<
                Result<ToolOutput, crate::tools::service::ToolError>,
                HarnessError,
            > = match self.deps.turn_timeout {
                Some(budget) => {
                    let started_call = std::time::Instant::now();
                    match tokio::time::timeout(budget, exec_fut).await {
                        Ok(inner) => Ok(inner),
                        Err(_) => Err(HarnessError::StalledTurn {
                            phase: TurnPhase::Act {
                                tool_name: call.name.clone(),
                            },
                            elapsed: started_call.elapsed(),
                        }),
                    }
                }
                None => Ok(exec_fut.await),
            };
            let inner = match exec_result {
                Ok(r) => r,
                Err(stalled) => return Err(stalled),
            };
            match inner {
```

> The original `match self.deps.tools.execute(...)` becomes `match inner` — keep the `Ok(output)` and `Err(e)` arms identical to Step 2.7's version (the trace + ToolResult/ToolError emit logic).

> Add `use crate::session::events::ToolOutput;` to agent.rs imports if not already there.

- [ ] **Step 3.15: Run test**

Run: `cargo test -p alephcore --lib harness::tests::stability::act_timeout_fires_with_phase_act_and_tool_name -- --nocapture`
Expected: PASS

### 3F. parent_cancel precedence

- [ ] **Step 3.16: Append test**

Append to `stability.rs`:

```rust
#[tokio::test]
async fn parent_cancel_takes_precedence_over_timeout() {
    let provider: Arc<dyn AiProvider> = Arc::new(HangingProvider);
    let (session, sid) = fresh_session("cancel-vs-timeout").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let mut deps = minimal_deps(session, tools, provider);
    // Long timeout (1s) so the parent cancel wins.
    deps.turn_timeout = Some(std::time::Duration::from_secs(1));
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel_clone.cancel();
    });

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.run(&sid, &mut cb, &cancel),
    ).await.expect("must return within 2s");

    assert!(matches!(result, Err(HarnessError::Cancelled)),
            "expected Cancelled, got {result:?}");
}
```

> Note: This requires that the cancel check fires while the LLM is still hung. The current `agent.rs` checks `cancel.is_cancelled()` only at the **top** of each iteration. A hung Think will not see parent cancel mid-await. Solution below.

- [ ] **Step 3.17: Make Think await race against parent cancel**

Replace the Step 3.10 wrapping with this version that races against `cancel`:

```rust
        let response = {
            let llm_fut = self.deps.llm.process(payload);
            let started = std::time::Instant::now();
            match self.deps.turn_timeout {
                Some(budget) => {
                    tokio::select! {
                        biased;
                        _ = parent_cancel.cancelled() => {
                            return Err(HarnessError::Cancelled);
                        }
                        _ = tokio::time::sleep(budget) => {
                            return Err(HarnessError::StalledTurn {
                                phase: TurnPhase::Think,
                                elapsed: started.elapsed(),
                            });
                        }
                        r = llm_fut => match r {
                            Ok(r) => r,
                            Err(e) => return Err(HarnessError::Llm(e)),
                        },
                    }
                }
                None => {
                    tokio::select! {
                        biased;
                        _ = parent_cancel.cancelled() => {
                            return Err(HarnessError::Cancelled);
                        }
                        r = llm_fut => match r {
                            Ok(r) => r,
                            Err(e) => return Err(HarnessError::Llm(e)),
                        },
                    }
                }
            }
        };
```

> This requires `run_turn_internal` to take `parent_cancel: &CancellationToken`. Update its signature and the single caller (`Harness::run` in agent.rs:Step 1.13).

- [ ] **Step 3.18: Update run_turn_internal signature + caller**

Change `agent.rs` `run_turn_internal`:

```rust
    async fn run_turn_internal(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
        iterations: usize,
        tool_calls_made: usize,
        parent_cancel: &CancellationToken,
    ) -> Result<(TurnState, usize, bool), HarnessError> {
```

In `Harness::run`, update the call site (was `self.run_turn_internal(session_id, callback, iterations, tool_calls_made).await`):

```rust
            match self
                .run_turn_internal(session_id, callback, iterations, tool_calls_made, cancel)
                .await
            {
```

In the trait method `run_turn` (`agent.rs:491`), update the inner call:

```rust
        let cancel = tokio_util::sync::CancellationToken::new();
        self.run_turn_internal(session_id, callback, iterations, tool_calls_made, &cancel)
            .await
            .map(|(state, _, _)| state)
```

- [ ] **Step 3.19: Run cancel test**

Run: `cargo test -p alephcore --lib harness::tests::stability::parent_cancel_takes_precedence_over_timeout -- --nocapture`
Expected: PASS

- [ ] **Step 3.20: Re-run think_timeout test**

Run: `cargo test -p alephcore --lib harness::tests::stability::think_timeout_fires_with_phase_think -- --nocapture`
Expected: PASS (still works after select! refactor)

### 3G. outcome_mapping_for_stalled_turn

- [ ] **Step 3.21: Append test**

Append to `stability.rs`:

```rust
#[tokio::test]
async fn outcome_mapping_for_stalled_turn() {
    let (sink, events) = RecordingTraceSink::new();
    let provider: Arc<dyn AiProvider> = Arc::new(HangingProvider);
    let (session, sid) = fresh_session("trace-stalled").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let mut deps = minimal_deps(session, tools, provider);
    deps.turn_timeout = Some(std::time::Duration::from_millis(150));
    deps.trace_sink = Some(sink);
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.run(&sid, &mut cb, &cancel),
    ).await.expect("must return within 2s");

    let captured = events.lock().unwrap().clone();
    let session_completed = captured.iter().rev().find_map(|e| match e {
        LoopTraceEvent::SessionCompleted { outcome, .. } => Some(*outcome),
        _ => None,
    }).expect("SessionCompleted must be emitted");
    assert_eq!(
        session_completed,
        crate::harness::trace::LoopTraceSessionOutcome::Cancelled,
        "StalledTurn should map to Cancelled outcome",
    );
}
```

- [ ] **Step 3.22: Run test**

Run: `cargo test -p alephcore --lib harness::tests::stability::outcome_mapping_for_stalled_turn -- --nocapture`
Expected: PASS (the err arm in `Harness::run` from Step 1.13 maps any non-Ok exit to `Cancelled`).

> If FAIL: re-check Step 1.13's `Err(e) => { let session_outcome = match &e { ... } => Cancelled, ... }`. The wildcard arm should already cover `StalledTurn` since both `Cancelled`-class and `Stalled`/`StalledTurn` map to `Cancelled`.

- [ ] **Step 3.23: Full harness sweep**

Run: `cargo test -p alephcore --lib harness:: -- --nocapture`
Expected: ALL PASS

- [ ] **Step 3.24: Commit Step 3**

```bash
git add src/harness/agent.rs src/harness/deps.rs src/harness/trait_def.rs \
        src/harness/mod.rs src/harness/tests/ \
        src/agents/subagent_spawner.rs src/orchestrator/harness_bridge.rs \
        tests/harness_run_e2e.rs
git commit -m "feat(harness): per-turn timeout with TurnPhase classification + cancel precedence"
```

---

## Task 4: Step 4 — StallTracker record_activity Dispersion

**Files:**
- Modify: `src/harness/agent.rs`
- Modify: `src/harness/tests/stability.rs`

### 4A. TDD

- [ ] **Step 4.1: Append failing test**

Append to `stability.rs`:

```rust
use crate::harness::stall::StallConfig;

#[tokio::test]
async fn cross_turn_stall_still_works() {
    // Provider returns text-only "thinking..." every turn — model produces
    // no tool_calls, no progress. With stall timeout 200ms and turn cadence
    // ~50ms, after a few turns is_stalled() should fire.
    //
    // Without Task 4: record_activity is called after every turn, so the
    // tracker keeps resetting and never trips.
    // With Task 4: record_activity is also called inside the turn (after
    // Think completes), but if the model truly produces nothing useful for
    // longer than the stall budget, eventually is_stalled() trips.
    //
    // We force this by wrapping the provider in a deliberate sleep so the
    // turn-to-turn cadence exceeds the stall budget.
    struct SlowTextProvider;
    impl AiProvider for SlowTextProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            Box::pin(async move {
                // 100ms per LLM call; nothing happens between calls.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                Ok(ProviderResponse::text_only("...".into()))
            })
        }
        fn name(&self) -> &str { "slow-text" }
        fn color(&self) -> &str { "#000000" }
    }

    let provider: Arc<dyn AiProvider> = Arc::new(SlowTextProvider);
    let (session, sid) = fresh_session("cross-turn-stall").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let mut deps = minimal_deps(session, tools, provider);
    // After Task 4, record_activity also fires inside the Think completion
    // path, so this test specifically exercises the "no Think completion at all"
    // case. We force that by pre-stalling the tracker via a 0-second budget.
    deps.stall_config = Some(StallConfig::default()
        .with_timeout(std::time::Duration::from_millis(50))
        .with_check_interval(std::time::Duration::from_millis(10)));
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();

    // Sleep first to age the tracker past its budget BEFORE first turn.
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        harness.run(&sid, &mut cb, &cancel),
    ).await.expect("must return within 2s");

    assert!(matches!(result, Err(HarnessError::Stalled { .. })),
            "expected Stalled, got {result:?}");
}
```

- [ ] **Step 4.2: Run test**

Run: `cargo test -p alephcore --lib harness::tests::stability::cross_turn_stall_still_works -- --nocapture`
Expected: PASS already (this scenario doesn't depend on the new dispersion — it's the pre-existing path). If FAIL, re-read `agent.rs:443-447` to confirm `is_stalled()` check still runs at top of loop.

> The dispersion (next step) is **defensive** — it ensures `record_activity` ALSO fires mid-turn so a long Think doesn't falsely trip stall. We add a second test for that.

### 4B. Disperse record_activity calls

- [ ] **Step 4.3: Add record_activity after Think completes**

In `agent.rs` `run_turn_internal`, immediately after the `self.deps.session.emit_event(session_id, assistant_event).await?;` line (around `agent.rs:208` in the post-Step-1.13 layout), insert:

```rust
        if let Some(ref tracker) = self.stall_tracker {
            tracker.record_activity().await;
        }
```

- [ ] **Step 4.4: Add record_activity after each tool execute**

In `agent.rs` `act()`, after the `match inner { Ok(output) => { ... emit_event(result_event) ... } }` arm and after the `Err(e) => { ... emit_event(error_event) ... }` arm — i.e., at the **end** of the `for call in tool_calls` body, just before the closing brace of the for loop, insert:

```rust
            if let Some(ref tracker) = self.stall_tracker {
                tracker.record_activity().await;
            }
```

- [ ] **Step 4.5: Append defensive test**

Append to `stability.rs`:

```rust
#[tokio::test]
async fn long_think_does_not_falsely_trip_stall() {
    // Provider takes 80ms per Think. Stall budget is 200ms. Model produces
    // text-only after first turn → Done. Without Task 4 dispersion, the
    // tracker would be aged 80ms+ at top of next iteration check, but with
    // dispersion it's reset right after Think.
    struct EightyMsThinkProvider {
        n: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl AiProvider for EightyMsThinkProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let n = self.n.clone();
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                let v = n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if v == 0 {
                    Ok(ProviderResponse {
                        text: None,
                        tool_calls: vec![NativeToolCall {
                            id: "c".into(), name: "ok_x".into(),
                            arguments: serde_json::json!({}),
                        }],
                        thinking: None, thinking_signature: None,
                        stop_reason: StopReason::ToolUse, usage: None,
                    })
                } else {
                    Ok(ProviderResponse::text_only("done".into()))
                }
            })
        }
        fn name(&self) -> &str { "80ms" }
        fn color(&self) -> &str { "#000000" }
    }

    let provider: Arc<dyn AiProvider> = Arc::new(EightyMsThinkProvider {
        n: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    });
    let (session, sid) = fresh_session("no-false-stall").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let mut deps = minimal_deps(session, tools, provider);
    deps.stall_config = Some(StallConfig::default()
        .with_timeout(std::time::Duration::from_millis(200))
        .with_check_interval(std::time::Duration::from_millis(10)));
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        harness.run(&sid, &mut cb, &cancel),
    ).await.expect("must finish within 3s");

    result.expect("legitimate two-turn run must succeed without stalling");
}
```

- [ ] **Step 4.6: Run defensive test**

Run: `cargo test -p alephcore --lib harness::tests::stability::long_think_does_not_falsely_trip_stall -- --nocapture`
Expected: PASS

- [ ] **Step 4.7: Full harness sweep**

Run: `cargo test -p alephcore --lib harness:: -- --nocapture`
Expected: ALL PASS

- [ ] **Step 4.8: Commit Step 4**

```bash
git add src/harness/agent.rs src/harness/tests/stability.rs
git commit -m "feat(harness): disperse stall record_activity into Think and Act seams"
```

---

## Task 5: Final Verification + CHANGELOG

- [ ] **Step 5.1: workspace test sweep**

Run: `just test-all`
Expected: ALL GREEN (core + desktop + proptest)

- [ ] **Step 5.2: clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: 0 warnings

- [ ] **Step 5.3: line-count budget verify**

Run: `wc -l src/harness/*.rs`
Expected: total ≤ 2300 lines (current 2066 + ≤234 budget)

Run: `ls src/harness/*.rs | wc -l`
Expected: 10 files (no new .rs at harness root level)

- [ ] **Step 5.4: Update CHANGELOG.md**

Append entry under the next unreleased section in `CHANGELOG.md`. If a `## [Unreleased]` heading doesn't exist, add one above the most recent dated section.

```markdown
### Added
- Harness per-turn timeout (`HarnessDeps.turn_timeout`) with `TurnPhase` classification — Think/Act phases independently watchdogged via `tokio::select!` + cancel race.
- Harness `TraceSink` fire points across the full turn lifecycle (TurnStarted, TurnStateEntered, TextEmitted, ToolCall lifecycle, TurnCompleted, SessionCompleted).
- Harness consecutive-failure cap (`HarnessDeps.consecutive_failure_cap`) — terminates the loop after N turns of all-failed tool calls to prevent infinite retry.

### Fixed
- Tool failures inside `act()` no longer abort the entire session. Errors are now persisted as `ToolError` events and surfaced to the next Think as `tool_result.is_error=true`, matching Claude Code recoverable-error semantics.
- `StallTracker::record_activity` now fires after Think completion and after each tool call, eliminating false stalls during legitimate long Think phases.
```

- [ ] **Step 5.5: Manual long-run smoke (optional but recommended)**

Run: `cargo run --bin aleph-server --release` (background) and exercise it for ≥30 minutes with a looping-tool-error mock if available, observing logs for steady TraceSink emissions and stable memory.

If smoke unavailable, mark this step complete with: "Skipped — no mock harness available locally; verified via test-all suite."

- [ ] **Step 5.6: Commit CHANGELOG**

```bash
git add CHANGELOG.md
git commit -m "docs(changelog): harness P0 stability rescue (4 commits)"
```

- [ ] **Step 5.7: Push or hand off**

```bash
git log --oneline -8
```

Expected: 5 commits visible (test scaffold, 4 P0 commits, CHANGELOG):
```
xxxxxxx docs(changelog): harness P0 stability rescue (4 commits)
xxxxxxx feat(harness): disperse stall record_activity into Think and Act seams
xxxxxxx feat(harness): per-turn timeout with TurnPhase classification + cancel precedence
xxxxxxx feat(harness): rescue tool errors back to model + cap consecutive failures
xxxxxxx feat(harness): wire TraceSink fire points across full turn lifecycle
xxxxxxx test(harness): scaffold stability test module
```

Push when ready.

---

## Self-Review Notes

The author ran the spec-coverage check inline and confirmed:

- **Spec §3 (act error rescue + consecutive cap):** Tasks 2.5–2.14 cover both halves with 3 tests.
- **Spec §4 (timeout + StallTracker):** Tasks 3.1–3.22 (timeout) and 4.1–4.6 (record_activity dispersion) cover both. 4 tests total — `cross_turn_stall_still_works` and `long_think_does_not_falsely_trip_stall` together cover the §4.5 "cross_turn_stall_still_works" requirement (the latter is the defensive complement).
- **Spec §5 (TraceSink wiring):** Tasks 1.1–1.16 cover the 5 fire points + zero-overhead + outcome mapping (last via 3.21–3.22).
- **Spec §6 (sequence):** Tasks 1, 2, 3, 4 strictly preserve the 4-step order. Task 0 is a pre-step and Task 5 is post-verification.
- **Spec §7 (risks):** Each risk has a corresponding mitigation in the plan — `HarnessError::Tool` retained (Step 2.7), `Option` defaults (Step 2.1, 3.5), trait doc regulation (Step 1.1), `Option<usize>` cap (Step 2.1).
- **Spec §8 (acceptance):** Task 5 line-count + file-count + clippy + test-all all included.

**No placeholders detected.** All test code is complete; all file paths are exact; all commit messages are written out.

**Type consistency check:** `TurnPhase` is added in trait_def.rs (Step 3.1), re-exported in mod.rs (Step 3.3), used in HarnessError (Step 3.2), and imported in agent.rs (Step 3.10) and tests (Step 3.8). All occurrences use `TurnPhase::Think` / `TurnPhase::Act { tool_name }` consistently.
