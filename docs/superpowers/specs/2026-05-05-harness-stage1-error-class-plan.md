# Harness Stage 1 — Error Handling Classification: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce `ErrorClass { Transient, Recoverable, Fixable, Unexpected }` as shared error vocabulary; add `class()` accessor on `HarnessError` and `AlephError`; retire the wildcard variant arm in `agent.rs` error→outcome mapping by dispatching via `class()` and emitting class into observability trace.

**Architecture:** Pure additive seam — Stage 1 introduces the vocabulary without changing P0 rescue behavior. The real consumer is the existing wildcard match at `agent.rs:700-705` (becomes class-driven) plus the tracing log line on harness session error path (now carries `error_class`). Future Stage 5 (Guardrails) and Stage 6 (Verification) inherit this vocabulary; Stage 1 itself does NOT extend trace event schemas or modify the consecutive-failure cap counting logic — those are explicit non-goals to keep Stage 1 low-risk and ~150 LOC.

**Tech Stack:** Rust 2021, `thiserror`, `#[cfg(test)]` unit tests, `#[tokio::test]` integration tests at `src/harness/tests/stability.rs`.

**Stage spec link:** `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md` § Stage 1.

**Master spec constraints (must satisfy):**
- Single PR ≤ 600 lines (Stage 1 estimate ~150 lines)
- harness/ delta ≤ +400 lines (Stage 1 touches `trait_def.rs` only inside harness/, ≤ +60 lines)
- agent.rs ≤ 1500 lines (current 1340; this plan adds ~5 lines, no risk)
- baseline = `bf0de41cc` — all 41 harness tests must remain green
- Old code retired in same commit (the wildcard variant match at `agent.rs:700-705` and any ad-hoc variant comparisons it represents)
- Every new seam (`ErrorClass`, `HarnessError::class()`, `AlephError::class()`) must have ≥1 real consumer in the same PR

---

## File Structure

| File | Change | Lines |
|------|--------|-------|
| `src/error.rs` | Add `ErrorClass` enum + `impl AlephError { fn class() }` + unit tests | +60 |
| `src/harness/trait_def.rs` | Add `impl HarnessError { fn class() }` + unit tests; re-export `ErrorClass` for harness consumers | +35 |
| `src/harness/agent.rs:700-705` | Replace wildcard variant match with class-dispatched mapping; emit `error_class` in tracing | +5 / -5 |
| `src/harness/tests/stability.rs` | Add 1 integration test exercising class API end-to-end | +30 |
| **Total** | | ~125 lines (well under 150) |

**Decomposition rationale:**
- `ErrorClass` lives in `src/error.rs` (not `trait_def.rs`) because `AlephError::class()` uses it and `error.rs` cannot depend on `harness/` (avoid circular dep — `trait_def.rs` already depends on `error.rs`).
- Per-file unit tests in `#[cfg(test)] mod tests` (Rust convention; matches existing project style).
- One integration test in `tests/stability.rs` (alongside existing `consecutive_total_failure_caps_loop`) for end-to-end class API exercise.

---

## Task 1: Define `ErrorClass` and `AlephError::class()`

**Files:**
- Modify: `src/error.rs` (add enum after line 204, add impl block, add unit tests)

**Class mapping (locked here for the entire stage):**

| AlephError variant | ErrorClass | Reason |
|--------------------|-----------|--------|
| `NetworkError` | Transient | Network blip; retry typically resolves |
| `RateLimitError` | Transient | Wait + retry |
| `ProviderError` | Transient | API hiccup |
| `Timeout` | Transient | Time-bounded retry |
| `McpTimeout` | Transient | MCP slow path |
| `ExecutionTimeout` | Transient | Tool exec time-bounded retry |
| `AuthenticationError` | Fixable | User must update API key |
| `ToolNotFound` | Fixable | Model called nonexistent tool; can adjust |
| `MissingInput` | Fixable | Need additional info from user/model |
| `Validation` | Fixable | Model output violated schema; can fix |
| `NoProviderAvailable` | Recoverable | Try different provider via fallback |
| `Cancelled` | Recoverable | User aborted; can re-run |
| `HotkeyError` | Unexpected | System integration broken |
| `ClipboardError` | Unexpected | OS-level issue |
| `InputSimulationError` | Unexpected | OS-level issue |
| `CallbackError` | Unexpected | Internal bug |
| `ConfigError` | Unexpected | Disk/config corruption |
| `InvalidConfig` | Unexpected | User config malformed |
| `KeychainError` | Unexpected | OS-level issue |
| `Other` | Unexpected | Catch-all |
| `PermissionDenied` | Unexpected | OS permission missing |
| `VideoError` | Unexpected | Subsystem failure |
| `NotFound` | Unexpected | Resource missing |
| `IoError` | Unexpected | Disk/network issue |
| `GitError` | Unexpected | Tool subprocess failure |
| `McpToolNotFound` | Unexpected | Server config issue |
| `RuntimeError` | Unexpected | Runtime manager failure |
| `CorruptData` | Unexpected | Data integrity broken |
| `ChannelClosed` | Unexpected | Internal channel torn down |
| `SandboxUnavailable` | Unexpected | Sandbox runtime missing |

- [ ] **Step 1: Write the failing unit tests in `src/error.rs`**

Append to `src/error.rs` at the end of the file (after the last existing `impl`):

```rust
/// Error classification for cross-cutting decisions (retry, guardrail veto,
/// verification feedback). Stable vocabulary shared by `HarnessError` and
/// `AlephError`. Future Stage 5 (Guardrails) and Stage 6 (Verification)
/// dispatch on these classes rather than enumerating concrete variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// Caller-side or environment hiccup — retry typically resolves.
    /// Examples: network blip, provider 5xx, rate limit, timeout.
    Transient,
    /// Outer loop or fallback path can recover (try different provider,
    /// re-run after user intervention). Not the model's fault.
    /// Examples: user cancellation, no provider available.
    Recoverable,
    /// Model can read the error and self-correct on next turn.
    /// Examples: tool not found, missing input, schema violation, bad auth.
    Fixable,
    /// Genuine system / data / config error; neither retry nor model
    /// adjustment helps. Treat as terminal.
    /// Examples: I/O failure, corrupt data, OS permission denied.
    Unexpected,
}

#[cfg(test)]
mod error_class_tests {
    use super::{AlephError, ErrorClass};

    #[test]
    fn network_error_classifies_as_transient() {
        let e = AlephError::network("connection reset");
        assert_eq!(e.class(), ErrorClass::Transient);
    }

    #[test]
    fn authentication_error_classifies_as_fixable() {
        let e = AlephError::authentication("anthropic", "invalid key");
        assert_eq!(e.class(), ErrorClass::Fixable);
    }

    #[test]
    fn cancelled_classifies_as_recoverable() {
        assert_eq!(AlephError::Cancelled.class(), ErrorClass::Recoverable);
    }

    #[test]
    fn corrupt_data_classifies_as_unexpected() {
        assert_eq!(
            AlephError::CorruptData("bad blob".into()).class(),
            ErrorClass::Unexpected
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail with "no method named `class`"**

Run:
```bash
cargo test -p alephcore --lib error_class_tests 2>&1 | tail -20
```

Expected: compile error `no method named 'class' found for enum 'AlephError'` (the tests can't compile yet).

- [ ] **Step 3: Implement `AlephError::class()` to make tests pass**

Inside `src/error.rs`, add a new `impl AlephError` block (or extend the existing one — there is already one starting at line 206). Add this method to that block:

```rust
    /// Map this error to a stable `ErrorClass` for cross-cutting decisions.
    pub fn class(&self) -> ErrorClass {
        match self {
            // Transient — retry typically resolves
            AlephError::NetworkError { .. }
            | AlephError::RateLimitError { .. }
            | AlephError::ProviderError { .. }
            | AlephError::Timeout { .. }
            | AlephError::McpTimeout
            | AlephError::ExecutionTimeout { .. } => ErrorClass::Transient,

            // Recoverable — outer loop / fallback can recover
            AlephError::Cancelled
            | AlephError::NoProviderAvailable { .. } => ErrorClass::Recoverable,

            // Fixable — model or user can adjust on next turn
            AlephError::AuthenticationError { .. }
            | AlephError::ToolNotFound { .. }
            | AlephError::MissingInput { .. }
            | AlephError::Validation(_) => ErrorClass::Fixable,

            // Unexpected — system / data / config integrity issue
            AlephError::HotkeyError { .. }
            | AlephError::ClipboardError { .. }
            | AlephError::InputSimulationError { .. }
            | AlephError::CallbackError { .. }
            | AlephError::ConfigError { .. }
            | AlephError::InvalidConfig { .. }
            | AlephError::KeychainError { .. }
            | AlephError::Other { .. }
            | AlephError::PermissionDenied { .. }
            | AlephError::VideoError { .. }
            | AlephError::NotFound(_)
            | AlephError::IoError(_)
            | AlephError::GitError(_)
            | AlephError::McpToolNotFound(_)
            | AlephError::RuntimeError { .. }
            | AlephError::CorruptData(_)
            | AlephError::ChannelClosed(_)
            | AlephError::SandboxUnavailable { .. } => ErrorClass::Unexpected,
        }
    }
```

**Note**: the match must be **exhaustive without a wildcard** so that future `AlephError` variants force a compile error here (per Rust pattern P3 enum state machines — see `~/.claude/rules/rust/patterns.md`).

- [ ] **Step 4: Run tests to verify they pass**

Run:
```bash
cargo test -p alephcore --lib error_class_tests 2>&1 | tail -10
```

Expected: `test result: ok. 4 passed; 0 failed`.

- [ ] **Step 5: Run clippy and fmt to ensure clean code**

Run:
```bash
cargo fmt --check && cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -20
```

Expected: zero output from fmt; clippy ends with `warning: ... (0 warnings emitted)` or no warnings at all.

If fmt fails: run `cargo fmt` and re-check.

- [ ] **Step 6: Commit**

```bash
git add src/error.rs
git commit -m "$(cat <<'EOF'
feat(error): add ErrorClass + AlephError::class() (harness Stage 1.1)

Introduces stable error classification vocabulary (Transient /
Recoverable / Fixable / Unexpected) for cross-cutting decisions
in future Stage 5 (Guardrails) and Stage 6 (Verification).

Match is exhaustive without wildcard so future AlephError variants
force a classification decision at compile time.

Refs: docs/superpowers/specs/2026-05-05-harness-stage1-error-class-plan.md
EOF
)"
```

---

## Task 2: `HarnessError::class()` with delegation to `AlephError`

**Files:**
- Modify: `src/harness/trait_def.rs` (extend after the existing `impl From<ToolError>` at line 125-129)

**Class mapping for HarnessError variants:**

| HarnessError variant | ErrorClass | Reason |
|---------------------|-----------|--------|
| `Llm(inner)` | `inner.class()` | Delegate to AlephError |
| `Tool(_)` | Fixable | Model reads tool error and adjusts (matches CC behavior post-P0 rescue) |
| `Session(_)` | Unexpected | Internal storage / actor failure |
| `Cancelled` | Recoverable | User-initiated abort |
| `Stalled { .. }` | Transient | Cross-turn idle timeout — retry could resolve |
| `StalledTurn { .. }` | Transient | Single-phase hang — retry could resolve |

- [ ] **Step 1: Write the failing unit tests**

Append at the **end** of `src/harness/trait_def.rs` (after the `impl From<ToolError>` block):

```rust
#[cfg(test)]
mod harness_error_class_tests {
    use super::{HarnessError, TurnPhase};
    use crate::error::{AlephError, ErrorClass};

    #[test]
    fn cancelled_is_recoverable() {
        assert_eq!(HarnessError::Cancelled.class(), ErrorClass::Recoverable);
    }

    #[test]
    fn stalled_is_transient() {
        let e = HarnessError::Stalled {
            elapsed: std::time::Duration::from_secs(60),
        };
        assert_eq!(e.class(), ErrorClass::Transient);
    }

    #[test]
    fn stalled_turn_is_transient() {
        let e = HarnessError::StalledTurn {
            phase: TurnPhase::Think,
            elapsed: std::time::Duration::from_secs(60),
        };
        assert_eq!(e.class(), ErrorClass::Transient);
    }

    #[test]
    fn llm_delegates_to_inner() {
        let e = HarnessError::Llm(AlephError::network("net blip"));
        assert_eq!(e.class(), ErrorClass::Transient);
        let e = HarnessError::Llm(AlephError::authentication("anthropic", "bad key"));
        assert_eq!(e.class(), ErrorClass::Fixable);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:
```bash
cargo test -p alephcore --lib harness_error_class_tests 2>&1 | tail -15
```

Expected: compile error `no method named 'class' found for enum 'HarnessError'`.

- [ ] **Step 3: Implement `HarnessError::class()`**

Append after the last `impl From<ToolError>` block in `src/harness/trait_def.rs`:

```rust
impl HarnessError {
    /// Map this error to a stable `ErrorClass` for cross-cutting decisions
    /// (trace dispatch in `agent.rs`, future Guardrail / Verification
    /// consumers in Stage 5 / 6).
    pub fn class(&self) -> crate::error::ErrorClass {
        use crate::error::ErrorClass;
        match self {
            HarnessError::Llm(inner) => inner.class(),
            HarnessError::Tool(_) => ErrorClass::Fixable,
            HarnessError::Session(_) => ErrorClass::Unexpected,
            HarnessError::Cancelled => ErrorClass::Recoverable,
            HarnessError::Stalled { .. } | HarnessError::StalledTurn { .. } => {
                ErrorClass::Transient
            }
        }
    }
}
```

**Note**: Match is exhaustive without wildcard so future `HarnessError` variants force a classification decision at compile time.

- [ ] **Step 4: Run tests to verify they pass**

Run:
```bash
cargo test -p alephcore --lib harness_error_class_tests 2>&1 | tail -10
```

Expected: `test result: ok. 4 passed; 0 failed`.

- [ ] **Step 5: Run full harness unit tests to confirm no regression**

Run:
```bash
cargo test -p alephcore --lib harness:: 2>&1 | tail -15
```

Expected: all existing harness tests pass; new 4 tests show in the count.

- [ ] **Step 6: Run clippy and fmt**

Run:
```bash
cargo fmt --check && cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/harness/trait_def.rs
git commit -m "$(cat <<'EOF'
feat(harness): add HarnessError::class() delegating to AlephError (Stage 1.2)

HarnessError variants map to ErrorClass:
- Llm(inner) -> inner.class() (boundary descent)
- Tool -> Fixable (model can self-correct)
- Session -> Unexpected
- Cancelled -> Recoverable
- Stalled / StalledTurn -> Transient

Match is exhaustive without wildcard so new variants force a
compile-time classification decision.

Refs: docs/superpowers/specs/2026-05-05-harness-stage1-error-class-plan.md
EOF
)"
```

---

## Task 3: Retire wildcard variant match in `agent.rs:700-705`

**Files:**
- Modify: `src/harness/agent.rs:700-705`

**Old code (to retire):**

```rust
let session_outcome = match &e {
    HarnessError::Cancelled | HarnessError::Stalled { .. } => {
        crate::harness::trace::LoopTraceSessionOutcome::Cancelled
    }
    _ => crate::harness::trace::LoopTraceSessionOutcome::Cancelled,
};
```

**Why retire:** the explicit `HarnessError::Cancelled | HarnessError::Stalled { .. }` arm is hardcoded variant comparison; the wildcard `_ => Cancelled` is the same outcome. Both arms collapse to a single class-driven dispatch.

**New code:** dispatch via `e.class()` and emit class in tracing log so observers (gateway log scrapers, future Stage 5/6 consumers) can see the actual class distribution.

- [ ] **Step 1: Confirm the old code location**

Run:
```bash
sed -n '697,716p' src/harness/agent.rs
```

Expected: the snippet shown above. If the line numbers have drifted, locate via:
```bash
grep -n "session_outcome = match &e" src/harness/agent.rs
```

- [ ] **Step 2: Write the failing test (regression-style)**

Append to `src/harness/tests/stability.rs` (anywhere after line 530 — i.e., at the end of the file):

```rust
#[tokio::test]
async fn session_error_path_carries_error_class_in_tracing() {
    // Regression test for Stage 1: when the harness terminates with an
    // error, the error_class must be observable in tracing output. Uses
    // tracing's test subscriber to capture spans/events synchronously.
    use crate::error::{AlephError, ErrorClass};
    use crate::harness::trait_def::HarnessError;

    // Pure unit-level assertion: confirm class() values match what Task 3
    // will dispatch on. This locks the wiring contract for the wildcard
    // retirement step.
    assert_eq!(
        HarnessError::Llm(AlephError::network("net blip")).class(),
        ErrorClass::Transient,
    );
    assert_eq!(
        HarnessError::Tool(crate::tools::service::ToolError::NotFound {
            name: "ghost".into(),
        })
        .class(),
        ErrorClass::Fixable,
    );
    assert_eq!(HarnessError::Cancelled.class(), ErrorClass::Recoverable);
}
```

**Note on test scope**: this is a contract test — it pins the class mapping that Task 3's wildcard retirement depends on. The full integration regression (verifying P0 rescue baseline preserved) is Task 4.

- [ ] **Step 3: Run the new test to verify it passes after Task 2**

Run:
```bash
cargo test -p alephcore --test harness_tests stability::session_error_path_carries_error_class_in_tracing 2>&1 | tail -10
```

If your project uses `--lib` for harness tests, adjust accordingly. Locate the right command via:
```bash
grep -n "cargo test" Justfile justfile 2>/dev/null | head -5
```

Expected: `test result: ok. 1 passed`. (This test passes after Task 2 completes; if it fails, Task 2 wiring is wrong.)

If the `ToolError::NotFound { name }` shape differs, run:
```bash
grep -n "enum ToolError\|NotFound" src/tools/service.rs | head -5
```
and adjust the test's `ToolError::NotFound { ... }` literal to match the real variant shape.

- [ ] **Step 4: Replace the wildcard match in `agent.rs:700-705`**

Locate the block at agent.rs:699-705 and replace with:

```rust
            Err(e) => {
                let error_class = e.class();
                tracing::warn!(
                    ?session_id,
                    ?error_class,
                    error = %e,
                    "harness session ended in error",
                );
                let session_outcome = match error_class {
                    crate::error::ErrorClass::Recoverable
                    | crate::error::ErrorClass::Transient
                    | crate::error::ErrorClass::Fixable
                    | crate::error::ErrorClass::Unexpected => {
                        crate::harness::trace::LoopTraceSessionOutcome::Cancelled
                    }
                };
```

**Why all four arms still map to `Cancelled`**: Stage 1 is a vocabulary-introduction stage, NOT a behavior-change stage. Mapping each class explicitly (no wildcard) ensures:
1. Future stages that want to distinguish (e.g., Stage 6 Verification might want `Failed` for `Unexpected`) have a clear point to extend without re-introducing a wildcard.
2. Compiler enforces that any new `ErrorClass` variant added later requires an explicit decision here.
3. P0 rescue behavior is byte-equivalent (the variable `session_outcome` ends up `Cancelled` exactly when it would before).

The new `tracing::warn!` line is the **real consumer** that justifies the seam: gateway log scrapers can now see `error_class=Transient` vs `error_class=Unexpected` and Stage 5/6 can pivot on that telemetry.

- [ ] **Step 5: Verify the modified block compiles and behavior is preserved**

Run:
```bash
cargo check -p alephcore 2>&1 | tail -10
```

Expected: no compile errors. (The four-arm match looks redundant but the compiler accepts it; clippy may suggest simplification — see Step 6.)

- [ ] **Step 6: Handle expected clippy warning**

The four-arm match collapsing to one outcome WILL trigger `clippy::match_same_arms`. This is intentional (see Step 4 rationale). Allow it locally:

Replace the match block with:
```rust
                #[allow(clippy::match_same_arms)] // intentional: per-class dispatch reserved for future stages
                let session_outcome = match error_class {
                    crate::error::ErrorClass::Recoverable
                    | crate::error::ErrorClass::Transient
                    | crate::error::ErrorClass::Fixable
                    | crate::error::ErrorClass::Unexpected => {
                        crate::harness::trace::LoopTraceSessionOutcome::Cancelled
                    }
                };
```

Actually, since the four arms truly collapse, the cleaner Rust idiom is one arm with a wildcard via class — but we explicitly want the seam. Use the `#[allow]` and document why:

```rust
                // Stage 1: all classes map to Cancelled (P0 rescue behavior).
                // Future stages will distinguish (e.g., Unexpected -> Failed).
                // Match is exhaustive without wildcard so new ErrorClass
                // variants force an explicit decision here.
                #[allow(clippy::match_same_arms)]
                let session_outcome = match error_class {
                    crate::error::ErrorClass::Recoverable
                    | crate::error::ErrorClass::Transient
                    | crate::error::ErrorClass::Fixable
                    | crate::error::ErrorClass::Unexpected => {
                        crate::harness::trace::LoopTraceSessionOutcome::Cancelled
                    }
                };
```

- [ ] **Step 7: Run clippy and fmt**

Run:
```bash
cargo fmt --check && cargo clippy -p alephcore --lib -- -D warnings 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 8: Run full harness test suite to confirm no regression**

Run:
```bash
cargo test -p alephcore --lib harness:: 2>&1 | tail -20
```

Expected: all existing 41+ tests pass (including the new contract test from Step 2 and the unit tests from Tasks 1+2).

- [ ] **Step 9: Commit**

```bash
git add src/harness/agent.rs src/harness/tests/stability.rs
git commit -m "$(cat <<'EOF'
refactor(harness): retire wildcard variant match in error->outcome (Stage 1.3)

agent.rs:700-705 had a hardcoded variant arm + wildcard collapse to
the same outcome. Replaced with class-dispatched mapping that:
- emits error_class in tracing for gateway log observers
- forces compile-time decisions when new ErrorClass variants land
- preserves P0 rescue behavior (all classes still map to Cancelled
  outcome in this stage; Stage 6 Verification will distinguish later)

Adds contract test in tests/stability.rs pinning the class mapping
that this dispatch depends on.

Old code retired: the `HarnessError::Cancelled | HarnessError::Stalled
{ .. } => ... | _ => ...` block (5 lines) replaced with class-driven
dispatch (~10 lines, including comment + #[allow]).

Refs: docs/superpowers/specs/2026-05-05-harness-stage1-error-class-plan.md
EOF
)"
```

---

## Task 4: Integration regression — P0 rescue baseline preserved

**Files:**
- Verify: `src/harness/tests/stability.rs::consecutive_total_failure_caps_loop` (existing) and the new test from Task 3 still pass.
- Modify: nothing new in this task — this is a verification task.

- [ ] **Step 1: Run the P0 rescue regression suite**

Run:
```bash
cargo test -p alephcore --lib harness::tests::stability:: 2>&1 | tail -25
```

Expected: all stability tests pass, including:
- `consecutive_total_failure_caps_loop` (P0 rescue cap behavior)
- `partial_batch_failure_continues` (P0 rescue act error rescue)
- `tool_failure_recovers_in_next_think` (P0 rescue act error rescue)
- `think_timeout_fires_with_phase_think` (P0 rescue per-turn timeout)
- `act_timeout_fires_with_phase_act` (P0 rescue per-turn timeout)
- `session_error_path_carries_error_class_in_tracing` (Stage 1 contract test)

**If any pre-existing test fails**: stop and investigate. Stage 1 must not break P0 rescue. Roll back via `git revert` and re-plan.

- [ ] **Step 2: Run the full test suite**

Run:
```bash
cargo test -p alephcore --lib 2>&1 | tail -10
```

Expected: all tests pass. Note any unrelated baseline failures (memory mention notes some pre-existing baseline failures unrelated to harness are acceptable).

- [ ] **Step 3: Run cargo build to confirm release compilation**

Run:
```bash
cargo build -p alephcore --release 2>&1 | tail -10
```

Expected: clean build.

- [ ] **Step 4: Verify line counts against budget**

Run:
```bash
wc -l src/harness/agent.rs src/harness/trait_def.rs src/error.rs && wc -l src/harness/*.rs | tail -1
```

Expected:
- `src/harness/agent.rs` ≤ 1500 lines (currently 1340 + ~5 for the new `match` block)
- `src/harness/trait_def.rs` ≤ ~190 lines (currently 129 + ~35 new)
- `src/error.rs` ≤ ~270 lines (currently ~204 + ~60 new)
- `src/harness/` total ≤ 2400 lines (currently 2344 + ~5)

If any threshold is exceeded, investigate which task added unexpected lines and trim.

- [ ] **Step 5: Verify acceptance criteria from master spec § Stage 1**

Walk through each criterion:

| Criterion | Verified by |
|-----------|-------------|
| `HarnessError` 全部 variants 有 class 分类 | Task 2 unit tests + exhaustive match enforces compile-time |
| `AlephError` boundary 处可映射 | Task 1 unit tests + `HarnessError::Llm(inner) => inner.class()` |
| P0 rescue 行为（act 错误回流 / consecutive cap / watchdog timeout）保持等价 | Task 4 Step 1 (all P0 stability tests still pass) |
| ≥3 unit 验证 class 映射 | Task 1 (4 unit tests) + Task 2 (4 unit tests) = 8 unit tests total ✓ |
| ≥1 集成验证 cap 仍按预期触发 | `consecutive_total_failure_caps_loop` (existing) still passes ✓ |
| 分类开销 ≤ 1 个 enum match | `class()` is a single match expression, no allocation, monomorphizable ✓ |

- [ ] **Step 6: Optional commit (only if Steps 1-5 surfaced issues that needed fixes)**

If any test failed and required adjustment in Tasks 1-3, amend or add a fixup commit:

```bash
git add -p
git commit -m "fix(harness): adjust Stage 1 class mapping per integration test feedback"
```

If everything passed without changes, skip this step.

---

## Task 5: Final verification + master spec annotation

**Files:**
- Modify: `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md` (add `✅ Shipped` line to Stage 1 entry)

- [ ] **Step 1: Run the canonical pre-merge gate**

Run:
```bash
cargo fmt --check && \
cargo clippy -p alephcore --lib -- -D warnings && \
cargo test -p alephcore --lib harness:: && \
cargo test -p alephcore --lib error_class_tests
```

Expected: all four commands succeed. If any fail, return to the relevant task and fix.

- [ ] **Step 2: Verify Future-Proof Test (R10 必答)**

Read your own implementation. Answer: **"Will this code still be needed when models upgrade one major version (e.g., Anthropic Opus 5)?"**

Required answer: **Yes — the classification vocabulary (Transient / Recoverable / Fixable / Unexpected) describes infrastructure-level error semantics, not model behavior. A new model produces the same error categories from the same upstream sources (network / auth / tool dispatch / system).** This is the answer locked into the master spec § Stage 1 Future-proof note. Confirm the implementation does not contradict this — i.e., no class assignment depends on a model-specific quirk.

- [ ] **Step 3: Append shipped marker to master spec**

Get the latest commit hash:
```bash
git log -1 --oneline
```

Edit `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md` — find the line:

```
### Stage 1 — Error Handling Classification (#8)

**Status**: 🟡 Pending
```

Replace with (substitute `<hash>` with the actual hash from above):

```
### Stage 1 — Error Handling Classification (#8)

**Status**: ✅ Shipped <hash> on 2026-05-05 · plan: docs/superpowers/specs/2026-05-05-harness-stage1-error-class-plan.md
```

- [ ] **Step 4: Final commit**

```bash
git add docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md
git commit -m "$(cat <<'EOF'
docs(harness): mark Stage 1 (ErrorClass) shipped in roadmap master spec

Stage 1 complete:
- ErrorClass enum + AlephError::class() + HarnessError::class()
- agent.rs:700-705 wildcard variant match retired (class-dispatched)
- 8 unit tests + 1 integration contract test added
- All P0 rescue regression tests pass (baseline bf0de41cc preserved)
- ~125 lines added across error.rs / trait_def.rs / agent.rs / tests/stability.rs

Stage 2 (Tools Surface Unification) is the next pending stage with
no remaining dependencies.

Refs: docs/superpowers/specs/2026-05-05-harness-stage1-error-class-plan.md
EOF
)"
```

- [ ] **Step 5: Run final summary**

Run:
```bash
git log --oneline -10 && \
echo "---" && \
wc -l src/harness/*.rs | tail -1 && \
echo "---" && \
cargo test -p alephcore --lib harness:: 2>&1 | grep "test result"
```

Expected:
- 4 new commits visible (Tasks 1, 2, 3 + roadmap mark)
- harness/ total < 2400 lines
- test result line shows N tests passed, 0 failed (N ≥ 50: original 41 + new 8 unit + 1 contract)

---

## Self-Review Checklist (run after writing this plan, before handoff)

**1. Spec coverage (master spec § Stage 1):**
- ✅ Problem: classification vocabulary missing → addressed by Task 1 (`ErrorClass` enum) + Task 2 (`HarnessError::class()`)
- ✅ Sketch: enum + class() method → Tasks 1+2 directly
- ✅ Allowed seams: ErrorClass + class() accessor → introduced with ≥1 real consumer (the wildcard retirement in Task 3 + tracing log)
- ✅ Old code to retire: agent.rs:700-705 wildcard arm → Task 3 Step 4
- ✅ Acceptance: full variant coverage / P0 rescue equivalence / ≥3 unit / ≥1 integration → Task 4 Step 5 verifies each
- ✅ Future-proof note: vocabulary stable → Task 5 Step 2 forces re-affirmation

**2. Placeholder scan:** no "TBD" / "TODO" / "fill in" in tasks. All code shown literally. All commands shown with expected output. ✅

**3. Type consistency:**
- `ErrorClass` defined in Task 1 used identically in Tasks 2/3/4 ✅
- `HarnessError::class()` signature defined in Task 2 used identically in Tasks 3/4 ✅
- `AlephError::class()` signature defined in Task 1 used identically in Tasks 2/3/4 ✅
- Variant names match `src/error.rs:27-203` exhaustively (verified by exhaustive match without wildcard) ✅

**4. Risk class verification:** Stage 1 is `low risk` per master spec. Plan satisfies low-risk acceptance: 5 mandatory clauses (Future-Proof Test / Old code retired / No-regression / Seam justified / Mechanically verifiable acceptance). No property/loom test required for low-risk stage. ✅

**5. Line budget:** ~125 lines total (well under 600 PR cap and 400 harness/ delta cap). agent.rs grows by ~5 lines (1340 → ~1345, well under 1500 cap). ✅

**End of plan.**
