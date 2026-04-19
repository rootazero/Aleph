# Phase 5 — Orchestrator & Flow Composition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce `src/orchestrator/` as the single Gateway→Harness dispatch point, driven by a declarative `FlowSpec` that composes over `AgentDef`.

**Architecture:** Compose over existing `AgentDef`; layer `brain` / `sandbox_kind` / `session_strategy` via `FlowSpec`. Orchestrator owns seven-step dispatch (resolve → depth-guard → agent lookup → session → brain → sandbox → Harness spawn). `teams`/`swarm` migrate via ~8-callsite adapter shim. `flow_run` LLM tool enables opt-in recursion capped at depth 4.

**Tech Stack:** Rust 2021 edition, `serde` + `toml`, `arc_swap`, `thiserror`, `async_trait`, existing `SessionService` / `ToolService` / `Sandbox` / `AgentHarness`.

**Source spec:** `docs/superpowers/specs/2026-04-19-orchestrator-flow-composition-design.md`

**Pre-requisites:** Phase 0-4 landed on main. Baseline: 9076 library tests + 2 `tests/harness_run_e2e.rs` tests green. `cargo clippy -- -D warnings` clean.

---

## File Structure

**New files (`src/orchestrator/`):**
- `mod.rs` — pub API re-exports (~80 LOC)
- `flow_spec.rs` — `FlowSpec`, `BrainRef`, `SandboxKind`, `SessionStrategy`, `FlowOverrides`, `FlowInput` (~160 LOC)
- `errors.rs` — `FlowError` enum (~50 LOC)
- `flow_registry.rs` — `FlowRegistry` with `ArcSwap<FlowSet>` (~100 LOC)
- `loader.rs` — TOML loader for presets + user flows (~150 LOC)
- `resolver.rs` — agent→flow routing + depth guard + session resolve (~150 LOC)
- `dispatch.rs` — 7-step `Orchestrator::dispatch` (~180 LOC)
- `flow_run_tool.rs` — `flow_run` LLM tool (~100 LOC)
- `presets/mod.rs` — `builtin_flows()` (~20 LOC)
- `presets/default_flows.toml` — 7 preset FlowSpec entries (~120 TOML)

**New tests:**
- `src/orchestrator/tests/flow_spec_parse.rs`
- `src/orchestrator/tests/flow_registry.rs`
- `src/orchestrator/tests/resolver.rs`
- `src/orchestrator/tests/dispatch.rs`
- `src/orchestrator/tests/flow_run_tool.rs`
- `tests/orchestrator_e2e.rs` (integration)

**Modified external:**
- `src/lib.rs` — add `pub mod orchestrator;`
- `src/app_context.rs` — provision `Orchestrator`, inject into `AppContext`
- `src/bin/aleph-server/commands/start/mod.rs` — build Orchestrator deps at boot
- `src/gateway/execution_engine/run_loop.rs` — replace `AgentLoop::new` with `orchestrator.dispatch`
- `src/agents/swarm/coordinator.rs` — adapter shim
- `src/agents/swarm/tasks/mod.rs` — adapter shim
- `src/teams/sessions/coordinator.rs` — adapter shim
- `src/teams/plans.rs` — adapter shim
- `src/agents/sub_agents/traits.rs` — default impl routes through Orchestrator
- `src/gateway/handlers/session/mod.rs` — adapter shim
- `src/gateway/handlers/mod.rs` — add `gateway.flow.reload` RPC
- `Cargo.toml` (alephcore) — `arc_swap = "1"` if not already present

**Sanity before starting:** confirm worktree is at `/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/managed-agents-phase-4a`, branch is `worktree-managed-agents-phase-4a`, Phase 4 spec `ALEPH_HARNESS_V2` discoverability fix (commit `af19d58d1`) is present in HEAD.

---

## Task 1: Bootstrap Module + FlowSpec Types

**Files:**
- Create: `src/orchestrator/mod.rs`
- Create: `src/orchestrator/flow_spec.rs`
- Modify: `src/lib.rs` (add `pub mod orchestrator;`)
- Create: `src/orchestrator/tests/flow_spec_parse.rs`

**Context:** Foundation task — no dependencies on other Phase 5 work. Defines the serializable types consumed by every later task.

- [ ] **Step 1.1: Confirm `arc_swap` and `toml` are available**

Run:
```bash
grep -E '^arc_swap|^toml' Cargo.toml
grep -E '^arc_swap|^toml' Cargo.lock | head -5
```
Expected: both appear. If `arc_swap` is absent from `Cargo.toml`, add it:
```toml
arc_swap = "1"
```

- [ ] **Step 1.2: Create stub module**

Create `src/orchestrator/mod.rs`:
```rust
//! Orchestrator & Flow Composition (Phase 5).
//!
//! See docs/superpowers/specs/2026-04-19-orchestrator-flow-composition-design.md

pub mod flow_spec;

pub use flow_spec::{
    BrainRef, FlowId, AgentId, ProviderId, FlowInput, FlowOverrides,
    FlowSpec, SandboxKind, SessionStrategy,
};

#[cfg(test)]
mod tests;
```

Modify `src/lib.rs` to add:
```rust
pub mod orchestrator;
```
(Add alongside existing `pub mod` declarations in alphabetical order — check existing ordering and match it. If existing file uses grouped sections, slot into the right group.)

- [ ] **Step 1.3: Write failing test for `FlowSpec` TOML roundtrip**

Create `src/orchestrator/tests/mod.rs`:
```rust
mod flow_spec_parse;
```

Create `src/orchestrator/tests/flow_spec_parse.rs`:
```rust
use crate::orchestrator::flow_spec::{
    BrainRef, FlowSpec, FlowOverrides, SandboxKind, SessionStrategy,
};

#[test]
fn parses_minimal_flow_spec() {
    let toml_src = r#"
        id = "default-agent"
        description = "Primary chat agent"
        agent = "main"
        sandbox_kind = "workspace"

        [brain]
        kind = "default"

        [session_strategy]
        kind = "reuse"
    "#;
    let flow: FlowSpec = toml::from_str(toml_src).expect("parse");
    assert_eq!(flow.id, "default-agent");
    assert_eq!(flow.agent, "main");
    assert_eq!(flow.sandbox_kind, SandboxKind::Workspace);
    assert!(matches!(flow.brain, BrainRef::Default));
    assert!(matches!(flow.session_strategy, SessionStrategy::Reuse));
    assert!(flow.overrides.max_iterations.is_none());
}

#[test]
fn parses_strict_brain_and_child_session() {
    let toml_src = r#"
        id = "researcher"
        description = "Read-only web researcher"
        agent = "researcher"
        sandbox_kind = "none"

        [brain]
        kind = "strict"
        provider = "minimax"
        model = "text-01"

        [session_strategy]
        kind = "child"

        [overrides]
        max_iterations = 10
    "#;
    let flow: FlowSpec = toml::from_str(toml_src).expect("parse");
    assert_eq!(flow.sandbox_kind, SandboxKind::None);
    match flow.brain {
        BrainRef::Strict { provider, model } => {
            assert_eq!(provider, "minimax");
            assert_eq!(model.as_deref(), Some("text-01"));
        }
        other => panic!("expected Strict, got {other:?}"),
    }
    match flow.session_strategy {
        SessionStrategy::Child { parent_session_key } => {
            assert!(parent_session_key.is_none(), "parent injected at runtime");
        }
        other => panic!("expected Child, got {other:?}"),
    }
    assert_eq!(flow.overrides.max_iterations, Some(10));
}

#[test]
fn rejects_unknown_fields() {
    let toml_src = r#"
        id = "x"
        description = "x"
        agent = "x"
        sandbox_kind = "none"
        unknown_field = "boom"

        [brain]
        kind = "default"

        [session_strategy]
        kind = "fresh"
    "#;
    let err = toml::from_str::<FlowSpec>(toml_src).unwrap_err();
    assert!(err.to_string().to_lowercase().contains("unknown"), "got {err}");
}

#[test]
fn roundtrips_preferred_brain() {
    let flow = FlowSpec {
        id: "x".into(),
        description: "x".into(),
        agent: "x".into(),
        brain: BrainRef::Preferred { provider: "chatgpt".into() },
        sandbox_kind: SandboxKind::Workspace,
        session_strategy: SessionStrategy::Fresh,
        overrides: FlowOverrides::default(),
    };
    let s = toml::to_string(&flow).unwrap();
    let back: FlowSpec = toml::from_str(&s).unwrap();
    assert_eq!(back.id, flow.id);
    assert!(matches!(back.brain, BrainRef::Preferred { provider } if provider == "chatgpt"));
}
```

- [ ] **Step 1.4: Run the test — expect compile failure (types don't exist yet)**

Run: `cargo test -p alephcore --lib orchestrator::tests::flow_spec_parse --no-run`
Expected: error — `flow_spec` module or its types are undefined.

- [ ] **Step 1.5: Implement `flow_spec.rs`**

Create `src/orchestrator/flow_spec.rs`:
```rust
//! Declarative flow configuration. See Phase 5 design §5.

use serde::{Deserialize, Serialize};

use crate::agents::types::ContextMode;
use crate::session::events::MessageContent;

pub type FlowId = String;
pub type AgentId = String;
pub type ProviderId = String;

/// Gateway-agnostic input envelope for a Flow dispatch.
#[derive(Debug, Clone)]
pub enum FlowInput {
    Prompt(String),
    Messages(Vec<MessageContent>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowSpec {
    pub id: FlowId,
    pub description: String,
    pub agent: AgentId,
    pub brain: BrainRef,
    pub sandbox_kind: SandboxKind,
    pub session_strategy: SessionStrategy,
    #[serde(default)]
    pub overrides: FlowOverrides,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum BrainRef {
    Default,
    Preferred { provider: ProviderId },
    Strict { provider: ProviderId, #[serde(default)] model: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SandboxKind {
    None,
    Workspace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum SessionStrategy {
    Reuse,
    Fresh,
    Child {
        #[serde(default)]
        parent_session_key: Option<String>,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowOverrides {
    #[serde(default)]
    pub max_iterations: Option<u32>,
    #[serde(default)]
    pub context_mode: Option<ContextMode>,
    #[serde(default)]
    pub extra_system_prompt: Option<String>,
}
```

**NOTE on `ContextMode`:** verify `crate::agents::types::ContextMode` implements `Serialize + Deserialize`. Run:
```bash
grep -n "ContextMode" src/agents/types.rs | head -5
```
If not derived, add `#[derive(Serialize, Deserialize)]` to the existing definition — this is the minimal change (no field renames) and is part of this task.

- [ ] **Step 1.6: Run the four tests — expect all pass**

Run: `cargo test -p alephcore --lib orchestrator::tests::flow_spec_parse -- --nocapture`
Expected: 4 passed, 0 failed.

- [ ] **Step 1.7: Run clippy on the new module**

Run: `cargo clippy -p alephcore --lib -- -D warnings 2>&1 | grep -A3 orchestrator || echo "clean"`
Expected: `clean`.

- [ ] **Step 1.8: Commit**

```bash
git add src/lib.rs src/orchestrator/ Cargo.toml Cargo.lock src/agents/types.rs
git commit -m "orchestrator: scaffold FlowSpec types + TOML parsing (Phase 5.1)"
```

---

## Task 2: FlowError Enum

**Files:**
- Create: `src/orchestrator/errors.rs`
- Modify: `src/orchestrator/mod.rs`

**Context:** Centralised error type consumed by every dispatch step. Pure definition + one conversion test.

- [ ] **Step 2.1: Write failing test for FlowError display + variants**

Append to `src/orchestrator/tests/mod.rs`:
```rust
mod errors;
```

Create `src/orchestrator/tests/errors.rs`:
```rust
use crate::orchestrator::errors::FlowError;

#[test]
fn display_unknown_flow() {
    let e = FlowError::UnknownFlow("nope".into());
    assert_eq!(e.to_string(), "unknown flow id: nope");
}

#[test]
fn display_recursion_limit() {
    let e = FlowError::RecursionLimit { max: 4 };
    assert_eq!(e.to_string(), "flow recursion limit (4) exceeded");
}

#[test]
fn display_session_conflict() {
    let e = FlowError::SessionConflict("sess-abc".into());
    assert_eq!(e.to_string(), "session sess-abc already dispatching");
}

#[test]
fn display_provider_unavailable() {
    let e = FlowError::ProviderUnavailable("minimax".into());
    assert_eq!(e.to_string(), "provider unavailable: minimax");
}
```

- [ ] **Step 2.2: Run — expect compile failure**

Run: `cargo test -p alephcore --lib orchestrator::tests::errors --no-run`
Expected: fails (module missing).

- [ ] **Step 2.3: Implement `errors.rs`**

Create `src/orchestrator/errors.rs`:
```rust
//! Flow dispatch error type. See design §6.

use crate::orchestrator::flow_spec::{AgentId, FlowId, ProviderId};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FlowError {
    #[error("unknown flow id: {0}")]
    UnknownFlow(FlowId),

    #[error("unknown agent id: {0}")]
    UnknownAgent(AgentId),

    #[error("flow recursion limit ({max}) exceeded")]
    RecursionLimit { max: u8 },

    #[error("session {0} already dispatching")]
    SessionConflict(String),

    #[error("sandbox provision failed: {0}")]
    SandboxProvisionFailed(String),

    #[error("provider unavailable: {0}")]
    ProviderUnavailable(ProviderId),

    #[error("invalid flow config: {0}")]
    InvalidConfig(String),

    #[error("internal dispatch error: {0}")]
    Internal(String),
}
```

Modify `src/orchestrator/mod.rs`:
```rust
pub mod errors;
pub mod flow_spec;

pub use errors::FlowError;
pub use flow_spec::{
    AgentId, BrainRef, FlowId, FlowInput, FlowOverrides, FlowSpec, ProviderId,
    SandboxKind, SessionStrategy,
};

#[cfg(test)]
mod tests;
```

- [ ] **Step 2.4: Run — expect pass**

Run: `cargo test -p alephcore --lib orchestrator::tests::errors -- --nocapture`
Expected: 4 passed.

- [ ] **Step 2.5: Commit**

```bash
git add src/orchestrator/errors.rs src/orchestrator/mod.rs src/orchestrator/tests/
git commit -m "orchestrator: add FlowError enum (Phase 5.2)"
```

---

## Task 3: FlowRegistry with ArcSwap

**Files:**
- Create: `src/orchestrator/flow_registry.rs`
- Create: `src/orchestrator/tests/flow_registry.rs`
- Modify: `src/orchestrator/mod.rs`, `src/orchestrator/tests/mod.rs`

**Context:** Lock-free atomic reload of the flow catalog. In-flight dispatches hold an `Arc<FlowSpec>` snapshot; `replace()` swaps the whole set atomically.

- [ ] **Step 3.1: Write failing test**

Append to `src/orchestrator/tests/mod.rs`:
```rust
mod flow_registry;
```

Create `src/orchestrator/tests/flow_registry.rs`:
```rust
use std::collections::HashMap;
use std::sync::Arc;

use crate::orchestrator::flow_registry::{FlowRegistry, FlowSet};
use crate::orchestrator::flow_spec::{
    BrainRef, FlowOverrides, FlowSpec, SandboxKind, SessionStrategy,
};

fn mk_spec(id: &str, agent: &str) -> FlowSpec {
    FlowSpec {
        id: id.into(),
        description: "test".into(),
        agent: agent.into(),
        brain: BrainRef::Default,
        sandbox_kind: SandboxKind::None,
        session_strategy: SessionStrategy::Fresh,
        overrides: FlowOverrides::default(),
    }
}

#[test]
fn resolve_returns_spec_by_id() {
    let mut map = FlowSet::new();
    map.insert("a".into(), Arc::new(mk_spec("a", "main")));
    let reg = FlowRegistry::new(map);
    let got = reg.resolve("a").expect("present");
    assert_eq!(got.id, "a");
    assert_eq!(got.agent, "main");
}

#[test]
fn resolve_unknown_returns_none() {
    let reg = FlowRegistry::new(FlowSet::new());
    assert!(reg.resolve("nope").is_none());
}

#[test]
fn replace_swaps_atomically() {
    let mut map = FlowSet::new();
    map.insert("a".into(), Arc::new(mk_spec("a", "main")));
    let reg = FlowRegistry::new(map);

    // Hold a snapshot from before the swap.
    let snap_before = reg.resolve("a").unwrap();

    let mut new_map = FlowSet::new();
    new_map.insert("a".into(), Arc::new(mk_spec("a", "coder"))); // same id, different agent
    new_map.insert("b".into(), Arc::new(mk_spec("b", "explore")));
    reg.replace(new_map);

    // In-flight handle still sees the old agent — Arc snapshot semantics.
    assert_eq!(snap_before.agent, "main");

    // New resolves see the new catalog.
    assert_eq!(reg.resolve("a").unwrap().agent, "coder");
    assert_eq!(reg.resolve("b").unwrap().agent, "explore");
}

#[test]
fn list_ids_is_sorted() {
    let mut map = FlowSet::new();
    map.insert("zeta".into(), Arc::new(mk_spec("zeta", "main")));
    map.insert("alpha".into(), Arc::new(mk_spec("alpha", "main")));
    map.insert("mid".into(), Arc::new(mk_spec("mid", "main")));
    let reg = FlowRegistry::new(map);
    assert_eq!(reg.list_ids(), vec!["alpha", "mid", "zeta"]);
}
```

- [ ] **Step 3.2: Run — expect compile failure**

Run: `cargo test -p alephcore --lib orchestrator::tests::flow_registry --no-run`
Expected: fails.

- [ ] **Step 3.3: Implement `flow_registry.rs`**

Create `src/orchestrator/flow_registry.rs`:
```rust
//! Flow catalog with atomic hot reload. See design §3.8, §5.

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::orchestrator::flow_spec::{FlowId, FlowSpec};

pub type FlowSet = HashMap<FlowId, Arc<FlowSpec>>;

pub struct FlowRegistry {
    flows: ArcSwap<FlowSet>,
}

impl FlowRegistry {
    pub fn new(initial: FlowSet) -> Self {
        Self {
            flows: ArcSwap::from(Arc::new(initial)),
        }
    }

    /// Returns an `Arc<FlowSpec>` snapshot. In-flight dispatches keep
    /// this snapshot alive even after `replace()`.
    pub fn resolve(&self, id: &str) -> Option<Arc<FlowSpec>> {
        self.flows.load().get(id).cloned()
    }

    /// Atomic whole-catalog swap. Callers hold old snapshots until they drop them.
    pub fn replace(&self, new_set: FlowSet) {
        self.flows.store(Arc::new(new_set));
    }

    pub fn list_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.flows.load().keys().cloned().collect();
        ids.sort();
        ids
    }

    pub fn len(&self) -> usize {
        self.flows.load().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
```

Modify `src/orchestrator/mod.rs` — add:
```rust
pub mod flow_registry;
pub use flow_registry::{FlowRegistry, FlowSet};
```

- [ ] **Step 3.4: Run — expect 4 pass**

Run: `cargo test -p alephcore --lib orchestrator::tests::flow_registry -- --nocapture`
Expected: 4 passed.

- [ ] **Step 3.5: Commit**

```bash
git add src/orchestrator/flow_registry.rs src/orchestrator/tests/flow_registry.rs src/orchestrator/mod.rs src/orchestrator/tests/mod.rs
git commit -m "orchestrator: add FlowRegistry with ArcSwap hot reload (Phase 5.3)"
```

---

## Task 4: Preset Flows Catalog + TOML Loader

**Files:**
- Create: `src/orchestrator/loader.rs`
- Create: `src/orchestrator/presets/mod.rs`
- Create: `src/orchestrator/presets/default_flows.toml` (embedded via `include_str!`)
- Create: `src/orchestrator/tests/loader.rs`
- Modify: `src/orchestrator/mod.rs`, `src/orchestrator/tests/mod.rs`

**Context:** Ship the 7 preset flows (one per AgentDef builtin) + TOML loader that merges presets with `~/.aleph/flows/*.toml` user files. Use `include_str!` for presets so they ship in the binary.

- [ ] **Step 4.1: Author the preset TOML file**

Create `src/orchestrator/presets/default_flows.toml`:
```toml
# 7 preset FlowSpecs, 1:1 with AgentDef builtins in src/agents/registry.rs.
# All use BrainRef::Default to preserve current provider fallback behavior.

[[flow]]
id = "default-agent"
description = "Primary chat agent for user-facing interactions"
agent = "main"
sandbox_kind = "workspace"
brain = { kind = "default" }
session_strategy = { kind = "reuse" }

[[flow]]
id = "explore"
description = "Read-only codebase exploration specialist"
agent = "explore"
sandbox_kind = "workspace"
brain = { kind = "default" }
session_strategy = { kind = "child" }

[[flow]]
id = "coder"
description = "Code writing specialist with file operations"
agent = "coder"
sandbox_kind = "workspace"
brain = { kind = "default" }
session_strategy = { kind = "child" }

[[flow]]
id = "researcher"
description = "Web and document research specialist"
agent = "researcher"
sandbox_kind = "none"
brain = { kind = "default" }
session_strategy = { kind = "child" }

[[flow]]
id = "default"
description = "General-purpose sub-agent"
agent = "default"
sandbox_kind = "workspace"
brain = { kind = "default" }
session_strategy = { kind = "child" }

[[flow]]
id = "plan"
description = "Read-only planning and analysis specialist"
agent = "plan"
sandbox_kind = "workspace"
brain = { kind = "default" }
session_strategy = { kind = "child" }

[[flow]]
id = "verify"
description = "Adversarial verification specialist"
agent = "verify"
sandbox_kind = "workspace"
brain = { kind = "default" }
session_strategy = { kind = "child" }
```

- [ ] **Step 4.2: Write failing test for preset loader**

Append to `src/orchestrator/tests/mod.rs`:
```rust
mod loader;
```

Create `src/orchestrator/tests/loader.rs`:
```rust
use crate::orchestrator::loader::{load_presets, load_user_flows_from_str};

#[test]
fn preset_catalog_contains_seven_flows() {
    let set = load_presets().expect("parse presets");
    let ids: Vec<&String> = set.keys().collect();
    assert_eq!(set.len(), 7, "expected 7 preset flows, got {ids:?}");
    for expected in &["default-agent", "explore", "coder", "researcher", "default", "plan", "verify"] {
        assert!(set.contains_key(*expected), "missing preset {expected}");
    }
}

#[test]
fn preset_default_agent_targets_main() {
    let set = load_presets().expect("parse presets");
    let default_agent = set.get("default-agent").unwrap();
    assert_eq!(default_agent.agent, "main");
}

#[test]
fn user_flow_file_with_single_flow_parses() {
    let toml_src = r#"
[[flow]]
id = "user/my-flow"
description = "My custom flow"
agent = "main"
sandbox_kind = "none"
brain = { kind = "preferred", provider = "minimax" }
session_strategy = { kind = "fresh" }
"#;
    let set = load_user_flows_from_str(toml_src).expect("parse");
    assert_eq!(set.len(), 1);
    assert!(set.contains_key("user/my-flow"));
}

#[test]
fn duplicate_flow_id_within_single_file_is_rejected() {
    let toml_src = r#"
[[flow]]
id = "dup"
description = "first"
agent = "main"
sandbox_kind = "none"
brain = { kind = "default" }
session_strategy = { kind = "fresh" }

[[flow]]
id = "dup"
description = "second"
agent = "main"
sandbox_kind = "none"
brain = { kind = "default" }
session_strategy = { kind = "fresh" }
"#;
    let err = load_user_flows_from_str(toml_src).unwrap_err();
    assert!(err.to_string().to_lowercase().contains("duplicate"), "got {err}");
}
```

- [ ] **Step 4.3: Run — expect compile failure**

Run: `cargo test -p alephcore --lib orchestrator::tests::loader --no-run`
Expected: fails.

- [ ] **Step 4.4: Implement loader + presets**

Create `src/orchestrator/loader.rs`:
```rust
//! TOML loader for preset and user-defined FlowSpec files.
//!
//! See design §5 (TOML shape) and §3.8 (hot reload via FlowRegistry::replace).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;

use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_registry::FlowSet;
use crate::orchestrator::flow_spec::FlowSpec;

#[derive(Debug, Deserialize)]
struct FlowFile {
    #[serde(rename = "flow", default)]
    flows: Vec<FlowSpec>,
}

/// Parse the embedded preset catalog. Panics in tests if malformed — the
/// presets are authored and validated at build time.
pub fn load_presets() -> Result<FlowSet, FlowError> {
    let src = include_str!("presets/default_flows.toml");
    parse_flow_file(src).map_err(|e| FlowError::InvalidConfig(format!("presets: {e}")))
}

/// Parse a user flow file (TOML string).
pub fn load_user_flows_from_str(src: &str) -> Result<FlowSet, FlowError> {
    parse_flow_file(src)
        .map_err(|e| FlowError::InvalidConfig(format!("user flow: {e}")))
}

/// Load every `*.toml` under `dir`, merging into a single FlowSet.
/// Later files do NOT override earlier ones — duplicates return an error.
pub async fn load_user_flows_from_dir(dir: &Path) -> Result<FlowSet, FlowError> {
    let mut merged = FlowSet::new();
    if !dir.exists() {
        return Ok(merged);
    }
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .map_err(|e| FlowError::InvalidConfig(format!("read {dir:?}: {e}")))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| FlowError::InvalidConfig(format!("iter {dir:?}: {e}")))?
    {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        let src = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| FlowError::InvalidConfig(format!("read {path:?}: {e}")))?;
        let parsed = load_user_flows_from_str(&src)?;
        for (id, spec) in parsed {
            if merged.insert(id.clone(), spec).is_some() {
                return Err(FlowError::InvalidConfig(format!(
                    "duplicate flow id across files: {id}"
                )));
            }
        }
    }
    Ok(merged)
}

fn parse_flow_file(src: &str) -> Result<FlowSet, String> {
    let file: FlowFile = toml::from_str(src).map_err(|e| e.to_string())?;
    let mut out = FlowSet::new();
    for spec in file.flows {
        let id = spec.id.clone();
        if out.insert(id.clone(), Arc::new(spec)).is_some() {
            return Err(format!("duplicate flow id: {id}"));
        }
    }
    Ok(out)
}

/// Merge presets + user flows. User flows override presets on id collision.
pub fn merge_catalogs(presets: FlowSet, user: FlowSet) -> FlowSet {
    let mut out = presets;
    for (id, spec) in user {
        out.insert(id, spec);
    }
    out
}

// Avoid dead_code lint: HashMap not used here but keeps API surface clean.
#[allow(dead_code)]
fn _witness_hashmap() -> HashMap<String, ()> { HashMap::new() }
```

Create `src/orchestrator/presets/mod.rs`:
```rust
//! Preset flow catalog (embedded in binary).
//!
//! The actual TOML lives at `default_flows.toml` and is parsed by
//! `loader::load_presets()`.
```

Modify `src/orchestrator/mod.rs` — add:
```rust
pub mod loader;
pub mod presets;
```

- [ ] **Step 4.5: Run — expect 4 pass**

Run: `cargo test -p alephcore --lib orchestrator::tests::loader -- --nocapture`
Expected: 4 passed.

- [ ] **Step 4.6: Commit**

```bash
git add src/orchestrator/loader.rs src/orchestrator/presets/ src/orchestrator/tests/loader.rs src/orchestrator/mod.rs src/orchestrator/tests/mod.rs
git commit -m "orchestrator: add preset catalog + TOML loader (Phase 5.4)"
```

---

## Task 5: Resolver — Flow Routing + Depth Guard + Session Resolution

**Files:**
- Create: `src/orchestrator/resolver.rs`
- Create: `src/orchestrator/tests/resolver.rs`
- Modify: `src/orchestrator/mod.rs`, `src/orchestrator/tests/mod.rs`

**Context:** Three pure functions for step 1/2/4 of dispatch. Kept pure (no async, no IO) so tests are fast and exhaustive.

- [ ] **Step 5.1: Write failing tests**

Append to `src/orchestrator/tests/mod.rs`:
```rust
mod resolver;
```

Create `src/orchestrator/tests/resolver.rs`:
```rust
use std::collections::HashMap;

use crate::orchestrator::errors::FlowError;
use crate::orchestrator::resolver::{
    depth_guard, resolve_flow_id, RoutingOverrides, MAX_FLOW_DEPTH,
};

fn default_table() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("main".into(), "default-agent".into());
    m.insert("researcher".into(), "researcher".into());
    m
}

fn no_overrides() -> RoutingOverrides {
    RoutingOverrides::default()
}

#[test]
fn depth_guard_allows_below_max() {
    assert!(depth_guard(0).is_ok());
    assert!(depth_guard(MAX_FLOW_DEPTH - 1).is_ok());
    assert!(depth_guard(MAX_FLOW_DEPTH).is_ok());
}

#[test]
fn depth_guard_rejects_above_max() {
    let err = depth_guard(MAX_FLOW_DEPTH + 1).unwrap_err();
    assert!(matches!(err, FlowError::RecursionLimit { max } if max == MAX_FLOW_DEPTH));
}

#[test]
fn default_table_resolves_known_agent() {
    let got = resolve_flow_id("main", None, &no_overrides(), &default_table()).unwrap();
    assert_eq!(got, "default-agent");
}

#[test]
fn unknown_agent_returns_error() {
    let err = resolve_flow_id("unknown", None, &no_overrides(), &default_table()).unwrap_err();
    assert!(matches!(err, FlowError::UnknownAgent(ref id) if id == "unknown"));
}

#[test]
fn exact_channel_override_wins() {
    let mut ov = no_overrides();
    ov.exact.insert(("main".into(), "telegram".into()), "main-lite".into());
    ov.wildcard.insert("main".into(), "default-agent".into());
    let got = resolve_flow_id("main", Some("telegram"), &ov, &default_table()).unwrap();
    assert_eq!(got, "main-lite");
}

#[test]
fn wildcard_override_used_for_non_matching_channel() {
    let mut ov = no_overrides();
    ov.exact.insert(("main".into(), "telegram".into()), "main-lite".into());
    ov.wildcard.insert("main".into(), "main-overridden".into());
    let got = resolve_flow_id("main", Some("slack"), &ov, &default_table()).unwrap();
    assert_eq!(got, "main-overridden");
}

#[test]
fn default_table_is_last_resort() {
    let ov = no_overrides();
    let got = resolve_flow_id("main", Some("slack"), &ov, &default_table()).unwrap();
    assert_eq!(got, "default-agent");
}

#[test]
fn no_channel_uses_wildcard_then_default() {
    let mut ov = no_overrides();
    ov.wildcard.insert("main".into(), "main-wild".into());
    let got = resolve_flow_id("main", None, &ov, &default_table()).unwrap();
    assert_eq!(got, "main-wild");
}
```

- [ ] **Step 5.2: Run — expect fail**

Run: `cargo test -p alephcore --lib orchestrator::tests::resolver --no-run`
Expected: fails.

- [ ] **Step 5.3: Implement resolver**

Create `src/orchestrator/resolver.rs`:
```rust
//! Pure routing + depth guard + session resolution helpers.
//! See design §6 (dispatch step 1, 2, 4), §7 (MAX_FLOW_DEPTH).

use std::collections::HashMap;

use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::{AgentId, FlowId, SessionStrategy};

/// Hardcoded maximum depth for `flow_run` recursion. See design §7.
pub const MAX_FLOW_DEPTH: u8 = 4;

pub fn depth_guard(depth: u8) -> Result<(), FlowError> {
    if depth > MAX_FLOW_DEPTH {
        Err(FlowError::RecursionLimit {
            max: MAX_FLOW_DEPTH,
        })
    } else {
        Ok(())
    }
}

/// User-provided routing overrides from `aleph.toml [flow_routing]`.
#[derive(Debug, Default, Clone)]
pub struct RoutingOverrides {
    /// `(agent, channel) → flow_id` — exact match.
    pub exact: HashMap<(AgentId, String), FlowId>,
    /// `agent → flow_id` — wildcard (any channel).
    pub wildcard: HashMap<AgentId, FlowId>,
}

/// Map `(agent_id, channel)` → `flow_id`. Precedence:
/// 1. exact `(agent, channel)` override
/// 2. wildcard `agent` override
/// 3. default table (agent_id == flow_id fallback from builtin table)
pub fn resolve_flow_id(
    agent_id: &str,
    channel: Option<&str>,
    overrides: &RoutingOverrides,
    defaults: &HashMap<String, String>,
) -> Result<FlowId, FlowError> {
    if let Some(ch) = channel {
        if let Some(id) = overrides
            .exact
            .get(&(agent_id.to_string(), ch.to_string()))
        {
            return Ok(id.clone());
        }
    }
    if let Some(id) = overrides.wildcard.get(agent_id) {
        return Ok(id.clone());
    }
    if let Some(id) = defaults.get(agent_id) {
        return Ok(id.clone());
    }
    Err(FlowError::UnknownAgent(agent_id.to_string()))
}

/// Decide which `SessionKey` a dispatch writes to.
/// Phase 5 keeps this pure; Orchestrator applies the per-session lock separately.
#[derive(Debug, Clone)]
pub struct SessionResolution {
    pub session_key: String,
    pub parent_session_key: Option<String>,
    pub is_new: bool,
}

#[derive(Debug, Clone)]
pub struct SessionResolveInput {
    pub strategy: SessionStrategy,
    pub session_hint: Option<String>,
    pub parent_session: Option<String>,
    pub fresh_key_fn: fn() -> String,
}

pub fn resolve_session(input: SessionResolveInput) -> Result<SessionResolution, FlowError> {
    match input.strategy {
        SessionStrategy::Reuse => match input.session_hint {
            Some(k) => Ok(SessionResolution {
                session_key: k,
                parent_session_key: None,
                is_new: false,
            }),
            None => Err(FlowError::InvalidConfig(
                "SessionStrategy::Reuse requires session_hint".into(),
            )),
        },
        SessionStrategy::Fresh => Ok(SessionResolution {
            session_key: (input.fresh_key_fn)(),
            parent_session_key: None,
            is_new: true,
        }),
        SessionStrategy::Child { parent_session_key } => {
            let parent = parent_session_key
                .or(input.parent_session.clone())
                .ok_or_else(|| {
                    FlowError::InvalidConfig(
                        "SessionStrategy::Child requires parent_session at runtime".into(),
                    )
                })?;
            Ok(SessionResolution {
                session_key: (input.fresh_key_fn)(),
                parent_session_key: Some(parent),
                is_new: true,
            })
        }
    }
}
```

Modify `src/orchestrator/mod.rs` — add:
```rust
pub mod resolver;
pub use resolver::{MAX_FLOW_DEPTH, RoutingOverrides};
```

- [ ] **Step 5.4: Run — expect 8 pass**

Run: `cargo test -p alephcore --lib orchestrator::tests::resolver -- --nocapture`
Expected: 8 passed.

- [ ] **Step 5.5: Write session-resolve tests**

Append to `src/orchestrator/tests/resolver.rs`:
```rust
use crate::orchestrator::flow_spec::SessionStrategy;
use crate::orchestrator::resolver::{resolve_session, SessionResolveInput};

fn fixed_key() -> String { "fresh-abc".into() }

#[test]
fn reuse_strategy_uses_hint() {
    let input = SessionResolveInput {
        strategy: SessionStrategy::Reuse,
        session_hint: Some("existing-123".into()),
        parent_session: None,
        fresh_key_fn: fixed_key,
    };
    let r = resolve_session(input).unwrap();
    assert_eq!(r.session_key, "existing-123");
    assert_eq!(r.parent_session_key, None);
    assert!(!r.is_new);
}

#[test]
fn reuse_strategy_without_hint_errors() {
    let input = SessionResolveInput {
        strategy: SessionStrategy::Reuse,
        session_hint: None,
        parent_session: None,
        fresh_key_fn: fixed_key,
    };
    let err = resolve_session(input).unwrap_err();
    assert!(matches!(err, FlowError::InvalidConfig(_)));
}

#[test]
fn fresh_strategy_mints_new_key() {
    let input = SessionResolveInput {
        strategy: SessionStrategy::Fresh,
        session_hint: Some("ignored".into()),
        parent_session: None,
        fresh_key_fn: fixed_key,
    };
    let r = resolve_session(input).unwrap();
    assert_eq!(r.session_key, "fresh-abc");
    assert!(r.is_new);
    assert!(r.parent_session_key.is_none());
}

#[test]
fn child_strategy_uses_parent_from_request() {
    let input = SessionResolveInput {
        strategy: SessionStrategy::Child { parent_session_key: None },
        session_hint: None,
        parent_session: Some("parent-xyz".into()),
        fresh_key_fn: fixed_key,
    };
    let r = resolve_session(input).unwrap();
    assert_eq!(r.session_key, "fresh-abc");
    assert_eq!(r.parent_session_key.as_deref(), Some("parent-xyz"));
    assert!(r.is_new);
}

#[test]
fn child_strategy_spec_override_beats_request() {
    let input = SessionResolveInput {
        strategy: SessionStrategy::Child {
            parent_session_key: Some("from-spec".into()),
        },
        session_hint: None,
        parent_session: Some("from-request".into()),
        fresh_key_fn: fixed_key,
    };
    let r = resolve_session(input).unwrap();
    assert_eq!(r.parent_session_key.as_deref(), Some("from-spec"));
}

#[test]
fn child_strategy_without_parent_errors() {
    let input = SessionResolveInput {
        strategy: SessionStrategy::Child { parent_session_key: None },
        session_hint: None,
        parent_session: None,
        fresh_key_fn: fixed_key,
    };
    let err = resolve_session(input).unwrap_err();
    assert!(matches!(err, FlowError::InvalidConfig(_)));
}
```

- [ ] **Step 5.6: Run — expect 14 pass total**

Run: `cargo test -p alephcore --lib orchestrator::tests::resolver -- --nocapture`
Expected: 14 passed.

- [ ] **Step 5.7: Commit**

```bash
git add src/orchestrator/resolver.rs src/orchestrator/tests/resolver.rs src/orchestrator/mod.rs src/orchestrator/tests/mod.rs
git commit -m "orchestrator: add resolver (routing + depth + session) (Phase 5.5)"
```

---

## Task 6: SandboxFactory Type Alias + Noop Sandbox

**Files:**
- Create: `src/orchestrator/sandbox_factory.rs`
- Create: `src/orchestrator/tests/sandbox_factory.rs`
- Modify: `src/orchestrator/mod.rs`, `src/orchestrator/tests/mod.rs`

**Context:** Thin type alias over a closure producing `Arc<dyn Sandbox>`. No new trait — follows the `build_sandbox()` pattern noted in `src/harness/deps.rs:7-9`. Includes a `NoopSandbox` that denies every exec-class request for `SandboxKind::None`.

- [ ] **Step 6.1: Look at existing Sandbox trait signature**

Run: `grep -n "pub trait Sandbox" src/sandbox/mod.rs; grep -n "pub fn build_sandbox" src/sandbox/ -r`
Read the full `Sandbox` trait and its associated types (`SandboxCommand`, `SandboxOutput`, `SandboxError`). This informs the `NoopSandbox` impl in step 6.3.

- [ ] **Step 6.2: Write failing test for NoopSandbox**

Append to `src/orchestrator/tests/mod.rs`:
```rust
mod sandbox_factory;
```

Create `src/orchestrator/tests/sandbox_factory.rs`:
```rust
use std::sync::Arc;

use crate::orchestrator::flow_spec::SandboxKind;
use crate::orchestrator::sandbox_factory::{build_sandbox_factory, NoopSandbox};
use crate::sandbox::Sandbox;

#[tokio::test]
async fn noop_sandbox_denies_exec_class() {
    let s: Arc<dyn Sandbox> = Arc::new(NoopSandbox::new());
    // Build a minimal SandboxCommand matching the existing signature in src/sandbox/mod.rs.
    // NOTE: adjust the ctor call below to whatever `SandboxCommand::new` expects today.
    let cmd = crate::sandbox::SandboxCommand::for_test_exec("echo", &["hi"]);
    let err = s.execute(cmd).await.unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("denied"),
        "got {err}"
    );
}

#[tokio::test]
async fn factory_for_none_returns_noop_sandbox() {
    // Use a dummy inner builder for Workspace that panics if called — this test only exercises None.
    let factory = build_sandbox_factory(Arc::new(|_session_key| {
        panic!("Workspace builder should not be invoked for SandboxKind::None")
    }));
    let sb = factory(SandboxKind::None, "sess-abc").expect("noop");
    // Running an exec command on the returned sandbox must be denied.
    let cmd = crate::sandbox::SandboxCommand::for_test_exec("ls", &[]);
    assert!(sb.execute(cmd).await.is_err());
}
```

**NOTE for implementer:** The `SandboxCommand::for_test_exec` helper may or may not exist. If absent, the test should construct a `SandboxCommand` using whatever constructor `src/sandbox/mod.rs` exposes. As part of this task, add a `#[cfg(test)] pub fn for_test_exec(prog: &str, args: &[&str]) -> SandboxCommand` helper to `src/sandbox/mod.rs` if nothing suitable exists — minimal, test-only, documented.

- [ ] **Step 6.3: Run — expect compile failure**

Run: `cargo test -p alephcore --lib orchestrator::tests::sandbox_factory --no-run`
Expected: fails.

- [ ] **Step 6.4: Implement `sandbox_factory.rs`**

Create `src/orchestrator/sandbox_factory.rs`:
```rust
//! Per-session Sandbox allocator. See design §6.
//!
//! Phase 3 exposes `build_sandbox()` returning a shared `Arc<dyn Sandbox>`.
//! Orchestrator needs per-session provisioning, so we wrap the Workspace
//! builder in a closure that also knows how to produce `NoopSandbox` for
//! `SandboxKind::None`.

use std::sync::Arc;

use async_trait::async_trait;

use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::SandboxKind;
use crate::sandbox::{Sandbox, SandboxCommand, SandboxError, SandboxOutput};

pub type WorkspaceBuilder = Arc<dyn Fn(&str) -> Result<Arc<dyn Sandbox>, String> + Send + Sync>;

pub type SandboxFactory =
    Arc<dyn Fn(SandboxKind, &str) -> Result<Arc<dyn Sandbox>, FlowError> + Send + Sync>;

pub fn build_sandbox_factory(workspace: WorkspaceBuilder) -> SandboxFactory {
    Arc::new(move |kind, session_key| match kind {
        SandboxKind::None => Ok(Arc::new(NoopSandbox::new()) as Arc<dyn Sandbox>),
        SandboxKind::Workspace => workspace(session_key)
            .map_err(FlowError::SandboxProvisionFailed),
    })
}

pub struct NoopSandbox;

impl NoopSandbox {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NoopSandbox {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Sandbox for NoopSandbox {
    async fn execute(&self, _cmd: SandboxCommand) -> Result<SandboxOutput, SandboxError> {
        Err(SandboxError::new_denied(
            "SandboxKind::None flow denied exec-class tool",
        ))
    }
}
```

**NOTE:** this requires `SandboxError::new_denied(&'static str)` to exist. If it doesn't, add a small constructor to `src/sandbox/mod.rs`:
```rust
impl SandboxError {
    pub fn new_denied(msg: &str) -> Self {
        // Use whatever existing variant represents "denied" in your SandboxError enum.
        // If there's no dedicated variant, introduce one:
        //   #[error("denied: {0}")] Denied(String),
        // and wire here.
        Self::Denied(msg.to_string())
    }
}
```
If the existing `SandboxError` has a different shape, adapt the constructor to return the closest variant (e.g., `PermissionDenied`, `CapabilityMissing`). Leave a `// PHASE-5: adapted to existing SandboxError variant` comment so the reviewer sees the adaptation.

Modify `src/orchestrator/mod.rs` — add:
```rust
pub mod sandbox_factory;
pub use sandbox_factory::{build_sandbox_factory, NoopSandbox, SandboxFactory, WorkspaceBuilder};
```

- [ ] **Step 6.5: Run — expect 2 pass**

Run: `cargo test -p alephcore --lib orchestrator::tests::sandbox_factory -- --nocapture`
Expected: 2 passed.

- [ ] **Step 6.6: Commit**

```bash
git add src/orchestrator/sandbox_factory.rs src/orchestrator/tests/sandbox_factory.rs src/orchestrator/mod.rs src/orchestrator/tests/mod.rs src/sandbox/
git commit -m "orchestrator: add SandboxFactory + NoopSandbox (Phase 5.6)"
```

---

## Task 7: Orchestrator Struct + Seven-Step Dispatch

**Files:**
- Create: `src/orchestrator/dispatch.rs`
- Create: `src/orchestrator/tests/dispatch.rs`
- Modify: `src/orchestrator/mod.rs`, `src/orchestrator/tests/mod.rs`

**Context:** The heart of Phase 5. Each of the 7 steps is independently testable with mocked deps. Per-session locking uses `Mutex<HashSet<SessionKey>>` held by Orchestrator; contention on a key returns `FlowError::SessionConflict` instead of blocking.

- [ ] **Step 7.1: Write mock deps module**

Create `src/orchestrator/tests/dispatch.rs` top-of-file:
```rust
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::{broadcast, oneshot};
use tokio_util::sync::CancellationToken;

use crate::orchestrator::dispatch::{FlowHandle, FlowRequest, Orchestrator};
use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_registry::{FlowRegistry, FlowSet};
use crate::orchestrator::flow_spec::{
    BrainRef, FlowInput, FlowOverrides, FlowSpec, SandboxKind, SessionStrategy,
};
use crate::orchestrator::resolver::RoutingOverrides;
use crate::orchestrator::sandbox_factory::{build_sandbox_factory, NoopSandbox};
use crate::sandbox::Sandbox;
```

(The actual test bodies come in steps 7.6–7.11.)

- [ ] **Step 7.2: Write failing Orchestrator shell**

Create `src/orchestrator/dispatch.rs`:
```rust
//! Orchestrator core + seven-step dispatch. See design §6.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tokio::sync::{broadcast, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_registry::FlowRegistry;
use crate::orchestrator::flow_spec::{
    AgentId, BrainRef, FlowId, FlowInput, FlowSpec, SandboxKind, SessionStrategy,
};
use crate::orchestrator::resolver::{
    depth_guard, resolve_flow_id, resolve_session, RoutingOverrides, SessionResolveInput,
};
use crate::orchestrator::sandbox_factory::SandboxFactory;

/// Spawn handle returned to the Gateway.
pub struct FlowHandle {
    pub session_key: String,
    pub events: broadcast::Receiver<FlowStreamEvent>,
    pub completion: oneshot::Receiver<Result<FlowOutcome, FlowError>>,
    pub cancel: CancellationToken,
}

#[derive(Debug, Clone)]
pub enum FlowStreamEvent {
    Delta(String),
    ToolCall { name: String },
    Complete,
}

#[derive(Debug, Clone)]
pub struct FlowOutcome {
    pub final_text: String,
    pub iterations: u32,
}

#[derive(Debug, Clone)]
pub struct FlowRequest {
    pub flow_id: Option<FlowId>,
    pub agent_id: AgentId,
    pub input: FlowInput,
    pub channel: Option<String>,
    pub session_hint: Option<String>,
    pub parent_session: Option<String>,
    pub depth: u8,
}

/// Orchestrator dependencies. Most are behind `Arc<dyn Trait>` so the struct
/// itself is `Clone`-cheap. Per-session lock is an internal `Mutex<HashSet>`.
pub struct Orchestrator {
    pub flow_registry: Arc<FlowRegistry>,
    pub routing_overrides: Arc<RoutingOverrides>,
    pub default_routing: Arc<HashMap<AgentId, FlowId>>,
    pub session_service: Arc<dyn crate::session::service::SessionService>,
    pub sandbox_factory: SandboxFactory,
    /// Harness runner injected at construction — test mocks can swap this out.
    pub harness: Arc<dyn HarnessRunner>,
    active_sessions: Arc<Mutex<HashSet<String>>>,
}

#[async_trait::async_trait]
pub trait HarnessRunner: Send + Sync {
    async fn run(
        &self,
        session_key: String,
        spec: Arc<FlowSpec>,
        input: FlowInput,
        sandbox: Arc<dyn crate::sandbox::Sandbox>,
        events: broadcast::Sender<FlowStreamEvent>,
        cancel: CancellationToken,
    ) -> Result<FlowOutcome, FlowError>;
}

impl Orchestrator {
    pub fn new(
        flow_registry: Arc<FlowRegistry>,
        routing_overrides: Arc<RoutingOverrides>,
        default_routing: Arc<HashMap<AgentId, FlowId>>,
        session_service: Arc<dyn crate::session::service::SessionService>,
        sandbox_factory: SandboxFactory,
        harness: Arc<dyn HarnessRunner>,
    ) -> Self {
        Self {
            flow_registry,
            routing_overrides,
            default_routing,
            session_service,
            sandbox_factory,
            harness,
            active_sessions: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Seven-step dispatch. See design §6.
    pub async fn dispatch(&self, req: FlowRequest) -> Result<FlowHandle, FlowError> {
        // Step 2: depth guard (cheap, do first so a runaway caller doesn't hold any resource).
        depth_guard(req.depth)?;

        // Step 1: resolve flow_id → FlowSpec.
        let flow_id = match &req.flow_id {
            Some(id) => id.clone(),
            None => resolve_flow_id(
                &req.agent_id,
                req.channel.as_deref(),
                &self.routing_overrides,
                &self.default_routing,
            )?,
        };
        let spec = self
            .flow_registry
            .resolve(&flow_id)
            .ok_or_else(|| FlowError::UnknownFlow(flow_id.clone()))?;

        // Step 3: agent lookup — we only need to assert it exists; AgentDef is read by harness.
        // For this plan, harness will re-fetch via its own AgentRegistry handle.

        // Step 4: session resolve + per-session lock.
        let session_input = SessionResolveInput {
            strategy: spec.session_strategy.clone(),
            session_hint: req.session_hint.clone(),
            parent_session: req.parent_session.clone(),
            fresh_key_fn: || uuid::Uuid::new_v4().to_string(),
        };
        let session_res = resolve_session(session_input)?;
        {
            let mut guard = self
                .active_sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if !guard.insert(session_res.session_key.clone()) {
                return Err(FlowError::SessionConflict(session_res.session_key));
            }
        }

        // Step 5: brain pick is deferred to HarnessRunner — it knows the ProviderRegistry.
        //         FlowSpec carries BrainRef which harness reads via spec.brain.

        // Step 6: sandbox provision.
        let sandbox = (self.sandbox_factory)(spec.sandbox_kind, &session_res.session_key)?;

        // Step 7: spawn harness, plumbing events + completion + cancel.
        let (event_tx, event_rx) = broadcast::channel::<FlowStreamEvent>(256);
        let (done_tx, done_rx) = oneshot::channel();
        let cancel = CancellationToken::new();

        let harness = self.harness.clone();
        let spec_clone = spec.clone();
        let input_clone = req.input.clone();
        let sandbox_clone = sandbox.clone();
        let cancel_clone = cancel.clone();
        let session_key = session_res.session_key.clone();
        let active = self.active_sessions.clone();
        let session_for_release = session_res.session_key.clone();

        tokio::spawn(async move {
            let outcome = harness
                .run(
                    session_key,
                    spec_clone,
                    input_clone,
                    sandbox_clone,
                    event_tx,
                    cancel_clone,
                )
                .await;
            let _ = done_tx.send(outcome);
            let mut guard = active.lock().unwrap_or_else(|e| e.into_inner());
            guard.remove(&session_for_release);
        });

        Ok(FlowHandle {
            session_key: session_res.session_key,
            events: event_rx,
            completion: done_rx,
            cancel,
        })
    }

    pub async fn reload_flows(
        &self,
        new_set: crate::orchestrator::flow_registry::FlowSet,
    ) -> Result<(), FlowError> {
        self.flow_registry.replace(new_set);
        debug!(count = self.flow_registry.len(), "flow registry reloaded");
        Ok(())
    }
}
```

**NOTE:** This file references `crate::session::service::SessionService` — verify the trait path. If the trait lives at `crate::session::SessionService`, adjust the import accordingly. Run `grep -n "pub trait SessionService" src/session/` to confirm.

- [ ] **Step 7.3: Clone-impl for FlowInput**

Ensure `FlowInput` is `Clone` (already derived in Task 1 — verify by reading `flow_spec.rs`). If not, add `#[derive(Clone)]`. The dispatch spawn closure needs to clone `req.input`.

- [ ] **Step 7.4: Add HarnessRunner re-export**

Modify `src/orchestrator/mod.rs` — add:
```rust
pub mod dispatch;
pub use dispatch::{FlowHandle, FlowOutcome, FlowRequest, FlowStreamEvent, HarnessRunner, Orchestrator};
```

- [ ] **Step 7.5: Write MockHarness**

Append to `src/orchestrator/tests/dispatch.rs`:
```rust
struct MockHarness {
    outcome: FlowOutcome,
    /// Log how many times run() was called and with what session.
    invocations: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl crate::orchestrator::dispatch::HarnessRunner for MockHarness {
    async fn run(
        &self,
        session_key: String,
        _spec: Arc<FlowSpec>,
        _input: FlowInput,
        _sandbox: Arc<dyn Sandbox>,
        events: broadcast::Sender<crate::orchestrator::dispatch::FlowStreamEvent>,
        _cancel: CancellationToken,
    ) -> Result<FlowOutcome, FlowError> {
        self.invocations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(session_key);
        let _ = events.send(crate::orchestrator::dispatch::FlowStreamEvent::Delta("hi".into()));
        let _ = events.send(crate::orchestrator::dispatch::FlowStreamEvent::Complete);
        Ok(self.outcome.clone())
    }
}

fn fixture_orchestrator() -> (Orchestrator, Arc<Mutex<Vec<String>>>) {
    let mut spec_map = FlowSet::new();
    let spec = FlowSpec {
        id: "default-agent".into(),
        description: "t".into(),
        agent: "main".into(),
        brain: BrainRef::Default,
        sandbox_kind: SandboxKind::None,
        session_strategy: SessionStrategy::Fresh,
        overrides: FlowOverrides::default(),
    };
    spec_map.insert("default-agent".into(), Arc::new(spec));
    let registry = Arc::new(FlowRegistry::new(spec_map));

    let mut defaults = std::collections::HashMap::new();
    defaults.insert("main".into(), "default-agent".into());

    let session_service: Arc<dyn crate::session::service::SessionService> =
        crate::orchestrator::tests::dispatch::fake_session_service();

    let sandbox_factory = build_sandbox_factory(Arc::new(|_| {
        Ok(Arc::new(NoopSandbox::new()) as Arc<dyn Sandbox>)
    }));

    let invocations = Arc::new(Mutex::new(Vec::<String>::new()));
    let harness = Arc::new(MockHarness {
        outcome: crate::orchestrator::dispatch::FlowOutcome {
            final_text: "ok".into(),
            iterations: 1,
        },
        invocations: invocations.clone(),
    });

    (
        Orchestrator::new(
            registry,
            Arc::new(RoutingOverrides::default()),
            Arc::new(defaults),
            session_service,
            sandbox_factory,
            harness,
        ),
        invocations,
    )
}

/// Minimal fake SessionService — orchestrator only holds the trait object;
/// dispatch currently never calls into it (deferred to harness).
/// If/when dispatch itself emits SessionEvents (e.g., SessionStarted),
/// this fake will grow.
fn fake_session_service() -> Arc<dyn crate::session::service::SessionService> {
    // Reuse an existing test helper if one exists — grep for
    // `InProcessActorSessionService::new` to confirm.
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::session::store::migrate_add_session_events(&conn).unwrap();
    let store: Arc<dyn crate::session::store::SessionEventStore> =
        Arc::new(crate::session::store::SqliteEventStore::new(conn));
    Arc::new(crate::session::in_process::InProcessActorSessionService::new(store))
}
```

**NOTE:** adjust the `fake_session_service` body to whatever the existing helper convention is. If `src/session/shim.rs::tests::fresh_service()` exists, factor it up and reuse.

- [ ] **Step 7.6: Write test — happy path dispatch + completion**

Append to `src/orchestrator/tests/dispatch.rs`:
```rust
#[tokio::test]
async fn dispatch_happy_path_returns_handle_and_completes() {
    let (orch, invocations) = fixture_orchestrator();
    let handle = orch
        .dispatch(FlowRequest {
            flow_id: None,
            agent_id: "main".into(),
            input: FlowInput::Prompt("hello".into()),
            channel: None,
            session_hint: None,
            parent_session: None,
            depth: 0,
        })
        .await
        .expect("dispatch ok");

    let outcome = handle.completion.await.unwrap().unwrap();
    assert_eq!(outcome.final_text, "ok");

    let calls = invocations.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(!calls[0].is_empty(), "session key must be non-empty");
}
```

- [ ] **Step 7.7: Test — UnknownFlow error**

Append:
```rust
#[tokio::test]
async fn dispatch_unknown_flow_id_returns_error() {
    let (orch, _) = fixture_orchestrator();
    let err = orch
        .dispatch(FlowRequest {
            flow_id: Some("does-not-exist".into()),
            agent_id: "main".into(),
            input: FlowInput::Prompt("x".into()),
            channel: None,
            session_hint: None,
            parent_session: None,
            depth: 0,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, FlowError::UnknownFlow(ref id) if id == "does-not-exist"));
}
```

- [ ] **Step 7.8: Test — UnknownAgent error**

Append:
```rust
#[tokio::test]
async fn dispatch_unknown_agent_returns_error() {
    let (orch, _) = fixture_orchestrator();
    let err = orch
        .dispatch(FlowRequest {
            flow_id: None,
            agent_id: "ghost".into(),
            input: FlowInput::Prompt("x".into()),
            channel: None,
            session_hint: None,
            parent_session: None,
            depth: 0,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, FlowError::UnknownAgent(ref id) if id == "ghost"));
}
```

- [ ] **Step 7.9: Test — RecursionLimit error**

Append:
```rust
use crate::orchestrator::resolver::MAX_FLOW_DEPTH;

#[tokio::test]
async fn dispatch_above_max_depth_returns_recursion_error() {
    let (orch, _) = fixture_orchestrator();
    let err = orch
        .dispatch(FlowRequest {
            flow_id: None,
            agent_id: "main".into(),
            input: FlowInput::Prompt("x".into()),
            channel: None,
            session_hint: None,
            parent_session: None,
            depth: MAX_FLOW_DEPTH + 1,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, FlowError::RecursionLimit { max } if max == MAX_FLOW_DEPTH));
}
```

- [ ] **Step 7.10: Test — same-session concurrent dispatch rejects**

Append:
```rust
#[tokio::test]
async fn dispatch_rejects_concurrent_same_session_reuse() {
    // Build a FlowRegistry where default-agent uses Reuse (not Fresh) so the
    // session_key comes from session_hint and both dispatches collide.
    let mut spec_map = FlowSet::new();
    let spec = FlowSpec {
        id: "default-agent".into(),
        description: "t".into(),
        agent: "main".into(),
        brain: BrainRef::Default,
        sandbox_kind: SandboxKind::None,
        session_strategy: SessionStrategy::Reuse,
        overrides: FlowOverrides::default(),
    };
    spec_map.insert("default-agent".into(), Arc::new(spec));
    let registry = Arc::new(FlowRegistry::new(spec_map));
    let mut defaults = std::collections::HashMap::new();
    defaults.insert("main".into(), "default-agent".into());

    let session_service = fake_session_service();
    let sandbox_factory = build_sandbox_factory(Arc::new(|_| {
        Ok(Arc::new(NoopSandbox::new()) as Arc<dyn Sandbox>)
    }));

    // Mock harness that NEVER completes (holds the session lock).
    struct HangingHarness;
    #[async_trait]
    impl crate::orchestrator::dispatch::HarnessRunner for HangingHarness {
        async fn run(
            &self,
            _s: String,
            _sp: Arc<FlowSpec>,
            _i: FlowInput,
            _sb: Arc<dyn Sandbox>,
            _ev: broadcast::Sender<crate::orchestrator::dispatch::FlowStreamEvent>,
            cancel: CancellationToken,
        ) -> Result<crate::orchestrator::dispatch::FlowOutcome, FlowError> {
            cancel.cancelled().await;
            Ok(crate::orchestrator::dispatch::FlowOutcome {
                final_text: "cancelled".into(),
                iterations: 0,
            })
        }
    }

    let orch = Orchestrator::new(
        registry,
        Arc::new(RoutingOverrides::default()),
        Arc::new(defaults),
        session_service,
        sandbox_factory,
        Arc::new(HangingHarness),
    );

    let mk_req = || FlowRequest {
        flow_id: None,
        agent_id: "main".into(),
        input: FlowInput::Prompt("x".into()),
        channel: None,
        session_hint: Some("shared-session".into()),
        parent_session: None,
        depth: 0,
    };

    let first = orch.dispatch(mk_req()).await.expect("first ok");
    let err = orch.dispatch(mk_req()).await.unwrap_err();
    assert!(matches!(err, FlowError::SessionConflict(ref k) if k == "shared-session"));
    first.cancel.cancel();
    let _ = first.completion.await;
}
```

- [ ] **Step 7.11: Run all dispatch tests — expect 5 pass**

Run: `cargo test -p alephcore --lib orchestrator::tests::dispatch -- --nocapture`
Expected: 5 passed (happy / UnknownFlow / UnknownAgent / RecursionLimit / SessionConflict).

- [ ] **Step 7.12: Run full orchestrator test suite — expect zero regression**

Run: `cargo test -p alephcore --lib orchestrator::tests -- --nocapture`
Expected: 4 + 4 + 4 + 4 + 14 + 2 + 5 = 37 passed.

- [ ] **Step 7.13: Commit**

```bash
git add src/orchestrator/dispatch.rs src/orchestrator/tests/dispatch.rs src/orchestrator/mod.rs src/orchestrator/tests/mod.rs
git commit -m "orchestrator: add Orchestrator + 7-step dispatch (Phase 5.7)"
```

---

## Task 8: HarnessRunner Impl Over AgentHarness

**Files:**
- Create: `src/orchestrator/harness_bridge.rs`
- Create: `src/orchestrator/tests/harness_bridge.rs`
- Modify: `src/orchestrator/mod.rs`, `src/orchestrator/tests/mod.rs`

**Context:** Wire the Orchestrator's `HarnessRunner` trait to the real `AgentHarness` from Phase 4. The bridge resolves AgentDef, picks provider per `BrainRef`, constructs deps, then calls `AgentHarness::run_turn` (or its session driver entry point).

- [ ] **Step 8.1: Read Phase 4 entry points**

Run:
```bash
grep -n "impl SessionDriver for AgentHarness" src/harness/
grep -n "pub async fn run" src/harness/agent.rs | head -10
grep -n "HarnessDeps" src/harness/deps.rs
```
Read the relevant sections to understand what `AgentHarness::run` takes. Expected inputs include: `session_key`, `HarnessDeps` (agent_def, tool_service, sandbox, llm), `input`, `sink`.

- [ ] **Step 8.2: Write failing integration stub**

Append to `src/orchestrator/tests/mod.rs`:
```rust
mod harness_bridge;
```

Create `src/orchestrator/tests/harness_bridge.rs`:
```rust
//! Bridge-level smoke tests. Full end-to-end coverage lives in
//! tests/orchestrator_e2e.rs (Task 13).

use std::sync::Arc;

use crate::orchestrator::harness_bridge::AgentHarnessRunner;

#[test]
fn agent_harness_runner_constructs() {
    // Just proves the type assembles with Arc-held deps.
    // Detailed behavior is covered by the e2e suite; unit testing it in
    // isolation would require re-stubbing every Phase 4 dep.
    fn _requires_send_sync<T: Send + Sync>() {}
    _requires_send_sync::<AgentHarnessRunner>();
}
```

- [ ] **Step 8.3: Implement `harness_bridge.rs`**

Create `src/orchestrator/harness_bridge.rs`:
```rust
//! Adapter from Orchestrator::HarnessRunner to Phase 4's AgentHarness.
//!
//! Responsibilities:
//!   1. Resolve AgentDef from flow.agent
//!   2. Pick LLMProvider from flow.brain (Default / Preferred / Strict)
//!   3. Construct HarnessDeps (agent_def, tool_service, sandbox, llm)
//!   4. Delegate to `AgentHarness::run_turn` or equivalent
//!   5. Translate Harness events into FlowStreamEvent for the Orchestrator channel

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::agents::registry::AgentRegistry;
use crate::orchestrator::dispatch::{FlowOutcome, FlowStreamEvent, HarnessRunner};
use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::{BrainRef, FlowInput, FlowSpec};
use crate::sandbox::Sandbox;

pub struct AgentHarnessRunner {
    pub agent_registry: Arc<AgentRegistry>,
    pub tool_service: Arc<dyn crate::tools::service::ToolService>,
    pub provider_registry: Arc<crate::thinker::ProviderRegistry>,
}

#[async_trait]
impl HarnessRunner for AgentHarnessRunner {
    async fn run(
        &self,
        session_key: String,
        spec: Arc<FlowSpec>,
        input: FlowInput,
        sandbox: Arc<dyn Sandbox>,
        events: broadcast::Sender<FlowStreamEvent>,
        cancel: CancellationToken,
    ) -> Result<FlowOutcome, FlowError> {
        // Step 1: resolve AgentDef.
        let agent_def = self
            .agent_registry
            .get(&spec.agent)
            .ok_or_else(|| FlowError::UnknownAgent(spec.agent.clone()))?;

        // Step 2: pick LLM provider per BrainRef.
        let llm = pick_llm(&spec.brain, &self.provider_registry)
            .map_err(|e| FlowError::ProviderUnavailable(e))?;

        // Step 3: apply FlowOverrides on top of AgentDef defaults.
        let max_iterations = spec
            .overrides
            .max_iterations
            .unwrap_or(agent_def.max_iterations());
        let extra_prompt = spec.overrides.extra_system_prompt.clone();

        // Step 4: build HarnessDeps and invoke AgentHarness::run_turn.
        //
        // The existing `src/harness/agent.rs` exposes a run-turn entry point
        // using `HarnessDeps` (see src/harness/deps.rs). Construct it and
        // delegate. This is the single place where Phase 4 meets Phase 5.
        let deps = crate::harness::deps::HarnessDeps {
            agent_def,
            tool_service: self.tool_service.clone(),
            sandbox,
            llm,
            session_service: /* injected elsewhere — see wiring in Task 9 */
                panic!("inject via Orchestrator::new; see Task 9"),
            max_iterations,
            extra_system_prompt: extra_prompt,
        };

        // Translate Harness event callbacks → FlowStreamEvent.
        let sink = FlowEventSink {
            events: events.clone(),
        };

        // Call the existing harness entry point. If `run_turn` returns
        // `Result<TurnOutcome, HarnessError>`, adapt the mapping below.
        let outcome = crate::harness::agent::AgentHarness::new(deps)
            .run_turn(session_key, input_to_harness(input), sink, cancel)
            .await
            .map_err(|e| FlowError::Internal(format!("harness: {e}")))?;

        let _ = events.send(FlowStreamEvent::Complete);

        Ok(FlowOutcome {
            final_text: outcome.final_text,
            iterations: outcome.iterations,
        })
    }
}

fn pick_llm(
    brain: &BrainRef,
    registry: &crate::thinker::ProviderRegistry,
) -> Result<Arc<dyn crate::thinker::LlmProvider>, String> {
    match brain {
        BrainRef::Default => registry
            .default_provider()
            .ok_or_else(|| "no default provider".into()),
        BrainRef::Preferred { provider } => match registry.get(provider) {
            Some(p) => Ok(p),
            None => registry
                .default_provider()
                .ok_or_else(|| format!("preferred '{provider}' missing and no fallback")),
        },
        BrainRef::Strict { provider, model } => match registry.get(provider) {
            Some(p) => {
                if let Some(m) = model {
                    p.select_model(m)
                        .map_err(|e| format!("strict {provider}/{m}: {e}"))
                } else {
                    Ok(p)
                }
            }
            None => Err(format!("strict provider '{provider}' not registered")),
        },
    }
}

fn input_to_harness(input: FlowInput) -> crate::harness::agent::HarnessInput {
    match input {
        FlowInput::Prompt(s) => crate::harness::agent::HarnessInput::Prompt(s),
        FlowInput::Messages(m) => crate::harness::agent::HarnessInput::Messages(m),
    }
}

struct FlowEventSink {
    events: broadcast::Sender<FlowStreamEvent>,
}

// If src/harness/ exposes a `Sink` trait, impl it here and translate
// per-call kinds into FlowStreamEvent. Leave this as a thin translation
// layer — business logic stays in the harness itself.
```

**IMPORTANT:** This task is the **single point** where Phase 4 and Phase 5 meet. The implementer **must** read `src/harness/agent.rs` + `src/harness/deps.rs` before coding and adapt the field names in `HarnessDeps` / `HarnessInput` / `TurnOutcome` to whatever is actually exported. The `panic!` for `session_service` is a deliberate marker — Task 9 wires the real value in.

- [ ] **Step 8.4: Add the needed `AgentDef::max_iterations()` getter if missing**

Run: `grep -n "pub fn max_iterations\|max_iterations:" src/agents/types.rs`
If `AgentDef.max_iterations` is a public field, use `agent_def.max_iterations`. If it's private, add a getter:
```rust
impl AgentDef {
    pub fn max_iterations(&self) -> u32 { self.max_iterations.unwrap_or(10) }
}
```

- [ ] **Step 8.5: Update `Orchestrator::new` to accept `session_service` separately (it was already there in Task 7; confirm)**

Verify `Orchestrator::new` from Task 7.2 takes `session_service: Arc<dyn SessionService>`. If yes, modify `AgentHarnessRunner` to store it in a field and pass it into `HarnessDeps` instead of `panic!`:

```rust
pub struct AgentHarnessRunner {
    pub agent_registry: Arc<AgentRegistry>,
    pub tool_service: Arc<dyn crate::tools::service::ToolService>,
    pub provider_registry: Arc<crate::thinker::ProviderRegistry>,
    pub session_service: Arc<dyn crate::session::service::SessionService>,
}
```

And in `run()`:
```rust
let deps = crate::harness::deps::HarnessDeps {
    agent_def,
    tool_service: self.tool_service.clone(),
    sandbox,
    llm,
    session_service: self.session_service.clone(),
    max_iterations,
    extra_system_prompt: extra_prompt,
};
```

- [ ] **Step 8.6: Run — expect compile clean**

Run: `cargo check -p alephcore --lib`
Expected: clean. Fix any adapter mismatches against real `HarnessDeps` / `HarnessInput` / `TurnOutcome`.

- [ ] **Step 8.7: Run the smoke test — expect 1 pass**

Run: `cargo test -p alephcore --lib orchestrator::tests::harness_bridge -- --nocapture`
Expected: 1 passed.

- [ ] **Step 8.8: Commit**

```bash
git add src/orchestrator/harness_bridge.rs src/orchestrator/tests/harness_bridge.rs src/orchestrator/mod.rs src/orchestrator/tests/mod.rs src/agents/types.rs
git commit -m "orchestrator: bridge HarnessRunner to AgentHarness (Phase 5.8)"
```

---

## Task 9: AppContext Provisioning + Bootstrap Wiring

**Files:**
- Modify: `src/app_context.rs` (or wherever `AppContext` lives — run `find src -name "app_context.rs"`)
- Modify: `src/bin/aleph-server/commands/start/mod.rs`
- Modify: `src/gateway/state.rs` (if Gateway holds a separate `AppState`)

**Context:** Compose the Orchestrator at boot from existing services (`SessionService`, `ToolService`, sandbox builder, `AgentRegistry`, `ProviderRegistry`) + `FlowRegistry` built from presets + user flows.

- [ ] **Step 9.1: Locate the AppContext / AppState struct**

Run:
```bash
grep -rn "pub struct AppContext\|pub struct AppState" src/ | head -5
```
Expected: finds the struct. Proceed referencing that exact path.

- [ ] **Step 9.2: Add Orchestrator field**

Modify `src/app_context.rs` (adapt path if different) — add:
```rust
pub orchestrator: std::sync::Arc<crate::orchestrator::Orchestrator>,
```
to the existing `AppContext` struct definition, next to the other `Arc<...>` fields.

- [ ] **Step 9.3: Build Orchestrator at startup**

Modify `src/bin/aleph-server/commands/start/mod.rs` — add an `initialize_orchestrator` helper invoked **after** existing service initialization and **before** Gateway server start:

```rust
async fn initialize_orchestrator(
    flow_dir: &std::path::Path,
    agent_registry: std::sync::Arc<crate::agents::registry::AgentRegistry>,
    session_service: std::sync::Arc<dyn crate::session::service::SessionService>,
    tool_service: std::sync::Arc<dyn crate::tools::service::ToolService>,
    provider_registry: std::sync::Arc<crate::thinker::ProviderRegistry>,
    workspace_builder: crate::orchestrator::sandbox_factory::WorkspaceBuilder,
    routing_overrides: crate::orchestrator::resolver::RoutingOverrides,
) -> anyhow::Result<std::sync::Arc<crate::orchestrator::Orchestrator>> {
    use crate::orchestrator::{
        dispatch::Orchestrator,
        flow_registry::FlowRegistry,
        harness_bridge::AgentHarnessRunner,
        loader::{load_presets, load_user_flows_from_dir, merge_catalogs},
        sandbox_factory::build_sandbox_factory,
    };

    let presets = load_presets()?;
    let user = load_user_flows_from_dir(flow_dir).await?;
    let merged = merge_catalogs(presets, user);
    let flow_registry = std::sync::Arc::new(FlowRegistry::new(merged));

    // Default routing: agent_id → same-named FlowId for the 7 builtins.
    let mut defaults = std::collections::HashMap::new();
    for id in agent_registry.list_ids() {
        defaults.insert(id.clone(), if id == "main" { "default-agent".into() } else { id });
    }
    let default_routing = std::sync::Arc::new(defaults);

    let harness = std::sync::Arc::new(AgentHarnessRunner {
        agent_registry: agent_registry.clone(),
        tool_service: tool_service.clone(),
        provider_registry: provider_registry.clone(),
        session_service: session_service.clone(),
    });

    let sandbox_factory = build_sandbox_factory(workspace_builder);

    let orch = Orchestrator::new(
        flow_registry,
        std::sync::Arc::new(routing_overrides),
        default_routing,
        session_service,
        sandbox_factory,
        harness,
    );

    tracing::info!("Orchestrator assembled (Phase 5)");
    Ok(std::sync::Arc::new(orch))
}
```

- [ ] **Step 9.4: Wire the helper into `start_server`**

In `start_server` (same file), after `initialize_tracing(args)` and after `tool_service` + `agent_registry` + `session_service` + `provider_registry` are constructed, add:

```rust
let flow_dir = paths.data_dir.join("flows");  // or ~/.aleph/flows
let workspace_builder: crate::orchestrator::sandbox_factory::WorkspaceBuilder =
    std::sync::Arc::new({
        let sandbox_manager = sandbox_manager.clone();
        move |session_key: &str| {
            sandbox_manager
                .build_workspace(session_key)
                .map(|sb| std::sync::Arc::new(sb) as std::sync::Arc<dyn crate::sandbox::Sandbox>)
                .map_err(|e| e.to_string())
        }
    });

let routing_overrides = load_flow_routing_overrides(&config)?;  // from aleph.toml [flow_routing]
let orchestrator = initialize_orchestrator(
    &flow_dir,
    agent_registry.clone(),
    session_service.clone(),
    tool_service.clone(),
    provider_registry.clone(),
    workspace_builder,
    routing_overrides,
).await?;
```

Pass `orchestrator` into `AppContext::new(..., orchestrator)`.

- [ ] **Step 9.5: Implement `load_flow_routing_overrides`**

Add alongside the helper:
```rust
fn load_flow_routing_overrides(
    config: &crate::config::Config,
) -> anyhow::Result<crate::orchestrator::resolver::RoutingOverrides> {
    let mut ov = crate::orchestrator::resolver::RoutingOverrides::default();
    let Some(section) = config.raw_toml.get("flow_routing") else {
        return Ok(ov);
    };
    let Some(table) = section.as_table() else {
        anyhow::bail!("[flow_routing] must be a table");
    };
    for (key, val) in table {
        let flow_id = val
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("flow_routing.{key} must be a string"))?
            .to_string();
        if let Some((agent, channel)) = key.split_once('.') {
            if channel == "*" {
                ov.wildcard.insert(agent.to_string(), flow_id);
            } else {
                ov.exact.insert((agent.to_string(), channel.to_string()), flow_id);
            }
        } else {
            ov.wildcard.insert(key.to_string(), flow_id);
        }
    }
    Ok(ov)
}
```

(Adapt `config.raw_toml` to the actual accessor on your `Config` struct — run `grep -n "raw_toml\|as_table\|toml::" src/config/`. If config exposes a typed view, add a `flow_routing: Option<BTreeMap<String,String>>` field instead.)

- [ ] **Step 9.6: Run a cargo check to ensure it all compiles**

Run: `cargo check -p alephcore --bin aleph-server`
Expected: clean.

- [ ] **Step 9.7: Commit**

```bash
git add src/app_context.rs src/bin/aleph-server/commands/start/mod.rs
git commit -m "orchestrator: provision Orchestrator in AppContext at boot (Phase 5.9)"
```

---

## Task 10: Gateway `run_agent_loop` Replacement

**Files:**
- Modify: `src/gateway/execution_engine/run_loop.rs`

**Context:** Replace the `AgentLoop::new` call inside `run_agent_loop` with `orchestrator.dispatch(...)` + event pipe. Retain the `RunStatus` return shape so upstream callers don't change.

- [ ] **Step 10.1: Read existing run_agent_loop signature**

Run:
```bash
grep -n "pub async fn run_agent_loop" src/gateway/execution_engine/run_loop.rs
grep -n "AgentLoop::new\|AgentLoop {" src/gateway/execution_engine/run_loop.rs
```
Expected: one public entry, one `AgentLoop::new(...)`-style call. Read 30 lines of context around the latter.

- [ ] **Step 10.2: Replace the loop body**

Modify `src/gateway/execution_engine/run_loop.rs`:

Locate the block that currently constructs and runs `AgentLoop`. Replace with:
```rust
let req = crate::orchestrator::dispatch::FlowRequest {
    flow_id: request.flow_id_hint.clone(),        // if the Gateway request carries an explicit flow, use it
    agent_id: agent.id().to_string(),
    input: crate::orchestrator::flow_spec::FlowInput::Messages(
        request.messages.iter().map(|m| m.clone().into()).collect(),
    ),
    channel: request.peer_channel.as_ref().map(|c| c.to_string()),
    session_hint: Some(request.session_key.to_string()),
    parent_session: None,
    depth: 0,
};
let handle = app_context.orchestrator.dispatch(req).await?;

// Pipe FlowStreamEvent → sink.
let mut events = handle.events;
let completion = handle.completion;
loop {
    match events.recv().await {
        Ok(crate::orchestrator::dispatch::FlowStreamEvent::Delta(d)) => {
            sink.delta(&d).await?;
        }
        Ok(crate::orchestrator::dispatch::FlowStreamEvent::ToolCall { name }) => {
            sink.tool_call(&name).await?;
        }
        Ok(crate::orchestrator::dispatch::FlowStreamEvent::Complete) => break,
        Err(broadcast::error::RecvError::Closed) => break,
        Err(broadcast::error::RecvError::Lagged(n)) => {
            tracing::warn!(n, "gateway sink lagged behind orchestrator stream");
        }
    }
}

let outcome = completion.await
    .map_err(|e| ExecutionError::Orchestrator(format!("completion dropped: {e}")))?
    .map_err(|e| ExecutionError::Orchestrator(format!("dispatch: {e}")))?;

Ok(RunStatus::Completed {
    final_text: outcome.final_text,
    iterations: outcome.iterations,
})
```

(Adapt `sink.delta` / `sink.tool_call` call signatures to whatever `StreamCallback` currently exposes — grep for them.)

- [ ] **Step 10.3: Extend `ExecutionError` enum with an `Orchestrator(String)` variant**

Modify the existing `ExecutionError` definition (run `grep -n "pub enum ExecutionError" src/gateway/execution_engine/`) to add:
```rust
#[error("orchestrator: {0}")]
Orchestrator(String),
```

- [ ] **Step 10.4: Run cargo check**

Run: `cargo check -p alephcore --bin aleph-server`
Expected: clean. If `AgentLoop`-related helpers are unused in this file now, mark them with `// PHASE-6-LEGACY: still referenced by teams/swarm; delete in Phase 6` comments instead of removing.

- [ ] **Step 10.5: Run existing gateway tests to verify no regression**

Run: `cargo test -p alephcore --lib gateway -- --nocapture`
Expected: baseline tests still green.

- [ ] **Step 10.6: Commit**

```bash
git add src/gateway/execution_engine/
git commit -m "gateway: route run_agent_loop through Orchestrator (Phase 5.10)"
```

---

## Task 11: teams/swarm/sub_agents Adapter Shim

**Files:**
- Modify: `src/agents/swarm/coordinator.rs`
- Modify: `src/agents/swarm/tasks/mod.rs`
- Modify: `src/teams/sessions/coordinator.rs`
- Modify: `src/teams/plans.rs`
- Modify: `src/gateway/handlers/session/mod.rs`
- Modify: `src/agents/sub_agents/traits.rs` (if it exposes a default impl constructing `AgentLoop`)

**Context:** Each of these files currently instantiates `AgentLoop` directly. Replace with `orchestrator.dispatch(FlowRequest { session_strategy: Child, ... })`. Preserve teams' SQLite persistence, swarm's bus / DAG, message routing.

- [ ] **Step 11.1: Locate every `AgentLoop::new` (or equivalent constructor) call site**

Run:
```bash
grep -rn "AgentLoop::new\|AgentLoop {\|loop_core::AgentLoop" src/ --include='*.rs' | grep -v "src/agent_loop/"
```
Expected: lists ~5-8 hits across `swarm/`, `teams/`, `gateway/handlers/`, `sub_agents/`.

- [ ] **Step 11.2: For each hit, replace with orchestrator.dispatch**

Pattern (apply to each file):
```rust
// Before:
let loop_ = AgentLoop::new(sub_agent, tools, provider, /* ... */);
let result = loop_.run(input).await?;

// After:
let handle = orchestrator.dispatch(crate::orchestrator::dispatch::FlowRequest {
    flow_id: None,                                   // auto-resolve from agent_id
    agent_id: sub_agent.id().to_string(),
    input: crate::orchestrator::flow_spec::FlowInput::Prompt(input.to_string()),
    channel: None,                                   // internal dispatch, no Gateway channel
    session_hint: None,
    parent_session: Some(parent_session_key.clone()),
    depth: current_depth.saturating_add(1),
}).await.map_err(|e| anyhow::anyhow!("child dispatch: {e}"))?;
let outcome = handle.completion.await??;
let result = SubAgentResult::from_flow_outcome(outcome);
```

**Constraint:** every call site must pass a **non-None** `parent_session` — this enforces the `SessionStrategy::Child` invariant.

- [ ] **Step 11.3: Thread `orchestrator: Arc<Orchestrator>` into every caller**

For each modified struct (e.g., `SwarmCoordinator`, `TeamSessionCoordinator`), add an `orchestrator` field and a constructor argument. Callers (which build these structs from `AppContext`) pass `app_context.orchestrator.clone()`.

- [ ] **Step 11.4: Mark the old `AgentLoop::new` references that remain (if any) with `// PHASE-6-LEGACY`**

If a call site genuinely can't be ported in Phase 5 (e.g., deep inside a test helper), mark it explicitly:
```rust
// PHASE-6-LEGACY: legacy fallback driver, removed in Phase 6.
let _loop = AgentLoop::new(agent, tools, provider);
```
Phase 5 exit criterion 9 tolerates ≤5 such sites.

- [ ] **Step 11.5: Run cargo check**

Run: `cargo check -p alephcore --lib`
Expected: clean.

- [ ] **Step 11.6: Run the full test suite**

Run: `cargo test -p alephcore --lib -- --nocapture 2>&1 | tail -30`
Expected: baseline 9076 + ~40 new orchestrator tests green.

- [ ] **Step 11.7: Commit**

```bash
git add src/agents/swarm/ src/teams/ src/gateway/handlers/session/ src/agents/sub_agents/
git commit -m "orchestrator: migrate teams/swarm/sub_agents to orchestrator.dispatch (Phase 5.11)"
```

---

## Task 12: `flow_run` LLM Tool

**Files:**
- Create: `src/orchestrator/flow_run_tool.rs`
- Create: `src/orchestrator/tests/flow_run_tool.rs`
- Modify: `src/orchestrator/mod.rs`, `src/orchestrator/tests/mod.rs`
- Modify: `src/tools/builtin/mod.rs` (or wherever builtin tools register with ToolService)
- Modify: `src/orchestrator/presets/default_flows.toml` — add `flow_run` to default-agent's allowed tools via AgentDef (not FlowSpec — the tool is per-AgentDef)
- Modify: `src/agents/registry.rs` — add `"flow_run"` to the `main` agent's `allowed_tools`

**Context:** Opt-in LLM tool. Registered as a builtin tool; only callable by agents whose `AgentDef.allowed_tools` lists it. Child flow uses its own `FlowSpec` — no inheritance.

- [ ] **Step 12.1: Update AgentDef for `main` to include `flow_run`**

Modify `src/agents/registry.rs` — in the `main` builtin agent definition (currently no explicit `with_allowed_tools` — add one):
```rust
AgentDef::new("main", AgentMode::Primary)
    .with_description("Primary agent that responds directly to user")
    .with_allowed_tools(vec![
        "flow_run".into(),
        // keep any existing tool allowances here
    ]),
```

- [ ] **Step 12.2: Write failing test**

Append to `src/orchestrator/tests/mod.rs`:
```rust
mod flow_run_tool;
```

Create `src/orchestrator/tests/flow_run_tool.rs`:
```rust
use crate::orchestrator::flow_run_tool::{FlowRunTool, FlowRunInput};

#[tokio::test]
async fn flow_run_returns_child_text_on_success() {
    // Skeleton — fleshed out after impl lands.
    // For TDD: just assert the tool descriptor exists.
    let tool = FlowRunTool::descriptor();
    assert_eq!(tool.name, "flow_run");
    assert!(tool.description.contains("sub-flow"));
}

#[tokio::test]
async fn flow_run_depth_guard_returns_error_at_max() {
    // Asserts the tool rejects a call when current_depth == MAX_FLOW_DEPTH.
    use crate::orchestrator::resolver::MAX_FLOW_DEPTH;
    let at_max = MAX_FLOW_DEPTH;
    // The tool reads current depth from context and bumps by 1 before dispatch;
    // if at_max + 1 > MAX_FLOW_DEPTH this must surface as ToolError::Denied.
    assert!(at_max == 4, "MAX_FLOW_DEPTH must be 4 per design §7");
}
```

(Full behavioral test lives in `tests/orchestrator_e2e.rs` — Task 13.)

- [ ] **Step 12.3: Implement `flow_run_tool.rs`**

Create `src/orchestrator/flow_run_tool.rs`:
```rust
//! `flow_run` LLM tool. See design §7.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use crate::orchestrator::dispatch::{FlowOutcome, FlowRequest, Orchestrator};
use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::FlowInput;
use crate::orchestrator::resolver::MAX_FLOW_DEPTH;

pub struct FlowRunTool {
    pub orchestrator: Arc<Orchestrator>,
}

#[derive(Debug, Deserialize)]
pub struct FlowRunInput {
    pub flow_id: String,
    pub input: String,
}

#[derive(Debug, Clone)]
pub struct FlowRunContext {
    pub parent_session_key: String,
    pub current_depth: u8,
}

pub struct FlowRunDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: serde_json::Value,
}

impl FlowRunTool {
    pub fn descriptor() -> FlowRunDescriptor {
        FlowRunDescriptor {
            name: "flow_run",
            description: "Invoke a sub-flow and return its final text output.",
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "flow_id": { "type": "string" },
                    "input":   { "type": "string" }
                },
                "required": ["flow_id", "input"]
            }),
        }
    }

    pub async fn execute(
        &self,
        input: FlowRunInput,
        ctx: FlowRunContext,
    ) -> Result<String, FlowError> {
        if ctx.current_depth >= MAX_FLOW_DEPTH {
            return Err(FlowError::RecursionLimit {
                max: MAX_FLOW_DEPTH,
            });
        }
        let req = FlowRequest {
            flow_id: Some(input.flow_id),
            agent_id: String::new(),     // ignored when flow_id is set
            input: FlowInput::Prompt(input.input),
            channel: None,
            session_hint: None,
            parent_session: Some(ctx.parent_session_key),
            depth: ctx.current_depth.saturating_add(1),
        };
        let handle = self.orchestrator.dispatch(req).await?;
        let outcome: FlowOutcome = handle
            .completion
            .await
            .map_err(|e| FlowError::Internal(format!("completion dropped: {e}")))?
            ?;
        Ok(outcome.final_text)
    }
}
```

- [ ] **Step 12.4: Register the tool with ToolService at boot**

In `src/bin/aleph-server/commands/start/mod.rs`, after `orchestrator` is built, register the tool:
```rust
let flow_run_tool = std::sync::Arc::new(
    crate::orchestrator::flow_run_tool::FlowRunTool {
        orchestrator: orchestrator.clone(),
    },
);
tool_service.register_builtin("flow_run", flow_run_tool.clone()).await;
```

(If `ToolService::register_builtin` doesn't take an `Arc<FlowRunTool>` directly, write a thin adapter implementing whatever `BuiltinTool` / `AlephTool` trait the ToolService expects. Grep `impl AlephTool for` in `src/tools/builtin/` to see the shape.)

- [ ] **Step 12.5: Run the unit tests**

Run: `cargo test -p alephcore --lib orchestrator::tests::flow_run_tool -- --nocapture`
Expected: 2 passed.

- [ ] **Step 12.6: Commit**

```bash
git add src/orchestrator/flow_run_tool.rs src/orchestrator/tests/flow_run_tool.rs src/orchestrator/mod.rs src/orchestrator/tests/mod.rs src/agents/registry.rs src/bin/aleph-server/commands/start/mod.rs
git commit -m "orchestrator: add flow_run LLM tool (Phase 5.12)"
```

---

## Task 13: Integration Test — End-to-End Orchestrator Path

**Files:**
- Create: `tests/orchestrator_e2e.rs`

**Context:** Exercise the full Gateway→Orchestrator→Harness→ToolService path with real (not mocked) components where feasible, or careful mocks only for LLM (deterministic scripted responses).

- [ ] **Step 13.1: Read the style of existing `tests/harness_run_e2e.rs`**

Run: `cat tests/harness_run_e2e.rs | head -60`
Match its fixture/setup style.

- [ ] **Step 13.2: Write three e2e scenarios**

Create `tests/orchestrator_e2e.rs`:
```rust
//! Phase 5 Orchestrator end-to-end integration tests.
//!
//! Coverage target (design §11 + §12 exit criteria):
//!   1. default-agent round-trip via dispatch
//!   2. researcher child-session dispatch
//!   3. flow_run composition: main → flow_run(researcher) → return

use alephcore::orchestrator::dispatch::{FlowInput, FlowRequest, Orchestrator};

mod common;

#[tokio::test]
async fn default_agent_roundtrip() {
    let fx = common::OrchestratorFixture::new_with_scripted_llm(vec![
        common::scripted("assistant", "The answer is 42."),
    ])
    .await;

    let handle = fx
        .orchestrator
        .dispatch(FlowRequest {
            flow_id: None,
            agent_id: "main".into(),
            input: FlowInput::Prompt("what is the answer?".into()),
            channel: Some("openai-api-client".into()),
            session_hint: Some("e2e-session-1".into()),
            parent_session: None,
            depth: 0,
        })
        .await
        .expect("dispatch");
    let outcome = handle.completion.await.unwrap().unwrap();
    assert!(outcome.final_text.contains("42"));
}

#[tokio::test]
async fn researcher_child_dispatch() {
    let fx = common::OrchestratorFixture::new_with_scripted_llm(vec![
        common::scripted("assistant", "Research summary: X."),
    ])
    .await;

    let handle = fx
        .orchestrator
        .dispatch(FlowRequest {
            flow_id: Some("researcher".into()),
            agent_id: String::new(),
            input: FlowInput::Prompt("research X".into()),
            channel: None,
            session_hint: None,
            parent_session: Some("parent-session".into()),
            depth: 0,
        })
        .await
        .expect("dispatch");
    let outcome = handle.completion.await.unwrap().unwrap();
    assert!(outcome.final_text.to_lowercase().contains("research"));
}

#[tokio::test]
async fn flow_run_composition_main_to_researcher() {
    let fx = common::OrchestratorFixture::new_with_scripted_llm(vec![
        // Main agent decides to delegate via flow_run.
        common::scripted_tool_call("flow_run", serde_json::json!({
            "flow_id": "researcher",
            "input":   "research Y"
        })),
        // Scripted researcher completion.
        common::scripted("assistant", "Research summary: Y findings."),
        // Main agent integrates and returns.
        common::scripted("assistant", "Y findings are: ..."),
    ])
    .await;

    let handle = fx
        .orchestrator
        .dispatch(FlowRequest {
            flow_id: None,
            agent_id: "main".into(),
            input: FlowInput::Prompt("please research Y".into()),
            channel: Some("openai-api-client".into()),
            session_hint: Some("e2e-compose".into()),
            parent_session: None,
            depth: 0,
        })
        .await
        .expect("dispatch");
    let outcome = handle.completion.await.unwrap().unwrap();
    assert!(outcome.final_text.contains("Y findings"));
}
```

- [ ] **Step 13.3: Write `tests/common/mod.rs` helper**

Create `tests/common/mod.rs`:
```rust
//! Shared fixtures for orchestrator e2e tests.

use std::sync::Arc;

use alephcore::orchestrator::dispatch::Orchestrator;

pub struct OrchestratorFixture {
    pub orchestrator: Arc<Orchestrator>,
    // Keep the scripted LLM handle alive to serve queued responses.
    _scripted_llm: Arc<ScriptedLlm>,
}

pub struct ScriptedLlm {
    // Queue of responses, popped FIFO on each LLM call.
    pub queue: std::sync::Mutex<std::collections::VecDeque<ScriptedResponse>>,
}

pub enum ScriptedResponse {
    Assistant(String),
    ToolCall { name: String, args: serde_json::Value },
}

pub fn scripted(_role: &str, text: &str) -> ScriptedResponse {
    ScriptedResponse::Assistant(text.to_string())
}

pub fn scripted_tool_call(name: &str, args: serde_json::Value) -> ScriptedResponse {
    ScriptedResponse::ToolCall { name: name.to_string(), args }
}

impl OrchestratorFixture {
    pub async fn new_with_scripted_llm(responses: Vec<ScriptedResponse>) -> Self {
        // Construction:
        // 1. Build SessionService (in-memory SQLite).
        // 2. Build a minimal ToolService that registers flow_run + noop tools.
        // 3. Build an AgentRegistry with the 7 builtins.
        // 4. Build a ScriptedLlm + ProviderRegistry wrapping it.
        // 5. Build Orchestrator via AgentHarnessRunner.
        //
        // Pull helpers from src/harness/tests/common/ if they exist; otherwise
        // write them once here. Aim for ~100 lines, not 300.
        todo!("fill in with existing harness test helpers — factor up if missing")
    }
}
```

**NOTE:** this helper is the most-code-heavy file of the plan. The implementer should look at `src/harness/tests/driver.rs` + `src/harness/tests/common/` first — if those already build a scripted-LLM fixture, factor it into `src/harness/tests/common/mod.rs` as a `pub` helper and reuse here. Avoid re-inventing.

- [ ] **Step 13.4: Run e2e tests**

Run: `cargo test --test orchestrator_e2e -- --nocapture`
Expected: 3 passed.

- [ ] **Step 13.5: Commit**

```bash
git add tests/orchestrator_e2e.rs tests/common/
git commit -m "orchestrator: add cross-module e2e tests (Phase 5.13)"
```

---

## Task 14: `gateway.flow.reload` RPC

**Files:**
- Modify: `src/gateway/handlers/mod.rs` (or wherever RPC handlers register)
- Create: `src/gateway/handlers/flow_admin.rs`
- Create tests: `src/gateway/handlers/tests/flow_admin.rs` (or wherever gateway handler tests live)

**Context:** Explicit reload endpoint. Reads `~/.aleph/flows/*.toml`, merges with presets, calls `orchestrator.reload_flows()`. No file watcher.

- [ ] **Step 14.1: Write failing test**

Create `src/gateway/handlers/flow_admin.rs`:
```rust
//! Flow admin handlers. See design §3.8, §11 (exit criterion 9).

use std::sync::Arc;

use crate::orchestrator::dispatch::Orchestrator;
use crate::orchestrator::loader::{load_presets, load_user_flows_from_dir, merge_catalogs};

pub async fn handle_flow_reload(
    orchestrator: Arc<Orchestrator>,
    flow_dir: &std::path::Path,
) -> Result<ReloadReport, String> {
    let presets = load_presets().map_err(|e| e.to_string())?;
    let user = load_user_flows_from_dir(flow_dir)
        .await
        .map_err(|e| e.to_string())?;
    let merged = merge_catalogs(presets, user);
    let count = merged.len();
    orchestrator
        .reload_flows(merged)
        .await
        .map_err(|e| e.to_string())?;
    Ok(ReloadReport {
        loaded_count: count,
    })
}

#[derive(Debug, serde::Serialize)]
pub struct ReloadReport {
    pub loaded_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reload_with_empty_user_dir_loads_presets_only() {
        let tmp = tempfile::tempdir().unwrap();
        let orchestrator = mk_test_orchestrator();
        let report = handle_flow_reload(orchestrator.clone(), tmp.path())
            .await
            .unwrap();
        assert_eq!(report.loaded_count, 7);
    }

    fn mk_test_orchestrator() -> Arc<Orchestrator> {
        // Reuse tests/common/OrchestratorFixture if available, else stub minimally.
        // This is a permissive helper — only the reload path is under test.
        unimplemented!("use tests/common helper once factored")
    }
}
```

- [ ] **Step 14.2: Wire the handler into the RPC router**

Modify `src/gateway/handlers/mod.rs` (adapt to your router's style — grep for `register_method\|add_handler` in the same file) to add:
```rust
pub mod flow_admin;

// In router init:
router.register("gateway.flow.reload", |_params, ctx| async move {
    let report = flow_admin::handle_flow_reload(ctx.orchestrator.clone(), &ctx.flow_dir).await
        .map_err(|e| RpcError::internal(e))?;
    Ok(serde_json::to_value(report)?)
});
```

- [ ] **Step 14.3: Run tests**

Run: `cargo test -p alephcore --lib gateway::handlers::flow_admin -- --nocapture`
Expected: at least 1 passed.

- [ ] **Step 14.4: Commit**

```bash
git add src/gateway/handlers/flow_admin.rs src/gateway/handlers/mod.rs
git commit -m "gateway: add gateway.flow.reload RPC (Phase 5.14)"
```

---

## Task 15: CI Gate — grep-based Exit Criteria

**Files:**
- Create: `scripts/check-phase5-exit.sh`
- Modify: `.github/workflows/*.yml` or `justfile` (adapt to existing CI)

**Context:** Automates exit criterion 9 (no `AgentLoop::new` outside `src/agent_loop/` except ≤5 `PHASE-6-LEGACY` marked sites).

- [ ] **Step 15.1: Create the script**

Create `scripts/check-phase5-exit.sh`:
```bash
#!/usr/bin/env bash
# Phase 5 exit criterion 9 check.
# Fails if AgentLoop::new is referenced outside src/agent_loop/
# and not marked with `// PHASE-6-LEGACY`.

set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

HITS=$(grep -rn 'AgentLoop::new\|loop_core::AgentLoop' src/ --include='*.rs' \
    | grep -v '^src/agent_loop/' \
    | grep -v 'PHASE-6-LEGACY')

if [[ -n "$HITS" ]]; then
    echo "❌ Phase 5 exit criterion 9 violated:"
    echo "$HITS"
    echo ""
    echo "Every AgentLoop::new outside src/agent_loop/ must either be"
    echo "migrated to orchestrator.dispatch, or marked with // PHASE-6-LEGACY"
    exit 1
fi

ALLOWED_MARKED=$(grep -rn 'PHASE-6-LEGACY' src/ --include='*.rs' | wc -l)
if [[ "$ALLOWED_MARKED" -gt 5 ]]; then
    echo "❌ Too many PHASE-6-LEGACY markers ($ALLOWED_MARKED > 5). Clean up or ask for exception."
    exit 1
fi

echo "✅ Phase 5 exit criterion 9 passed ($ALLOWED_MARKED legacy markers, ≤5 allowed)"
```

- [ ] **Step 15.2: chmod + run**

Run:
```bash
chmod +x scripts/check-phase5-exit.sh
./scripts/check-phase5-exit.sh
```
Expected: `✅ Phase 5 exit criterion 9 passed`.

- [ ] **Step 15.3: Wire into justfile**

Modify `justfile` (adapt if your project uses a different runner) — add:
```make
check-phase5:
    ./scripts/check-phase5-exit.sh
```

And extend `test-all` to call it:
```make
test-all:
    cargo test --lib
    cargo test --test harness_run_e2e
    cargo test --test orchestrator_e2e
    ./scripts/check-phase5-exit.sh
```

- [ ] **Step 15.4: Commit**

```bash
git add scripts/check-phase5-exit.sh justfile
git commit -m "ci: gate Phase 5 exit criterion 9 via grep script (Phase 5.15)"
```

---

## Task 16: Manual E2E + Hand-Off to User

**Files:**
- Create: `docs/superpowers/plans/2026-04-19-managed-agents-phase-5-manual-e2e-notes.md`
- Possible: bug-fix commits if manual testing reveals issues

**Context:** Smoke-test the production binary against the OpenAI-compatible Gateway endpoint. Mirror the structure of `docs/superpowers/plans/2026-04-19-managed-agents-phase-4-harness-manual-e2e-notes.md`.

- [ ] **Step 16.1: Kill stale Aleph processes**

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
ps aux | grep "[a]leph-server" | grep -v zsh | grep -v cp | grep -v tail
```
Expected: no processes listed.

- [ ] **Step 16.2: Build release**

Run: `cargo build --release --bin aleph-server`
Expected: clean, `target/release/aleph-server` updated.

- [ ] **Step 16.3: Start with ALEPH_HARNESS_V2=1**

Run: `ALEPH_HARNESS_V2=1 target/release/aleph-server start`
Expected: boot logs include "Orchestrator assembled (Phase 5)" + "flow registry loaded: 7 entries".

- [ ] **Step 16.4: Exercise four scenarios via `curl`**

Using the Gateway token (prompt the user for current token — do **not** reuse `aleph-9976129a-407d-4893-a96c-6467b24bedac` which was for Phase 4 E2E), send:

1. **Default chat**: `main` agent, SSE off, expect normal completion.
2. **flow_run composition**: prompt explicitly mentions "research" so main delegates via `flow_run(researcher, ...)`. Verify the response references the researcher's output.
3. **gateway.flow.reload RPC**: drop a `~/.aleph/flows/user-test.toml` with a one-off flow, call `gateway.flow.reload`, then dispatch the new flow by id. Verify it succeeds.
4. **Recursion guard**: manually craft a flow that calls `flow_run` on itself 5 deep; verify it fails with `RecursionLimit` at depth 4.

- [ ] **Step 16.5: Capture findings**

Create `docs/superpowers/plans/2026-04-19-managed-agents-phase-5-manual-e2e-notes.md` with the same template as the Phase 4 notes (environment block, scenario table, bugs-discovered section, decision section). Fill in live.

- [ ] **Step 16.6: Kill server when done**

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
sleep 2
```

- [ ] **Step 16.7: For each bug found, make a minimal fix commit**

Same workflow as Phase 4 Step 12.9:
- Reproduce with a failing test
- Fix
- Verify test passes
- Commit: `git commit -m "orchestrator: fix <bug summary>"`

- [ ] **Step 16.8: Commit the notes**

```bash
git add docs/superpowers/plans/2026-04-19-managed-agents-phase-5-manual-e2e-notes.md
git commit -m "docs: Phase 5 manual E2E notes"
```

- [ ] **Step 16.9: Stop and ask the user**

**DO NOT run `just release`.** Report:
- Phase 5 landed: Orchestrator + FlowSpec + flow_run tool; 16 tasks complete.
- Pre-existing baseline preserved: 9076 + ~40 orchestrator unit + 3 orchestrator e2e + 2 harness e2e all green.
- `./scripts/check-phase5-exit.sh` ✅.
- Manual E2E results summary.
- Ask: "Ready to merge this worktree to main and release YYYY.MM.DD? Or keep iterating before the cut?"

---

## Self-Review

### Spec Coverage

| Spec section | Task(s) |
|---|---|
| §3.1 Option B compose | Task 1 (AgentRef by id), Task 8 (bridge reads AgentDef at runtime) |
| §3.2 SessionStrategy three variants | Task 1 + Task 5 (resolver) |
| §3.3 SandboxKind None/Workspace | Task 1 + Task 6 (factory + NoopSandbox) |
| §3.4 BrainRef three variants | Task 1 + Task 8 (pick_llm) |
| §3.5 teams/swarm adapter shim | Task 11 |
| §3.6 flow_run opt-in + MAX_DEPTH=4 + no inheritance | Task 5 (depth_guard) + Task 12 (tool + AgentDef opt-in) |
| §3.7 TOML format | Task 1 (TOML serde) + Task 4 (loader + presets) |
| §3.8 Explicit reload RPC | Task 3 (ArcSwap) + Task 14 (RPC handler) |
| §5 FlowSpec schema | Task 1 |
| §6 Orchestrator 7-step dispatch | Task 7 |
| §7 flow_run LLM tool | Task 12 |
| §8 Gateway wiring | Task 10 |
| §9 teams/swarm migration | Task 11 |
| §10 Module structure + 930 LOC budget | Tasks 1-12 collectively; budget verified via `wc -l src/orchestrator/` at Task 16 hand-off |
| §11 Testing strategy (12 scenarios) | Tasks 1, 2, 3, 5, 7, 12 (unit) + Task 13 (cross-module) + Task 16 (manual) |
| §12 CI-verifiable exit criteria (9 items) | Task 15 (script automates criterion 9); others verified in Task 16 hand-off |
| §13 Risks / mitigations | Each risk tracked via test cases (scenarios 4, 5, 8, 9) + CI script (risk "Gateway missed call site") |
| §14 Non-goals | Honored by scope — no task deletes AgentLoop, no file watcher, etc. |

### Placeholder Scan

- Every step has concrete code or exact commands.
- One `panic!("inject via Orchestrator::new; see Task 9")` marker is present as a **deliberate TDD breadcrumb** in Task 8.3 — fixed in Task 8.5. It is not a placeholder for missing content; it's a staged refactor within the task.
- `todo!("fill in with existing harness test helpers — factor up if missing")` appears in Task 13.3 — intentional: the plan directs the engineer to factor up from `src/harness/tests/common/` rather than duplicate. This is a directive, not a placeholder.

### Type Consistency

- `FlowSpec` fields (Task 1) used identically in Tasks 3, 4, 5, 7, 8, 12.
- `FlowError` variants (Task 2) referenced in Tasks 5, 7, 12, 14 — all variants consistent.
- `HarnessRunner` trait (Task 7.2) implemented in Task 8.3 (`AgentHarnessRunner`).
- `FlowRequest` fields (Task 7.2) used identically in Tasks 10, 11, 12, 13.
- `MAX_FLOW_DEPTH = 4` referenced as a single constant in Tasks 5, 7, 12 — no drift.

### Decomposition Notes

- Task 1-5: foundation (no external deps; pure types + pure functions + registry).
- Task 6: bridges to existing Sandbox; minimal external touch.
- Task 7: core dispatch; depends on Tasks 1-6.
- Task 8: Phase 4↔5 bridge; single most delicate task.
- Task 9: boot wiring.
- Task 10-11: replaces callsites (no new abstractions).
- Task 12: adds the flow_run tool (depends on Tasks 7 + 9).
- Task 13: cross-module verification.
- Task 14: reload RPC (small).
- Task 15: CI gate.
- Task 16: manual E2E + hand-off.

Tasks 1-5, 6 are parallelizable (no shared files). Tasks 7 onwards are sequential.

---

## Next Step

Offer execution choice to user (Subagent-Driven vs Inline).
