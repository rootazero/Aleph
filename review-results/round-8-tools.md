# Logic Review Report — src/tools
**Module**: src/tools
**Scope**: full module (68 .rs files)
**Date**: 2026-08-29
**Mode**: strict

## Findings

### [Critical] `ToolError::Cancelled` silently overwrites a real tool error after cancel
- **Location**: `src/tools/scoped/dispatch.rs:1158-1170` (`Err(_) if cancel.is_cancelled() => Err(ToolError::Cancelled { .. })`)
- **Trigger condition**: A tool returns a `ToolError::Execution` (or `Transport`/`Validation`) for a real failure reason (e.g. exit-1, network blip). The harness's per-call `CancellationToken` happens to be fired at the *same moment* (e.g. a sibling task dropped the future and propagated cancel), so `cancel.is_cancelled()` returns `true` on the very line we read the result.
- **Expected behavior**: The model and audit trail see the real reason for the failure so it can route around it. `ToolError::Cancelled` is reserved for "the run was stopped" with no judgment on the call.
- **Actual behavior**: The real error is replaced wholesale with `ToolError::Cancelled { name }` because the cancel check is performed after the tool result is in hand. The kind classifier (`classify_tool_error`) then maps it to `ToolErrorKind::Cancelled`, and the model is told "the run was stopped" about a call that actually had a network failure. The downstream consequence is two-fold: (1) the harness's cross-batch failure memo (`is_retryable` → false) bans the call for the rest of the run for the wrong reason, and (2) the persistence hint renders empty (`render_persistence_hint` returns "" for `Cancelled`), so no ladder-climbing suggestion reaches the model.
- **Suggested fix**: Distinguish "the tool returned its own error AND the cancel was signalled" from "the cancel fired and we never got a real result". Only rewrite to `Cancelled` when the result itself is missing/cancelled-shaped. The minimal change is to also gate on the variant of the incoming `err` (e.g. only collapse `ToolError::Execution { cause: "… cancelled", .. }` from `RegistryToolAdapter::execute`, which is the only signal a tool surfaces when it sees cancel mid-execution).

### [Critical] `execute_inner` computes `effective_exec_tier()` outside the dispatch future — `PlanGate` flip on a mid-call scratchpad handoff sees the wrong tier
- **Location**: `src/tools/scoped/mod.rs:374-410` (`let tier_at_dispatch = self.effective_exec_tier();` captured before `async move`, then re-applied via `TURN_EXEC_TIER.scope(...)`)
- **Trigger condition**: A plan-mode turn invokes a `scratchpad` call which itself hits the `confirm_with_memory` path (a `requires_confirmation` floor). Inside that nested confirm, the tool body fires `PlanGate::release()`. By the time the inner `execute_inner` future runs the *next* call's tier would be the restore tier, but the *current* dispatch's `TURN_EXEC_TIER` was snapshotted before the flip — so any `approval::lift_ask_under_full_tier` decision made during the current call still sees `Plan` instead of `Ask`/`Auto`/`Full`.
- **Expected behavior**: The `TURN_EXEC_TIER` task-local reflects the live tier at the time of the dispatch's `lift_ask_under_full_tier` read.
- **Actual behavior**: `tier_at_dispatch` is a one-shot read; the plan-gate's `AtomicBool` is re-read only at the next call's `effective_exec_tier()`. A lift that depends on the tier flips made *during* this dispatch (rare but real for `scratchpad` chains) will use the stale value. The tier atomics use `AcqRel`/`Acquire`, so the stale value is not torn — but it is wrong.
- **Suggested fix**: Read `self.effective_exec_tier()` *inside* the `async move` future at the point where the inner future is spawned — wrap the read in the same `async move`, and apply `TURN_EXEC_TIER.scope(...)` around the inner `execute_inner` call rather than the outer future. Or, equivalently, scope `TURN_EXEC_TIER` around the inner `execute_inner(name, input, cancel)` awaitable (which is what `turn_context` already does for `TURN_CONTEXT`).

### [Critical] `mcp_scope_view::McpScopedToolService::execute` ignores extras
- **Location**: `src/tools/mcp_scope_view.rs:48-55` (`execute` and `execute_with_cancel` both forward only to `self.parent.execute(...)`)
- **Trigger condition**: An extras-only tool (a per-agent MCP-scope tool not exposed by the parent `ScopedToolService`) is called by name. The harness routes through `McpScopedToolService::execute` and the extras are silently skipped — the tool returns `ToolError::NotFound` from the parent.
- **Expected behavior**: Either the extras tool runs (matching `describe`, `list`, `metadata_schema`, and `dispatchable_list`, all of which DO include the extras) OR `describe`/`list` filter out the extras so the surface stays consistent.
- **Actual behavior**: The four read surfaces (`list`, `describe`, `metadata_schema`, `dispatchable_list`) merge extras into the visible/declarable set, so the LLM is told the tool exists and is callable. `execute` and `execute_with_cancel` are the only call sites that do not consult extras — they forward unconditionally to the parent. The discrepancy is visible to the model: it sees a tool name in `describe`, attempts to call it, and gets `ToolError::NotFound`. The Stage I MVP comment acknowledges the deferral but does not address that the inconsistency lives *now*, not later.
- **Suggested fix**: Either (a) make `execute`/`execute_with_cancel` look up extras and route them through their own handler, mirroring the read sides; or (b) make `describe`/`list`/`metadata_schema`/`dispatchable_list` exclude extras until execute learns to dispatch them. The cleanest unblock is to delete the extras from the surfaces until execute catches up; that prevents the model from invoking a name that would 404.

### [Warning] `runtime.rs::resolve` does a one-shot separator swap and never tries mixed-direction
- **Location**: `src/tools/runtime.rs:246-258` (`resolve` only swaps `.`↔`_` once via `String::replace`)
- **Risk**: A tool name containing both `.` and `_` (e.g. `a.b_c`) hits the `.`-branch first and gets `.`→`_` once, producing `a_b_c`. If the registered tool is named `a_b_c` and the model emits `a.b_c`, this works. But if the registered tool is `a.b_c` and the model emits `a_b_c`, the `else if name.contains('_')` branch is *not* tried because the `if name.contains('.')` branch was taken and returned `None`. The asymmetry means certain name pairs cannot be aliased in both directions.
- **Current impact**: low — the production loop tools (`file_ops`, `apply_patch`, etc.) are underscore-style only, and dot/underscore drift is the standard `file.ops`↔`file_ops` pair, both single-style. But `apply_patch` paths use `.` (e.g. `src/main.rs`), so a tool name that mirrors a file path component (extremely unlikely but possible) would hit this.
- **Suggestion**: After the first attempt fails, also try the *opposite* direction swap (`name.replace('_', '.')` if the `.`→`_` did not match, and vice-versa) — or unconditionally try all three forms (literal, dot→underscore, underscore→dot) before returning `None`. The cost is two extra `HashMap` lookups on the cold path.

### [Warning] `scoped/dispatch.rs::route_and_execute` cancel path collapses non-cancel errors
- **Location**: `src/tools/scoped/dispatch.rs:1158-1170` (same location as Critical finding #1; warning because it overlaps but is a milder variant)
- **Risk**: If the tool returns `ToolError::ApprovalExpired` after the cancellation token has been fired (e.g. an approval card that was queued earlier finally returned after `/stop`), the cancel-check rewrites it to `ToolError::Cancelled`. The model loses the approval-expired hint, which is exactly the hint that says "nobody answered — try a non-interactive approach" rather than "you were stopped".
- **Current impact**: medium — the approval-expired error carries actionable retry advice via the persistence hint; cancelling it silently costs the model that advice. The condition is rare (approval card resolution racing with stop) but the persisted ledger entry will say "Cancelled" for a real approval outcome.
- **Suggestion**: Same fix as Critical #1 — only rewrite when the incoming error itself is a tool-level "cancelled" sentinel (e.g. `ToolError::Execution { cause: "tool X cancelled" }` produced by `RegistryToolAdapter::execute` and `McpRegistryTool::execute` on their `select!` cancel branch).

### [Warning] `truncate_with_budget` produces effectively-empty output at very small budgets
- **Location**: `src/tools/result_processing.rs:614-651` (`head_chars = 0 * 7 / 10 = 0; tail_chars = 0; tail_start = text.len()` → output is just the elision marker)
- **Risk**: A caller passing `budget_tokens = 0` (or a degenerate value the token estimator rounds to 0 chars) produces a body of `"\n... [output truncated, ~N tokens omitted] ...\n"` with the actual content erased — neither the head nor the tail survives. The trigger is rare in production but easy to hit on tool-result budgets derived from very small window ceilings.
- **Current impact**: low — `resolve_result_budget` clamps values via `.min(ceiling)`, and the smallest ceiling in tests is 2_400. But the budget pipeline is exposed publicly and a future caller with a tighter ceiling could silently lose content.
- **Suggestion**: Clamp `target_chars` to a minimum of e.g. 256 (or one full `MAX_SALIENT_LINES` worth of head) before computing head/tail. Or, if the budget cannot accommodate even that, return the literal elision marker and skip `format!` so the caller sees the explicit "truncated to nothing" case.

### [Warning] `dispatch::execute_with_cancel` reads `is_idempotent_builtin_name(name)` *twice* with the wrong scope
- **Location**: `src/tools/scoped/mod.rs:351` (span attribute `"tool.idempotent" = idempotent`) and `src/tools/scoped/dispatch.rs:632` (the actual retry gate uses `self.inner.is_idempotent(name) || crate::tools::retry::is_idempotent_builtin_name(name)`)
- **Risk**: The span attribute `tool.idempotent` is computed from the *literal* tool name via `is_idempotent_builtin_name(name)` and stamped into the tracing span at dispatch time. The actual retry decision a few lines below also checks `self.inner.is_idempotent(name)`. The two diverge for: (a) MCP tools declared idempotent by the server (correctly retryable, but the span says `false` because the builtin table doesn't know them); (b) builtins in the table whose name was resolved through dot/underscore aliasing (e.g. `file.ops` → `file_ops`, but the span attribute was computed on the literal `file.ops` and returned `false`). Tracing consumers see a `false` flag for what the retry layer just executed as `true`.
- **Current impact**: low — observability only, no correctness impact. But the span attribute drives the `tool.retry` correlation key in the next iteration (`retry::execute_with_one_shot_backoff`'s `tracing::info!`), so the retry-event log entry pairs with a stale `tool.idempotent` value.
- **Suggestion**: Move the idempotency derivation to a shared helper that is called from both the span construction and the retry gate — same source of truth, same alias-aware canonicalization.

### [Warning] `record_approval_decision` calls `decision.detail()` twice with no panic-safety
- **Location**: `src/tools/scoped/dispatch.rs:1100-1117` (`detail: match rule { Some(rule) => format!("{} [gate: {rule}]", decision.detail()), None => decision.detail() }`)
- **Risk**: Trivial — `decision.detail()` is a plain `&str`-returning method, no panic path. Calling it twice is wasted work, not a bug. Listed only because it appeared in the panic-audit pass.
- **Current impact**: none.
- **Suggestion**: Bind once: `let detail = decision.detail(); match rule { ... }`.

### [Warning] `in_flight.rs` `Mutex<HashMap>` is contended on the tool hot path
- **Location**: `src/tools/in_flight.rs:96-110` (`register`/`cancel`/`list`/`len` all lock a single `Arc<Mutex<HashMap>>`)
- **Risk**: Every tool call from the harness Act phase takes this lock twice (register on entry, drop-guard remove on exit). At the documented scale of "one harness per session" the doc comment says contention is "essentially nil", but `subagent_spawner` builds a separate tool service per spawned child, and `run_loop` builds a per-request service — i.e. *one process-global* registry serves every concurrent run. A Panel multi-session deployment with N concurrent runs each issuing tool calls sees 2N lock acquisitions per tool call, all serialised through one mutex. The lock is short (HashMap insert/remove) but it is on the agent hot path.
- **Current impact**: low-to-medium — at the `max_iterations=1000` interactive cap, the lock is held at most ~few ms per acquisition. But the harness's parallel fast path (`stream::iter(..).buffer_unordered`) is bottlenecked through this single mutex.
- **Suggestion**: Switch to `DashMap` (already in the workspace) or `parking_lot::Mutex` with a sharded map, or move the per-call bookkeeping to a per-session structure passed through `ScopedToolService` (where it already exists for `ToolResultStore`).

### [Warning] `ToolHandlerRegistry::register` clones the handler `Arc` once per `rcu` attempt
- **Location**: `src/tools/registry.rs:62-79` (`Arc::clone(&handler)` inside the rcu closure runs on every concurrent attempt)
- **Risk**: For N threads racing on the same name, the `Arc::clone(&handler)` line runs N times. With heavy contention on the MCP bridge (a server connecting/disconnecting rapidly), this is N-1 wasted atomic increments per `register` failure. Not a correctness issue, just a hot-path cost.
- **Current impact**: low.
- **Suggestion**: Capture the `Arc` once outside the rcu closure (`let handler_arc = handler.clone(); self.inner.rcu(... using handler_arc ...)`).

### [Warning] `text_tool_call::coerce_arguments` silently passes non-string non-object arguments through
- **Location**: `src/tools/text_tool_call.rs:159-163` (`match value { Value::String(s) => …, other => other }`)
- **Risk**: A model that double-encodes a JSON array of arguments (e.g. `{"arguments": "[1, 2, 3]"}`) hits the `Value::String` arm and gets re-decoded correctly. A model that emits a raw array (e.g. `{"arguments": [1, 2, 3]}`) passes through unchanged. The downstream tool then sees a JSON array where it expected an object — every JSON-Schema-typed tool rejects this with `ValidationFailed`. The promotion path's intent is "save a tool call", and a non-object arguments shape already fails at the tool's argument-deserialization step regardless. The risk is in the diagnostics: the model sees a `ValidationFailed` for a call that *looked* successful at the promotion seam, and the persistence hint says "validation is a caller bug" rather than "the tool-call format is wrong, switch the encoding".
- **Current impact**: low — most tools declare an object schema; the `args` field of a tool that takes an array is rare.
- **Suggestion**: Reject non-object arguments during promotion (bail out, return `None` from `promote_plain_text_tool_calls`), so the model sees a clean "not promoted" path rather than a validation failure on its first call.

### [Warning] `mcp_adapter::fence_block` does not fence strings nested deeper than `MAX_FENCE_DEPTH`
- **Location**: `src/tools/adapters/mcp_adapter.rs:236-260` (`fence_object_strings` returns early when `depth >= MAX_FENCE_DEPTH`)
- **Risk**: The `data` and `blob` keys are skipped (intentional, base64), but a deeply-nested MCP response (depth ≥ 5) carries a `text`-keyed string that the model reads unfenced and unscrubbed. A malicious MCP server nesting its payload past 4 levels would inject unmarked prompt content into the model's context.
- **Current impact**: low — the MCP protocol's content blocks are shallow (typically 1-2 levels), and `MAX_FENCE_DEPTH = 4` leaves headroom. But the constant is not a magic-number, it is "generous headroom" per the comment, so a server nesting one more level would defeat the fence.
- **Suggestion**: Treat `MAX_FENCE_DEPTH` as a *minimum* coverage, not a ceiling. If the recursion budget is exhausted on a `text` key, fall back to `fence_opaque` for that subtree (serialize the remaining JSON, wrap with `wrap_external_content`) so the boundary marker is never silently absent.

### [Warning] `redundant_calls.rs` / `no_progress.rs` fingerprint the result with the offload marker replaced — but the marker's tail (`(<n> tokens, <tool>)`) still varies when the offloaded file size differs
- **Location**: `src/tools/result_store.rs:651-678` (`stabilize_persisted_ref` keeps `(<n> tokens, <tool>)`; `src/tools/redundant_calls.rs:160-167` consumes it via `stabilize_persisted_ref`)
- **Risk**: The stabilizer replaces the path with `<offloaded>` but keeps the size tail. If the offloaded blob's size varies across calls (the test `offloaded_results_of_different_sizes_are_still_distinguished` asserts this), two genuinely identical-content loops are fingerprinted as *different* if the offload path changed size between them — e.g. an offload that just hit the threshold on call 1 (size = threshold + 1) and a subsequent identical call that hit the threshold + 2 due to platform rounding. The redundant-loop detector then incorrectly considers them distinct.
- **Current impact**: low — the test only asserts the inverse direction (different sizes correctly distinguished). Real-world identical-content loops usually produce byte-identical offloads, so the false-positive rate is negligible.
- **Suggestion**: Document the invariant — the comment "the part that still varies with the content rather than with the dispatch" is correct for "size" but the offload size is a function of the token estimate, which can differ across dispatches for the same underlying content if the estimator rounds differently. Either (a) document explicitly that offloaded markers are NOT considered identical when the size differs, or (b) also fold the size tail into the placeholder when the path is replaced.

### [Warning] `result_processing.rs::apply_result_budget` `inline_error_digest` may return empty body when distill finds errors but render budget is 0
- **Location**: `src/tools/result_processing.rs:591-607` (`Some(_) => distill_or_truncate(text, budget.saturating_sub(footer_tokens))`)
- **Risk**: When `budget.saturating_sub(footer_tokens) == 0` and the original payload has typed signal, the body comes back empty; the composed result is just the footer. A model that needs the inline error preview to plan its next call gets only the marker, no preview, defeating the offload's value.
- **Current impact**: low — only at degenerate budgets (footer alone exceeds the budget). Most tools have non-degenerate budgets.
- **Suggestion**: Reserve a minimum body budget (e.g. 64 tokens) for the inline preview, and let the footer shrink via `estimate_tokens_smart` instead of the other way around.

### [Warning] `markdown_skill::loader.rs::find_skill_files` skips hidden directories but does not skip the hidden SKILL.md itself
- **Location**: `src/tools/markdown_skill/loader.rs:84-93` (`filter_entry` only prunes directories; the `is_skill_file_static` check accepts any `SKILL.md` regardless of `.`-prefix)
- **Risk**: A `.SKILL.md` file at the root of the skills directory (a hidden file in a hidden directory's parent) would be loaded as a tool. Production skill authors using `.toolname/SKILL.md` work via the directory-name filter, so this is a contrived case — but `Skill_loader` also loads `*.skill.md` (lowercase suffix) which would pick up e.g. `Dockerfile.skill.md`.
- **Current impact**: very low.
- **Suggestion**: Either drop the `*.skill.md` arm (it is permissive beyond what the spec says), or filter hidden files at the same depth as hidden directories.

### [Warning] `context.rs::ToolContextHandle` uses `tokio::sync::RwLock` directly
- **Location**: `src/tools/context.rs:37,78` (`pub type ToolContextHandle = Arc<tokio::sync::RwLock<ToolContext>>`)
- **Risk**: The `AGENTS.md` sync-primitive rule says "Arc/Mutex/RwLock/atomics come from `crate::sync_primitives`". `tokio::sync::RwLock` is the *correct* type for an async-context RwLock (the standard `RwLock` from `crate::sync_primitives` is `parking_lot::RwLock` and must not be held across `.await`). The exception is implicit: tokio's async locks are deliberately the right tool here.
- **Current impact**: none — the rule's intent (don't hold a sync lock across await) is honoured.
- **Suggestion**: Add a comment near the type alias stating the deviation: `// tokio::sync::RwLock is correct here — std::sync would deadlock if held across .await.`.

### [Warning] `dispatch::execute_with_cancel` span attribute `session.id` is empty for non-session runs
- **Location**: `src/tools/scoped/mod.rs:351` (`"session.id" = %self.hook_session_id` where `hook_session_id: String::new()` until `with_hook_executor` is called)
- **Risk**: Tracing consumers see `session.id=""` for every dispatch without `with_hook_executor`, which is the common case for direct tools.invoke calls and tests. The trace correlates poorly to the originating session.
- **Current impact**: low — observability only.
- **Suggestion**: Stamp `session.id` from `turn_context.session_key.to_key_string()` when `turn_context` is set (and `hook_session_id` is empty), so tracing always has the session id.

### [Suggested Test] `route_and_execute` cancel-attribute rewrite coverage
```rust
#[tokio::test]
async fn cancel_does_not_overwrite_a_real_execution_error() {
    // Build a ScopedToolService that always returns a non-cancellation
    // error. Fire the cancel token before execute(). The dispatched error
    // MUST be Execution (or whatever the tool produced), not Cancelled.
    // Current behavior: cancel.is_cancelled() rewrites it to Cancelled.
    let registry = /* register a tool that returns Ok(ToolResult::Error { error: "boom", retryable: false }) */;
    let svc = /* ScopedToolService */;
    let cancel = CancellationToken::new();
    cancel.cancel(); // already fired before the call starts
    let err = svc.execute_with_cancel("tool", json!({}), cancel).await.unwrap_err();
    assert!(matches!(err, ToolError::Execution { .. }), "got {err:?}");
}

#[tokio::test]
async fn cancel_rewrites_only_the_tools_cancelled_sentinel() {
    // Tool returns ToolError::Execution { cause: "tool X cancelled", .. }
    // — the shape RegistryToolAdapter and McpRegistryTool produce when
    // their select! arm fires. cancel.is_cancelled() is true. This SHOULD
    // be rewritten to Cancelled (the whole point of the rewrite).
}
```

### [Suggested Test] `dispatchable_list` of `McpScopedToolService` matches `execute` reachability
```rust
#[tokio::test]
async fn extras_visible_in_describe_but_not_in_execute_is_a_bug() {
    // Today: extras appear in describe/list/metadata_schema/dispatchable_list
    // but execute() does not route them. Document the inconsistency as a
    // test that asserts at minimum one of the two:
    //   (a) execute() routes extras; OR
    //   (b) describe/list/etc. exclude extras until execute catches up.
    let parent = /* ScopedToolService with no parent definitions */;
    let extras = vec![ToolRegistration {
        name: "extras_only".into(),
        description: "...".into(),
        parameters: json!({"type":"object"}),
        plugin_id: "p".into(),
    }];
    let svc = McpScopedToolService::new(Arc::new(parent), extras);
    let listed = svc.list().await;
    let desc = svc.describe("extras_only").await;
    let exec = svc.execute("extras_only", json!({})).await;
    // Either describe is None and execute returns NotFound (consistent),
    // OR describe is Some and execute returns Ok (consistent).
    let consistent = (desc.is_none() && matches!(exec, Err(ToolError::NotFound { .. })))
        || (desc.is_some() && exec.is_ok());
    assert!(consistent, "describe/list/execute must agree");
}
```

### [Suggested Test] `tier_at_dispatch` plan-gate mid-call flip
```rust
#[tokio::test]
async fn plan_gate_flip_during_dispatch_uses_new_tier_for_lift() {
    // Set up a ScopedToolService with a PlanGate and a tool body that calls
    // gate.release() then immediately reads current_exec_tier() (the lift
    // reads this via TURN_EXEC_TIER). The expected tier at the moment of
    // the lift read is the restore tier, not Plan. Today, the lift reads
    // the stale tier captured before the dispatch began.
    let gate = PlanGate::new(ExecTier::Auto);
    let tc = TurnContext { plan_gate: Some(Arc::new(gate.clone())), .. };
    let svc = /* ScopedToolService::new(...).with_turn_context(tc) */;
    // ... tool body asserts on current_exec_tier() ...
    // After the call, gate.is_released() should be true AND the lift
    // should have read ExecTier::Auto.
}
```

### [Suggested Test] `fence_block` does not silently leave deeply-nested strings unfenced
```rust
#[test]
fn fence_block_falls_back_to_opaque_when_depth_cap_hits() {
    // Construct a value nested deeper than MAX_FENCE_DEPTH that carries a
    // text-bearing string at the deepest level. Assert that the deepest
    // text is either fenced or wrapped with wrap_external_content — never
    // silently passed through unmarked. Today, the depth-cap branch
    // returns without touching the string.
    let value = json!({
        "a": { "a": { "a": { "a": { "a": { "text": "HOSTILE" } } } } }
    });
    let fenced = fence_block(value, &ContentSource::McpTool { server: "x".into(), tool: "t".into() });
    let rendered = serde_json::to_string(&fenced).unwrap();
    assert!(rendered.contains("HOSTILE") && rendered.contains("<system-reminder>"),
        "deepest text must be either neutrally serialized or fenced, never raw: {rendered}");
}
```

### [Suggested Test] `truncate_with_budget` does not erase content at small budgets
```rust
#[test]
fn truncate_with_budget_preserves_at_least_a_head_or_tail_at_small_budgets() {
    let text: String = "x".repeat(1000);
    let out = truncate_with_budget(&text, 1);
    // Today: head_chars = 0, tail_chars = 0, output is just the elision marker.
    // Should: keep at least some head OR some tail even at degenerate budgets.
    assert!(out.contains("x"), "small-budget truncation must preserve some content, got: {out}");
}
```

### [Suggested Test] `runtime.rs::resolve` does not lose mixed-separator names
```rust
#[test]
fn resolve_tries_both_separator_directions() {
    // Register a tool named "a.b.c". resolve("a_b_c") should find it after
    // trying the underscore→dot swap. Today: resolve("a_b_c") does the
    // underscore→dot swap first (else if), so this case works. But
    // resolve("a.b_c") with a registered "a_b_c" only does the dot→underscore
    // swap once (a_b_c), and finds it. The mixed case "a.b_c.d" with
    // registered "a_b_c_d" works. The asymmetric case is the failure mode.
    let mut reg = LoopToolRegistry::new();
    reg.register(/* tool named "weird.tool" */);
    let reg = Arc::new(reg);
    assert_eq!(reg.resolve("weird_tool").map(|t| t.name()), Some("weird.tool"));
    assert_eq!(reg.resolve("weird.tool").map(|t| t.name()), Some("weird.tool"));
}
```

## Cross-Module Findings

- **Wiring verified**: `build_request_tool_service` (`src/gateway/execution_engine/tool_service_builder.rs:153`) is the production entry point and constructs `ScopedToolService` per request with all wiring hooks (confirmation, hook_executor, turn_context, deferred, tool_health, tool_permissions, exec_tier, unattended). The boot-time `set_mcp_tool_registry` (`aleph-server/commands/start/mod.rs:219`) hands the `ToolHandlerRegistry` to `run_loop` so MCP tools join the request. `subagent_spawner` (`src/agents/subagent_spawner/mod.rs:798`) wires `McpScopedToolService` for spawned children. Every documented seam is wired.
- **Wiring regression risk**: `McpScopedToolService` exposes extras in describe/list/metadata_schema/dispatchable_list but NOT in execute. A model that calls an extras-only tool gets `ToolError::NotFound` after being told the tool exists. See Critical finding #3 above.
- **CapabilitySlot semantics**: All `CapabilitySlot` handles in this module (`tools/in_flight`, `tools/result_store`, `tools/turn_budget`, `tools/result_processing::RESULT_BUDGET_CEILING`, and the gateway's `tools/registry::MCP_TOOL_REGISTRY`, `CONFIRMATION_REQUESTER`, `CONFIG_APPROVAL_REQUESTER`) use either `FailsClosed` or `IndistinguishableDefault`. The `MissingSemantics::ConsumerDecides` variant in `in_flight.rs` (line 79) is documented in-file as having two distinct readers (gateway RPC vs orchestrator harness) and the sentence a diagnostic prints differs by reader — that is a deliberate trade, but the harness-side branch (`HarnessDeps.in_flight_tool_calls: Option<_>`) means the RPC exists while the harness silently never registers anything. Worth confirming this is intentional at boot.
- **`dispatchable_list` completeness**: `ScopedToolService::dispatchable_list` (`src/tools/scoped/mod.rs:281-302`) correctly augments `list()` with the deferred tier, and applies the allow/deny/health gates to the deferred entries too. The comment about tool-name repair (a deferred tool's correct name must reach the Exact tier, otherwise the Fuzzy tier rewrites it into a different resident tool) is enforced: `dispatchable_list` is the dispatchable set, and the name-repairer consults this set. `McpScopedToolService::dispatchable_list` correctly forwards to the parent's `dispatchable_list` rather than the trait default (`list()`), so the deferred tier is preserved through the wrapper.
- **No dead public API in production**: every `pub fn`/`pub struct` in this module is referenced somewhere in the crate (verified via grep on `use` sites and constructor calls). `McpScopedToolService` is the closest thing to orphaned but it is wired by `subagent_spawner`.
- **TURN_CONTEXT vs TURN_EXEC_TIER layering**: both are scoped by the same chokepoint (`ScopedToolService::execute_with_cancel`), but `TURN_CONTEXT` is wrapped *inside* the inner async block while `TURN_EXEC_TIER` is wrapped *outside* it (after the move). See Critical finding #2 — the inside/outside asymmetry means a `PlanGate` flip during the inner call does not change `TURN_EXEC_TIER` until the next dispatch.

## Summary

| Level | Count |
|-------|-------|
| Critical | 3 |
| Warning | 13 |
| Suggested Test | 6 |
| Cross-Module | 5 (wiring observations) |