# Subagent Uplift P1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Repay subagent infrastructure debt — wire 5 HarnessDeps fields (Stage A), close the recursion-guard hole (Stage B), integrate LaneScheduler with subagent spawning (Stage C), and verify cancellation propagation end-to-end (Stage D).

**Architecture:** Single PR / 4 atomic commits A→B→C→D / ~625 lines (incl. ~350 test lines). Zero changes to `src/harness/` (R10 thin-harness redline). Stage A creates a new `src/orchestrator/` module to share Phase-6 builders between main runner and subagent spawner. Stage B closes the recursion guard at `AgentDef::is_tool_allowed`. Stage C adds a `LaneScheduler::try_reserve` fail-fast API and wires it into `SpawnerBase`. Stage D ships 3 integration tests for parent→child CancellationToken propagation; fix scope discovered via test failures.

**Tech Stack:** Rust (Aleph core lib + `aleph-server` bin), tokio, tokio-util CancellationToken, async-trait, thiserror, mockall.

**References:**
- Spec: `docs/superpowers/specs/2026-05-08-subagent-uplift-p1-design.md`
- Roadmap: `docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md`
- R10 philosophy: `docs/reference/HARNESS_PHILOSOPHY.md`
- Project redlines: `CLAUDE.md` (R1–R10)

---

## Pre-Flight Checks

- [ ] **PF-1: Verify clean working tree (no uncommitted P1 work)**

```bash
git status --short
```

Expected: Only pre-existing modifications listed in initial gitStatus (Cargo.lock, desktop/*, etc.). No `src/agents/`, `src/scheduler/`, `src/orchestrator/`, `src/bin/aleph-server/commands/start/orchestrator_init.rs`, or `tests/integration/` modifications.

- [ ] **PF-2: Verify baseline tests pass**

```bash
cargo test -p alephcore --lib agents::types
cargo test -p alephcore --lib agents::allowlist_tool_service
cargo test -p alephcore --lib scheduler::lane_scheduler
cargo test -p alephcore --lib scheduler::lane_config
```

Expected: All green. Document any pre-existing red as baseline (don't try to fix in this PR; see `project_baseline_test_failures.md` memory note).

- [ ] **PF-3: Lock R10 baseline**

```bash
wc -l src/harness/*.rs | tail -1
ls src/harness/*.rs | wc -l
```

Expected: ≤ 1500 total lines, exactly 9 files. Record exact numbers in scratch notes; verify unchanged at end of each commit.

- [ ] **PF-4: Lock `subagent_spawner.rs` line baseline**

```bash
wc -l src/agents/subagent_spawner.rs
```

Expected: ~620 lines (anything above 600 is a 0.4 budget signal). Record value.

---

## Stage A — Shared `deps_builder` Module (Commit 1)

**Goal:** Extract Phase-6 builders to `src/orchestrator/deps_builder.rs`; wire 5 HarnessDeps fields (`fallback_llm` / `stall_config` / `consecutive_failure_cap` / `turn_timeout` / `trace_sink`) on subagent path.

### Task A1: Create `src/orchestrator/` module skeleton

**Files:**
- Create: `src/orchestrator/mod.rs`
- Create: `src/orchestrator/deps_builder.rs`
- Modify: `src/lib.rs` (add `pub mod orchestrator;`)

- [ ] **A1.1: Create `src/orchestrator/mod.rs`**

```rust
//! Orchestrator-scope assembly utilities.
//!
//! This module is the single source of HarnessDeps assembly logic shared
//! between the main runner (`aleph-server` bin) and the subagent spawner
//! (`agents::subagent_spawner`). Per the P1 zero-override decision,
//! subagents inherit identical config — no override params accepted.

pub mod deps_builder;

pub use deps_builder::{build_fallback_llm, build_stability_triple, StabilityTriple};
```

- [ ] **A1.2: Create `src/orchestrator/deps_builder.rs` with `StabilityTriple` struct + builder fn skeletons (no impl yet)**

```rust
//! Shared HarnessDeps builder functions.
//!
//! Used by both the main runner (`aleph-server` bin's `orchestrator_init.rs`)
//! and the subagent spawner (`agents::subagent_spawner`) to assemble
//! HarnessDeps fields consistently. Subagents inherit identical config; no
//! override params are accepted (per P1 zero-override decision).

use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;
use crate::harness::StallConfig;
use crate::providers::AiProvider;

/// Stability triple — three independent Optionals derived from `[stability]`.
///
/// Returned as a struct (not tuple) so consumers can name fields and future
/// additions don't break callers.
pub struct StabilityTriple {
    pub stall_config: Option<StallConfig>,
    pub consecutive_failure_cap: Option<usize>,
    pub turn_timeout: Option<Duration>,
}

/// Build the optional Stage 5b single-step fallback provider from
/// `[fallback_provider]`. Returns `None` if:
/// - section missing
/// - `provider` matches `primary_provider_key` ASCII-case-insensitively
/// - `provider` not present in `[providers]` map (warn log)
/// - `create_provider` failure (warn log)
pub fn build_fallback_llm(
    config: &Config,
    primary_provider_key: &str,
) -> Option<Arc<dyn AiProvider>> {
    todo!("moved from src/bin/aleph-server/commands/start/orchestrator_init.rs in A2")
}

/// Build the P0 rescue triple from `[stability]`. Each field is independent.
pub fn build_stability_triple(config: &Config) -> StabilityTriple {
    todo!("moved from src/bin/aleph-server/commands/start/orchestrator_init.rs in A2")
}
```

- [ ] **A1.3: Add `pub mod orchestrator;` to `src/lib.rs`**

Find the existing `pub mod` block in `src/lib.rs` (probably near other top-level modules like `pub mod agents;`). Add `pub mod orchestrator;` in alphabetical order.

```bash
grep -n "^pub mod" src/lib.rs | head -20
```

Expected output: list of `pub mod` declarations. Insert `pub mod orchestrator;` between `pub mod memory;` and `pub mod providers;` (or wherever alphabetical fit is).

- [ ] **A1.4: Verify the skeleton compiles (with `todo!()` panics)**

```bash
cargo check -p alephcore
```

Expected: Compiles with warnings about `unreachable code` after `todo!()`. No errors.

### Task A2: Move builder implementations from `orchestrator_init.rs` to `deps_builder.rs`

**Files:**
- Modify: `src/orchestrator/deps_builder.rs` (replace `todo!()` bodies)
- Modify: `src/bin/aleph-server/commands/start/orchestrator_init.rs` (delete inline impl, keep callers)

- [ ] **A2.1: Replace `build_fallback_llm` `todo!()` with the actual implementation moved from orchestrator_init.rs:190-223**

Open `src/orchestrator/deps_builder.rs` and replace the body of `build_fallback_llm`:

```rust
pub fn build_fallback_llm(
    config: &Config,
    primary_provider_key: &str,
) -> Option<Arc<dyn AiProvider>> {
    let fb = config.fallback_provider.as_ref()?;
    if fb.provider.eq_ignore_ascii_case(primary_provider_key) {
        tracing::warn!(
            provider = %fb.provider,
            "fallback_provider self-reference; disabling"
        );
        return None;
    }
    let pc = match config.providers.get(&fb.provider) {
        Some(c) => c.clone(),
        None => {
            tracing::warn!(
                provider = %fb.provider,
                "fallback_provider not found in [providers]; disabling"
            );
            return None;
        }
    };
    match crate::providers::create_provider(&fb.provider, pc) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!(
                provider = %fb.provider,
                error = %e,
                "fallback_provider create_provider failed; disabling"
            );
            None
        }
    }
}
```

Note: change `alephcore::providers::create_provider` to `crate::providers::create_provider` — this code now lives inside the alephcore lib.

- [ ] **A2.2: Replace `build_stability_triple` `todo!()` with implementation, returning `StabilityTriple` struct (not tuple)**

```rust
pub fn build_stability_triple(config: &Config) -> StabilityTriple {
    let Some(s) = config.stability.as_ref() else {
        return StabilityTriple {
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
        };
    };
    let stall_config = s.stall_timeout_secs.map(|secs| {
        let mut sc = crate::harness::StallConfig::default()
            .with_timeout(Duration::from_secs(secs));
        if let Some(ci) = s.stall_check_interval_secs {
            sc = sc.with_check_interval(Duration::from_secs(ci));
        }
        sc
    });
    StabilityTriple {
        stall_config,
        consecutive_failure_cap: s.consecutive_failure_cap,
        turn_timeout: s.turn_timeout_secs.map(Duration::from_secs),
    }
}
```

- [ ] **A2.3: Update `orchestrator_init.rs` to delegate to the shared module**

Open `src/bin/aleph-server/commands/start/orchestrator_init.rs`. Find lines 190–223 (`fn build_fallback_llm`). Delete the entire fn body, replace with a forwarding wrapper:

```rust
fn build_fallback_llm(
    config: &Config,
    primary_provider_key: &str,
) -> Option<Arc<dyn alephcore::providers::AiProvider>> {
    alephcore::orchestrator::build_fallback_llm(config, primary_provider_key)
}
```

Find lines 231–254 (`fn build_stability_triple`). Delete the body. Replace with:

```rust
fn build_stability_triple(
    config: &Config,
) -> (
    Option<alephcore::harness::StallConfig>,
    Option<usize>,
    Option<std::time::Duration>,
) {
    let triple = alephcore::orchestrator::build_stability_triple(config);
    (triple.stall_config, triple.consecutive_failure_cap, triple.turn_timeout)
}
```

The wrapper preserves the existing tuple-returning signature so the rest of `orchestrator_init.rs` (and its 13 builder unit tests + 4 init_audit tests) keeps working unchanged.

- [ ] **A2.4: Run existing main-runner builder tests to confirm no regression**

```bash
cargo test -p aleph-server --bin aleph-server -- orchestrator_init
```

Expected: Phase-6's 13 builder unit tests + 4 init_audit tests still pass (they exercise the wrappers, which now delegate). If the bin test target name differs, find it via `grep -r 'name = ' src/bin/aleph-server/Cargo.toml`.

- [ ] **A2.5: Add unit tests for the shared module in `deps_builder.rs`**

Append to `src/orchestrator/deps_builder.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::{FallbackProviderToml, ProviderConfig, StabilityToml};

    fn cfg_with_fallback(
        fb: Option<FallbackProviderToml>,
        providers: Vec<(&str, ProviderConfig)>,
    ) -> Config {
        let mut providers_map: std::collections::HashMap<String, ProviderConfig> =
            std::collections::HashMap::new();
        for (k, v) in providers {
            providers_map.insert(k.to_string(), v);
        }
        Config {
            fallback_provider: fb,
            providers: providers_map,
            ..Config::default()
        }
    }

    fn mock_provider_config() -> ProviderConfig {
        let mut pc = ProviderConfig::test_config("mock-model");
        pc.protocol = Some("mock".to_string());
        pc
    }

    #[test]
    fn fallback_returns_none_when_section_missing() {
        let cfg = Config::default();
        assert!(build_fallback_llm(&cfg, "primary").is_none());
    }

    #[test]
    fn fallback_returns_none_on_self_reference() {
        let cfg = cfg_with_fallback(
            Some(FallbackProviderToml {
                provider: "Primary".to_string(),
            }),
            vec![("primary", mock_provider_config())],
        );
        // ASCII case-insensitive match → self-reference detected.
        assert!(build_fallback_llm(&cfg, "primary").is_none());
    }

    #[test]
    fn stability_triple_independence_all_none() {
        let cfg = Config::default();
        let triple = build_stability_triple(&cfg);
        assert!(triple.stall_config.is_none());
        assert!(triple.consecutive_failure_cap.is_none());
        assert!(triple.turn_timeout.is_none());
    }

    #[test]
    fn stability_triple_only_turn_timeout_set() {
        let cfg = Config {
            stability: Some(StabilityToml {
                stall_timeout_secs: None,
                stall_check_interval_secs: None,
                consecutive_failure_cap: None,
                turn_timeout_secs: Some(60),
            }),
            ..Config::default()
        };
        let triple = build_stability_triple(&cfg);
        assert!(triple.stall_config.is_none());
        assert!(triple.consecutive_failure_cap.is_none());
        assert_eq!(triple.turn_timeout, Some(Duration::from_secs(60)));
    }
}
```

Note: imports of `FallbackProviderToml`, `ProviderConfig`, `StabilityToml` — adjust if these are not at crate root (`use crate::config::types::...` may be the actual path; verify with `rg "pub struct StabilityToml"`).

- [ ] **A2.6: Run new tests + verify R10 baseline unchanged**

```bash
cargo test -p alephcore --lib orchestrator::deps_builder
wc -l src/harness/*.rs | tail -1
```

Expected: 4 unit tests pass; harness line count unchanged from PF-3 baseline.

### Task A3: Add 5 fields to `SpawnerBase` and `AgentRuntime`

**Files:**
- Modify: `src/agents/subagent_spawner.rs:47-80` (SpawnerBase struct)
- Modify: `src/agents/runtime.rs:102-127` (AgentRuntime struct + builder methods)

- [ ] **A3.1: Add 5 fields to `SpawnerBase`**

Open `src/agents/subagent_spawner.rs`. After line 79 (`guardrails: Option<Arc<crate::guardrails::GuardrailRegistry>>`), add:

```rust
    /// Stage A (P1) — fallback LLM from `[fallback_provider]`. `None` when
    /// not configured or when self-referencing the primary. Inherited
    /// identically from main runner.
    pub fallback_llm: Option<Arc<dyn AiProvider>>,
    /// Stage A (P1) — stall watchdog config from `[stability]`. `None` when
    /// `stall_timeout_secs` is unset.
    pub stall_config: Option<crate::harness::StallConfig>,
    /// Stage A (P1) — bounded consecutive-failure cap from `[stability]`.
    pub consecutive_failure_cap: Option<usize>,
    /// Stage A (P1) — per-turn wall-clock timeout from `[stability]`.
    pub turn_timeout: Option<std::time::Duration>,
    /// Stage A (P1) — trace sink, cloned from parent's HarnessDeps.
    /// Subagent run events flow into the same sink as the main runner.
    pub trace_sink: Option<Arc<dyn crate::harness::TraceSink>>,
```

- [ ] **A3.2: Add 5 corresponding fields to `AgentRuntime`**

Open `src/agents/runtime.rs`. After line 126 (`guardrails: Option<Arc<crate::guardrails::GuardrailRegistry>>`), add:

```rust
    /// Stage A (P1) — fallback LLM threaded into SpawnerBase. `None` keeps
    /// legacy "no fallback" behavior.
    fallback_llm: Option<Arc<dyn AiProvider>>,
    /// Stage A (P1) — stall watchdog config threaded into SpawnerBase.
    stall_config: Option<crate::harness::StallConfig>,
    /// Stage A (P1) — consecutive-failure cap threaded into SpawnerBase.
    consecutive_failure_cap: Option<usize>,
    /// Stage A (P1) — per-turn timeout threaded into SpawnerBase.
    turn_timeout: Option<std::time::Duration>,
    /// Stage A (P1) — trace sink threaded into SpawnerBase.
    trace_sink: Option<Arc<dyn crate::harness::TraceSink>>,
```

- [ ] **A3.3: Update `AgentRuntime::new` to default 5 new fields to `None`**

Find the `Self { ... }` block in `AgentRuntime::new` (around line 140). Add after `guardrails: None`:

```rust
            fallback_llm: None,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
            trace_sink: None,
```

- [ ] **A3.4: Add 5 `with_*` builder methods to `AgentRuntime` (mirror existing `with_guardrails` pattern)**

Find `with_guardrails` around line 156. After it, add:

```rust
    /// Stage A (P1) — wire the fallback LLM. Subagents inherit it identically.
    pub fn with_fallback_llm(mut self, fallback: Arc<dyn AiProvider>) -> Self {
        self.fallback_llm = Some(fallback);
        self
    }

    /// Stage A (P1) — wire the stall watchdog config.
    pub fn with_stall_config(mut self, config: crate::harness::StallConfig) -> Self {
        self.stall_config = Some(config);
        self
    }

    /// Stage A (P1) — wire the consecutive-failure cap.
    pub fn with_consecutive_failure_cap(mut self, cap: usize) -> Self {
        self.consecutive_failure_cap = Some(cap);
        self
    }

    /// Stage A (P1) — wire the per-turn wall-clock timeout.
    pub fn with_turn_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.turn_timeout = Some(timeout);
        self
    }

    /// Stage A (P1) — wire the trace sink. Subagents emit into the same sink.
    pub fn with_trace_sink(mut self, sink: Arc<dyn crate::harness::TraceSink>) -> Self {
        self.trace_sink = Some(sink);
        self
    }
```

- [ ] **A3.5: Update `AgentRuntime::execute_via_harness` SpawnerBase construction**

Open `src/agents/runtime.rs:282-293` (the `let base = SpawnerBase { ... }` block). Add 5 new fields:

```rust
        let base = SpawnerBase {
            session: self.session.clone(),
            parent_tools: self.parent_tools.clone(),
            sandbox: self.sandbox.clone(),
            provider: self.provider.clone(),
            chain: parent_chain,
            raw_memory_writer: self.raw_memory_writer.clone(),
            capture_registry: self.capture_registry.clone(),
            parent_agent_id: self.parent_agent_id.clone(),
            parent_session_id: self.parent_session_id.clone(),
            guardrails: self.guardrails.clone(),
            // Stage A (P1):
            fallback_llm: self.fallback_llm.clone(),
            stall_config: self.stall_config.clone(),
            consecutive_failure_cap: self.consecutive_failure_cap,
            turn_timeout: self.turn_timeout,
            trace_sink: self.trace_sink.clone(),
        };
```

- [ ] **A3.6: Update `make_base` test helper at `subagent_spawner.rs:623-640` to include 5 new fields as `None`**

Find `fn make_base` and its `SpawnerBase { ... }` literal. Add:

```rust
        SpawnerBase {
            session: ...,
            parent_tools: ...,
            sandbox: ...,
            provider: provider.clone(),
            chain: ChainContext::default(),
            raw_memory_writer: None,
            capture_registry: None,
            parent_agent_id: None,
            parent_session_id: None,
            guardrails: None,
            // Stage A (P1):
            fallback_llm: None,
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
            trace_sink: None,
        }
```

- [ ] **A3.7: Build to verify all SpawnerBase construction sites are updated**

```bash
cargo check -p alephcore
```

Expected: clean. If `error: missing fields` shows another construction site, update it the same way (probably tests or runtime.rs).

### Task A4: Wire 5 fields into `subagent_spawner::spawn` HarnessDeps

**Files:**
- Modify: `src/agents/subagent_spawner.rs:200-225` (the `HarnessDeps { ... }` block)

- [ ] **A4.1: Replace 5 `None` with `base.*.clone()` reads**

Open `src/agents/subagent_spawner.rs:200-225`. Replace as follows:

```rust
    let deps = HarnessDeps {
        session: base.session.clone(),
        tools: scoped_tools,
        sandbox: base.sandbox.clone(),
        llm,
        verifier_chain: None,
        context_budget: None,
        context_compactor: None,
        skill_prefetcher: None,
        // Stage A (P1) — was None; now inherited from parent SpawnerBase.
        trace_sink: base.trace_sink.clone(),
        system_prompt: Some(system_prompt),
        prompt_builder: std::sync::Arc::new(crate::harness::prompt::DefaultPromptBuilder),
        chain_context: child_chain.clone(),
        guardrails: base.guardrails.clone(),
        // Stage A (P1) — was None; now inherited from parent SpawnerBase.
        fallback_llm: base.fallback_llm.clone(),
        max_iterations: max_iter,
        power: None,
        // Stage A (P1) — was None for all three; now inherited from parent.
        stall_config: base.stall_config.clone(),
        consecutive_failure_cap: base.consecutive_failure_cap,
        turn_timeout: base.turn_timeout,
    };
```

- [ ] **A4.2: Build verify**

```bash
cargo check -p alephcore
```

Expected: clean.

- [ ] **A4.3: Run all subagent_spawner tests to ensure no regression**

```bash
cargo test -p alephcore --lib agents::subagent_spawner
```

Expected: All existing subagent_spawner tests pass (the 5 new fields default to `None` in test fixtures, so behavior is identical to before).

### Task A5: Integration test — `subagent_inherits_5_fields`

**Files:**
- Create: `tests/integration/subagent_deps_inherit.rs`

- [ ] **A5.1: Research existing test fixture patterns**

Read these specific spots in `src/agents/subagent_spawner.rs` and document on scratch (not committed):

```bash
sed -n '600,750p' src/agents/subagent_spawner.rs > /tmp/subagent_spawner_test_fixture.txt
cat /tmp/subagent_spawner_test_fixture.txt
```

Record:
- Line of `fn make_base(provider: ...)` definition
- Line of any `MockProvider` / `RecordingProvider` / `ProviderProbe` struct
- Line of any `FakeSession` / `FakeTools` / `MockSandbox` constructors
- Test functions that already exercise spawn() end-to-end (look for `#[tokio::test]` markers)

The integration test in A5.2 will adapt these patterns. If no recording provider exists, A5.2 must add one inline.

- [ ] **A5.2: Determine the assertion strategy (R10-safe)**

The straightforward approach (probe HarnessDeps after spawn) requires either:
- (a) A `pub(crate)` accessor on `AgentHarness` exposing the `HarnessDeps` snapshot — touches `src/harness/`, R10 risk
- (b) A recording `AiProvider` that captures the HarnessDeps reference handed to it via the LLM call — no harness changes
- (c) A structural test that asserts SpawnerBase fields equal expected values, plus a separate hand-trace audit comment that A4.1's wiring is sound — weakest, but R10-safe

**Pick (b) if subagent_spawner.rs already has a recording provider pattern (from A5.1 research). Pick (c) if not.** Document the choice in scratch.

If (b): A5.3 implements a `RecordingProvider` that exposes the captured HarnessDeps fields after spawn returns. The provider is invoked by `AgentHarness::run` at LLM-call time; at that point HarnessDeps is fully assembled.

If (c): A5.3 asserts SpawnerBase populated values + walks through the spawn fn's HarnessDeps construction site (lines 200-225) by reading the file and verifying the wiring assertion. The test itself only verifies that SpawnerBase carries the values; the wiring is verified by A4.3's regression check.

- [ ] **A5.3: Write the integration test using the chosen strategy**

Create `tests/integration/subagent_deps_inherit.rs` with this skeleton; fill the marked sections from A5.1 research:

```rust
//! Stage A integration: SpawnerBase carries the 5 P1 fields and
//! subagent's HarnessDeps inherits them identically.

use std::sync::Arc;
use std::time::Duration;

use alephcore::harness::StallConfig;
use tokio_util::sync::CancellationToken;

// === BEGIN: copy the relevant mock structs from
// src/agents/subagent_spawner.rs:600-750 (FakeSession, FakeTools,
// FakeSandbox, FakeProvider patterns) ===
//
// ... (paste mocks here, adapted for `pub` access since we're outside
// the spawner module)
// === END copied mocks ===

#[tokio::test]
async fn subagent_base_carries_5_p1_fields() {
    use alephcore::agents::subagent_spawner::{spawn, SpawnRequest, SpawnerBase};
    use alephcore::agents::{AgentDef, AgentMode};
    use alephcore::harness::chain_context::ChainContext;

    let stall = StallConfig::default().with_timeout(Duration::from_secs(123));
    let cap = 7usize;
    let turn = Duration::from_secs(456);

    // Build the mocks (adapted from A5.1 research output):
    let session = Arc::new(/* FakeSession from research */);
    let tools = Arc::new(/* FakeTools from research */);
    let sandbox = Arc::new(/* FakeSandbox from research */);
    let provider = Arc::new(/* FakeProvider that returns immediately */);

    let base = SpawnerBase {
        session,
        parent_tools: tools,
        sandbox,
        provider,
        chain: ChainContext::default(),
        raw_memory_writer: None,
        capture_registry: None,
        parent_agent_id: None,
        parent_session_id: None,
        guardrails: None,
        // The 5 Stage A fields with sentinel values:
        fallback_llm: None, // can stay None — the test asserts the field travels, not its value
        stall_config: Some(stall.clone()),
        consecutive_failure_cap: Some(cap),
        turn_timeout: Some(turn),
        trace_sink: None,
        lane_scheduler: None, // Stage C field (added in C3.3)
    };

    // Strategy (b) — recording provider exposes captured HarnessDeps after spawn:
    // OR
    // Strategy (c) — assert SpawnerBase fields directly + walk the file (no spawn call).

    // Strategy (c) example:
    assert_eq!(base.stall_config.as_ref().unwrap().timeout(), Duration::from_secs(123));
    assert_eq!(base.consecutive_failure_cap, Some(cap));
    assert_eq!(base.turn_timeout, Some(turn));

    // For completeness, run spawn() to verify the build doesn't error:
    let agent_def = AgentDef::new("test", AgentMode::SubAgent);
    let cancel = CancellationToken::new();
    let req = SpawnRequest {
        agent_def: &agent_def,
        task: "test task",
        context_summary: None,
        model: None,
        timeout_secs: 1,
        cancel,
    };
    let _result = spawn(&base, req).await; // may Err with a benign message; we only care it compiled.
}
```

This is a structural-correctness test (strategy c) — augment with strategy b if the research in A5.1 surfaces an existing recording-provider pattern.

- [ ] **A5.4: Run integration test**

```bash
cargo test --test subagent_deps_inherit
```

Expected: PASS. If FAIL with "missing field" or "no method" errors, the mocks need adaptation — return to A5.1 and copy the actual signatures.

### Task A6: Documentation — update `MULTI_AGENT_SYSTEM.md` for Stage A

**Files:**
- Modify: `docs/reference/MULTI_AGENT_SYSTEM.md`

- [ ] **A6.1: Find HarnessDeps inheritance section**

```bash
grep -n "HarnessDeps\|inherit\|guardrails" docs/reference/MULTI_AGENT_SYSTEM.md
```

Locate the section discussing what subagents inherit from parents.

- [ ] **A6.2: Update to mention 5 P1 fields**

Replace any "subagent inherits guardrails only" wording with:

> Subagents inherit the following from their parent via `SpawnerBase`:
> - `guardrails` (Stage 5a) — Input/Output/ToolCall checks
> - `fallback_llm` (Stage A, 2026-05-08) — Stage 5b single-step fallback
> - `stall_config`, `consecutive_failure_cap`, `turn_timeout` (Stage A) — P0 stability triple
> - `trace_sink` (Stage A) — observability sink
>
> Per the P1 zero-override decision, subagents do not currently support per-agent overrides for these fields. AgentDef may be extended with `Option<T>` overrides in P4 if needed, with full backward compatibility.

### Task A7: Commit Stage A

- [ ] **A7.1: Run the full lib test suite**

```bash
cargo test -p alephcore --lib
cargo test --test subagent_deps_inherit
cargo clippy -p alephcore -- -D warnings
```

Expected: All green.

- [ ] **A7.2: Verify R10 baseline**

```bash
wc -l src/harness/*.rs | tail -1
ls src/harness/*.rs | wc -l
```

Expected: total ≤ 1500, exactly 9 files (matches PF-3 baseline).

- [ ] **A7.3: Commit Stage A**

```bash
git add src/orchestrator/ src/lib.rs src/bin/aleph-server/commands/start/orchestrator_init.rs src/agents/subagent_spawner.rs src/agents/runtime.rs tests/integration/subagent_deps_inherit.rs docs/reference/MULTI_AGENT_SYSTEM.md
git commit -m "agents: extract Phase-6 builders to src/orchestrator/deps_builder.rs

Stage A of P1 subagent uplift. Wires 5 HarnessDeps fields on the subagent
spawn path that were hardcoded None: fallback_llm, stall_config,
consecutive_failure_cap, turn_timeout, trace_sink.

- New src/orchestrator/deps_builder.rs holds build_fallback_llm and
  build_stability_triple as pub fns (StabilityTriple struct return).
- orchestrator_init.rs delegates to the shared module via thin wrappers
  preserving existing test surface (13 builder + 4 init_audit tests
  unchanged).
- SpawnerBase + AgentRuntime gain 5 fields with builder-style with_*
  methods. Subagents inherit identically (zero-override per P1 spec).
- Integration test subagent_deps_inherit verifies end-to-end propagation.

R10: zero src/harness/ changes. Spec
docs/superpowers/specs/2026-05-08-subagent-uplift-p1-design.md § 2."
```

---

## Stage B — Recursion Guard (Commit 2)

**Goal:** Close the recursion-guard hole by making `AgentDef::is_tool_allowed` deny `"subagent"` when mode is `SubAgent`. Update notes/docs.

### Task B1: Unit tests for mode-aware deny

**Files:**
- Modify: `src/agents/types.rs` (add tests in existing `#[cfg(test)] mod tests`)

- [ ] **B1.1: Add 3 unit tests AFTER the existing tests in types.rs**

Open `src/agents/types.rs`. After `test_with_when_to_use` (line 280), add:

```rust
    #[test]
    fn subagent_mode_denies_subagent_tool_even_with_wildcard() {
        // SubAgent mode: structurally forbidden from spawning further subagents.
        // Recursion guard overrides allowlist (even "*").
        let agent = AgentDef::new("test", AgentMode::SubAgent);
        // allowed_tools defaults to ["*"] — without the guard this would allow.
        assert!(!agent.is_tool_allowed("subagent"));
    }

    #[test]
    fn subagent_mode_denies_subagent_tool_with_explicit_entry() {
        // Even if "subagent" is explicitly listed in allowed_tools, mode-deny
        // overrides. Recursion safety is a system invariant, not a knob.
        let agent = AgentDef::new("test", AgentMode::SubAgent)
            .with_allowed_tools(vec!["subagent".into(), "read".into()]);
        assert!(!agent.is_tool_allowed("subagent"));
        // Other tools work as expected.
        assert!(agent.is_tool_allowed("read"));
    }

    #[test]
    fn primary_mode_allows_subagent_tool() {
        // Primary mode is unaffected — main agent retains full subagent
        // spawning capability.
        let agent = AgentDef::new("test", AgentMode::Primary)
            .with_allowed_tools(vec!["subagent".into()]);
        assert!(agent.is_tool_allowed("subagent"));
    }
```

- [ ] **B1.2: Run tests to verify all 3 FAIL**

```bash
cargo test -p alephcore --lib agents::types::tests::subagent_mode_denies_subagent_tool_even_with_wildcard agents::types::tests::subagent_mode_denies_subagent_tool_with_explicit_entry agents::types::tests::primary_mode_allows_subagent_tool
```

Expected: 2 FAIL (the deny tests; subagent currently allowed), 1 PASS (primary allows). The two failures confirm the bug.

### Task B2: Implement mode-aware deny

**Files:**
- Modify: `src/agents/types.rs:142-154` (`is_tool_allowed` body)

- [ ] **B2.1: Add the mode-aware deny check at the top of `is_tool_allowed`**

Replace lines 142–154:

```rust
    /// Check if a tool is allowed for this agent.
    ///
    /// SubAgent-mode agents are structurally forbidden from invoking the
    /// `subagent` tool — this prevents infinite recursion regardless of
    /// allowlist contents (including wildcard `"*"` or explicit `"subagent"`
    /// entries). Primary-mode agents are unaffected. See
    /// docs/reference/MULTI_AGENT_SYSTEM.md for the recursion-protection
    /// design.
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        // Recursion guard: SubAgent mode is structurally forbidden from
        // spawning further subagents. Overrides allowlist (even "*" / explicit
        // "subagent") because recursion safety is a system invariant.
        if matches!(self.mode, AgentMode::SubAgent) && tool_name == "subagent" {
            return false;
        }

        // Check denied list first.
        if self.denied_tools.iter().any(|t| t == tool_name) {
            return false;
        }

        // Check allowed list.
        if self.allowed_tools.iter().any(|t| t == "*") {
            return true;
        }

        self.allowed_tools.iter().any(|t| t == tool_name)
    }
```

- [ ] **B2.2: Run the 3 new tests + the existing types tests**

```bash
cargo test -p alephcore --lib agents::types
```

Expected: all green (existing 16 tests + 3 new = 19 tests).

- [ ] **B2.3: Run AllowlistToolService tests to verify no regression**

```bash
cargo test -p alephcore --lib agents::allowlist_tool_service
```

Expected: 6 existing tests pass (they exercise execute/list/describe via is_tool_allowed; mode-deny path doesn't fire for their fixtures).

### Task B3: Comment + doc updates

**Files:**
- Modify: `src/agents/subagent_tool.rs:6` (misleading comment)
- Modify: `docs/reference/MULTI_AGENT_SYSTEM.md` (recursion protection section)

- [ ] **B3.1: Update `subagent_tool.rs:6` misleading comment**

Open `src/agents/subagent_tool.rs`. Find line 6 (search for `excludes the subagent tool`). Replace the comment with the truth:

```rust
//! Subagent tool registration. SubAgent-mode agents are denied invocation
//! of this tool via `AgentDef::is_tool_allowed` (recursion guard); see
//! `agents/types.rs` for the rule.
```

- [ ] **B3.2: Update `MULTI_AGENT_SYSTEM.md` recursion protection section**

```bash
grep -n "recursion\|infinite recursion" docs/reference/MULTI_AGENT_SYSTEM.md | head -10
```

Find the section discussing recursion protection. Replace stale wording with:

> ## Recursion Protection
>
> SubAgent-mode agents are structurally denied from invoking the `subagent`
> tool. Enforcement lives in `AgentDef::is_tool_allowed`
> (`src/agents/types.rs`), which overrides any explicit allowlist entry
> (including wildcard `"*"`). Primary-mode agents retain full subagent
> spawning capability.
>
> Two additional defense layers exist:
> - `ChainContext::child()` depth guard (`subagent_spawner.rs:114-117`)
>   returns `None` when `max_depth` is reached, surfacing as a `"chain
>   depth exceeded"` error.
> - `LaneScheduler::check_recursion_depth` (`scheduler/lane_scheduler.rs`)
>   tracks parent→child relationships across the run lifetime.

### Task B4: Integration test — `recursion_guard_end_to_end`

**Files:**
- Create: `tests/integration/recursion_guard.rs`

- [ ] **B4.1: Write the integration test**

```rust
//! Stage B integration: end-to-end verification that a SubAgent-mode agent
//! cannot invoke the `subagent` tool. Exercises the
//! AllowlistToolService::execute / list / describe paths via the
//! is_tool_allowed mode-deny rule.

use std::sync::Arc;

use alephcore::agents::allowlist_tool_service::AllowlistToolService;
use alephcore::agents::{AgentDef, AgentMode};
use alephcore::session::events::{ToolOutput, ToolOutputMetadata};
use alephcore::tools::service::{ToolDefinition, ToolError, ToolService, ToolSource};
use async_trait::async_trait;
use serde_json::{json, Value};

struct ParentToolsWithSubagent;

#[async_trait]
impl ToolService for ParentToolsWithSubagent {
    async fn execute(&self, name: &str, _: Value) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput {
            value: json!({ "tool": name }),
            metadata: ToolOutputMetadata::default(),
        })
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        ["read", "subagent"]
            .iter()
            .map(|n| ToolDefinition {
                name: (*n).into(),
                description: "test".into(),
                input_schema: json!({}),
                source: ToolSource::Builtin,
                metadata: Default::default(),
            })
            .collect()
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        self.list().await.into_iter().find(|d| d.name == name)
    }

    fn dispatcher_schema(&self) -> Arc<[alephcore::dispatcher::ToolDefinition]> {
        Arc::from([])
    }
}

#[tokio::test]
async fn subagent_mode_cannot_see_or_execute_subagent_tool() {
    // Build a SubAgent-mode agent with wildcard allowlist (would allow
    // everything without the recursion guard).
    let mut agent_def = AgentDef::new("test_subagent", AgentMode::SubAgent);
    agent_def.allowed_tools = vec!["*".into()];
    let agent_def = Arc::new(agent_def);

    let svc =
        AllowlistToolService::new(Arc::new(ParentToolsWithSubagent), agent_def.clone());

    // 1. list() — subagent must not appear, despite "*" allowlist.
    let listed = svc.list().await;
    let names: Vec<&str> = listed.iter().map(|d| d.name.as_str()).collect();
    assert!(
        !names.contains(&"subagent"),
        "subagent tool leaked into SubAgent-mode list: {names:?}"
    );
    assert!(names.contains(&"read"), "non-subagent tools should pass");

    // 2. describe("subagent") — must return None.
    assert!(
        svc.describe("subagent").await.is_none(),
        "describe('subagent') should return None for SubAgent mode"
    );

    // 3. execute("subagent", ...) — must return PermissionDenied.
    let err = svc
        .execute("subagent", json!({}))
        .await
        .expect_err("execute('subagent') should fail for SubAgent mode");
    assert!(
        matches!(err, ToolError::PermissionDenied { .. }),
        "expected PermissionDenied, got: {err:?}"
    );
}

#[tokio::test]
async fn primary_mode_can_invoke_subagent_tool() {
    // Sanity: Primary mode with explicit allowlist works as expected.
    let mut agent_def = AgentDef::new("test_primary", AgentMode::Primary);
    agent_def.allowed_tools = vec!["subagent".into(), "read".into()];
    let agent_def = Arc::new(agent_def);

    let svc =
        AllowlistToolService::new(Arc::new(ParentToolsWithSubagent), agent_def.clone());

    let result = svc.execute("subagent", json!({})).await;
    assert!(result.is_ok(), "Primary should allow subagent: {result:?}");
}
```

- [ ] **B4.2: Run the integration test — expect PASS**

```bash
cargo test --test recursion_guard
```

Expected: 2 tests pass (subagent denied, primary allowed).

### Task B5: Commit Stage B

- [ ] **B5.1: Run full lib + integration tests**

```bash
cargo test -p alephcore --lib agents::
cargo test --test recursion_guard
cargo test --test subagent_deps_inherit
cargo clippy -p alephcore -- -D warnings
```

Expected: All green.

- [ ] **B5.2: Verify R10 baseline**

```bash
wc -l src/harness/*.rs | tail -1
ls src/harness/*.rs | wc -l
```

Expected: unchanged.

- [ ] **B5.3: Commit Stage B**

```bash
git add src/agents/types.rs src/agents/subagent_tool.rs docs/reference/MULTI_AGENT_SYSTEM.md tests/integration/recursion_guard.rs
git commit -m "agents: add SubAgent-mode recursion guard to is_tool_allowed

Stage B of P1 subagent uplift. Closes the recursion-guard hole flagged
in subagent_tool.rs:6 (the comment claimed exclusion but no code
enforced it).

- AgentDef::is_tool_allowed denies tool name 'subagent' when
  AgentMode::SubAgent, overriding allowlist (including wildcard '*').
- AllowlistToolService unchanged — execute/list/describe all flow
  through is_tool_allowed, so the deny is consistent across all paths.
- Comment in subagent_tool.rs:6 rewritten to point at the actual rule.
- MULTI_AGENT_SYSTEM.md recursion protection section updated.
- Integration test recursion_guard verifies end-to-end SubAgent agent
  cannot list/describe/execute the subagent tool.

R10: zero src/harness/ changes. Spec
docs/superpowers/specs/2026-05-08-subagent-uplift-p1-design.md § 3."
```

---

## Stage C — LaneScheduler Integration (Commit 3)

**Goal:** Add `LaneScheduler::try_reserve` (fail-fast API), wire it into the subagent spawn path, change `Lane::Subagent` default `max_concurrent` from 8 to 4, delete the line-113 TODO, free wins via `record_spawn` + `check_recursion_depth`.

### Task C1: Add `SchedulerError::LaneBudgetExhausted` variant + `try_reserve` API

**Files:**
- Modify: `src/scheduler/lane_scheduler.rs:18-23` (SchedulerError enum)
- Modify: `src/scheduler/lane_scheduler.rs` (add `try_reserve` method)
- Modify: `src/scheduler/lane_scheduler.rs:113` (delete TODO comment)

- [ ] **C1.1: Add `LaneBudgetExhausted` variant to `SchedulerError`**

Open `src/scheduler/lane_scheduler.rs`. Replace lines 18–23:

```rust
/// Scheduler-specific errors
#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    /// The requested lane has no configured quota
    #[error("unknown lane: {0:?}")]
    UnknownLane(Lane),
    /// Lane budget exhausted — no permits available without queueing.
    /// Used by `try_reserve` for fail-fast semantics.
    #[error("lane {lane:?} budget exhausted (max={max})")]
    LaneBudgetExhausted { lane: Lane, max: usize },
}
```

- [ ] **C1.2: Add `try_reserve` method to `LaneScheduler`**

After the `enqueue` method (around line 96), insert:

```rust
    /// Atomically attempt to reserve a lane slot without queueing.
    ///
    /// Unlike `enqueue` + `try_schedule_next`, this is **fail-fast**:
    /// if global capacity or lane capacity is exhausted, returns
    /// `SchedulerError::LaneBudgetExhausted` immediately. The caller is
    /// responsible for translating this to a domain-appropriate error
    /// (e.g., spawner → `ToolError::Execution`).
    ///
    /// On success, returns a `ScheduleGuard` whose `Drop` releases permits
    /// via RAII. Caller MUST also invoke `on_run_complete` on all exit
    /// paths to clear lane state tracking (the guard handles permits;
    /// `on_run_complete` handles state).
    ///
    /// # Errors
    /// - `UnknownLane` — lane not configured in `LaneConfig::quotas`
    /// - `LaneBudgetExhausted` — global or lane capacity exhausted
    pub async fn try_reserve(
        &self,
        run_id: String,
        lane: Lane,
    ) -> Result<ScheduleGuard, SchedulerError> {
        let state = self
            .lanes
            .get(&lane)
            .ok_or(SchedulerError::UnknownLane(lane))?;

        // Acquire global permit first (matches try_schedule_next ordering).
        let global_permit = self.global_semaphore.try_acquire().map_err(|_| {
            SchedulerError::LaneBudgetExhausted {
                lane,
                max: self.config.global_max_concurrent,
            }
        })?;

        // Acquire lane permit; on failure, release global to avoid leaking.
        let lane_max = self
            .config
            .quotas
            .get(&lane)
            .map(|q| q.max_concurrent)
            .unwrap_or(0);
        let lane_permit = match state.try_acquire_permit() {
            Some(permit) => permit,
            None => {
                drop(global_permit);
                return Err(SchedulerError::LaneBudgetExhausted {
                    lane,
                    max: lane_max,
                });
            }
        };

        // Mark running; permits become RAII responsibility of ScheduleGuard.
        state.mark_running(run_id.clone()).await;
        // SAFETY: ScheduleGuard takes ownership of permit release via its
        // own Drop impl, ensuring permits are returned exactly once even
        // if the caller panics. We forget the SemaphorePermits here so
        // their own Drop does not run.
        std::mem::forget(global_permit);
        std::mem::forget(lane_permit);

        Ok(ScheduleGuard {
            global_semaphore: Arc::clone(&self.global_semaphore),
            lane_semaphore: Arc::clone(state.semaphore()),
        })
    }
```

- [ ] **C1.3: Delete the obsolete TODO at line 113**

Find the comment `// TODO: In future, we can apply per-run priority boosts here` (currently around line 113 inside `try_schedule_next`). Delete that one line. The surrounding code:

```rust
        // Sort lanes by priority (highest first), applying anti-starvation boosts
        let mut lanes_by_priority: Vec<_> = self
```

(no comment line).

- [ ] **C1.4: Add unit tests for `try_reserve`**

In `src/scheduler/lane_scheduler.rs`'s `#[cfg(test)] mod tests`, append:

```rust
    #[tokio::test]
    async fn try_reserve_succeeds_with_capacity() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        // Subagent default after Stage C is 4; reserve 4 should succeed.
        let mut guards = vec![];
        for i in 0..4 {
            let guard = scheduler
                .try_reserve(format!("sub-{i}"), Lane::Subagent)
                .await
                .expect("reserve should succeed within capacity");
            guards.push(guard);
        }
        assert_eq!(guards.len(), 4);
    }

    #[tokio::test]
    async fn try_reserve_fails_when_lane_exhausted() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        // Fill the Subagent lane (default cap 4).
        let mut _guards = vec![];
        for i in 0..4 {
            _guards.push(
                scheduler
                    .try_reserve(format!("sub-{i}"), Lane::Subagent)
                    .await
                    .expect("first 4 reserves should succeed"),
            );
        }
        // 5th must fail with LaneBudgetExhausted.
        let err = scheduler
            .try_reserve("sub-5".to_string(), Lane::Subagent)
            .await
            .expect_err("5th reserve should fail");
        match err {
            SchedulerError::LaneBudgetExhausted { lane, max } => {
                assert_eq!(lane, Lane::Subagent);
                assert_eq!(max, 4);
            }
            other => panic!("expected LaneBudgetExhausted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn try_reserve_fails_when_global_exhausted() {
        // Global cap 1; lane has plenty of room.
        let config = LaneConfig {
            global_max_concurrent: 1,
            ..LaneConfig::default()
        };
        let scheduler = LaneScheduler::new(config);

        let _guard = scheduler
            .try_reserve("first".to_string(), Lane::Subagent)
            .await
            .expect("first reserve uses the only global slot");

        let err = scheduler
            .try_reserve("second".to_string(), Lane::Subagent)
            .await
            .expect_err("second reserve should fail (global exhausted)");
        assert!(matches!(err, SchedulerError::LaneBudgetExhausted { .. }));
    }

    #[tokio::test]
    async fn try_reserve_unknown_lane() {
        // Build config without Cron quota.
        let mut config = LaneConfig::default();
        config.quotas.remove(&Lane::Cron);
        let scheduler = LaneScheduler::new(config);

        let err = scheduler
            .try_reserve("x".to_string(), Lane::Cron)
            .await
            .expect_err("unknown lane must fail");
        assert!(matches!(err, SchedulerError::UnknownLane(Lane::Cron)));
    }

    #[tokio::test]
    async fn guard_drop_releases_permit() {
        let config = LaneConfig::default();
        let scheduler = LaneScheduler::new(config);

        // Fill Subagent lane (default 4).
        let mut held = vec![];
        for i in 0..4 {
            held.push(
                scheduler
                    .try_reserve(format!("sub-{i}"), Lane::Subagent)
                    .await
                    .unwrap(),
            );
        }

        // Drop one guard; permit should be released.
        drop(held.pop().unwrap());

        // Now we can reserve again.
        let _guard = scheduler
            .try_reserve("sub-replacement".to_string(), Lane::Subagent)
            .await
            .expect("after drop, reserve should succeed");
    }
```

- [ ] **C1.5: Run scheduler tests**

```bash
cargo test -p alephcore --lib scheduler::lane_scheduler
```

Expected: all green (existing tests + 5 new = 5 new pass).

### Task C2: Change `Lane::Subagent` default capacity 8 → 4

**Files:**
- Modify: `src/scheduler/lane.rs:25` (default_max_concurrent)
- Modify: `src/scheduler/lane_config.rs:108-111` (test assertion)

- [ ] **C2.1: Change default in `Lane::default_max_concurrent`**

Open `src/scheduler/lane.rs:22-29`. Change:

```rust
    pub fn default_max_concurrent(&self) -> usize {
        match self {
            Lane::Main => 2,
            Lane::Subagent => 4,  // P1 Stage C: was 8; per Q6a sweet spot for personal AI scenarios
            Lane::Cron => 2,
            Lane::Nested => 4,
        }
    }
```

- [ ] **C2.2: Update affected test in `lane_config.rs`**

Open `src/scheduler/lane_config.rs:108-111`. Change the comment + assertion:

```rust
        // Subagent lane: 4 concurrent (P1 Stage C), 500k tokens/min, priority 5
        let subagent_quota = config.get_quota(&Lane::Subagent).unwrap();
        assert_eq!(subagent_quota.max_concurrent, 4);
        assert_eq!(subagent_quota.token_budget_per_min, 500_000);
        assert_eq!(subagent_quota.priority, 5);
```

- [ ] **C2.3: Run scheduler tests to confirm no other test broke**

```bash
cargo test -p alephcore --lib scheduler::
```

Expected: all green. If any test asserts `max_concurrent == 8` for Subagent, update similarly.

### Task C3: Add `LaneScheduler` to `AgentRuntime` + `SpawnerBase`

**Files:**
- Modify: `src/agents/runtime.rs` (add field + builder method)
- Modify: `src/agents/subagent_spawner.rs:47-79` (add field to SpawnerBase)

- [ ] **C3.1: Add `lane_scheduler` field to `AgentRuntime`**

In `src/agents/runtime.rs`, after the 5 Stage A fields, add:

```rust
    /// Stage C (P1) — lane budget enforcement for subagent spawns.
    /// `None` keeps the legacy "no lane wiring" behavior; `Some(_)` enforces
    /// the configured Subagent lane cap with fail-fast semantics.
    lane_scheduler: Option<Arc<crate::scheduler::LaneScheduler>>,
```

In `AgentRuntime::new`, default it to `None`:

```rust
            lane_scheduler: None,
```

- [ ] **C3.2: Add `with_lane_scheduler` builder method on `AgentRuntime`**

After the 5 Stage A `with_*` methods:

```rust
    /// Stage C (P1) — wire the lane scheduler. Subagent spawns reserve
    /// `Lane::Subagent` budget; on exhaustion, spawn returns
    /// `ToolError::Execution`.
    pub fn with_lane_scheduler(
        mut self,
        scheduler: Arc<crate::scheduler::LaneScheduler>,
    ) -> Self {
        self.lane_scheduler = Some(scheduler);
        self
    }
```

- [ ] **C3.3: Add `lane_scheduler` field to `SpawnerBase`**

In `src/agents/subagent_spawner.rs`, after the 5 Stage A fields:

```rust
    /// Stage C (P1) — lane budget enforcement. `None` skips lane checks
    /// (legacy behavior); `Some(_)` reserves on entry, releases on exit.
    pub lane_scheduler: Option<Arc<crate::scheduler::LaneScheduler>>,
```

- [ ] **C3.4: Update `AgentRuntime::execute_via_harness` SpawnerBase construction to pass scheduler**

Add `lane_scheduler: self.lane_scheduler.clone(),` to the `SpawnerBase { ... }` literal (alongside the 5 Stage A fields).

- [ ] **C3.5: Update `make_base` test helper**

In `src/agents/subagent_spawner.rs:629`, add `lane_scheduler: None,` to the literal.

- [ ] **C3.6: Verify build**

```bash
cargo check -p alephcore
```

Expected: clean.

### Task C4: Wire `try_reserve` + `record_spawn` + `check_recursion_depth` into `spawn`

**Files:**
- Modify: `src/agents/subagent_spawner.rs` (wrap spawn body)

- [ ] **C4.1: Insert lane reservation at the top of `spawn` (after step 1, the chain depth check)**

Open `src/agents/subagent_spawner.rs:111`. After the existing chain depth check (lines 112-117) and BEFORE step 2 (ephemeral session key creation, line 120), insert:

```rust
    // Stage C (P1) — lane budget reservation (fail-fast). Skipped when
    // `base.lane_scheduler` is None (legacy callers / tests).
    let lane_run_id = format!("subagent-{}", uuid::Uuid::new_v4());
    let parent_run_id = base.chain.chain_id.clone();
    let lane_guard = if let Some(scheduler) = base.lane_scheduler.as_ref() {
        // Defense-in-depth recursion check (third layer; ChainContext + mode
        // deny are layers 1-2). Free win since the tracker exists.
        scheduler
            .check_recursion_depth(&parent_run_id)
            .await
            .map_err(|e| format!("sub-agent failed: recursion depth exceeded: {e}"))?;

        let guard = scheduler
            .try_reserve(lane_run_id.clone(), crate::scheduler::Lane::Subagent)
            .await
            .map_err(|e| format!("sub-agent failed: subagent lane budget exhausted: {e}"))?;

        // Track parent→child for recursion accounting.
        scheduler.record_spawn(&parent_run_id, &lane_run_id).await;

        Some((scheduler.clone(), guard))
    } else {
        None
    };
```

Note the error mapping: the existing `spawn` returns `Result<LoopRunResult, String>`. The "sub-agent failed:" prefix matches the existing convention used elsewhere in spawn (e.g., line 127: `"sub-agent failed: attach session"`). The downstream `subagent_tool.rs` translates this string error into `ToolError::Execution`, so the final user-facing path matches the spec § 4.4 mapping.

- [ ] **C4.2: Add lane release at all exit points of `spawn`**

The current `spawn` has multiple early returns (lines 248–258, 290). For each `return Err(...)` path AND the final `Ok(result)` return, ensure `lane_guard` is released cleanly. The cleanest pattern: capture the result and release before returning.

Restructure step 7 + 8 + 9 of `spawn` so the entire flow goes through a single `let outcome = ...; release; return outcome;` shape. Specifically:

Replace the `match outcome { ... }` block (lines 247-259) and step 8/9 (lines 261-291) with:

```rust
    let result: Result<LoopRunResult, String> = (async {
        match outcome {
            Err(_elapsed) => Err(format!("Sub-agent timed out after {}s", req.timeout_secs)),
            Ok(Err(panic_payload)) => {
                let msg = panic_message(&panic_payload);
                Err(format!("sub-agent panicked: {msg}"))
            }
            Ok(Ok(Err(e))) => Err(format!("sub-agent failed: {e}")),
            Ok(Ok(Ok(()))) => {
                // 8. Query the harness directly for hit_limit.
                let hit_limit = harness.hit_limit();
                let result = extract_run_result(
                    base.session.as_ref(),
                    &child_id,
                    &child_chain,
                    hit_limit,
                )
                .await?;

                // 9. Spec 1 G2 — fire-and-forget Delegation emit.
                if let Some(writer) = base.raw_memory_writer.clone() {
                    let summary = result.final_text.clone().unwrap_or_default();
                    let parent_id = base
                        .parent_agent_id
                        .clone()
                        .unwrap_or_else(|| "default".to_string());
                    crate::a2a::sub_agent::emit_delegation_primitives(
                        writer,
                        req.task.to_string(),
                        summary,
                        parent_id,
                        base.parent_session_id.clone(),
                        req.agent_def.id.clone(),
                        base.capture_registry.clone(),
                    );
                }

                Ok(result)
            }
        }
    })
    .await;

    // Stage C (P1) — release lane permit + clear lane state on every exit
    // path (Ok / Err / panic-rescued).
    if let Some((scheduler, guard)) = lane_guard {
        scheduler
            .on_run_complete(&lane_run_id, crate::scheduler::Lane::Subagent, Some(guard))
            .await;
    }

    result
}
```

This replaces the pattern of multiple early `return Err(...)` with a single tail-return after lane cleanup.

- [ ] **C4.3: Verify build**

```bash
cargo check -p alephcore
```

Expected: clean. If borrow checker complains about `lane_guard` being moved, refactor to use `Option::take` or move the `lane_guard.is_some()` check inside the async block.

- [ ] **C4.4: Run existing subagent tests to verify no regression**

```bash
cargo test -p alephcore --lib agents::subagent_spawner
```

Expected: all green (existing tests pass `lane_scheduler: None` so the new path is skipped).

### Task C5: Integration test — `lane_budget_4_ok_5th_busy`

**Files:**
- Create: `tests/integration/lane_budget.rs`

- [ ] **C5.1: Write the integration test**

```rust
//! Stage C integration: verify lane budget enforcement for subagent spawns.
//! - 4 concurrent subagent spawns succeed.
//! - 5th spawn fails with "subagent lane budget exhausted" error string.
//! - After one completes, the 5th can succeed.

use std::sync::Arc;

use alephcore::scheduler::{Lane, LaneConfig, LaneScheduler};

#[tokio::test]
async fn try_reserve_4_ok_5th_busy_then_recover() {
    let scheduler = Arc::new(LaneScheduler::new(LaneConfig::default()));

    // Reserve all 4 Subagent slots.
    let mut held = vec![];
    for i in 0..4 {
        let guard = scheduler
            .try_reserve(format!("sub-{i}"), Lane::Subagent)
            .await
            .expect("first 4 reserves should succeed");
        held.push(guard);
    }

    // 5th must fail.
    let err = scheduler
        .try_reserve("sub-5".to_string(), Lane::Subagent)
        .await
        .expect_err("5th reserve should fail");
    assert!(
        format!("{err}").contains("budget exhausted"),
        "error message should mention 'budget exhausted': {err}"
    );

    // Release one slot via on_run_complete (simulates spawn exit).
    let released_guard = held.pop().unwrap();
    scheduler
        .on_run_complete("sub-3", Lane::Subagent, Some(released_guard))
        .await;

    // 5th can now succeed.
    let _guard = scheduler
        .try_reserve("sub-5".to_string(), Lane::Subagent)
        .await
        .expect("after release, reserve should succeed");
}
```

- [ ] **C5.2: Run integration test**

```bash
cargo test --test lane_budget
```

Expected: PASS.

### Task C6: Documentation — update `MULTI_AGENT_SYSTEM.md` for Stage C

**Files:**
- Modify: `docs/reference/MULTI_AGENT_SYSTEM.md`

- [ ] **C6.1: Update Lane priority section to reflect Stage C wiring**

Find the section discussing "Lane priority" or "Main > Nested > Subagent > Cron". Replace stale wording with:

> ## Lane Budget Enforcement
>
> Subagent spawns reserve a `Lane::Subagent` permit via
> `LaneScheduler::try_reserve` (added in P1 Stage C). The default Subagent
> capacity is 4 concurrent runs (changed from 8 in P1 Stage C, per the
> "personal AI sweet spot" decision). On exhaustion, the spawner returns
> `ToolError::Execution { name: "subagent", cause: "subagent lane budget
> exhausted (max=4)" }` — the LLM is responsible for retry policy
> (R7 LLM Sovereignty).
>
> The lane scheduler is wired into `AgentRuntime` via
> `with_lane_scheduler`; legacy callers without a scheduler skip lane
> checks (Option semantics).

### Task C7: Commit Stage C

- [ ] **C7.1: Run full lib + integration tests + clippy**

```bash
cargo test -p alephcore --lib scheduler::
cargo test -p alephcore --lib agents::
cargo test --test lane_budget
cargo test --test recursion_guard
cargo test --test subagent_deps_inherit
cargo clippy -p alephcore -- -D warnings
```

Expected: all green.

- [ ] **C7.2: Verify R10 baseline**

```bash
wc -l src/harness/*.rs | tail -1
ls src/harness/*.rs | wc -l
```

Expected: unchanged.

- [ ] **C7.3: Commit Stage C**

```bash
git add src/scheduler/ src/agents/runtime.rs src/agents/subagent_spawner.rs tests/integration/lane_budget.rs docs/reference/MULTI_AGENT_SYSTEM.md
git commit -m "scheduler: wire LaneScheduler into subagent spawn path

Stage C of P1 subagent uplift. Adds fail-fast LaneScheduler::try_reserve
and integrates it into the subagent spawn lifecycle.

- New SchedulerError::LaneBudgetExhausted { lane, max } variant.
- New try_reserve(run_id, lane) -> Result<ScheduleGuard, SchedulerError>
  API; atomic acquire, no queueing.
- Lane::Subagent default max_concurrent changed from 8 to 4 (Q6a).
- AgentRuntime + SpawnerBase gain lane_scheduler: Option<Arc<...>>;
  with_lane_scheduler builder method on AgentRuntime.
- subagent_spawner::spawn reserves on entry, releases on every exit
  path (RAII guard + explicit on_run_complete for state cleanup).
- Free wins: check_recursion_depth (defense layer 3) + record_spawn
  for parent→child accounting via existing RecursionTracker.
- Deleted obsolete TODO at lane_scheduler.rs:113.
- Integration test lane_budget verifies 4-OK-5th-busy-then-recover.

R10: zero src/harness/ changes. Spec
docs/superpowers/specs/2026-05-08-subagent-uplift-p1-design.md § 4."
```

---

## Stage D — Cancellation Propagation Tests + Fix (Commit 4)

**Goal:** Ship 3 integration tests verifying parent CancellationToken propagates to subagent in 3 scenarios. Fix scope discovered via test failures (predicted: 0 fixes if Phase-6 already wired tokens correctly; otherwise spawner-side fix bounded to ≤ 30 lines).

### Task D1: Set up `tests/integration/cancellation_chain.rs` fixture

**Files:**
- Create: `tests/integration/cancellation_chain.rs`

- [ ] **D1.1: Write the test fixture and infrastructure**

```rust
//! Stage D integration: parent CancellationToken → subagent harness
//! cancellation propagation. Verifies three scenarios:
//! 1. Parent cancels while subagent is awaiting an LLM response
//! 2. Parent cancels while subagent is awaiting a tool call
//! 3. Parent's turn_timeout fires, cascading cancel to subagent

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use alephcore::providers::AiProvider;
use alephcore::providers::adapter::{ProviderResponse, RequestPayload};
use alephcore::session::events::{ToolOutput, ToolOutputMetadata};
use alephcore::tools::service::{ToolDefinition, ToolError, ToolService, ToolSource};

/// LLM provider that hangs forever waiting on the cancellation token.
/// Models a long-running LLM call that must honor cancellation.
struct HangingLlmProvider {
    cancel: CancellationToken,
}

#[async_trait]
impl AiProvider for HangingLlmProvider {
    async fn invoke(
        &self,
        _payload: RequestPayload,
    ) -> alephcore::error::Result<ProviderResponse> {
        // Block until cancelled; simulates a long-running LLM stream.
        self.cancel.cancelled().await;
        Err(alephcore::error::Error::Cancelled)
    }
}

/// Tool service whose `execute` blocks until cancelled.
/// Models a long-running tool that honors cancellation.
struct HangingTool {
    cancel: CancellationToken,
}

#[async_trait]
impl ToolService for HangingTool {
    async fn execute(
        &self,
        _name: &str,
        _input: Value,
    ) -> Result<ToolOutput, ToolError> {
        self.cancel.cancelled().await;
        Err(ToolError::Other("cancelled".into()))
    }

    async fn list(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "hanging_tool".into(),
            description: "always hangs until cancelled".into(),
            input_schema: serde_json::json!({}),
            source: ToolSource::Builtin,
            metadata: Default::default(),
        }]
    }

    async fn describe(&self, name: &str) -> Option<ToolDefinition> {
        self.list().await.into_iter().find(|d| d.name == name)
    }

    fn dispatcher_schema(&self) -> Arc<[alephcore::dispatcher::ToolDefinition]> {
        Arc::from([])
    }
}

// Helpers — D2.2 / D3.2 / D4 fill these in concretely (run_subagent_*
// functions below). Each test below requires:
//   1. A SpawnerBase configured with HangingLlmProvider + HangingTool
//   2. A SpawnRequest with simple agent_def
//   3. A way to observe whether the subagent task completed (cancelled
//      vs hung)
// Mock fixture types (FakeSession, FakeTools, FakeSandbox) are reused
// from `tests/integration/subagent_deps_inherit.rs` (Stage A research).
```

**Note for executor:** the trait signatures (`AiProvider::invoke` etc.) may need adjustment to match the actual production trait. Use `cargo check --tests` to surface mismatches; the existing test patterns in `src/agents/subagent_spawner.rs:600-900` are the reference for these mocks.

- [ ] **D1.2: Verify fixture compiles**

```bash
cargo check --tests --test cancellation_chain
```

Expected: clean. If `AiProvider::invoke` signature differs, look up the trait and align.

### Task D2: Test 1 — `parent_cancel_stops_child_at_llm_await`

**Files:**
- Modify: `tests/integration/cancellation_chain.rs`

- [ ] **D2.1: Append Test 1**

```rust
#[tokio::test]
async fn parent_cancel_stops_child_at_llm_await() {
    timeout(Duration::from_secs(5), async {
        let parent_token = CancellationToken::new();
        let child_token = parent_token.child_token();

        // Spawn subagent in background using HangingLlmProvider.
        let task_handle = tokio::spawn({
            let child_token = child_token.clone();
            async move {
                // Build SpawnerBase with HangingLlmProvider + minimal mocks.
                // Run spawn() with cancel=child_token.
                //
                // Executor: fill in using the patterns from
                // src/agents/subagent_spawner.rs:600-900 (`make_base`,
                // `tests` module). The key requirement: spawn() must
                // observe child_token and bail when cancelled.
                run_subagent_with_hanging_llm(child_token).await
            }
        });

        // Let subagent start + reach LLM await.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Cancel from parent side.
        parent_token.cancel();

        // Subagent should terminate within 1s (well under outer 5s timeout).
        let result = task_handle.await.expect("task did not panic");
        // Accept either: explicit Cancelled, or any error mentioning cancel/timeout.
        match result {
            Err(s) => assert!(
                s.contains("cancel") || s.contains("timed out") || s.contains("Cancelled"),
                "expected cancel-related error, got: {s}"
            ),
            Ok(_) => panic!("subagent should not return Ok after cancel"),
        }
    })
    .await
    .expect("subagent did not respond to parent cancel within 5s");
}

async fn run_subagent_with_hanging_llm(
    cancel: CancellationToken,
) -> Result<alephcore::agents::runtime::LoopRunResult, String> {
    // Wire SpawnerBase with HangingLlmProvider + minimal mocks copied from
    // the A5.1 research output. The structure mirrors the test fixture in
    // src/agents/subagent_spawner.rs:600-750. Concretely:
    //   1. session = Arc<FakeSession> (or whatever the existing fixture uses)
    //   2. parent_tools = Arc<FakeTools>
    //   3. sandbox = Arc<FakeSandbox>
    //   4. provider = Arc::new(HangingLlmProvider { cancel: cancel.clone() })
    //   5. SpawnerBase with all 5 P1 fields = None (default), lane_scheduler = None
    //   6. SpawnRequest { agent_def: simple SubAgent def, task, cancel }
    //   7. spawn(&base, req).await
    //
    // See A5.3 for the SpawnerBase literal pattern with the new fields.
    use alephcore::agents::subagent_spawner::{spawn, SpawnRequest, SpawnerBase};
    use alephcore::agents::{AgentDef, AgentMode};
    use alephcore::harness::chain_context::ChainContext;

    let provider = Arc::new(HangingLlmProvider { cancel: cancel.clone() });
    // Mocks copied from A5.1 research output (FakeSession, FakeTools, FakeSandbox).
    let base = SpawnerBase {
        session: /* Arc<FakeSession> from A5 mocks */,
        parent_tools: /* Arc<FakeTools> from A5 mocks */,
        sandbox: /* Arc<FakeSandbox> from A5 mocks */,
        provider,
        chain: ChainContext::default(),
        raw_memory_writer: None,
        capture_registry: None,
        parent_agent_id: None,
        parent_session_id: None,
        guardrails: None,
        fallback_llm: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        trace_sink: None,
        lane_scheduler: None,
    };
    let agent_def = AgentDef::new("test_subagent", AgentMode::SubAgent);
    let req = SpawnRequest {
        agent_def: &agent_def,
        task: "hang on llm",
        context_summary: None,
        model: None,
        timeout_secs: 30, // outer test timeout is 5s, this should never trigger
        cancel,
    };
    spawn(&base, req).await
}
```

- [ ] **D2.2: Substitute concrete mock instances into the helper above**

The `run_subagent_with_hanging_llm` body in D2.1 has 3 placeholder comments (`/* Arc<FakeSession> from A5 mocks */` etc.). Replace them with the actual mock instances copied from the A5.1 research output. After substitution:

```bash
cargo check --tests --test cancellation_chain
```

Expected: clean compile. If the mock signatures don't match the production traits (e.g., `SessionService::attach` returns a different `Result` type), fix the mock to match by reading the actual trait at the location reported by the error.

The test passes when `spawn()` returns within 1s of `parent_token.cancel()`. Currently it returns `Err(string)` on cancel — accept any cancel-shaped error (the test's `match` block already does this).

- [ ] **D2.3: Run Test 1**

```bash
cargo test --test cancellation_chain parent_cancel_stops_child_at_llm_await
```

Expected: PASS within 5s, OR FAIL (revealing fix needed). If FAIL: see Task D5 for fix scope.

### Task D3: Test 2 — `parent_cancel_stops_child_at_tool_await`

**Files:**
- Modify: `tests/integration/cancellation_chain.rs`

- [ ] **D3.1: Append Test 2**

Same shape as Test 1, but the subagent's loop reaches a `HangingTool::execute` await rather than an LLM await. Use a non-hanging LLM provider (returns a tool-call response immediately) + `HangingTool` for the tool service. After tool call begins, parent cancels.

```rust
#[tokio::test]
async fn parent_cancel_stops_child_at_tool_await() {
    timeout(Duration::from_secs(5), async {
        let parent_token = CancellationToken::new();
        let child_token = parent_token.child_token();

        let task_handle = tokio::spawn({
            let child_token = child_token.clone();
            async move {
                run_subagent_with_hanging_tool(child_token).await
            }
        });

        tokio::time::sleep(Duration::from_millis(100)).await;
        parent_token.cancel();

        let result = task_handle.await.expect("task did not panic");
        match result {
            Err(s) => assert!(
                s.contains("cancel") || s.contains("timed out") || s.contains("Cancelled"),
                "expected cancel-related error, got: {s}"
            ),
            Ok(_) => panic!("subagent should not return Ok after cancel"),
        }
    })
    .await
    .expect("subagent did not respond to parent cancel within 5s");
}

async fn run_subagent_with_hanging_tool(
    cancel: CancellationToken,
) -> Result<alephcore::agents::runtime::LoopRunResult, String> {
    // SpawnerBase with a fast-response LLM (returns one tool_use block
    // requesting `hanging_tool`) + HangingTool as the tool service. The
    // subagent loop calls the tool and hangs until cancel fires.
    //
    // The fast-response LLM is a minimal mock: invoke() returns a
    // ProviderResponse containing one tool_use block. After the harness
    // dispatches it via tools, the tool blocks on cancel.cancelled().
    use alephcore::agents::subagent_spawner::{spawn, SpawnRequest, SpawnerBase};
    use alephcore::agents::{AgentDef, AgentMode};
    use alephcore::harness::chain_context::ChainContext;

    // Inline the fast-response LLM mock here (parallel to HangingLlmProvider
    // in D1.1; the only difference is invoke() returns one tool_use block
    // for "hanging_tool" instead of hanging).
    struct FastToolCallProvider;
    #[async_trait]
    impl AiProvider for FastToolCallProvider {
        async fn invoke(
            &self,
            _payload: RequestPayload,
        ) -> alephcore::error::Result<ProviderResponse> {
            // Construct a ProviderResponse with one tool_use block.
            // The exact ProviderResponse shape is in
            // src/providers/adapter.rs — adapt to match.
            // For the test, the call signature is the contract; the
            // executor adapts the literal to match the actual struct.
            unimplemented!(
                "Construct ProviderResponse with one tool_use for 'hanging_tool'.\n\
                 See src/providers/adapter.rs for ProviderResponse fields.\n\
                 Pattern: ProviderResponse {{ blocks: vec![ToolUse {{ name: 'hanging_tool', ... }}], ... }}"
            )
        }
    }

    let provider = Arc::new(FastToolCallProvider);
    let tools = Arc::new(HangingTool { cancel: cancel.clone() });
    let base = SpawnerBase {
        session: /* Arc<FakeSession> from A5 mocks */,
        parent_tools: tools,
        sandbox: /* Arc<FakeSandbox> from A5 mocks */,
        provider,
        chain: ChainContext::default(),
        raw_memory_writer: None,
        capture_registry: None,
        parent_agent_id: None,
        parent_session_id: None,
        guardrails: None,
        fallback_llm: None,
        stall_config: None,
        consecutive_failure_cap: None,
        turn_timeout: None,
        trace_sink: None,
        lane_scheduler: None,
    };
    let agent_def = AgentDef::new("test_subagent", AgentMode::SubAgent);
    let req = SpawnRequest {
        agent_def: &agent_def,
        task: "trigger hanging tool",
        context_summary: None,
        model: None,
        timeout_secs: 30,
        cancel,
    };
    spawn(&base, req).await
}
```

**Note for executor:** the `unimplemented!()` in `FastToolCallProvider::invoke` requires reading the actual `ProviderResponse` struct in `src/providers/adapter.rs` to construct a literal that compiles. This is a discrete research step — once you have the struct fields, the constructor is mechanical.

- [ ] **D3.2: Implement `run_subagent_with_hanging_tool`**

Mock LLM provider returns a single tool-call response (`hanging_tool`). Tool service is `HangingTool`. Compose into SpawnerBase. Call `spawn()`.

- [ ] **D3.3: Run Test 2**

```bash
cargo test --test cancellation_chain parent_cancel_stops_child_at_tool_await
```

Expected: PASS within 5s, OR FAIL.

### Task D4: Test 3 — `parent_turn_timeout_cascades_to_child`

**Files:**
- Modify: `tests/integration/cancellation_chain.rs`

- [ ] **D4.1: Append Test 3**

This test verifies that when the parent's HarnessDeps has `turn_timeout = Some(1s)` and the parent's loop times out, the subagent it spawned also terminates.

```rust
#[tokio::test]
async fn parent_turn_timeout_cascades_to_child() {
    timeout(Duration::from_secs(5), async {
        // Parent harness with 1s turn_timeout. Parent spawns long-running
        // subagent (HangingLlmProvider). Parent's turn_timeout fires, parent
        // cancels its own work, parent's CancellationToken cancels the
        // subagent.
        //
        // The cleanest realization: instead of running a real parent
        // harness loop, we directly construct a CancellationToken + a 1s
        // tokio::time::sleep that calls token.cancel() on elapsed. This
        // models the parent's turn_timeout firing.

        let parent_token = CancellationToken::new();
        let child_token = parent_token.child_token();

        // Parent's "turn_timeout" — fires after 1s, cancels the parent
        // token, which propagates to the child via the child_token chain.
        let _timeout_handle = {
            let parent_token = parent_token.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(1)).await;
                parent_token.cancel();
            })
        };

        let task_handle = tokio::spawn({
            let child_token = child_token.clone();
            async move {
                run_subagent_with_hanging_llm(child_token).await
            }
        });

        let result = task_handle.await.expect("task did not panic");
        match result {
            Err(s) => assert!(
                s.contains("cancel") || s.contains("timed out") || s.contains("Cancelled"),
                "expected cancel-related error, got: {s}"
            ),
            Ok(_) => panic!("subagent should not return Ok after timeout cascade"),
        }
    })
    .await
    .expect("subagent did not respond to parent turn_timeout within 5s");
}
```

- [ ] **D4.2: Run Test 3**

```bash
cargo test --test cancellation_chain parent_turn_timeout_cascades_to_child
```

Expected: PASS within 5s, OR FAIL.

### Task D5: Apply fix if any test fails

**Files (predicted, choose based on which tests failed):**
- Most likely: `src/agents/subagent_spawner.rs` (token plumbing)
- Worst case (R10 risk): `src/harness/agent.rs` — STOP and add R10 line audit before editing

- [ ] **D5.1: If all 3 tests pass, skip to D6**

If the test suite at the end of D4.2 is fully green:
- Fix scope = 0
- The 3 tests serve as regression locks
- Move to commit

- [ ] **D5.2: If Test 1 fails (LLM await cancellation): fix in spawner**

Most likely cause: `req.cancel` is passed to `harness_for_run.run(&sid, &mut cb, &cancel)` (line 243), but the harness internals don't `select!` on it during LLM streaming.

**First, verify the spawner-side wiring is correct:**

```bash
grep -n "cancel\|cancellation_token\|CancellationToken" src/agents/subagent_spawner.rs
```

The token IS passed to `harness.run`. So if the test fails, the fix lives inside `src/harness/agent.rs` (R10 risk).

**Before editing `src/harness/agent.rs`:**

```bash
wc -l src/harness/*.rs | tail -1
```

Confirm baseline ≤ 1500. If editing puts it over, the entire P1 PR is blocked — surface to human review.

If under budget: locate the LLM await path inside `agent.rs` and wrap it with `tokio::select! { _ = cancel.cancelled() => return Err(...), x = llm.invoke(...) => x }`. This is a structural correctness fix, not a cognition layer (laneness 5-no preserved).

- [ ] **D5.3: If Test 2 fails (tool await cancellation): same audit, fix at the tool-call await site**

- [ ] **D5.4: If Test 3 fails (turn_timeout cascade): verify HarnessDeps.turn_timeout is honored AND its expiration triggers a cancel on the harness's CancellationToken**

Fix is most likely in `agent.rs` if the harness owns its own timer separate from the cancel token. This is the deepest R10 risk path — pause and consult human.

- [ ] **D5.5: After any fix, re-run all 3 tests**

```bash
cargo test --test cancellation_chain
```

Expected: all PASS within 5s.

### Task D6: Commit Stage D

- [ ] **D6.1: Final full test run**

```bash
cargo test -p alephcore --lib
cargo test --test cancellation_chain
cargo test --test recursion_guard
cargo test --test subagent_deps_inherit
cargo test --test lane_budget
cargo clippy -p alephcore -- -D warnings
```

Expected: all green.

- [ ] **D6.2: Verify R10 baseline**

```bash
wc -l src/harness/*.rs | tail -1
ls src/harness/*.rs | wc -l
```

Expected: ≤ 1500, exactly 9 files. If a fix in D5 increased lines, ensure still ≤ 1500.

- [ ] **D6.3: Commit Stage D**

```bash
git add tests/integration/cancellation_chain.rs
# If D5 produced a fix, also add the touched file (e.g., src/agents/subagent_spawner.rs or src/harness/agent.rs).
git commit -m "tests: add cancellation propagation tests for parent→subagent

Stage D of P1 subagent uplift. Three integration tests verify parent
CancellationToken reaches the subagent harness:

1. parent_cancel_stops_child_at_llm_await — parent cancels while child
   is awaiting LLM response
2. parent_cancel_stops_child_at_tool_await — parent cancels while child
   is awaiting tool call
3. parent_turn_timeout_cascades_to_child — parent's turn_timeout fires,
   propagates cancel to child

All tests wrap with tokio::time::timeout(Duration::from_secs(5)) so
cancel-leak regressions surface as test timeouts, not infinite hangs.

[If D5 fix shipped: add summary lines describing what was fixed and why
it was needed.]

Spec docs/superpowers/specs/2026-05-08-subagent-uplift-p1-design.md § 5."
```

---

## Final PR Verification

- [ ] **PR-1: Verify all 4 commits**

```bash
git log --oneline -5
```

Expected: 4 new commits on top of the previous HEAD (`15a22f008` was the P1 spec commit). Stage A → B → C → D in order.

- [ ] **PR-2: Verify R10 hard checks one final time**

```bash
wc -l src/harness/*.rs | tail -1
ls src/harness/*.rs | wc -l
```

Expected: total ≤ 1500, exactly 9 files.

- [ ] **PR-3: Verify total diff size**

```bash
git diff --stat 15a22f008 HEAD
```

Expected: total insertions ~625 lines (roadmap budget). Acceptable up to ~750 (10–20% overrun); higher signals scope drift — pause and review.

- [ ] **PR-4: Run the full test suite + clippy + build release**

```bash
cargo test -p alephcore --lib
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo build --release
```

Expected: all green.

- [ ] **PR-5: Append `✅ Shipped` to roadmap entries**

Open `docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md`. For each Stage A/B/C/D entry, change:
```
**Status**: 📋 Planned · plan: TBD（P1 phase 时认领）
```
To:
```
**Status**: ✅ Shipped: <commit hash> on 2026-05-08
```

Also append at file head, after the YAML frontmatter:
```
✅ P1 Shipped: <PR url> on 2026-05-08
```

Open `docs/superpowers/specs/2026-05-08-subagent-uplift-p1-design.md` (P1 spec). For each `**Status**: 📋 Planned · plan: TBD` entry on Stages A/B/C/D, change to:
```
**Status**: ✅ Shipped: <commit hash>
```

- [ ] **PR-6: Push branch + open PR**

```bash
git push -u origin main  # if working directly on main per CLAUDE.md branch policy
# OR: git checkout -b subagent-uplift-p1 && git push -u origin subagent-uplift-p1
```

Then `gh pr create` per project convention. PR title: `feat(agents): subagent uplift P1 — debt repayment (Stage A/B/C/D)`. PR body should reference both spec files and summarize the 4 commits.

---

## Risk Tracker

If you hit any of these during implementation, document inline + escalate:

| Risk | Trigger | Action |
|------|---------|--------|
| Stage A: SpawnerBase needs `Config` directly (not just precomputed fields) | A3 reveals builder needs Config at spawn time | Add `harness_config: Arc<Config>` to SpawnerBase; mark in commit msg |
| Stage C: `Lane::Subagent` default 8→4 breaks scheduler integration tests beyond what C2 covers | C2.3 finds extra failures | Update each affected test; record locations in risk log |
| Stage D: fix lands in `src/harness/agent.rs` and pushes file > existing budget | D5.2-4 R10 audit fails | STOP. Surface to human. Do not commit. Consider deferring D fix to a follow-up PR with explicit R10 review. |
| Stage A: subagent_inherits_5_fields test requires touching `src/harness/` for an accessor | A5.2 hits architectural friction | Use structural test (assert SpawnerBase fields directly, document the assumption) rather than HarnessDeps probe |
| `cancellation_chain.rs` fixture types diverge from real provider/tool traits | D1.2 build error | Adapt mocks to actual trait signatures via `cargo check --tests` iteration |
