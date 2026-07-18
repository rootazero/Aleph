# Harness Stage 5a — Guardrails Pipeline (Input + Output) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the first two of three Guardrail trait surfaces (Input + Output) into `AgentHarness`, with at least one real `PiiSecretsGuardrail` impl that wraps the existing `PiiEngine` + `SecretLeakDetector` modules. ToolCall callsite + `on_model_fallback` wiring deferred to Stage 5b.

**Architecture:** Add `src/guardrails/` (a new top-level module, not under `src/harness/` per master spec § 0.4 R10 budget). Three traits (`InputGuardrail` / `OutputGuardrail` / `ToolCallGuardrail`) returning `GuardrailDecision { Allow, Sanitize(Replacement), Block(ErrorClass), Warn }`. `GuardrailRegistry` holds three `Vec<Arc<dyn ...>>` sorted by priority + an `AtomicBool` kill-switch. `AgentHarness` consults the registry at two callsites in 5a: input guardrail at `agent.rs:147` (after fetching the session log, before `prompt_builder.assemble()`) and output guardrail at `agent.rs:246` (after `text_content()`, before `emit_event(AssistantMessage)`).

**Tech Stack:** Rust 1.x, `async_trait`, `tokio`, existing `PiiEngine` (src/pii/engine.rs), existing `SecretLeakDetector` (src/secrets/leak_detector.rs), existing `ContentSanitizer` (src/security/content_sanitizer.rs). No new third-party deps.

**Master spec reference:** `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md` § Stage 5 (lines 264-313). Risk class: high. Single-PR cap ≤ 600 lines (estimate ~580). Per-stage `harness/` delta cap ≤ +400 lines (estimate ~+90 net to harness — adds 1 field on `HarnessDeps`, 2 callsite blocks in `agent.rs`, 1 test file).

**Baseline commit:** `e3ca255c6` (post-Stage 4 ship). All 61 harness tests + 6 chain tests green; pre-existing `spawn_tool_allowlist_enforced_via_harness` flake confirmed orthogonal.

---

## File Structure

| File | Action | Responsibility |
|---|---|---|
| `src/guardrails/mod.rs` | Create | Module entry; re-exports |
| `src/guardrails/decision.rs` | Create | `GuardrailDecision` enum + `Replacement` struct + `BlockReason` |
| `src/guardrails/traits.rs` | Create | `InputGuardrail` / `OutputGuardrail` / `ToolCallGuardrail` traits |
| `src/guardrails/registry.rs` | Create | `GuardrailRegistry` + `GuardrailRegistryBuilder` + `disable_all()` kill-switch |
| `src/guardrails/pii_secrets.rs` | Create | `PiiSecretsGuardrail` impl wrapping `PiiEngine` + `SecretLeakDetector` |
| `src/guardrails/tests/mod.rs` | Create | Test module index |
| `src/guardrails/tests/input.rs` | Create | Stage 5a input-callsite integration test |
| `src/guardrails/tests/output.rs` | Create | Stage 5a output-callsite integration test |
| `src/guardrails/tests/registry.rs` | Create | Registry semantics + `disable_all` test |
| `src/guardrails/tests/loom.rs` | Create | `cfg(loom)` concurrent reader test |
| `src/guardrails/tests/bench.rs` | Create | Noop perf observation (release-only) |
| `src/lib.rs` | Modify | Add `pub mod guardrails;` |
| `src/harness/deps.rs` | Modify | Add `pub guardrails: Option<Arc<GuardrailRegistry>>` field |
| `src/harness/agent.rs` | Modify | 2 callsites (input @ line ~147, output @ line ~246) |
| `src/agents/subagent_spawner.rs` | Modify | Construction site: add `guardrails: deps.guardrails.clone(),` |
| `src/orchestrator/harness_bridge.rs` | Modify | Construction site: `guardrails: None,` |
| `src/harness/tests/{driver,think,act,stability,task10_wiring,chain}.rs` | Modify | All test fixtures: add `guardrails: None,` |
| `tests/harness_run_e2e.rs` | Modify | Test fixture: add `guardrails: None,` |
| `CHANGELOG.md` | Modify | Append Stage 5a entry under `## [Unreleased]` |
| `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md` | Modify | Stage 5 status → 🟡 5a Shipped, 5b Pending |

---

## Acceptance Criteria (Stage 5a slice of master spec § Stage 5)

- ✅ `trait InputGuardrail` + `trait OutputGuardrail` + `trait ToolCallGuardrail` defined (ToolCall trait shipped in 5a even though callsite waits for 5b — keeps the API surface coherent)
- ✅ `GuardrailDecision { Allow, Sanitize, Block(ErrorClass), Warn }` reuses Stage 1 `ErrorClass`
- ✅ `GuardrailRegistry` registered via `HarnessDeps`; `disable_all()` runtime kill-switch works
- ✅ ≥1 real `PiiSecretsGuardrail` impl wrapping `PiiEngine` + `SecretLeakDetector`
- ✅ Input callsite wired in `agent.rs` — sensitive data in latest `UserMessage` triggers `Sanitize` (text rewrite) or `Block` (forces `Done` + `on_safety_block` callback)
- ✅ Output callsite wired in `agent.rs` — sensitive data in `response.text` triggers `Sanitize` (text rewrite, blocks rewritten) or `Block` (forces `HarnessError::Llm(...)` with `ErrorClass::Fixable`)
- ✅ Noop path (registry `None` or empty) is zero-await, zero-clone, zero-allocation in steady state
- ✅ ≥1 input integration test, ≥1 output integration test, ≥1 registry+disable test, ≥1 loom test (cfg(loom)), ≥1 noop perf observation
- ✅ ToolCall callsite wired but trait surface present (defers to 5b for full impl + tests)
- ✅ Rollback: dropping `guardrails: Some(...)` to `None` reverts behaviour 1:1

**Out of scope for 5a (deferred to 5b):**
- ToolCall callsite in `agent.rs::act` before `tools.execute(...)`
- `on_model_fallback` callback wiring to ProviderRegistry
- Fallback integration test
- ToolCall integration test

---

## Task 1 — Module skeleton: decision.rs + traits.rs + mod.rs

**Files:**
- Create: `src/guardrails/mod.rs`
- Create: `src/guardrails/decision.rs`
- Create: `src/guardrails/traits.rs`
- Modify: `src/lib.rs` (add `pub mod guardrails;`)

**Goal:** Establish the trait surface so `HarnessDeps` can refer to `Arc<GuardrailRegistry>` in Task 2.

### Step 1.1 — `decision.rs`: GuardrailDecision enum + Replacement

```rust
//! Guardrail decisions returned by Input/Output/ToolCall trait methods.
//!
//! Decisions reuse Stage 1 `ErrorClass` (src/error.rs) so all rejection modes
//! share the same retry / fixable / unexpected vocabulary as the rest of the
//! harness.

use crate::error::ErrorClass;

/// Outcome of a guardrail evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardrailDecision {
    /// Content is fine — pass through unchanged.
    Allow,
    /// Replace the content (e.g. PII redaction). Caller MUST swap in
    /// `replacement.text` before continuing.
    Sanitize(Replacement),
    /// Reject and abort. `class` tells the orchestrator how to propagate:
    /// `Fixable` feeds back into the model, `Unexpected` aborts the session.
    Block { reason: String, class: ErrorClass },
    /// Allow but record the warning (no caller-visible mutation).
    Warn { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    pub text: String,
    /// Human-readable label of the rule that fired (used in audit + tracing).
    pub source: String,
}

impl GuardrailDecision {
    pub fn is_block(&self) -> bool {
        matches!(self, GuardrailDecision::Block { .. })
    }
    pub fn is_allow(&self) -> bool {
        matches!(self, GuardrailDecision::Allow)
    }
}
```

### Step 1.2 — `traits.rs`: three traits

```rust
//! The three Guardrail trait surfaces: Input (turn entry), Output (turn exit),
//! ToolCall (per dispatch).
//!
//! All three are `Send + Sync + 'static` and `async_trait` to permit IO-bound
//! impls (e.g. external classifier service). Stage 5a ships the trait surface;
//! Stage 5b wires ToolCall into `agent.rs::act`.

use async_trait::async_trait;
use serde_json::Value;

use crate::guardrails::decision::GuardrailDecision;

/// Inspects user-provided input before it enters the LLM request.
#[async_trait]
pub trait InputGuardrail: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn evaluate_input(&self, text: &str) -> GuardrailDecision;
}

/// Inspects model output before it is persisted / streamed to channel.
#[async_trait]
pub trait OutputGuardrail: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn evaluate_output(&self, text: &str) -> GuardrailDecision;
}

/// Inspects each tool dispatch before `ToolService::execute(...)`.
/// Stage 5a defines the trait; Stage 5b wires the callsite.
#[async_trait]
pub trait ToolCallGuardrail: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn evaluate_tool_call(&self, tool_name: &str, args: &Value) -> GuardrailDecision;
}
```

### Step 1.3 — `mod.rs`: re-exports + tests submodule

```rust
//! Stage 5 — Guardrails Pipeline (#9).
//!
//! Three trait surfaces (`InputGuardrail`, `OutputGuardrail`,
//! `ToolCallGuardrail`) consulted by `AgentHarness` at three callsites
//! (turn entry, model output, tool dispatch). Decisions reuse Stage 1
//! `ErrorClass` so block reasons share the harness-wide retry vocabulary.
//!
//! Stage 5a ships Input + Output + Registry + PiiSecretsGuardrail.
//! Stage 5b wires ToolCall + on_model_fallback.

pub mod decision;
pub mod pii_secrets;
pub mod registry;
pub mod traits;

pub use decision::{GuardrailDecision, Replacement};
pub use pii_secrets::PiiSecretsGuardrail;
pub use registry::{GuardrailRegistry, GuardrailRegistryBuilder};
pub use traits::{InputGuardrail, OutputGuardrail, ToolCallGuardrail};

#[cfg(test)]
mod tests {
    mod bench;
    mod input;
    mod loom;
    mod output;
    mod registry;
}
```

### Step 1.4 — Wire into `src/lib.rs`

Locate the existing `pub mod ...;` block in `src/lib.rs` and add `pub mod guardrails;` alphabetically.

- [ ] **Verify:** `cargo check -p alephcore` compiles. New module is not yet consumed; clippy warns about dead code — that's fine, will be quieted in Task 4 once HarnessDeps wires it.

---

## Task 2 — Registry: GuardrailRegistry + disable_all kill-switch

**Files:**
- Create: `src/guardrails/registry.rs`

**Goal:** Holds three `Vec<Arc<dyn ...>>` sorted by registration order, plus a `AtomicBool` runtime kill-switch.

### Step 2.1 — Implementation

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::Value;

use crate::error::ErrorClass;
use crate::guardrails::decision::GuardrailDecision;
use crate::guardrails::traits::{InputGuardrail, OutputGuardrail, ToolCallGuardrail};

/// Aggregated registry of all three guardrail surfaces. Constructed once at
/// startup and held inside `HarnessDeps` as `Option<Arc<GuardrailRegistry>>`.
///
/// `disable_all()` flips an `AtomicBool` so every evaluation short-circuits
/// to `GuardrailDecision::Allow`. This is the high-risk rollback knob from
/// master spec § Stage 5 acceptance.
pub struct GuardrailRegistry {
    input: Vec<Arc<dyn InputGuardrail>>,
    output: Vec<Arc<dyn OutputGuardrail>>,
    tool_call: Vec<Arc<dyn ToolCallGuardrail>>,
    enabled: AtomicBool,
}

impl GuardrailRegistry {
    pub fn builder() -> GuardrailRegistryBuilder {
        GuardrailRegistryBuilder::default()
    }

    pub fn empty() -> Self {
        Self {
            input: Vec::new(),
            output: Vec::new(),
            tool_call: Vec::new(),
            enabled: AtomicBool::new(true),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// Runtime kill-switch — flips `enabled` to false. All three `evaluate_*`
    /// methods short-circuit to `Allow` until `enable_all()` is called.
    pub fn disable_all(&self) {
        self.enabled.store(false, Ordering::Release);
    }

    pub fn enable_all(&self) {
        self.enabled.store(true, Ordering::Release);
    }

    pub fn input_count(&self) -> usize { self.input.len() }
    pub fn output_count(&self) -> usize { self.output.len() }
    pub fn tool_call_count(&self) -> usize { self.tool_call.len() }

    /// Sequentially evaluate input guardrails. Stops at first non-Allow.
    /// Returns `Allow` if disabled or if all guardrails allow.
    pub async fn evaluate_input(&self, text: &str) -> GuardrailDecision {
        if !self.is_enabled() {
            return GuardrailDecision::Allow;
        }
        for g in &self.input {
            let d = g.evaluate_input(text).await;
            if !d.is_allow() {
                return d;
            }
        }
        GuardrailDecision::Allow
    }

    pub async fn evaluate_output(&self, text: &str) -> GuardrailDecision {
        if !self.is_enabled() {
            return GuardrailDecision::Allow;
        }
        for g in &self.output {
            let d = g.evaluate_output(text).await;
            if !d.is_allow() {
                return d;
            }
        }
        GuardrailDecision::Allow
    }

    pub async fn evaluate_tool_call(&self, tool_name: &str, args: &Value) -> GuardrailDecision {
        if !self.is_enabled() {
            return GuardrailDecision::Allow;
        }
        for g in &self.tool_call {
            let d = g.evaluate_tool_call(tool_name, args).await;
            if !d.is_allow() {
                return d;
            }
        }
        GuardrailDecision::Allow
    }
}

#[derive(Default)]
pub struct GuardrailRegistryBuilder {
    input: Vec<Arc<dyn InputGuardrail>>,
    output: Vec<Arc<dyn OutputGuardrail>>,
    tool_call: Vec<Arc<dyn ToolCallGuardrail>>,
}

impl GuardrailRegistryBuilder {
    pub fn with_input(mut self, g: Arc<dyn InputGuardrail>) -> Self {
        self.input.push(g);
        self
    }
    pub fn with_output(mut self, g: Arc<dyn OutputGuardrail>) -> Self {
        self.output.push(g);
        self
    }
    pub fn with_tool_call(mut self, g: Arc<dyn ToolCallGuardrail>) -> Self {
        self.tool_call.push(g);
        self
    }
    pub fn build(self) -> GuardrailRegistry {
        GuardrailRegistry {
            input: self.input,
            output: self.output,
            tool_call: self.tool_call,
            enabled: AtomicBool::new(true),
        }
    }
}

// Suppress unused-import warning until Task 4 wires this in
#[cfg(test)]
#[allow(unused)]
fn _assert_send_sync() {
    fn check<T: Send + Sync>() {}
    check::<GuardrailRegistry>();
    check::<Arc<GuardrailRegistry>>();
}

#[cfg(test)]
#[allow(unused)]
const _: () = {
    let _ = ErrorClass::Fixable; // keep import live for re-exports later
};
```

- [ ] **Verify:** `cargo check -p alephcore` clean. `cargo test -p alephcore --lib guardrails::` passes the `_assert_send_sync` smoke.

---

## Task 3 — PiiSecretsGuardrail: real impl wrapping pii + secrets

**Files:**
- Create: `src/guardrails/pii_secrets.rs`

**Goal:** ≥1 real consumer of all three trait surfaces, wrapping the existing `PiiEngine` and `SecretLeakDetector`.

### Step 3.1 — Implementation strategy

`PiiEngine::filter(text)` returns `FilterResult { filtered, detections }` — already does redaction and detection. `SecretLeakDetector::scan_outbound(text)` returns `LeakDecision::{Allow, Block { reason }}`. Compose them:

1. Run `secret_detector.scan_outbound(text)` first — secrets MUST block, never sanitize (a redacted secret is still a leak signal).
2. Run `pii_engine.filter(text)` second — PII may sanitize.

Adapter logic (per trait):
- **Input**: scan inbound user text; secret hit → `Block { Fixable, "user input contains secret" }`; pii detection → `Sanitize { text=filtered, source="pii.{rule}" }`; else `Allow`.
- **Output**: scan outbound model text; secret hit → `Block { Fixable, "model output contained injected secret" }` (this is the canonical bidirectional leak case); pii hit → `Sanitize`; else `Allow`.
- **ToolCall**: serialize args to JSON string; same pipeline. Sanitize replaces the JSON; the harness re-parses (Stage 5b will validate).

```rust
//! Real `PiiSecretsGuardrail` impl wrapping `PiiEngine` + `SecretLeakDetector`.
//!
//! Order of evaluation: secret leak detector first (must Block, never Sanitize),
//! then PII engine (may Sanitize). Both come from existing src/pii and
//! src/secrets modules — this file is a trait adapter, not a re-implementation.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::error::ErrorClass;
use crate::guardrails::decision::{GuardrailDecision, Replacement};
use crate::guardrails::traits::{InputGuardrail, OutputGuardrail, ToolCallGuardrail};
use crate::pii::engine::PiiEngine;
use crate::secrets::leak_detector::{LeakDecision, LeakDetector as SecretLeakDetector};

const NAME: &str = "pii_secrets";

pub struct PiiSecretsGuardrail {
    pii: Option<Arc<RwLock<PiiEngine>>>,
    secrets: Arc<SecretLeakDetector>,
}

impl PiiSecretsGuardrail {
    pub fn new(
        pii: Option<Arc<RwLock<PiiEngine>>>,
        secrets: Arc<SecretLeakDetector>,
    ) -> Self {
        Self { pii, secrets }
    }

    /// Construct a guardrail using the global PII engine snapshot (if any) and
    /// a fresh secret leak detector. Convenience for the boot path.
    pub fn from_globals() -> Self {
        Self::new(PiiEngine::global(), Arc::new(SecretLeakDetector::new()))
    }

    async fn evaluate(&self, text: &str) -> GuardrailDecision {
        // 1. Secret leak detector — never sanitize.
        match self.secrets.scan_outbound(text) {
            LeakDecision::Allow => {}
            LeakDecision::Block { reason } => {
                return GuardrailDecision::Block {
                    reason,
                    class: ErrorClass::Fixable,
                };
            }
        }
        // 2. PII engine — may sanitize.
        if let Some(engine) = self.pii.as_ref() {
            let guard = engine.read().await;
            let result = guard.filter(text);
            if result.has_detections() && result.filtered != text {
                return GuardrailDecision::Sanitize(Replacement {
                    text: result.filtered,
                    source: format!("pii ({} detection(s))", result.detections.len()),
                });
            }
        }
        GuardrailDecision::Allow
    }
}

#[async_trait]
impl InputGuardrail for PiiSecretsGuardrail {
    fn name(&self) -> &str { NAME }
    async fn evaluate_input(&self, text: &str) -> GuardrailDecision {
        self.evaluate(text).await
    }
}

#[async_trait]
impl OutputGuardrail for PiiSecretsGuardrail {
    fn name(&self) -> &str { NAME }
    async fn evaluate_output(&self, text: &str) -> GuardrailDecision {
        self.evaluate(text).await
    }
}

#[async_trait]
impl ToolCallGuardrail for PiiSecretsGuardrail {
    fn name(&self) -> &str { NAME }
    async fn evaluate_tool_call(&self, _tool_name: &str, args: &Value) -> GuardrailDecision {
        let serialized = match serde_json::to_string(args) {
            Ok(s) => s,
            Err(_) => return GuardrailDecision::Allow,
        };
        self.evaluate(&serialized).await
    }
}
```

- [ ] **Verify:** `cargo check -p alephcore` clean.

---

## Task 4 — HarnessDeps + agent.rs callsites

**Files:**
- Modify: `src/harness/deps.rs`
- Modify: `src/harness/agent.rs`

### Step 4.1 — `deps.rs` field

Add after `pub chain_context: ChainContext,` (line ~66):

```rust
    /// Stage 5 seam (#9). Optional registry consulted at three callsites in
    /// `AgentHarness::run_turn_internal`: turn entry (input), model output
    /// emit (output), and tool dispatch (tool-call, Stage 5b). `None` is
    /// equivalent to "no guardrails registered" — zero-cost noop path.
    pub guardrails: Option<Arc<crate::guardrails::GuardrailRegistry>>,
```

### Step 4.2 — Input callsite in `agent.rs:147`

Insert **after** `let events = self.deps.session.get_events(...).await?` and `let tail_start = tail_start_index(&events);`, **before** `let ctx = TurnContext::new(...)`:

```rust
        // Stage 5a: Input guardrail. Evaluate the latest UserMessage in the
        // tail. Block forces an early Done with on_safety_block; Sanitize
        // overwrites the text in the persisted event (single rewrite).
        if let Some(registry) = self.deps.guardrails.as_ref() {
            if let Some(decision) = self.evaluate_input_guardrail(session_id, &events, tail_start, registry).await? {
                match decision {
                    GuardrailControl::Block { reason } => {
                        callback.on_safety_block(&reason);
                        return Ok((TurnState::Done, 0, false));
                    }
                    GuardrailControl::SanitizedReplacement => {
                        // The replacement was already persisted by
                        // `evaluate_input_guardrail`; refetch the events so
                        // the prompt builder sees the rewritten text.
                        let events = self.deps.session.get_events(session_id, None, None).await?;
                        let tail_start = tail_start_index(&events);
                        let ctx = crate::harness::prompt::TurnContext::new(&events, tail_start);
                        // ... continue with refreshed ctx (use a flag, not duplicated control flow)
                    }
                }
            }
        }
```

(Refined control-flow uses a `let mut events_for_ctx = events;` rebind to avoid duplication — see implementation in Task 4.)

### Step 4.3 — Output callsite in `agent.rs:246`

Insert **after** `let text = response.text_content();` and **before** `if !text.is_empty()`:

```rust
        // Stage 5a: Output guardrail. Block returns HarnessError::Llm with
        // ErrorClass::Fixable so the orchestrator can retry. Sanitize
        // rewrites `text` in place; tool_use blocks are left unmodified
        // (Stage 5b's ToolCallGuardrail covers their args).
        let text = if let Some(registry) = self.deps.guardrails.as_ref() {
            match registry.evaluate_output(&text).await {
                GuardrailDecision::Allow | GuardrailDecision::Warn { .. } => text,
                GuardrailDecision::Sanitize(rep) => {
                    callback.on_safety_block(&format!("output sanitized by {}", rep.source));
                    rep.text
                }
                GuardrailDecision::Block { reason, class: _ } => {
                    callback.on_safety_block(&reason);
                    return Err(HarnessError::Llm(crate::error::AlephError::other(
                        format!("output guardrail blocked: {reason}"),
                    )));
                }
            }
        } else {
            text
        };
```

### Step 4.4 — Helper method `evaluate_input_guardrail`

Private helper extracted to `impl AgentHarness` (placed below `evaluate_stop_hooks` near line ~565):

```rust
    /// Stage 5a: input-guardrail evaluation against the latest UserMessage in
    /// the tail. On `Sanitize`, persists a replacement event (`UserMessage`
    /// with rewritten text) and returns `SanitizedReplacement` so the caller
    /// refetches `events`. On `Block`, returns `Block { reason }`.
    async fn evaluate_input_guardrail(
        &self,
        session_id: &SessionId,
        events: &[crate::session::events::EventRecord],
        tail_start: usize,
        registry: &crate::guardrails::GuardrailRegistry,
    ) -> Result<Option<GuardrailControl>, HarnessError> {
        // Find the latest UserMessage in the tail.
        let latest_user_text = events[tail_start..].iter().rev().find_map(|r| {
            if let SessionEvent::UserMessage { content, .. } = &r.event {
                Some(content.text.clone())
            } else {
                None
            }
        });
        let Some(text) = latest_user_text else {
            return Ok(None);
        };
        match registry.evaluate_input(&text).await {
            GuardrailDecision::Allow | GuardrailDecision::Warn { .. } => Ok(None),
            GuardrailDecision::Sanitize(rep) => {
                let new_turn = uuid::Uuid::new_v4();
                let replacement_event = SessionEvent::UserMessage {
                    turn_id: new_turn,
                    content: MessageContent {
                        text: rep.text,
                        blocks: Vec::new(),
                        thinking: None,
                        thinking_signature: None,
                    },
                    at: now_ms(),
                };
                self.deps.session.emit_event(session_id, replacement_event).await?;
                Ok(Some(GuardrailControl::SanitizedReplacement))
            }
            GuardrailDecision::Block { reason, class: _ } => {
                Ok(Some(GuardrailControl::Block { reason }))
            }
        }
    }

    enum GuardrailControl {
        Block { reason: String },
        SanitizedReplacement,
    }
```

(Move `enum GuardrailControl` to file top, not inside `impl`.)

- [ ] **Verify:** `cargo check -p alephcore` clean.

---

## Task 5 — Construction-site updates (HarnessDeps callers)

**Files:**
- Modify: `src/agents/subagent_spawner.rs`
- Modify: `src/orchestrator/harness_bridge.rs`
- Modify: `src/harness/tests/{driver,think,act,stability,task10_wiring,chain}.rs`
- Modify: `src/harness/agent.rs` in-file tests
- Modify: `tests/harness_run_e2e.rs`

**Goal:** Add `guardrails: None,` (or `guardrails: deps.guardrails.clone(),` for spawner) to every `HarnessDeps { ... }` struct literal.

**Lesson from Stage 4:** use `grep -rn "HarnessDeps {" src tests --include="*.rs" -l` to enumerate callers — there are 11 files. Stage 4 had a 19-site count when including in-file struct literals; a follow-up `cargo check` will surface anything missed.

For each site:
- Production sites (subagent_spawner, harness_bridge): pass-through from outer deps where applicable; otherwise `guardrails: None,`
- Test sites: `guardrails: None,`

- [ ] **Verify:** `cargo check -p alephcore` clean across `--lib --tests --bins`.

---

## Task 6 — Tests

**Files:**
- Create: `src/guardrails/tests/{mod,input,output,registry,loom,bench}.rs`

### Test 1 — Registry semantics + disable_all (`tests/registry.rs`)

```rust
// 1. empty registry → Allow on all three surfaces
// 2. registered Block guardrail → returns Block
// 3. disable_all() flips Block to Allow
// 4. enable_all() restores
// 5. multiple guardrails → first non-Allow wins (sequential)
```

### Test 2 — Input integration (`tests/input.rs`)

Wire a fake guardrail that returns `Sanitize` on text containing "SECRET". Drive `AgentHarness` through one turn with a UserMessage like "the password is SECRET". Assert:
- The persisted UserMessage tail event contains the sanitized text
- The model receives the sanitized prompt (assert via captured AiProvider payload)

### Test 3 — Output integration (`tests/output.rs`)

Fake guardrail returns `Block` on output containing "leak". Scripted provider responds with "here is a leak". Assert:
- Harness returns `HarnessError::Llm`
- `on_safety_block` callback fires
- No `AssistantMessage` is persisted (block precedes emit)

### Test 4 — Loom (`tests/loom.rs` with `#[cfg(loom)]`)

```rust
#[cfg(loom)]
#[test]
fn registry_concurrent_evaluate_and_disable() {
    loom::model(|| {
        let r = Arc::new(GuardrailRegistry::empty());
        let r2 = r.clone();
        let t1 = loom::thread::spawn(move || {
            // can race with disable_all
            let _ = futures::executor::block_on(r2.evaluate_input("x"));
        });
        r.disable_all();
        t1.join().unwrap();
    });
}
```

### Test 5 — Noop perf (`tests/bench.rs`, gated `#[cfg(not(debug_assertions))]` or `#[ignore]`)

Loop 10_000 turns through `evaluate_input` on an empty registry. Assert wall-clock < 1 ms (rough sanity, not a strict gate — record observed median into the test output for future regression hunts).

- [ ] **Verify:** `cargo test -p alephcore --lib guardrails::` passes 4 tests (loom and bench gated).

---

## Task 7 — CHANGELOG + master spec status

**Files:**
- Modify: `CHANGELOG.md`
- Modify: `docs/superpowers/specs/2026-05-05-harness-12-module-roadmap-design.md` (Stage 5 entry)

### CHANGELOG entry

```markdown
## [Unreleased]

### Added — Stage 5a (Guardrails Pipeline · Input + Output)

- `src/guardrails/` module with three trait surfaces (`InputGuardrail`,
  `OutputGuardrail`, `ToolCallGuardrail`), `GuardrailRegistry` aggregator,
  and `PiiSecretsGuardrail` real impl wrapping existing PiiEngine +
  SecretLeakDetector.
- `HarnessDeps.guardrails: Option<Arc<GuardrailRegistry>>` field.
- Two new callsites in `AgentHarness::run_turn_internal`: input guardrail
  (turn entry, before prompt assembly) and output guardrail (after model
  text, before AssistantMessage persist).
- Runtime kill-switch `GuardrailRegistry::disable_all()` for high-risk
  rollback per master spec § Stage 5 acceptance.

### Deferred to Stage 5b

- ToolCall callsite in `act()` before `tools.execute`.
- `on_model_fallback` callback wiring to ProviderRegistry fallback list.
```

### Master spec status

Update Stage 5 line:

```
**Status**: 🟡 5a Shipped <commit> on 2026-05-05 · plan: docs/superpowers/specs/2026-05-05-harness-stage5a-guardrails-pipeline-plan.md · 5b Pending
```

- [ ] **Verify:** `git diff CHANGELOG.md` shows new section under Unreleased; master spec shows split status.

---

## Verification — final acceptance walkthrough

- [ ] `cargo check -p alephcore --lib --tests --bins` clean
- [ ] `cargo test -p alephcore --lib guardrails::` ≥4 passing
- [ ] `cargo test -p alephcore --lib harness::` ≥61 passing (Stage 4 baseline)
- [ ] `cargo test -p alephcore --test harness_run_e2e` 2 passing
- [ ] R10 budget: `wc -l src/harness/*.rs src/harness/tests/*.rs` — total within ±100 lines of pre-5a baseline (5870 + ~90 = ~5960)
- [ ] `agent.rs` line count ≤ 1500 (target ~1340 after the two callsites + helper)
- [ ] CHANGELOG + master spec entry committed

---

## Commit chain (target 4 commits, mirrors Stage 4 cadence)

1. `docs: ship Stage 5a plan` (this file + master spec status flip to 🟡 in-flight)
2. `feat(guardrails): module skeleton — traits, decision, registry, PiiSecretsGuardrail` (Tasks 1-3)
3. `feat(harness): wire input/output guardrail callsites` (Tasks 4-5)
4. `test(guardrails): integration + registry + loom + bench` (Task 6) + `docs: flip Stage 5a to ✅ Shipped` (Task 7)

---

## Out of scope (Stage 5b separate plan)

- ToolCall callsite at `agent.rs:404` (before `tools.execute`)
- ToolCall integration test
- `on_model_fallback` ProviderRegistry wiring (requires ProviderRegistry fallback-list audit first)
- Fallback-triggered integration test
- (Possibly) `GuardrailDecision::Sanitize` for ToolCall → re-parse JSON validation
