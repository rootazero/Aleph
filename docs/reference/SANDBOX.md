# Sandbox — Phase 3 Reference

> Where exec-class tools actually run. Introduced in the managed-agents Phase 3
> refactor. See [GLOSSARY.md](./GLOSSARY.md#sandbox) for the term's Anthropic
> meaning and its relation to the rest of the system.

## Overview

The `Sandbox` trait (`src/sandbox/mod.rs`) is the one-method seam between an
exec-class tool and the operating system:

```rust
#[async_trait]
pub trait Sandbox: Send + Sync + 'static {
    async fn execute(
        &self,
        command: SandboxCommand,
    ) -> Result<SandboxOutput, SandboxError>;
}
```

Production boot wires an `Arc<dyn Sandbox>` pointing at `WorkspaceSandbox`
(`src/sandbox/workspace.rs`). `WorkspaceSandbox` owns three concerns:

1. **Workspace provisioning** — materialize `~/.aleph/workspaces/{hash(session_id)}/`
   on first exec, keep it alive for the session, reuse on subsequent calls.
2. **Capability enforcement** — classify every `SandboxCommand` against the
   session's baseline `SandboxCapabilities`, escalate out-of-baseline requests
   through `ApprovalGate`, cache per-session grants.
3. **OS isolation** — delegate the actual subprocess launch to an
   `OsSandboxDriverTrait` implementation (today: macOS `sandbox-exec` via
   `OsSandboxDriver` in `src/exec/sandbox/executor.rs`).

The relationship at a glance:

```
exec-class tool (code_exec, bash_exec, …)
        │   Arc<dyn Sandbox>
        ▼
WorkspaceSandbox   ──► ApprovalGate (capability elevation)
        │
        │   OsSandboxDriverTrait
        ▼
OsSandboxDriver    ──► macOS sandbox-exec binary
```

## Lifecycle — per-session workspace

`WorkspaceSandbox` keeps a `HashMap<SessionId, Arc<SessionWorkspace>>` behind
an `RwLock`. `for_session(&sid)` is the entry point:

- **Fast path:** `read().await` → cache hit → return the existing
  `Arc<SessionWorkspace>`.
- **Slow path:** `write().await` → double-check (another task may have created
  it) → `tokio::fs::create_dir_all(cwd)` → insert into the map.

The on-disk path is deterministic:

```
workspace_root / session_key_to_filename(session_id)
```

`session_key_to_filename` (`src/sandbox/workspace.rs:114`) SHA-256s the
JSON-serialized `SessionId` and truncates to 16 bytes (32 hex chars). That
keeps the path short and safe across every `SessionKey` variant regardless of
the characters those variants may carry.

Each `SessionWorkspace` carries:

- `cwd: PathBuf` — the materialized directory
- `baseline: SandboxCapabilities` — policy ceiling (today: `::strict()`)
- `granted_elevations: RwLock<HashSet<SandboxCapabilities>>` — per-session
  cache of approvals the user has already granted

## The six-step `execute` pipeline

`WorkspaceSandbox::execute` (`src/sandbox/workspace.rs:124`) implements the
spec §8 pipeline:

1. **Session resolve** — `self.for_session(&cmd.session_id)` → lazy dir
   creation if first call; cached `Arc<SessionWorkspace>` otherwise.
2. **cwd validate** — `cmd.cwd` is either `None` (defaults to workspace root)
   or a path that must `starts_with(&ws.cwd)`. Anything else returns
   `SandboxError::CapabilityDenied { reason: "cwd outside workspace root" }`.
3. **Capability check** —
   `cmd.capabilities.is_within(&ws.baseline)` is the fast path (no approval).
   Otherwise consult `granted_elevations`; if the request is within a prior
   grant, pass. Otherwise ask `ApprovalGate::request_approval_for_tool`.
   - `ApprovalOutcome::Approved` → insert `cmd.capabilities` into
     `granted_elevations` (future same-or-narrower requests are cached).
   - `ApprovalOutcome::Denied` | `Timeout` → `SandboxError::CapabilityDenied`.
4. **Profile generate** — `os_driver.profile_for(&caps, &cwd)` returns an
   opaque `OsSandboxProfile` (on macOS, SBPL profile text).
5. **Run** — `os_driver.run(program, args, env, stdin, cwd, profile, timeout,
   max_output_bytes)`. Default timeout 60s, default output budget 1 MB (split
   stdout + stderr). Both override-able on the `WorkspaceSandbox` via
   `with_timeout` / `with_max_output_bytes`.
6. **Audit** — emit a `tracing::info!(target: "capability_ledger", …)` record
   carrying `session_id`, `program`, `caps`, `exit_code`, `signal`,
   `duration_ms`. This is the capability-ledger hook; downstream tracing
   subscribers can sink it to whatever store is desired.

Inside the pipeline, `SandboxCapabilities::is_within` (`src/sandbox/capabilities.rs:36`)
enforces four monotonic checks: `fs_read` ⊆ (prefix), `fs_write` ⊆ (prefix),
`network` (`None ⊆ AllowHosts ⊆ AllowAll`), `spawn_subprocess` (false ⊆ any).

## Capabilities

`SandboxCapabilities` (`src/sandbox/capabilities.rs`) is a plain struct:

```rust
pub struct SandboxCapabilities {
    pub fs_read: Vec<PathBuf>,
    pub fs_write: Vec<PathBuf>,
    pub network: NetworkPolicy,
    pub spawn_subprocess: bool,
}

pub enum NetworkPolicy {
    None,
    AllowAll,
    AllowHosts { hosts: Vec<String> },
}
```

`::strict()` (equivalent to `::default()`) is the workspace baseline: no fs
access outside the cwd (which the OS driver auto-grants via the seatbelt
profile), no network, no subprocess spawn. Any command that needs more must
escalate via `ApprovalGate`.

## Task-local `SESSION_ID`

Exec-class tools don't know the current session id — their
`AlephTool::execute` signature doesn't carry it. The sandbox subsystem uses a
tokio `task_local!` to thread the id without touching every tool trait:

- `crate::sandbox::context::SESSION_ID` — declared in `src/sandbox/context.rs`
- `current_session() -> Option<SessionId>` — the read helper
- **Writer:** `crate::session::invoke_with_session_trace`
  (`src/session/tool_trace.rs:17`) wraps `tool_svc.execute(...)` in a
  `SESSION_ID.scope(session_id.clone(), async move { ... })`.

This gives tools a single, narrow mechanism:
`crate::sandbox::context::current_session()` returns `Some(sid)` inside the
scope and `None` outside. Tools that need a workspace but are called outside
a session can choose their fallback policy (today they rely on the caller
having passed a `SandboxCommand { session_id, ... }` explicitly).

## `SandboxConfig`

Boot-time tuning lives on `SandboxConfig` (`src/sandbox/config.rs`):

```rust
pub struct SandboxConfig {
    pub workspace_root: PathBuf,       // default: ~/.aleph/workspaces
    pub enabled: bool,                 // default: true
    pub default_timeout_seconds: u64,  // default: 60
    pub max_output_bytes: usize,       // default: 1 MiB
}
```

Serde reads the `[sandbox]` TOML section with defaults so existing configs
keep working. Tests / CI can set `enabled = false` to disable the subsystem;
`build_sandbox` will then hand back a `NoopSandbox` whose `execute` always
errors with `SandboxError::Other("sandbox disabled: …")` — a deliberate
fail-fast, not a silent bypass.

## Factory — `build_sandbox`

`src/sandbox/factory.rs` composes the `Arc<dyn Sandbox>` at boot:

```rust
pub fn build_sandbox(
    cfg: &SandboxConfig,
    driver: Arc<dyn OsSandboxDriverTrait>,
    approval: Arc<ApprovalGate>,
) -> Arc<dyn Sandbox>;
```

`src/bin/aleph-server/commands/start/mod.rs` wires this during start-up:
build a single `ApprovalGate` (currently requesterless — see *Known
Limitations* in `CHANGELOG.md`) → build the `OsSandboxAdapter` → wrap in
`OsSandboxDriver` → call `build_sandbox(&loaded_app_config.sandbox,
os_driver, approval_gate)`. The resulting `Arc<dyn Sandbox>` is threaded
through tool registration into every exec-class tool constructor
(`CodeExecTool::with_sandbox`, `BashExecTool::with_sandbox`, …).

The same `approval_gate` is also attached to the `PermissionLayer` via
`PermissionLayer::set_approver`, so Ask-tier tool confirmations and
sandbox capability escalations share one gate. The global
`[policies.tool_permissions]` table plus an empty per-agent default are
merged into a `LayeredPermissionResolver` (via
`AgentPermissionFilter::build`) and attached via
`PermissionLayer::set_smart_filter`. Boot logs
`"wired LayeredPermissionResolver into PermissionLayer..."` on success.
Per-agent overrides plug in at Phase 4 session activation.

### Exec-class double-prompt risk (H4)

When `bash` / `code_exec` are classified `Ask` in
`[policies.tool_permissions]`, `PermissionLayer` and `WorkspaceSandbox`
would each request approval independently — two prompts. The current
global-default `Allow` for exec tools avoids this in practice. A Phase 4
follow-up will add an exec-class exclude-list to
`LayeredPermissionResolver` so the sandbox owns the prompt for those
tools.

## Relation to `ExecSecurityGate`

`ExecSecurityGate` (`src/executor/exec_security_gate.rs`) is a **pre-exec
filesystem-write guard** for `file_write` / `file_edit`. It sits at a
different layer: it validates target paths and scopes against the exec policy
*before* any subprocess is spawned. The Sandbox subsystem, by contrast, owns
the *how-it-runs* side — workspace, capabilities, OS isolation — for tools
that do spawn processes.

Today's split:

- `file_write`, `file_edit` stay behind `ExecSecurityGate` (direct disk I/O,
  no subprocess).
- `code_exec`, `bash_exec`, and future exec-class tools route through
  `Arc<dyn Sandbox>`.

The two can coexist in a single tool pipeline without double-guarding, because
they check different concerns: gate = "is this path allowed to change?",
sandbox = "is this process allowed to do what it's asking to do?".

## Two-tier permissions → `SmartFilter`

Phase 3 Task 2 backfilled the placeholder `SmartFilter` trait (introduced in
Phase 2) with a concrete policy-backed resolver:

- `LayeredPermissionResolver` (`src/tools/middleware/permission/resolver.rs`)
  wraps an `ArcSwap<ToolPermissionsConfig>` that holds the already-merged
  global + per-agent config (most-restrictive-wins via
  `ToolPermissionsConfig::merge`). Its `classify(name, _input)` returns
  `Classification::Allow` / `Confirm` / `Deny` based on
  `PermissionAction::Allow` / `Ask` / `Deny`. `swap(new_merged)` supports
  live-reload without swapping the filter.
- `AgentPermissionFilter::build(global, agent)`
  (`src/tools/middleware/permission/agent_filter.rs`) is the convenience
  constructor orchestrator paths use when they know which agent is running.

This is the *tool-level* policy lane — it gates the `ToolService::execute`
call itself. The Sandbox's capability check is the *capability-level* policy
lane: once a tool call is allowed through, the Sandbox still scrutinizes what
the subprocess is allowed to read / write / network / spawn.

## Exec-class tool consumption pattern

Exec-class tools hold the sandbox in their state and consume it on execute:

```rust
pub struct CodeExecTool {
    // ...
    sandbox: Option<Arc<dyn Sandbox>>,
}

impl CodeExecTool {
    pub fn with_sandbox(mut self, sandbox: Arc<dyn Sandbox>) -> Self {
        self.sandbox = Some(sandbox);
        self
    }
}

#[async_trait]
impl AlephTool for CodeExecTool {
    async fn execute(&self, input: Value) -> Result<ToolOutput, AlephToolError> {
        let sandbox = self.sandbox.as_ref().ok_or_else(|| {
            // fail-fast — boot wiring must attach a sandbox
        })?;
        let cmd = SandboxCommand { /* … */ };
        let output = sandbox.execute(cmd).await?;
        // map SandboxOutput into ToolOutput
    }
}
```

Two rules follow from this:

- **No `Command::new` anywhere in `src/builtin_tools/`** for exec-class tools.
  The sandbox seam is the only subprocess path.
- **`with_sandbox` is the sole injection point.** Boot wiring in
  `src/executor/builtin_registry/builder.rs` takes the shared
  `Arc<dyn Sandbox>` from `AppContext` and calls `with_sandbox` on every
  exec-class tool it registers. A tool constructed without a sandbox returns
  a structured "sandbox not configured" error instead of falling back to
  unscoped execution — the default is safe, not permissive.

## Testing pattern

The sandbox stack is testable at every seam without real subprocesses or OS
sandboxing:

- **Fake OS driver** — implement `OsSandboxDriverTrait`, count `run` calls,
  return a canned `SandboxOutput`. Used by the unit tests in
  `src/sandbox/workspace.rs` and the integration test.
- **Fake approval requester** — implement `ApprovalRequester` and hand the
  resulting box to `ApprovalGate::new(cfg, Some(Box::new(requester)))`. The
  unit tests use this to drive every branch of step 3 (approve / deny /
  cached).
- **`MockSandbox`** (`src/sandbox/mod.rs:50`, `#[cfg(test)]`) — records every
  `SandboxCommand` and returns a canned `SandboxOutput`. Used by exec-class
  tools (`bash_exec`, `code_exec`) to assert they route through the sandbox
  seam.
- **Integration** — `tests/sandbox_capability_approval.rs` drives the full
  pipeline via the public surface (`build_sandbox`, `Arc<dyn Sandbox>`,
  `SandboxCommand`) with the fake driver + fake requester wiring. Four cases:
  strict caps skip approval; network-elevated approve reaches driver; spawn
  denied returns `CapabilityDenied` without reaching driver; approval cached
  across two calls in the same session.

Tests that don't touch exec at all can skip the subsystem entirely by
constructing a `SandboxConfig { enabled: false, .. }` — `build_sandbox` then
returns `NoopSandbox`.

## Source map

| File | Role |
|------|------|
| `src/sandbox/mod.rs` | `Sandbox` trait + re-exports + `MockSandbox` test helper |
| `src/sandbox/command.rs` | `SandboxCommand`, `SandboxOutput`, `SandboxError` |
| `src/sandbox/capabilities.rs` | `SandboxCapabilities` + `NetworkPolicy` + `is_within` |
| `src/sandbox/workspace.rs` | `WorkspaceSandbox` + six-step pipeline |
| `src/sandbox/driver.rs` | `OsSandboxDriverTrait` + `OsSandboxProfile` |
| `src/sandbox/factory.rs` | `build_sandbox` + `NoopSandbox` |
| `src/sandbox/config.rs` | `SandboxConfig` (boot-time tunables) |
| `src/sandbox/context.rs` | `SESSION_ID` task-local + `current_session()` |
| `src/exec/sandbox/executor.rs` | `OsSandboxDriver` (macOS `sandbox-exec` driver) |
| `src/session/tool_trace.rs` | `invoke_with_session_trace` — sets `SESSION_ID.scope(...)` |
| `src/tools/middleware/permission/resolver.rs` | `LayeredPermissionResolver` → `SmartFilter` |
| `src/tools/middleware/permission/agent_filter.rs` | `AgentPermissionFilter::build` |
| `tests/sandbox_capability_approval.rs` | End-to-end capability approval flow |

## References

- **Spec:** `docs/superpowers/specs/2026-04-19-sandbox-workspace-design.md`
- **Plan:** `docs/superpowers/plans/2026-04-19-managed-agents-phase-3-sandbox.md`
- **Glossary:** [GLOSSARY.md](./GLOSSARY.md) — `Sandbox`, `WorkspaceSandbox`,
  `OsSandboxDriver`, `SandboxCapabilities`, `LayeredPermissionResolver`,
  `AgentPermissionFilter`
