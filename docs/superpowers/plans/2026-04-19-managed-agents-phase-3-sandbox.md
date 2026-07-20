# Managed-Agents Phase 3 — Sandbox + WorkspaceSandbox — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce `src/sandbox/` with a `Sandbox` trait and `WorkspaceSandbox` implementation; rename the existing `SandboxManager` to `OsSandboxDriver`; migrate exec-class tools to dispatch via `Arc<dyn Sandbox>`; backfill Phase 2's `SmartFilter` placeholder by wiring a real `LayeredPermissionResolver` against Aleph's existing two-tier (global + per-agent) tool permission system.

**Architecture:** Sandbox is orthogonal to Tools — exec tools opt-in by holding `Arc<dyn Sandbox>` in their constructor and calling `sandbox.execute(SandboxCommand)` instead of `Command::new(...)`. `WorkspaceSandbox` provisions `~/.aleph/workspaces/{session_id}/` lazily per session and drives macOS seatbelt through `OsSandboxDriver`. A tokio `task_local!` `SESSION_ID` threads session context without changing the `AlephTool` trait. Two-layer permission: tool-level via existing global+agent policy; capability-level via `ApprovalGate` single-request escalation.

**Tech Stack:** Rust 2024, tokio (task_local + RwLock + time::timeout), async_trait, thiserror, arc-swap, serde, sha2 (for `session_key_to_filename`), tracing.

**Source spec:** `docs/superpowers/specs/2026-04-19-sandbox-workspace-design.md` §10 steps 10.1–10.9.

---

## File Structure (after Phase 3)

**Created:**
- `src/sandbox/mod.rs` — `Sandbox` trait + re-exports
- `src/sandbox/command.rs` — `SandboxCommand`, `SandboxOutput`, `NetworkPolicy`, `SandboxError`
- `src/sandbox/capabilities.rs` — `SandboxCapabilities` + `is_within`
- `src/sandbox/context.rs` — `SESSION_ID` task-local + `current_session()`
- `src/sandbox/workspace.rs` — `WorkspaceSandbox` + `SessionWorkspace`
- `src/sandbox/driver.rs` — `OsSandboxDriverTrait` + `OsSandboxProfile`
- `src/tools/middleware/permission/resolver.rs` — `ToolPermissionResolver`, `LayeredPermissionResolver`, `SessionAgentResolver`
- `src/tools/middleware/permission/agent_filter.rs` — `AgentPermissionFilter` + `effective_trust`
- `docs/reference/SANDBOX.md` — new

**Renamed:**
- `src/exec/sandbox/executor.rs::SandboxManager` → `OsSandboxDriver` (implements `OsSandboxDriverTrait`)

**Modified:**
- `src/tools/middleware/permission.rs` — `SmartFilter::classify` becomes `async`; ScriptedFilter test mock updated
- `src/tools/facade.rs` — `build_tool_service` injects real `AgentPermissionFilter`
- `src/session/tool_trace.rs` — `invoke_with_session_trace` wraps `tool_svc.execute` in `SESSION_ID.scope(...)`
- `src/bin/aleph-server/commands/start/mod.rs` — `build_sandbox` + boot wiring
- ~5–10 exec-class tool files under `src/builtin_tools/` (exact list discovered in Task 8.1)
- `docs/reference/GLOSSARY.md` — Sandbox entry → present tense
- `docs/reference/ARCHITECTURE.md` — add SANDBOX.md cross-link
- `CHANGELOG.md` — `[Unreleased]` entries

---

## Pre-flight

- [ ] **Pre-1: Worktree setup**

Use the `EnterWorktree` tool with `name: "managed-agents-phase-3"`. If HEAD is stale (inherited session-start HEAD), fast-forward:
```bash
git merge main --ff-only
```
Confirm: `git log --oneline -3` shows `cffd9fcf6 docs: add Phase 3 Sandbox + WorkspaceSandbox design` at or near HEAD.

- [ ] **Pre-2: Baseline snapshot**

Run:
```bash
echo "=== Phase 3 baseline ===" > /tmp/phase3-baseline.txt
echo "-- Command::new callers in builtin_tools (target: 0 after Phase 3) --" >> /tmp/phase3-baseline.txt
grep -rn 'Command::new\|tokio::process::Command' src/builtin_tools/ >> /tmp/phase3-baseline.txt
echo "-- SandboxManager references (target: 0 after rename) --" >> /tmp/phase3-baseline.txt
grep -rn '\bSandboxManager\b' src/ >> /tmp/phase3-baseline.txt
echo "-- ScriptedFilter production usage (target: test-only after Task 2) --" >> /tmp/phase3-baseline.txt
grep -rn 'ScriptedFilter\|ScriptedApprover' src/ >> /tmp/phase3-baseline.txt
cat /tmp/phase3-baseline.txt
```
Record exact pre-rename counts. Task 10 diffs against this.

- [ ] **Pre-3: Baseline build**

Run: `cargo check -p alephcore 2>&1 | tail -3`
Expected: `Finished dev`

Run: `cargo test -p alephcore --lib 2>&1 | tail -5`
Expected: `test result: FAILED. 9029+ passed; 2 failed; ...` — the two pre-existing failures. Phase 3 must not introduce new failures.

**Foreground cargo only; `timeout: 600000`. No `run_in_background` for cargo — prior phases had background tasks killed prematurely.**

---

## Task 1: Discover the existing global + per-agent tool permission system

**Files:** none modified; produces a findings document for Task 2.

**Context:** Phase 2 shipped `SmartFilter` / `Approver` as placeholder traits because the subagent didn't find concrete types. Phase 3 Task 1 is a discovery-only task to locate the real implementations and confirm the two-tier structure (global + per-agent, each Deny/Confirm/Allow).

- [ ] **Step 1.1: Grep for permission type names**

```bash
grep -rnE 'enum\s+TrustLevel|enum\s+ToolPermission|pub\s+struct\s+AgentPermissions|pub\s+struct\s+GlobalToolPermissions|trust_level|require_confirmation' src/ 2>/dev/null | head -40
grep -rnE 'tool_permissions|allowed_tools|tool_whitelist|tool_blacklist|fn\s+is_tool_allowed|fn\s+classify_tool|fn\s+check_tool_permission' src/ 2>/dev/null | head -30
```

Record matches. Identify:
1. The `TrustLevel` (or equivalent) enum — its variants, where it's defined, what serde tag it uses
2. The struct holding **global** tool permissions — field name, serde key, where it's loaded from config
3. The struct holding **per-agent** tool permissions — same questions
4. The session → agent mapping path — grep for `agent_id`, `current_agent`, `active_agent`, or examine `SessionIdentityMeta`

- [ ] **Step 1.2: Verify the three-variant shape**

The user confirmed three tiers: 禁止 (Deny) / 询问 (Confirm) / 开启 (Allow). Confirm the discovered enum has exactly these variants (possibly named `Disabled/Full/Confirm`, `Deny/Ask/Allow`, or similar). If the enum has extra variants (e.g. a 4th "silent" or "trusted"), document and pick a collapse rule for Task 2.

- [ ] **Step 1.3: Inspect config loading**

Grep for where the global permissions are read from `aleph.toml` / runtime config. Identify:
- Is it stored as `Arc<ArcSwap<...>>`, `Arc<RwLock<...>>`, or plain?
- Is there a hot-reload path on config change?
- Does the agent config travel the same path?

- [ ] **Step 1.4: Produce a findings report** (inline, no commit)

Write a short markdown-ish findings block printed to stdout (no file yet):
```
=== Phase 3 Task 1 findings ===
TrustLevel enum:     path:line — variants: Allow / Confirm / Deny
Global permissions:  path:line — type: Foo — stored in: path
Agent permissions:   path:line — type: Bar — stored in: path
Session→agent:       path:line — via: ...
Hot-reload:          yes/no — mechanism: ...
Extra variants:      none / list them
```

This block drives Task 2's code. If any piece is genuinely absent (e.g. there's no global layer, only per-agent), flag it; Task 2 simplifies accordingly.

- [ ] **Step 1.5: No commit yet**

Task 1 produces knowledge only. Move directly to Task 2.

---

## Task 2: Phase 2 backfill — real permission resolver

**Files:**
- Modify: `src/tools/middleware/permission.rs` — make `SmartFilter::classify` async; update `ScriptedFilter`; add `.await` in `PermissionLayer::execute`
- Create: `src/tools/middleware/permission/resolver.rs`
- Create: `src/tools/middleware/permission/agent_filter.rs`
- Modify: `src/tools/facade.rs` — inject real filter

**Context:** Using Task 1's findings, connect the real two-tier permission data to Phase 2's `SmartFilter` trait surface. "Most-restrictive wins" combines global + agent into an effective `TrustLevel`, which then maps to `Classification`.

- [ ] **Step 2.1: Make `SmartFilter::classify` async**

Currently in `src/tools/middleware/permission.rs`:
```rust
pub trait SmartFilter: Send + Sync + 'static {
    fn classify(&self, tool_name: &str) -> Classification;
}
```

Change to:
```rust
#[async_trait::async_trait]
pub trait SmartFilter: Send + Sync + 'static {
    async fn classify(&self, tool_name: &str) -> Classification;
}
```

Update `ScriptedFilter` test mock to match (add `#[async_trait]` and `async fn`). Update `PermissionLayer::execute`:
```rust
match self.smart_filter.load_full().as_ref().as_ref() {
    Some(filter) => match filter.classify(name).await {
        Classification::Allow => {}
        Classification::Deny { reason } => return Err(ToolError::PermissionDenied { name: name.into(), reason }),
        Classification::Confirm { reason } => { /* existing approver logic */ }
    },
    None => {}
}
```

Run: `cargo check -p alephcore 2>&1 | tail -5` → `Finished dev`
Run: `cargo test -p alephcore --lib tools::middleware::permission 2>&1 | tail -15` → all 7 Phase 2 tests still pass.

- [ ] **Step 2.2: Create `src/tools/middleware/permission/resolver.rs`**

```rust
//! Two-tier tool permission resolver: global + per-agent, most-restrictive wins.

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;

use crate::session::service::SessionId;

// ADAPT these imports to the real types discovered in Task 1:
//   TrustLevel  — from config::types::... or similar
//   GlobalToolPermissions — the struct holding global map
//   AgentPermissions      — the struct holding per-agent map
//   AgentId               — identifier type for an agent
use crate::config::types::permissions::{
    TrustLevel, GlobalToolPermissions, AgentPermissions, AgentId,
};

#[async_trait]
pub trait SessionAgentResolver: Send + Sync + 'static {
    async fn agent_for(&self, session_id: &SessionId) -> AgentId;
}

#[async_trait]
pub trait ToolPermissionResolver: Send + Sync + 'static {
    async fn trust_for(&self, session_id: &SessionId, tool_name: &str) -> TrustLevel;
}

pub struct LayeredPermissionResolver {
    global:        Arc<ArcSwap<GlobalToolPermissions>>,
    session_agent: Arc<dyn SessionAgentResolver>,
    agents:        Arc<ArcSwap<HashMap<AgentId, AgentPermissions>>>,
    default_trust: TrustLevel,
}

impl LayeredPermissionResolver {
    pub fn new(
        global: Arc<ArcSwap<GlobalToolPermissions>>,
        session_agent: Arc<dyn SessionAgentResolver>,
        agents: Arc<ArcSwap<HashMap<AgentId, AgentPermissions>>>,
    ) -> Self {
        Self { global, session_agent, agents, default_trust: TrustLevel::Confirm }
    }
}

#[async_trait]
impl ToolPermissionResolver for LayeredPermissionResolver {
    async fn trust_for(&self, sid: &SessionId, tool: &str) -> TrustLevel {
        // ADAPT: replace `.get(tool)` calls with the real accessor method names
        // discovered in Task 1 (e.g. `.trust_for_tool(tool)` or similar).
        let global_trust = self.global.load().get(tool).unwrap_or(self.default_trust);
        let agent_id = self.session_agent.agent_for(sid).await;
        let agent_trust = self.agents.load()
            .get(&agent_id)
            .and_then(|p| p.get(tool))
            .unwrap_or(global_trust);
        effective_trust(global_trust, agent_trust)
    }
}

pub fn effective_trust(global: TrustLevel, agent: TrustLevel) -> TrustLevel {
    match (global, agent) {
        (TrustLevel::Deny, _)    | (_, TrustLevel::Deny)    => TrustLevel::Deny,
        (TrustLevel::Confirm, _) | (_, TrustLevel::Confirm) => TrustLevel::Confirm,
        _ => TrustLevel::Allow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_trust_3x3_matrix() {
        use TrustLevel::*;
        // most-restrictive-wins matrix
        assert_eq!(effective_trust(Allow, Allow),   Allow);
        assert_eq!(effective_trust(Allow, Confirm), Confirm);
        assert_eq!(effective_trust(Allow, Deny),    Deny);
        assert_eq!(effective_trust(Confirm, Allow),   Confirm);
        assert_eq!(effective_trust(Confirm, Confirm), Confirm);
        assert_eq!(effective_trust(Confirm, Deny),    Deny);
        assert_eq!(effective_trust(Deny, Allow),   Deny);
        assert_eq!(effective_trust(Deny, Confirm), Deny);
        assert_eq!(effective_trust(Deny, Deny),    Deny);
    }
}
```

If Task 1 found the discovered types have different variant names (e.g., `Full` instead of `Allow`), replace `TrustLevel::Allow` etc. in the matrix with the real variants. The matrix still must cover all 9 combinations.

- [ ] **Step 2.3: Create `src/tools/middleware/permission/agent_filter.rs`**

```rust
//! Adapter that plugs LayeredPermissionResolver into Phase 2's SmartFilter trait.

use std::sync::Arc;

use async_trait::async_trait;

use crate::sandbox::context::current_session;  // defined in Task 5
use crate::session::service::SessionId;
use crate::tools::middleware::permission::resolver::ToolPermissionResolver;
use crate::tools::middleware::permission::{Classification, SmartFilter};

// ADAPT: TrustLevel path from Task 1 findings
use crate::config::types::permissions::TrustLevel;

pub struct AgentPermissionFilter {
    resolver: Arc<dyn ToolPermissionResolver>,
}

impl AgentPermissionFilter {
    pub fn new(resolver: Arc<dyn ToolPermissionResolver>) -> Self {
        Self { resolver }
    }
}

#[async_trait]
impl SmartFilter for AgentPermissionFilter {
    async fn classify(&self, tool_name: &str) -> Classification {
        let sid = current_session().unwrap_or_else(SessionId::default_ephemeral);
        let trust = self.resolver.trust_for(&sid, tool_name).await;
        match trust {
            TrustLevel::Allow => Classification::Allow,
            TrustLevel::Confirm => Classification::Confirm {
                reason: format!("agent/global policy requires confirmation for '{tool_name}'"),
            },
            TrustLevel::Deny => Classification::Deny {
                reason: format!("tool '{tool_name}' is disabled by policy"),
            },
        }
    }
}
```

**Dependency note:** `current_session()` doesn't exist yet — it's defined in Task 5. For now, this file won't compile until Task 5. **Workaround for Task 2:** use a local helper `fn _current_session_placeholder() -> Option<SessionId> { None }` (with a `TODO(Phase 3 Task 5): replace with crate::sandbox::context::current_session` comment) so this file compiles; Task 5 will replace that helper with the real import. `SessionId::default_ephemeral` may also not exist — if not, use `SessionId::Ephemeral { agent_id: "default".into() }` or whatever constructor Phase 0 established.

- [ ] **Step 2.4: Register the new sub-modules**

Edit `src/tools/middleware/permission.rs` (or `permission/mod.rs` if you restructure to a directory) to add:
```rust
pub mod resolver;
pub mod agent_filter;

pub use resolver::{LayeredPermissionResolver, SessionAgentResolver, ToolPermissionResolver, effective_trust};
pub use agent_filter::AgentPermissionFilter;
```

If `permission.rs` was a single file, convert to a directory layout: rename `permission.rs` → `permission/mod.rs`, then add `resolver.rs` and `agent_filter.rs` alongside.

- [ ] **Step 2.5: Wire into `build_tool_service`**

Edit `src/tools/facade.rs`. Change signature to accept the three new resolver deps (or read them from `AppConfig`):

```rust
pub fn build_tool_service(
    server: Arc<crate::tools::server::AlephToolServer>,
    config: &ToolServiceConfig,
    global_perms: Arc<ArcSwap<GlobalToolPermissions>>,
    agents_perms: Arc<ArcSwap<HashMap<AgentId, AgentPermissions>>>,
    session_agent: Arc<dyn SessionAgentResolver>,
    approver: Arc<dyn Approver>,
) -> (Arc<dyn ToolService>, Arc<ToolRegistry>) {
    let registry = Arc::new(ToolRegistry::new());
    register_builtins_into(&registry, &server);
    let core    = Arc::new(CoreDispatch::new(registry.clone()));
    let timeout = Arc::new(TimeoutLayer::new(core, config.default_timeout(), config.per_tool_durations()));
    let ctxrule = Arc::new(ContextRuleLayer::new(timeout));
    let resolver = Arc::new(LayeredPermissionResolver::new(global_perms, session_agent, agents_perms));
    let filter   = Arc::new(AgentPermissionFilter::new(resolver));
    let perm     = Arc::new(PermissionLayer::with_policy(ctxrule, filter, approver));
    let audit    = Arc::new(ExecAuditLayer::new(perm));
    (audit, registry)
}
```

Update call sites in `src/bin/aleph-server/commands/start/mod.rs` to pass the new args. Use existing `ApprovalGate`-backed `Approver` (from Phase 2 Task 7's blanket impl).

- [ ] **Step 2.6: Build + test**

Run: `cargo check -p alephcore 2>&1 | tail -5` → `Finished dev`
Run: `cargo test -p alephcore --lib tools::middleware::permission 2>&1 | tail -15` → all prior tests plus `effective_trust_3x3_matrix` pass.

- [ ] **Step 2.7: Commit**

```bash
git add src/tools/ src/bin/aleph-server/
git commit -m "tools: wire real agent + global permission resolver (Phase 2 backfill)

Phase 3 Task 2: LayeredPermissionResolver queries both global and
per-agent permission tables; AgentPermissionFilter adapts it to the
SmartFilter trait (now async). 3x3 effective_trust matrix test verifies
most-restrictive-wins. ScriptedFilter stays as test-only mock; production
path now uses AgentPermissionFilter via build_tool_service."
```

---

## Task 3: Sandbox module scaffold

**Files:**
- Create: `src/sandbox/mod.rs`
- Create: `src/sandbox/command.rs`
- Create: `src/sandbox/capabilities.rs`
- Create: `src/sandbox/context.rs` (stub; real task_local in Task 5)
- Create: `src/sandbox/workspace.rs` (stub)
- Create: `src/sandbox/driver.rs`
- Modify: `src/lib.rs` — register `pub mod sandbox;`

**Context:** Type scaffold only. No runtime logic in workspace or driver; stubs so `cargo check` passes.

- [ ] **Step 3.1: Create `src/sandbox/capabilities.rs`**

```rust
//! SandboxCapabilities — what a command is allowed to do inside the sandbox.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SandboxCapabilities {
    #[serde(default)]
    pub fs_read: Vec<PathBuf>,
    #[serde(default)]
    pub fs_write: Vec<PathBuf>,
    #[serde(default)]
    pub network: NetworkPolicy,
    #[serde(default)]
    pub spawn_subprocess: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NetworkPolicy {
    None,
    AllowAll,
    AllowHosts { hosts: Vec<String> },
}

impl Default for NetworkPolicy {
    fn default() -> Self { Self::None }
}

impl SandboxCapabilities {
    /// Baseline: read/write within workspace cwd, no network, no subprocess spawn.
    pub fn strict() -> Self { Self::default() }

    /// Is `self` ⊆ `baseline` (fs subset; Network ordered None ⊆ AllowHosts ⊆ AllowAll; spawn monotonic)?
    pub fn is_within(&self, baseline: &Self) -> bool {
        let fs_read_ok = self.fs_read.iter().all(|p| baseline.fs_read.iter().any(|b| p.starts_with(b)));
        let fs_write_ok = self.fs_write.iter().all(|p| baseline.fs_write.iter().any(|b| p.starts_with(b)));
        let net_ok = network_within(&self.network, &baseline.network);
        let spawn_ok = !self.spawn_subprocess || baseline.spawn_subprocess;
        fs_read_ok && fs_write_ok && net_ok && spawn_ok
    }
}

fn network_within(child: &NetworkPolicy, baseline: &NetworkPolicy) -> bool {
    use NetworkPolicy::*;
    match (child, baseline) {
        (None, _) => true,
        (_, AllowAll) => true,
        (AllowAll, _) => false,
        (AllowHosts { hosts: c }, AllowHosts { hosts: b }) => c.iter().all(|h| b.contains(h)),
        (AllowHosts { .. }, None) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_is_default() {
        let s = SandboxCapabilities::strict();
        assert!(s.fs_read.is_empty());
        assert!(s.fs_write.is_empty());
        assert_eq!(s.network, NetworkPolicy::None);
        assert!(!s.spawn_subprocess);
    }

    #[test]
    fn empty_is_within_anything() {
        let baseline = SandboxCapabilities {
            fs_read: vec!["/tmp".into()],
            ..Default::default()
        };
        assert!(SandboxCapabilities::default().is_within(&baseline));
    }

    #[test]
    fn network_allowall_not_within_none() {
        let child = SandboxCapabilities { network: NetworkPolicy::AllowAll, ..Default::default() };
        let baseline = SandboxCapabilities::strict();
        assert!(!child.is_within(&baseline));
    }

    #[test]
    fn network_none_within_allowall() {
        let child = SandboxCapabilities::strict();
        let baseline = SandboxCapabilities { network: NetworkPolicy::AllowAll, ..Default::default() };
        assert!(child.is_within(&baseline));
    }

    #[test]
    fn spawn_subprocess_monotonic() {
        let child_spawns = SandboxCapabilities { spawn_subprocess: true, ..Default::default() };
        let baseline_no = SandboxCapabilities::strict();
        let baseline_yes = SandboxCapabilities { spawn_subprocess: true, ..Default::default() };
        assert!(!child_spawns.is_within(&baseline_no));
        assert!(child_spawns.is_within(&baseline_yes));
    }

    #[test]
    fn fs_write_subset_by_prefix() {
        let child = SandboxCapabilities { fs_write: vec!["/tmp/foo/bar".into()], ..Default::default() };
        let baseline = SandboxCapabilities { fs_write: vec!["/tmp/foo".into()], ..Default::default() };
        assert!(child.is_within(&baseline));
    }
}
```

- [ ] **Step 3.2: Create `src/sandbox/command.rs`**

```rust
//! SandboxCommand, SandboxOutput, SandboxError.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::sandbox::capabilities::SandboxCapabilities;
use crate::session::service::SessionId;

#[derive(Debug, Clone)]
pub struct SandboxCommand {
    pub session_id: SessionId,
    pub program: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub stdin: Option<Vec<u8>>,
    pub cwd: Option<PathBuf>,
    pub capabilities: SandboxCapabilities,
    pub timeout: Option<Duration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub truncated: bool,
    pub duration_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("capability denied: {reason}")]
    CapabilityDenied { reason: String },

    #[error("seatbelt profile generation failed: {0}")]
    ProfileGeneration(String),

    #[error("io error: {0}")]
    Io(String),

    #[error("timeout after {elapsed_ms}ms")]
    Timeout { elapsed_ms: u64 },

    #[error("{0}")]
    Other(String),
}
```

- [ ] **Step 3.3: Create `src/sandbox/context.rs` (stub)**

```rust
//! Task-local SESSION_ID (Task 5 turns this into the real task_local).

use crate::session::service::SessionId;

/// Placeholder — replaced with tokio::task_local! in Task 5.
pub fn current_session() -> Option<SessionId> {
    None
}
```

- [ ] **Step 3.4: Create `src/sandbox/driver.rs`**

```rust
//! OsSandboxDriverTrait — the seam between WorkspaceSandbox and OS-level seatbelt.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;

use crate::sandbox::capabilities::SandboxCapabilities;
use crate::sandbox::command::{SandboxError, SandboxOutput};

/// OS-specific seatbelt / sandbox-exec profile bytes or handle.
/// Opaque to WorkspaceSandbox.
#[derive(Debug, Clone)]
pub struct OsSandboxProfile {
    pub contents: String,   // macOS: sandbox-exec SBPL profile text
}

#[async_trait]
pub trait OsSandboxDriverTrait: Send + Sync + 'static {
    fn profile_for(
        &self,
        capabilities: &SandboxCapabilities,
        cwd: &Path,
    ) -> Result<OsSandboxProfile, SandboxError>;

    #[allow(clippy::too_many_arguments)]
    async fn run(
        &self,
        program: &str,
        args: &[String],
        env: &HashMap<String, String>,
        stdin: Option<&[u8]>,
        cwd: &Path,
        profile: &OsSandboxProfile,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<SandboxOutput, SandboxError>;
}
```

- [ ] **Step 3.5: Create `src/sandbox/workspace.rs` (stub)**

```rust
//! WorkspaceSandbox — lazy per-session workspace directory (Task 6 fills in).

//! Real impl in Task 6.
```

- [ ] **Step 3.6: Create `src/sandbox/mod.rs`**

```rust
//! Sandbox — "where to execute" abstraction, orthogonal to Tools.
//!
//! Exec-class tools hold Arc<dyn Sandbox> and call sandbox.execute(cmd)
//! instead of Command::new(...). WorkspaceSandbox provisions
//! ~/.aleph/workspaces/{session_id}/ lazily and drives macOS seatbelt
//! through OsSandboxDriver.
//!
//! See: docs/superpowers/specs/2026-04-19-sandbox-workspace-design.md

use async_trait::async_trait;

pub mod capabilities;
pub mod command;
pub mod context;
pub mod driver;
pub mod workspace;

pub use capabilities::{NetworkPolicy, SandboxCapabilities};
pub use command::{SandboxCommand, SandboxError, SandboxOutput};
pub use context::current_session;
pub use driver::{OsSandboxDriverTrait, OsSandboxProfile};

#[async_trait]
pub trait Sandbox: Send + Sync + 'static {
    async fn execute(
        &self,
        command: SandboxCommand,
    ) -> Result<SandboxOutput, SandboxError>;
}
```

- [ ] **Step 3.7: Register in `src/lib.rs`**

Find existing `pub mod` grouping; add `pub mod sandbox;` alphabetically or at the grouping's end.

- [ ] **Step 3.8: Build**

Run: `cargo check -p alephcore 2>&1 | tail -5` → `Finished dev`.
Run: `cargo test -p alephcore --lib sandbox::capabilities 2>&1 | tail -10` → 6 tests pass.

- [ ] **Step 3.9: Commit**

```bash
git add src/sandbox/ src/lib.rs
git commit -m "sandbox: add module scaffold — trait + types + capabilities

Phase 3 Task 3: Sandbox trait, SandboxCommand/Output/Error, Capabilities
with is_within set containment. context.rs and workspace.rs are stubs
(filled in Tasks 5-6). OsSandboxDriverTrait separates WorkspaceSandbox
from the OS-level driver (renamed in Task 4)."
```

---

## Task 4: Rename `SandboxManager` → `OsSandboxDriver` + implement trait

**Files:**
- Rename: `src/exec/sandbox/executor.rs` keeps its path, but its `SandboxManager` type is renamed
- Modify: every caller of `SandboxManager` — discovered via grep

**Context:** The existing struct's name conflicts with the new `Sandbox` trait. Rename preserves behavior exactly but restructures so `profile_for` / `run` are trait-method impls.

- [ ] **Step 4.1: Inspect existing `SandboxManager`**

```bash
head -80 src/exec/sandbox/executor.rs
grep -rn '\bSandboxManager\b' src/
```
Record current methods and fields. You're refactoring in place, not rewriting.

- [ ] **Step 4.2: Rename the struct**

In `src/exec/sandbox/executor.rs`:
1. `pub struct SandboxManager { ... }` → `pub struct OsSandboxDriver { ... }`
2. `impl SandboxManager { ... }` → `impl OsSandboxDriver { ... }`
3. Module-level docstring updated:
   ```rust
   //! OsSandboxDriver — OS-level sandbox-exec profile driver (macOS).
   //!
   //! Consumed by WorkspaceSandbox in src/sandbox/. Do NOT confuse with
   //! src/sandbox/mod.rs::Sandbox (the agent-level trait).
   ```

- [ ] **Step 4.3: Update all references via sed**

```bash
grep -rl '\bSandboxManager\b' src/ | while read f; do
  sed -i '' 's/\bSandboxManager\b/OsSandboxDriver/g' "$f"
done
grep -rn '\bSandboxManager\b' src/
```
Last grep should return zero output.

- [ ] **Step 4.4: Implement `OsSandboxDriverTrait`**

Move the existing profile-generation and execute logic into the trait impl. The existing inherent methods likely have different signatures; adapt:

```rust
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use crate::sandbox::capabilities::SandboxCapabilities;
use crate::sandbox::command::{SandboxError, SandboxOutput};
use crate::sandbox::driver::{OsSandboxDriverTrait, OsSandboxProfile};

#[async_trait]
impl OsSandboxDriverTrait for OsSandboxDriver {
    fn profile_for(
        &self,
        capabilities: &SandboxCapabilities,
        cwd: &Path,
    ) -> Result<OsSandboxProfile, SandboxError> {
        // Call existing inherent profile generation.
        // The existing method likely returns String or the project's own profile type;
        // wrap into OsSandboxProfile.
        let contents = self.generate_profile_from_capabilities(capabilities, cwd)
            .map_err(|e| SandboxError::ProfileGeneration(e.to_string()))?;
        Ok(OsSandboxProfile { contents })
    }

    async fn run(
        &self,
        program: &str,
        args: &[String],
        env: &HashMap<String, String>,
        stdin: Option<&[u8]>,
        cwd: &Path,
        profile: &OsSandboxProfile,
        timeout: Duration,
        max_output_bytes: usize,
    ) -> Result<SandboxOutput, SandboxError> {
        // Call the existing execute_sandboxed equivalent. Map its return type
        // to SandboxOutput.
        self.execute_with_profile(program, args, env, stdin, cwd, &profile.contents, timeout, max_output_bytes)
            .await
            .map_err(|e| match e {
                // ADAPT: match on the actual error types of the existing method
                _ => SandboxError::Other(e.to_string()),
            })
    }
}
```

If the existing methods don't accept all these parameters (e.g., they don't take `env` or `stdin`), extend them minimally — add the missing fields as parameters. This is NOT a rewrite; keep the macOS seatbelt + subprocess logic exactly as it was. If the existing `generate_profile` accepts `&FallbackPolicy` or some other type, translate from `SandboxCapabilities` at this boundary.

- [ ] **Step 4.5: Build + test**

Run: `cargo check -p alephcore 2>&1 | tail -5` → `Finished dev`
Run: `cargo test -p alephcore --lib exec::sandbox 2>&1 | tail -15` → existing tests pass under the new name.

- [ ] **Step 4.6: Commit**

```bash
git add src/
git commit -m "exec: rename SandboxManager to OsSandboxDriver + impl OsSandboxDriverTrait

Phase 3 Task 4: mechanical rename across src/ (zero behavior change).
OsSandboxDriver now implements OsSandboxDriverTrait from src/sandbox/
so WorkspaceSandbox can drive it through the trait without type coupling."
```

---

## Task 5: Task-local `SESSION_ID` + extend `invoke_with_session_trace`

**Files:**
- Modify: `src/sandbox/context.rs` — replace stub with real task_local
- Modify: `src/session/tool_trace.rs` — wrap `tool_svc.execute` in `SESSION_ID.scope(...)`
- Modify: `src/tools/middleware/permission/agent_filter.rs` — replace placeholder `current_session` with real import

- [ ] **Step 5.1: Replace `src/sandbox/context.rs` stub**

```rust
//! Task-local SESSION_ID — per-invocation session context for exec-class tools.
//!
//! Agent_loop's invoke_with_session_trace wraps tool dispatch in
//! SESSION_ID.scope(sid, ...).await. Exec-class tools read via current_session()
//! inside their AlephTool::call() implementation.
//!
//! IMPORTANT: tokio::spawn does NOT inherit task-locals. Use
//! SESSION_ID.sync_scope(sid.clone(), fut) when spawning subtasks that
//! need the context.

use crate::session::service::SessionId;

tokio::task_local! {
    pub static SESSION_ID: SessionId;
}

pub fn current_session() -> Option<SessionId> {
    SESSION_ID.try_with(|sid| sid.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ADAPT: use whatever SessionKey constructor is in this codebase.
    fn sample_sid() -> SessionId {
        crate::routing::session_key::SessionKey::ephemeral("test")
    }

    #[tokio::test]
    async fn out_of_scope_returns_none() {
        assert!(current_session().is_none());
    }

    #[tokio::test]
    async fn in_scope_returns_some() {
        let sid = sample_sid();
        SESSION_ID.scope(sid.clone(), async {
            assert_eq!(current_session(), Some(sid.clone()));
        }).await;
    }

    #[tokio::test]
    async fn nested_scope_inner_wins() {
        let outer = crate::routing::session_key::SessionKey::ephemeral("outer");
        let inner = crate::routing::session_key::SessionKey::ephemeral("inner");
        SESSION_ID.scope(outer.clone(), async {
            SESSION_ID.scope(inner.clone(), async {
                assert_eq!(current_session(), Some(inner.clone()));
            }).await;
            assert_eq!(current_session(), Some(outer.clone()));
        }).await;
    }

    #[tokio::test]
    async fn spawned_subtask_loses_context() {
        let sid = sample_sid();
        let handle = SESSION_ID.scope(sid.clone(), async {
            tokio::spawn(async { current_session() }).await.unwrap()
        }).await;
        assert!(handle.is_none());
    }
}
```

- [ ] **Step 5.2: Run task-local tests**

Run: `cargo test -p alephcore --lib sandbox::context 2>&1 | tail -15` → 4 tests pass.

- [ ] **Step 5.3: Extend `invoke_with_session_trace` in `src/session/tool_trace.rs`**

Add the `SESSION_ID.scope` wrapper around the inner `tool_svc.execute`:

```rust
use crate::sandbox::context::SESSION_ID;

pub async fn invoke_with_session_trace(
    tool_svc: &Arc<dyn ToolService>,
    session_svc: &Arc<dyn SessionService>,
    session_id: &SessionId,
    turn_id: TurnId,
    call_id: String,
    name: String,
    input: Value,
) -> Result<ToolOutput, ToolError> {
    let _ = session_svc.emit_event(session_id, SessionEvent::ToolCallRequested {
        turn_id, call_id: call_id.clone(), name: name.clone(),
        input: input.clone(), at: now_ms(),
    }).await;

    // Scope SESSION_ID so exec tools can read it via current_session().
    let result = SESSION_ID
        .scope(session_id.clone(), tool_svc.execute(&name, input))
        .await;

    match &result {
        Ok(output) => {
            let _ = session_svc.emit_event(session_id, SessionEvent::ToolResult {
                turn_id, call_id, output: output.clone(), at: now_ms(),
            }).await;
        }
        Err(ToolError::PermissionDenied { reason, .. }) => {
            let _ = session_svc.emit_event(session_id, SessionEvent::ToolCallDenied {
                turn_id, call_id, reason: reason.clone(), at: now_ms(),
            }).await;
        }
        Err(e) => {
            let _ = session_svc.emit_event(session_id, SessionEvent::ToolError {
                turn_id, call_id, error: e.to_string(), at: now_ms(),
            }).await;
        }
    }
    result
}
```

Update `tool_trace.rs`'s existing tests if they assert on the inner future's shape — they should still pass because `SESSION_ID.scope` returns the future's output unchanged.

- [ ] **Step 5.4: Replace `current_session` placeholder in `agent_filter.rs`**

Edit `src/tools/middleware/permission/agent_filter.rs` — remove the TODO and local placeholder, use `use crate::sandbox::context::current_session;` (already imported per Task 2.3).

- [ ] **Step 5.5: Build + test**

Run: `cargo check -p alephcore 2>&1 | tail -5` → `Finished dev`
Run: `cargo test -p alephcore --lib session::tool_trace sandbox::context 2>&1 | tail -15` → all pass.

- [ ] **Step 5.6: Commit**

```bash
git add src/sandbox/context.rs src/session/tool_trace.rs src/tools/middleware/permission/agent_filter.rs
git commit -m "sandbox: SESSION_ID task-local + tool_trace integration

Phase 3 Task 5: tokio::task_local! for session context; agent_loop's
invoke_with_session_trace scopes the task-local around tool dispatch so
exec-class tools and AgentPermissionFilter both read it via
current_session(). Tests verify scope get/set, nested scopes, and that
spawned subtasks lose context (documented, intentional)."
```

---

## Task 6: `WorkspaceSandbox` implementation

**Files:**
- Modify: `src/sandbox/workspace.rs` — replace stub with full impl
- Modify: `Cargo.toml` — add `sha2 = "0.10"` to `[dependencies]` (for `session_key_to_filename` hash)

- [ ] **Step 6.1: Ensure sha2 dep**

```bash
grep -n '^sha2' Cargo.toml
```
If absent, add `sha2 = "0.10"` to `[dependencies]` (Aleph might already have it for other hashing; check first).

- [ ] **Step 6.2: Implement `src/sandbox/workspace.rs`**

```rust
//! WorkspaceSandbox — lazy per-session workspace directory + capability enforcement.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::agent_loop::exec_approval::gate::{ApprovalGate, ApprovalOutcome};
use crate::sandbox::capabilities::SandboxCapabilities;
use crate::sandbox::command::{SandboxCommand, SandboxError, SandboxOutput};
use crate::sandbox::driver::OsSandboxDriverTrait;
use crate::sandbox::Sandbox;
use crate::session::service::SessionId;

pub struct WorkspaceSandbox {
    workspace_root: PathBuf,
    sessions: Arc<RwLock<HashMap<SessionId, Arc<SessionWorkspace>>>>,
    os_driver: Arc<dyn OsSandboxDriverTrait>,
    approval_gate: Arc<ApprovalGate>,
    default_timeout: Duration,
    max_output_bytes: usize,
}

struct SessionWorkspace {
    session_id: SessionId,
    cwd: PathBuf,
    baseline: SandboxCapabilities,
    granted_elevations: RwLock<HashSet<SandboxCapabilities>>,
}

impl WorkspaceSandbox {
    pub fn new(
        workspace_root: PathBuf,
        os_driver: Arc<dyn OsSandboxDriverTrait>,
        approval_gate: Arc<ApprovalGate>,
    ) -> Self {
        Self {
            workspace_root,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            os_driver,
            approval_gate,
            default_timeout: Duration::from_secs(60),
            max_output_bytes: 1024 * 1024, // 1 MB total budget
        }
    }

    pub fn with_timeout(mut self, t: Duration) -> Self { self.default_timeout = t; self }
    pub fn with_max_output_bytes(mut self, n: usize) -> Self { self.max_output_bytes = n; self }

    async fn for_session(&self, sid: &SessionId) -> Result<Arc<SessionWorkspace>, SandboxError> {
        if let Some(ws) = self.sessions.read().await.get(sid).cloned() { return Ok(ws); }
        let mut sessions = self.sessions.write().await;
        if let Some(ws) = sessions.get(sid).cloned() { return Ok(ws); }

        let cwd = self.workspace_root.join(session_key_to_filename(sid));
        tokio::fs::create_dir_all(&cwd)
            .await
            .map_err(|e| SandboxError::Io(format!("create workspace dir: {e}")))?;

        let ws = Arc::new(SessionWorkspace {
            session_id: sid.clone(),
            cwd,
            baseline: SandboxCapabilities::strict(),
            granted_elevations: RwLock::new(HashSet::new()),
        });
        sessions.insert(sid.clone(), ws.clone());
        Ok(ws)
    }
}

fn session_key_to_filename(sid: &SessionId) -> String {
    let json = serde_json::to_string(sid).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..16])
}

#[async_trait]
impl Sandbox for WorkspaceSandbox {
    async fn execute(&self, cmd: SandboxCommand) -> Result<SandboxOutput, SandboxError> {
        // 1. Resolve session workspace
        let ws = self.for_session(&cmd.session_id).await?;

        // 2. Resolve cwd (None → root; Some(path) must start with root)
        let cwd = match &cmd.cwd {
            None => ws.cwd.clone(),
            Some(p) if p.starts_with(&ws.cwd) => p.clone(),
            Some(_) => {
                return Err(SandboxError::CapabilityDenied {
                    reason: "cwd outside workspace root".into(),
                });
            }
        };

        // 3. Capability check
        if !cmd.capabilities.is_within(&ws.baseline) {
            let already_granted = {
                let granted = ws.granted_elevations.read().await;
                granted.iter().any(|g| cmd.capabilities.is_within(g))
            };
            if !already_granted {
                let reason = format_capability_request(&cmd.program, &cmd.capabilities);
                let outcome = self.approval_gate
                    .request_approval_for_tool(&cmd.program, &reason)
                    .await;
                match outcome {
                    ApprovalOutcome::Approved => {
                        ws.granted_elevations.write().await.insert(cmd.capabilities.clone());
                    }
                    ApprovalOutcome::Denied | ApprovalOutcome::Timeout => {
                        return Err(SandboxError::CapabilityDenied {
                            reason: "user denied elevated capability request".into(),
                        });
                    }
                }
            }
        }

        // 4. Generate OS-level profile
        let profile = self.os_driver.profile_for(&cmd.capabilities, &cwd)?;

        // 5. Run
        let timeout = cmd.timeout.unwrap_or(self.default_timeout);
        let output = self.os_driver
            .run(&cmd.program, &cmd.args, &cmd.env, cmd.stdin.as_deref(),
                 &cwd, &profile, timeout, self.max_output_bytes)
            .await?;

        // 6. Audit log (capability ledger)
        tracing::info!(
            target: "capability_ledger",
            session_id = ?cmd.session_id,
            program = %cmd.program,
            caps = ?cmd.capabilities,
            exit = ?output.exit_code,
            signal = ?output.signal,
            duration_ms = output.duration_ms,
            "sandbox.execute"
        );

        Ok(output)
    }
}

fn format_capability_request(program: &str, caps: &SandboxCapabilities) -> String {
    let mut parts = Vec::new();
    if !caps.fs_read.is_empty()  { parts.push(format!("fs_read={:?}", caps.fs_read)); }
    if !caps.fs_write.is_empty() { parts.push(format!("fs_write={:?}", caps.fs_write)); }
    if caps.network != crate::sandbox::capabilities::NetworkPolicy::None {
        parts.push(format!("network={:?}", caps.network));
    }
    if caps.spawn_subprocess { parts.push("spawn=true".into()); }
    format!("{program} requests elevated capabilities: {}", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // A no-op driver for tests — avoids invoking macOS sandbox-exec.
    struct FakeDriver {
        captured: RwLock<Option<String>>,
        returns: SandboxOutput,
    }

    impl FakeDriver {
        fn new() -> Self {
            Self {
                captured: RwLock::new(None),
                returns: SandboxOutput {
                    stdout: b"ok".to_vec(),
                    stderr: Vec::new(),
                    exit_code: Some(0),
                    signal: None,
                    truncated: false,
                    duration_ms: 5,
                },
            }
        }
    }

    #[async_trait]
    impl OsSandboxDriverTrait for FakeDriver {
        fn profile_for(&self, _caps: &SandboxCapabilities, _cwd: &Path)
            -> Result<crate::sandbox::driver::OsSandboxProfile, SandboxError> {
            Ok(crate::sandbox::driver::OsSandboxProfile { contents: "".into() })
        }
        async fn run(
            &self, program: &str, _args: &[String], _env: &HashMap<String, String>,
            _stdin: Option<&[u8]>, _cwd: &Path,
            _profile: &crate::sandbox::driver::OsSandboxProfile,
            _timeout: Duration, _max_output_bytes: usize,
        ) -> Result<SandboxOutput, SandboxError> {
            *self.captured.write().await = Some(program.into());
            Ok(self.returns.clone())
        }
    }

    fn sid() -> SessionId {
        crate::routing::session_key::SessionKey::ephemeral("ws-test")
    }

    async fn fresh() -> (WorkspaceSandbox, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let driver: Arc<dyn OsSandboxDriverTrait> = Arc::new(FakeDriver::new());
        let gate = /* ADAPT: construct an ApprovalGate with an auto-approve mock */;
        let ws = WorkspaceSandbox::new(tmp.path().to_path_buf(), driver, gate);
        (ws, tmp)
    }

    #[tokio::test]
    async fn lazy_creates_session_dir() {
        let (ws, _tmp) = fresh().await;
        let sid = sid();
        let _ = ws.for_session(&sid).await.unwrap();
        let expected = ws.workspace_root.join(session_key_to_filename(&sid));
        assert!(expected.exists());
    }

    #[tokio::test]
    async fn execute_happy_path() {
        let (ws, _tmp) = fresh().await;
        let out = ws.execute(SandboxCommand {
            session_id: sid(),
            program: "echo".into(),
            args: vec!["hello".into()],
            env: HashMap::new(),
            stdin: None,
            cwd: None,
            capabilities: SandboxCapabilities::strict(),
            timeout: None,
        }).await.unwrap();
        assert_eq!(out.stdout, b"ok");  // FakeDriver returns "ok"
    }

    #[tokio::test]
    async fn cwd_outside_root_denied() {
        let (ws, _tmp) = fresh().await;
        let err = ws.execute(SandboxCommand {
            session_id: sid(),
            program: "echo".into(),
            args: vec![],
            env: HashMap::new(),
            stdin: None,
            cwd: Some("/usr/bin".into()),
            capabilities: SandboxCapabilities::strict(),
            timeout: None,
        }).await.unwrap_err();
        assert!(matches!(err, SandboxError::CapabilityDenied { .. }));
    }

    #[test]
    fn session_key_filename_is_deterministic() {
        let a = session_key_to_filename(&sid());
        let b = session_key_to_filename(&sid());
        assert_eq!(a, b);
        assert!(a.len() == 32);  // 16-byte hex
    }
}
```

**ADAPT `ApprovalGate` construction in `fresh()`:** the real `ApprovalGate` may require a channel/trait object (Phase 2 noted `Box<dyn ApprovalRequester>`). Either construct a minimal mock that auto-approves, or factor the approval call behind a trait and mock that. If heavy, move capability-approval tests to Task 9's integration tests and drop them from unit here.

- [ ] **Step 6.3: Build + test**

Run: `cargo check -p alephcore 2>&1 | tail -5` → `Finished dev`
Run: `cargo test -p alephcore --lib sandbox::workspace 2>&1 | tail -15` → tests pass.

- [ ] **Step 6.4: Commit**

```bash
git add Cargo.toml Cargo.lock src/sandbox/workspace.rs
git commit -m "sandbox: WorkspaceSandbox implementation

Phase 3 Task 6: lazy per-session workspace directory under
~/.aleph/workspaces/{sha256(session_id)[:16]}/; 6-step execute pipeline
(resolve session, validate cwd, capability check + approval cache,
generate profile, run via OsSandboxDriver, capability_ledger audit).
Tests use FakeDriver to avoid invoking real macOS sandbox-exec."
```

---

## Task 7: AppContext assembly — wire `Arc<dyn Sandbox>` at boot

**Files:**
- Create: `src/sandbox/builder.rs` — `build_sandbox(...)`
- Modify: `src/sandbox/mod.rs` — re-export `build_sandbox`
- Modify: `src/bin/aleph-server/commands/start/mod.rs` — construct and hold `Arc<dyn Sandbox>` for exec-tool registration
- Modify: `src/config/types/tools.rs` or a new config file — `SandboxConfig` with `workspace_root`, `default_timeout_seconds`, `max_output_bytes`

- [ ] **Step 7.1: Define `SandboxConfig`**

Add to `src/config/types/tools.rs` alongside `ToolServiceConfig`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Root directory for per-session workspaces. Default: ~/.aleph/workspaces
    #[serde(default = "default_workspace_root")]
    pub workspace_root: PathBuf,

    /// Default tool execution timeout (seconds)
    #[serde(default = "default_sandbox_timeout")]
    pub default_timeout_seconds: u64,

    /// Maximum combined stdout+stderr bytes
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: usize,
}

fn default_workspace_root() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".aleph").join("workspaces")
}

fn default_sandbox_timeout() -> u64 { 60 }
fn default_max_output_bytes() -> usize { 1024 * 1024 }

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            workspace_root: default_workspace_root(),
            default_timeout_seconds: 60,
            max_output_bytes: 1024 * 1024,
        }
    }
}
```

Add `pub sandbox: SandboxConfig` to the root `Config` struct (Phase 2's `tool_service` field pattern).

- [ ] **Step 7.2: Create `src/sandbox/builder.rs`**

```rust
//! Compose WorkspaceSandbox at startup.

use std::sync::Arc;
use std::time::Duration;

use crate::agent_loop::exec_approval::gate::ApprovalGate;
use crate::config::types::tools::SandboxConfig;
use crate::exec::sandbox::executor::OsSandboxDriver;
use crate::sandbox::driver::OsSandboxDriverTrait;
use crate::sandbox::workspace::WorkspaceSandbox;
use crate::sandbox::Sandbox;

pub fn build_sandbox(
    os_driver: Arc<dyn OsSandboxDriverTrait>,
    approval_gate: Arc<ApprovalGate>,
    config: &SandboxConfig,
) -> Arc<dyn Sandbox> {
    let ws = WorkspaceSandbox::new(config.workspace_root.clone(), os_driver, approval_gate)
        .with_timeout(Duration::from_secs(config.default_timeout_seconds))
        .with_max_output_bytes(config.max_output_bytes);
    Arc::new(ws)
}

pub fn default_os_driver() -> Arc<dyn OsSandboxDriverTrait> {
    Arc::new(OsSandboxDriver::default())  // ADAPT to real OsSandboxDriver constructor
}
```

Add `pub mod builder; pub use builder::build_sandbox;` to `src/sandbox/mod.rs`.

- [ ] **Step 7.3: Wire into boot**

Find `start_server` in `src/bin/aleph-server/commands/start/mod.rs`. After `approval_gate` and `OsSandboxDriver` (which already exist) are constructed:

```rust
let os_driver = crate::sandbox::builder::default_os_driver();
let sandbox: Arc<dyn Sandbox> = crate::sandbox::build_sandbox(
    os_driver,
    approval_gate.clone(),
    &config.sandbox,
);
// `sandbox` is then passed into exec-class tool constructors during builtin registration
```

If `ApprovalGate` is not currently available at this point of boot, trace back where it's constructed and hoist as needed. Keep edits minimal.

- [ ] **Step 7.4: Build**

Run: `cargo check -p alephcore 2>&1 | tail -5` → `Finished dev`
Run: `cargo build --bin aleph-server 2>&1 | tail -5` → builds.
Run: `cargo test -p alephcore --lib 2>&1 | tail -10` → no new failures.

- [ ] **Step 7.5: Commit**

```bash
git add src/sandbox/builder.rs src/sandbox/mod.rs src/config/types/tools.rs src/config/structs.rs src/bin/aleph-server/
git commit -m "sandbox: build_sandbox assembly + SandboxConfig

Phase 3 Task 7: SandboxConfig (workspace_root, timeout, max_output_bytes)
in root Config; build_sandbox composes WorkspaceSandbox + OsSandboxDriver
at boot. Arc<dyn Sandbox> threaded into aleph-server startup for
subsequent exec-tool constructor injection."
```

---

## Task 8: Migrate exec-class tools — one commit per tool

**Files:** every file under `src/builtin_tools/` that calls `Command::new` or `tokio::process::Command::new`.

**Context:** Each exec-class tool's constructor gains an `Arc<dyn Sandbox>` field; `AlephTool::call` swaps its direct Command invocation for `sandbox.execute(SandboxCommand { ... })`. LLM-visible args gain `allow_network` / `allow_subprocess` / `extra_writable_paths` fields.

- [ ] **Step 8.1: Enumerate**

```bash
grep -rn 'Command::new\|tokio::process::Command' src/builtin_tools/ | tee /tmp/phase3-exec-tools.txt
```
Expect ~5–10 hits. Every file listed is a target.

- [ ] **Step 8.2: Per-tool migration recipe**

For each target file:

1. Add `sandbox: Arc<dyn Sandbox>` field
2. Update constructor to take and store it
3. Extend args struct:
   ```rust
   #[serde(default)] pub allow_network: bool,
   #[serde(default)] pub allow_subprocess: bool,
   #[serde(default)] pub extra_writable_paths: Vec<PathBuf>,
   pub timeout_secs: Option<u64>,
   ```
4. Add `impl <Args>` with `into_capabilities()` helper
5. In `call()`:
   ```rust
   let session_id = crate::sandbox::current_session()
       .ok_or_else(|| anyhow::anyhow!("{tool_name} requires session context"))?;
   let cmd = SandboxCommand {
       session_id,
       program: /* program name */,
       args: /* build from Args */,
       env: HashMap::new(),
       stdin: /* if the tool passes stdin */,
       cwd: /* if tool specifies; else None */,
       capabilities: args.into_capabilities(),
       timeout: args.timeout_secs.map(Duration::from_secs),
   };
   let out = self.sandbox.execute(cmd).await.map_err(sandbox_err_to_anyhow)?;
   // Unpack SandboxOutput → tool's return struct (UTF-8-lossy stdout, etc.)
   ```
6. Update the tool's registration site (builtin registry) to pass `sandbox.clone()`
7. Update unit tests — use a `MockSandbox` that records `SandboxCommand` and returns canned `SandboxOutput`

**Helper mock** — add to `src/sandbox/mod.rs` under `#[cfg(test)]`:

```rust
#[cfg(test)]
pub mod test_util {
    use super::*;
    use tokio::sync::Mutex;

    pub struct MockSandbox {
        pub calls: Mutex<Vec<SandboxCommand>>,
        pub response: SandboxOutput,
    }

    impl MockSandbox {
        pub fn new(response: SandboxOutput) -> Arc<Self> {
            Arc::new(Self { calls: Mutex::new(Vec::new()), response })
        }
    }

    #[async_trait::async_trait]
    impl Sandbox for MockSandbox {
        async fn execute(&self, cmd: SandboxCommand) -> Result<SandboxOutput, SandboxError> {
            self.calls.lock().await.push(cmd);
            Ok(self.response.clone())
        }
    }
}
```

- [ ] **Step 8.3: Commit per tool**

Example commit message: `bash_exec: migrate to Sandbox`.
Each commit touches 1 tool + its test + the registration site (if touched). `cargo check` + `cargo test -p alephcore --lib <module>` per commit.

- [ ] **Step 8.4: Final verification after all tools migrated**

Run:
```bash
grep -rn 'Command::new\|tokio::process::Command' src/builtin_tools/ || echo "(none)"
```
Expected: `(none)`. All subprocess spawns live inside `OsSandboxDriver`.

Run: `cargo test -p alephcore --lib 2>&1 | tail -10` → no new failures vs baseline.

---

## Task 9: Capability approval integration test

**Files:**
- Create: `tests/sandbox_capability_approval.rs`

**Context:** End-to-end test: exec-class tool requests `allow_network: true`; `ApprovalGate` receives the request; based on the mock's decision the tool either succeeds or returns `ToolError::PermissionDenied`. `SessionEvent` log reflects the outcome.

- [ ] **Step 9.1: Write the integration test**

```rust
//! Integration: bash_exec with allow_network triggers Sandbox capability approval.

use std::sync::Arc;

use alephcore::sandbox::{Sandbox, SandboxCommand, SandboxCapabilities, NetworkPolicy};
// ADAPT: import actual bash_exec path + SessionService + ToolService

#[tokio::test]
async fn capability_request_approved_then_cached() {
    // 1. Construct: real Sandbox with FakeOsSandboxDriver + mock ApprovalGate (approves)
    // 2. Execute SandboxCommand with NetworkPolicy::AllowAll
    // 3. Assert: approval gate saw 1 request; sandbox returned Ok
    // 4. Execute again with same capability
    // 5. Assert: approval gate did NOT see a new request (cache hit); sandbox returned Ok again
}

#[tokio::test]
async fn capability_request_denied() {
    // 1. mock ApprovalGate denies
    // 2. Execute → assert Err(SandboxError::CapabilityDenied)
}

#[tokio::test]
async fn baseline_request_never_asks() {
    // 1. strict() capabilities
    // 2. Execute → assert approval gate recorded 0 calls
}
```

Fill the test bodies with concrete construction following the `MockSandbox` / `FakeDriver` patterns from Task 6.

- [ ] **Step 9.2: Run**

Run: `cargo test -p alephcore --test sandbox_capability_approval 2>&1 | tail -15` → 3 passed.

- [ ] **Step 9.3: Commit**

```bash
git add tests/sandbox_capability_approval.rs
git commit -m "sandbox: capability-approval integration tests

Phase 3 Task 9: end-to-end verification of the three approval paths —
approved (with cache), denied, baseline (no ask)."
```

---

## Task 10: Documentation + CHANGELOG + final verification + release gate

**Files:**
- Create: `docs/reference/SANDBOX.md`
- Modify: `docs/reference/GLOSSARY.md`
- Modify: `docs/reference/ARCHITECTURE.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 10.1: Write `docs/reference/SANDBOX.md`**

```markdown
# Sandbox

> Tool execution environment — Phase 3 of the [managed-agents refactor](../superpowers/specs/2026-04-19-sandbox-workspace-design.md).

## Trait

`src/sandbox/mod.rs::Sandbox`:
```rust
pub trait Sandbox: Send + Sync + 'static {
    async fn execute(&self, command: SandboxCommand) -> Result<SandboxOutput, SandboxError>;
}
```

## Implementation

`WorkspaceSandbox` (`src/sandbox/workspace.rs`):
- Per-session `~/.aleph/workspaces/{hashed_session_id}/` directory, created lazily
- Six-step execute pipeline: resolve session → validate cwd → capability check (with approval escalation) → profile generation → run → capability_ledger audit
- Drives macOS seatbelt via `OsSandboxDriver` (`src/exec/sandbox/executor.rs`)

## Capability model

`SandboxCapabilities` declares what a command is allowed to do:
- `fs_read` / `fs_write` — extra paths outside workspace root
- `network` — None / AllowAll / AllowHosts
- `spawn_subprocess` — fork permission

LLM must explicitly request capabilities beyond baseline via tool args (e.g. `bash_exec { allow_network: true }`). Escalations trigger one-shot approval via `ApprovalGate`.

## Permission layers

- **Tool-level** (PermissionLayer, Phase 2): existing two-tier agent + global policy
- **Capability-level** (Sandbox): independent per-call approval for capability escalations

Both use the same `ApprovalGate`.

## Task-local `SESSION_ID`

`agent_loop` wraps `tool_svc.execute(...)` in `SESSION_ID.scope(sid, ...)`; exec-class tools read via `current_session()` inside `AlephTool::call()`. `tokio::spawn` does NOT inherit task-locals — use `SESSION_ID.sync_scope(sid, fut)` if spawning subtasks.

## Non-exec tools

Tools that don't spawn subprocesses (memory_*, llm_call, thinker_*) do NOT hold `Arc<dyn Sandbox>` and do NOT participate in capability approval. Their policy is governed by `ExecSecurityGate` and `PermissionLayer`.
```

- [ ] **Step 10.2: Update GLOSSARY.md**

Find the "Sandbox" entry. Flip to present tense:
```markdown
**Aleph today:** Agent-level `Sandbox` trait in `src/sandbox/`. `WorkspaceSandbox`
provisions `~/.aleph/workspaces/{sid}/` lazily and drives macOS seatbelt via
`OsSandboxDriver` (renamed from `SandboxManager`). Exec-class tools hold
`Arc<dyn Sandbox>` and call `sandbox.execute(cmd)`. See
[SANDBOX.md](./SANDBOX.md).
```

- [ ] **Step 10.3: Add ARCHITECTURE.md cross-link**

Near the other Phase cross-links, add:
```markdown
### Sandbox

Exec-class tools delegate subprocess execution to a `Sandbox`. The default
`WorkspaceSandbox` isolates each session in its own workspace directory and
enforces capability escalations via `ApprovalGate`. See
[SANDBOX.md](./SANDBOX.md).
```

- [ ] **Step 10.4: CHANGELOG entry**

Append under `## [Unreleased]`:
```markdown
### Added
- **Sandbox trait + WorkspaceSandbox:** `src/sandbox/` introduces the "where
  to execute" abstraction. Exec-class tools hold `Arc<dyn Sandbox>` and dispatch
  subprocess runs via `sandbox.execute(SandboxCommand)`. Lazy per-session
  workspace directories under `~/.aleph/workspaces/{sid}/`. Two-level permission:
  tool-level via existing agent+global policy, capability-level (network / fs
  outside workspace / subprocess spawn) via `ApprovalGate` single-request
  escalation. Phase 3 of the managed-agents refactor.
- **task-local SESSION_ID:** `src/sandbox/context.rs` provides a `tokio::task_local!`
  for exec-class tools to retrieve the current session without changing the
  `AlephTool` trait surface.

### Changed
- `src/exec/sandbox/executor.rs::SandboxManager` renamed to `OsSandboxDriver`;
  now implements `OsSandboxDriverTrait` for consumption by `WorkspaceSandbox`.
- Phase 2's `SmartFilter` placeholder is now wired to the real
  `LayeredPermissionResolver` / `AgentPermissionFilter`, querying Aleph's
  existing two-tier tool permission system (global + per-agent Deny/Confirm/Allow,
  most-restrictive wins).
- `SmartFilter::classify` is now `async` — the only impact on test fixtures.
- Exec-class tools in `src/builtin_tools/` (bash_exec, …) now hold `Arc<dyn Sandbox>`
  and expose `allow_network` / `allow_subprocess` / `extra_writable_paths` in their
  args so the LLM must declare capability needs explicitly.
```

- [ ] **Step 10.5: Final verification gate**

```bash
echo "=== no Command::new in builtin_tools ==="
grep -rn 'Command::new\|tokio::process::Command' src/builtin_tools/ || echo "(none)"
echo ""
echo "=== SandboxManager fully renamed ==="
grep -rn '\bSandboxManager\b' src/ || echo "(none)"
echo ""
echo "=== ScriptedFilter is test-only ==="
grep -rn 'ScriptedFilter' src/ | grep -v '#\[cfg(test)\]\|cfg_attr\|/tests/' | head
echo ""
echo "=== src/sandbox/ layout ==="
ls src/sandbox/
echo ""
echo "=== full test suite ==="
cargo test -p alephcore --lib 2>&1 | tail -10
```

Expected:
- First two greps: `(none)`
- `ScriptedFilter` lines are all test-cfg-gated
- `src/sandbox/` has: mod.rs, command.rs, capabilities.rs, context.rs, workspace.rs, driver.rs, builder.rs
- Test result matches or exceeds baseline (9029+ passed / 2 pre-existing failed)

- [ ] **Step 10.6: Clippy on new paths**

```bash
cargo clippy -p alephcore -- -D warnings 2>&1 | grep -E 'src/sandbox/|src/tools/middleware/permission/' | head -20
```
Expected: no lines from these paths.

- [ ] **Step 10.7: Commits**

```bash
git add docs/reference/SANDBOX.md docs/reference/GLOSSARY.md docs/reference/ARCHITECTURE.md
git commit -m "docs: Sandbox reference + glossary + architecture cross-link (Phase 3)"

git add CHANGELOG.md
git commit -m "changelog: note Phase 3 Sandbox trait + WorkspaceSandbox"
```

- [ ] **Step 10.8: Release gate — STOP**

Phase 3 is code-complete. Do NOT auto-release. Present to user:

> "Phase 3 implementation complete on branch `worktree-managed-agents-phase-3`. All commits green, no new test failures beyond the 2 pre-existing. Next step options:
>
> 1. **Merge to main** — `git -C /Volumes/TBU4/Workspace/Aleph merge worktree-managed-agents-phase-3 --no-ff`
> 2. **Release** — `just release $(date +%Y.%m.%d)`
> 3. **Both**
> 4. **Start Phase 4 brainstorm** (Harness — Think→Act loop rewrite)
>
> Which?"

Only proceed on explicit choice.

---

## Non-Goals (explicit)

- Not implementing Linux seccomp / Windows Job Object — `OsSandboxDriver` remains macOS-focused; non-macOS platforms fall back with `tracing::warn!`
- Not auto-deleting session workspace directories — leave for user inspection
- Not changing Gateway RPC surface
- Not migrating non-exec tools to Sandbox — `file_write` / `file_edit` stay on `tokio::fs` / existing `ExecSecurityGate`

## Rollback

If any task's gate fails and the cause isn't obvious within 15 minutes:
```bash
git revert <sha>
```
Never `git reset --hard` without explicit user consent.

## Done-ness Signals

Phase 3 is done when:
1. All 10 tasks checked off
2. `grep -rn 'Command::new\|tokio::process::Command' src/builtin_tools/` → zero hits
3. `grep -rn '\bSandboxManager\b' src/` → zero hits
4. `ScriptedFilter` appears only under `#[cfg(test)]`
5. Capability approval flow works end-to-end (Task 9 integration test green)
6. `cargo test -p alephcore --lib` baseline preserved (9029+ passed / 2 pre-existing failed)
7. CHANGELOG entry committed
8. User made a merge/release decision at Step 10.8

Proceed to **Phase 4 brainstorming** only after all signals are green.

---

## Status: Complete

All 10 tasks landed on branch `worktree-managed-agents-phase-3`. Final HEAD
contains the docs commit from Task 10; release decision deferred per the
Task 10.8 instruction to stop at the release gate.

| Task | Commit | Summary |
|------|--------|---------|
| 2 | `d8854cfc1` | `tools: backfill LayeredPermissionResolver wiring SmartFilter to two-tier permissions` |
| 3 | `eb90858bb` | `sandbox: add module scaffold — trait + types + capabilities` |
| 4 | `422a506ff` | `sandbox: rename SandboxManager to OsSandboxDriver and impl OsSandboxDriverTrait` |
| 5 | `8c4ae31f4` | `sandbox: wire SESSION_ID task-local through invoke_with_session_trace` |
| 6 | `34f5440e2` | `sandbox: implement WorkspaceSandbox with lazy per-session workspace` |
| 7 | `ddcc36750` | `sandbox: add SandboxConfig, build_sandbox factory, boot wiring` |
| 8 | `7307fbfb1` | `sandbox: migrate code_exec and bash_exec to Arc<dyn Sandbox>` |
| 8 (follow-up) | `bc139ec10` | `sandbox: wire Arc<dyn Sandbox> through registration into exec-class tools` |
| 8 (clippy) | `8c260c2ad` | `sandbox: fix clippy into_* naming on CodeExecArgs::as_capabilities` |
| 9 | `abd7faa2c` | `tests: capability approval integration flow for WorkspaceSandbox` |
| 10 | *(docs commit)* | `docs: Phase 3 Sandbox reference, glossary, CHANGELOG` |

Verification at Task 10:
- `cargo check -p alephcore` — `Finished dev`
- `cargo test -p alephcore --lib` — 9054 passed / 2 failed (pre-existing) / 20 ignored
- `cargo test -p alephcore --test sandbox_capability_approval` — 4/4 pass
- `cargo test -p alephcore --lib sandbox::` — 39/39 pass
