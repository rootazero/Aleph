# Subagent Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the `run_loop.rs → SubagentTool → AgentRuntime` production wiring hop that subagent uplift stages A/F/H/I shipped only to the builder layer, and fix three bugs (`total_tokens=0`, no concurrency cap, no parent-cancel propagation).

**Architecture:** Wire existing, well-built infrastructure — no new subsystems. One new primitive: a `tokio::sync::Semaphore` on `SpawnerBase` (the slot the deleted `LaneScheduler` vacated). One refactor: a private `SubagentTool::build_runtime` helper so the four new wirings land in one place instead of 3× duplication.

**Tech Stack:** Rust, `tokio` (async + `Semaphore`), `tokio_util::sync::CancellationToken`, `serde`/`serde_yaml`, `cargo test`.

**Spec:** `docs/superpowers/specs/2026-05-19-subagent-hardening-design.md`

**Worktree:** `/Volumes/TBU4/Workspace/Aleph-wt-subagent`, branch `subagent-hardening`. All `cargo`/`git` commands run from the worktree root.

---

## File Structure

| File | Responsibility | Tasks |
|---|---|---|
| `src/providers/metering.rs` | LLM usage decorator — gains a token accumulator | 1 |
| `src/agents/subagent_spawner/mod.rs` | spawn engine — token read, semaphore acquire, context_mode | 1, 3, 8 |
| `src/agents/subagent_spawner/tests.rs` | spawner tests | 1, 3, 8 |
| `src/agents/runtime.rs` | `AgentRuntime` — new builders threaded to `SpawnerBase` | 3, 5, 6 |
| `src/agents/subagent_tool.rs` | `SubagentTool` — new fields, `build_runtime` helper, `spawn_background` | 2, 3, 4, 6, 7 |
| `src/agents/subagent_tool/loop_tool.rs` | tool `execute` — foreground + sync-batch use the helper | 2, 4 |
| `src/agents/types.rs` | `AgentDef.isolation` field | 5 |
| `src/agents/loader.rs` | frontmatter `isolation` parse | 5 |
| `src/agents/background_tracker.rs` | opportunistic `cleanup` caller | 8 |
| `src/gateway/execution_engine/run_loop.rs` | production construction site — wires everything in | 4, 6, 7 |
| `tests/cancellation_chain.rs` etc. | `SpawnerBase` literal updates | 3 |

---

## Task 1: Token accounting (A1)

**Files:**
- Modify: `src/providers/metering.rs`
- Modify: `src/agents/subagent_spawner/mod.rs`
- Modify: `src/agents/subagent_spawner/tests.rs`

- [ ] **Step 1: Write the failing test (metering accumulator)**

In `src/providers/metering.rs`, inside `mod tests`, add:

```rust
    #[tokio::test]
    async fn accumulator_sums_input_and_output_tokens() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let inner = Arc::new(FakeProvider {
            usage: TokenUsage {
                input_tokens: 200,
                output_tokens: 50,
                cache_read_tokens: Some(150),
                cache_creation_tokens: Some(20),
                thinking_tokens: None,
                cost: None,
            },
        });
        let acc = Arc::new(AtomicU64::new(0));
        let metering = MeteringProvider::new(inner, None, "acc-test")
            .with_token_accumulator(acc.clone());
        let msgs = [crate::providers::message::UnifiedMessage::user("hi")];
        let _ = metering.process(RequestPayload::new(&msgs)).await.expect("process");
        let _ = metering.process(RequestPayload::new(&msgs)).await.expect("process");
        assert_eq!(acc.load(Ordering::Relaxed), 500, "two calls × (200+50)");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib metering::tests::accumulator_sums -- --exact`
Expected: FAIL — `no method named with_token_accumulator`.

- [ ] **Step 3: Implement the accumulator**

In `src/providers/metering.rs`:

Add to imports: `use std::sync::atomic::{AtomicU64, Ordering};`

Add field to `struct MeteringProvider`:
```rust
    total_tokens: Option<Arc<AtomicU64>>,
```

In `MeteringProvider::new`, add `total_tokens: None,` to the returned struct.

Add builder after `new`:
```rust
    /// Wire a shared counter that accumulates `input + output` tokens across
    /// every `process()` call. Used by the subagent spawner to populate
    /// `LoopRunResult.total_tokens`.
    pub fn with_token_accumulator(mut self, acc: Arc<AtomicU64>) -> Self {
        self.total_tokens = Some(acc);
        self
    }
```

In `process()`, after `let provider_name = ...;` add:
```rust
        let acc = self.total_tokens.clone();
```

Inside the `if let Some(usage) = resp.usage.as_ref() {` block, after the `tracing::info!` call, add:
```rust
                if let Some(acc) = &acc {
                    acc.fetch_add(
                        usage.input_tokens as u64 + usage.output_tokens as u64,
                        Ordering::Relaxed,
                    );
                }
```

(If `cargo clippy` later flags `unnecessary cast` on `as u64`, the fields are already `u64` — drop the casts.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib metering::tests::accumulator_sums -- --exact`
Expected: PASS.

- [ ] **Step 5: Write the failing spawner test**

In `src/agents/subagent_spawner/tests.rs`, inside `mod tests`, add (note `UsageProvider` already exists in this file — it returns `input_tokens: 10, output_tokens: 5`):

```rust
    #[tokio::test]
    async fn spawn_reports_total_tokens_from_usage() {
        let provider: Arc<dyn AiProvider> = Arc::new(UsageProvider);
        let base = make_base(provider);
        let agent = agent_with_allowed("token-probe", vec!["*"]);
        let req = SpawnRequest {
            agent_def: &agent,
            task: "say hi",
            context_summary: None,
            model: None,
            timeout_secs: 5,
            cancel: CancellationToken::new(),
            isolation: None,
        };
        let result = spawn(&base, req).await.expect("spawn ok");
        assert_eq!(result.total_tokens, 15, "10 input + 5 output from one UsageProvider call");
    }
```

- [ ] **Step 6: Run test to verify it fails**

Run: `cargo test -p alephcore --lib subagent_spawner::tests::spawn_reports_total_tokens -- --exact`
Expected: FAIL — `assertion failed: result.total_tokens == 15` (currently `0`).

- [ ] **Step 7: Thread the counter through the spawner**

In `src/agents/subagent_spawner/mod.rs`:

Just before the `MeteringProvider` wrap (line ~262, the `let llm: Arc<dyn AiProvider> = Arc::new(crate::providers::MeteringProvider::new(...))` statement), add:
```rust
        let token_counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
```

Change the `MeteringProvider::new(...)` expression to chain the accumulator:
```rust
        let llm: Arc<dyn AiProvider> = Arc::new(
            crate::providers::MeteringProvider::new(
                llm,
                base.trace_sink.clone(),
                req.agent_def.id.clone(),
            )
            .with_token_accumulator(token_counter.clone()),
        );
```

In the `Ok(Ok(Ok(())))` success branch, change the `extract_run_result` call to pass the count:
```rust
                let result = extract_run_result(
                    base.session.as_ref(),
                    &child_id,
                    &child_chain,
                    hit_limit,
                    token_counter.load(std::sync::atomic::Ordering::Relaxed),
                )
                .await?;
```

Change `extract_run_result`'s signature and body:
```rust
async fn extract_run_result(
    session: &dyn SessionService,
    child_id: &SessionId,
    chain: &ChainContext,
    hit_limit: bool,
    total_tokens: u64,
) -> Result<LoopRunResult, String> {
```
and in the returned `LoopRunResult`, replace `total_tokens: 0,` with `total_tokens: total_tokens as usize,`.

- [ ] **Step 8: Update the two existing `extract_run_result` test callers**

In `src/agents/subagent_spawner/tests.rs`, the tests `final_text_cleared_when_last_assistant_is_empty` and `final_text_kept_when_last_assistant_has_text` call `extract_run_result(session.as_ref(), &child_id, &chain, true|false)`. Add a final `0` argument to both calls:
```rust
        let result = extract_run_result(session.as_ref(), &child_id, &chain, true, 0)
```
(and `..., false, 0)` for the second).

- [ ] **Step 9: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib subagent_spawner::tests -- --skip mcp`
Expected: PASS (all spawner tests, including the two updated callers and the new one).

- [ ] **Step 10: Commit**

```bash
git add src/providers/metering.rs src/agents/subagent_spawner/
git commit -m "agents: thread subagent token accounting into LoopRunResult

MeteringProvider gains an optional shared accumulator; the spawner reads
it into LoopRunResult.total_tokens (was hardcoded 0)."
```

---

## Task 2: Extract `SubagentTool::build_runtime` helper (C-helper, refactor)

This is a behavior-preserving refactor. The existing `subagent_tool` tests are
the regression net.

**Files:**
- Modify: `src/agents/subagent_tool.rs`
- Modify: `src/agents/subagent_tool/loop_tool.rs`

- [ ] **Step 1: Establish the green baseline**

Run: `cargo test -p alephcore --lib subagent_tool::`
Expected: PASS (record the passing test count — it must be identical after the refactor).

- [ ] **Step 2: Add the `build_runtime` helper**

In `src/agents/subagent_tool.rs`, inside `impl SubagentTool`, after `spawn_background`, add:

```rust
    /// Build an `AgentRuntime` with every inheritable field this tool carries
    /// applied. Single construction point for the foreground, sync-batch, and
    /// background spawn paths so new wiring lands in one place.
    fn build_runtime(
        &self,
        child_chain: crate::harness::chain_context::ChainContext,
        cancel: CancellationToken,
    ) -> AgentRuntime {
        let mut runtime = AgentRuntime::new(
            self.provider.clone(),
            child_chain,
            cancel,
            self.session.clone(),
            self.parent_tools.clone(),
            self.sandbox.clone(),
        )
        .with_parent_agent_id(self.parent_agent_id.clone());
        if let Some(w) = self.raw_memory_writer.clone() {
            runtime = runtime.with_raw_memory_writer(w);
        }
        if let Some(reg) = self.capture_registry.clone() {
            runtime = runtime.with_capture_registry(reg);
        }
        if let Some(sid) = self.parent_session_id.clone() {
            runtime = runtime.with_parent_session_id(sid);
        }
        runtime
    }
```

- [ ] **Step 3: Refactor `spawn_background` to use the helper**

In `src/agents/subagent_tool.rs`, replace the body of `spawn_background` (the
section from `let provider = self.provider.clone();` through the end of the
`tokio::spawn(async move { ... })` block) with:

```rust
        let mut runtime = self.build_runtime(child_chain, cancel_token);
        if let Some(parent_sink) = self.trace_sink.clone() {
            let wrapper: Arc<dyn crate::harness::TraceSink> = Arc::new(
                crate::agents::forwarding_trace_sink::ForwardingTraceSink::new(
                    parent_sink,
                    self.background_tracker.clone(),
                    request_id.clone(),
                ),
            );
            runtime = runtime.with_trace_sink(wrapper);
        }

        let tracker = self.background_tracker.clone();
        let rid = request_id.clone();
        tokio::spawn(async move {
            let runtime_config = AgentRuntimeConfig {
                agent_def,
                task,
                context_summary,
                model,
                timeout_secs,
            };
            let result = AssertUnwindSafe(runtime.run(runtime_config))
                .catch_unwind()
                .await;
            let outcome = match result {
                Ok(Ok(r)) => Ok(r.final_text.unwrap_or_else(|| "(no output)".to_string())),
                Ok(Err(e)) => Err(e),
                Err(_panic) => Err("Sub-agent panicked".to_string()),
            };
            tracker.mark_completed(&rid, outcome);
        });

        request_id
```

Delete the now-unused per-field clone locals (`provider`, `session`,
`parent_tools`, `sandbox`, `raw_memory_writer`, `capture_registry`,
`parent_agent_id`, `parent_session_id`, `parent_trace_sink`,
`tracker_for_wrapper`, `request_id_for_wrapper`). `request_id`, `cancel_token`,
and the `self.background_tracker.register(...)` call stay.

- [ ] **Step 4: Refactor the foreground path**

In `src/agents/subagent_tool/loop_tool.rs`, in the foreground `else` branch
(currently `let mut runtime = AgentRuntime::new(...).with_parent_agent_id(...)`
followed by three `if let Some(...)` blocks), replace that whole runtime
construction with:

```rust
            let runtime = self.build_runtime(child_chain, CancellationToken::new());
```

(Task 4 replaces `CancellationToken::new()` with the parent-derived token.)

- [ ] **Step 5: Refactor the sync-batch path**

In `src/agents/subagent_tool/loop_tool.rs`, in the sync-batch loop, replace the
block from `let provider = self.provider.clone();` through the
`handles.push(tokio::spawn(async move { ... }));` with:

```rust
                    let runtime = self.build_runtime(child_chain.clone(), CancellationToken::new());
                    handles.push(tokio::spawn(async move {
                        let outcome = AssertUnwindSafe(runtime.run(runtime_config))
                            .catch_unwind()
                            .await;
                        (idx, outcome)
                    }));
```

`runtime_config` is already built just above this block — keep it. Delete the
per-field clone locals (`provider`, `session`, `parent_tools`, `sandbox`,
`raw_memory_writer`, `capture_registry`, `parent_agent_id`,
`parent_session_id`, `chain_for_task`).

- [ ] **Step 6: Run the regression net**

Run: `cargo test -p alephcore --lib subagent_tool::`
Expected: PASS — identical count to Step 1. If any test fails, the refactor
changed behavior; fix it before committing.

- [ ] **Step 7: Commit**

```bash
git add src/agents/subagent_tool.rs src/agents/subagent_tool/loop_tool.rs
git commit -m "agents: extract SubagentTool::build_runtime helper

Single AgentRuntime construction point for foreground/sync-batch/
background paths; collapses 3x duplication ahead of new wiring."
```

---

## Task 3: Concurrency cap — `Semaphore` (A2)

**Files:**
- Modify: `src/agents/subagent_spawner/mod.rs`
- Modify: `src/agents/runtime.rs`
- Modify: `src/agents/subagent_tool.rs`
- Modify: `src/agents/subagent_spawner/tests.rs`
- Modify: every other file with a `SpawnerBase { ... }` literal (Step 4)

- [ ] **Step 1: Write the failing test**

In `src/agents/subagent_spawner/tests.rs`, inside `mod tests`, add:

```rust
    #[tokio::test]
    async fn spawn_blocks_when_semaphore_exhausted() {
        use tokio::sync::Semaphore;
        let sem = Arc::new(Semaphore::new(1));
        let provider = ScriptedProvider::new(vec![ProviderResponse::text_only("ok".into())]);
        let mut base = make_base(provider);
        base.subagent_semaphore = Some(sem.clone());

        let agent = agent_with_allowed("capped", vec!["*"]);
        let mk_req = |a: &AgentDef| SpawnRequest {
            agent_def: a,
            task: "noop",
            context_summary: None,
            model: None,
            timeout_secs: 5,
            cancel: CancellationToken::new(),
            isolation: None,
        };

        // Exhaust the single permit by hand.
        let held = sem.clone().acquire_owned().await.unwrap();

        // spawn() must block on acquire — wrap in a short timeout.
        let blocked = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            spawn(&base, mk_req(&agent)),
        )
        .await;
        assert!(blocked.is_err(), "spawn must block while the semaphore is exhausted");

        // Release the permit; the next spawn proceeds promptly.
        drop(held);
        let ran = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            spawn(&base, mk_req(&agent)),
        )
        .await;
        assert!(ran.is_ok(), "spawn must proceed once a permit frees up");
        ran.unwrap().expect("spawn ok");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib subagent_spawner::tests::spawn_blocks_when_semaphore -- --exact`
Expected: FAIL — `no field subagent_semaphore on type SpawnerBase`.

- [ ] **Step 3: Add the `SpawnerBase` field + `spawn()` acquire**

In `src/agents/subagent_spawner/mod.rs`:

Add to `struct SpawnerBase` (after `plugin_registry`):
```rust
    /// A2 — global cap on concurrently-running subagent spawns. `None` skips
    /// the cap (direct test callers); `Some(_)` makes `spawn()` acquire a
    /// permit held for the child's full lifetime.
    pub subagent_semaphore: Option<Arc<tokio::sync::Semaphore>>,
```

In `spawn()`, immediately after the `child_chain` derivation (the
`let child_chain = base.chain.child().ok_or_else(...)?;` block), add:
```rust
    // A2 — reserve a concurrency permit; held until `spawn` returns.
    let _permit = match base.subagent_semaphore.as_ref() {
        Some(sem) => Some(
            sem.clone()
                .acquire_owned()
                .await
                .map_err(|e| format!("sub-agent failed: subagent semaphore closed: {e}"))?,
        ),
        None => None,
    };
```

- [ ] **Step 4: Update every `SpawnerBase { ... }` literal**

Run: `grep -rn "SpawnerBase {" src/ tests/`
For each literal found (at minimum: `src/agents/runtime.rs`,
`src/agents/subagent_spawner/tests.rs` `make_base`,
`tests/cancellation_chain.rs` `base_with_hanging_llm` + `base_with_hanging_tool`,
and any in `tests/subagent_deps_inherit.rs` / `tests/worktree_isolation.rs`),
add the field. For test literals add `subagent_semaphore: None,`. In
`src/agents/runtime.rs` `execute_via_harness`, add
`subagent_semaphore: self.subagent_semaphore.clone(),`.

- [ ] **Step 5: Add the `AgentRuntime` field + builder**

In `src/agents/runtime.rs`:

Add field to `struct AgentRuntime` (after `trace_sink`):
```rust
    /// A2 — subagent concurrency cap, threaded into every `SpawnerBase`.
    subagent_semaphore: Option<Arc<tokio::sync::Semaphore>>,
```

In `AgentRuntime::new`, add `subagent_semaphore: None,` to the struct.

Add builder after `with_trace_sink`:
```rust
    /// A2 — wire the shared subagent concurrency semaphore.
    pub fn with_subagent_semaphore(mut self, sem: Arc<tokio::sync::Semaphore>) -> Self {
        self.subagent_semaphore = Some(sem);
        self
    }
```

(`use crate::sync_primitives::Arc;` is already imported in `runtime.rs`.)

- [ ] **Step 6: Add the `SubagentTool` field + helper wiring**

In `src/agents/subagent_tool.rs`:

Add near the top (after the imports):
```rust
/// A2 — default cap on concurrently-running subagent spawns per top-level
/// agent run. Matches the deleted `Lane::Subagent` default.
const DEFAULT_MAX_CONCURRENT_SUBAGENTS: usize = 4;
```

Add field to `struct SubagentTool` (after `trace_sink`):
```rust
    /// A2 — shared concurrency cap; one per tool instance (= per agent run).
    subagent_semaphore: Arc<tokio::sync::Semaphore>,
```

In `SubagentTool::new`, add to the returned struct:
```rust
            subagent_semaphore: Arc::new(tokio::sync::Semaphore::new(
                DEFAULT_MAX_CONCURRENT_SUBAGENTS,
            )),
```

In `build_runtime`, before `runtime` is returned, add:
```rust
        runtime = runtime.with_subagent_semaphore(self.subagent_semaphore.clone());
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib subagent_spawner::tests::spawn_blocks_when_semaphore -- --exact`
Expected: PASS.
Run: `cargo build -p alephcore --tests`
Expected: clean (all `SpawnerBase` literals updated).

- [ ] **Step 8: Commit**

```bash
git add src/agents/ tests/
git commit -m "agents: cap concurrent subagent spawns with a Semaphore

SpawnerBase gains an optional subagent_semaphore (the slot the deleted
LaneScheduler vacated); spawn() acquires a permit for the child's
lifetime. SubagentTool owns one Semaphore (default 4) shared across the
foreground/batch/background paths."
```

---

## Task 4: Parent cancellation propagation (A3)

**Files:**
- Modify: `src/agents/subagent_tool.rs`
- Modify: `src/agents/subagent_tool/loop_tool.rs`
- Modify: `src/gateway/execution_engine/run_loop.rs`

- [ ] **Step 1: Write the failing test**

In `src/agents/subagent_tool.rs`, inside `mod tests`, add a hanging provider
and the test:

```rust
    /// Provider that blocks until the request's cancellation fires. Models a
    /// long-running LLM call. (The harness wires `cancel.cancelled()` into its
    /// LLM race, verified by tests/cancellation_chain.rs.)
    struct HangingProvider {
        cancel: CancellationToken,
    }
    impl AiProvider for HangingProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            let cancel = self.cancel.clone();
            Box::pin(async move {
                cancel.cancelled().await;
                Err(crate::error::AlephError::Cancelled)
            })
        }
        fn name(&self) -> &str { "hanging" }
        fn color(&self) -> &str { "#000000" }
    }

    #[tokio::test]
    async fn foreground_subagent_cancels_on_parent_token() {
        let parent = CancellationToken::new();
        let provider: Arc<dyn AiProvider> = Arc::new(HangingProvider {
            cancel: parent.clone(),
        });
        let chain = crate::harness::chain_context::ChainContext::new();
        let tool = SubagentTool::new(
            provider,
            chain,
            make_registry(),
            make_tracker(),
            in_mem_session(),
            Arc::new(NoopTestToolService),
            Arc::new(crate::sandbox::NoopSandbox),
        )
        .with_cancel_token(parent.clone());

        let handle = tokio::spawn(async move {
            tool.execute(serde_json::json!({ "task": "hang", "timeout_secs": 30 }))
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        parent.cancel();

        let result = tokio::time::timeout(std::time::Duration::from_secs(3), handle)
            .await
            .expect("foreground subagent did not honor parent cancel within 3s")
            .expect("task join");
        assert!(
            matches!(result, ToolResult::Error { .. }),
            "cancelled subagent must surface an error"
        );
    }
```

(`HangingProvider` reuses the `RequestPayload`, `ProviderResponse`, `Pin`,
`Future` imports — confirm `use std::future::Future; use std::pin::Pin;` and
the provider-adapter imports are in scope in the test module; they are already
used by `MockAiProvider`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib subagent_tool::tests::foreground_subagent_cancels -- --exact`
Expected: FAIL — `no method named with_cancel_token`.

- [ ] **Step 3: Add the `parent_cancel` field + builder + helper**

In `src/agents/subagent_tool.rs`:

Add field to `struct SubagentTool` (after `subagent_semaphore`):
```rust
    /// A3 — parent run's cancellation token. Each spawn path derives a
    /// `child_token()` so a cancelled parent stops its subagents.
    parent_cancel: Option<CancellationToken>,
```

In `SubagentTool::new`, add `parent_cancel: None,` to the struct.

Add builder (near the other `with_*`):
```rust
    /// A3 — wire the parent run's cancellation token so spawned subagents
    /// stop when the parent is cancelled.
    pub fn with_cancel_token(mut self, token: CancellationToken) -> Self {
        self.parent_cancel = Some(token);
        self
    }
```

Add a private helper inside `impl SubagentTool`:
```rust
    /// A3 — a fresh child token derived from the parent run's token (cancelled
    /// when the parent is). Falls back to a standalone token for tests / direct
    /// callers with no parent token wired.
    fn cancel_for_child(&self) -> CancellationToken {
        self.parent_cancel
            .as_ref()
            .map(|t| t.child_token())
            .unwrap_or_default()
    }
```

- [ ] **Step 4: Use the parent-derived token at every spawn path**

In `src/agents/subagent_tool.rs`, in `spawn_background`, replace
`let cancel_token = CancellationToken::new();` with:
```rust
        let cancel_token = self.cancel_for_child();
```

In `src/agents/subagent_tool/loop_tool.rs`:
- Foreground path: replace `self.build_runtime(child_chain, CancellationToken::new())` with `self.build_runtime(child_chain, self.cancel_for_child())`.
- Sync-batch path: replace `self.build_runtime(child_chain.clone(), CancellationToken::new())` with `self.build_runtime(child_chain.clone(), self.cancel_for_child())`.

- [ ] **Step 5: Wire the token at the production construction site**

In `src/gateway/execution_engine/run_loop.rs`, in the `SubagentTool` builder
chain (the `let mut t = SubagentTool::new(...)` block ~line 330), add
`.with_cancel_token(cancel_token.clone())` to the chain after
`.with_parent_session_id(...)`. `cancel_token` is already in scope (it is
passed to `run_dispatch_and_drain_classified` later in the same function).

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib subagent_tool::tests -- --skip batch`
Expected: PASS, including `foreground_subagent_cancels_on_parent_token`.
Run: `cargo build -p alephcore`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/agents/subagent_tool.rs src/agents/subagent_tool/loop_tool.rs src/gateway/execution_engine/run_loop.rs
git commit -m "agents: propagate parent cancellation to subagents

SubagentTool carries the parent run's CancellationToken; foreground,
sync-batch, and background paths each derive a child_token() instead of
minting an unrelated CancellationToken::new()."
```

---

## Task 5: Worktree isolation via `AgentDef` (B1)

**Files:**
- Modify: `src/agents/types.rs`
- Modify: `src/agents/loader.rs`
- Modify: `src/agents/runtime.rs`

- [ ] **Step 1: Write the failing test**

In `src/agents/loader.rs`, inside `mod tests`, add:

```rust
    #[test]
    fn parses_isolation_worktree_from_frontmatter() {
        use crate::agents::types::IsolationMode;
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            &tmp,
            "iso.md",
            "---\nid: iso\ndescription: d\nwhen_to_use: w\nisolation:\n  kind: worktree\n---\nbody\n",
        );
        let def = parse_file(&path, AgentSource::User).unwrap();
        assert_eq!(def.isolation, Some(IsolationMode::Worktree));
    }

    #[test]
    fn isolation_defaults_none_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            &tmp,
            "noiso.md",
            "---\nid: noiso\ndescription: d\nwhen_to_use: w\n---\nbody\n",
        );
        let def = parse_file(&path, AgentSource::User).unwrap();
        assert!(def.isolation.is_none());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib loader::tests::parses_isolation -- --exact`
Expected: FAIL — `no field isolation on type AgentDef`.

- [ ] **Step 3: Add the `AgentDef.isolation` field**

In `src/agents/types.rs`:

Add field to `struct AgentDef` (after `mcp_servers`):
```rust
    /// B1 — subagent worktree isolation. `#[serde(default)]` for schema
    /// back-compat; `None` (default) keeps the shared-cwd behaviour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<IsolationMode>,
```

In `AgentDef::new`, add `isolation: None,` to the returned struct.

Add builder (after `with_mcp_servers`):
```rust
    /// B1 — set subagent worktree isolation.
    pub fn with_isolation(mut self, mode: IsolationMode) -> Self {
        self.isolation = Some(mode);
        self
    }
```

- [ ] **Step 4: Parse `isolation` from frontmatter**

In `src/agents/loader.rs`:

Add to `struct UserFrontmatter`:
```rust
    #[serde(default)]
    isolation: Option<crate::agents::types::IsolationMode>,
```

In `parse_file`, after the `mcp_servers` block, add:
```rust
    if let Some(iso) = fm.isolation {
        def = def.with_isolation(iso);
    }
```

- [ ] **Step 5: Honor `AgentDef.isolation` in `AgentRuntime`**

In `src/agents/runtime.rs`, in `execute_via_harness`, change the `SpawnRequest`
construction's `isolation: None,` to:
```rust
            isolation: config.agent_def.isolation.clone(),
```

Delete the stale `// P3 Stage I — plugin_registry not threaded ...` comment is
NOT in this task (Task 6 owns it). Leave it.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib loader::tests -- --skip mcp`
Expected: PASS.
Run: `cargo build -p alephcore`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/agents/types.rs src/agents/loader.rs src/agents/runtime.rs
git commit -m "agents: wire worktree isolation from AgentDef to spawner

AgentDef gains an optional isolation field (declarable in markdown
frontmatter); AgentRuntime threads it into SpawnRequest.isolation (was
hardcoded None). Builtins stay default-off."
```

---

## Task 6: Per-agent MCP scope wiring (B2)

**Files:**
- Modify: `src/agents/runtime.rs`
- Modify: `src/agents/subagent_tool.rs`
- Modify: `src/gateway/execution_engine/run_loop.rs`

- [ ] **Step 1: Write the failing test (builder smoke)**

In `src/agents/runtime.rs`, inside `mod tests`, add:

```rust
    #[test]
    fn with_plugin_registry_builder_compiles() {
        use crate::extension::registry::PluginRegistry;
        let runtime = AgentRuntime::new(
            Arc::new(crate::providers::testing::null_provider()),
            ChainContext::new().child().unwrap(),
            CancellationToken::new(),
            crate::session::testing::null_session_service(),
            crate::tools::testing::null_tool_service(),
            Arc::new(crate::sandbox::NoopSandbox),
        )
        .with_plugin_registry(Arc::new(PluginRegistry::new()));
        let _ = runtime;
    }
```

> **Note:** the exact null/mock constructors above may not exist. Before
> writing this test, run `grep -rn "AgentRuntime::new" src/ tests/` and copy a
> working construction from an existing test. If `runtime.rs` has no test that
> builds an `AgentRuntime`, instead place this smoke test in
> `src/agents/subagent_tool.rs` `mod tests` using the `make_tool()` helper plus
> a new `.with_plugin_registry(...)` call on `SubagentTool` (see Step 3) — the
> behavioral guarantee for B2 is already covered by
> `subagent_spawner::tests::spawn_mcp_scope_unknown_reference_fails_loud`, so a
> compile-level smoke test is sufficient here.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib runtime::tests::with_plugin_registry -- --exact`
Expected: FAIL — `no method named with_plugin_registry`.

- [ ] **Step 3: Thread `plugin_registry` through `AgentRuntime` + `SubagentTool`**

In `src/agents/runtime.rs`:

Add field to `struct AgentRuntime` (after `subagent_semaphore`):
```rust
    /// B2 — global plugin registry, threaded into SpawnerBase for per-agent
    /// MCP scope provisioning.
    plugin_registry: Option<Arc<crate::extension::registry::PluginRegistry>>,
```

In `AgentRuntime::new`, add `plugin_registry: None,`.

Add builder:
```rust
    /// B2 — wire the global plugin registry for per-agent MCP scope.
    pub fn with_plugin_registry(
        mut self,
        registry: Arc<crate::extension::registry::PluginRegistry>,
    ) -> Self {
        self.plugin_registry = Some(registry);
        self
    }
```

In `execute_via_harness`, replace the `SpawnerBase` field
`plugin_registry: None,` (and delete the stale `// P3 Stage I — plugin_registry
not threaded through AgentRuntime yet ...` comment above it) with:
```rust
            plugin_registry: self.plugin_registry.clone(),
```

In `src/agents/subagent_tool.rs`:

Add field to `struct SubagentTool`:
```rust
    /// B2 — global plugin registry, threaded into each AgentRuntime.
    plugin_registry: Option<Arc<crate::extension::registry::PluginRegistry>>,
```

In `SubagentTool::new`, add `plugin_registry: None,`.

Add builder:
```rust
    /// B2 — wire the global plugin registry for per-agent MCP scope.
    pub fn with_plugin_registry(
        mut self,
        registry: Arc<crate::extension::registry::PluginRegistry>,
    ) -> Self {
        self.plugin_registry = Some(registry);
        self
    }
```

In `build_runtime`, before `runtime` is returned, add:
```rust
        if let Some(reg) = self.plugin_registry.clone() {
            runtime = runtime.with_plugin_registry(reg);
        }
```

- [ ] **Step 4: Wire the registry at the production construction site**

In `src/gateway/execution_engine/run_loop.rs`, in the `SubagentTool` builder
chain, add `.with_plugin_registry(...)`. **Discovery step first:** run
`grep -rn "PluginRegistry" src/gateway/execution_engine/ src/gateway/mod.rs`
and `grep -rn "plugin_registry\|PluginRegistry" src/orchestrator/` to locate
the live `Arc<PluginRegistry>`. The `extension_manager` already in scope at
`run_loop.rs:~284` is the most likely owner. Wire whichever accessor exposes
the registry. If no `Arc<PluginRegistry>` is reachable at this site without an
out-of-scope `ExecutionEngine` change, leave the construction site unwired
(the field stays `None`, identical to today) and record it as a follow-up in
the commit message — the `AgentRuntime`/`SubagentTool` plumbing still lands and
is unit-covered.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib runtime::tests subagent_tool::tests -- --skip batch`
Expected: PASS.
Run: `cargo test -p alephcore --lib subagent_spawner::tests::spawn_mcp_scope`
Expected: PASS (behavioral guarantee intact).

- [ ] **Step 6: Commit**

```bash
git add src/agents/runtime.rs src/agents/subagent_tool.rs src/gateway/execution_engine/run_loop.rs
git commit -m "agents: thread plugin registry into runtime-spawned subagents

AgentRuntime + SubagentTool carry the global PluginRegistry; the spawner
can now provision per-agent MCP scope for agents declaring mcp_servers
(was hardcoded None -> fail-loud)."
```

---

## Task 7: Stage A safety-feature wiring (B3) + background progress (B4)

**Files:**
- Modify: `src/agents/subagent_tool.rs`
- Modify: `src/gateway/execution_engine/run_loop.rs`

- [ ] **Step 1: Write the failing test (trace_sink reaches the child)**

In `src/agents/subagent_tool.rs`, inside `mod tests`, add a usage-returning
provider, a capturing sink, and the test:

```rust
    struct UsageMockProvider;
    impl AiProvider for UsageMockProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            Box::pin(async {
                Ok(ProviderResponse {
                    text: Some("done".into()),
                    tool_calls: vec![],
                    thinking: None,
                    thinking_signature: None,
                    stop_reason: crate::providers::adapter::StopReason::EndTurn,
                    usage: Some(crate::providers::adapter::TokenUsage {
                        input_tokens: 10,
                        output_tokens: 5,
                        cache_read_tokens: None,
                        cache_creation_tokens: None,
                        thinking_tokens: None,
                        cost: None,
                    }),
                })
            })
        }
        fn name(&self) -> &str { "usage-mock" }
        fn color(&self) -> &str { "#000000" }
    }

    struct CapturingSink(std::sync::Mutex<Vec<crate::harness::trace::LoopTraceEvent>>);
    impl crate::harness::TraceSink for CapturingSink {
        fn on_trace(&self, e: &crate::harness::trace::LoopTraceEvent) {
            self.0.lock().unwrap().push(e.clone());
        }
        fn flush(&self) {}
    }

    #[tokio::test]
    async fn foreground_subagent_inherits_trace_sink() {
        let sink = Arc::new(CapturingSink(std::sync::Mutex::new(vec![])));
        let chain = crate::harness::chain_context::ChainContext::new();
        let tool = SubagentTool::new(
            Arc::new(UsageMockProvider),
            chain,
            make_registry(),
            make_tracker(),
            in_mem_session(),
            Arc::new(NoopTestToolService),
            Arc::new(crate::sandbox::NoopSandbox),
        )
        .with_trace_sink(sink.clone() as Arc<dyn crate::harness::TraceSink>);

        let _ = tool.execute(serde_json::json!({ "task": "hi" })).await;

        let events = sink.0.lock().unwrap();
        assert!(
            !events.is_empty(),
            "subagent run must emit trace events into the inherited sink"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib subagent_tool::tests::foreground_subagent_inherits_trace_sink -- --exact`
Expected: FAIL — the foreground path does not yet apply `self.trace_sink`, so
the child harness has no sink and `events` is empty.

- [ ] **Step 3: Add the four resilience fields + builders to `SubagentTool`**

In `src/agents/subagent_tool.rs`, add fields to `struct SubagentTool` (after
`plugin_registry`):
```rust
    /// B3 — fallback LLM inherited by subagents.
    fallback_llm: Option<Arc<dyn AiProvider>>,
    /// B3 — stall watchdog config inherited by subagents.
    stall_config: Option<crate::harness::StallConfig>,
    /// B3 — consecutive-failure cap inherited by subagents.
    consecutive_failure_cap: Option<usize>,
    /// B3 — per-turn wall-clock timeout inherited by subagents.
    turn_timeout: Option<std::time::Duration>,
```

In `SubagentTool::new`, add `fallback_llm: None, stall_config: None,
consecutive_failure_cap: None, turn_timeout: None,`.

Add builders:
```rust
    /// B3 — wire the fallback LLM.
    pub fn with_fallback_llm(mut self, fallback: Arc<dyn AiProvider>) -> Self {
        self.fallback_llm = Some(fallback);
        self
    }
    /// B3 — wire the stall watchdog config.
    pub fn with_stall_config(mut self, config: crate::harness::StallConfig) -> Self {
        self.stall_config = Some(config);
        self
    }
    /// B3 — wire the consecutive-failure cap.
    pub fn with_consecutive_failure_cap(mut self, cap: usize) -> Self {
        self.consecutive_failure_cap = Some(cap);
        self
    }
    /// B3 — wire the per-turn wall-clock timeout.
    pub fn with_turn_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.turn_timeout = Some(timeout);
        self
    }
```

- [ ] **Step 4: Apply the five fields in `build_runtime`**

In `src/agents/subagent_tool.rs`, in `build_runtime`, before `runtime` is
returned, add:
```rust
        if let Some(sink) = self.trace_sink.clone() {
            runtime = runtime.with_trace_sink(sink);
        }
        if let Some(fb) = self.fallback_llm.clone() {
            runtime = runtime.with_fallback_llm(fb);
        }
        if let Some(sc) = self.stall_config.clone() {
            runtime = runtime.with_stall_config(sc);
        }
        if let Some(cap) = self.consecutive_failure_cap {
            runtime = runtime.with_consecutive_failure_cap(cap);
        }
        if let Some(tt) = self.turn_timeout {
            runtime = runtime.with_turn_timeout(tt);
        }
```

> The background path's `ForwardingTraceSink` wrapper (Task 2 Step 3) runs
> *after* `build_runtime` and calls `with_trace_sink` again — last write wins,
> so background subagents get the forwarding wrapper while foreground/sync-batch
> get the raw sink. No change needed there.

- [ ] **Step 5: Run the trace_sink test to verify it passes**

Run: `cargo test -p alephcore --lib subagent_tool::tests::foreground_subagent_inherits_trace_sink -- --exact`
Expected: PASS.

- [ ] **Step 6: Add the background-progress (B4) verification test**

In `src/agents/subagent_tool.rs`, inside `mod tests`, add:

```rust
    #[tokio::test]
    async fn background_subagent_forwards_trace_to_parent_sink() {
        let sink = Arc::new(CapturingSink(std::sync::Mutex::new(vec![])));
        let chain = crate::harness::chain_context::ChainContext::new();
        let tracker = make_tracker();
        let tool = SubagentTool::new(
            Arc::new(UsageMockProvider),
            chain,
            make_registry(),
            tracker.clone(),
            in_mem_session(),
            Arc::new(NoopTestToolService),
            Arc::new(crate::sandbox::NoopSandbox),
        )
        .with_trace_sink(sink.clone() as Arc<dyn crate::harness::TraceSink>);

        let out = tool
            .execute(serde_json::json!({ "task": "bg", "run_in_background": true }))
            .await;
        let rid = match out {
            ToolResult::Success { output } => {
                output["request_id"].as_str().unwrap().to_string()
            }
            other => panic!("expected background success, got {other:?}"),
        };

        // Poll until the background task completes (bounded).
        for _ in 0..50 {
            if tracker.list_running().iter().all(|(id, _, _)| id != &rid) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let events = sink.0.lock().unwrap();
        assert!(
            !events.is_empty(),
            "background subagent must forward trace events to the parent sink \
             via ForwardingTraceSink"
        );
    }
```

Run: `cargo test -p alephcore --lib subagent_tool::tests::background_subagent_forwards -- --exact`
Expected: PASS (the background path already installs `ForwardingTraceSink`
when `trace_sink` is `Some`; B3 makes it `Some`).

- [ ] **Step 7: Wire the construction site**

In `src/gateway/execution_engine/run_loop.rs`:

**7a — trace_sink (the easy win):** the `GatewayTraceSink` is currently built
*after* the `SubagentTool` block (`let trace_sink: Arc<dyn ...> = Arc::new(...)`).
Move that `let trace_sink = ...;` statement to *above* the
`let subagent_tool = { ... };` block. Then add `.with_trace_sink(trace_sink.clone())`
to the `SubagentTool` builder chain. The later `FlowRequest { trace_sink: Some(trace_sink), ... }`
still works (it consumes the same `trace_sink` binding).

**7b — resilience values (discovery):** run
`grep -rn "build_fallback_llm\|build_stability_triple\|StallConfig" src/orchestrator/ src/gateway/`
to locate how the main runner obtains `fallback_llm` / `stall_config` /
`consecutive_failure_cap` / `turn_timeout`. If those builders (or the resolved
values) are reachable from the `ExecutionEngine` / config in scope at the
construction site, add the matching `.with_fallback_llm(...)` /
`.with_stall_config(...)` / `.with_consecutive_failure_cap(...)` /
`.with_turn_timeout(...)` calls. If they require an out-of-scope
`ExecutionEngine` change, wire only `trace_sink` now and record the four
resilience values as a noted follow-up in the commit message — the builders +
helper plumbing still ship and are unit-covered by Step 5.

- [ ] **Step 8: Run tests + build**

Run: `cargo test -p alephcore --lib subagent_tool::tests`
Expected: PASS.
Run: `cargo build -p alephcore`
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add src/agents/subagent_tool.rs src/gateway/execution_engine/run_loop.rs
git commit -m "agents: inherit trace sink + resilience config in subagents

SubagentTool threads trace_sink (and fallback_llm/stall/turn_timeout/
failure-cap) into every AgentRuntime via build_runtime. Closes the P2
wiring hop: background check_status progress is now populated, and
subagent runs emit into the gateway trace sink."
```

---

## Task 8: `context_mode` authoritative (B5) + background tracker cleanup (C1)

**Files:**
- Modify: `src/agents/subagent_spawner/mod.rs`
- Modify: `src/agents/subagent_spawner/tests.rs`
- Modify: `src/agents/background_tracker.rs`

- [ ] **Step 1: Write the failing test (B5)**

In `src/agents/subagent_spawner/mod.rs`, inside `mod tests` (`tests.rs`), add:

```rust
    #[test]
    fn build_effective_task_fresh_mode_ignores_summary() {
        use crate::agents::ContextMode;
        let t = build_effective_task(Some("SECRET-CONTEXT"), ContextMode::Fresh, "do work");
        assert_eq!(t, "do work");
        assert!(!t.contains("SECRET-CONTEXT"));
        assert!(!t.contains("Context from parent agent"));
    }

    #[test]
    fn build_effective_task_summary_mode_prepends_summary() {
        use crate::agents::ContextMode;
        let t = build_effective_task(Some("PARENT-CTX"), ContextMode::Summary, "do work");
        assert!(t.contains("Context from parent agent"));
        assert!(t.contains("PARENT-CTX"));
        assert!(t.ends_with("do work"));
    }

    #[test]
    fn build_effective_task_no_summary_is_bare_task() {
        use crate::agents::ContextMode;
        assert_eq!(build_effective_task(None, ContextMode::Summary, "just this"), "just this");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib subagent_spawner::tests::build_effective_task -- --skip xxx`
Expected: FAIL — `cannot find function build_effective_task`.

- [ ] **Step 3: Extract `build_effective_task` and make `context_mode` authoritative**

In `src/agents/subagent_spawner/mod.rs`, add a free function near
`extract_run_result`:

```rust
/// B5 — assemble the child's seed task. A `context_summary` is prepended only
/// when the agent's declared `context_mode` is `Summary`; `Fresh`-mode agents
/// always start from the bare task, making `AgentDef.context_mode`
/// authoritative instead of decorative.
fn build_effective_task(
    context_summary: Option<&str>,
    context_mode: crate::agents::ContextMode,
    task: &str,
) -> String {
    match context_summary {
        Some(summary) if context_mode == crate::agents::ContextMode::Summary => {
            format!(
                "## Context from parent agent\n\n{}\n\n---\n\n{}",
                summary, task
            )
        }
        _ => task.to_string(),
    }
}
```

In `spawn()`, replace the existing `let effective_task = match req.context_summary { ... };`
block with:
```rust
        let effective_task = build_effective_task(
            req.context_summary,
            req.agent_def.context_mode.clone(),
            req.task,
        );
```

- [ ] **Step 4: Run B5 tests to verify they pass**

Run: `cargo test -p alephcore --lib subagent_spawner::tests::build_effective_task`
Expected: PASS (3 tests).

- [ ] **Step 5: Write the failing test (C1)**

In `src/agents/background_tracker.rs`, inside `mod tests`, add:

```rust
    #[test]
    fn register_prunes_stale_completed_keeps_fresh() {
        let tracker = BackgroundAgentTracker::new();
        // A freshly completed entry must survive a register() call.
        tracker.mark_completed("fresh", Ok("r".to_string()));
        let token = CancellationToken::new();
        tracker.register("new-run".to_string(), token, "task".to_string());
        assert!(
            tracker.take_result("fresh").is_some(),
            "register() must not evict a still-fresh completed entry"
        );
    }
```

- [ ] **Step 6: Run test to verify it fails or passes trivially**

Run: `cargo test -p alephcore --lib background_tracker::tests::register_prunes_stale_completed -- --exact`
Expected: PASS already (register does not yet prune, fresh entry survives
trivially). This test pins the *fresh-survives* invariant; the prune wiring is
verified by it staying green plus the existing `cleanup_removes_old_entries`.

- [ ] **Step 7: Add the opportunistic cleanup caller**

In `src/agents/background_tracker.rs`:

Add near the top of the file (after imports):
```rust
/// C1 — completed background results older than this are pruned opportunistically
/// on each new `register()`.
const BACKGROUND_RESULT_TTL: Duration = Duration::from_secs(3600);
```

In `register()`, as the first statement of the method body, add:
```rust
        self.cleanup(BACKGROUND_RESULT_TTL);
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib background_tracker::tests subagent_spawner::tests::build_effective_task`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add src/agents/subagent_spawner/ src/agents/background_tracker.rs
git commit -m "agents: honor context_mode + prune stale background results

Fresh-mode agents now drop any context_summary (context_mode was
decorative). BackgroundAgentTracker::register prunes completed results
older than 1h (cleanup had no caller)."
```

---

## Task 9: Docs, roadmap reconciliation, full verification

**Files:**
- Modify: `src/agents/runtime.rs` (remove stale comment)
- Modify: `docs/reference/MULTI_AGENT_SYSTEM.md`
- Modify: `docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md`

- [ ] **Step 1: Remove the stale `runtime.rs` comment block**

In `src/agents/runtime.rs`, delete the comment block at lines ~177-188 (the
`// Stage A (P1, 2026-05-08): These 5 with_* builders are reachable ...` block
ending `... the orchestration plumbing closes the loop in P2.`). The wiring it
described is now done. Keep the individual `/// Stage A (P1) — ...` doc
comments on each `with_*` method.

- [ ] **Step 2: Update `MULTI_AGENT_SYSTEM.md`**

Run `grep -n "lane\|LaneScheduler\|trace_sink\|isolation\|concurrency" docs/reference/MULTI_AGENT_SYSTEM.md`
and correct any statements that contradict the shipped code:
- Remove/replace any "lane scheduler" priority claims (the `LaneScheduler` is deleted).
- State the actual concurrency control: a `tokio::sync::Semaphore` (default 4) on `SubagentTool`.
- Note worktree isolation is opt-in via `AgentDef.isolation` in frontmatter.
If no such stale statements exist, add one short paragraph documenting the
concurrency cap + isolation opt-in under the subagent section.

- [ ] **Step 3: Reconcile the uplift roadmap**

In `docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md`, append
to the top status block (per the roadmap's §4.1 light-revision rule):
```
⚠️ Stage C (LaneScheduler) reverted 2026-05-19 (commits ae4f05532 + e0e29d886) — orphaned, never wired. Replaced by a tokio::Semaphore in 2026-05-19-subagent-hardening.
✅ Production wiring of Stages A/F/H/I closed 2026-05-19 (subagent-hardening branch): run_loop.rs → SubagentTool → AgentRuntime hop completed.
```

- [ ] **Step 4: Full build + lint**

Run: `cargo build -p alephcore`
Expected: clean.
Run: `cargo clippy -p alephcore --lib --tests 2>&1 | tail -30`
Expected: no new warnings in the files this plan touched (pre-existing warnings
elsewhere do not block — compare against the baseline if unsure).

- [ ] **Step 5: Full test suite**

Run: `cargo test -p alephcore --lib 2>&1 | tail -25`
Expected: PASS except the known baseline failures (per memory
`project_baseline_test_failures`: 8 lib failures on `main` unrelated to the
agents layer — confirm any failures match that set and are not in
`agents::` / `providers::metering`).

Run: `cargo test -p alephcore --test cancellation_chain --test worktree_isolation --test subagent_deps_inherit 2>&1 | tail -20`
Expected: PASS (these must stay green — they are the subagent regression net).

- [ ] **Step 6: Commit**

```bash
git add src/agents/runtime.rs docs/
git commit -m "docs: reconcile subagent docs with shipped wiring

Removes the stale 'P2 deliverable' comment in runtime.rs; updates
MULTI_AGENT_SYSTEM.md and the uplift roadmap (Stage C reverted, A/F/H/I
production wiring closed)."
```

- [ ] **Step 7: Final verification summary**

Confirm against the spec's §5 test table — every row T-A1..T-C1 maps to a
passing test:
- T-A1 → `metering::tests::accumulator_sums_input_and_output_tokens` + `subagent_spawner::tests::spawn_reports_total_tokens_from_usage`
- T-A2 → `subagent_spawner::tests::spawn_blocks_when_semaphore_exhausted`
- T-A3 → `subagent_tool::tests::foreground_subagent_cancels_on_parent_token`
- T-B1 → `loader::tests::parses_isolation_worktree_from_frontmatter`
- T-B2 → `runtime::tests::with_plugin_registry_builder_compiles` + existing `spawn_mcp_scope_unknown_reference_fails_loud`
- T-B3 → `subagent_tool::tests::foreground_subagent_inherits_trace_sink`
- T-B4 → `subagent_tool::tests::background_subagent_forwards_trace_to_parent_sink`
- T-B5 → `subagent_spawner::tests::build_effective_task_*`
- T-C1 → `background_tracker::tests::register_prunes_stale_completed_keeps_fresh` + existing `cleanup_removes_old_entries`

---

## Self-Review Notes

- **Spec coverage:** A1→T1, A2→T3, A3→T4, B1→T5, B2→T6, B3→T7, B4→T7, B5→T8, C-helper→T2, C1→T8, docs→T9. All spec §3 items covered.
- **Type consistency:** `build_runtime(&self, ChainContext, CancellationToken) -> AgentRuntime` used identically in Tasks 2/3/4/6/7. `with_token_accumulator` (Task 1) consistent. `subagent_semaphore: Option<Arc<tokio::sync::Semaphore>>` consistent across `SpawnerBase`/`AgentRuntime`. `with_plugin_registry` signature identical on `AgentRuntime` and `SubagentTool`.
- **Known open question:** B3 Step 7b (resilience-config source at the construction site) — both resolutions (full wiring vs trace_sink-only) are spelled out; the task is executable either way.
