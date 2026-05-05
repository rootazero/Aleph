# Stage 5b — Guardrails Pipeline (#9) — ToolCall Callsite + on_model_fallback

**Status:** Plan
**Spec date:** 2026-05-06
**Depends on:** Stage 5a (`docs/superpowers/specs/2026-05-05-harness-stage5a-guardrails-pipeline-plan.md`, shipped)
**Closes:** Master roadmap § Stage 5

---

## 1. Goal

Stage 5a shipped the input + output guardrail callsites and the `GuardrailRegistry`
seam. Stage 5b closes module #9 by:

1. Wiring the **ToolCall** callsite at `agent.rs::act()` so each
   `tools.execute(...)` is gated by `registry.evaluate_tool_call(name, &args)`.
   - `Block` → emit `ToolError`, fire `on_safety_block`, **skip THIS call**
     (do NOT abort the rest of the batch).
   - `Sanitize` → re-parse `replacement.text` into `call.arguments` then proceed.
   - `Allow` / `Warn` → pass through (Warn logs at `tracing::warn!` once).
2. Wiring the **`on_model_fallback`** callback (currently dead at
   `callback.rs:67`). On `ErrorClass::Transient` failure of `self.deps.llm`,
   the harness retries once with `self.deps.fallback_llm` (new
   `Option<Arc<dyn AiProvider>>` field). On success the callback fires with the
   primary error reason + the fallback provider's display name.

Sub-stage split rationale (master spec § Stage 5): the two pieces are
independent (one is mid-batch tool gating, the other is Think-phase provider
swap) and naturally split test surfaces. Keeping them in the same PR is fine
here because both touch only `agent.rs` + `deps.rs` and the combined diff is
well under the 600-line threshold.

---

## 2. Architecture

### 2.1 ToolCall callsite

The call goes **after** `SessionEvent::ToolCallRequested` is emitted (so the
audit log records the original args even when blocked) and **before**
`self.deps.tools.execute(...)`. Layout (pseudocode):

```rust
// agent.rs::act()  (between line 466 and line 468)
if let Some(registry) = self.deps.guardrails.as_ref() {
    match registry.evaluate_tool_call(&call.name, &call.arguments).await {
        GuardrailDecision::Allow => { /* fall through */ }
        GuardrailDecision::Warn { reason } => {
            tracing::warn!(?session_id, tool = %call.name, reason = %reason,
                "tool-call guardrail warned");
        }
        GuardrailDecision::Sanitize(rep) => {
            // Replace args with parsed JSON; on parse failure, fall back to
            // a JSON string Value so the tool still gets *something* (it
            // will likely reject, which is the safe default).
            call.arguments = serde_json::from_str(&rep.text)
                .unwrap_or_else(|_| serde_json::Value::String(rep.text));
            tracing::info!(?session_id, tool = %call.name, source = %rep.source,
                "tool-call args sanitized");
        }
        GuardrailDecision::Block { reason, class: _ } => {
            callback.on_safety_block(&reason);
            // Persist as ToolError so the next Think turn sees it as a
            // failed tool result (is_error=true), and continue the batch.
            let error_event = SessionEvent::ToolError {
                turn_id,
                call_id: call.id.clone(),
                error: format!("guardrail blocked: {reason}"),
                at: now_ms(),
            };
            if let Err(emit_err) =
                self.deps.session.emit_event(session_id, error_event).await
            {
                tracing::warn!(?session_id, ?emit_err,
                    "failed to persist guardrail-block ToolError");
            }
            self.emit(/* ToolCallCompleted with Error result */);
            continue;  // skip THIS call, batch continues
        }
    }
}
```

Note `call` is moved into the `for` loop in current code (`for call in tool_calls`);
to mutate `call.arguments` on `Sanitize` we change the bind to `mut call` —
small local edit, no semantic side-effect.

### 2.2 on_model_fallback wiring

`HarnessDeps` gets one new optional field:

```rust
pub fallback_llm: Option<Arc<dyn AiProvider>>,
```

The Think-phase LLM call site (lines 244-278) is wrapped:

```rust
let primary_result = /* existing tokio::select! match */;
let response = match primary_result {
    Ok(r) => r,
    Err(primary_err) => {
        // Only attempt fallback for Transient errors (network, rate-limit,
        // provider-side glitches). Fixable / Unexpected / Recoverable bypass.
        let class = primary_err.class();
        if class == ErrorClass::Transient {
            if let Some(fallback) = self.deps.fallback_llm.as_ref() {
                let reason = primary_err.to_string();
                let fb_name = fallback.name();  // assumes AiProvider::name()
                match fallback.process(payload_clone).await {
                    Ok(r) => {
                        callback.on_model_fallback(&reason, fb_name);
                        r
                    }
                    Err(fb_err) => {
                        tracing::warn!(primary = %reason, fallback = %fb_err,
                            "fallback provider also failed");
                        return Err(HarnessError::Llm(primary_err));
                    }
                }
            } else {
                return Err(HarnessError::Llm(primary_err));
            }
        } else {
            return Err(HarnessError::Llm(primary_err));
        }
    }
};
```

**Provider naming:** `AiProvider` already exposes `fn name(&self) -> &str` (or
similar — verified during implementation; if absent we either add it as a
default-method `&'static str` in the trait or use type_name as a fallback).

**Cancellation:** the fallback path also runs inside the existing
`parent_cancel.cancelled()` race so cancel still wins.

**Timeout:** the fallback also respects `turn_timeout` (separate budget,
not shared, since the primary already burned its budget).

### 2.3 No bigger refactor

We deliberately do NOT:
- Wire `FailoverProvider` here. That's a multi-tier failover with health
  monitoring; this seam is single-step "primary→fallback, once". Users who
  want N-tier failover wrap their primary in `FailoverProvider` and pass that
  as `deps.llm` — orthogonal to the harness callback.
- Add a fallback chain (`Vec<Arc<dyn AiProvider>>`) — YAGNI: zero current
  consumers want more than one fallback at this seam.
- Reuse Stage 1 `consecutive_failure_cap` mechanics — that operates on
  Tool failures, not LLM failures.

---

## 3. File Structure

| File | Δ | Reason |
| --- | --- | --- |
| `src/harness/deps.rs` | +5 | new `fallback_llm: Option<Arc<dyn AiProvider>>` field + doc |
| `src/harness/agent.rs` | ~+90/−4 | ToolCall callsite (~50) + Think fallback (~40) |
| `src/harness/tests/guardrails.rs` | +60 | ToolCall Block/Sanitize/Allow integration tests |
| `src/harness/tests/think.rs` | +50 | fallback-triggered test |
| `src/guardrails/tests/loom.rs` | new ~40 | loom registry concurrent evaluate vs disable_all |
| `src/guardrails/mod.rs` | +1 | `mod loom;` (cfg(loom)) |
| All other HarnessDeps construction sites (~17) | +1 each | `fallback_llm: None,` |
| `CHANGELOG.md` | +6 | unreleased entry |
| `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md` | ~3 | flip Stage 5 to ✅ Shipped |

Estimated harness delta: agent.rs ~+90 → 1465 lines (under 1500 cap, ~35
headroom). If we run over, we extract a `apply_tool_call_guardrail` helper
into a free function (mirrors `apply_input_guardrail` from 5a).

---

## 4. Acceptance

1. ToolCall callsite enforced at `agent.rs::act()`:
   - Block → `ToolError` event persisted, `on_safety_block` fired, batch continues.
   - Sanitize → args replaced, tool executes with sanitized args.
   - Allow → no overhead beyond a registry lookup.
2. `on_model_fallback` fires when:
   - Primary returns `ErrorClass::Transient`, AND
   - `deps.fallback_llm` is `Some(provider)`, AND
   - Fallback returns Ok.
3. Tests:
   - ≥1 integration test for ToolCall Block path (asserts batch continues for siblings).
   - ≥1 integration test for ToolCall Sanitize path (asserts tool sees rewritten args).
   - ≥1 integration test for fallback-engaged path (asserts callback fires + Think completes via fallback).
   - ≥1 loom test for `GuardrailRegistry` (concurrent `evaluate_tool_call` vs `disable_all`).
4. R10 audit:
   - `src/harness/` line cap respected (agent.rs ≤ 1500).
   - 9-file harness module list unchanged.
   - No new harness sub-modules.
5. CHANGELOG entry under `[Unreleased]`.
6. Master roadmap spec status flipped to `✅ Shipped` for Stage 5.
7. Working tree clean; lib tests green; pre-existing baseline failures
   (`spawn_tool_allowlist_enforced_via_harness`, etc) unchanged.

---

## 5. Tasks

### Task 1 — ToolCall callsite

1.1 In `agent.rs::act()`, change `for call in tool_calls` → `for mut call in tool_calls`.
1.2 Insert the guardrail block between `ToolCallRequested` emit (line 466)
    and `tools.execute` (line 468).
1.3 On `Block`, emit `SessionEvent::ToolError`, fire `on_safety_block`, emit
    `ToolCallCompleted { result: Error { error, retryable: false } }`, `continue`.
1.4 On `Sanitize`, mutate `call.arguments`, log at `tracing::info!`.
1.5 On `Warn`, log at `tracing::warn!`.
1.6 Verify with `cargo check -p alephcore --lib`.

### Task 2 — HarnessDeps `fallback_llm`

2.1 Add `pub fallback_llm: Option<Arc<dyn AiProvider>>,` to `HarnessDeps`
    with doc comment describing single-step Transient retry semantics.
2.2 Update all ~17 construction sites with `fallback_llm: None,`. For
    `subagent_spawner.rs:195`, propagate parent's value (`fallback_llm: deps.fallback_llm.clone()`).
2.3 Verify with `cargo check --tests`.

### Task 3 — Think-phase fallback wiring

3.1 Confirm `AiProvider` exposes a `name()` method (or a similar identifier).
    If not, add a default trait method returning `std::any::type_name::<Self>()`
    or extend the trait with an explicit `&str`.
3.2 Refactor lines 244-278 to capture the LLM result without early `?` on
    `Err`. Keep `parent_cancel` and `turn_timeout` racing semantics intact.
3.3 On `Err(primary)` with `class()==Transient` and `fallback_llm.is_some()`,
    invoke fallback (with fresh `turn_timeout` if set, racing against
    `parent_cancel`). On Ok, fire `callback.on_model_fallback(&primary_err.to_string(), fallback.name())`.
3.4 Verify with `cargo check -p alephcore --lib`.

### Task 4 — Tests

4.1 In `src/harness/tests/guardrails.rs`, add three tests:
    - `tool_call_block_skips_one_call_but_continues_batch`
    - `tool_call_sanitize_rewrites_args`
    - `tool_call_allow_passes_through`
    Build a `MultiToolService` test double that captures the args each tool
    gets, plus a `BlockingGuardrail` impl of `ToolCallGuardrail`.
4.2 In `src/harness/tests/think.rs`, add `fallback_llm_engaged_on_transient_error`:
    primary provider returns `AlephError::NetworkError`, fallback returns Ok,
    assert `CapturingCallback::on_model_fallback` was invoked.
4.3 In `src/guardrails/tests/`, add new file `loom.rs` (cfg(loom)) with
    `concurrent_evaluate_vs_disable_all` scenario.
4.4 Wire `mod loom;` into `src/guardrails/mod.rs` test list (cfg(loom) gated).
4.5 Run `cargo test -p alephcore --lib` and confirm all new tests pass and
    baseline counts only increased (no new breakage).

### Task 5 — CHANGELOG + master spec

5.1 Append to `## [Unreleased]` § Added an entry: "Stage 5b — Guardrails
    ToolCall callsite + on_model_fallback wiring (closes module #9)."
5.2 In master roadmap spec, replace Stage 5 row's status with
    `✅ Shipped on 2026-05-06 · plan: <this-doc> · 5a + 5b complete`.
5.3 Verify both edits with `git diff`.

### Task 6 — Commit chain

Same atomic-commit cadence as 5a:
1. `docs: add Stage 5b Guardrails ToolCall + fallback plan`
2. `feat(harness): wire Stage 5b ToolCall callsite + fallback_llm seam`
3. `test(harness/guardrails): tool-call + fallback integration + loom`
4. `docs: ship Stage 5 — flip master spec + CHANGELOG`

---

## 6. CHANGELOG entry (draft)

```markdown
### Added (Harness Stage 5b — closes #9 Guardrails Pipeline)
- Tool-call guardrail callsite at `AgentHarness::act` — `Block` skips a single
  tool call (batch continues), `Sanitize` rewrites args, `Allow`/`Warn` pass
  through. Reuses the `GuardrailRegistry` shipped in 5a.
- `HarnessDeps.fallback_llm: Option<Arc<dyn AiProvider>>` — single-step
  Transient-error fallback at the Think-phase LLM call. Fires the previously
  dead `HarnessCallback::on_model_fallback` when the fallback succeeds.
- Loom test for `GuardrailRegistry` covering concurrent `evaluate_tool_call`
  vs `disable_all`.
- Master roadmap Stage 5 ✅ Shipped (5a + 5b complete).
```

---

## 7. Verification matrix

| Item | Method |
| --- | --- |
| ToolCall Block path | guardrails integration test asserts: 2-call batch with first blocked, second sees `tools.execute` and the original `ToolError` for #1 in session log |
| ToolCall Sanitize path | tool service double records args, asserts they equal the replacement JSON |
| ToolCall Allow / no registry | existing tests still pass — zero new failures |
| Fallback path | think test asserts `CapturingCallback.fallback_calls.len() == 1` and `response.text == fallback's reply` |
| Loom registry | `RUSTFLAGS=--cfg loom cargo test --lib --features loom guardrails::tests::loom` exits 0 |
| R10 line cap | `wc -l src/harness/agent.rs ≤ 1500` |
| Pre-existing flake | `spawn_tool_allowlist_enforced_via_harness` still fails (orthogonal); not a regression |

---

## 8. Out of scope

- Multi-tier provider failover (use existing `FailoverProvider` as `deps.llm`).
- Per-tool guardrail allowlists (a registry-level enabling flag is the
  current granularity; a future `Stage 5c` could add per-tool config if
  evidence demands).
- Auto-replay of the model turn after `Sanitize` on the **output** path (5a
  decision: Sanitize replaces text in-place; the model is not re-prompted —
  that's a heavier semantics change and orthogonal to 5b).
- ProviderRegistry / health-monitor integration. The single fallback hook is
  enough for current consumers.

---

## 9. Rollback

- `HarnessDeps.fallback_llm` is `Option`-seamed — set to `None` in production
  to disable. No code path required to remove.
- `GuardrailRegistry::disable_all()` (already shipped in 5a) flips an
  `AtomicBool`; works for the new ToolCall path identically.
- Reverting the commit chain leaves Stage 5a entirely intact (no shared state
  changes beyond field additions).
