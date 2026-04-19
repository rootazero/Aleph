# Managed-Agents Phase 4 — Harness (Think→Act) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the 4,559-line `src/agent_loop/loop_core.rs` monolith with a < 600-line `src/harness/` module that implements Anthropic's managed-agents Think→Act loop, after first relocating cross-cutting concerns (context pre-processing, retry, safety, streaming, approval plumbing, session scope) out of `agent_loop/` to their proper homes.

**Architecture:** Two sub-phases. **Phase 4a** (Tasks 1–6) relocates concerns while the old loop stays in production behind default wiring. **Phase 4b** (Tasks 7–12) writes a new `src/harness/` with `Harness` trait + `AgentHarness` impl, selectable at startup via `ALEPH_HARNESS_V2=1`. All twelve tasks are independently shippable; `cargo test` must be green after each.

**Tech Stack:** Rust async (tokio, async_trait), `Arc<dyn Trait>` composition, `task_local!` scope propagation, `mockall` + `#[tokio::test]` for unit tests.

---

## Source Documents

- Spec: `docs/superpowers/specs/2026-04-19-harness-think-act-design.md`
- Roadmap: `docs/superpowers/specs/2026-04-18-managed-agents-refactor-roadmap.md` §8 Phase 4
- Predecessor specs: Phase 1 SessionService, Phase 2 ToolService façade, Phase 3 Sandbox/Workspace

## Baseline (Do Not Regress)

- Pre-existing test baseline at Phase 3 merge (`2ee280ab2`): **9059 pass / 2 known-fail / 20 ignored**.
- The 2 known failures are `telegram::config::tests::parse_v2_config_directly` and `memory::notes::ingest::prompts::tests::base_prompt_snapshot`. Do not "fix" these; just do not add more failures.
- Every task ends with `cargo test --lib -p alephcore` (or equivalent scope) and must show `pass/fail` counts equal to baseline.
- `cargo clippy -- -D warnings` must pass on touched files.
- `cargo fmt` must leave no diffs.

## Release Policy

**Do NOT release at any point in this plan.** When a task says "ship" it means "merge to main" — CalVer release is a separate, user-initiated action via `just release YYYY.MM.DD`. After the final task, **stop and ask the user** whether to release.

## Anchor File Paths (Pre-Verified)

| What | Path |
|---|---|
| `SessionService` trait | `src/session/service.rs` |
| `SessionEvent` enum | `src/session/events.rs` |
| `ToolService` trait | `src/tools/service.rs` |
| `Sandbox` trait | `src/sandbox/mod.rs` |
| `ApprovalRequester` trait | `src/agent_loop/exec_approval/gate.rs` |
| `ChannelApprovalBridge` | `src/exec/approval/channel_bridge.rs` |
| `LayeredPermissionResolver` | `src/tools/middleware/permission/resolver.rs` |
| `invoke_with_session_trace` | `src/session/tool_trace.rs` |
| Old loop monolith | `src/agent_loop/loop_core.rs` |

## File Structure After Phase 4

```
src/harness/                          # NEW (Phase 4b)
├── mod.rs                            # pub re-exports
├── trait_def.rs                      # Harness trait + TurnState + HarnessError
├── agent.rs                          # AgentHarness impl
├── deps.rs                           # HarnessDeps bundle
└── tests/                            # unit + integration tests
    ├── think.rs
    ├── act.rs
    └── run.rs

src/session/tool_trace.rs             # MODIFIED (4a.1 — helper extracted)
src/approval/adapters.rs              # NEW (4a.2 — ChannelApprovalBridgeAdapter)
src/tools/middleware/permission/resolver.rs  # MODIFIED (4a.3 — exec-class exclude-list)
src/tools/middleware/pre_llm/         # NEW (4a.4 — pre-LLM middleware)
├── mod.rs
└── trait_def.rs

src/agent_loop/loop_core.rs           # SHRINKS from 4559 → <1500 lines
src/agent_loop/context_budget/        # DELETED after 4a.4
src/agent_loop/compaction/            # DELETED after 4a.4
src/agent_loop/truncation_recovery.rs # DELETED after 4a.4
src/agent_loop/sections/              # DELETED after 4a.4
src/agent_loop/retry.rs               # DELETED after 4a.5 (logic moved)
src/agent_loop/safety.rs              # DELETED after 4a.5 (logic moved)
src/agent_loop/streaming_bridge.rs    # DELETED after 4a.5 (logic moved)
```

---

# Phase 4a — Relocate Cross-Cutting Concerns (Tasks 1–6)

## Task 1: H2 — SESSION_ID Scope Propagation to All Tool Entry Points

**Files:**
- Modify: `src/session/tool_trace.rs` (extract helper)
- Modify: cron dispatch call sites (discovered below)
- Modify: heartbeat call sites (discovered below)
- Modify: direct-dispatch call sites (discovered below)
- Test: `tests/session_scope_propagation.rs` (create)

**Context:** `CodeExecTool` reads `SESSION_ID` from a task_local; if unset, it returns "no active session context". Currently only `invoke_with_session_trace` sets it. Cron/heartbeat/direct-dispatch paths bypass this and fail.

- [ ] **Step 1.1: Discover every call site that invokes a tool outside `invoke_with_session_trace`**

Run:
```bash
grep -rn "ToolService" src/ --include='*.rs' | grep -v "tool_trace.rs" | grep -v "test" | grep "execute\|invoke"
grep -rn "SESSION_ID" src/ --include='*.rs' | grep -v "tool_trace.rs"
```
Expected: a list of 3–8 files. Capture the list — these are the call sites to retrofit.

- [ ] **Step 1.2: Write failing integration test**

Create `tests/session_scope_propagation.rs`:

```rust
//! Verifies SESSION_ID task_local is set at all tool entry points.

use alephcore::routing::session_key::SessionKey;
use alephcore::session::tool_trace::with_session_scope;
use alephcore::session::SESSION_ID;

#[tokio::test]
async fn with_session_scope_sets_task_local() {
    let sid = SessionKey::from("test-session-42");
    let observed = with_session_scope(&sid, async {
        SESSION_ID.try_with(|s| s.clone()).ok()
    })
    .await;
    assert_eq!(observed, Some(sid));
}

#[tokio::test]
async fn outside_scope_no_session_id() {
    let observed = SESSION_ID.try_with(|s| s.clone()).ok();
    assert!(observed.is_none());
}
```

- [ ] **Step 1.3: Run test (expect fail — helper does not exist yet)**

Run: `cargo test --test session_scope_propagation -- --nocapture`
Expected: compile error — `with_session_scope` not found.

- [ ] **Step 1.4: Extract `with_session_scope` helper from `invoke_with_session_trace`**

In `src/session/tool_trace.rs`, add:

```rust
use std::future::Future;

/// Sets `SESSION_ID` task_local for the duration of `fut`.
/// Use at every tool entry point — direct dispatch, cron, heartbeat.
pub async fn with_session_scope<F, T>(session_id: &SessionId, fut: F) -> T
where
    F: Future<Output = T>,
{
    SESSION_ID.scope(session_id.clone(), fut).await
}
```

Then refactor `invoke_with_session_trace` to call it:

```rust
pub async fn invoke_with_session_trace(
    tool_svc: &Arc<dyn ToolService>,
    session_svc: &Arc<dyn SessionService>,
    session_id: &SessionId,
    name: &str,
    input: serde_json::Value,
) -> Result<ToolOutput, ToolError> {
    with_session_scope(session_id, async {
        // existing body (emit events, call tool_svc.execute, ...)
    })
    .await
}
```

Keep the existing body identical — only the scoping is factored out.

- [ ] **Step 1.5: Run test (expect pass)**

Run: `cargo test --test session_scope_propagation -- --nocapture`
Expected: 2 passed.

- [ ] **Step 1.6: Retrofit each call site from Step 1.1**

For each call site discovered, wrap the `tool_svc.execute(...)` (or equivalent) call in `with_session_scope(&session_id, async { ... }).await`. Do this one file at a time; run `cargo check` after each.

Example (shape — exact location per site varies):
```rust
// BEFORE
let output = tool_svc.execute(name, input).await?;

// AFTER
let output = with_session_scope(&session_id, async {
    tool_svc.execute(name, input).await
}).await?;
```

- [ ] **Step 1.7: Add targeted regression test for cron path**

In `tests/session_scope_propagation.rs`, add:

```rust
#[tokio::test]
async fn cron_dispatch_sets_session_id() {
    // Simulate the exact shape cron uses. Reproduce the failure path
    // that used to report "no active session context" by calling
    // through the retrofitted cron dispatcher helper and asserting
    // no such error.
    // (Wire this to the actual cron entry point discovered in Step 1.1.)
}
```

Populate the body using the concrete helper name discovered in Step 1.1. If cron wraps its dispatch in a function, call it directly from the test; if it dispatches via a scheduled job, use the in-process job runner.

- [ ] **Step 1.8: Run full suite**

Run: `cargo test --lib -p alephcore && cargo test --test session_scope_propagation`
Expected: baseline preserved (9059 pass / 2 known-fail / 20 ignored) + new tests green.

- [ ] **Step 1.9: Clippy + fmt**

Run: `cargo clippy -- -D warnings && cargo fmt --check`
Expected: no warnings, no diffs.

- [ ] **Step 1.10: Commit**

```bash
git add src/session/tool_trace.rs tests/session_scope_propagation.rs <other modified files>
git commit -m "session: scope SESSION_ID at all tool entry points (H2)"
```

---

## Task 2: H1 — ApprovalRequester Adapter over ChannelApprovalBridge

**Files:**
- Create: `src/approval/adapters.rs`
- Modify: `src/approval/mod.rs` (register module)
- Modify: Phase 3 `WorkspaceSandbox` wiring site (find with grep below)
- Test: `src/approval/adapters.rs` inline `#[cfg(test)] mod tests`

**Context:** `ChannelApprovalBridge` uses legacy `ApprovalRequest { command, cwd, session_key, ... }`. Tool-level `ApprovalRequester::request_approval(tool_name, reason) -> ApprovalOutcome` needs different shape. Phase 3 deferred this with `tracing::warn! + Denied` fallback.

- [ ] **Step 2.1: Inspect the legacy `ApprovalRequest` shape**

Run:
```bash
grep -n "pub struct ApprovalRequest\|ApprovalRequest {" src/exec/approval/channel_bridge.rs src/exec/approval/mod.rs
```
Expected: a struct with fields like `command: String`, `cwd: PathBuf`, `session_key: SessionKey`, plus result sender. Record the exact field list.

- [ ] **Step 2.2: Find the Phase 3 fallback site**

Run:
```bash
grep -rn "Phase 3 fallback\|tracing::warn\|ApprovalRequester" src/sandbox/ src/tools/middleware/
```
Expected: the site in WorkspaceSandbox / PermissionLayer that currently logs and returns `Denied`.

- [ ] **Step 2.3: Write failing unit test**

Create `src/approval/adapters.rs`:

```rust
//! Adapter: bridges tool-level `ApprovalRequester` onto the legacy
//! `ChannelApprovalBridge` transport.

use std::sync::Arc;

use async_trait::async_trait;

use crate::agent_loop::exec_approval::gate::{ApprovalOutcome, ApprovalRequester};
use crate::exec::approval::channel_bridge::ChannelApprovalBridge;

pub struct ChannelApprovalBridgeAdapter {
    bridge: Arc<ChannelApprovalBridge>,
}

impl ChannelApprovalBridgeAdapter {
    pub fn new(bridge: Arc<ChannelApprovalBridge>) -> Self {
        Self { bridge }
    }
}

#[async_trait]
impl ApprovalRequester for ChannelApprovalBridgeAdapter {
    async fn request_approval(&self, tool_name: &str, reason: &str) -> ApprovalOutcome {
        // Map tool-level request → legacy ApprovalRequest shape.
        // `command` is synthesized from tool_name; `cwd` and `session_key`
        // come from the current SESSION_ID scope.
        let outcome = self
            .bridge
            .request_for_tool(tool_name, reason)
            .await;
        match outcome {
            crate::exec::approval::types::BridgeOutcome::Approved => ApprovalOutcome::Approved,
            crate::exec::approval::types::BridgeOutcome::Denied => ApprovalOutcome::Denied,
            crate::exec::approval::types::BridgeOutcome::Timeout => ApprovalOutcome::Timeout,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn adapter_forwards_approved() {
        let bridge = Arc::new(ChannelApprovalBridge::for_test_always_approved());
        let adapter = ChannelApprovalBridgeAdapter::new(bridge);
        let out = adapter.request_approval("code_exec", "run ls").await;
        assert_eq!(out, ApprovalOutcome::Approved);
    }

    #[tokio::test]
    async fn adapter_forwards_denied() {
        let bridge = Arc::new(ChannelApprovalBridge::for_test_always_denied());
        let adapter = ChannelApprovalBridgeAdapter::new(bridge);
        let out = adapter.request_approval("code_exec", "rm -rf").await;
        assert_eq!(out, ApprovalOutcome::Denied);
    }
}
```

Register in `src/approval/mod.rs`:
```rust
pub mod adapters;
```

- [ ] **Step 2.4: If `ChannelApprovalBridge` lacks `request_for_tool` or test helpers, add them**

The test helpers (`for_test_always_approved`, `for_test_always_denied`) and the `request_for_tool(tool_name, reason)` method may not yet exist. Open `src/exec/approval/channel_bridge.rs` and either:
- (a) add them as the minimum needed to make the adapter test compile, or
- (b) if the existing `request(&self, ApprovalRequest) -> ...` method is the only entry, synthesize an `ApprovalRequest` inside the adapter.

Keep additions minimal; no speculative API surface.

- [ ] **Step 2.5: Run adapter test (expect pass)**

Run: `cargo test --lib -p alephcore approval::adapters`
Expected: 2 passed.

- [ ] **Step 2.6: Replace the Phase 3 fallback at the wiring site**

At the site discovered in Step 2.2, replace:
```rust
// OLD
tracing::warn!("Phase 3: approval requester not wired; denying");
ApprovalOutcome::Denied
```
with:
```rust
// NEW
ChannelApprovalBridgeAdapter::new(channel_bridge.clone())
    .request_approval(tool_name, reason)
    .await
```

(Exact wiring depends on whether the site holds `Arc<ChannelApprovalBridge>` already. If not, thread it through via the nearest constructor.)

- [ ] **Step 2.7: Write integration test for end-to-end approval**

Create `tests/exec_approval_e2e.rs` (or extend an existing one):

```rust
#[tokio::test]
async fn exec_tool_under_ask_policy_produces_one_prompt() {
    // Set up: tool classified `Ask`, channel bridge records prompt count.
    // Invoke exec tool. Assert: exactly 1 prompt recorded, outcome Approved.
    // (Exact fixture depends on test harness; reuse Phase 3 exec tests as template.)
}
```

- [ ] **Step 2.8: Run full suite**

Run: `cargo test -p alephcore`
Expected: baseline preserved + new tests green.

- [ ] **Step 2.9: Clippy + fmt + commit**

```bash
cargo clippy -- -D warnings && cargo fmt
git add src/approval/ src/exec/approval/channel_bridge.rs tests/exec_approval_e2e.rs <wiring site>
git commit -m "approval: bridge tool-level ApprovalRequester onto ChannelApprovalBridge (H1)"
```

---

## Task 3: H4 — Exec-Class Exclude-List on LayeredPermissionResolver

**Files:**
- Modify: `src/tools/middleware/permission/resolver.rs`
- Modify: `src/tools/middleware/permission/mod.rs` (PermissionLayer flow)
- Test: inline `#[cfg(test)]` in resolver.rs + integration test

**Context:** For exec-class tools under `Ask` policy, both `PermissionLayer` and `WorkspaceSandbox` currently call `ApprovalGate::request_approval_for_tool`, producing two prompts. Fix: exec-class tools bypass `PermissionLayer`'s approval call; only `WorkspaceSandbox` asks.

- [ ] **Step 3.1: Write failing unit test**

Append to `src/tools/middleware/permission/resolver.rs`:

```rust
#[cfg(test)]
mod exec_class_tests {
    use super::*;

    #[test]
    fn is_exec_class_recognizes_code_exec() {
        let resolver = LayeredPermissionResolver::from_merged(Default::default());
        assert!(resolver.is_exec_class("code_exec"));
        assert!(resolver.is_exec_class("shell"));
    }

    #[test]
    fn is_exec_class_rejects_read_only_tools() {
        let resolver = LayeredPermissionResolver::from_merged(Default::default());
        assert!(!resolver.is_exec_class("read_file"));
        assert!(!resolver.is_exec_class("list_dir"));
    }
}
```

- [ ] **Step 3.2: Run test (expect fail)**

Run: `cargo test --lib -p alephcore resolver::exec_class_tests`
Expected: compile error — `is_exec_class` not found.

- [ ] **Step 3.3: Add exec-class detection**

In `src/tools/middleware/permission/resolver.rs`, add:

```rust
/// Tools whose approval is owned downstream by WorkspaceSandbox.
/// PermissionLayer must NOT call ApprovalGate for these — doing so
/// would produce a duplicate prompt for the same action.
const EXEC_CLASS_TOOLS: &[&str] = &["code_exec", "shell"];

impl LayeredPermissionResolver {
    pub fn is_exec_class(&self, tool_name: &str) -> bool {
        EXEC_CLASS_TOOLS.contains(&tool_name)
    }
}
```

- [ ] **Step 3.4: Run unit test (expect pass)**

Run: `cargo test --lib -p alephcore resolver::exec_class_tests`
Expected: 2 passed.

- [ ] **Step 3.5: Wire exclude-list into PermissionLayer flow**

Find the site where `PermissionLayer` calls `ApprovalGate::request_approval_for_tool`:
```bash
grep -n "request_approval_for_tool\|ApprovalGate" src/tools/middleware/permission/
```

Modify the flow so that when `resolver.is_exec_class(tool_name)` is true and the policy is `Ask`, the `PermissionLayer` returns `Allow` (letting the call proceed) without invoking `ApprovalGate`. The downstream `WorkspaceSandbox` will then be the sole approval source.

Example shape (exact code depends on existing flow):
```rust
// Inside PermissionLayer::decide (or equivalent):
if resolver.is_exec_class(tool_name) && policy == Ask {
    // Exec-class tools: defer approval to Sandbox. Do not prompt here.
    return Decision::Allow;
}
// ... existing Ask-flow that calls ApprovalGate ...
```

- [ ] **Step 3.6: Write integration test — exec tool produces exactly one prompt**

Create `tests/exec_approval_single_prompt.rs`:

```rust
//! Regression: H4 — exec-class tools under Ask policy must produce
//! exactly ONE approval prompt (owned by WorkspaceSandbox), not two.

#[tokio::test]
async fn code_exec_under_ask_prompts_once() {
    // Fixture: ChannelApprovalBridge that counts prompts.
    // Set policy to Ask for code_exec.
    // Invoke code_exec.
    // Assert: counter == 1.
    // (Reuse Task 2's e2e test harness as the base; add a prompt-counter.)
}
```

- [ ] **Step 3.7: Run full suite**

Run: `cargo test -p alephcore`
Expected: baseline preserved + new tests green.

- [ ] **Step 3.8: Clippy + fmt + commit**

```bash
cargo clippy -- -D warnings && cargo fmt
git add src/tools/middleware/permission/ tests/exec_approval_single_prompt.rs
git commit -m "permission: exempt exec-class tools from PermissionLayer approval (H4)"
```

---

## Task 4: Extract Context Pre-Processing to ToolService Pre-LLM Middleware

**Files:**
- Create: `src/tools/middleware/pre_llm/mod.rs`
- Create: `src/tools/middleware/pre_llm/trait_def.rs`
- Move: `src/agent_loop/context_budget/` → `src/tools/middleware/pre_llm/budget/`
- Move: `src/agent_loop/compaction/` → `src/tools/middleware/pre_llm/compaction/`
- Move: `src/agent_loop/truncation_recovery.rs` → `src/tools/middleware/pre_llm/truncation.rs`
- Move: `src/agent_loop/sections/` → `src/tools/middleware/pre_llm/sections/`
- Modify: `src/agent_loop/loop_core.rs` (call middleware chain instead of inline)
- Test: unit tests at each new location + `tests/pre_llm_middleware.rs`

**Context:** These four concerns all transform messages before the LLM call. They belong to the ToolService's pre-LLM pipeline, not the loop driver. Extracting them shrinks `loop_core.rs` by ~1,700 lines and lets Phase 4b's thin Harness inherit them automatically.

- [ ] **Step 4.1: Introduce the minimal `PreLLMMiddleware` trait**

Create `src/tools/middleware/pre_llm/trait_def.rs`:

```rust
//! Pre-LLM middleware: transforms the prompt messages before generation.
//!
//! Keep the trait surface *tiny*. YAGNI. One method, one owned input, one
//! owned output. Do not generalize until a second concrete need appears.

use async_trait::async_trait;

use crate::providers::adapter::Message;

#[async_trait]
pub trait PreLLMMiddleware: Send + Sync {
    async fn transform(&self, messages: Vec<Message>) -> Vec<Message>;
}
```

Create `src/tools/middleware/pre_llm/mod.rs`:

```rust
pub mod trait_def;
pub use trait_def::PreLLMMiddleware;

pub mod budget;
pub mod compaction;
pub mod truncation;
pub mod sections;
```

- [ ] **Step 4.2: Write failing trait-shape test**

Create `tests/pre_llm_middleware.rs`:

```rust
use alephcore::providers::adapter::Message;
use alephcore::tools::middleware::pre_llm::PreLLMMiddleware;
use async_trait::async_trait;

struct Noop;

#[async_trait]
impl PreLLMMiddleware for Noop {
    async fn transform(&self, messages: Vec<Message>) -> Vec<Message> {
        messages
    }
}

#[tokio::test]
async fn noop_middleware_is_identity() {
    let noop = Noop;
    let msgs = vec![];
    let out = noop.transform(msgs.clone()).await;
    assert_eq!(out.len(), msgs.len());
}
```

- [ ] **Step 4.3: Run test (expect pass — trait + noop are enough)**

Run: `cargo test --test pre_llm_middleware`
Expected: 1 passed.

- [ ] **Step 4.4: Move `context_budget/` and wrap in the trait**

```bash
git mv src/agent_loop/context_budget src/tools/middleware/pre_llm/budget
```

In `src/tools/middleware/pre_llm/budget/mod.rs`, add an implementor:

```rust
// ... existing re-exports ...

use async_trait::async_trait;
use crate::providers::adapter::Message;
use crate::tools::middleware::pre_llm::PreLLMMiddleware;

pub struct BudgetMiddleware {
    // existing budget fields
}

#[async_trait]
impl PreLLMMiddleware for BudgetMiddleware {
    async fn transform(&self, messages: Vec<Message>) -> Vec<Message> {
        // Call into existing budget logic that produced the trimmed messages.
        // Keep internal functions pub(super) or pub(crate) as needed.
        self.apply(messages)
    }
}
```

Update `src/agent_loop/mod.rs` (remove `pub mod context_budget;`) and update the loop_core.rs import path to the new location.

- [ ] **Step 4.5: Run `cargo check` and fix all import breakages**

Run: `cargo check -p alephcore`
Expected: no errors. If breakages, update import paths mechanically.

- [ ] **Step 4.6: Repeat Steps 4.4–4.5 for `compaction/`, `truncation_recovery.rs`, `sections/`**

For each:
1. `git mv` to the new location.
2. Wrap its public entry point in a `*Middleware` struct that implements `PreLLMMiddleware`.
3. Update the old call site in `loop_core.rs` to call the middleware via the trait.
4. Remove the old module declaration from `src/agent_loop/mod.rs`.
5. `cargo check` after each one.

Rename mapping:
- `compaction/` → `pre_llm/compaction/` with `CompactionMiddleware`
- `truncation_recovery.rs` → `pre_llm/truncation.rs` with `TruncationMiddleware`
- `sections/` → `pre_llm/sections/` with `SectionsMiddleware` (skill prefetch etc.)

- [ ] **Step 4.7: Wire a middleware chain in `loop_core.rs`**

Replace the sequence of inline transforms with a chain:

```rust
let middlewares: Vec<Arc<dyn PreLLMMiddleware>> = vec![
    Arc::new(BudgetMiddleware::new(/* args from config */)),
    Arc::new(CompactionMiddleware::new(/* args */)),
    Arc::new(SectionsMiddleware::new(/* args */)),
    // TruncationMiddleware runs *after* the LLM response, not here — leave it wired where it currently is.
];

let mut msgs = initial_messages;
for mw in &middlewares {
    msgs = mw.transform(msgs).await;
}
```

Document (one-line comment) that truncation is post-LLM and stays at its current call site.

- [ ] **Step 4.8: Run full suite**

Run: `cargo test -p alephcore`
Expected: baseline preserved. Budget/compaction/sections integration tests that previously lived in `agent_loop` now live under their new paths; verify all still green.

- [ ] **Step 4.9: Clippy + fmt + commit**

```bash
cargo clippy -- -D warnings && cargo fmt
git add -A
git commit -m "tools: move context pre-processing to ToolService pre-LLM middleware"
```

---

## Task 5: Relocate `retry`, `safety`, `streaming_bridge` to Proper Homes

**Files:**
- Move: `src/agent_loop/retry.rs` → absorbed into `src/tools/middleware/retry.rs` (new) OR into `src/providers/adapter.rs` (if truly provider-layer)
- Move: `src/agent_loop/safety.rs` → `src/session/ingress_safety.rs` (new)
- Move: `src/agent_loop/streaming_bridge.rs` → `src/session/streaming.rs`
- Modify: `src/agent_loop/loop_core.rs` (update call sites)

**Context:** These three are not loop concerns. `retry` is either provider-level (LLM call retry) or tool-level (tool invocation retry); inspect to decide. `safety` is a Session ingress concern (hard-filter incoming user text before event append). `streaming_bridge` is event emit transport.

- [ ] **Step 5.1: Inspect `retry.rs` to decide destination**

Run:
```bash
head -80 src/agent_loop/retry.rs
```

Decision rule:
- If it retries `LlmProvider::complete` calls → new home: `src/providers/retry.rs`.
- If it retries `ToolService::execute` calls → new home: `src/tools/middleware/retry.rs`.
- If both → split the file.

Record the decision.

- [ ] **Step 5.2: Move `retry.rs` to the chosen home**

```bash
git mv src/agent_loop/retry.rs <new-path>
```

Update imports in `loop_core.rs` and elsewhere. Update `src/agent_loop/mod.rs` to remove the module.

If retry is a middleware, consider also making it a `PreLLMMiddleware` implementor OR a separate trait if retry needs to happen *around* the LLM call, not as a pre-transform. Retry *around* the call is typically a decorator over `LlmProvider`, not a message transform — so wrap `LlmProvider` with a `RetryingLlmProvider` struct rather than adding it to `PreLLMMiddleware`.

- [ ] **Step 5.3: Move `safety.rs` to `src/session/ingress_safety.rs`**

```bash
git mv src/agent_loop/safety.rs src/session/ingress_safety.rs
```

Expose a pub entry point `pub fn check_user_text(text: &str) -> SafetyDecision` (or whatever the existing API is) and call it from the SessionService's user-message ingress path, not from `loop_core.rs`.

Find the ingress site:
```bash
grep -rn "SessionEvent::UserMessage" src/session/ src/gateway/
```

Wire the call so every `UserMessage` event goes through `ingress_safety::check_user_text` *before* being appended to the session log. If the current architecture only has one ingress function, splice it there.

- [ ] **Step 5.4: Move `streaming_bridge.rs` to `src/session/streaming.rs`**

```bash
git mv src/agent_loop/streaming_bridge.rs src/session/streaming.rs
```

Update `src/session/mod.rs`:
```rust
pub mod streaming;
```

Update all `use alephcore::agent_loop::streaming_bridge::...` to `use alephcore::session::streaming::...`.

- [ ] **Step 5.5: Run `cargo check` and repair imports**

Run: `cargo check -p alephcore`
Expected: no errors.

- [ ] **Step 5.6: Ensure existing tests for these three modules still run at new locations**

Run:
```bash
cargo test --lib -p alephcore retry
cargo test --lib -p alephcore ingress_safety
cargo test --lib -p alephcore streaming
```
Expected: all existing tests pass at new module paths.

- [ ] **Step 5.7: Run full suite**

Run: `cargo test -p alephcore`
Expected: baseline preserved.

- [ ] **Step 5.8: Clippy + fmt + commit**

```bash
cargo clippy -- -D warnings && cargo fmt
git add -A
git commit -m "agent_loop: relocate retry/safety/streaming to proper homes"
```

---

## Task 6: Close Out Residue of `tool_pipeline` / `tool_orchestrator` / `tool_execution_context`

**Files:**
- Read: `src/agent_loop/tool_pipeline.rs` (1363 lines)
- Read: `src/agent_loop/tool_orchestrator.rs` (585 lines)
- Read: `src/agent_loop/tool_execution_context.rs` (148 lines)
- Likely-modify: `src/tools/facade.rs` (absorb what belongs in ToolService)
- Modify: `src/agent_loop/loop_core.rs` (update call sites to go through ToolService)

**Context:** Phase 2 moved much of tool dispatch into ToolService but left these three files with unabsorbed logic. This task audits each, merges what's ToolService-layer into `src/tools/`, and documents any remainder that cannot be merged until Phase 5/6.

- [ ] **Step 6.1: Read each file and classify responsibilities**

For each of the three files, write a one-paragraph summary in the commit message capturing:
- What the file does today.
- Which responsibilities belong in ToolService (move now).
- Which responsibilities belong in Orchestrator (defer to Phase 5).

Output this classification into `docs/reference/AGENT_LOOP_TOOL_EXECUTION.md` (existing doc; append a "Phase 4 residue audit" section).

- [ ] **Step 6.2: Move ToolService-layer code into `src/tools/`**

For any code that belongs in ToolService (e.g., input validation, schema enforcement, output normalization that isn't already in `src/tools/facade.rs`), move it with `git mv` or copy+adapt. Update call sites.

- [ ] **Step 6.3: Leave Orchestrator-layer code with a `// TODO(Phase 5)` marker**

For code that must wait for Phase 5 (Orchestrator), leave it in `src/agent_loop/` and mark each such block:
```rust
// TODO(Phase 5): move to Orchestrator. See docs/superpowers/specs/2026-04-18-managed-agents-refactor-roadmap.md §8 Phase 5.
```

One TODO comment per logical block; do not litter.

- [ ] **Step 6.4: Run full suite**

Run: `cargo test -p alephcore`
Expected: baseline preserved.

- [ ] **Step 6.5: Verify `loop_core.rs` has shrunk materially**

Run: `wc -l src/agent_loop/loop_core.rs`
Expected: under 1,500 lines (down from 4,559). If not, review what's still inlined and either move it or document why it stays for Phase 5.

- [ ] **Step 6.6: Clippy + fmt + commit**

```bash
cargo clippy -- -D warnings && cargo fmt
git add -A
git commit -m "agent_loop: close Phase 2 ToolService residue; mark remaining for Phase 5"
```

---

# Phase 4b — New Harness Module + Cut-Over (Tasks 7–12)

## Task 7: Harness Trait Skeleton

**Files:**
- Create: `src/harness/mod.rs`
- Create: `src/harness/trait_def.rs`
- Create: `src/harness/deps.rs`
- Create: `src/harness/agent.rs` (stub)
- Modify: `src/lib.rs` (add `pub mod harness;`)

**Context:** Define the `Harness` trait + `AgentHarness` skeleton. No logic yet — just the shapes that Tasks 8–9 fill in.

- [ ] **Step 7.1: Write failing trait-shape doc-test**

Create `src/harness/trait_def.rs`:

```rust
//! Harness: the Think→Act loop driver.
//!
//! Stateless; all state lives in `SessionService`. Dependencies injected
//! at construction. One call to `run_turn` produces one Think→Act cycle.
//!
//! ```
//! use alephcore::harness::{Harness, TurnState};
//! fn _assert_object_safe(_: Box<dyn Harness>) {}
//! ```

use async_trait::async_trait;

use crate::providers::adapter::LlmError;
use crate::session::service::{SessionError, SessionId};
use crate::tools::service::ToolError;

#[async_trait]
pub trait Harness: Send + Sync {
    /// One Think→Act turn; returns whether the session should continue.
    async fn run_turn(&self, session_id: &SessionId) -> Result<TurnState, HarnessError>;

    /// Loop `run_turn` until Done.
    async fn run(&self, session_id: &SessionId) -> Result<(), HarnessError> {
        loop {
            match self.run_turn(session_id).await? {
                TurnState::Continue => continue,
                TurnState::Done => return Ok(()),
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnState {
    Continue,
    Done,
}

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("llm error: {0}")]
    Llm(#[from] LlmError),
    #[error("tool error: {0}")]
    Tool(#[from] ToolError),
    #[error("session error: {0}")]
    Session(#[from] SessionError),
    #[error("cancelled")]
    Cancelled,
}
```

- [ ] **Step 7.2: Create `deps.rs`**

Create `src/harness/deps.rs`:

```rust
//! Harness dependency bundle — assembled once at startup.

use std::sync::Arc;

use crate::providers::adapter::LlmProvider;
use crate::sandbox::SandboxFactory;
use crate::session::service::SessionService;
use crate::tools::service::ToolService;

pub struct HarnessDeps {
    pub session: Arc<dyn SessionService>,
    pub tools: Arc<dyn ToolService>,
    pub sandbox_factory: Arc<dyn SandboxFactory>,
    pub llm: Arc<dyn LlmProvider>,
}
```

(If `SandboxFactory` trait does not yet exist at the exact path, check `src/sandbox/mod.rs` — Phase 3 introduced it. If the exact name differs, adjust the use path; do not create a shim.)

- [ ] **Step 7.3: Create stub `agent.rs`**

Create `src/harness/agent.rs`:

```rust
//! AgentHarness — the concrete Think→Act implementation.

use async_trait::async_trait;

use crate::harness::deps::HarnessDeps;
use crate::harness::trait_def::{Harness, HarnessError, TurnState};
use crate::session::service::SessionId;

pub struct AgentHarness {
    deps: HarnessDeps,
}

impl AgentHarness {
    pub fn new(deps: HarnessDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl Harness for AgentHarness {
    async fn run_turn(&self, _session_id: &SessionId) -> Result<TurnState, HarnessError> {
        todo!("Task 8: Think phase")
    }
}
```

- [ ] **Step 7.4: Create `mod.rs` and wire into `lib.rs`**

Create `src/harness/mod.rs`:

```rust
//! Harness — Anthropic-style Think→Act driver.
//!
//! Phase 4 of the managed-agents refactor.
//! Spec: docs/superpowers/specs/2026-04-19-harness-think-act-design.md

pub mod agent;
pub mod deps;
pub mod trait_def;

pub use agent::AgentHarness;
pub use deps::HarnessDeps;
pub use trait_def::{Harness, HarnessError, TurnState};
```

Add to `src/lib.rs`:
```rust
pub mod harness;
```

- [ ] **Step 7.5: Run `cargo check`**

Run: `cargo check -p alephcore`
Expected: no errors. Object-safety doc-test compiles.

- [ ] **Step 7.6: Run `cargo test --doc`**

Run: `cargo test --doc -p alephcore harness`
Expected: 1 doc-test passes.

- [ ] **Step 7.7: Clippy + fmt + commit**

```bash
cargo clippy -- -D warnings && cargo fmt
git add src/harness src/lib.rs
git commit -m "harness: scaffold Harness trait + AgentHarness stub (Phase 4b.1)"
```

---

## Task 8: AgentHarness Think Phase

**Files:**
- Modify: `src/harness/agent.rs`
- Create: `src/harness/tests/think.rs`
- Modify: `src/harness/mod.rs` (export tests module under `#[cfg(test)]`)

**Context:** Implement the Think half of `run_turn`: fetch tail of session events since the last assistant message, build LLM prompt, call LLM, emit AssistantMessage, return `Done` if no tool_use.

- [ ] **Step 8.1: Add helper for tail slicing**

In `src/harness/agent.rs`, add a private helper:

```rust
use crate::session::events::{SessionEvent, SessionEventRecord};

/// Returns events strictly *after* the last `AssistantMessage`.
/// If there is no prior assistant message, returns all events.
fn tail_since_last_assistant(records: &[SessionEventRecord]) -> &[SessionEventRecord] {
    let last_idx = records
        .iter()
        .rposition(|r| matches!(r.event, SessionEvent::AssistantMessage { .. }));
    match last_idx {
        Some(i) => &records[i + 1..],
        None => records,
    }
}
```

- [ ] **Step 8.2: Write failing Think unit test**

Create `src/harness/tests/think.rs`:

```rust
//! Think phase unit tests — mocks SessionService + LlmProvider.

use std::sync::Arc;

use async_trait::async_trait;

use crate::harness::{AgentHarness, Harness, HarnessDeps, TurnState};
use crate::providers::adapter::{LlmError, LlmProvider, ProviderRequest, ProviderResponse};
use crate::session::events::{MessageContent, SessionEvent, SessionEventRecord, TurnId};
use crate::session::service::{SessionError, SessionHandle, SessionId, SessionService};
use crate::tools::service::{ToolDefinition, ToolError, ToolOutput, ToolService};

// ---------- mocks ----------

struct MockSession {
    records: Arc<parking_lot::Mutex<Vec<SessionEventRecord>>>,
}

#[async_trait]
impl SessionService for MockSession {
    async fn attach(&self, id: SessionId) -> Result<SessionHandle, SessionError> { /* ... */ }
    async fn get_events(&self, _id: &SessionId, _from: Option<u64>, _to: Option<u64>)
        -> Result<Vec<SessionEventRecord>, SessionError>
    {
        Ok(self.records.lock().clone())
    }
    async fn emit_event(&self, _id: &SessionId, event: SessionEvent) -> Result<u64, SessionError> {
        let mut v = self.records.lock();
        let seq = v.len() as u64;
        v.push(SessionEventRecord { seq, event });
        Ok(seq)
    }
    async fn subscribe(&self, _id: &SessionId)
        -> Result<tokio::sync::broadcast::Receiver<SessionEventRecord>, SessionError>
    { unimplemented!() }
    async fn wake(&self, _id: &SessionId) -> Result<SessionHandle, SessionError> { unimplemented!() }
    async fn detach(&self, _id: &SessionId) -> Result<(), SessionError> { Ok(()) }
}

struct MockLlmNoTools;

#[async_trait]
impl LlmProvider for MockLlmNoTools {
    async fn complete(&self, _req: ProviderRequest) -> Result<ProviderResponse, LlmError> {
        Ok(ProviderResponse { text: "hi".into(), tool_uses: vec![], /* ... */ })
    }
}

struct MockToolsEmpty;

#[async_trait]
impl ToolService for MockToolsEmpty {
    async fn execute(&self, _n: &str, _i: serde_json::Value) -> Result<ToolOutput, ToolError> {
        unreachable!()
    }
    async fn list(&self) -> Vec<ToolDefinition> { vec![] }
    async fn describe(&self, _n: &str) -> Option<ToolDefinition> { None }
}

// ---------- test ----------

#[tokio::test]
async fn think_with_no_tool_use_returns_done() {
    let session = Arc::new(MockSession { records: Arc::new(parking_lot::Mutex::new(vec![])) });
    let tools: Arc<dyn ToolService> = Arc::new(MockToolsEmpty);
    let llm: Arc<dyn LlmProvider> = Arc::new(MockLlmNoTools);
    // sandbox_factory: use an existing in-process stub from Phase 3.

    let deps = HarnessDeps { session: session.clone(), tools, sandbox_factory: todo_phase3_factory(), llm };
    let harness = AgentHarness::new(deps);

    let sid = SessionId::from("t-session");
    let outcome = harness.run_turn(&sid).await.expect("run_turn ok");

    assert_eq!(outcome, TurnState::Done);
    let records = session.records.lock();
    assert!(records.iter().any(|r| matches!(r.event, SessionEvent::AssistantMessage { .. })));
}

fn todo_phase3_factory() -> Arc<dyn crate::sandbox::SandboxFactory> {
    // Use the Phase 3 in-process factory helper (check src/sandbox/ for the existing test helper).
    unimplemented!("Wire to Phase 3 test factory")
}
```

Register in `src/harness/mod.rs`:
```rust
#[cfg(test)]
mod tests {
    mod think;
}
```

- [ ] **Step 8.3: Run test (expect compile fail — AgentHarness still has `todo!()`)**

Run: `cargo test --lib -p alephcore harness::tests::think`
Expected: compile succeeds, test panics at `todo!()`.

- [ ] **Step 8.4: Implement Think phase**

Replace `todo!()` in `src/harness/agent.rs`:

```rust
use crate::session::events::{MessageContent, SessionEvent};

#[async_trait]
impl Harness for AgentHarness {
    async fn run_turn(&self, session_id: &SessionId) -> Result<TurnState, HarnessError> {
        // 1. Fetch event tail since last assistant message.
        let all = self.deps.session.get_events(session_id, None, None).await?;
        let tail = tail_since_last_assistant(&all);

        // 2. Build LLM prompt from tail. `build_prompt` lives alongside.
        let request = build_prompt(tail);

        // 3. Call LLM.
        let response = self.deps.llm.complete(request).await?;

        // 4. Emit AssistantMessage.
        let turn_id = current_turn_id(&all);
        let at = now_ms();
        self.deps
            .session
            .emit_event(
                session_id,
                SessionEvent::AssistantMessage {
                    turn_id,
                    content: MessageContent { text: response.text.clone(), blocks: vec![] },
                    at,
                },
            )
            .await?;

        // 5. If no tool_use, Done.
        if response.tool_uses.is_empty() {
            return Ok(TurnState::Done);
        }

        // Act phase implemented in Task 9.
        self.act(session_id, turn_id, response.tool_uses).await?;
        Ok(TurnState::Continue)
    }
}

fn build_prompt(tail: &[SessionEventRecord]) -> ProviderRequest {
    // Convert tail → messages. Middleware (Task 4) transforms them *downstream*
    // inside the LLM provider wrapper or ToolService pre-LLM pipeline, not here.
    // Here we only do the dumb shape transform.
    let messages: Vec<Message> = tail
        .iter()
        .filter_map(|r| match &r.event {
            SessionEvent::UserMessage { content, .. } => Some(Message::user(&content.text)),
            SessionEvent::ToolResult { output, .. } => Some(Message::tool_result_json(&output.value)),
            _ => None,
        })
        .collect();
    ProviderRequest { messages, /* other fields default */ }
}

fn current_turn_id(records: &[SessionEventRecord]) -> TurnId {
    records
        .iter()
        .rev()
        .find_map(|r| match &r.event {
            SessionEvent::TurnStarted { turn_id, .. } => Some(*turn_id),
            _ => None,
        })
        .unwrap_or_else(uuid::Uuid::new_v4)
}

fn now_ms() -> crate::session::events::Timestamp {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}
```

(`act` is defined in Task 9; for this task stub it as `async fn act(&self, _sid: &SessionId, _turn: TurnId, _calls: Vec<ToolCall>) -> Result<(), HarnessError> { Ok(()) }`.)

- [ ] **Step 8.5: Run test (expect pass)**

Run: `cargo test --lib -p alephcore harness::tests::think`
Expected: 1 passed.

- [ ] **Step 8.6: Add negative-path tests**

Append to `think.rs`:

```rust
struct MockLlmError;

#[async_trait]
impl LlmProvider for MockLlmError {
    async fn complete(&self, _req: ProviderRequest) -> Result<ProviderResponse, LlmError> {
        Err(LlmError::Transport("simulated".into()))
    }
}

#[tokio::test]
async fn think_llm_error_maps_to_harness_llm() {
    let session = Arc::new(MockSession { records: Arc::new(parking_lot::Mutex::new(vec![])) });
    let tools: Arc<dyn ToolService> = Arc::new(MockToolsEmpty);
    let llm: Arc<dyn LlmProvider> = Arc::new(MockLlmError);
    let deps = HarnessDeps { session, tools, sandbox_factory: todo_phase3_factory(), llm };
    let harness = AgentHarness::new(deps);
    let err = harness.run_turn(&SessionId::from("x")).await.unwrap_err();
    assert!(matches!(err, crate::harness::HarnessError::Llm(_)));
}
```

- [ ] **Step 8.7: Run tests (expect pass)**

Run: `cargo test --lib -p alephcore harness::tests::think`
Expected: 2 passed.

- [ ] **Step 8.8: Full suite + clippy + fmt + commit**

```bash
cargo test -p alephcore
cargo clippy -- -D warnings && cargo fmt
git add src/harness
git commit -m "harness: implement Think phase (4b.2)"
```

---

## Task 9: AgentHarness Act Phase

**Files:**
- Modify: `src/harness/agent.rs` (implement `act`)
- Create: `src/harness/tests/act.rs`

**Context:** Implement the Act half: iterate `Vec<ToolCall>` sequentially, call `ToolService::execute`, emit `ToolResult` or `ToolError` per call, map errors to `HarnessError::Tool`.

- [ ] **Step 9.1: Write failing test — sequential tool execution**

Create `src/harness/tests/act.rs`:

```rust
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;

use crate::harness::{AgentHarness, Harness, HarnessDeps, HarnessError, TurnState};
// (reuse MockSession + MockLlm from think.rs — either cfg(test) pub or duplicate shortly)
use super::think::{MockSession, todo_phase3_factory};
use crate::providers::adapter::{LlmError, LlmProvider, ProviderRequest, ProviderResponse, ToolCall};
use crate::session::events::SessionEvent;
use crate::session::service::SessionId;
use crate::tools::service::{ToolDefinition, ToolError, ToolOutput, ToolService};

struct MockLlmTwoTools;
#[async_trait]
impl LlmProvider for MockLlmTwoTools {
    async fn complete(&self, _req: ProviderRequest) -> Result<ProviderResponse, LlmError> {
        Ok(ProviderResponse {
            text: "using tools".into(),
            tool_uses: vec![
                ToolCall { call_id: "c1".into(), name: "read_file".into(), input: serde_json::json!({"p": "/a"}) },
                ToolCall { call_id: "c2".into(), name: "read_file".into(), input: serde_json::json!({"p": "/b"}) },
            ],
        })
    }
}

struct MockTools { log: Arc<Mutex<Vec<String>>> }
#[async_trait]
impl ToolService for MockTools {
    async fn execute(&self, name: &str, input: serde_json::Value) -> Result<ToolOutput, ToolError> {
        self.log.lock().push(format!("{name}:{input}"));
        Ok(ToolOutput { value: serde_json::json!({"ok": true}), metadata: Default::default() })
    }
    async fn list(&self) -> Vec<ToolDefinition> { vec![] }
    async fn describe(&self, _n: &str) -> Option<ToolDefinition> { None }
}

#[tokio::test]
async fn act_executes_tools_sequentially() {
    let session = Arc::new(MockSession { records: Arc::new(Mutex::new(vec![])) });
    let log = Arc::new(Mutex::new(vec![]));
    let tools: Arc<dyn ToolService> = Arc::new(MockTools { log: log.clone() });
    let llm: Arc<dyn LlmProvider> = Arc::new(MockLlmTwoTools);
    let deps = HarnessDeps { session: session.clone(), tools, sandbox_factory: todo_phase3_factory(), llm };
    let harness = AgentHarness::new(deps);

    let outcome = harness.run_turn(&SessionId::from("s")).await.unwrap();
    assert_eq!(outcome, TurnState::Continue);

    let log = log.lock();
    assert_eq!(log.len(), 2);
    assert!(log[0].starts_with("read_file:"));
    assert!(log[1].starts_with("read_file:"));

    let events = session.records.lock();
    let tool_results = events.iter().filter(|r| matches!(r.event, SessionEvent::ToolResult { .. })).count();
    assert_eq!(tool_results, 2);
}
```

Register in `src/harness/mod.rs`:
```rust
#[cfg(test)]
mod tests {
    pub mod think;
    pub mod act;
}
```

- [ ] **Step 9.2: Run test (expect fail — `act` stub emits nothing)**

Run: `cargo test --lib -p alephcore harness::tests::act`
Expected: assertion failures — `log.len()` is 0.

- [ ] **Step 9.3: Implement `act`**

Replace the stub in `src/harness/agent.rs`:

```rust
use crate::providers::adapter::ToolCall;

impl AgentHarness {
    async fn act(
        &self,
        session_id: &SessionId,
        turn_id: TurnId,
        tool_calls: Vec<ToolCall>,
    ) -> Result<(), HarnessError> {
        // Sequential execution. Vec<ToolCall> signature preserves the future
        // concurrency seam; we do not parallelize today (see spec §8).
        for call in tool_calls {
            self.deps
                .session
                .emit_event(
                    session_id,
                    SessionEvent::ToolCallRequested {
                        turn_id,
                        call_id: call.call_id.clone(),
                        name: call.name.clone(),
                        input: call.input.clone(),
                        at: now_ms(),
                    },
                )
                .await?;

            match self.deps.tools.execute(&call.name, call.input).await {
                Ok(output) => {
                    self.deps
                        .session
                        .emit_event(
                            session_id,
                            SessionEvent::ToolResult {
                                turn_id,
                                call_id: call.call_id,
                                output,
                                at: now_ms(),
                            },
                        )
                        .await?;
                }
                Err(e) => {
                    let msg = e.to_string();
                    self.deps
                        .session
                        .emit_event(
                            session_id,
                            SessionEvent::ToolError {
                                turn_id,
                                call_id: call.call_id,
                                error: msg,
                                at: now_ms(),
                            },
                        )
                        .await?;
                    return Err(HarnessError::Tool(e));
                }
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 9.4: Run test (expect pass)**

Run: `cargo test --lib -p alephcore harness::tests::act`
Expected: 1 passed.

- [ ] **Step 9.5: Add negative-path test — tool failure classifies to HarnessError::Tool**

Append to `act.rs`:

```rust
struct MockToolsFail;
#[async_trait]
impl ToolService for MockToolsFail {
    async fn execute(&self, n: &str, _i: serde_json::Value) -> Result<ToolOutput, ToolError> {
        Err(ToolError::Execution { name: n.into(), cause: "boom".into() })
    }
    async fn list(&self) -> Vec<ToolDefinition> { vec![] }
    async fn describe(&self, _n: &str) -> Option<ToolDefinition> { None }
}

#[tokio::test]
async fn act_tool_failure_returns_harness_tool_error() {
    let session = Arc::new(MockSession { records: Arc::new(Mutex::new(vec![])) });
    let tools: Arc<dyn ToolService> = Arc::new(MockToolsFail);
    let llm: Arc<dyn LlmProvider> = Arc::new(MockLlmTwoTools);
    let deps = HarnessDeps { session: session.clone(), tools, sandbox_factory: todo_phase3_factory(), llm };
    let harness = AgentHarness::new(deps);

    let err = harness.run_turn(&SessionId::from("s")).await.unwrap_err();
    assert!(matches!(err, HarnessError::Tool(_)));

    let events = session.records.lock();
    assert!(events.iter().any(|r| matches!(r.event, SessionEvent::ToolError { .. })));
}
```

- [ ] **Step 9.6: Run tests (expect pass)**

Run: `cargo test --lib -p alephcore harness::tests::act`
Expected: 2 passed.

- [ ] **Step 9.7: Full suite + clippy + fmt + commit**

```bash
cargo test -p alephcore
cargo clippy -- -D warnings && cargo fmt
git add src/harness
git commit -m "harness: implement Act phase with sequential tool execution (4b.3)"
```

---

## Task 10: `run` Loop Integration Test

**Files:**
- Create: `src/harness/tests/run.rs`
- Create: `tests/harness_run_e2e.rs`

**Context:** `Harness::run` is defined via default impl on the trait. Verify end-to-end that `run` loops correctly until `Done` using a scripted LLM.

- [ ] **Step 10.1: Write integration test using real SessionService + scripted LLM**

Create `tests/harness_run_e2e.rs`:

```rust
//! End-to-end Harness run test.
//! Uses real InProcessSessionService + scripted LLM + no-op ToolService.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;

use alephcore::harness::{AgentHarness, Harness, HarnessDeps};
use alephcore::providers::adapter::{LlmError, LlmProvider, ProviderRequest, ProviderResponse, ToolCall};
use alephcore::session::events::SessionEvent;
use alephcore::session::in_process::InProcessSessionService;
use alephcore::session::service::{SessionId, SessionService};
use alephcore::tools::service::{ToolDefinition, ToolError, ToolOutput, ToolService};

/// Scripted LLM: returns one tool_use, then no tool_use → Done.
struct Scripted { step: Arc<Mutex<u32>> }

#[async_trait]
impl LlmProvider for Scripted {
    async fn complete(&self, _req: ProviderRequest) -> Result<ProviderResponse, LlmError> {
        let mut step = self.step.lock();
        let resp = match *step {
            0 => ProviderResponse {
                text: "first turn".into(),
                tool_uses: vec![ToolCall { call_id: "c".into(), name: "noop".into(), input: serde_json::json!({}) }],
            },
            _ => ProviderResponse { text: "done".into(), tool_uses: vec![] },
        };
        *step += 1;
        Ok(resp)
    }
}

struct NoopTool;
#[async_trait]
impl ToolService for NoopTool {
    async fn execute(&self, _n: &str, _i: serde_json::Value) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput { value: serde_json::json!({}), metadata: Default::default() })
    }
    async fn list(&self) -> Vec<ToolDefinition> { vec![] }
    async fn describe(&self, _n: &str) -> Option<ToolDefinition> { None }
}

#[tokio::test]
async fn harness_run_loops_until_done() {
    let session: Arc<dyn SessionService> = Arc::new(InProcessSessionService::new_for_test());
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTool),
        sandbox_factory: alephcore::sandbox::test_factory(),
        llm: Arc::new(Scripted { step: Arc::new(Mutex::new(0)) }),
    };
    let harness = AgentHarness::new(deps);

    let sid = SessionId::from("e2e-1");
    session.attach(sid.clone()).await.unwrap();

    harness.run(&sid).await.expect("run completes");

    let events = session.get_events(&sid, None, None).await.unwrap();
    let assistant = events.iter().filter(|r| matches!(r.event, SessionEvent::AssistantMessage { .. })).count();
    assert_eq!(assistant, 2, "expected two assistant messages (one per turn)");
    let tool_results = events.iter().filter(|r| matches!(r.event, SessionEvent::ToolResult { .. })).count();
    assert_eq!(tool_results, 1);
}
```

(`InProcessSessionService::new_for_test` and `alephcore::sandbox::test_factory` are helpers from Phase 1 and Phase 3 respectively. If the exact names differ, grep and adapt.)

- [ ] **Step 10.2: Run test (expect pass)**

Run: `cargo test --test harness_run_e2e`
Expected: 1 passed.

- [ ] **Step 10.3: Add multi-session isolation test**

Append to `tests/harness_run_e2e.rs`:

```rust
#[tokio::test]
async fn multiple_sessions_do_not_cross_contaminate() {
    let session: Arc<dyn SessionService> = Arc::new(InProcessSessionService::new_for_test());
    let deps = HarnessDeps {
        session: session.clone(),
        tools: Arc::new(NoopTool),
        sandbox_factory: alephcore::sandbox::test_factory(),
        llm: Arc::new(Scripted { step: Arc::new(Mutex::new(0)) }),
    };
    let harness = Arc::new(AgentHarness::new(deps));

    let sid_a = SessionId::from("sess-A");
    let sid_b = SessionId::from("sess-B");
    session.attach(sid_a.clone()).await.unwrap();
    session.attach(sid_b.clone()).await.unwrap();

    harness.run(&sid_a).await.unwrap();
    harness.run(&sid_b).await.unwrap();

    let a_events = session.get_events(&sid_a, None, None).await.unwrap();
    let b_events = session.get_events(&sid_b, None, None).await.unwrap();
    assert!(!a_events.is_empty());
    assert!(!b_events.is_empty());
    // Session A's events do not mention session B and vice versa.
    for rec in &a_events {
        let j = serde_json::to_string(&rec.event).unwrap();
        assert!(!j.contains("sess-B"));
    }
}
```

- [ ] **Step 10.4: Run tests (expect pass)**

Run: `cargo test --test harness_run_e2e`
Expected: 2 passed.

- [ ] **Step 10.5: Full suite + clippy + fmt + commit**

```bash
cargo test -p alephcore
cargo clippy -- -D warnings && cargo fmt
git add src/harness tests/harness_run_e2e.rs
git commit -m "harness: integration test for run loop + multi-session isolation (4b.4)"
```

---

## Task 11: AppContext Env Var Wiring — `ALEPH_HARNESS_V2`

**Files:**
- Modify: `src/app_context.rs` (or wherever the driver is assembled — grep below)
- Modify: `CHANGELOG.md` (add entry to unreleased section)

**Context:** At startup, read `ALEPH_HARNESS_V2`. If `1`/`true`, construct `AgentHarness` and use it as the session driver. Else use the old `agent_loop` path. Both paths share every downstream dependency.

- [ ] **Step 11.1: Locate the session driver assembly point**

Run:
```bash
grep -rn "agent_loop::loop_core\|AgentLoop::new\|loop_core::run" src/ --include='*.rs' | head
```
Expected: one or two sites in `src/app_context.rs` or `src/bin/aleph-server/`. Record the path.

- [ ] **Step 11.2: Add the env var toggle**

At the assembly site, replace:
```rust
// OLD
let driver = agent_loop::loop_core::AgentLoop::new(/* ... */);
```
with:
```rust
// NEW
let use_v2 = std::env::var("ALEPH_HARNESS_V2")
    .ok()
    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    .unwrap_or(false);

let driver: Arc<dyn SessionDriver> = if use_v2 {
    tracing::info!("ALEPH_HARNESS_V2=1; using new AgentHarness");
    Arc::new(alephcore::harness::AgentHarness::new(HarnessDeps {
        session: session_svc.clone(),
        tools: tool_svc.clone(),
        sandbox_factory: sandbox_factory.clone(),
        llm: llm_provider.clone(),
    }))
} else {
    Arc::new(agent_loop::loop_core::AgentLoop::new(/* same args as before */))
};
```

If `SessionDriver` trait does not exist today, introduce it as the minimum seam:

```rust
// src/session/driver.rs
#[async_trait]
pub trait SessionDriver: Send + Sync {
    async fn drive(&self, session_id: &SessionId) -> anyhow::Result<()>;
}
```

Implement `SessionDriver for AgentHarness` (delegate to `Harness::run`) and `SessionDriver for AgentLoop` (delegate to whatever method drives the session today). Keep the trait pub(crate).

- [ ] **Step 11.3: Verify `cargo check` passes under both configurations**

Run:
```bash
cargo check -p alephcore
ALEPH_HARNESS_V2=0 cargo check -p alephcore
ALEPH_HARNESS_V2=1 cargo check -p alephcore
```
Expected: all three pass (env var is runtime; cargo check is identical — the variation is purely for documentation that the code path compiles).

- [ ] **Step 11.4: Run tests under both env settings**

Run:
```bash
cargo test -p alephcore
ALEPH_HARNESS_V2=1 cargo test -p alephcore
```
Expected: **both** match baseline (9059 pass / 2 known-fail / 20 ignored). If v2 has extra tests (from Tasks 7–10), count is higher — confirm pass count increases only; no new failures.

- [ ] **Step 11.5: Add CHANGELOG entry**

In `CHANGELOG.md`, under the next unreleased version section (create one if none exists), add:

```markdown
### Added
- New Think→Act harness available behind `ALEPH_HARNESS_V2=1` env var (opt-in). Default remains the legacy `agent_loop` driver; next release will flip the default.
```

Do **not** bump VERSION; do **not** run `just release`.

- [ ] **Step 11.6: Clippy + fmt + commit**

```bash
cargo clippy -- -D warnings && cargo fmt
git add src/ CHANGELOG.md
git commit -m "harness: wire AppContext to select v2 via ALEPH_HARNESS_V2 env var (4b.5)"
```

---

## Task 12: Manual End-to-End + Hand-Off to User

**Files:**
- No code changes unless bugs surface.
- Possible: bugfix commits if manual testing reveals issues.

**Context:** Run a real binary with `ALEPH_HARNESS_V2=1` against the usual dev flows and verify nothing regresses. Collect notes; if bugs appear, fix each as its own small commit.

- [x] **Step 12.1: Kill any running aleph processes before starting**

Run:
```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
ps aux | grep "[a]leph-server" | grep -v zsh | grep -v cp | grep -v tail
```
Expected: no aleph-server processes listed.

- [x] **Step 12.2: Build release binary**

Run: `cargo build --release --bin aleph-server`
Expected: build succeeds.

- [x] **Step 12.3: Start with v2 enabled**

Run: `ALEPH_HARNESS_V2=1 target/release/aleph-server start`
Expected: server starts; logs show `ALEPH_HARNESS_V2=1; using new AgentHarness`.

- [x] **Step 12.4: Exercise chat flow**

Send a simple chat message via CLI (`aleph chat "hello, what is 2+2?"`) or the configured UI. Verify:
- Response returns.
- Session event log contains `AssistantMessage`.
- No `HarnessError` in logs.

- [x] **Step 12.5: Exercise tool-using flow**

Ask the model to use a tool (e.g., "list files in ~"). Verify:
- Tool is invoked.
- Session log contains `ToolCallRequested` + `ToolResult`.
- Response returned after tool result.

- [x] **Step 12.6: Exercise cron path**

If a cron job is configured, let one fire (or trigger manually). Verify:
- No "no active session context" errors (H2 regression check).
- Tool invoked successfully.

- [x] **Step 12.7: Exercise exec-class approval flow**

Ask the model to run a shell command. Verify:
- Exactly **one** approval prompt appears (H4 regression check).
- Prompt shape is readable (H1 regression check).

- [x] **Step 12.8: Capture findings**

Create `docs/superpowers/plans/2026-04-19-managed-agents-phase-4-harness-manual-e2e-notes.md` with:

```markdown
# Phase 4 Manual E2E Notes — YYYY-MM-DD

## Environment
- Binary: target/release/aleph-server (commit SHA)
- Env: ALEPH_HARNESS_V2=1

## Scenarios
| Scenario | Result | Notes |
|---|---|---|
| Chat: hello | PASS/FAIL | ... |
| Tool use: ls | PASS/FAIL | ... |
| Cron path | PASS/FAIL | ... |
| Exec approval | PASS/FAIL | ... |

## Bugs Discovered
(None / list each with commit fixing it)

## Decision
- [ ] Ready to recommend flipping default to v2 next release
- [ ] Bugs found — addressed in follow-up commits; re-run E2E
```

Fill in rows as you test.

- [x] **Step 12.9: For each bug found, make a minimal fix commit**

If bugs appear:
- Write a failing unit/integration test first.
- Fix the code.
- Verify test passes.
- Commit: `git commit -m "harness: fix <bug summary>"`

- [x] **Step 12.10: Kill aleph-server after testing**

Run:
```bash
pkill -f "target/release/aleph-server" 2>/dev/null
sleep 2
ps aux | grep "[a]leph-server" | grep -v zsh | grep -v cp | grep -v tail
```
Expected: clean.

- [x] **Step 12.11: Commit the notes**

```bash
git add docs/superpowers/plans/2026-04-19-managed-agents-phase-4-harness-manual-e2e-notes.md
git commit -m "docs: Phase 4 manual E2E notes"
```

- [x] **Step 12.12: Stop and ask the user**

**DO NOT run `just release`.** Report to the user:
- Phase 4a + 4b merged to main. Tasks 1–12 complete.
- Pre-existing test baseline preserved under both `ALEPH_HARNESS_V2=0` and `ALEPH_HARNESS_V2=1`.
- `loop_core.rs` shrunk from 4559 → <1500 lines.
- `src/harness/` total LOC within budget.
- Manual E2E results summary.
- Ask: "Ready to release YYYY.MM.DD? Or keep accumulating changes before cutting?"

---

## Self-Review

### Spec Coverage

| Spec section | Task(s) |
|---|---|
| §1 Goal — shrink agent_loop, introduce Harness | Tasks 4–6 (4a), 7–11 (4b) |
| §4.1 Phase 4a relocation table | Tasks 1–6 |
| §4.2 Phase 4b new Harness + cut-over | Tasks 7–11 |
| §5 Module layout | Task 7 |
| §6 Harness trait | Task 7 |
| §7 AgentHarness impl | Tasks 7, 8, 9 |
| §8 Concurrent tool calls deferred (Vec<ToolCall> seam) | Task 9 Step 9.3 |
| §9 No event schema changes | Tasks 8–9 use existing variants |
| §10 H1 / H2 / H4 detail | Tasks 2 / 1 / 3 |
| §11 PR slicing 4a | Tasks 1–6 |
| §12 PR slicing 4b | Tasks 7–12 |
| §13 Testing strategy | Task-embedded tests + Task 10 integration |
| §14 Risks (behavioral drift, cut-over divergence) | Task 11 Step 11.4 matrix + Task 12 manual |
| §15 Open questions (Q15.1 build_prompt, Q15.3 tail helper) | Task 8 Step 8.1 (tail helper internal to harness module) |
| §16 Success criteria | Task 11 Step 11.4 + Task 12 Step 12.8 |

### Placeholder Scan

- `todo_phase3_factory()` in Task 8 test code — this is a real placeholder, but the accompanying comment instructs the engineer to wire to the Phase 3 in-process factory helper that already exists. This is a wiring step, not a TODO left for later.
- `TODO(Phase 5)` markers in Task 6 Step 6.3 — intentional; documented forward-references to Phase 5, not unfinished work in Phase 4.
- No other `TBD`, `TODO`, `implement later`, or vague prose.

### Type Consistency

- `HarnessDeps` has fields `session`, `tools`, `sandbox_factory`, `llm` consistently in Tasks 7, 8, 9, 10, 11.
- `AgentHarness::new(deps: HarnessDeps)` consistent across tasks.
- `TurnState::{Continue, Done}` consistent.
- `HarnessError::{Llm, Tool, Session, Cancelled}` consistent and used consistently in tests.
- `tail_since_last_assistant(records: &[SessionEventRecord]) -> &[SessionEventRecord]` defined once (Task 8 Step 8.1), used internally.
- `SessionService::emit_event` (not `emit`) — matches the real trait surface shown in Phase 1's `src/session/service.rs`.
- `SessionService::get_events(id, from, to)` with `from, to: Option<EventSeq>` — matches real trait.
