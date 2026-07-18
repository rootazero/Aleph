# Failover/Override Provider Behavior-Protocol Pass-Through — Design

**Date:** 2026-06-20
**Status:** Approved (design)
**Scope:** Single concern — make `AiProvider` behavior-resolution methods see through live-primary wrappers (`FailoverProvider`, `ModelOverrideProvider`).

## Problem

`AiProvider`'s behavior-resolution accessors return **borrowed** strings:

```rust
fn protocol(&self) -> &str { "unknown" }                  // providers/mod.rs:254
fn model_behavior_override(&self) -> Option<&str> { None } // providers/mod.rs:263
```

`FailoverProvider` (providers/failover/provider.rs:769-781) implements only
`name`/`color`/`supports_native_tools`. It can delegate `supports_native_tools`
to the live primary —

```rust
fn supports_native_tools(&self) -> bool {
    self.primary.current().supports_native_tools()   // returns bool (Copy) — fine
}
```

— but it **cannot** delegate `protocol()` / `model_behavior_override()`, because
`self.primary.current()` returns a **temporary `Arc<dyn AiProvider>`** (read
per-call to honor `set_default` hot-reload), and a `&str` cannot be returned
borrowing into a temporary. So those two methods fall through to the trait
defaults `"unknown"` / `None`.

The 777-line comment states the intent: *"The wrapper should look like its live
primary for behavior-resolution."* The borrowed return type is what blocked it.

### Consequences (all current consumers get the wrong value through failover)

Three real consumers resolve model behavior as
`model_behavior_override().or_else(|| protocol_to_behavior(protocol()))`:

1. `orchestrator/harness_bridge/runner_impl.rs:250` — `ModelRobustnessProfile::for_behavior(..)` (the per-model loop-watchdog tuning; the reported kimi-via-failover symptom).
2. `gateway/execution_engine/run_loop/inner.rs:538` — `load_model_behavior(..)`.
3. `orchestrator/harness_bridge/prompt_build.rs:440` — the system prompt's per-family operational directives (`with_provider_protocol`).

Through `FailoverProvider`, all three read `"unknown"` → robustness profile
collapses to `conservative()`, and the prompt advertises the wrong family. The
fan-out distinctness fix (harness-multimodel-robustness) is independent of the
profile, so it still works — but per-model tuning never reaches failover runs.

### Secondary gap

`ModelOverrideProvider` (providers/model_override_provider.rs) delegates
`protocol()` to its inner but **omits `model_behavior_override()`** → it falls to
the default `None`, dropping the inner provider's override. Sub-agent model pins
wrap with `ModelOverrideProvider`, so sub-agents lose the override too.

## Approach (chosen: change trait returns to owned)

Change the two accessors to owned-capable returns:

```rust
fn protocol(&self) -> Cow<'_, str> { Cow::Borrowed("unknown") }
fn model_behavior_override(&self) -> Option<Cow<'_, str>> { None }
```

- Static impls return `Cow::Borrowed("…")` — **zero allocation on the common path**.
- `FailoverProvider` returns `Cow::Owned(self.primary.current().protocol().into_owned())`, drawing from the live primary without a borrow-into-temporary.
- Changing the return type makes the compiler **enumerate every call site** — no consumer can silently retain the old `"unknown"` behavior, and the borrowed-`protocol()` footgun (a future consumer silently getting `"unknown"` through failover) is **permanently removed** (there is one canonical method, and it is failover-transparent).

### Rejected alternatives

- **Additive owned sibling** (`behavior_protocol() -> String` next to the borrowed `protocol()`): smaller diff, but leaves borrowed `protocol()` returning `"unknown"` through failover — a latent footgun and two ways to read one fact. Rejected for leaving the trap armed.
- **Cache a primary `Arc` field in `FailoverProvider` and borrow from it:** defeats the per-call `current()` hot-reload contract (`set_default` swaps would go stale). Rejected.

## Change Inventory (all mechanical, no new dependencies)

| File | Change |
|---|---|
| `providers/mod.rs` | Trait default `protocol` → `Cow<'_, str>` (`Cow::Borrowed("unknown")`); `model_behavior_override` → `Option<Cow<'_, str>>` (`None`). `use std::borrow::Cow;`. |
| `providers/http_provider.rs` | `protocol` → `Cow::Borrowed(self.adapter.name())`; `model_behavior_override` → `self.config.model_behavior.as_deref().map(Cow::Borrowed)`. |
| `providers/ollama.rs` | `protocol` → `Cow::Borrowed("ollama")`. |
| `providers/metering.rs` | Both return types follow inner; bodies unchanged (`self.inner.protocol()` / `self.inner.model_behavior_override()` already return the new types). |
| `providers/model_override_provider.rs` | `protocol` return type follows inner (body unchanged); **add** `model_behavior_override` delegating to `self.inner` (Secondary gap fix). |
| `providers/failover/provider.rs` | **Add** `protocol` and `model_behavior_override`, each `self.primary.current().<m>()` with `.into_owned()` → `Cow::Owned` (mirrors existing `supports_native_tools`). |

### Consumer sites (logic unchanged; adapt to owned return)

`protocol_to_behavior(protocol: &str) -> Option<&'static str>` and
`ModelRobustnessProfile::for_behavior(name: Option<&str>) -> Self` keep their
signatures — their results are `'static` / owned and do not borrow the input.
Each consumer binds the `Cow` (and `Option<Cow>`) to a local first, then passes
`&str` slices:

- `runner_impl.rs:250` — bind `let proto = llm.protocol();` and `let ovr = llm.model_behavior_override();`, then `for_behavior(ovr.as_deref().or_else(|| protocol_to_behavior(&proto))).clamped()`.
- `run_loop/inner.rs:538` — same local-binding shape; `behavior_name: Option<&str>` borrows the locals across the `load_model_behavior(..).await` (locals are owned `Cow`/`Option<Cow>`, `Send`); `protocol = %proto` uses `Cow`'s `Display`.
- `prompt_build.rs:440` — `.map_or_else(|| provider.protocol().into_owned(), |s| s.into_owned())` (or `.to_string()`), still yielding `String` for `with_provider_protocol`.

## Testing (TDD)

New regression unit tests (these fail before the fix):

1. **Failover transparency** (`providers/failover/tests.rs`): a `FailoverProvider` whose primary reports `protocol() == "anthropic"` and `model_behavior_override() == Some("kimi")` → assert the failover wrapper returns the same. (Before: `"unknown"` / `None`.)
2. **Override pass-through** (`providers/model_override_provider.rs` tests): inner with `model_behavior_override() == Some("openrouter-anthropic")` → assert the wrapped provider returns `Some(...)`. (Before: `None`.)

Adjust the existing `ollama.rs` assertion `assert_eq!(provider.protocol(), "ollama")` to compare against the `Cow` (`.as_ref()` or `Cow::Borrowed("ollama")`).

`robustness_profile.rs` tests are unaffected (`for_behavior(Option<&str>)` unchanged).

## Out of Scope (deliberate, P6)

- `as_http_provider()` through `FailoverProvider` / `ModelOverrideProvider` stays `None` — unrelated to behavior resolution; failover has no single streaming handle.
- `for_behavior` / `protocol_to_behavior` / `ModelRobustnessProfile` internals — the fan-out distinctness fix is orthogonal and already in effect.

## Estimated blast radius

6 files changed + 3 consumer sites + tests, ~+50/−15 lines, no new dependencies.
