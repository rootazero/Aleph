# Failover/Override Provider Behavior-Protocol Pass-Through Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `FailoverProvider` and `ModelOverrideProvider` report their live primary/inner provider's `protocol()` and `model_behavior_override()`, so per-model robustness profiles and prompt directives resolve correctly for failover- and pin-routed models.

**Architecture:** Change the two `AiProvider` behavior-resolution accessors from borrowed returns (`&str` / `Option<&str>`) to owned-capable returns (`Cow<'_, str>` / `Option<Cow<'_, str>>`). Static providers return `Cow::Borrowed` (zero allocation); wrappers return `Cow::Owned` drawn from the live primary/inner — which a borrowed return could not do, because `FailoverProvider::primary.current()` yields a temporary `Arc`. Task 1 lands the type-only refactor (behavior-preserving); Task 2 adds the wrapper delegation that fixes the bug, test-first.

**Tech Stack:** Rust, `std::borrow::Cow`, existing `alephcore` provider traits. No new dependencies.

## Global Constraints

- No new dependencies (R3). Only `std::borrow::Cow` (std).
- Static-provider `protocol()` impls MUST return `Cow::Borrowed(...)` — zero allocation on the common path.
- `protocol_to_behavior(protocol: &str) -> Option<&'static str>` and `ModelRobustnessProfile::for_behavior(name: Option<&str>) -> Self` signatures MUST NOT change — their outputs are owned/`'static` and do not borrow the input.
- `as_http_provider()` is OUT OF SCOPE — do not add or change it on any wrapper.
- `for_behavior` / `protocol_to_behavior` / `ModelRobustnessProfile` internals are OUT OF SCOPE — do not touch them.
- Code comments in English. Surgical changes only — every changed line traces to this fix.
- cargo restraint: scope every test run to the named filters below. Never run `just test-all`, `--workspace`, or the full lib suite.
- In a fresh worktree the Bash shell is non-interactive and does not source `.zshrc`. Prefix every cargo command with:
  `export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH"`

---

### Task 1: Owned-return trait refactor (type-only, behavior-preserving)

Change the trait signatures and every concrete impl + every consumer so the
codebase compiles and all existing tests pass. NO wrapper delegation is added
here — `FailoverProvider` keeps returning the trait defaults (`"unknown"` /
`None`) exactly as it does today, and `ModelOverrideProvider` keeps returning
`None` for the override exactly as today. This task is a pure type refactor;
its correctness is guarded by the existing provider test suite plus the one
adjusted `ollama` assertion. Behavior is byte-identical to before.

**Files:**
- Modify: `src/providers/mod.rs` — trait defaults `protocol` (~line 254) and `model_behavior_override` (~line 263); add `use std::borrow::Cow;`.
- Modify: `src/providers/http_provider.rs` — `protocol` (~626) and `model_behavior_override` (~630); add `use std::borrow::Cow;`.
- Modify: `src/providers/ollama.rs` — `protocol` (~519) and its test assertion (~612); add `use std::borrow::Cow;`.
- Modify: `src/providers/metering.rs` — `protocol` (~97) and `model_behavior_override` (~101); add `use std::borrow::Cow;`.
- Modify: `src/providers/model_override_provider.rs` — `protocol` (~62); add `use std::borrow::Cow;`. (Do NOT add `model_behavior_override` here — that is Task 2.)
- Modify: `src/orchestrator/harness_bridge/runner_impl.rs` — behavior resolution (~250-255).
- Modify: `src/gateway/execution_engine/run_loop/inner.rs` — behavior resolution block (~533-552).
- Modify: `src/orchestrator/harness_bridge/prompt_build.rs` — `provider_protocol` (~440-443).

**Interfaces:**
- Produces:
  - `AiProvider::protocol(&self) -> std::borrow::Cow<'_, str>` (default `Cow::Borrowed("unknown")`)
  - `AiProvider::model_behavior_override(&self) -> Option<std::borrow::Cow<'_, str>>` (default `None`)
- Consumes: `crate::providers::model_behaviors::protocol_to_behavior(&str) -> Option<&'static str>`, `crate::verification::ModelRobustnessProfile::for_behavior(Option<&str>) -> Self` — unchanged.

- [ ] **Step 1: Change the trait default signatures**

In `src/providers/mod.rs`, add the import near the other `use` lines (after line 55):

```rust
use std::borrow::Cow;
```

Replace the two default methods (currently returning `&str` / `Option<&str>`):

```rust
    /// Protocol name for model behavior resolution.
    ///
    /// Returns the protocol identifier (e.g., "openai", "anthropic", "gemini", "ollama")
    /// used to select appropriate model behavior directives.
    fn protocol(&self) -> Cow<'_, str> {
        Cow::Borrowed("unknown")
    }

    /// Model behavior override from provider config.
    ///
    /// When set, this takes precedence over the protocol-based auto-mapping.
    /// Used for providers like `OpenRouter` that use one protocol but route to
    /// a different model family.
    fn model_behavior_override(&self) -> Option<Cow<'_, str>> {
        None
    }
```

- [ ] **Step 2: Update `http_provider.rs` impl**

Add `use std::borrow::Cow;` near its other imports. Replace the two methods (currently `protocol` returns `self.adapter.name()` and `model_behavior_override` returns `self.config.model_behavior.as_deref()`):

```rust
    fn protocol(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.adapter.name())
    }

    fn model_behavior_override(&self) -> Option<Cow<'_, str>> {
        self.config.model_behavior.as_deref().map(Cow::Borrowed)
    }
```

- [ ] **Step 3: Update `ollama.rs` impl and its test assertion**

Add `use std::borrow::Cow;` near its other imports. Replace `protocol`:

```rust
    fn protocol(&self) -> Cow<'_, str> {
        Cow::Borrowed("ollama")
    }
```

Change the existing test assertion at ~line 612 from
`assert_eq!(provider.protocol(), "ollama");` to:

```rust
        assert_eq!(provider.protocol().as_ref(), "ollama");
```

- [ ] **Step 4: Update `metering.rs` impl (delegation bodies unchanged)**

Add `use std::borrow::Cow;` near its other imports. Change only the return types — the bodies already delegate to `self.inner`:

```rust
    fn protocol(&self) -> Cow<'_, str> {
        self.inner.protocol()
    }

    fn model_behavior_override(&self) -> Option<Cow<'_, str>> {
        self.inner.model_behavior_override()
    }
```

- [ ] **Step 5: Update `model_override_provider.rs` `protocol` only**

Add `use std::borrow::Cow;` near its other imports. Change the `protocol` return type (body unchanged):

```rust
    fn protocol(&self) -> Cow<'_, str> {
        self.inner.protocol()
    }
```

Do NOT add `model_behavior_override` in this task.

- [ ] **Step 6: Update consumer — `runner_impl.rs`**

Replace the behavior-resolution expression (currently
`llm.model_behavior_override().or_else(|| protocol_to_behavior(llm.protocol()))`)
with the fully-inlined owned form (temporaries live for the call; nothing is
bound across the later `llm` move):

```rust
        let robustness_profile = crate::verification::ModelRobustnessProfile::for_behavior(
            llm.model_behavior_override().as_deref().or_else(|| {
                crate::providers::model_behaviors::protocol_to_behavior(&llm.protocol())
            }),
        )
        .clamped();
```

- [ ] **Step 7: Update consumer — `run_loop/inner.rs`**

This site reuses the resolved name across an `.await` and a log, so bind owned
locals. Replace the block body (currently lines ~538-551):

```rust
                let behavior_override = provider.model_behavior_override();
                let protocol = provider.protocol();
                let behavior_name: Option<&str> = behavior_override
                    .as_deref()
                    .or_else(|| protocol_to_behavior(&protocol));
                let content = match behavior_name {
                    Some(name) => load_model_behavior(name).await,
                    None => None,
                };
                info!(
                    run_id = run_id,
                    protocol = %protocol,
                    behavior_name = ?behavior_name,
                    loaded = content.is_some(),
                    "Model behavior resolved"
                );
```

- [ ] **Step 8: Update consumer — `prompt_build.rs`**

Replace the `provider_protocol` resolution (currently uses `.to_string()` on
`&str`) with `.into_owned()` on the owned returns:

```rust
        let provider_protocol = provider
            .model_behavior_override()
            .map_or_else(|| provider.protocol().into_owned(), |s| s.into_owned());
        builder = builder.with_provider_protocol(provider_protocol);
```

- [ ] **Step 9: Compile + run the touched provider tests**

Run:
```bash
export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH"
cargo test -p alephcore --lib providers::
```
Expected: PASS. The whole `alephcore` lib compiles (so any consumer type error
in `runner_impl` / `run_loop/inner` / `prompt_build` surfaces here), and all
existing provider tests — including the adjusted `ollama` assertion — pass.
Behavior is unchanged: `FailoverProvider::protocol()` still returns `"unknown"`,
`ModelOverrideProvider::model_behavior_override()` still returns `None`.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "providers: return owned Cow from protocol()/model_behavior_override()"
```

---

### Task 2: Wrapper transparency (the bug fix, test-first)

Add the pass-through delegation that makes `FailoverProvider` report its live
primary's protocol/override and `ModelOverrideProvider` report its inner's
override. Written test-first: the regression tests fail against Task 1's code
(wrappers still return defaults), then pass once delegation is added.

**Files:**
- Modify: `src/providers/failover/provider.rs` — add `protocol` + `model_behavior_override` to the `impl AiProvider for FailoverProvider` block (after `supports_native_tools`, ~line 781); add `use std::borrow::Cow;`.
- Modify: `src/providers/failover/tests.rs` — add a configurable-behavior primary mock + a transparency test.
- Modify: `src/providers/model_override_provider.rs` — add `model_behavior_override` delegation to the impl; add a pass-through test.

**Interfaces:**
- Consumes (from Task 1): `AiProvider::protocol(&self) -> Cow<'_, str>`, `AiProvider::model_behavior_override(&self) -> Option<Cow<'_, str>>`; `FailoverProvider.primary: Arc<dyn DefaultProviderHandle>` with `.current() -> Arc<dyn AiProvider>`; test helpers `build(primary: Arc<dyn AiProvider>, catalog: Vec<(&str, Vec<&str>)>, fallbacks: Vec<FailoverNode>) -> FailoverProvider` and `StaticDefault`.
- Produces: failover-/pin-transparent behavior resolution for all three consumers.

- [ ] **Step 1: Write the failing failover transparency test**

In `src/providers/failover/tests.rs`, add a configurable primary mock and a
test. Place near the other test providers:

```rust
/// Primary whose behavior-resolution fields are configurable, so the
/// failover wrapper's pass-through can be asserted.
struct BehaviorProvider {
    protocol: &'static str,
    behavior: Option<&'static str>,
}

impl AiProvider for BehaviorProvider {
    fn process<'a>(
        &'a self,
        _payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        Box::pin(async { Ok(ProviderResponse::text_only("primary".to_string())) })
    }
    fn name(&self) -> &str {
        "primary"
    }
    fn color(&self) -> &str {
        "#000"
    }
    fn protocol(&self) -> std::borrow::Cow<'_, str> {
        std::borrow::Cow::Borrowed(self.protocol)
    }
    fn model_behavior_override(&self) -> Option<std::borrow::Cow<'_, str>> {
        self.behavior.map(std::borrow::Cow::Borrowed)
    }
}

#[test]
fn failover_reports_live_primary_behavior() {
    let primary = Arc::new(BehaviorProvider {
        protocol: "anthropic",
        behavior: Some("kimi"),
    });
    let failover = build(primary, vec![], vec![]);
    assert_eq!(failover.protocol().as_ref(), "anthropic");
    assert_eq!(failover.model_behavior_override().as_deref(), Some("kimi"));
}
```

- [ ] **Step 2: Write the failing override pass-through test**

In `src/providers/model_override_provider.rs`, inside the existing
`#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn delegates_behavior_override_to_inner() {
        struct OverrideInner;
        impl AiProvider for OverrideInner {
            fn process<'a>(
                &'a self,
                _payload: RequestPayload<'a>,
            ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
                Box::pin(async { Ok(ProviderResponse::text_only("inner".to_string())) })
            }
            fn name(&self) -> &str {
                "inner"
            }
            fn color(&self) -> &str {
                "#000"
            }
            fn model_behavior_override(&self) -> Option<std::borrow::Cow<'_, str>> {
                Some(std::borrow::Cow::Borrowed("openrouter-anthropic"))
            }
        }
        let wrapped = ModelOverrideProvider::new(Arc::new(OverrideInner), "m");
        assert_eq!(
            wrapped.model_behavior_override().as_deref(),
            Some("openrouter-anthropic")
        );
    }
```

- [ ] **Step 3: Run both tests to verify they fail**

Run:
```bash
export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH"
cargo test -p alephcore --lib failover_reports_live_primary_behavior delegates_behavior_override_to_inner
```
Expected: BOTH FAIL. `failover_reports_live_primary_behavior` gets `"unknown"`
(not `"anthropic"`) and `None` (not `Some("kimi")`);
`delegates_behavior_override_to_inner` gets `None` (not `Some("openrouter-anthropic")`).

- [ ] **Step 4: Add `FailoverProvider` delegation**

In `src/providers/failover/provider.rs`, add `use std::borrow::Cow;` near its
other imports. In the `impl AiProvider for FailoverProvider` block, immediately
after the `supports_native_tools` method (~line 780), add:

```rust
    // Behavior-resolution must reflect the live primary, like
    // `supports_native_tools` above. `current()` yields a temporary `Arc`,
    // so the value is copied out (`Cow::Owned`) rather than borrowed.
    fn protocol(&self) -> Cow<'_, str> {
        Cow::Owned(self.primary.current().protocol().into_owned())
    }

    fn model_behavior_override(&self) -> Option<Cow<'_, str>> {
        self.primary
            .current()
            .model_behavior_override()
            .map(|c| Cow::Owned(c.into_owned()))
    }
```

- [ ] **Step 5: Add `ModelOverrideProvider` override delegation**

In `src/providers/model_override_provider.rs`, in the
`impl AiProvider for ModelOverrideProvider` block, after the existing
`protocol` method, add:

```rust
    fn model_behavior_override(&self) -> Option<std::borrow::Cow<'_, str>> {
        self.inner.model_behavior_override()
    }
```

(`use std::borrow::Cow;` was added to this file in Task 1; the fully-qualified
path here is also fine.)

- [ ] **Step 6: Run both tests to verify they pass**

Run:
```bash
export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$PATH"
cargo test -p alephcore --lib failover_reports_live_primary_behavior delegates_behavior_override_to_inner
```
Expected: BOTH PASS.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "providers: failover/override wrappers pass through live behavior protocol"
```

---

## Notes for the implementer

- The `build(...)` helper and `StaticDefault` already exist in `failover/tests.rs` and `providers/default_handle.rs` — reuse them; do not invent new constructors.
- `&Cow<str>` coerces to `&str` at call sites (deref coercion), so `protocol_to_behavior(&llm.protocol())` and `protocol_to_behavior(&protocol)` compile without `.as_ref()`.
- `Cow<'_, str>` implements `Display`, so `protocol = %protocol` in `tracing` macros works.
- Do not change `as_http_provider`, `for_behavior`, `protocol_to_behavior`, or any `ModelRobustnessProfile` internals.
- If a `use std::borrow::Cow;` already exists in a file, do not duplicate it.
