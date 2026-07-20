# L2 `total_tokens` Wiring Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `RunSummary` / `FlowOutcome` / `LoopRunResult` / harness trace events report real provider-reported token usage instead of a hardcoded `0`.

**Architecture:** Add a cumulative token counter to `AgentHarness` (an `AtomicU64`, mirroring the existing `hit_limit: AtomicBool`). The Think phase already holds the provider `ProviderResponse`; it sums that turn's `usage` components and adds them to the counter. After a run, the orchestrator bridge and the subagent spawner read `harness.total_tokens()` — exactly as they already read `harness.hit_limit()`. The gateway `event_drain` path then auto-fills because it already reads `FlowOutcome.total_tokens`.

**Tech Stack:** Rust, `tokio`, `std::sync::atomic`, in-crate unit + integration tests (`cargo test -p alephcore`).

**Spec:** `docs/superpowers/specs/2026-05-19-l2-total-tokens-wiring-design.md`

**Token-total definition:** `total_tokens = input_tokens + output_tokens + cache_read_tokens + cache_creation_tokens`. `thinking_tokens` is excluded — Anthropic's `output_tokens` already includes thinking tokens, so adding it would double-count. Missing (`None`) cache components count as 0.

**Baseline noise:** Known pre-existing failures (`cron_job_new_sets_defaults`, `slack_contract_test`, `cron_probe`, and a batch of parallel-isolation failures) are unrelated to this work — see `project_baseline_test_failures.md`. Verify with the targeted commands in each task, not the full suite.

---

### Task 1: Harness token accumulator

Adds the pure summation helper, the `AtomicU64` counter on `AgentHarness`, the `total_tokens()` accessor, and the per-turn accumulation in the Think phase. These are interdependent (the helper needs a real caller, the accumulation needs the field) so they land in one commit — this also avoids a `dead_code` lint window.

**Files:**
- Modify: `src/harness/agent.rs` (struct field, accessor, `new()`, helper fn, tests)
- Modify: `src/harness/agent/think.rs` (per-turn accumulation)

- [ ] **Step 1: Write the failing helper unit tests**

Add these three tests inside the `#[cfg(test)] mod tests { ... }` block in `src/harness/agent.rs` (the block begins at line 447). Place them right after the `fresh_session` async fn (around line 615):

```rust
    #[test]
    fn turn_token_total_sums_four_components() {
        use crate::providers::adapter::TokenUsage;
        let usage = Some(TokenUsage {
            input_tokens: 100,
            output_tokens: 250,
            cache_read_tokens: Some(40),
            cache_creation_tokens: Some(10),
            thinking_tokens: Some(999),
            cost: None,
        });
        // 100 + 250 + 40 + 10 = 400. thinking_tokens (999) is excluded.
        assert_eq!(super::turn_token_total(&usage), 400);
    }

    #[test]
    fn turn_token_total_none_usage_is_zero() {
        assert_eq!(super::turn_token_total(&None), 0);
    }

    #[test]
    fn turn_token_total_treats_missing_cache_as_zero() {
        use crate::providers::adapter::TokenUsage;
        let usage = Some(TokenUsage {
            input_tokens: 7,
            output_tokens: 11,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            thinking_tokens: None,
            cost: None,
        });
        assert_eq!(super::turn_token_total(&usage), 18);
    }
```

- [ ] **Step 2: Run the helper tests to verify they fail**

Run: `cargo test -p alephcore --lib turn_token_total 2>&1 | tail -20`
Expected: FAIL — compile error, `cannot find function turn_token_total in module super`.

- [ ] **Step 3: Add the `turn_token_total` helper**

In `src/harness/agent.rs`, add this free function immediately before the `#[cfg(test)]` attribute on line 447 (i.e. after the final `impl` block, outside any `impl`):

```rust
/// Sum the provider-reported token components for one LLM call.
///
/// `total_tokens` = `input + output + cache_read + cache_creation`. Cache
/// components default to 0 when the provider omits them. `thinking_tokens`
/// is intentionally excluded: Anthropic's `output_tokens` already counts
/// thinking tokens, so adding it would double-count.
fn turn_token_total(usage: &Option<crate::providers::adapter::TokenUsage>) -> u64 {
    match usage {
        None => 0,
        Some(u) => {
            u64::from(u.input_tokens)
                + u64::from(u.output_tokens)
                + u64::from(u.cache_read_tokens.unwrap_or(0))
                + u64::from(u.cache_creation_tokens.unwrap_or(0))
        }
    }
}
```

- [ ] **Step 4: Run the helper tests to verify they pass**

Run: `cargo test -p alephcore --lib turn_token_total 2>&1 | tail -20`
Expected: PASS — 3 tests pass.

- [ ] **Step 5: Write the failing accumulator test**

Add a usage-returning provider and a test inside the same `#[cfg(test)] mod tests` block in `src/harness/agent.rs`. Place the provider after the `SleepingProvider` impl (around line 577) and the test after `fresh_session`:

```rust
    struct UsageProvider {
        usage: crate::providers::adapter::TokenUsage,
    }

    impl AiProvider for UsageProvider {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
            let usage = self.usage.clone();
            Box::pin(async move {
                Ok(ProviderResponse {
                    text: Some("done".to_string()),
                    stop_reason: StopReason::EndTurn,
                    usage: Some(usage),
                    ..Default::default()
                })
            })
        }

        fn name(&self) -> &str {
            "usage"
        }

        fn color(&self) -> &str {
            "#00ff00"
        }
    }
```

```rust
    #[tokio::test]
    async fn harness_accumulates_provider_token_usage() {
        use crate::providers::adapter::TokenUsage;
        let provider: Arc<dyn AiProvider> = Arc::new(UsageProvider {
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 20,
                cache_read_tokens: Some(5),
                cache_creation_tokens: Some(3),
                thinking_tokens: Some(99),
                cost: None,
            },
        });

        let (session, sid) = fresh_session("test-tokens").await;
        let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(AlwaysOkTools);
        let sandbox: Arc<dyn crate::sandbox::Sandbox> = Arc::new(crate::sandbox::NoopSandbox);

        let deps = HarnessDeps {
            session,
            tools,
            sandbox,
            llm: provider,
            verifier_chain: None,
            context_budget: None,
            context_compactor: None,
            skill_prefetcher: None,
            trace_sink: None,
            system_prompt: None,
            prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
            chain_context: crate::harness::chain_context::ChainContext::default(),
            guardrails: None,
            fallback_llm: None,
            max_iterations: Some(3),
            power: None,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
        };
        let harness = super::AgentHarness::new(deps);
        let mut cb = NoopHarnessCallback;
        let cancel = tokio_util::sync::CancellationToken::new();
        harness.run(&sid, &mut cb, &cancel).await.expect("run ok");

        // Single text-only turn: input + output + cache_read + cache_creation
        // = 10 + 20 + 5 + 3 = 38. thinking_tokens (99) is excluded.
        assert_eq!(harness.total_tokens(), 38);
    }
```

- [ ] **Step 6: Run the accumulator test to verify it fails**

Run: `cargo test -p alephcore --lib harness_accumulates_provider_token_usage 2>&1 | tail -20`
Expected: FAIL — compile error, `no method named total_tokens found for struct AgentHarness`.

- [ ] **Step 7: Add the `AtomicU64` import**

In `src/harness/agent.rs`, change line 24 from:

```rust
use std::sync::atomic::{AtomicBool, Ordering};
```

to:

```rust
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
```

- [ ] **Step 8: Add the `total_tokens` field to `AgentHarness`**

In `src/harness/agent.rs`, in the `pub struct AgentHarness { ... }` definition (lines 65-74), add a field after `hit_limit: AtomicBool,`:

```rust
pub struct AgentHarness {
    pub(super) deps: HarnessDeps,
    /// Tracks agent activity for stall detection. `None` when stall detection
    /// is disabled (no `stall_config` in deps).
    pub(super) stall_tracker: Option<crate::harness::deps::StallTracker>,
    /// Set when `context_budget.before_turn` returns `FinalReply`. Surfaced
    /// through [`AgentHarness::hit_limit`] so the orchestrator bridge can
    /// populate `FlowOutcome::hit_limit`.
    hit_limit: AtomicBool,
    /// Cumulative provider-reported token usage across every LLM call in
    /// this run (`input + output + cache_read + cache_creation`). Read after
    /// the run via [`AgentHarness::total_tokens`] by the orchestrator bridge
    /// and subagent spawner. A harness instance serves a single run, so the
    /// counter is never reset.
    total_tokens: AtomicU64,
}
```

- [ ] **Step 9: Initialise the field in `new()`**

In `src/harness/agent.rs`, in `AgentHarness::new` (lines 77-87), add the field to the constructed `Self`:

```rust
        Self {
            deps,
            stall_tracker,
            hit_limit: AtomicBool::new(false),
            total_tokens: AtomicU64::new(0),
        }
```

- [ ] **Step 10: Add the `total_tokens()` accessor**

In `src/harness/agent.rs`, add this method immediately after `reset_hit_limit` (after line 99, inside `impl AgentHarness`):

```rust
    /// Cumulative provider-reported token usage observed across every LLM
    /// call in this run. Components summed: `input + output + cache_read +
    /// cache_creation` (see `turn_token_total`). A harness instance serves
    /// exactly one run, so this counter is never reset.
    pub fn total_tokens(&self) -> u64 {
        self.total_tokens.load(Ordering::Relaxed)
    }
```

- [ ] **Step 11: Accumulate the turn's tokens in the Think phase**

In `src/harness/agent/think.rs`, find the end of the `let response = match primary_result { ... };` block — it ends with these lines (around line 187-189):

```rust
            Err(primary_err) => return Err(HarnessError::Llm(primary_err)),
        };

        // 4. Emit AssistantMessage preserving any tool_use intent in `blocks`.
```

Insert the accumulation between the closing `};` and the `// 4.` comment, so the region becomes:

```rust
            Err(primary_err) => return Err(HarnessError::Llm(primary_err)),
        };

        // Accumulate this turn's provider-reported token usage. Counted here
        // — right after the LLM call — so a turn whose output is later
        // blocked by a guardrail still reflects the tokens the provider
        // billed. Excludes `thinking_tokens`; see `turn_token_total`.
        let turn_tokens = super::turn_token_total(&response.usage);
        self.total_tokens.fetch_add(turn_tokens, Ordering::Relaxed);

        // 4. Emit AssistantMessage preserving any tool_use intent in `blocks`.
```

(`src/harness/agent/think.rs` already imports `std::sync::atomic::Ordering` on line 3 — no new import needed.)

- [ ] **Step 12: Run the accumulator test to verify it passes**

Run: `cargo test -p alephcore --lib harness_accumulates_provider_token_usage 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 13: Run lint to confirm no dead-code / clippy warnings**

Run: `cargo clippy -p alephcore --lib 2>&1 | tail -20`
Expected: no warnings about `turn_token_total` or `total_tokens` (the helper is used by `think.rs`, the field by the accessor + `think.rs`).

- [ ] **Step 14: Commit**

```bash
git add src/harness/agent.rs src/harness/agent/think.rs
git commit -m "harness: accumulate provider-reported token usage per run"
```

---

### Task 2: Surface `total_tokens` in turn metrics and `SessionCompleted`

Fills the four `total_tokens: 0` literals in the harness trace structs: `LoopTraceTurnMetrics` (per-turn, from `turn_tokens`) and `LoopTraceEvent::SessionCompleted` (cumulative, from the accumulator).

**Files:**
- Modify: `src/harness/agent/think.rs:278,329` (`LoopTraceTurnMetrics`)
- Modify: `src/harness/agent.rs:297,327` (`SessionCompleted`)
- Modify: `src/harness/tests/stability.rs` (new provider + test)

- [ ] **Step 1: Write the failing trace test**

In `src/harness/tests/stability.rs`, add a usage-returning provider. Place it after the `HangingProvider` impl (around line 63, before `OneShotToolProvider`):

```rust
/// Provider that returns one text-only response carrying a fixed token
/// `usage` — the Think loop sees no tool_calls and terminates in one turn.
pub(super) struct UsageTextProvider {
    pub(super) usage: crate::providers::adapter::TokenUsage,
}

impl AiProvider for UsageTextProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = AlephResult<ProviderResponse>> + Send + 'a>> {
        let usage = self.usage.clone();
        Box::pin(async move {
            Ok(ProviderResponse {
                text: Some("done".to_string()),
                stop_reason: crate::providers::adapter::StopReason::EndTurn,
                usage: Some(usage),
                ..Default::default()
            })
        })
    }
    fn name(&self) -> &str {
        "usage-text"
    }
    fn color(&self) -> &str {
        "#00ff00"
    }
}
```

Then add this test after the `recording_sink_captures_full_lifecycle` test (after line 336):

```rust
#[tokio::test]
async fn session_completed_and_turn_metrics_carry_total_tokens() {
    use crate::providers::adapter::TokenUsage;
    let (sink, events) = RecordingTraceSink::new();
    let provider: Arc<dyn AiProvider> = Arc::new(UsageTextProvider {
        usage: TokenUsage {
            input_tokens: 8,
            output_tokens: 14,
            cache_read_tokens: Some(2),
            cache_creation_tokens: None,
            thinking_tokens: None,
            cost: None,
        },
    });
    let (session, sid) = fresh_session("trace-tokens").await;
    let tools: Arc<dyn crate::tools::service::ToolService> = Arc::new(MixedTools);

    let mut deps = minimal_deps(session, tools, provider);
    deps.trace_sink = Some(sink);
    let harness = AgentHarness::new(deps);

    let mut cb = NoopHarnessCallback;
    let cancel = tokio_util::sync::CancellationToken::new();
    harness.run(&sid, &mut cb, &cancel).await.expect("run ok");

    let captured = events.lock().unwrap().clone();
    // Single text-only turn: 8 + 14 + 2 = 24.
    let session_completed = captured.iter().find_map(|e| match e {
        LoopTraceEvent::SessionCompleted { total_tokens, .. } => Some(*total_tokens),
        _ => None,
    });
    assert_eq!(
        session_completed,
        Some(24),
        "SessionCompleted.total_tokens should be the cumulative sum",
    );
    let turn_metrics = captured.iter().find_map(|e| match e {
        LoopTraceEvent::TurnCompleted { metrics, .. } => Some(metrics.total_tokens),
        _ => None,
    });
    assert_eq!(
        turn_metrics,
        Some(24),
        "TurnCompleted metrics.total_tokens should be the turn's usage sum",
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p alephcore --lib session_completed_and_turn_metrics_carry_total_tokens 2>&1 | tail -25`
Expected: FAIL — assertion failure, `session_completed` is `Some(0)` (the literal `total_tokens: 0` is still in place).

- [ ] **Step 3: Fill `total_tokens` in `SessionCompleted` (success branch)**

In `src/harness/agent.rs`, in the `match result { Ok(outcome) => { ... } }` arm (around lines 292-301), change the `SessionCompleted` construction:

```rust
                self.emit(|| crate::harness::trace::LoopTraceEvent::SessionCompleted {
                    outcome,
                    iterations,
                    tool_calls_made,
                    total_tokens: self.total_tokens.load(Ordering::Relaxed) as usize,
                    hit_limit: matches!(
                        outcome,
                        crate::harness::trace::LoopTraceSessionOutcome::HitLimit,
                    ),
                    final_text: None,
                });
```

- [ ] **Step 4: Fill `total_tokens` in `SessionCompleted` (error branch)**

In `src/harness/agent.rs`, in the `Err(e) => { ... }` arm (around lines 323-330), change the `SessionCompleted` construction:

```rust
                self.emit(|| crate::harness::trace::LoopTraceEvent::SessionCompleted {
                    outcome: session_outcome,
                    iterations,
                    tool_calls_made,
                    total_tokens: self.total_tokens.load(Ordering::Relaxed) as usize,
                    hit_limit: false,
                    final_text: None,
                });
```

- [ ] **Step 5: Fill `total_tokens` in the `zero_metrics` turn metrics**

In `src/harness/agent/think.rs`, find the `zero_metrics` binding (around lines 273-279) and change `total_tokens: 0,`:

```rust
        let zero_metrics = crate::harness::trace::LoopTraceTurnMetrics {
            requested_tool_calls: 0,
            executed_tool_calls: 0,
            productive: false,
            consecutive_errors: 0,
            total_tokens: turn_tokens as usize,
        };
```

- [ ] **Step 6: Fill `total_tokens` in the tool-calls-branch turn metrics**

In `src/harness/agent/think.rs`, find the `metrics_for_trace` assignment in the `else` (tool-calls) branch (around lines 324-330) and change `total_tokens: 0,`:

```rust
            metrics_for_trace = crate::harness::trace::LoopTraceTurnMetrics {
                requested_tool_calls: requested,
                executed_tool_calls: executed,
                productive: executed > 0,
                consecutive_errors: 0,
                total_tokens: turn_tokens as usize,
            };
```

- [ ] **Step 7: Run the test to verify it passes**

Run: `cargo test -p alephcore --lib session_completed_and_turn_metrics_carry_total_tokens 2>&1 | tail -25`
Expected: PASS.

- [ ] **Step 8: Run the harness test suite for regressions**

Run: `cargo test -p alephcore --lib harness:: 2>&1 | tail -25`
Expected: PASS — no regressions in existing harness tests.

- [ ] **Step 9: Commit**

```bash
git add src/harness/agent.rs src/harness/agent/think.rs src/harness/tests/stability.rs
git commit -m "harness: surface total_tokens in turn metrics and SessionCompleted"
```

---

### Task 3: Populate `FlowOutcome.total_tokens` from the harness

Wires `harness.total_tokens()` into the orchestrator bridge's `FlowOutcome`, mirroring the adjacent `hit_limit: harness.hit_limit()`. The gateway `event_drain` path already reads `FlowOutcome.total_tokens`, so it auto-fills.

**Files:**
- Modify: `src/orchestrator/harness_bridge.rs:374-383` (`FlowOutcome` construction)
- Modify: `tests/common/mod.rs` (`ScriptedLlm` returns a `usage`)
- Modify: `tests/orchestrator_e2e.rs` (assert `outcome.total_tokens`)

- [ ] **Step 1: Make the scripted e2e provider report `usage`**

In `tests/common/mod.rs`, change the import on line 24 from:

```rust
use alephcore::providers::adapter::{ProviderResponse, RequestPayload};
```

to:

```rust
use alephcore::providers::adapter::{ProviderResponse, RequestPayload, StopReason, TokenUsage};
```

Then in `impl AiProvider for ScriptedLlm`, change the `process` body (around lines 99-103) from:

```rust
        Box::pin(async move {
            let mut q = self.queue.lock().await;
            let text = q.pop().unwrap_or_else(|| self.sticky.clone());
            Ok(ProviderResponse::text_only(text))
        })
```

to:

```rust
        Box::pin(async move {
            let mut q = self.queue.lock().await;
            let text = q.pop().unwrap_or_else(|| self.sticky.clone());
            // Report a fixed token usage so e2e tests can assert the
            // usage-surfacing path. 7 + 11 = 18 tokens per call.
            Ok(ProviderResponse {
                text: Some(text),
                stop_reason: StopReason::EndTurn,
                usage: Some(TokenUsage {
                    input_tokens: 7,
                    output_tokens: 11,
                    cache_read_tokens: None,
                    cache_creation_tokens: None,
                    thinking_tokens: None,
                    cost: None,
                }),
                ..Default::default()
            })
        })
```

- [ ] **Step 2: Write the failing assertion in the e2e test**

In `tests/orchestrator_e2e.rs`, in `default_agent_roundtrip`, add a token assertion right after the existing `assert!(outcome.final_text.contains("42"), ...)` block (after line 43):

```rust
    // ScriptedLlm reports 7 input + 11 output tokens per call; this flow
    // makes one LLM call, so the bridge surfaces 18 tokens.
    assert_eq!(
        outcome.total_tokens, 18,
        "FlowOutcome.total_tokens should reflect provider-reported usage",
    );
```

- [ ] **Step 3: Run the e2e test to verify it fails**

Run: `cargo test -p alephcore --test orchestrator_e2e default_agent_roundtrip 2>&1 | tail -25`
Expected: FAIL — `assertion failed: outcome.total_tokens == 18`, left is `0`.

- [ ] **Step 4: Wire `total_tokens` into `FlowOutcome`**

In `src/orchestrator/harness_bridge.rs`, replace the comment + `FlowOutcome` construction (around lines 374-383):

```rust
        // `total_tokens` still defaults to 0 — provider-side usage surfacing
        // is outside Task-10 scope. `hit_limit` is now populated from the
        // budget sensor via `AgentHarness::hit_limit()`.
        let outcome = FlowOutcome {
            final_text,
            iterations,
            tool_calls_made,
            hit_limit: harness.hit_limit(),
            ..Default::default()
        };
```

with:

```rust
        // `total_tokens` and `hit_limit` are read straight off the harness
        // after the run: the harness retains the cumulative token counter
        // and the budget-sensor flag. `total_tokens` saturates into the
        // `u32` field (`as u32` would truncate; a run is realistically far
        // below `u32::MAX` tokens).
        let outcome = FlowOutcome {
            final_text,
            iterations,
            tool_calls_made,
            total_tokens: u32::try_from(harness.total_tokens()).unwrap_or(u32::MAX),
            hit_limit: harness.hit_limit(),
        };
```

- [ ] **Step 5: Run the e2e test to verify it passes**

Run: `cargo test -p alephcore --test orchestrator_e2e default_agent_roundtrip 2>&1 | tail -25`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/orchestrator/harness_bridge.rs tests/common/mod.rs tests/orchestrator_e2e.rs
git commit -m "orchestrator: populate FlowOutcome.total_tokens from the harness"
```

---

### Task 4: Populate `LoopRunResult.total_tokens` from the subagent harness

Threads `harness.total_tokens()` through `extract_run_result` into `LoopRunResult`, mirroring the existing `hit_limit` parameter.

**Files:**
- Modify: `src/agents/subagent_spawner/mod.rs:434-477` (`extract_run_result` signature + body, call site)
- Modify: `src/agents/subagent_spawner/tests.rs` (update direct `extract_run_result` calls, new tests)

- [ ] **Step 1: Write the failing tests**

In `src/agents/subagent_spawner/tests.rs`, add a token assertion to the existing `final_text_cleared_when_last_assistant_is_empty` test. Change its `extract_run_result` call (around line 644) from:

```rust
        let result = extract_run_result(session.as_ref(), &child_id, &chain, true)
            .await
            .expect("extract ok");
```

to:

```rust
        let result = extract_run_result(session.as_ref(), &child_id, &chain, true, 777)
            .await
            .expect("extract ok");
```

and add an assertion at the end of that test (after the `hit_limit` assertion):

```rust
        assert_eq!(result.total_tokens, 777, "total_tokens must propagate from caller");
```

Then add a new spawn-level test after `spawn_single_turn_returns_final_text` (after line 326):

```rust
    #[tokio::test]
    async fn spawn_reports_provider_token_usage() {
        use crate::providers::adapter::{StopReason, TokenUsage};
        let provider = ScriptedProvider::new(vec![ProviderResponse {
            text: Some("done".to_string()),
            tool_calls: vec![],
            thinking: None,
            thinking_signature: None,
            stop_reason: StopReason::EndTurn,
            usage: Some(TokenUsage {
                input_tokens: 12,
                output_tokens: 30,
                cache_read_tokens: Some(4),
                cache_creation_tokens: Some(2),
                thinking_tokens: None,
                cost: None,
            }),
        }]);
        let base = make_base(provider);

        let agent = agent_with_allowed("echo", vec!["*"]);
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
        // 12 + 30 + 4 + 2 = 48.
        assert_eq!(result.total_tokens, 48);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib subagent_spawner 2>&1 | tail -25`
Expected: FAIL — `extract_run_result` takes 4 arguments but 5 were supplied (compile error).

- [ ] **Step 3: Add the `total_tokens` parameter to `extract_run_result`**

In `src/agents/subagent_spawner/mod.rs`, change the `extract_run_result` signature (around lines 434-439) from:

```rust
async fn extract_run_result(
    session: &dyn SessionService,
    child_id: &SessionId,
    chain: &ChainContext,
    hit_limit: bool,
) -> Result<LoopRunResult, String> {
```

to:

```rust
async fn extract_run_result(
    session: &dyn SessionService,
    child_id: &SessionId,
    chain: &ChainContext,
    hit_limit: bool,
    total_tokens: u64,
) -> Result<LoopRunResult, String> {
```

- [ ] **Step 4: Use the parameter in the `LoopRunResult` construction**

In `src/agents/subagent_spawner/mod.rs`, in the `Ok(LoopRunResult { ... })` at the end of `extract_run_result` (around line 473-481), change `total_tokens: 0,`:

```rust
    Ok(LoopRunResult {
        final_text,
        iterations,
        tool_calls_made,
        total_tokens: total_tokens as usize,
        hit_limit,
        chain_id: chain.chain_id.clone(),
        depth: chain.depth,
    })
```

- [ ] **Step 5: Pass `harness.total_tokens()` at the production call site**

In `src/agents/subagent_spawner/mod.rs`, find the `extract_run_result` call in `spawn` (around line 367-369):

```rust
                let result =
                    extract_run_result(base.session.as_ref(), &child_id, &child_chain, hit_limit)
                        .await?;
```

Change it to also read the harness token counter (the `harness` `Arc` handle is in scope, just as `hit_limit` was read on the line above):

```rust
                let total_tokens = harness.total_tokens();
                let result = extract_run_result(
                    base.session.as_ref(),
                    &child_id,
                    &child_chain,
                    hit_limit,
                    total_tokens,
                )
                .await?;
```

- [ ] **Step 6: Update the other direct `extract_run_result` test call**

In `src/agents/subagent_spawner/tests.rs`, there is a second direct call (the control-case test, around line 672) of the form `extract_run_result(session.as_ref(), &child_id, &chain, false)`. Add a `, 0` argument so it becomes:

```rust
        let result = extract_run_result(session.as_ref(), &child_id, &chain, false, 0)
            .await
            .expect("extract ok");
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib subagent_spawner 2>&1 | tail -25`
Expected: PASS — including `spawn_reports_provider_token_usage` and `final_text_cleared_when_last_assistant_is_empty`.

- [ ] **Step 8: Commit**

```bash
git add src/agents/subagent_spawner/mod.rs src/agents/subagent_spawner/tests.rs
git commit -m "agents: populate LoopRunResult.total_tokens from the subagent harness"
```

---

### Task 5: Document the Cluster B `total_tokens: 0` sites

The remaining three literal `total_tokens: 0` sites are correct-by-design (no LLM call on those paths). Add a one-line comment at each so a future reader does not "fix" a non-bug. Comment-only — no tests.

**Files:**
- Modify: `src/gateway/execution_engine/simple.rs:139`
- Modify: `src/gateway/execution_engine/fast_path.rs:49,130`

- [ ] **Step 1: Comment the `simple.rs` site**

In `src/gateway/execution_engine/simple.rs`, find the `RunSummary` construction (around line 135-142) and add a comment on the `total_tokens` line:

```rust
                        summary: RunSummary {
                            // 0 is correct: SimpleExecutionEngine is the
                            // simulated/fallback engine used when no API key
                            // is set — no real LLM call is made.
                            total_tokens: 0,
                            tool_calls: 0,
                            loops: steps_completed,
                            final_response: Some(response.clone()),
                        },
```

- [ ] **Step 2: Comment the `fast_path.rs` success site**

In `src/gateway/execution_engine/fast_path.rs`, find the `RunSummary` in `finalize_fast_path_success` (around line 47-52, identified by `loops: steps_completed` and `tool_calls: 1`) and add a comment:

```rust
                summary: RunSummary {
                    // 0 is correct: the L0 slash-command fast path bypasses
                    // the agent loop and makes no LLM call (commands that
                    // need an LLM fall through to the loop instead).
                    total_tokens: 0,
                    tool_calls: 1,
                    loops: steps_completed,
                    final_response: Some(response),
                },
```

- [ ] **Step 3: Comment the `fast_path.rs` error site**

In `src/gateway/execution_engine/fast_path.rs`, find the `RunSummary` in `finalize_fast_path_error` (around line 124-129, identified by `loops: 0` and `tool_calls: 1`) and add a comment:

```rust
                summary: RunSummary {
                    // 0 is correct: the L0 slash-command fast path bypasses
                    // the agent loop and makes no LLM call.
                    total_tokens: 0,
                    tool_calls: 1,
                    loops: 0,
                    final_response: Some(error_response),
                },
```

- [ ] **Step 4: Verify it still compiles**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: clean compile (comment-only change).

- [ ] **Step 5: Commit**

```bash
git add src/gateway/execution_engine/simple.rs src/gateway/execution_engine/fast_path.rs
git commit -m "gateway: document why fast-path RunSummary.total_tokens is 0"
```

---

## Final Verification

- [ ] **Run all touched test suites:**

```bash
cargo test -p alephcore --lib harness:: 2>&1 | tail -15
cargo test -p alephcore --lib subagent_spawner 2>&1 | tail -15
cargo test -p alephcore --test orchestrator_e2e 2>&1 | tail -15
cargo clippy -p alephcore --lib 2>&1 | tail -15
```

Expected: all PASS, no clippy warnings. Ignore the known baseline failures listed in `project_baseline_test_failures.md` if they appear in unrelated suites.

- [ ] **Confirm no stray `total_tokens: 0` regressions:**

```bash
grep -rn "total_tokens: 0" src/ | grep -v "execution_engine/simple.rs\|execution_engine/fast_path.rs"
```

Expected: no output — every harness/orchestrator/subagent literal `0` has been replaced; only the two documented gateway files retain `total_tokens: 0`.
