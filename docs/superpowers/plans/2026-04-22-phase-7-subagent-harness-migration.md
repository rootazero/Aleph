# Phase 7: SubagentTool → Harness Spawner Migration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate `SubagentTool` off the legacy `AgentLoop` onto a new Harness-based spawner, then delete `src/agent_loop/` (~5,100 LOC net reduction).

**Architecture:** "就地换芯" — keep `SubagentTool`'s `LoopTool` API and `AgentRuntime::run(config) -> Result<LoopRunResult, String>` signature bit-identical; swap the internal engine from `AgentLoop` to a new `subagent_spawner` built on `AgentHarness`. Gateway path is unaffected (new `HarnessDeps` fields are Optional, default to None).

**Tech Stack:** Rust, Tokio, async-trait. Existing crates: `alephcore`. No new dependencies.

**Spec:** [docs/superpowers/specs/2026-04-22-phase-7-subagent-harness-migration-design.md](../specs/2026-04-22-phase-7-subagent-harness-migration-design.md)

**Worktree:** `.claude/worktrees/managed-agents-phase-7/` on branch `worktree-managed-agents-phase-7`.

**Baseline:** `main @ b1cf3379d`. Tests: 9133 lib passing + 2 pre-existing failures (`telegram::config::parse_v2_config_directly`, `memory::notes::ingest::prompts::base_prompt_snapshot`).

**Hard rules:**
- All commits prefixed `phase7:`, English bodies.
- No `git push`, no PR, no release in Phase 7 without explicit user approval.
- Each task ends with a `cargo test -p alephcore --lib` run that must show ≥ 9133 passing + exactly 2 failing.

---

## Spec Correction — LoopRunResult Fields

The spec under-specified `LoopRunResult` (listed 4 fields). Actual struct in `src/agent_loop/loop_core.rs:296-307` has **8 fields**. The relocated struct in Task 3 uses this full layout:

```rust
#[derive(Debug, Clone)]
pub struct LoopRunResult {
    pub final_text: Option<String>,
    pub iterations: usize,
    pub tool_calls_made: usize,
    pub total_tokens: usize,
    pub hit_limit: bool,
    pub cancelled: bool,
    pub chain_id: String,
    pub depth: u32,
}
```

Spawner fills them as:
- `hit_limit` ← `harness.hit_limit()`
- `cancelled` ← `true` iff outer `cancel.is_cancelled()` after run
- `chain_id` ← `base.chain.chain_id.clone()`
- `depth` ← `base.chain.depth` (child depth, already incremented by `child()`)
- `total_tokens` ← `0` (phase 7 accepts, per spec §2)

---

## File Structure (new + modified)

| Path | Role | Task |
|------|------|------|
| `src/harness/deps.rs` | Add `system_prompt`/`max_iterations` fields | T1, T2 |
| `src/harness/agent.rs` | Inject `system_prompt` into payload + override `run()` for cap | T1, T2 |
| `src/orchestrator/harness_bridge.rs` | Gateway `HarnessDeps` literal gains `system_prompt: None, max_iterations: None` | T1 |
| `src/agents/runtime.rs` | `LoopRunResult` local definition; `execute_via_harness` method | T3, T6, T7 |
| `src/agents/allowlist_tool_service.rs` (NEW) | `ToolService` decorator filtering by `agent_def.is_tool_allowed` | T4 |
| `src/agents/subagent_spawner.rs` (NEW) | Child `SessionKey::Ephemeral` → seed → harness.run → extract `LoopRunResult` | T5 |
| `src/agents/subagent_tool.rs` | Add 3 ctor args; delete fork path | T6, T7 |
| `src/agents/mod.rs` | Export new modules; remove `SharedSnapshot` re-export | T4, T5, T9 |
| `src/tools/scoped.rs` | Update 2 `SubagentTool::new` callers | T6 |
| `src/gateway/execution_engine/run_loop.rs` | Update 1 `SubagentTool::new` caller | T6 |
| `src/agent_loop/` (DELETE dir) | Delete all 4 files + directory | T9 |
| `src/lib.rs` | Remove `pub mod agent_loop;` | T9 |
| `scripts/check-phase7-exit.sh` (NEW) | Exit-gate script | T11 |

---

## Task 1: HarnessDeps gains `system_prompt` + AgentHarness injects it

**Files:**
- Modify: `src/harness/deps.rs`
- Modify: `src/harness/agent.rs:269` (RequestPayload construction)
- Modify: `src/orchestrator/harness_bridge.rs:117-127` (HarnessDeps literal)
- Test: add to existing `#[cfg(test)] mod tests` in `src/harness/agent.rs`

### Pre-flight

- [ ] **1.1 Baseline sanity**

Run:
```bash
cargo test -p alephcore --lib --no-run 2>&1 | tail -5
```
Expected: `Finished test [...]` with zero compile errors. If compile fails, stop and report.

### Red → Green → Refactor

- [ ] **1.2 Add `system_prompt` field to `HarnessDeps`**

In `src/harness/deps.rs`, append the new Optional field to the struct (keep all existing fields and doc-comments intact):

```rust
pub struct HarnessDeps {
    // ... existing fields unchanged ...
    pub trace_sink: Option<Arc<dyn TraceSink>>,

    /// System prompt injected into every RequestPayload. Subagent path builds
    /// this via PromptBuilder at spawn time; Gateway passes None for now.
    pub system_prompt: Option<String>,
}
```

- [ ] **1.3 Update Gateway `HarnessDeps` literal in `harness_bridge.rs`**

In `src/harness/../orchestrator/harness_bridge.rs` around line 117-127 (the `HarnessDeps { session, tools, sandbox, llm, stop_hooks, context_budget, context_compactor, skill_prefetcher, trace_sink } ` literal), add `system_prompt: None,` at the end:

```rust
let deps = HarnessDeps {
    session: self.session_service.clone(),
    tools,
    sandbox,
    llm,
    stop_hooks: self.stop_hooks.clone(),
    context_budget: self.context_budget.clone(),
    context_compactor: self.context_compactor.clone(),
    skill_prefetcher: self.skill_prefetcher.clone(),
    trace_sink: trace_sink.clone(),
    system_prompt: None,
};
```

- [ ] **1.4 Confirm crate compiles with the new field (no behavior change yet)**

Run:
```bash
cargo check -p alephcore --lib 2>&1 | tail -10
```
Expected: zero errors (baseline still 9133 passing because field is unused).

- [ ] **1.5 Write the failing test for system_prompt injection**

Append to `src/harness/agent.rs` inside `#[cfg(test)] mod tests`:

```rust
#[tokio::test]
async fn system_prompt_flows_into_request_payload() {
    use crate::providers::adapter::RequestPayload;
    use crate::providers::response::ProviderResponse;
    use crate::providers::AiProvider;
    use crate::session::events::{MessageContent, SessionEvent, TurnTrigger, now_ms};
    use crate::routing::session_key::SessionKey;
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
    use std::sync::{Arc, Mutex};

    /// Mock provider that records the system_prompt it receives, returns empty response.
    struct RecordingProvider {
        captured: Arc<Mutex<Option<String>>>,
    }

    #[async_trait::async_trait]
    impl AiProvider for RecordingProvider {
        fn name(&self) -> &str { "recording" }
        async fn process(&self, payload: RequestPayload<'_>) -> crate::error::Result<ProviderResponse> {
            *self.captured.lock().unwrap() = payload.system_prompt.map(|s| s.to_string());
            Ok(ProviderResponse::text("ok"))  // empty tool_calls → TurnState::Done
        }
        fn as_http_provider(&self) -> Option<&dyn crate::providers::HttpProvider> { None }
    }

    let captured = Arc::new(Mutex::new(None));
    let provider = Arc::new(RecordingProvider { captured: captured.clone() });

    // Build a minimal session service.
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate_add_session_events(&conn).unwrap();
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    let session: Arc<dyn crate::session::service::SessionService> =
        Arc::new(InProcessActorSessionService::new(store));

    // Minimal ToolService + Sandbox stubs (both required by HarnessDeps).
    let tools = crate::tools::service::testing::in_memory(); // helper, see note below
    let sandbox = crate::sandbox::noop_sandbox();             // helper, see note below

    let sid = SessionKey::ephemeral("test-syspr");
    session.attach(sid.clone()).await.unwrap();
    // Seed a user turn.
    let turn = uuid::Uuid::new_v4();
    session.emit_event(&sid, SessionEvent::TurnStarted {
        turn_id: turn, trigger: TurnTrigger::UserMessage, at: now_ms(),
    }).await.unwrap();
    session.emit_event(&sid, SessionEvent::UserMessage {
        turn_id: turn, content: MessageContent { text: "hello".into(), blocks: vec![] }, at: now_ms(),
    }).await.unwrap();

    let deps = crate::harness::deps::HarnessDeps {
        session: session.clone(),
        tools,
        sandbox,
        llm: provider,
        stop_hooks: None,
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
        trace_sink: None,
        system_prompt: Some("ROLE: SPEC-BOT".into()),
    };
    let harness = crate::harness::agent::AgentHarness::new(deps);
    let mut cb = crate::harness::callback::NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    harness.run(&sid, &mut cb, &cancel).await.expect("harness run");

    let got = captured.lock().unwrap().clone();
    assert_eq!(got.as_deref(), Some("ROLE: SPEC-BOT"));
}
```

**Note on helpers `testing::in_memory()` and `noop_sandbox()`:** If these don't already exist, introduce them inline as `#[cfg(test)]` helpers in the respective modules. Keep them minimal:

- `crate::tools::service::testing::in_memory()` → returns `Arc<dyn ToolService>` with empty tool list, `execute` always `Err(ToolError::NotFound { name })`.
- `crate::sandbox::noop_sandbox()` → returns `Arc<dyn Sandbox>` whose `exec` is unimplemented!() (acceptable because the test provider never triggers a tool call).

Check first: `grep -rn 'in_memory\|noop_sandbox\|NoopSandbox' src/tools/ src/sandbox/` — if either exists, use the existing one instead of creating.

- [ ] **1.6 Run the test to confirm it fails**

Run:
```bash
cargo test -p alephcore --lib system_prompt_flows_into_request_payload 2>&1 | tail -30
```
Expected: test FAILS with `assertion failed: left=None, right=Some("ROLE: SPEC-BOT")` (because `run_turn` still calls `RequestPayload::new(&messages)` — no system_prompt set).

- [ ] **1.7 Implement system_prompt injection in `run_turn`**

In `src/harness/agent.rs` around line 269, replace:

```rust
let payload = RequestPayload::new(&messages);
```

with:

```rust
let payload = match self.deps.system_prompt.as_deref() {
    Some(sp) => RequestPayload::new(&messages).with_system(Some(sp)),
    None => RequestPayload::new(&messages),
};
```

- [ ] **1.8 Run the test to confirm it passes**

```bash
cargo test -p alephcore --lib system_prompt_flows_into_request_payload 2>&1 | tail -10
```
Expected: `1 passed`.

- [ ] **1.9 Run full lib suite — baseline holds**

```bash
cargo test -p alephcore --lib 2>&1 | tail -5
```
Expected: `test result: ok. 9134 passed; 2 failed` (baseline + 1 new test).

- [ ] **1.10 Clippy clean**

```bash
cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -10
```
Expected: zero errors.

- [ ] **1.11 Commit**

```bash
git add src/harness/deps.rs src/harness/agent.rs src/orchestrator/harness_bridge.rs
# plus any new test-helper files from 1.5
git commit -m "phase7: HarnessDeps.system_prompt field + AgentHarness injection

Gateway passes None (preserves baseline behavior). Subagent path will
populate this via PromptBuilder in Task 5.

Test: system_prompt_flows_into_request_payload verifies payload.system_prompt
carries through to the provider."
```

---

## Task 2: HarnessDeps gains `max_iterations` + AgentHarness overrides `Harness::run`

**Files:**
- Modify: `src/harness/deps.rs`
- Modify: `src/harness/agent.rs` (add `impl Harness::run` method body)
- Modify: `src/orchestrator/harness_bridge.rs:117-127` (literal gains `max_iterations: None`)
- Test: append to `src/harness/agent.rs` `#[cfg(test)] mod tests`

- [ ] **2.1 Add `max_iterations` field to `HarnessDeps`**

In `src/harness/deps.rs`, append after `system_prompt`:

```rust
    /// Hard iteration cap. When set, AgentHarness::run forces TurnState::Done
    /// after that many Continue turns and sets hit_limit=true. None → unbounded
    /// (current Gateway default).
    pub max_iterations: Option<usize>,
```

- [ ] **2.2 Update Gateway `HarnessDeps` literal**

In `src/orchestrator/harness_bridge.rs`, add to the literal after `system_prompt: None,`:

```rust
    max_iterations: None,
```

- [ ] **2.3 Confirm compile**

```bash
cargo check -p alephcore --lib 2>&1 | tail -5
```
Expected: zero errors.

- [ ] **2.4 Write the failing test for `max_iterations`**

Append to `#[cfg(test)] mod tests` in `src/harness/agent.rs`:

```rust
#[tokio::test]
async fn max_iterations_stops_runaway_loop() {
    use crate::providers::adapter::RequestPayload;
    use crate::providers::response::ProviderResponse;
    use crate::providers::adapter::NativeToolCall;
    use crate::providers::AiProvider;
    use crate::session::events::{MessageContent, SessionEvent, TurnTrigger, now_ms};
    use crate::routing::session_key::SessionKey;
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
    use std::sync::{Arc, Mutex};

    /// Provider that always returns a tool_call → forces TurnState::Continue forever.
    struct LoopingProvider { call_count: Arc<Mutex<usize>> }

    #[async_trait::async_trait]
    impl AiProvider for LoopingProvider {
        fn name(&self) -> &str { "looping" }
        async fn process(&self, _: RequestPayload<'_>) -> crate::error::Result<ProviderResponse> {
            *self.call_count.lock().unwrap() += 1;
            Ok(ProviderResponse {
                text: String::new(),
                tool_calls: vec![NativeToolCall {
                    id: format!("call-{}", self.call_count.lock().unwrap()),
                    name: "noop".into(),
                    arguments: serde_json::json!({}),
                }],
                ..ProviderResponse::text("")
            })
        }
        fn as_http_provider(&self) -> Option<&dyn crate::providers::HttpProvider> { None }
    }

    /// ToolService whose execute always succeeds (returns empty output) — the loop
    /// will keep calling 'noop' forever unless max_iterations kicks in.
    struct AlwaysOkTools;

    #[async_trait::async_trait]
    impl crate::tools::service::ToolService for AlwaysOkTools {
        async fn execute(&self, _: &str, _: serde_json::Value) -> Result<crate::session::events::ToolOutput, crate::tools::service::ToolError> {
            Ok(crate::session::events::ToolOutput {
                value: serde_json::json!({"ok": true}),
                metadata: Default::default(),
            })
        }
        async fn list(&self) -> Vec<crate::tools::service::ToolDefinition> { vec![] }
        async fn describe(&self, _: &str) -> Option<crate::tools::service::ToolDefinition> { None }
    }

    let call_count = Arc::new(Mutex::new(0usize));
    let provider = Arc::new(LoopingProvider { call_count: call_count.clone() });

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate_add_session_events(&conn).unwrap();
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    let session: Arc<dyn crate::session::service::SessionService> =
        Arc::new(InProcessActorSessionService::new(store));

    let sid = SessionKey::ephemeral("test-cap");
    session.attach(sid.clone()).await.unwrap();
    let turn = uuid::Uuid::new_v4();
    session.emit_event(&sid, SessionEvent::TurnStarted {
        turn_id: turn, trigger: TurnTrigger::UserMessage, at: now_ms(),
    }).await.unwrap();
    session.emit_event(&sid, SessionEvent::UserMessage {
        turn_id: turn, content: MessageContent { text: "go".into(), blocks: vec![] }, at: now_ms(),
    }).await.unwrap();

    let deps = crate::harness::deps::HarnessDeps {
        session: session.clone(),
        tools: Arc::new(AlwaysOkTools),
        sandbox: crate::sandbox::noop_sandbox(),
        llm: provider,
        stop_hooks: None,
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
        trace_sink: None,
        system_prompt: None,
        max_iterations: Some(3),
    };
    let harness = crate::harness::agent::AgentHarness::new(deps);
    let mut cb = crate::harness::callback::NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    harness.run(&sid, &mut cb, &cancel).await.expect("harness run");

    assert!(harness.hit_limit(), "hit_limit should be true after cap");
    // Exactly 3 Continue turns executed → provider.process called 3 times.
    assert_eq!(*call_count.lock().unwrap(), 3, "provider called 3 times before cap");
}

#[tokio::test]
async fn max_iterations_none_keeps_unbounded() {
    use crate::providers::adapter::{NativeToolCall, RequestPayload};
    use crate::providers::response::ProviderResponse;
    use crate::providers::AiProvider;
    use crate::session::events::{MessageContent, SessionEvent, TurnTrigger, now_ms};
    use crate::routing::session_key::SessionKey;
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
    use std::sync::{Arc, Mutex};

    /// Returns tool_call on the first 4 calls, then empty text on the 5th (Done).
    struct CountingProvider { n: Arc<Mutex<usize>> }

    #[async_trait::async_trait]
    impl AiProvider for CountingProvider {
        fn name(&self) -> &str { "counting" }
        async fn process(&self, _: RequestPayload<'_>) -> crate::error::Result<ProviderResponse> {
            let mut n = self.n.lock().unwrap();
            *n += 1;
            if *n <= 4 {
                Ok(ProviderResponse {
                    text: String::new(),
                    tool_calls: vec![NativeToolCall {
                        id: format!("c{}", *n),
                        name: "noop".into(),
                        arguments: serde_json::json!({}),
                    }],
                    ..ProviderResponse::text("")
                })
            } else {
                Ok(ProviderResponse::text("final"))
            }
        }
        fn as_http_provider(&self) -> Option<&dyn crate::providers::HttpProvider> { None }
    }

    let n = Arc::new(Mutex::new(0usize));
    let provider = Arc::new(CountingProvider { n: n.clone() });

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    migrate_add_session_events(&conn).unwrap();
    let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
    let session: Arc<dyn crate::session::service::SessionService> =
        Arc::new(InProcessActorSessionService::new(store));

    let sid = SessionKey::ephemeral("test-unbounded");
    session.attach(sid.clone()).await.unwrap();
    let turn = uuid::Uuid::new_v4();
    session.emit_event(&sid, SessionEvent::TurnStarted {
        turn_id: turn, trigger: TurnTrigger::UserMessage, at: now_ms(),
    }).await.unwrap();
    session.emit_event(&sid, SessionEvent::UserMessage {
        turn_id: turn, content: MessageContent { text: "go".into(), blocks: vec![] }, at: now_ms(),
    }).await.unwrap();

    let deps = crate::harness::deps::HarnessDeps {
        session: session.clone(),
        tools: Arc::new(AlwaysOkTools),
        sandbox: crate::sandbox::noop_sandbox(),
        llm: provider,
        stop_hooks: None,
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
        trace_sink: None,
        system_prompt: None,
        max_iterations: None, // unbounded
    };
    let harness = crate::harness::agent::AgentHarness::new(deps);
    let mut cb = crate::harness::callback::NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    harness.run(&sid, &mut cb, &cancel).await.expect("harness run");

    assert!(!harness.hit_limit(), "hit_limit must be false when max_iterations=None");
    // Provider called once per turn: 4 tool turns + 1 final → 5 total.
    assert_eq!(*n.lock().unwrap(), 5, "provider called 5 times total");
}
```

**Note:** If `NoopSandbox` / `noop_sandbox()` / `AlwaysOkTools` don't already exist elsewhere, build them inline in `#[cfg(test)]`. The pattern is identical to Task 1.

- [ ] **2.5 Run test to confirm it fails**

```bash
cargo test -p alephcore --lib max_iterations_stops_runaway_loop 2>&1 | tail -30
```
Expected: test hangs or fails — because the default `Harness::run` has no cap, the provider is called unboundedly (test will time out or we'll see `call_count` ≠ 3).

**If test hangs:** kill with Ctrl-C, and wrap the `harness.run` call in `tokio::time::timeout(Duration::from_secs(2), ...)` so the test fails fast rather than hanging. After implementation, timeout is no longer needed but harmless.

- [ ] **2.6 Override `Harness::run` on `AgentHarness`**

In `src/harness/agent.rs`, inside the `impl Harness for AgentHarness` block (currently has only `run_turn` at line 214-335), add a `run` method after `run_turn`:

```rust
    async fn run(
        &self,
        session_id: &SessionId,
        callback: &mut dyn HarnessCallback,
        cancel: &CancellationToken,
    ) -> Result<(), HarnessError> {
        let cap = self.deps.max_iterations;
        let mut iterations: usize = 0;
        loop {
            if cancel.is_cancelled() {
                return Err(HarnessError::Cancelled);
            }
            match self.run_turn(session_id, callback).await? {
                TurnState::Continue => {
                    iterations = iterations.saturating_add(1);
                    if let Some(limit) = cap {
                        if iterations >= limit {
                            self.hit_limit.store(true, Ordering::SeqCst);
                            callback.on_complete();
                            return Ok(());
                        }
                    }
                }
                TurnState::Done => {
                    callback.on_complete();
                    return Ok(());
                }
            }
        }
    }
```

- [ ] **2.7 Run test to confirm it passes**

```bash
cargo test -p alephcore --lib max_iterations_stops_runaway_loop 2>&1 | tail -10
```
Expected: `1 passed`.

- [ ] **2.8 Full suite + clippy**

```bash
cargo test -p alephcore --lib 2>&1 | tail -5
cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -5
```
Expected: `9136 passed; 2 failed` (baseline + T1 + T2 new tests); zero clippy errors.

- [ ] **2.9 Commit**

```bash
git add src/harness/deps.rs src/harness/agent.rs src/orchestrator/harness_bridge.rs
git commit -m "phase7: HarnessDeps.max_iterations + AgentHarness::run override

Counts Continue turns; when iterations >= cap, forces Done with
hit_limit=true. Gateway passes None (baseline unbounded). Tested via
LoopingProvider that would otherwise spin forever."
```

---

## Task 3: Relocate `LoopRunResult` to `agents::runtime`

**Files:**
- Modify: `src/agents/runtime.rs` (add local `LoopRunResult` definition)
- Modify: `src/agent_loop/loop_core.rs:296-307` (remove struct — but AgentLoop still uses it internally, so for THIS task we introduce an alias)
- Modify: `src/agents/mod.rs` (re-export)

**Strategy:** Define the struct in its new home (`agents::runtime`), then have the old location `pub use agents::runtime::LoopRunResult;` as a transitional alias. This lets any existing `crate::agent_loop::LoopRunResult` users keep compiling. The alias disappears when `agent_loop/` is deleted in Task 9.

- [ ] **3.1 Add `LoopRunResult` definition to `agents::runtime`**

In `src/agents/runtime.rs` near the top (after the `use` block, before `AgentRuntimeConfig`):

```rust
/// Outcome of a completed sub-agent run. Mirrors the legacy
/// `agent_loop::loop_core::LoopRunResult` field-for-field so that
/// `SubagentTool` and downstream consumers see zero behavior change.
#[derive(Debug, Clone)]
pub struct LoopRunResult {
    pub final_text: Option<String>,
    pub iterations: usize,
    pub tool_calls_made: usize,
    pub total_tokens: usize,
    pub hit_limit: bool,
    pub cancelled: bool,
    /// Chain ID shared across all depths in a subagent call chain.
    pub chain_id: String,
    /// Nesting depth (0 = root agent).
    pub depth: u32,
}
```

- [ ] **3.2 Delete the old `LoopRunResult` from `loop_core.rs` and re-export**

In `src/agent_loop/loop_core.rs`, replace lines 296-307 (the full struct definition) with:

```rust
// LoopRunResult has moved to `crate::agents::runtime`. This re-export is a
// transitional bridge; removed when `src/agent_loop/` is deleted in phase 7
// task 9.
pub use crate::agents::runtime::LoopRunResult;
```

- [ ] **3.3 Update the `agent_loop/mod.rs` re-export (line 113)**

In `src/agent_loop/mod.rs`, line 113 currently reads:

```rust
pub use loop_core::{AgentLoop, LoopCallback, LoopConfig, LoopProvider, LoopRunResult};
```

Change to:

```rust
pub use loop_core::{AgentLoop, LoopCallback, LoopConfig, LoopProvider};
pub use crate::agents::runtime::LoopRunResult;
```

- [ ] **3.4 Update `src/agents/runtime.rs` line 13 import**

The file currently has:

```rust
use crate::agent_loop::LoopRunResult;
```

Delete that line (now that `LoopRunResult` lives in the same module).

- [ ] **3.5 Compile-check**

```bash
cargo check -p alephcore --lib 2>&1 | tail -10
```
Expected: zero errors. If any module grep'd below needs tweaking:

```bash
grep -rn 'use crate::agent_loop::LoopRunResult\|use super::LoopRunResult\|agent_loop::loop_core::LoopRunResult' src/ --include='*.rs'
```
Adjust any direct references to use `crate::agents::runtime::LoopRunResult` (or leave them, since the re-export still works).

- [ ] **3.6 Run full suite**

```bash
cargo test -p alephcore --lib 2>&1 | tail -5
```
Expected: same as Task 2 (9136 passed + 2 failed) — this is a pure relocation, no behavior change.

- [ ] **3.7 Commit**

```bash
git add src/agents/runtime.rs src/agent_loop/loop_core.rs src/agent_loop/mod.rs
git commit -m "phase7: relocate LoopRunResult to agents::runtime

Canonical definition moves to agents::runtime so the new subagent
spawner (Task 5) can produce it without depending on agent_loop.
agent_loop::LoopRunResult becomes a transitional re-export, removed
when agent_loop/ is deleted in Task 9."
```

---

## Task 4: `AllowlistToolService` decorator

**Files:**
- Create: `src/agents/allowlist_tool_service.rs`
- Modify: `src/agents/mod.rs` (add `pub mod allowlist_tool_service;`)

### Design

- Holds `inner: Arc<dyn ToolService>` + `agent_def: Arc<AgentDef>`.
- `execute(name, input)`: if `!agent_def.is_tool_allowed(name)` → `Err(ToolError::PermissionDenied { name, reason: "agent disallowed" })`; else delegate.
- `list()`: call `inner.list().await`, filter to those with `agent_def.is_tool_allowed(&def.name)`.
- `describe(name)`: if `!agent_def.is_tool_allowed(name)` → `None`; else delegate.
- No HashSet materialization — `is_tool_allowed` handles `*` wildcard + denied_tools natively.

- [ ] **4.1 Write the failing tests in `src/agents/allowlist_tool_service.rs`**

Create a new file `src/agents/allowlist_tool_service.rs` with:

```rust
//! AllowlistToolService — filters a parent `ToolService` using `AgentDef::is_tool_allowed`.
//!
//! Used by the subagent spawner so that a sub-agent can only see / execute
//! the tools its AgentDef permits. Delegates all passing calls to the inner
//! service unchanged.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::agents::AgentDef;
use crate::session::events::ToolOutput;
use crate::tools::service::{ToolDefinition, ToolError, ToolService};

pub struct AllowlistToolService {
    inner: Arc<dyn ToolService>,
    agent_def: Arc<AgentDef>,
}

impl AllowlistToolService {
    pub fn new(inner: Arc<dyn ToolService>, agent_def: Arc<AgentDef>) -> Self {
        Self { inner, agent_def }
    }
}

#[async_trait]
impl ToolService for AllowlistToolService {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError> {
        if !self.agent_def.is_tool_allowed(name) {
            return Err(ToolError::PermissionDenied {
                name: name.to_string(),
                reason: format!("agent '{}' disallows this tool", self.agent_def.id),
            });
        }
        self.inner.execute(name, input).await
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        self.inner
            .list()
            .await
            .into_iter()
            .filter(|d| self.agent_def.is_tool_allowed(&d.name))
            .collect()
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        if !self.agent_def.is_tool_allowed(name) {
            return None;
        }
        self.inner.describe(name).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentDef, AgentMode};
    use crate::session::events::{ToolOutput, ToolOutputMetadata};
    use crate::tools::service::{ToolDefinition, ToolError, ToolService, ToolSource};
    use async_trait::async_trait;
    use serde_json::json;

    /// Test inner service with 3 fake tools: "read", "write", "exec".
    struct FakeTools;

    #[async_trait]
    impl ToolService for FakeTools {
        async fn execute(&self, name: &str, _: serde_json::Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: json!({"tool": name}),
                metadata: ToolOutputMetadata::default(),
            })
        }
        async fn list(&self) -> Vec<ToolDefinition> {
            ["read", "write", "exec"].iter()
                .map(|n| ToolDefinition {
                    name: (*n).into(),
                    description: "fake".into(),
                    input_schema: json!({}),
                    source: ToolSource::Builtin,
                    metadata: Default::default(),
                })
                .collect()
        }
        async fn describe(&self, name: &str) -> Option<ToolDefinition> {
            self.list().await.into_iter().find(|d| d.name == name)
        }
    }

    fn agent_with_allowed(tools: Vec<&str>) -> Arc<AgentDef> {
        let mut def = AgentDef::new("test", AgentMode::SubAgent);
        def.allowed_tools = tools.into_iter().map(String::from).collect();
        Arc::new(def)
    }

    #[tokio::test]
    async fn allowed_tool_executes_delegates_to_inner() {
        let def = agent_with_allowed(vec!["read"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        let out = svc.execute("read", json!({})).await.unwrap();
        assert_eq!(out.value, json!({"tool": "read"}));
    }

    #[tokio::test]
    async fn disallowed_tool_returns_permission_denied() {
        let def = agent_with_allowed(vec!["read"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        let err = svc.execute("exec", json!({})).await.unwrap_err();
        assert!(matches!(err, ToolError::PermissionDenied { .. }));
    }

    #[tokio::test]
    async fn empty_allowlist_denies_everything() {
        let def = agent_with_allowed(vec![]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        for name in ["read", "write", "exec"] {
            assert!(matches!(
                svc.execute(name, json!({})).await.unwrap_err(),
                ToolError::PermissionDenied { .. }
            ));
        }
    }

    #[tokio::test]
    async fn wildcard_allowlist_allows_everything() {
        let def = agent_with_allowed(vec!["*"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        for name in ["read", "write", "exec"] {
            assert!(svc.execute(name, json!({})).await.is_ok());
        }
    }

    #[tokio::test]
    async fn list_filters_to_allowed_subset() {
        let def = agent_with_allowed(vec!["read", "write"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        let list = svc.list().await;
        let names: Vec<_> = list.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["read", "write"]);
    }

    #[tokio::test]
    async fn describe_returns_none_for_disallowed() {
        let def = agent_with_allowed(vec!["read"]);
        let svc = AllowlistToolService::new(Arc::new(FakeTools), def);
        assert!(svc.describe("read").await.is_some());
        assert!(svc.describe("exec").await.is_none());
    }
}
```

- [ ] **4.2 Add module to `src/agents/mod.rs`**

Find the existing `pub mod` declarations in `src/agents/mod.rs` and add:

```rust
pub mod allowlist_tool_service;
```

- [ ] **4.3 Run tests — expect them to compile and pass immediately**

```bash
cargo test -p alephcore --lib agents::allowlist_tool_service:: 2>&1 | tail -15
```
Expected: 6 passed.

(Note: these are written in "test-only, implementation right next to it" style, so there's no separate red/green cycle — the implementation is authored before the tests compile. If you prefer strict TDD, comment out the impl body and confirm tests fail first.)

- [ ] **4.4 Verify `AgentDef::new(id, mode)` signature + `allowed_tools` field mutability**

```bash
grep -n 'pub fn new\|pub allowed_tools\|pub denied_tools\|pub fn is_tool_allowed' src/agents/types.rs
```
Confirm `AgentDef::new` takes `(id, AgentMode)` and `allowed_tools: Vec<String>` is public + mutable. If not, adjust the test helper `agent_with_allowed` accordingly.

- [ ] **4.5 Full suite + clippy**

```bash
cargo test -p alephcore --lib 2>&1 | tail -5
cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -5
```
Expected: `9142 passed; 2 failed` (+6 from this task); zero clippy errors.

- [ ] **4.6 Commit**

```bash
git add src/agents/allowlist_tool_service.rs src/agents/mod.rs
git commit -m "phase7: AllowlistToolService ToolService decorator

Wraps a parent ToolService and filters execute/list/describe by
AgentDef::is_tool_allowed. Used by the subagent spawner (Task 5)
to enforce per-agent tool scoping without mutating the shared
tool service."
```

---

## Task 5: `subagent_spawner` module + integration tests

**Files:**
- Create: `src/agents/subagent_spawner.rs`
- Modify: `src/agents/mod.rs` (add `pub mod subagent_spawner;`)

### Design Summary (from spec §5)

```
spawn(base: &SpawnerBase, req: SpawnRequest) -> Result<LoopRunResult, String>
  1. Resolve model: req.model.or(agent_def.model_hint)
  2. child SessionId: SessionKey::Ephemeral { agent_id, ephemeral_id = "subagent-<agent_id>-<nanos>" }
  3. system_prompt via PromptBuilder::new(PromptConfig::default()).with_agent(agent_def.clone()).build_system_prompt(&[])
  4. tools = Arc::new(AllowlistToolService::new(base.parent_tools.clone(), Arc::new(agent_def.clone())))
  5. HarnessDeps with max_iterations = Some(agent_def.max_iterations.unwrap_or(25) as usize), system_prompt = Some(built),
     stop_hooks/context_budget/... = None
  6. Seed UserMessage with task (+ context_summary prefix if present)
     Run inside AssertUnwindSafe(harness.run).catch_unwind() inside tokio::time::timeout
  7. Extract LoopRunResult from child session event log
```

- [ ] **5.1 Add module declaration to `src/agents/mod.rs`**

Add:
```rust
pub mod subagent_spawner;
```

- [ ] **5.2 Write the failing integration test (single-turn spawn)**

Create `src/agents/subagent_spawner.rs` with TESTS FIRST:

```rust
//! Subagent spawner — builds a child ephemeral session + Harness and runs it.
//!
//! Replaces the legacy `agent_loop::subagent_runner::run_subagent`.
//! The spawner is the only caller of `AgentHarness::new` outside Gateway's
//! `harness_bridge.rs`.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use tokio_util::sync::CancellationToken;

use crate::agents::allowlist_tool_service::AllowlistToolService;
use crate::agents::runtime::LoopRunResult;
use crate::agents::AgentDef;
use crate::harness::agent::AgentHarness;
use crate::harness::callback::NoopHarnessCallback;
use crate::harness::chain_context::ChainContext;
use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::Harness;
use crate::providers::AiProvider;
use crate::routing::session_key::SessionKey;
use crate::sandbox::Sandbox;
use crate::session::events::{now_ms, MessageContent, SessionEvent, TurnTrigger};
use crate::session::service::{SessionId, SessionService};
use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};
use crate::tools::service::ToolService;

pub struct SpawnerBase {
    pub session: Arc<dyn SessionService>,
    pub parent_tools: Arc<dyn ToolService>,
    pub sandbox: Arc<dyn Sandbox>,
    pub provider: Arc<dyn AiProvider>,
    pub chain: ChainContext,
}

pub struct SpawnRequest<'a> {
    pub agent_def: &'a AgentDef,
    pub task: &'a str,
    pub context_summary: Option<&'a str>,
    pub model: Option<&'a str>,
    pub timeout_secs: u64,
    pub cancel: CancellationToken,
}

pub async fn spawn(base: &SpawnerBase, req: SpawnRequest<'_>) -> Result<LoopRunResult, String> {
    // 1. Resolve model
    let resolved_model = req
        .model
        .map(|s| s.to_string())
        .or_else(|| req.agent_def.model_hint.clone());

    // 2. Build child session id
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let child_id = SessionKey::Ephemeral {
        agent_id: req.agent_def.id.clone(),
        ephemeral_id: format!("subagent-{}-{}", req.agent_def.id, nanos),
    };

    // 3. Build system prompt
    let system_prompt = PromptBuilder::new(PromptConfig::default())
        .with_agent(req.agent_def.clone())
        .build_system_prompt(&[]);

    // 4. Build filtered ToolService
    let tools: Arc<dyn ToolService> = Arc::new(AllowlistToolService::new(
        base.parent_tools.clone(),
        Arc::new(req.agent_def.clone()),
    ));

    // 4a. Wrap provider with model-override adapter if needed
    let llm: Arc<dyn AiProvider> = match resolved_model {
        Some(model) => Arc::new(ModelOverrideProvider { inner: base.provider.clone(), model }),
        None => base.provider.clone(),
    };

    // 5. Assemble HarnessDeps
    let deps = HarnessDeps {
        session: base.session.clone(),
        tools,
        sandbox: base.sandbox.clone(),
        llm,
        stop_hooks: None,
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
        trace_sink: None,
        system_prompt: Some(system_prompt),
        max_iterations: Some(req.agent_def.max_iterations.unwrap_or(25) as usize),
    };

    // 6. Seed task
    base.session
        .attach(child_id.clone())
        .await
        .map_err(|e| format!("attach child session: {e}"))?;

    let full_task = match req.context_summary {
        Some(summary) => format!(
            "## Context from parent agent\n\n{summary}\n\n---\n\n{}",
            req.task
        ),
        None => req.task.to_string(),
    };
    let turn = uuid::Uuid::new_v4();
    base.session
        .emit_event(
            &child_id,
            SessionEvent::TurnStarted {
                turn_id: turn,
                trigger: TurnTrigger::UserMessage,
                at: now_ms(),
            },
        )
        .await
        .map_err(|e| format!("seed TurnStarted: {e}"))?;
    base.session
        .emit_event(
            &child_id,
            SessionEvent::UserMessage {
                turn_id: turn,
                content: MessageContent {
                    text: full_task,
                    blocks: Vec::new(),
                },
                at: now_ms(),
            },
        )
        .await
        .map_err(|e| format!("seed UserMessage: {e}"))?;

    // 6a. Run harness with timeout + panic isolation
    let harness = AgentHarness::new(deps);
    let mut cb = NoopHarnessCallback;
    let run_future =
        AssertUnwindSafe(harness.run(&child_id, &mut cb, &req.cancel)).catch_unwind();
    let timed = tokio::time::timeout(Duration::from_secs(req.timeout_secs), run_future).await;

    let run_outcome = match timed {
        Err(_) => {
            return Err(format!(
                "sub-agent timed out after {}s",
                req.timeout_secs
            ))
        }
        Ok(Err(_panic)) => return Err("sub-agent panicked".to_string()),
        Ok(Ok(r)) => r,
    };
    if let Err(e) = run_outcome {
        return Err(format!("sub-agent failed: {e}"));
    }

    // 7. Extract LoopRunResult from event log
    extract_run_result(&*base.session, &child_id, &base.chain, harness.hit_limit())
        .await
}

/// Walks the child session event log and synthesizes a `LoopRunResult`.
async fn extract_run_result(
    session: &dyn SessionService,
    child_id: &SessionId,
    chain: &ChainContext,
    hit_limit: bool,
) -> Result<LoopRunResult, String> {
    let events = session
        .get_events(child_id, None, None)
        .await
        .map_err(|e| format!("read child session: {e}"))?;
    let mut final_text = String::new();
    let mut iterations = 0usize;
    let mut tool_calls_made = 0usize;
    for r in &events {
        match &r.event {
            SessionEvent::AssistantMessage { content, .. } => {
                final_text = content.text.clone();
                iterations += 1;
            }
            SessionEvent::ToolCallRequested { .. } => {
                tool_calls_made += 1;
            }
            _ => {}
        }
    }
    Ok(LoopRunResult {
        final_text: Some(final_text).filter(|s| !s.is_empty()),
        iterations,
        tool_calls_made,
        total_tokens: 0,
        hit_limit,
        cancelled: false, // Harness returning Ok means not cancelled; spawner level doesn't flag this
        chain_id: chain.chain_id.clone(),
        depth: chain.depth,
    })
}

/// Provider wrapper that stamps `payload.model` before delegating.
struct ModelOverrideProvider {
    inner: Arc<dyn AiProvider>,
    model: String,
}

#[async_trait::async_trait]
impl AiProvider for ModelOverrideProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }
    async fn process(
        &self,
        mut payload: crate::providers::adapter::RequestPayload<'_>,
    ) -> crate::error::Result<crate::providers::response::ProviderResponse> {
        payload.model = Some(self.model.clone());
        self.inner.process(payload).await
    }
    fn as_http_provider(&self) -> Option<&dyn crate::providers::HttpProvider> {
        // Streaming path deliberately bypassed; subagent uses non-streaming.
        None
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentDef, AgentMode};
    use crate::providers::adapter::{NativeToolCall, RequestPayload};
    use crate::providers::response::ProviderResponse;
    use crate::session::events::{ToolOutput, ToolOutputMetadata};
    use crate::session::in_process::InProcessActorSessionService;
    use crate::session::store::{migrate_add_session_events, SessionEventStore, SqliteEventStore};
    use crate::tools::service::{ToolDefinition, ToolError};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    // --- test helpers -------------------------------------------------------

    fn in_mem_session() -> Arc<dyn SessionService> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn SessionEventStore> = Arc::new(SqliteEventStore::new(conn));
        Arc::new(InProcessActorSessionService::new(store))
    }

    struct NoopTools;
    #[async_trait]
    impl ToolService for NoopTools {
        async fn execute(&self, name: &str, _: serde_json::Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: json!({"tool": name, "ok": true}),
                metadata: ToolOutputMetadata::default(),
            })
        }
        async fn list(&self) -> Vec<ToolDefinition> { vec![] }
        async fn describe(&self, _: &str) -> Option<ToolDefinition> { None }
    }

    /// Provider scripted with a sequence of responses. Each call pops the next.
    struct ScriptedProvider { queue: Mutex<Vec<ProviderResponse>> }
    #[async_trait]
    impl AiProvider for ScriptedProvider {
        fn name(&self) -> &str { "scripted" }
        async fn process(&self, _: RequestPayload<'_>) -> crate::error::Result<ProviderResponse> {
            let mut q = self.queue.lock().unwrap();
            if q.is_empty() {
                Ok(ProviderResponse::text("done"))
            } else {
                Ok(q.remove(0))
            }
        }
        fn as_http_provider(&self) -> Option<&dyn crate::providers::HttpProvider> { None }
    }

    fn make_agent_def() -> AgentDef {
        let mut def = AgentDef::new("spawner-test", AgentMode::SubAgent);
        def.allowed_tools = vec!["*".into()];
        def.max_iterations = Some(5);
        def
    }

    fn base_with(
        session: Arc<dyn SessionService>,
        provider: Arc<dyn AiProvider>,
    ) -> SpawnerBase {
        SpawnerBase {
            session,
            parent_tools: Arc::new(NoopTools),
            sandbox: crate::sandbox::noop_sandbox(),
            provider,
            chain: ChainContext::new(),
        }
    }

    fn req<'a>(def: &'a AgentDef, task: &'a str) -> SpawnRequest<'a> {
        SpawnRequest {
            agent_def: def,
            task,
            context_summary: None,
            model: None,
            timeout_secs: 30,
            cancel: CancellationToken::new(),
        }
    }

    // --- tests --------------------------------------------------------------

    #[tokio::test]
    async fn spawn_single_turn_returns_final_text() {
        let session = in_mem_session();
        let provider = Arc::new(ScriptedProvider {
            queue: Mutex::new(vec![ProviderResponse::text("hi from child")]),
        });
        let base = base_with(session, provider);
        let def = make_agent_def();
        let out = spawn(&base, req(&def, "say hi")).await.unwrap();
        assert_eq!(out.final_text.as_deref(), Some("hi from child"));
        assert_eq!(out.iterations, 1);
        assert_eq!(out.tool_calls_made, 0);
        assert!(!out.hit_limit);
        assert!(!out.cancelled);
        assert_eq!(out.depth, 0); // root chain
    }

    #[tokio::test]
    async fn spawn_multi_turn_counts_iterations_and_tool_calls() {
        let session = in_mem_session();
        let provider = Arc::new(ScriptedProvider {
            queue: Mutex::new(vec![
                // Turn 1: ask for a tool call
                ProviderResponse {
                    text: String::new(),
                    tool_calls: vec![NativeToolCall {
                        id: "c1".into(),
                        name: "noop".into(),
                        arguments: json!({}),
                    }],
                    ..ProviderResponse::text("")
                },
                // Turn 2: final reply
                ProviderResponse::text("all done"),
            ]),
        });
        let base = base_with(session, provider);
        let def = make_agent_def();
        let out = spawn(&base, req(&def, "do work")).await.unwrap();
        assert_eq!(out.iterations, 2);
        assert_eq!(out.tool_calls_made, 1);
        assert_eq!(out.final_text.as_deref(), Some("all done"));
    }

    #[tokio::test]
    async fn spawn_max_iter_sets_hit_limit() {
        let session = in_mem_session();
        // Always returns a tool call → never completes via Done; cap kicks in.
        struct AlwaysToolCall;
        #[async_trait]
        impl AiProvider for AlwaysToolCall {
            fn name(&self) -> &str { "always" }
            async fn process(&self, _: RequestPayload<'_>) -> crate::error::Result<ProviderResponse> {
                Ok(ProviderResponse {
                    text: String::new(),
                    tool_calls: vec![NativeToolCall {
                        id: "x".into(),
                        name: "noop".into(),
                        arguments: json!({}),
                    }],
                    ..ProviderResponse::text("")
                })
            }
            fn as_http_provider(&self) -> Option<&dyn crate::providers::HttpProvider> { None }
        }
        let provider = Arc::new(AlwaysToolCall);
        let base = base_with(session, provider);
        let mut def = make_agent_def();
        def.max_iterations = Some(3);
        let out = spawn(&base, req(&def, "spin")).await.unwrap();
        assert!(out.hit_limit);
        assert_eq!(out.iterations, 3);
    }

    #[tokio::test]
    async fn spawn_timeout_returns_timed_out_error() {
        let session = in_mem_session();
        /// Provider that sleeps longer than the timeout.
        struct SlowProvider;
        #[async_trait]
        impl AiProvider for SlowProvider {
            fn name(&self) -> &str { "slow" }
            async fn process(&self, _: RequestPayload<'_>) -> crate::error::Result<ProviderResponse> {
                tokio::time::sleep(Duration::from_secs(3)).await;
                Ok(ProviderResponse::text("late"))
            }
            fn as_http_provider(&self) -> Option<&dyn crate::providers::HttpProvider> { None }
        }
        let base = base_with(session, Arc::new(SlowProvider));
        let def = make_agent_def();
        let mut r = req(&def, "wait");
        r.timeout_secs = 1;
        let err = spawn(&base, r).await.unwrap_err();
        assert!(err.contains("timed out"));
    }

    #[tokio::test]
    async fn spawn_tool_allowlist_enforced_via_harness() {
        let session = in_mem_session();
        /// Returns a tool call to `forbidden_tool` once, then "done".
        struct ForbiddenCaller { first: Mutex<bool> }
        #[async_trait]
        impl AiProvider for ForbiddenCaller {
            fn name(&self) -> &str { "forbid" }
            async fn process(&self, _: RequestPayload<'_>) -> crate::error::Result<ProviderResponse> {
                let mut f = self.first.lock().unwrap();
                if *f {
                    *f = false;
                    Ok(ProviderResponse {
                        text: String::new(),
                        tool_calls: vec![NativeToolCall {
                            id: "x".into(),
                            name: "forbidden_tool".into(),
                            arguments: json!({}),
                        }],
                        ..ProviderResponse::text("")
                    })
                } else {
                    Ok(ProviderResponse::text("ok"))
                }
            }
            fn as_http_provider(&self) -> Option<&dyn crate::providers::HttpProvider> { None }
        }
        let base = base_with(session, Arc::new(ForbiddenCaller { first: Mutex::new(true) }));
        let mut def = make_agent_def();
        def.allowed_tools = vec!["noop".into()]; // forbidden_tool NOT allowed
        let err = spawn(&base, req(&def, "misbehave")).await.unwrap_err();
        assert!(
            err.contains("sub-agent failed") || err.contains("PermissionDenied") || err.contains("permission"),
            "err: {err}"
        );
    }
}
```

- [ ] **5.3 Run tests to confirm they all pass**

```bash
cargo test -p alephcore --lib agents::subagent_spawner:: 2>&1 | tail -20
```
Expected: 5 tests pass.

- [ ] **5.4 Full suite + clippy**

```bash
cargo test -p alephcore --lib 2>&1 | tail -5
cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -5
```
Expected: `9147 passed; 2 failed` (+5 from this task); zero clippy errors.

- [ ] **5.5 Commit**

```bash
git add src/agents/subagent_spawner.rs src/agents/mod.rs
git commit -m "phase7: subagent_spawner — Harness-based subagent execution

Replaces legacy agent_loop::subagent_runner. Builds a child ephemeral
SessionKey, seeds the task as a UserMessage, runs AgentHarness with
max_iterations + system_prompt + AllowlistToolService, then extracts
LoopRunResult from the event log.

Traffic hasn't flipped yet — AgentRuntime still calls run_subagent.
That swap happens in Task 6."
```

---

## Task 6: Flip `AgentRuntime::execute_fresh_path` → `execute_via_harness`

**Files:**
- Modify: `src/agents/runtime.rs`
- Modify: `src/agents/subagent_tool.rs` (ctor gains 3 new args)
- Modify: `src/tools/scoped.rs:368, 426` (update SubagentTool::new callers)
- Modify: `src/gateway/execution_engine/run_loop.rs:471` (update SubagentTool::new caller)
- Modify: `src/agents/subagent_tool.rs:784, 987` (update test helpers for SubagentTool::new)

### Strategy

The traffic flip requires propagating `session`, `parent_tools`, `sandbox` from boot wiring down to `AgentRuntime`. That means:

1. Add the 3 new fields to `AgentRuntime` + its `new()` signature.
2. Add 3 new fields to `SubagentTool` + its `new()` signature.
3. Update 5 `SubagentTool::new` callers (3 production + 2 tests).
4. In `SubagentTool::execute` where it constructs the `AgentRuntime`, pass the new fields through.
5. Replace `AgentRuntime::execute_fresh_path` body with a call to `subagent_spawner::spawn`.

- [ ] **6.1 Widen `AgentRuntime::new`**

In `src/agents/runtime.rs`, update the struct and constructor:

```rust
pub struct AgentRuntime {
    provider: Arc<dyn AiProvider>,
    tool_registry_factory: ToolRegistryFactory,
    safety_guard_factory: SafetyGuardFactory,
    child_chain: ChainContext,
    cancel_token: CancellationToken,
    session: Arc<dyn SessionService>,
    parent_tools: Arc<dyn ToolService>,
    sandbox: Arc<dyn Sandbox>,
}

impl AgentRuntime {
    pub fn new(
        provider: Arc<dyn AiProvider>,
        tool_registry_factory: ToolRegistryFactory,
        safety_guard_factory: SafetyGuardFactory,
        child_chain: ChainContext,
        cancel_token: CancellationToken,
        session: Arc<dyn SessionService>,
        parent_tools: Arc<dyn ToolService>,
        sandbox: Arc<dyn Sandbox>,
    ) -> Self {
        Self {
            provider,
            tool_registry_factory,
            safety_guard_factory,
            child_chain,
            cancel_token,
            session,
            parent_tools,
            sandbox,
        }
    }

    // ... run() unchanged ...

    async fn execute_via_harness(
        &self,
        config: &AgentRuntimeConfig,
    ) -> Result<LoopRunResult, String> {
        use crate::agents::subagent_spawner::{spawn, SpawnerBase, SpawnRequest};
        let base = SpawnerBase {
            session: self.session.clone(),
            parent_tools: self.parent_tools.clone(),
            sandbox: self.sandbox.clone(),
            provider: self.provider.clone(),
            chain: self.child_chain.clone(),
        };
        let req = SpawnRequest {
            agent_def: &config.agent_def,
            task: &config.task,
            context_summary: config.context_summary.as_deref(),
            model: config.model.as_deref(),
            timeout_secs: config.timeout_secs,
            cancel: self.cancel_token.clone(),
        };
        spawn(&base, req).await
    }
}
```

Also add the imports at the top of runtime.rs:

```rust
use crate::sandbox::Sandbox;
use crate::session::service::SessionService;
use crate::tools::service::ToolService;
```

- [ ] **6.2 Delete the old `execute_fresh_path`**

Remove the entire `async fn execute_fresh_path(...)` method body (roughly lines 204-224 in runtime.rs). Its only caller, `AgentRuntime::run`, gets redirected in the next step.

- [ ] **6.3 Redirect `AgentRuntime::run` to call the new method**

In `src/agents/runtime.rs` around line 146, replace:

```rust
let result = self.execute_fresh_path(&config).await;
```

with:

```rust
let result = self.execute_via_harness(&config).await;
```

- [ ] **6.4 Widen `SubagentTool::new`**

In `src/agents/subagent_tool.rs`, update struct + ctor:

```rust
pub struct SubagentTool {
    provider: Arc<dyn AiProvider>,
    tool_registry_factory: ToolRegistryFactory,
    safety_guard_factory: SafetyGuardFactory,
    chain: crate::harness::chain_context::ChainContext,
    agent_registry: Arc<AgentRegistry>,
    background_tracker: Arc<BackgroundAgentTracker>,
    teammate_manager: Option<Arc<TeammateManager>>,
    message_router: Option<Arc<MessageRouter>>,
    inbox: Option<Arc<Inbox>>,
    parent_agent_id: String,
    // NEW three fields:
    session: Arc<dyn crate::session::service::SessionService>,
    parent_tools: Arc<dyn crate::tools::service::ToolService>,
    sandbox: Arc<dyn crate::sandbox::Sandbox>,
    // REMOVED: shared_snapshot (fork path — cleared in Task 7)
}

impl SubagentTool {
    pub fn new(
        provider: Arc<dyn AiProvider>,
        tool_registry_factory: ToolRegistryFactory,
        safety_guard_factory: SafetyGuardFactory,
        chain: crate::harness::chain_context::ChainContext,
        agent_registry: Arc<AgentRegistry>,
        background_tracker: Arc<BackgroundAgentTracker>,
        session: Arc<dyn crate::session::service::SessionService>,
        parent_tools: Arc<dyn crate::tools::service::ToolService>,
        sandbox: Arc<dyn crate::sandbox::Sandbox>,
    ) -> Self {
        Self {
            provider,
            tool_registry_factory,
            safety_guard_factory,
            chain,
            agent_registry,
            background_tracker,
            teammate_manager: None,
            message_router: None,
            inbox: None,
            parent_agent_id: "primary".to_string(),
            session,
            parent_tools,
            sandbox,
            // shared_snapshot omitted — still present in struct for Task 7 deletion
        }
    }
    // ... other builder methods unchanged for this task ...
}
```

**Important:** In Task 6 we only ADD the new fields and ctor args. The `shared_snapshot` field and `with_shared_snapshot`/`should_fork`/`read_snapshot` methods are REMOVED in Task 7. Leaving them in place during Task 6 avoids a combinatorial compile break.

- [ ] **6.5 Update `AgentRuntimeConfig` construction inside `SubagentTool::execute`**

Inside `SubagentTool::execute` wherever it currently builds `AgentRuntime::new(...)`, pass the 3 new fields:

```rust
let runtime = AgentRuntime::new(
    self.provider.clone(),
    self.tool_registry_factory.clone(),
    self.safety_guard_factory.clone(),
    child_chain,
    cancel_token,
    self.session.clone(),
    self.parent_tools.clone(),
    self.sandbox.clone(),
);
```

Find the exact call sites via `grep -n 'AgentRuntime::new' src/agents/subagent_tool.rs`.

- [ ] **6.6 Update the 3 production `SubagentTool::new` callers**

**6.6a.** `src/tools/scoped.rs:368` — the block currently reads something like:

```rust
let st = Arc::new(crate::agents::subagent_tool::SubagentTool::new(
    provider.clone(),
    registry_factory.clone(),
    safety_factory.clone(),
    chain.clone(),
    agent_registry.clone(),
    bg_tracker.clone(),
));
```

Append the 3 new args. The caller context (around `tools/scoped.rs`) already holds `session`, `tool_service`, `sandbox` — find them in the surrounding scope via:

```bash
grep -n 'session: Arc\|tool_service: Arc\|sandbox: Arc\|dyn SessionService\|dyn ToolService\|dyn Sandbox' src/tools/scoped.rs | head -20
```

If they're not in scope, they must be threaded into the function. Add parameters to the enclosing function(s) as needed.

**6.6b.** `src/tools/scoped.rs:426` — same treatment.

**6.6c.** `src/gateway/execution_engine/run_loop.rs:471` — same treatment. The Gateway path has `session_service`, `tool_service`, and the per-request sandbox already in scope.

After editing, run:
```bash
cargo check -p alephcore --lib 2>&1 | tail -30
```

Expected: compile errors DROP to only those in `subagent_tool.rs`'s own test section (6.7). If there are compile errors outside these files, address them (missing fields threaded up).

- [ ] **6.7 Update the 2 test helpers in `subagent_tool.rs`**

`src/agents/subagent_tool.rs:784` (`make_tool` helper) and `:987` (the `let tool = SubagentTool::new(` inside a specific test). Add the 3 new fields using minimal stubs:

```rust
SubagentTool::new(
    provider,
    registry_factory,
    safety_factory,
    chain,
    agent_registry,
    bg_tracker,
    // NEW:
    {
        // in-memory session service (copy pattern from Task 5's in_mem_session)
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::session::store::migrate_add_session_events(&conn).unwrap();
        let store: Arc<dyn crate::session::store::SessionEventStore> =
            Arc::new(crate::session::store::SqliteEventStore::new(conn));
        Arc::new(crate::session::in_process::InProcessActorSessionService::new(store))
    },
    Arc::new(NoopTestToolService), // define inline struct stub in this test module
    crate::sandbox::noop_sandbox(),
)
```

Define `NoopTestToolService` inline at the top of the test module in `subagent_tool.rs`:

```rust
#[cfg(test)]
struct NoopTestToolService;

#[cfg(test)]
#[async_trait::async_trait]
impl crate::tools::service::ToolService for NoopTestToolService {
    async fn execute(&self, _: &str, _: serde_json::Value) -> Result<crate::session::events::ToolOutput, crate::tools::service::ToolError> {
        Err(crate::tools::service::ToolError::NotFound { name: "test".into() })
    }
    async fn list(&self) -> Vec<crate::tools::service::ToolDefinition> { vec![] }
    async fn describe(&self, _: &str) -> Option<crate::tools::service::ToolDefinition> { None }
}
```

- [ ] **6.8 Compile + full test suite**

```bash
cargo check -p alephcore --lib 2>&1 | tail -5
cargo test -p alephcore --lib 2>&1 | tail -5
cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -5
```

Expected:
- zero compile errors
- `9147 passed; 2 failed` (no new tests in this task, but no regressions)
- zero clippy errors

If SubagentTool tests regress (e.g., iteration count or tool_calls_made now differ from legacy AgentLoop path), the engineer must either: (a) confirm the new assertion holds via the Harness path and update the test, or (b) if it reveals a real behavior gap, STOP and flag for design review. Document any assertion changes in the commit message.

- [ ] **6.9 Commit**

```bash
git add src/agents/runtime.rs src/agents/subagent_tool.rs src/tools/scoped.rs src/gateway/execution_engine/run_loop.rs
git commit -m "phase7: flip AgentRuntime to Harness spawner

AgentRuntime now threads session/parent_tools/sandbox down to the new
subagent_spawner. SubagentTool::new widens by 3 args. Five call sites
updated (tools/scoped.rs x2, gateway run_loop.rs x1, subagent_tool.rs
test helpers x2).

This is the traffic-switch commit — subagents now run through the
Harness. Baseline (9147+2) holds."
```

---

## Task 7: Remove fork path from `SubagentTool` + `AgentRuntimeConfig`

**Files:**
- Modify: `src/agents/subagent_tool.rs`
- Modify: `src/agents/runtime.rs` (delete `prompt_snapshot` field + test assertion)

- [ ] **7.1 Delete `shared_snapshot` field from SubagentTool struct**

In `src/agents/subagent_tool.rs`, remove:
- line ~78: `shared_snapshot: Option<SharedSnapshot>,` from the struct.
- line ~109: `shared_snapshot: None,` from the ctor.
- lines ~138-141: the `with_shared_snapshot` method.
- lines ~147-159: the `should_fork` and `read_snapshot` methods.

- [ ] **7.2 Collapse the fork branches inside `execute`**

Find all usages via:

```bash
grep -n 'should_fork\|prompt_snapshot_clone\|read_snapshot\|shared_snapshot' src/agents/subagent_tool.rs
```

Expect matches around lines 623-625, 641-643, 680-681, 692. Replace each fork-branch with the "no-snapshot" direct path. For example, around line 623:

Before:
```rust
let should_fork_flag = self.should_fork(&args);
let prompt_snapshot_clone = if should_fork_flag {
    self.read_snapshot()
} else {
    None
};
// ... later:
let snapshot = if should_fork_flag { prompt_snapshot_clone } else { None };
// ... later:
AgentRuntimeConfig { prompt_snapshot: snapshot, ... }
```

After:
```rust
// Direct path — no fork. Subagent always starts fresh.
AgentRuntimeConfig { /* no prompt_snapshot field */, ... }
```

And around line 680:
```rust
let snapshot = if self.should_fork(&args) {
    self.read_snapshot()
} else {
    None
};
// ...
AgentRuntimeConfig { prompt_snapshot: snapshot, ... }
```

Becomes:
```rust
AgentRuntimeConfig { /* no prompt_snapshot field */, ... }
```

- [ ] **7.3 Remove `SharedSnapshot` import**

Delete line 19: `use crate::agent_loop::SharedSnapshot;`.

Also change line 66 (`chain: crate::agent_loop::chain_context::ChainContext,`) to reference the canonical path once the stub is cleaned up in Task 8. For this task, leave the stub path — Task 8 handles it.

- [ ] **7.4 Delete `prompt_snapshot` field from `AgentRuntimeConfig`**

In `src/agents/runtime.rs`, remove from the struct:

```rust
    pub prompt_snapshot: Option<PromptSnapshot>,  // DELETE
```

Also remove the `PromptSnapshot` use statement at the top of the file if now unused.

Update the test `agent_runtime_config_construction` at runtime.rs:325-340. Remove:

```rust
prompt_snapshot: None,
```

from the struct literal, and:

```rust
assert!(config.prompt_snapshot.is_none());
```

from the assertions.

- [ ] **7.5 Compile check**

```bash
cargo check -p alephcore --lib 2>&1 | tail -15
```

Expected: zero errors. If any caller of `AgentRuntimeConfig` still sets `prompt_snapshot`, grep and fix:

```bash
grep -rn 'prompt_snapshot:' src/ --include='*.rs'
```

- [ ] **7.6 Full test suite + clippy**

```bash
cargo test -p alephcore --lib 2>&1 | tail -5
cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -5
```
Expected: `9147 passed; 2 failed`; zero clippy errors. (If the `agent_runtime_config_construction` test count was 1 and we removed a field, the test count is unchanged since the test itself still exists — it just has fewer assertions.)

- [ ] **7.7 Commit**

```bash
git add src/agents/subagent_tool.rs src/agents/runtime.rs
git commit -m "phase7: delete fork path from SubagentTool

shared_snapshot / should_fork / read_snapshot / with_shared_snapshot
were dead code — zero production callers of with_shared_snapshot ever
supplied a Some(snapshot). All three fork branches in execute()
collapse to the direct path.

AgentRuntimeConfig.prompt_snapshot also removed."
```

---

## Task 8: Migrate 25 stub imports

**Files:**
- Any file under `src/` that imports `use crate::agent_loop::<stub>::...`

### Strategy

`src/agent_loop/mod.rs` contains 25 `pub mod X { pub use crate::canonical::path::*; }` stubs. Each maps to a canonical location. Task 9 deletes these stubs, so every external consumer must first be rewritten to use the canonical path directly.

- [ ] **8.1 List every stub usage**

```bash
grep -rn 'use crate::agent_loop::' src/ --include='*.rs' | grep -v 'src/agent_loop/' > /tmp/phase7_stub_uses.txt
wc -l /tmp/phase7_stub_uses.txt
cat /tmp/phase7_stub_uses.txt
```

Expected output: a list of `file:line: use crate::agent_loop::X::Y;` entries.

- [ ] **8.2 For each stub, rewrite to the canonical path**

The 25 stub → canonical mappings (from `src/agent_loop/mod.rs:20-91`):

| Stub path | Canonical path |
|-----------|----------------|
| `crate::agent_loop::adapters` | `crate::harness::adapters` |
| `crate::agent_loop::background_tracker` | `crate::agents::background_tracker` |
| `crate::agent_loop::chain_context` | `crate::harness::chain_context` |
| `crate::agent_loop::compaction` | `crate::memory::compaction` |
| `crate::agent_loop::context_budget` | `crate::harness::context_budget` |
| `crate::agent_loop::context_compactor` | `crate::harness::context_compactor` |
| `crate::agent_loop::exec_approval` | `crate::sandbox::exec_approval` |
| `crate::agent_loop::model_behaviors` | `crate::providers::model_behaviors` |
| `crate::agent_loop::provider_bridge` | `crate::harness::provider_bridge` |
| `crate::agent_loop::sections` | `crate::harness::sections` |
| `crate::agent_loop::skill_prefetch` | `crate::harness::skill_prefetch` |
| `crate::agent_loop::stop_hooks` | `crate::harness::stop_hooks` |
| `crate::agent_loop::subagent_teammates` | `crate::agents::teammates` |
| `crate::agent_loop::subagent_tool` | `crate::agents::subagent_tool` |
| `crate::agent_loop::tool` | `crate::tools::runtime` |
| `crate::agent_loop::tool_execution_context` | `crate::harness::tool_execution_context` |
| `crate::agent_loop::tool_info` | `crate::tools::info` |
| `crate::agent_loop::tool_orchestrator` | `crate::tools::orchestrator` |
| `crate::agent_loop::tool_pipeline` | `crate::tools::pipeline` |
| `crate::agent_loop::tool_refresh` | `crate::tools::refresh` |
| `crate::agent_loop::tool_result_store` | `crate::tools::result_store` |
| `crate::agent_loop::tool_summary` | `crate::harness::tool_summary` |
| `crate::agent_loop::trace` | `crate::harness::trace` |
| `crate::agent_loop::verify_stop_hook` | `crate::harness::verify_stop_hook` |

Plus the direct re-exports at `agent_loop/mod.rs:113-126`:

| Direct re-export | Canonical |
|------------------|-----------|
| `crate::agent_loop::SharedSnapshot` | (delete — fork path dead; any remaining consumers replace with `crate::thinker::prompt_builder::PromptSnapshot` wrapped in `Arc<RwLock<Option<_>>>`) |
| `crate::agent_loop::AgentLoop` | (delete — to be removed with loop_core) |
| `crate::agent_loop::LoopCallback` | (delete — to be removed with loop_core) |
| `crate::agent_loop::LoopConfig` | (delete — to be removed with loop_core) |
| `crate::agent_loop::LoopProvider` | `crate::harness::provider_bridge::LoopProvider` or delete if unused |
| `crate::agent_loop::LoopRunResult` | `crate::agents::runtime::LoopRunResult` |
| `crate::agent_loop::LoopTraceEvent` / etc. | `crate::harness::trace::LoopTraceEvent` / etc. |
| `crate::agent_loop::LoopTool` / `LoopToolRegistry` / etc. | `crate::tools::runtime::...` |
| `crate::agent_loop::ToolRefreshSource` | `crate::tools::refresh::ToolRefreshSource` |
| `crate::agent_loop::ToolInfo` | `crate::tools::info::ToolInfo` |
| `crate::agent_loop::RecoveryAction` / `RecoveryPhase` / `TruncationRecovery` | (delete — truncation_recovery is being deleted) |

Work through `/tmp/phase7_stub_uses.txt` line by line, rewriting each `use` statement to its canonical path. Batch commits by directory (e.g., commit all `src/thinker/*` import rewrites together).

After each directory, run:
```bash
cargo check -p alephcore --lib 2>&1 | tail -5
```

- [ ] **8.3 Verify zero residual stub imports**

```bash
grep -rn 'use crate::agent_loop::' src/ --include='*.rs' | grep -v 'src/agent_loop/'
```
Expected: empty output. If any lines remain, repeat 8.2 for those.

- [ ] **8.4 Also rewrite `SubagentTool` internal use (line 66)**

In `src/agents/subagent_tool.rs`, replace:

```rust
chain: crate::agent_loop::chain_context::ChainContext,
```

with:

```rust
chain: crate::harness::chain_context::ChainContext,
```

Plus the `fn new(chain: crate::agent_loop::chain_context::ChainContext, ...)` parameter signature.

- [ ] **8.5 Full suite + clippy**

```bash
cargo test -p alephcore --lib 2>&1 | tail -5
cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -5
```
Expected: `9147 passed; 2 failed`; zero clippy errors.

- [ ] **8.6 Commit**

```bash
git add src/ -A
git commit -m "phase7: migrate 25 agent_loop stub imports to canonical paths

Pre-deletion sweep: every 'use crate::agent_loop::X::Y' rewritten to
the canonical module path (e.g., crate::harness::chain_context,
crate::agents::background_tracker, crate::tools::runtime). Zero
residual imports — confirmed via grep.

Prepares Task 9 to delete src/agent_loop/ without collateral breakage."
```

---

## Task 9: Delete `src/agent_loop/` directory + `pub mod agent_loop;`

**Files:**
- Delete: `src/agent_loop/` (entire directory)
- Modify: `src/lib.rs`
- Modify: `src/agents/mod.rs` (remove `SharedSnapshot` re-export)

- [ ] **9.1 Confirm zero residual references**

```bash
grep -rn 'use crate::agent_loop\|crate::agent_loop::\|agent_loop::' src/ --include='*.rs' | grep -v 'src/agent_loop/' | grep -v '^[^:]*:[0-9]*://\|^[^:]*:[0-9]*: *//'
```

Expected: empty. If anything surfaces, fix it first by rewriting to canonical paths.

- [ ] **9.2 Remove `SharedSnapshot` re-export from `src/agents/mod.rs`**

Delete line 47-48 (the SharedSnapshot re-export block):

```rust
// SharedSnapshot is defined in agent_loop to avoid circular dependency
pub use crate::agent_loop::SharedSnapshot;
```

- [ ] **9.3 Delete the directory**

```bash
rm -rf src/agent_loop/
```

- [ ] **9.4 Remove module declaration from `src/lib.rs`**

In `src/lib.rs`, find and delete the line:

```rust
pub mod agent_loop;
```

- [ ] **9.5 Compile — expect some cleanup needed**

```bash
cargo check -p alephcore --lib 2>&1 | tail -40
```

Expected failures and their fixes:

| Error pattern | Fix |
|---------------|-----|
| `unresolved import crate::agent_loop` | grep the file, rewrite to canonical path (Task 8 should have covered these, but a stray one may remain) |
| `cannot find type 'SharedSnapshot' in this scope` | Delete dead uses; SharedSnapshot's only real consumer was SubagentTool's fork path, already deleted in Task 7 |
| `cannot find type 'AgentLoop'` / `LoopConfig` / `LoopProvider` | These were only used inside agent_loop itself — safe to delete any stragglers |

- [ ] **9.6 Full suite + clippy**

```bash
cargo test -p alephcore --lib 2>&1 | tail -5
cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -5
```

Expected:
- `cargo test` total passing MAY drop from 9147 if `loop_core.rs` had inline tests (verified: 0 tests in loop_core.rs + truncation_recovery.rs; safe)
- `9147 passed; 2 failed` should still hold
- zero clippy errors

- [ ] **9.7 Commit**

```bash
git add src/ -A
git commit -m "phase7: delete src/agent_loop/ directory

loop_core.rs (4558 LOC), truncation_recovery.rs (640 LOC),
subagent_runner.rs (90 LOC), mod.rs (127 LOC incl. 25 stub modules)
and the SharedSnapshot type alias all removed. pub mod agent_loop;
removed from src/lib.rs. agents::mod SharedSnapshot re-export removed.

Net LOC reduction ~5400 (deletions) − ~310 (Task 4+5 additions).
Baseline 9147+2 holds."
```

---

## Task 10: Observation-driven cleanup of factory fields

**Files:**
- Modify (possibly): `src/agents/runtime.rs`
- Modify (possibly): `src/agents/subagent_tool.rs`
- Modify (possibly): `src/tools/scoped.rs`, `src/gateway/execution_engine/run_loop.rs`

### Purpose

`tool_registry_factory` and `safety_guard_factory` are no longer consumed on the Harness path. This task decides whether to delete them outright or retain them for other consumers.

- [ ] **10.1 Find consumers of each factory**

```bash
grep -rn 'tool_registry_factory\|ToolRegistryFactory' src/ --include='*.rs' | head -30
grep -rn 'safety_guard_factory\|SafetyGuardFactory' src/ --include='*.rs' | head -30
```

- [ ] **10.2 Decision tree**

**Case A — both factories have zero consumers outside SubagentTool/AgentRuntime construction:**

- Delete both fields from `SubagentTool` and `AgentRuntime`.
- Delete both parameters from `SubagentTool::new` and `AgentRuntime::new`.
- Update the 5 SubagentTool::new callers (Task 6's list) to drop those args.
- Delete the `ToolRegistryFactory` / `SafetyGuardFactory` type aliases if now unused (check with `grep -rn 'type ToolRegistryFactory\|type SafetyGuardFactory' src/`).

**Case B — something else still consumes the factories:**

- Retain the fields. Add `#[allow(dead_code)]` above each field declaration with a comment: `// Retained for <consumer>; Harness path does not use. TODO: clean up when <consumer> migrates.`

**Case C — partial (one is used, the other is dead):**

- Apply Case A to the dead one, Case B to the alive one.

Document the decision in the commit message.

- [ ] **10.3 Compile + test + clippy**

```bash
cargo check -p alephcore --lib 2>&1 | tail -5
cargo test -p alephcore --lib 2>&1 | tail -5
cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -5
```

Expected: `9147 passed; 2 failed`; zero clippy errors.

- [ ] **10.4 Commit**

```bash
git add -A
git commit -m "phase7: remove dead factory fields from SubagentTool/AgentRuntime

[OR: retain with #[allow(dead_code)] — see case B]

Decision based on grep: <list consumers found or confirm zero>.
<describe which fields/ctor args got deleted and why>"
```

---

## Task 11: Exit-gate script + final verification

**Files:**
- Create: `scripts/check-phase7-exit.sh`

- [ ] **11.1 Create the exit-gate script**

Write `scripts/check-phase7-exit.sh`:

```sh
#!/usr/bin/env bash
# Phase 7 exit gate: verifies agent_loop/ is gone and baseline holds.

set -euo pipefail

# 1. agent_loop directory must be gone
if [ -d "src/agent_loop" ]; then
    echo "FAIL: src/agent_loop/ still exists"
    exit 1
fi

# 2. AgentLoop / LoopConfig symbols zero usage outside agents/runtime.rs
BAD=$(grep -rn -E '\b(AgentLoop|LoopConfig)\b' src/ --include='*.rs' \
    | grep -v 'src/agents/runtime.rs' || true)
if [ -n "$BAD" ]; then
    echo "FAIL: residual AgentLoop / LoopConfig references:"
    echo "$BAD"
    exit 1
fi

# 3. pub mod agent_loop must be removed from lib.rs
if grep -q 'pub mod agent_loop;' src/lib.rs; then
    echo "FAIL: pub mod agent_loop; still present in src/lib.rs"
    exit 1
fi

# 4. Baseline test count holds
OUT=$(cargo test -p alephcore --lib 2>&1 || true)
PASS=$(echo "$OUT" | awk '/test result:/ {for (i=1;i<=NF;i++) if ($i=="passed;") print $(i-1)}' | tail -n1)
FAIL=$(echo "$OUT" | awk '/test result:/ {for (i=1;i<=NF;i++) if ($i=="failed;") print $(i-1)}' | tail -n1)

if [ -z "${PASS:-}" ] || [ "$PASS" -lt 9133 ]; then
    echo "FAIL: passing count ${PASS:-unknown} < 9133 baseline"
    echo "$OUT" | tail -40
    exit 1
fi
if [ -z "${FAIL:-}" ] || [ "$FAIL" -gt 2 ]; then
    echo "FAIL: failing count ${FAIL:-unknown} > 2 (baseline)"
    echo "$OUT" | tail -40
    exit 1
fi

echo "OK: phase7 exit gate passed ($PASS passing, $FAIL failing)"
```

Make it executable:
```bash
chmod +x scripts/check-phase7-exit.sh
```

- [ ] **11.2 Run the exit gate**

```bash
./scripts/check-phase7-exit.sh
```

Expected output:
```
OK: phase7 exit gate passed (9147 passing, 2 failing)
```

If it fails, address the reported issue, commit the fix, and re-run.

- [ ] **11.3 Commit the exit gate**

```bash
git add scripts/check-phase7-exit.sh
git commit -m "phase7: add exit-gate script

Asserts: src/agent_loop/ gone, AgentLoop/LoopConfig symbols absent
outside agents::runtime, pub mod agent_loop removed from lib.rs,
baseline >= 9133 passing and exactly 2 (or fewer) failing."
```

- [ ] **11.4 Final git log summary**

```bash
git log --oneline main..HEAD
```

Expected: 11 commits, all with `phase7:` prefix.

- [ ] **11.5 Stop here and ask the user**

**DO NOT push, rebase, merge, create a PR, or run `just release`.** Phase 7 is a local refactor. Report:

```
Phase 7 complete on worktree-managed-agents-phase-7.

Commits: 11 (see git log main..HEAD)
LOC delta: -5,400 deleted, +310 added, net -5,090
Tests: 9147 passing, 2 failing (baseline preserved)
Clippy: zero errors
Exit gate: green

Ready for user review. Nothing pushed, nothing merged.
```

Wait for the user's next instruction.

---

## Definition of Done

- [ ] All 11 tasks merged onto `worktree-managed-agents-phase-7` in order.
- [ ] `scripts/check-phase7-exit.sh` exits 0.
- [ ] `cargo test -p alephcore --lib`: ≥ 9133 passing, exactly 2 failing (by name).
- [ ] `cargo clippy -p alephcore --lib -- -D warnings`: zero errors.
- [ ] `grep -rn 'use crate::agent_loop::' src/` returns nothing.
- [ ] `ls src/agent_loop/` fails with "No such file".
- [ ] Net LOC reduction ≥ 5,000.
- [ ] User has reviewed and approved final state.
- [ ] No release, no push, no PR.
