# Workspace Trace Persistence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the WebChat right-side workspace panel (and left-chat step summaries) survive reload / session-switch by rebuilding from the already-persisted `task_traces` store, and collapse left-chat intermediate steps into a bounded scrolling strip that auto-collapses on completion.

**Architecture:** A single projection function `apply_trace_event` feeds two sources — the live WS stream (during a run) and a new read-only RPC `trace.by_runs` (on load). Trace events are already persisted per-run (`task_id == run_id`) in the `task_traces` observability table (NOT the memory store). The panel replays persisted events through the same projection used live, then merges the rebuilt intermediate steps with the final answers from `chat.history`.

**Tech Stack:** Rust (alephcore gateway, rusqlite `StateDatabase`), Leptos/WASM panel (`aleph-panel` crate), JSON-RPC.

---

## File Structure

**Server (alephcore):**
- `src/gateway/handlers/trace_replay.rs` — add `handle_by_runs` (read-only, wraps existing `db.get_traces_by_task`).
- `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` — register `trace.by_runs` alongside `trace.list`/`trace.get`.

**Panel (aleph-panel):**
- `src/api/trace.rs` — add `TraceApi::by_runs`.
- `src/views/chat/events.rs` — extract `apply_trace_event` pure projection; live arm calls it.
- `src/views/chat/replay.rs` (new) — pure `merge_session_messages` helper.
- `src/components/chat_sidebar.rs` — fetch + replay on session load; drop hardcoded-empty hydration; reset badge.
- `src/views/chat/timeline.rs` — `TimelineRow::StepStrip` aggregation.
- `src/views/chat/messages.rs` — render `StepStrip` (scroll + auto-collapse).

---

## Task 1: Server RPC `trace.by_runs`

**Files:**
- Modify: `src/gateway/handlers/trace_replay.rs`
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:1277-1310`

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block at the end of `src/gateway/handlers/trace_replay.rs` (create the block if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::resilience::types::{AgentTask, RiskLevel, TaskTrace};
    use aleph_protocol::events::{AgentTraceEvent, AgentTraceTextKind};

    fn req(params: serde_json::Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "trace.by_runs".into(),
            params: Some(params),
            id: Some(json!(1)),
        }
    }

    async fn seed_run(db: &StateDatabase, run_id: &str, texts: &[&str]) {
        db.insert_agent_task(&AgentTask::new(run_id, "s", "coder", "x", RiskLevel::Low))
            .await
            .unwrap();
        for (i, t) in texts.iter().enumerate() {
            db.insert_trace(&TaskTrace::new(
                run_id,
                i as u32,
                AgentTraceEvent::TextEmitted {
                    iteration: i,
                    stream: AgentTraceTextKind::Final,
                    text: (*t).to_string(),
                },
            ))
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn by_runs_groups_events_per_run_in_step_order() {
        let db = Arc::new(StateDatabase::in_memory().unwrap());
        seed_run(&db, "run-a", &["a0", "a1"]).await;
        seed_run(&db, "run-b", &["b0"]).await;

        let resp = handle_by_runs(
            req(json!({ "run_ids": ["run-a", "run-b", "run-missing"] })),
            db,
        )
        .await;

        let result = resp.result.expect("success");
        let runs = result.get("runs").unwrap();
        assert_eq!(runs.get("run-a").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(runs.get("run-b").unwrap().as_array().unwrap().len(), 1);
        // Missing run → empty array, never an error.
        assert_eq!(runs.get("run-missing").unwrap().as_array().unwrap().len(), 0);
        // Step order preserved: first event of run-a is text "a0".
        let first = &runs.get("run-a").unwrap().as_array().unwrap()[0];
        assert_eq!(first.get("text").unwrap().as_str().unwrap(), "a0");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib trace_replay::tests::by_runs_groups_events_per_run_in_step_order`
Expected: FAIL — `handle_by_runs` not found.

- [ ] **Step 3: Write minimal implementation**

In `src/gateway/handlers/trace_replay.rs`, add after `handle_get`:

```rust
#[derive(Debug, Default, Deserialize)]
struct TraceByRunsParams {
    #[serde(default)]
    run_ids: Vec<String>,
}

/// Max distinct runs accepted per call (a chat session has a handful).
const MAX_RUNS: usize = 200;

/// Read-only: return the persisted agent-trace event stream for each given
/// run_id (= task_id), grouped by run, ordered by step_index. Unknown or
/// trace-less runs yield an empty array (never an error). Reads the
/// `task_traces` observability table only — never the memory store.
pub async fn handle_by_runs(request: JsonRpcRequest, db: Arc<StateDatabase>) -> JsonRpcResponse {
    let params: TraceByRunsParams = match request.params.as_ref() {
        Some(v) => match serde_json::from_value(v.clone()) {
            Ok(p) => p,
            Err(_) => {
                return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid params");
            }
        },
        None => TraceByRunsParams::default(),
    };

    let mut runs = serde_json::Map::new();
    for run_id in params.run_ids.into_iter().take(MAX_RUNS) {
        let events: Vec<Value> = match db.get_traces_by_task(&run_id).await {
            Ok(traces) => traces
                .into_iter()
                .map(|t| serde_json::to_value(&t.event).unwrap_or(Value::Null))
                .collect(),
            Err(e) => {
                tracing::warn!(run_id = %run_id, error = %e, "trace.by_runs: load failed");
                Vec::new()
            }
        };
        runs.insert(run_id, Value::Array(events));
    }
    JsonRpcResponse::success(request.id, json!({ "runs": runs }))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib trace_replay::tests::by_runs_groups_events_per_run_in_step_order`
Expected: PASS.

- [ ] **Step 5: Register the handler**

In `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs`, inside the `Some(trace_db) => { ... }` arm (right after the `trace.get` registration, before the closing `}` of the `Some` arm), add:

```rust
                let trace_by_runs_db = trace_db.clone();
                server.handlers_mut().register("trace.by_runs", move |req| {
                    let db = trace_by_runs_db.clone();
                    async move {
                        alephcore::gateway::handlers::trace_replay::handle_by_runs(req, db).await
                    }
                });
```

And in the `None => { ... }` arm, after the `trace.get` SERVICE_UNAVAILABLE registration, add:

```rust
                server
                    .handlers_mut()
                    .register("trace.by_runs", |req| async move {
                        alephcore::gateway::protocol::JsonRpcResponse::error(
                            req.id,
                            alephcore::gateway::protocol::SERVICE_UNAVAILABLE,
                            "trace.by_runs disabled: no state_database configured".to_string(),
                        )
                    });
```

> Note: `trace_db` was previously moved into the `trace.get` closure as `trace_get_db = trace_db;`. Change that line to `trace_get_db = trace_db.clone();` so `trace_db` remains available for the `trace.by_runs` clone above.

- [ ] **Step 6: Verify compile + commit**

Run: `cargo check -p alephcore && cargo build -p alephcore --bin aleph-server`
Expected: clean build.

```bash
git add src/gateway/handlers/trace_replay.rs src/bin/aleph-server/commands/start/builder/agent_init/mod.rs
git commit -m "gateway: add read-only trace.by_runs RPC for panel trace rehydration"
```

---

## Task 2: Panel `TraceApi::by_runs`

**Files:**
- Modify: `interfaces/webchat/src/api/trace.rs`

- [ ] **Step 1: Add the API method**

Add to `impl TraceApi` in `interfaces/webchat/src/api/trace.rs`:

```rust
    /// Fetch persisted agent-trace events for the given run_ids, grouped by
    /// run_id (= task_id). Used to rehydrate the chat step strip + workspace
    /// panel after reload / session switch. Unknown runs map to empty vecs.
    pub async fn by_runs(
        state: &DashboardState,
        run_ids: Vec<String>,
    ) -> Result<std::collections::HashMap<String, Vec<serde_json::Value>>, String> {
        let result = state
            .rpc_call("trace.by_runs", serde_json::json!({ "run_ids": run_ids }))
            .await?;
        let runs = result
            .get("runs")
            .cloned()
            .unwrap_or(serde_json::Value::Object(Default::default()));
        serde_json::from_value(runs).map_err(|e| format!("Failed to parse trace.by_runs: {e}"))
    }
```

- [ ] **Step 2: Verify compile**

Run: `cargo check -p aleph-panel`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/api/trace.rs
git commit -m "panel: add TraceApi::by_runs client for trace rehydration"
```

---

## Task 3: Extract `apply_trace_event` projection (live + replay share it)

**Files:**
- Modify: `interfaces/webchat/src/views/chat/events.rs`

- [ ] **Step 1: Write the failing test**

Add to the bottom of `interfaces/webchat/src/views/chat/events.rs`:

```rust
#[cfg(test)]
mod projection_tests {
    use super::*;
    use crate::state::layout::WorkspaceState;
    use crate::views::chat::state::ChatState;
    use leptos::prelude::*;
    use serde_json::json;

    #[test]
    fn apply_trace_event_builds_steps_and_payloads() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        let ws = WorkspaceState::new();

        // Two iterations: a tool call in step 1, narration in step 2.
        let events = vec![
            json!({ "kind": "turn_started", "iteration": 1 }),
            json!({ "kind": "tool_call_started", "iteration": 1,
                    "call": { "tool_id": "t1", "tool_name": "search", "input": { "q": "rust" } } }),
            json!({ "kind": "tool_call_completed", "iteration": 1,
                    "call": { "tool_id": "t1", "tool_name": "search", "duration_ms": 5 },
                    "result": { "ok": true } }),
            json!({ "kind": "turn_started", "iteration": 2 }),
            json!({ "kind": "text_emitted", "iteration": 2, "stream": "final", "text": "done" }),
        ];
        for ev in &events {
            apply_trace_event(chat, ws, "run-1", ev);
        }

        // Right-panel payloads captured.
        let payload = ws.get_tool_payload("run-1", "t1").expect("payload");
        assert!(payload.args.is_some());
        assert!(payload.result.is_some());

        // Left-chat intermediate step bubbles created with iteration tags.
        let msgs = chat.messages.get_untracked();
        let tagged: Vec<usize> = msgs.iter().filter_map(|m| m.iteration).collect();
        assert_eq!(tagged, vec![1, 2]);
        // Step 2 narration applied.
        assert!(msgs.iter().any(|m| m.iteration == Some(2) && m.content.contains("done")));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p aleph-panel --lib views::chat::events::projection_tests`
Expected: FAIL — `apply_trace_event` not found.

- [ ] **Step 3: Extract the function**

In `events.rs`, add this `pub(crate)` function (lift the body verbatim from the current `"agent_trace"` arm's inner `match kind { ... }`, lines ~93–178):

```rust
/// Project one persisted/live `AgentTraceEvent` (tagged `kind` or `type`)
/// onto ChatState (step bubbles, tool status, narration) + WorkspaceState
/// (tool args/result payloads, current iteration). The single projection
/// shared by the live WS stream and `trace.by_runs` replay so the two paths
/// can never drift.
pub(crate) fn apply_trace_event(
    chat: ChatState,
    workspace: WorkspaceState,
    run_id: &str,
    trace_event: &serde_json::Value,
) {
    let kind = trace_event
        .get("type")
        .or_else(|| trace_event.get("kind"))
        .and_then(|k| k.as_str())
        .unwrap_or("");

    match kind {
        "tool_call_started" => {
            let call = trace_event.get("call");
            let tool_id = call
                .and_then(|c| c.get("tool_id"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let tool_name = call
                .and_then(|c| c.get("tool_name"))
                .and_then(|t| t.as_str())
                .unwrap_or("tool");
            chat.update_tool(run_id, tool_id, tool_name, "running", None);
            workspace.note_activity();
            if !tool_id.is_empty() {
                let args = call
                    .and_then(|c| c.get("input").or_else(|| c.get("args")))
                    .cloned();
                if let Some(args) = args {
                    workspace.record_tool_args(run_id, tool_id, args);
                }
            }
        }
        "tool_call_completed" => {
            let call = trace_event.get("call");
            let tool_id = call
                .and_then(|c| c.get("tool_id"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let tool_name = call
                .and_then(|c| c.get("tool_name"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let duration = call
                .and_then(|c| c.get("duration_ms"))
                .and_then(|d| d.as_u64());
            let result = trace_event
                .get("result")
                .unwrap_or(&serde_json::Value::Null);
            let status = if result.get("Error").is_some() {
                "failed"
            } else {
                "completed"
            };
            chat.update_tool(run_id, tool_id, tool_name, status, duration);
            if !tool_id.is_empty() {
                workspace.record_tool_result(run_id, tool_id, result.clone());
            }
        }
        "tool_summary" => {
            if let Some(summary) = trace_event.get("summary").and_then(|s| s.as_str()) {
                append_reasoning(chat, summary);
            }
        }
        "turn_started" => {
            let Some(iteration) = trace_event
                .get("iteration")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
            else {
                return;
            };
            chat.begin_step(run_id, iteration);
            workspace.set_current_iteration(run_id, iteration);
        }
        "text_emitted" => {
            let Some(iteration) = trace_event
                .get("iteration")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
            else {
                return;
            };
            let text = trace_event
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("");
            chat.set_step_text(run_id, iteration, text);
        }
        _ => {}
    }
}
```

- [ ] **Step 4: Call it from the live arm**

Replace the body of the `"agent_trace" => { ... }` arm in `subscribe_run_events` with:

```rust
            "agent_trace" => {
                if let Ok(mut runs) = trace_runs.lock() {
                    runs.insert(run_id.to_string());
                }
                let Some(trace_event) = data.get("event") else {
                    return;
                };
                apply_trace_event(chat, workspace, run_id, trace_event);
            }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p aleph-panel --lib views::chat::events`
Expected: PASS (new projection test + existing event tests unchanged).

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/views/chat/events.rs
git commit -m "panel: extract apply_trace_event projection shared by live + replay"
```

---

## Task 4: Pure `merge_session_messages` helper

**Files:**
- Create: `interfaces/webchat/src/views/chat/replay.rs`
- Modify: `interfaces/webchat/src/views/chat/mod.rs` (add `pub(crate) mod replay;`)

- [ ] **Step 1: Write the failing test**

Create `interfaces/webchat/src/views/chat/replay.rs`:

```rust
//! Pure assembly of a session transcript from persisted history (user +
//! final answers) interleaved with trace-replayed intermediate step bubbles.

use super::messages::run_id_from_message_id;
use super::state::ChatMessage;
use crate::api::chat::ChatMessage as WireMessage;

/// Merge `history` (chronological user + final assistant rows from
/// `chat.history`) with `replayed` intermediate step bubbles (each id
/// `intermediate-{run}-{n}`, produced by replaying trace through
/// `apply_trace_event`). For each assistant run, its intermediate steps are
/// placed immediately BEFORE its final answer. Intermediates whose run has no
/// final in history are appended at the end, grouped by first appearance.
pub fn merge_session_messages(history: &[WireMessage], replayed: &[ChatMessage]) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::new();
    let mut consumed: Vec<bool> = vec![false; replayed.len()];

    for (i, h) in history.iter().enumerate() {
        if h.role == "assistant" {
            if let Some(run) = h.run_id.as_deref() {
                for (j, r) in replayed.iter().enumerate() {
                    if !consumed[j] && run_id_from_message_id(&r.id) == run {
                        out.push(r.clone());
                        consumed[j] = true;
                    }
                }
            }
        }
        out.push(ChatMessage {
            timestamp: h
                .timestamp
                .as_deref()
                .and_then(super::timeline::parse_wire_timestamp),
            id: h.run_id.clone().unwrap_or_else(|| format!("hist-{i}")),
            role: h.role.clone(),
            content: h.content.clone(),
            tool_calls: vec![],
            is_streaming: false,
            is_intermediate: false,
            error: None,
            model_info: None,
            iteration: None,
        });
    }

    // Orphan intermediates (run never produced a final in history).
    for (j, r) in replayed.iter().enumerate() {
        if !consumed[j] {
            out.push(r.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::chat::state::{ChatMessage, ToolCallEntry};

    fn wire(role: &str, content: &str, run: Option<&str>) -> WireMessage {
        WireMessage {
            role: role.into(),
            content: content.into(),
            run_id: run.map(|s| s.to_string()),
            timestamp: None,
            metadata: None,
        }
    }

    fn step(run: &str, n: usize, content: &str) -> ChatMessage {
        ChatMessage {
            id: format!("intermediate-{run}-{n}"),
            role: "assistant".into(),
            content: content.into(),
            tool_calls: Vec::<ToolCallEntry>::new(),
            is_streaming: false,
            is_intermediate: true,
            error: None,
            model_info: None,
            iteration: Some(n),
            timestamp: None,
        }
    }

    #[test]
    fn intermediates_precede_their_final_answer() {
        let history = vec![
            wire("user", "hi", None),
            wire("assistant", "final-a", Some("run-a")),
        ];
        let replayed = vec![step("run-a", 1, "s1"), step("run-a", 2, "s2")];

        let merged = merge_session_messages(&history, &replayed);
        let ids: Vec<&str> = merged.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["intermediate-run-a-1", "intermediate-run-a-2", "run-a"]
        );
        // The user message stays first.
        assert_eq!(merged[0].role, "user");
        assert_eq!(merged.last().unwrap().content, "final-a");
    }

    #[test]
    fn orphan_intermediates_appended_at_end() {
        let history = vec![wire("user", "hi", None)];
        let replayed = vec![step("run-x", 1, "s1")];
        let merged = merge_session_messages(&history, &replayed);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[1].id, "intermediate-run-x-1");
    }
}
```

- [ ] **Step 2: Register the module**

In `interfaces/webchat/src/views/chat/mod.rs`, add alongside the other `mod` lines:

```rust
pub(crate) mod replay;
```

- [ ] **Step 3: Run test to verify it passes**

Run: `cargo test -p aleph-panel --lib views::chat::replay`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/chat/replay.rs interfaces/webchat/src/views/chat/mod.rs
git commit -m "panel: add pure merge_session_messages for trace replay assembly"
```

---

## Task 5: Wire replay into session load

**Files:**
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs:185-216`

- [ ] **Step 1: Replace the history-load closure body**

Replace the `spawn_local(async move { match ChatApi::history(...) { Ok(history) => { ... } ... } });` block (lines ~185-216) with:

```rust
        leptos::task::spawn_local(async move {
            match ChatApi::history(&dash, &key, Some(50)).await {
                Ok(history) => {
                    // Collect assistant run_ids to fetch their persisted traces.
                    let run_ids: Vec<String> = {
                        let mut seen = std::collections::HashSet::new();
                        history
                            .iter()
                            .filter(|m| m.role == "assistant")
                            .filter_map(|m| m.run_id.clone())
                            .filter(|r| seen.insert(r.clone()))
                            .collect()
                    };

                    // Replay persisted trace into the already-cleared real
                    // `chat` (builds intermediate step bubbles + tool_calls)
                    // and the real `ws` (right-panel payloads). Then harvest
                    // those intermediates and merge them with the history
                    // finals into the correct order. Replaying into the real
                    // chat (vs a detached scratch ChatState) avoids creating
                    // signals outside a live reactive owner inside spawn_local.
                    let replayed: Vec<crate::views::chat::state::ChatMessage> = if run_ids
                        .is_empty()
                    {
                        Vec::new()
                    } else {
                        match crate::api::trace::TraceApi::by_runs(&dash, run_ids).await {
                            Ok(runs) => {
                                if let Some(ws) = workspace {
                                    for (run_id, events) in &runs {
                                        for ev in events {
                                            crate::views::chat::events::apply_trace_event(
                                                chat, ws, run_id, ev,
                                            );
                                        }
                                    }
                                    // Loading an existing session = all activity
                                    // already "seen"; clear the live-only badge +
                                    // active-iteration marker the replay set.
                                    ws.unseen_activity.set(0);
                                    ws.current_iteration.set(None);
                                }
                                // Harvest the intermediate bubbles replay just
                                // appended to the real chat, then clear so the
                                // merged ordered list overwrites cleanly.
                                let harvested = chat.messages.get_untracked();
                                chat.messages.set(Vec::new());
                                harvested
                            }
                            Err(e) => {
                                web_sys::console::warn_1(
                                    &format!("trace.by_runs failed: {e}").into(),
                                );
                                Vec::new()
                            }
                        }
                    };

                    let merged =
                        crate::views::chat::replay::merge_session_messages(&history, &replayed);
                    chat.messages.set(merged);
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Failed to load history: {e}").into());
                }
            }
        });
```

> This removes the old hardcoded `tool_calls: vec![]` / `iteration: None` hydration — those fields now come from the replayed bubbles via `merge_session_messages`.

- [ ] **Step 2: Verify compile**

Run: `cargo check -p aleph-panel`
Expected: clean. (If `workspace` is not in scope as `Option<WorkspaceState>` here, confirm the existing `if let Some(ws) = workspace` usage earlier in the same function — it is used at line ~179 `ws.reset()` — and reuse that binding name.)

- [ ] **Step 3: Run panel tests (no regressions)**

Run: `cargo test -p aleph-panel --lib`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/components/chat_sidebar.rs
git commit -m "panel: rehydrate chat steps + workspace payloads from persisted trace on load"
```

---

## Task 6: `TimelineRow::StepStrip` aggregation

**Files:**
- Modify: `interfaces/webchat/src/views/chat/timeline.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `timeline.rs`:

```rust
    #[test]
    fn consecutive_intermediates_fold_into_one_strip() {
        use super::*;
        let msgs = vec![
            msg_user("u1", "hi"),
            msg_step("intermediate-run-a-1", 1, "s1", false),
            msg_step("intermediate-run-a-2", 2, "s2", false),
            msg_final("run-a", "answer"),
        ];
        let rows = derive_timeline(&msgs, "Today", "Yesterday");
        // Expect: (sep) user, StepStrip{run-a, 2 steps}, final Message.
        let strips: Vec<&TimelineRow> = rows
            .iter()
            .filter(|r| matches!(r, TimelineRow::StepStrip { .. }))
            .collect();
        assert_eq!(strips.len(), 1);
        if let TimelineRow::StepStrip { run_id, steps, completed } = strips[0] {
            assert_eq!(run_id, "run-a");
            assert_eq!(steps.len(), 2);
            assert!(*completed, "no streaming step → completed");
        } else {
            panic!("expected StepStrip");
        }
        // The final answer remains a Message row, not folded.
        assert!(rows
            .iter()
            .any(|r| matches!(r, TimelineRow::Message { message, .. } if message.id == "run-a")));
    }
```

Add these test helpers to the same test module (if equivalents do not already exist):

```rust
    fn msg_user(id: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: id.into(), role: "user".into(), content: content.into(),
            tool_calls: vec![], is_streaming: false, is_intermediate: false,
            error: None, model_info: None, iteration: None, timestamp: None,
        }
    }
    fn msg_step(id: &str, it: usize, content: &str, streaming: bool) -> ChatMessage {
        ChatMessage {
            id: id.into(), role: "assistant".into(), content: content.into(),
            tool_calls: vec![], is_streaming: streaming, is_intermediate: true,
            error: None, model_info: None, iteration: Some(it), timestamp: None,
        }
    }
    fn msg_final(run: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: run.into(), role: "assistant".into(), content: content.into(),
            tool_calls: vec![], is_streaming: false, is_intermediate: false,
            error: None, model_info: None, iteration: None, timestamp: None,
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p aleph-panel --lib views::chat::timeline::tests::consecutive_intermediates_fold_into_one_strip`
Expected: FAIL — `TimelineRow::StepStrip` variant does not exist.

- [ ] **Step 3: Add the variant + aggregation**

In `timeline.rs`, extend the `TimelineRow` enum:

```rust
    /// A run's consecutive intermediate step bubbles, folded into one
    /// bounded scrolling strip. `completed` is true when none of the steps
    /// is still streaming (→ render auto-collapsed to a single summary line).
    StepStrip {
        run_id: String,
        steps: Vec<ChatMessage>,
        completed: bool,
    },
```

In `derive_timeline`, change the per-message loop so that an assistant message with `is_intermediate == true` accumulates into a pending strip instead of pushing a `Message` row. Flush the strip (push a `StepStrip` row) when the next message is not a same-run intermediate, then process that message normally. Concretely, replace the body of the message-iteration loop with:

```rust
    let mut pending: Vec<ChatMessage> = Vec::new();

    let flush = |rows: &mut Vec<TimelineRow>, pending: &mut Vec<ChatMessage>| {
        if pending.is_empty() {
            return;
        }
        let run_id = run_id_of(&pending[0]);
        let completed = pending.iter().all(|m| !m.is_streaming);
        rows.push(TimelineRow::StepStrip {
            run_id,
            steps: std::mem::take(pending),
            completed,
        });
    };

    for m in messages {
        // ... existing day-separator logic stays here, BUT call
        // `flush(&mut rows, &mut pending);` immediately before pushing a
        // DaySeparator so a strip never straddles a day boundary ...

        if m.role == "assistant" && m.is_intermediate {
            // Different run than the pending strip → flush first.
            if pending
                .first()
                .map(|p| run_id_of(p) != run_id_of(m))
                .unwrap_or(false)
            {
                flush(&mut rows, &mut pending);
            }
            pending.push(m.clone());
            continue;
        }

        flush(&mut rows, &mut pending);
        rows.push(TimelineRow::Message { message: m.clone(), clock: /* existing clock expr */ });
    }
    flush(&mut rows, &mut pending);
```

Add a small helper near the top of `timeline.rs`:

```rust
/// Run id behind a message id (`intermediate-{run}-{n}` or `{run}`).
fn run_id_of(m: &ChatMessage) -> String {
    crate::views::chat::messages::run_id_from_message_id(&m.id)
}
```

Update `row_key` to cover the new variant:

```rust
        TimelineRow::StepStrip { run_id, steps, completed } => {
            format!("strip:{run_id}:{}:{completed}", steps.len())
        }
```

> Keep the existing `DaySeparator` insertion logic intact; only add the `flush(...)` call right before a separator is pushed, and route intermediate messages into `pending`. Reuse whatever existing expression produced `clock` for `Message` rows.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p aleph-panel --lib views::chat::timeline`
Expected: PASS (new test + existing timeline tests).

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/chat/timeline.rs
git commit -m "panel: fold consecutive intermediate steps into a StepStrip timeline row"
```

---

## Task 7: Render `StepStrip` (scroll + auto-collapse)

**Files:**
- Modify: `interfaces/webchat/src/views/chat/messages.rs:130-145` (the `TimelineRow` match in `MessageList`)

- [ ] **Step 1: Handle the new row in the render match**

In `MessageList`, the `For` children closure matches `TimelineRow`. Add a `StepStrip` arm alongside `DaySeparator` and `Message`:

```rust
                                    TimelineRow::StepStrip { run_id, steps, completed } => view! {
                                        <StepStrip run_id=run_id steps=steps completed=completed />
                                    }
                                    .into_any(),
```

- [ ] **Step 2: Add the `StepStrip` component**

Add to `messages.rs` (private to the chat module):

```rust
/// A run's intermediate steps folded into a bounded, internally-scrolling
/// strip. Running (`completed == false`) → expanded, scrollable, stick to the
/// newest step. Done (`completed == true`) → collapsed to a single summary
/// line the user can click to expand. Keeps the left chat column short.
#[component]
fn StepStrip(run_id: String, steps: Vec<ChatMessage>, completed: bool) -> impl IntoView {
    // Collapsed by default once the run is complete; running runs start open.
    let open = RwSignal::new(!completed);
    let count = steps.len();
    let summary = format!("{count} steps");

    view! {
        <div class="flex justify-start my-1">
            <div class="max-w-[80%] w-full rounded-2xl border border-border/50 bg-surface-sunken/40">
                <button
                    type="button"
                    class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                           text-[11px] uppercase tracking-wider text-text-tertiary
                           hover:text-text-secondary"
                    on:click=move |_| open.update(|o| *o = !*o)
                >
                    <span>{summary}</span>
                    <span class="ml-auto">
                        {move || if open.get() { "▾" } else { "▸" }}
                    </span>
                </button>
                <Show when=move || open.get()>
                    <div class="max-h-[220px] overflow-y-auto px-2 pb-2 flex flex-col gap-1">
                        {steps
                            .clone()
                            .into_iter()
                            .map(|m| view! { <MessageBubble message=m clock=String::new() /> })
                            .collect_view()}
                    </div>
                </Show>
            </div>
        </div>
    }
}
```

- [ ] **Step 3: Verify compile + build the WASM bundle**

Run: `cargo check -p aleph-panel`
Expected: clean.

- [ ] **Step 4: Run all panel tests**

Run: `cargo test -p aleph-panel --lib`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/chat/messages.rs
git commit -m "panel: render StepStrip with bounded scroll + auto-collapse on completion"
```

---

## Task 8: Build + manual verification

- [ ] **Step 1: Rebuild WASM + server (resource-embed chain)**

Run:
```bash
just wasm
cargo build --release -p alephcore --bin aleph-server
```
Expected: clean build of dist + binary.

- [ ] **Step 2: Hot-swap the live daemon**

Per CLAUDE.md Panel↔Daemon chain (dev daemon path):
```bash
./target/release/aleph-server stop
cargo run --release -p alephcore --bin aleph-server start
```
(Or the `.app` daemon swap if testing inside the installed app.)

- [ ] **Step 3: Manual checks**

In the panel:
1. Run a multi-step agent task with tool calls; confirm right panel streams StepCards with args/result, and left chat shows a scrolling StepStrip that follows the newest step.
2. Let the run complete → StepStrip auto-collapses to a single `N steps` line on the left; right panel still shows full detail.
3. Switch to another session and back → both the StepStrip (expandable) and the right-panel StepCards/args/result are rebuilt (NOT empty).
4. Reload the panel → same rehydration holds.
5. Confirm no memory pollution: the Memory dashboard shows no new rows from this activity (trace lives in `task_traces`, not the memory store).

- [ ] **Step 4: Final commit (if any tweaks)**

```bash
git add -A
git commit -m "panel: workspace trace persistence — manual verification tweaks"
```

---

## Notes for the implementer

- **Run id helper:** `run_id_from_message_id` (in `messages.rs`) maps both `assistant-{run}` and `intermediate-{run}-{n}` ids back to `{run}`. Reuse it everywhere a run id is derived from a message id.
- **No memory writes:** every read in this plan targets `task_traces` (observability). Do not touch `src/memory/` or any `raw_memories`/notes path.
- **Projection parity is the contract:** if you change `apply_trace_event`, both live and replay change together — that is intentional. The `projection_tests` test guards it.
- **WASM test host:** `web_sys` panics off-wasm; the pure helpers (`merge_session_messages`, `derive_timeline`) and signal-only logic run on the host test target. Anything touching `web_sys` (the `spawn_local` wiring in Task 5, the rendered components) is verified manually in Task 8, not unit-tested.
