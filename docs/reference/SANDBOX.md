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
build a single `ApprovalGate` (requesterless at construction —
`ChannelApprovalBridgeAdapter` is injected via `set_requester` after the
channel registry comes up; see HITL P1 in
`docs/superpowers/specs/2026-05-19-hitl-loop-closure-design.md`) → build
the `OsSandboxAdapter` → wrap in `OsSandboxDriver` → call
`build_sandbox(&loaded_app_config.sandbox, os_driver, approval_gate)`.
The resulting `Arc<dyn Sandbox>` is threaded through tool registration
into every exec-class tool constructor (`CodeExecTool::with_sandbox`,
`BashExecTool::with_sandbox`, …).

The same `approval_gate` is also handed to the `ScopedToolService` via
`with_confirmation(confirm_tools, requester)`, so the `requires_confirmation`
tool seam (HITL P3: `vault_store` / `agent_delete` / `team_disband`) and
sandbox capability escalations share one gate. The pre-`ScopedToolService`
`PermissionLayer` / `LayeredPermissionResolver` / `AgentPermissionFilter`
chain was deleted in 2026-05-20 — it was unreachable because every
gateway request overrode it. Per-agent permission policy is a future
follow-up cycle.

## Relation to `file_write` / `file_edit`

`file_write` and `file_edit` enforce path/scope rules inline (see
`src/builtin_tools/`); they do not spawn subprocesses, so they do not
route through `Arc<dyn Sandbox>`. The Sandbox subsystem owns the
*how-it-runs* side — workspace, capabilities, OS isolation — for tools
that do spawn processes (`code_exec`, `bash_exec`).

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
| `tests/sandbox_capability_approval.rs` | End-to-end capability approval flow |

## Desktop Bridge boundary

The Swift `AlephBridge` helper process runs **outside** the Rust sandbox: it
must hold camera, microphone, speech, and accessibility TCC grants in its own
bundle, and it calls native frameworks that cannot run inside a `sandbox-exec`
profile. Conversely, the Rust core remains sandbox-friendly because all native
API calls are proxied to the helper over stdio.

Hard rules that must not be violated by any bridge handler:

- The bridge process **must not** open any TCP or Unix domain socket. Only the
  inherited stdio pipes are used for IPC.
- The bridge **must not** read or write `~/.aleph/data/`, the `.shared_token`
  file, or any other vault path. Vault access is exclusive to the Rust core.
  Concurrent writes from a second process corrupt the encrypted vault and
  unrecoverably destroy stored API keys, OAuth tokens, and embeddings (see
  CLAUDE.md, `.shared_token` incident).
- Permission status is owned by macOS TCC; the bridge merely reflects it via
  `perm.check` and returns `PermissionGuide` in `-32001` errors. The bridge
  does not grant or revoke permissions.
- Any new bridge handler added in the future **must** include a comment
  justifying why it does not touch `~/.aleph/`. Bridge code review checks this
  invariant.

## Cycle 1 hardening (2026-05-20)

Comparison against codex's three-OS sandbox (`/Volumes/TBU4/Github/codex`)
surfaced four classes of defect that Cycle 1 fixed end-to-end. See
[`docs/superpowers/specs/2026-05-20-sandbox-hardening-cycle1-design.md`][cycle1]
for the full design.

[cycle1]: ../superpowers/specs/2026-05-20-sandbox-hardening-cycle1-design.md

### Behavior changes (breaking)

- **`NetworkPolicy::AllowHosts` and `NetworkPolicy::ProxyOnly` now hard-fail**
  on Linux and Windows. Both used to silently degrade — Linux fell back to
  `--unshare-net` (no network at all), Windows wrote a no-op profile line.
  Callers that depended on the silent fallback get
  `SandboxError::UnsupportedPolicy` instead; the error message points at the
  follow-up spec that will implement the feature.
- **macOS `AllowHosts` now validates each entry parses as an IP address**.
  Seatbelt's `(remote ip ...)` matcher only accepts IP literals; hostnames
  silently never matched (the rule was a no-op). The new validation rejects
  hostnames with a remediation hint to pre-resolve to IPs.

### New capabilities

- **`SandboxCapabilities.max_memory_mb`** caps the sandboxed process's
  virtual address space on all three OSes:
  - **macOS / Linux**: `setrlimit(RLIMIT_AS)` applied via `pre_exec` on the
    sandbox helper (`sandbox-exec` / `bwrap`). The limit is inherited by the
    eventual target binary through exec.
  - **Windows**: `JOBOBJECT_EXTENDED_LIMIT_INFORMATION.ProcessMemoryLimit` on
    the Job Object that contains the sandboxed process.
- **`SandboxCapabilities.timeout_secs`** is a per-call override of the
  `SandboxConfig.default_timeout_seconds` ceiling.

### Dissolution — 937 lines of dead Windows code removed

The following modules contained stub façades pretending to implement Windows
security primitives but were verifiably never called from production code:

- `src/sandbox/platforms/windows/wfp.rs` — every method either returned a
  hardcoded `Err` or a no-op `Ok(())`.
- `src/sandbox/platforms/windows/appcontainer.rs` — every method returned
  `Err("requires windows-sys 0.61+")`.
- `src/sandbox/platforms/windows/acl.rs` — `dacl_allows_access` had zero
  callers tree-wide.
- `src/sandbox/platforms/windows/filter.rs` — `FilterSet` referenced only by
  its own `#[cfg(test)]` module.
- `src/sandbox/platforms/windows/token.rs` — `create_restricted_token` was
  imported by `driver.rs` but never invoked; the spawn path used plain
  `tokio::process::Command` with only a JobObject for protection.

Removing them follows R10's "YAGNI 撤回模式" — zero current consumers means
delete, not preserve for hypothetical future use. A future spec that
implements RestrictedToken / WFP / AppContainer can re-introduce focused,
working modules without inheriting the stub skeletons.

### Current Windows defense surface (post-SP-3a)

- **Job Object** (cycle 1): active-process limit (fork-bomb defense),
  kill-on-close, die-on-unhandled-exception, virtual-memory ceiling,
  UI restrictions.
- **Restricted token + Low IL** (SP-3a, shipped 2026-05-20):
  `WindowsSandboxDriver::run` wraps the target with
  `aleph-server sandbox-init-windows`, which calls
  `CreateRestrictedToken(self, DISABLE_MAX_PRIVILEGE)` →
  `SetTokenInformation(TokenIntegrityLevel = S-1-16-4096)` →
  `CreateProcessAsUserW(target)`. Result: target runs with no
  privileges and at Low integrity, isolated by both the JobObject
  container and the token-level authority drop. Stdio passes through
  via inherited HANDLEs.
- Soft-degrade: if `CreateProcessAsUserW` returns
  `ERROR_PRIVILEGE_NOT_HELD` (host lacks `SE_INCREASE_QUOTA`, common
  on locked-down server policies), the init falls back to plain
  `CreateProcessW` (JobObject still applies). Flip
  `WindowsSandboxConfig.require_restricted_token = true` to escalate
  to a hard spawn failure.
- **Not yet**: AppContainer (SP-6), WFP per-host network filtering
  (SP-3b — requires admin; likely superseded by SP-6), workspace DACL
  grants for Low-IL writes (separate small cycle).

### Linux resource limits (SP-5 — shipped 2026-05-20)

On Linux with cgroup v2 delegated to the user, `BubblewrapDriver::run`
creates a per-execution sub-cgroup under the aleph-server process's own
cgroup and applies:

- **`memory.max`** from `SandboxCapabilities.max_memory_mb`. RSS-based,
  so `mmap(PROT_NONE)` tricks that bypass `RLIMIT_AS` are caught.
  `memory.swap.max=0` always (no swap-pressure escape).
- **`cpu.max`** from `LinuxSandboxConfig.cpu_quota_percent`. `None` →
  unlimited; `Some(50)` → 50% of one core.
- **`pids.max`** from `LinuxSandboxConfig.max_pids` (default `Some(200)`).
  Hard cap on process count — defends against fork bombs beyond what
  bwrap's active-process limit provides.

The bwrap child PID is written to `cgroup.procs` via `pre_exec` so
membership is inherited through the bwrap → sandbox-init → target exec
chain. `Drop` on `CgroupV2Scope` runs after `wait_with_output()` and
`rmdir`s the cgroup directory; no orphan accumulation.

When cgroup v2 is unavailable (cgroup-v1-only systems, containers
without delegation, non-systemd hosts), `try_create` returns `None` and
the sandbox runs anyway — `RLIMIT_AS` continues to enforce a memory
ceiling, with one `tracing::warn!` explaining the degradation. Flip
`LinuxSandboxConfig.require_cgroups = true` to escalate to a hard
spawn failure instead.

### Linux defense-in-depth (SP-2 — shipped 2026-05-20)

bwrap's namespace isolation now sits underneath two additional Linux LSM
mechanisms, applied by a hidden `aleph-server sandbox-init` subcommand
that bwrap launches inside its mount namespace:

- **Landlock** (kernel ≥ 5.13): in-process FS ACL inside the mounts that
  bwrap already gave the child. `READ_FILE | READ_DIR | EXECUTE` on
  `SYSTEM_READ_PATHS` (`/usr`, `/lib`, `/lib64`, `/bin`, `/sbin`,
  `/etc`) + `SandboxCapabilities.fs_read`; full RW + Exec on
  `SandboxCapabilities.fs_write` + the session cwd. On kernels < 5.13
  landlock is soft-skipped with a warning unless
  `LinuxSandboxConfig.require_landlock = true`.
- **seccomp-bpf** (kernel ≥ 3.5, universal in practice): syscall
  denylist returning `EPERM` for ~28 entries covering filesystem
  manipulation (`mount`/`umount`/`pivot_root`/`chroot`), kernel reload
  (`kexec_*`), module loading (`*_module`), eBPF, perf, ptrace, kernel
  keyring, `userfaultfd`, io_uring, `mknodat`, swap, `syslog`,
  `reboot`, namespace switching, and `clone`/`unshare` with
  `CLONE_NEWUSER` (nested user-ns escape). Snapshot-pinned by the
  `seccomp_denylist_is_frozen` unit test.

The init subcommand is wired into the existing `aleph-server` binary
(no separate helper artifact — R3 core minimalism); bwrap bind-mounts
the running aleph-server read-only at `/aleph-sandbox-init` inside the
namespace, then exec's it. Two new crates pulled in Linux-only:
`landlock 0.4` and `seccompiler 0.5`.

### Hostname support (macOS, SP-4 — shipped 2026-05-20)

`NetworkPolicy::AllowHosts { hosts: ["github.com", "1.2.3.4"] }` is now
accepted on macOS. The pipeline gained one new step between approval (4)
and `profile_for` (5):

- `src/sandbox/dns.rs::resolve_hosts_in_capabilities` walks the hosts list,
  leaves IP literals untouched (`1.2.3.4`, `[::1]`, `1.2.3.4:443`, bare
  IPv6), and resolves hostnames via `tokio::net::lookup_host` with a 5 s
  timeout per hostname.
- On failure (NXDOMAIN, timeout, empty result), the command is rejected
  with `SandboxError::DnsResolutionFailed { hostname, source }` — fail
  closed, matching cycle 1's P7 posture.
- The resolved IPs replace the hostnames in `cmd.capabilities.network`
  before the driver sees them, so Seatbelt's `(remote ip ...)` matcher
  sees IP-only input. Defense-in-depth: the seatbelt driver still rejects
  non-IPs if called directly, so a future caller bypassing the workspace
  pipeline can't generate malformed SBPL.
- Linux (`bwrap`) and Windows (`JobObject`-only) continue to return
  `SandboxError::UnsupportedPolicy` for any `AllowHosts` — they have no
  IP-level matcher to feed. SP-2 / SP-3b / SP-6 will plug into the same
  DNS layer when those mechanisms land.

### Deferred follow-up specs

| ID | Scope | Why deferred |
|---|---|---|
| ~~SP-2~~ | ~~Linux Landlock + seccomp-bpf~~ | **Shipped (2026-05-20)** as `aleph-server sandbox-init` subcommand invoked by bwrap. See "Linux defense-in-depth" below. |
| ~~SP-3a~~ | ~~Windows RestrictedToken + Low IL~~ | **Shipped (2026-05-20)** via `sandbox-init-windows` subcommand using CreateRestrictedToken + SetTokenInformation. See "Current Windows defense surface" above. |
| SP-3b | Windows WFP per-host network filtering | Requires admin for filter installation. Likely superseded by SP-6 (AppContainer's network capability model) for most use cases. |
| ~~SP-4~~ | ~~Hostname-based filtering~~ | **Shipped (2026-05-20)** as workspace-layer DNS pre-resolution; macOS-scoped. See "Hostname support" below. |
| ~~SP-5~~ | ~~cgroups v2 Linux memory + CPU~~ | **Shipped (2026-05-20)** as host-side `CgroupV2Scope` in `BubblewrapDriver::run`. See "Linux resource limits" below. |
| SP-6 | Windows AppContainer | Requires `windows-sys` major-version upgrade. |

## References

- **Spec:** `docs/superpowers/specs/2026-04-19-sandbox-workspace-design.md`
- **Spec (Cycle 1):** `docs/superpowers/specs/2026-05-20-sandbox-hardening-cycle1-design.md`
- **Plan:** `docs/superpowers/plans/2026-04-19-managed-agents-phase-3-sandbox.md`
- **Glossary:** [GLOSSARY.md](./GLOSSARY.md) — `Sandbox`, `WorkspaceSandbox`,
  `OsSandboxDriver`, `SandboxCapabilities`
