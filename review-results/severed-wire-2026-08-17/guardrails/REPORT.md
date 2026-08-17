# Severed-Wire Audit — `src/guardrails`

- **Date:** 2026-08-17
- **Working tree:** `.worktrees/review-fix-2026-08-17` (HEAD, not the graph commit 9841b5b2 — every claim below was re-verified with `rg` against current code)
- **Method:** PRODUCED–CONSUMED symbol parity via `rg` (per `REVIEW_PROTOCOL.md`). All sweeps used `rg -n`, not `grep -n` (the CRLF `grep -n` quirk documented in `review-results/audit-cmd/seam.md`).
- **Read-only:** no source files modified. Only these two report files were written.

## Scope

| File | LOC | Notes |
|---|---|---|
| `src/guardrails/decision.rs` | 103 | `GuardrailDecision` + `Replacement` + 4 predicate helpers |
| `src/guardrails/mod.rs` | 28 | module + re-exports |
| `src/guardrails/pii_secrets.rs` | 474 | `PiiSecretsGuardrail` (3 constructors) |
| `src/guardrails/registry.rs` | 290 | `GuardrailRegistry` + builder + `SessionInputScreen` |
| `src/guardrails/traits.rs` | 35 | 3 trait surfaces |
| `src/guardrails/tests/{bench,input,output,registry}.rs` | 563 | read; test-only consumers do NOT count as production |

## Headline verdict

**The module is NOT severed as a whole.** All three trait surfaces are consumed at live harness callsites, the registry is built at boot from the `[guardrails]` config section, threaded through `HarnessDeps` into the main harness AND inherited by subagents. 8 severed-wire findings: 2 dead-or-test-only code surfaces, 1 inert runtime knob, 5 doc/path drifts. No form-3 (stale references to renamed symbols) and no form-6 markers (`#[allow(dead_code)]`, `#[deprecated]`) exist in the module.

### Verified-live wiring (for the record)

- Input surface: `GuardrailRegistry::screen_session_input` called at `src/harness/agent/think.rs:348` (`rg "screen_session_input" src/ bin/ interfaces/ shared/` → `src/harness/agent/think.rs:348`, plus test hits).
- Output surface: `registry.evaluate_output(&text)` at `src/harness/agent/think.rs:850` and `:1293` (grace-turn salvage path).
- Tool-call surface: `apply_tool_call_guardrail` at `src/harness/agent/guardrails.rs:23`, called from `src/harness/agent/act.rs:463` and `:800`.
- Boot: `build_guardrail_registry` (`src/bin/aleph-server/commands/start/orchestrator_init.rs:436`) reads `config.guardrails.enabled` + `secrets_config`, wires `PiiSecretsGuardrail::with_guard_and_resolver` (line 485) via `GuardrailRegistry::builder().with_input/with_output/with_tool_call` (lines 488–491); result stored at `orchestrator_init.rs:343`, threaded to `HarnessDeps` (`src/harness/deps.rs:91`) via `src/orchestrator/harness_bridge/mod.rs:97`.
- Subagent inheritance: `SubagentToolBuilder::with_guardrails` (`src/agents/subagent_tool/mod.rs:256`), populated at `src/gateway/execution_engine/run_loop/inner.rs:1084–1085`.
- `output_count()` consumed in production at `src/harness/agent/think.rs:141` (`may_stream_deltas` gate). `input_count`/`tool_call_count` have only test consumers (see F5-adjacent note; the counters themselves stay — `output_count` proves the pattern is load-bearing).
- Cross-crate: `rg -n "guardrails" interfaces/ shared/ desktop/` → **zero hits**. The module is consumed only inside `src/` (core) and `src/bin/` (server boot). Expected for an internal core seam (R1/R3), not a severed wire.

---

## Findings

### F1 — `GuardrailRegistry::{disable_all, enable_all, is_enabled}`: kill-switch declared, never wired (medium, DECIDE)

**Produced:** `src/guardrails/registry.rs:60` (`disable_all`), `:64` (`enable_all`), `:54` (`is_enabled`).
Module doc (registry.rs:7–8) calls this "the high-risk runtime rollback knob from master spec § Stage 5"; `src/harness/agent/think.rs:133` also references the "`disable_all` runtime kill-switch" as a live concept.

**Consumers (rg):**
```
$ rg -n "disable_all" src/ bin/ interfaces/ shared/
src/guardrails/registry.rs:7   (doc comment)
src/guardrails/registry.rs:60  (definition)
src/guardrails/tests/bench.rs:50
src/guardrails/tests/registry.rs:1,107,113,153,165,183
src/acp/tests.rs:374           (test fn name; calls AcpAdapterEntry preset enablement, NOT the registry)
src/harness/agent/think.rs:133 (doc comment only)
```
```
$ rg -n "enable_all" src/ bin/ interfaces/ shared/   (excluding src/guardrails/)
src/bin/aleph-server/main.rs:220   → tokio::runtime::Builder::enable_all()  (unrelated)
src/extension/registrar/mcp_registrar.rs:91 → tokio::runtime::Builder::enable_all() (unrelated)
src/tools/scoped/tests.rs:761      → ProgressiveDisclosureRewriter tests (unrelated)
```
```
$ rg -n "GuardrailRegistry::is_enabled|registry.is_enabled|\.is_enabled\(\)" src/guardrails/
src/guardrails/registry.rs:54  (definition)
src/guardrails/registry.rs:79,150,182,200  (internal production callers inside evaluate_* / screen_user_messages)
src/guardrails/tests/bench.rs:53
src/guardrails/tests/registry.rs:114,123
```

**Analysis:** `disable_all`/`enable_all` have **zero production consumers** — the only calls are in the module's own tests (`bench.rs`, `tests/registry.rs`). No admin command, IPC handler, ACP tool, or config-reload path invokes them (`rg -i "kill.?switch|rollback.*guardrail" src/ bin/` → only loop/goal/broadcast kill-switches, unrelated). `is_enabled` is consumed internally by the registry's own evaluate paths (so not dead code), but its `pub` surface has no external production caller.

**Form:** 2 (declared-but-never-wired stub — full implementation, nothing calls it in production).
**Severity:** medium (inert-but-meaningful surface; the module's own doc promises a runtime rollback knob that cannot be triggered).
**Decision:** **DECIDE** — the "master spec § Stage 5" reference in the doc means the intent may be real:
- Option A (CONNECT): wire an operator surface — e.g. an ACP tool or a gateway admin command that calls `disable_all()` — and keep the trio.
- Option B (CUT): delete `disable_all`/`enable_all` (registry.rs:60–66), downgrade `is_enabled` to non-pub (it has internal callers), and fix the two doc references (registry.rs:7–8, think.rs:133). Zero runtime behavior change either way — the `AtomicBool` is always `true` today.
**Risk:** none to runtime; the only risk of cutting is dropping a documented-but-unwired spec feature.

---

### F2 — `PiiSecretsGuardrail::new`: zero consumers anywhere (low, CUT)

**Produced:** `src/guardrails/pii_secrets.rs:38–44`.
Doc: "Construct over an existing orchestrator with no resolver. Placeholder substitution at the `tool_call` surface will be inert."

**Consumers (rg):**
```
$ rg -n "PiiSecretsGuardrail::new" src/ bin/ interfaces/ shared/
(no output — zero hits, including tests)
$ rg -n "PiiSecretsGuardrail" src/ bin/ interfaces/ shared/
src/bin/aleph-server/commands/start/orchestrator_init.rs:485   → ::with_guard_and_resolver(...)
src/guardrails/pii_secrets.rs:340,456                          → ::with_resolver(...) in #[cfg(test)] mods
```
**Analysis:** neither production nor tests use `new`. The boot path uses `with_guard_and_resolver`; the tests use `with_resolver`. This constructor is fully dead.
**Form:** 1 (visible symbol, zero production consumers).
**Severity:** low (pure dead code, harmless leftover).
**Decision:** **CUT** — delete `pii_secrets.rs:38–44`. No migration needed (no call sites exist).
**Risk:** none. Verification: `rg -n "PiiSecretsGuardrail::new" src/ bin/ interfaces/ shared/` stays empty; `cargo check` clean (not run here per audit constraints).

---

### F3 — `PiiSecretsGuardrail::with_resolver`: test-only, with a stale "boot path" doc (low, CUT)

**Produced:** `src/guardrails/pii_secrets.rs:59–62`.
Doc (lines 56–58): "Convenience for the boot path." — but the real boot path (`orchestrator_init.rs:485`) uses `with_guard_and_resolver`; `with_resolver` also drops the audit channel, which the doc itself says callers must avoid.

**Consumers (rg):**
```
$ rg -n "with_resolver" src/ bin/ interfaces/ shared/
src/guardrails/pii_secrets.rs:59   (definition)
src/guardrails/pii_secrets.rs:340  (#[cfg(test)] mod delegation_tests)
src/guardrails/pii_secrets.rs:456  (#[cfg(test)] mod input_blocking_tests)
src/extension/runtime/wasm/mod.rs:75,77,88  (load_plugin_with_resolver — unrelated)
```
**Analysis:** the only callers are two `#[cfg(test)]` modules inside `pii_secrets.rs`. Its stated purpose ("boot path") is not what production does.
**Form:** 4 (produced, consumed only by tests) + 5 (doc describes a purpose that no longer exists).
**Severity:** low.
**Decision:** **CUT** — delete `pii_secrets.rs:56–62`; migrate the two test call sites to `PiiSecretsGuardrail::with_guard_and_resolver(Arc::new(RuntimeSecurityGuard::default_guard()), resolver)` (delegation_tests at :340, input_blocking_tests at :456).
**Risk:** none (tests only; behavior identical — `with_resolver` is exactly that constructor).

---

### F4 — `GuardrailDecision::{is_block, is_allow, is_sanitize, is_warn}`: consumed only by tests (low, DECIDE)

**Produced:** `src/guardrails/decision.rs:44, 48, 52, 56` (`#[must_use] pub const fn`).

**Consumers (rg):**
```
$ rg -n "\.is_block\(\)" src/ bin/ interfaces/ shared/
src/guardrails/decision.rs:68,78,90,100        (decision.rs #[cfg(test)] mod)
src/guardrails/tests/registry.rs:98,99,103,124,134
src/guardrails/tests/output.rs:38
$ rg -n "\.is_allow\(\)" src/ bin/ interfaces/ shared/
src/guardrails/decision.rs:67,79,89,99        (tests)
src/guardrails/tests/registry.rs:83,84,88,115,116,120,208
src/guardrails/tests/input.rs:49
src/guardrails/tests/output.rs:49
$ rg -n "\.is_sanitize\(\)" src/ bin/ interfaces/ shared/
src/guardrails/decision.rs:69,88,101          (tests)
src/guardrails/tests/input.rs:37
$ rg -n "\.is_warn\(\)" src/ bin/ interfaces/ shared/
src/guardrails/decision.rs:98                 (test)
```
**Analysis:** all three production call sites (`think.rs:850`, `think.rs:1293`, `harness/agent/guardrails.rs:37`) match on `GuardrailDecision` directly and never call the predicates. The four helpers are test-only assertion sugar.
**Form:** 4.
**Severity:** low.
**Decision:** **DECIDE** — both options are safe and low-churn:
- Keep: they are self-documenting predicates on a public enum and serve as the test assertion surface (~12 call sites).
- CUT: replace test assertions with `matches!`/direct matching. Removal cannot break runtime behavior (no production consumer), but the diff churns 5 test files for zero runtime gain.
No doc/contract promises the API; recommend keeping unless a code-size pass targets them.

---

### F5 — `GuardrailRegistry::empty`: consumed only by tests (low, CUT)

**Produced:** `src/guardrails/registry.rs:45–52`.

**Consumers (rg):**
```
$ rg -n "GuardrailRegistry::empty" src/ bin/ interfaces/ shared/
src/guardrails/tests/bench.rs:13,35,48
src/guardrails/tests/registry.rs:82
src/harness/agent/think.rs:1571   (#[test] fn may_stream_deltas_streams_for_input_or_tool_call_only_registry)
```
**Analysis:** zero production callers. The boot path expresses "no guardrails" as `None` (`orchestrator_init.rs:446`), never as `Some(empty-registry)`; `may_stream_deltas` (think.rs:141) treats `None` and an empty registry identically for the gate. `empty()` is semantically identical to `GuardrailRegistryBuilder::default().build()`.
**Form:** 4.
**Severity:** low.
**Decision:** **CUT** — delete `registry.rs:45–52`; migrate the 5 test call sites to `GuardrailRegistry::builder().build()` (identical struct).
**Risk:** none. Verification: `rg -n "GuardrailRegistry::empty" src/ bin/ interfaces/ shared/` → no hits after migration.

---

### F6 — decision.rs doc: "control flow does NOT branch on `class`" contradicts the live output call-site (low, DECIDE)

**Produced (drift):** `src/guardrails/decision.rs:21–27` — the `Block` variant doc claims "control flow does NOT branch on it today: the output call-site turns every `Block` into a terminal `HarnessError`…".

**Evidence (rg):**
```
$ rg -n "control flow does NOT branch" src/guardrails/decision.rs
src/guardrails/decision.rs:23
$ sed -n '860,866p' src/harness/agent/think.rs
crate::guardrails::GuardrailDecision::Block { reason, class } => {
    callback.on_safety_block(&reason);
    let msg = format!("output guardrail blocked: {reason}");
    // Preserve the guardrail's ErrorClass through the wrapped
    // AlephError: `Fixable` (… ) → a Fixable-classed error; everything else …
    let err = match class {
        crate::error::ErrorClass::Fixable => crate::error::AlephError::Validation(msg),
        _ => crate::error::AlephError::other(msg),
    };
```
**Analysis:** the output call-site at `think.rs:860–864` **does** branch on `class` — `Fixable` → `AlephError::Validation`, everything else → `AlephError::other`. The doc's blanket "does NOT branch on it today" is stale (the input path `think.rs:348–352` and tool-call path `guardrails.rs:58` do ignore `class`, so only the output-call-site claim is wrong). The rest of the note — orchestrator classifies harness errors by message, phase6c TODO — is still accurate.
**Form:** 5 (name-drift residue: doc describes a reality that no longer exists).
**Severity:** low (misleading doc; risks a future reader "fixing" the branch back to unconditional).
**Decision:** **DECIDE** — rewrite decision.rs:23–25 to state that the output call-site selects `AlephError::Validation` vs `other` from `class` (input/tool-call paths ignore it). Doc-only change, no code touch.

---

### F7 — traits.rs doc: callsites point at `agent.rs::run_turn_internal`, fn lives in `think.rs` (low, DECIDE)

**Produced (drift):** `src/guardrails/traits.rs:6` — "All three callsites are live in `agent.rs::run_turn_internal` (input/output) and `agent/act.rs` (tool-call); see also `apply_tool_call_guardrail`."

**Evidence (rg):**
```
$ rg -n "run_turn_internal" src/harness/
src/harness/agent.rs:459,533   (caller loop + call, NOT the definition)
src/harness/agent/think.rs:299 (pub(crate) async fn run_turn_internal — definition)
src/guardrails/traits.rs:6     (the stale reference)
```
Input callsite: `think.rs:348`; output callsites: `think.rs:850` and `:1293`. `agent/act.rs` is correct for tool-call (calls `apply_tool_call_guardrail` at act.rs:463, 800).
**Analysis:** the trait doc sends readers to the wrong file for the definition — `run_turn_internal` is in `think.rs`, not `agent.rs` (agent.rs only invokes it at :533). This matches the documented "−55 护栏下沉" move of input screening into `screen_session_input`.
**Form:** 5 (stale path reference).
**Severity:** low.
**Decision:** **DECIDE** — update traits.rs:6 to point at `think.rs::run_turn_internal` (input/output) and `agent/act.rs` + `agent/guardrails.rs` (tool-call). Doc-only.

---

### F8 — tests/input.rs + tests/output.rs: "once the callsite / Task 4 lands" is stale (low, DECIDE)

**Produced (drift):**
- `src/guardrails/tests/input.rs:1–2` — "Input guardrail integration tests are wired in Task 4 **once the harness callsite lands**. This file currently only exercises the registry surface…"
- `src/guardrails/tests/output.rs:2–3` — "The harness-level callsite integration test lives in `src/harness/tests/` **once Task 4 lands**…"

**Evidence (rg):**
```
$ rg -n "screen_session_input" src/harness/agent/think.rs
src/harness/agent/think.rs:348   (callsite landed)
$ ls -la src/harness/tests/guardrails.rs
-rw-rw-r-- 1 zou zou 45927 … src/harness/tests/guardrails.rs   (45 KB integration suite exists)
```
**Analysis:** the harness input callsite landed (think.rs:348, plus output at :850/:1293 and tool-call at act.rs:463/:800), and `src/harness/tests/guardrails.rs` (45 KB, ~1300 lines) is exactly the harness-level integration suite the output.rs header says will exist "once Task 4 lands". Both headers describe a past state.
**Form:** 5 (comment residue).
**Severity:** low.
**Decision:** **DECIDE** — rewrite the two headers to state the current reality (registry-surface unit tests here; callsite integration tests in `src/harness/tests/guardrails.rs`). Doc-only.

---

## Summary table

| ID | Symbol / surface | Form | Severity | Decision |
|---|---|---|---|---|
| sw-gr-1 | `disable_all` / `enable_all` / `is_enabled` kill-switch (registry.rs:54,60,64) | 2 | medium | DECIDE (CONNECT via operator surface, or CUT) |
| sw-gr-2 | `PiiSecretsGuardrail::new` (pii_secrets.rs:38) | 1 | low | CUT |
| sw-gr-3 | `PiiSecretsGuardrail::with_resolver` (pii_secrets.rs:59) | 4+5 | low | CUT |
| sw-gr-4 | `GuardrailDecision::{is_block,is_allow,is_sanitize,is_warn}` (decision.rs:44–58) | 4 | low | DECIDE |
| sw-gr-5 | `GuardrailRegistry::empty` (registry.rs:45) | 4 | low | CUT |
| sw-gr-6 | decision.rs:23 doc "does NOT branch on class" vs think.rs:860 | 5 | low | DECIDE |
| sw-gr-7 | traits.rs:6 doc `agent.rs::run_turn_internal` vs think.rs:299 | 5 | low | DECIDE |
| sw-gr-8 | tests/input.rs:1, tests/output.rs:2 stale "Task 4 / callsite lands" | 5 | low | DECIDE |

**Totals:** 8 findings — 0 critical, 0 high, 1 medium, 7 low. Decisions: 3 CUT, 0 CONNECT (as final), 5 DECIDE.

## Deliberately skipped / not audited

- **No cargo runs** (protocol constraint). All claims are static `rg` parity.
- **Subagent plumbing** (`agents/subagent_spawner`, `agents/runtime.rs`, `orchestrator/dispatch.rs:619`, `runner_impl.rs:27`, `run_loop/inner.rs:1084`) — spot-checked only to confirm `GuardrailRegistry` is threaded to spawned subagents; not depth-audited (outside module scope).
- **`RuntimeSecurityGuard` / `secrets` / `security` modules** — consumed dependencies, not producers; their own wiring is out of scope. `pii-batch-3` Seam-G previously stamped `pii_secrets.rs` wiring OK (PiiEngine-independent); that remains true and does not overlap F2/F3 (constructor-level dead code, not delegation).
- **`GuardrailsToml` config** (`src/config/types/phase6_wiring.rs:19`, single `enabled` field) — read by boot, live; no finding. Note for F1: the config has no kill-switch field, so `disable_all` is the *only* intended kill-switch surface and it is unwired.
- **Master-spec § Stage 5 kill-switch intent** — cannot be verified from code; cited as the reason F1 is DECIDE rather than CUT.
- **`bin/`, `interfaces/`, `shared/`, `desktop/`** were swept for all candidate symbols (zero guardrails hits in interfaces/shared/desktop); `bin/` consumers are all in `orchestrator_init.rs`.
- No style/lint nits reported (out of scope).
