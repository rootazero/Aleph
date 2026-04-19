# Sandbox Trait + WorkspaceSandbox — Phase 3 Design

**Date**: 2026-04-19
**Status**: Design approved — ready for implementation plan
**Parent**: [managed-agents-refactor-roadmap](./2026-04-18-managed-agents-refactor-roadmap.md) §7 Phase 3
**Scope**: Aleph Core — new `src/sandbox/` module; rename existing `SandboxManager` → `OsSandboxDriver`; exec-class tools route through Sandbox; backfill Phase 2's permission integration with the existing agent + global tool permission system.

---

## 1. Goal

Introduce a `Sandbox` trait as the "where to execute" abstraction, orthogonal to `ToolService`. Implement `WorkspaceSandbox` that provisions a per-session `~/.aleph/workspaces/{session_id}/` directory lazily, enforces capability baselines via macOS seatbelt, and requests user approval for capability escalations through the existing `ApprovalGate`. Exec-class tools (bash_exec, file_write, …) consume `Arc<dyn Sandbox>` through their constructors; non-exec tools are untouched.

Simultaneously backfill Phase 2's `SmartFilter` placeholder by wiring it to Aleph's existing two-tier tool permission system (global + per-agent, each with Deny/Confirm/Allow).

## 2. Non-Goals

- Not a container / process-level sandbox — workspace sandbox only
- Not Linux or Windows OS sandboxing — macOS seatbelt plus a stub for other platforms (trait shape permits Linux seccomp / Windows Job Object later)
- Not a new permission system — Phase 3 integrates with the existing global + per-agent tool permissions; no replacement or parallel system
- Not changing `AlephTool` author-side trait signature (50+ existing tools untouched at the type level; only exec-class tools add a `sandbox: Arc<dyn Sandbox>` field)
- Not touching Gateway `tools.*` RPC or `session.*` RPC
- Not auto-deleting session workspace directories after session end (leftovers are useful for user inspection; explicit cleanup RPC may come later)

## 3. Decisions Locked (from brainstorming Q1–Q5 + sub-questions)

| Axis | Choice | Rationale |
|------|--------|-----------|
| Tool → Sandbox routing | **Tool-level opt-in**: exec-class tools hold `Arc<dyn Sandbox>` in their constructor and call `sandbox.execute(cmd)` directly. Non-exec tools have no Sandbox dependency. | Matches Anthropic's "Sandbox is where code runs; tool decides when it needs to"; zero middleware magic; types align with command-shaped input; grep shows which tools use sandbox |
| `SandboxCommand` shape | **Fine-grained struct** with `program / args / env / stdin / cwd / capabilities / timeout` | Capability info is explicit (audit log, approval dialog); seatbelt profile generator has complete info |
| Session context | **tokio `task_local!` SESSION_ID**; agent_loop sets via `invoke_with_session_trace` helper (Phase 2); exec tools read via `current_session()` helper | Zero `AlephTool` trait signature changes; industry-standard pattern (tracing / axum / tower); cross-process future-safe |
| Sandbox vs. PermissionLayer | **Two-layer model**: PermissionLayer enforces tool-level permission (existing agent + global system); Sandbox enforces capability-level escalation with single-request approval | Separation of concerns; no duplicated policy; two independent user consent decisions are legitimate |
| `SandboxManager` rename | **`OsSandboxDriver`** | Future-proof across Linux/Windows impls; `Driver` suffix telegraphs "low-level"; grep scales to 3-way split |
| `max_output_bytes` | Hardcoded 1 MB (512 KB stdout + 512 KB stderr) in v1 | YAGNI; revisit if users ask for more |
| `timeout` default | 60s, capped by `ToolServiceConfig.default_timeout_seconds` | Double safety net |
| `cwd: None` default | Workspace session root | Most tool cases need it; explicit override still allowed inside root |
| `file_write` / `file_edit` | **Stay outside Sandbox** — they are in-process `tokio::fs` ops; `ExecSecurityGate` governs them | Sandbox's job is subprocess execution; in-process fs is a different concern layer |
| LLM capability opt-in | Tool args carry `allow_network` / `allow_subprocess` / `extra_writable_paths` | LLM must declare intent explicitly; user approval sees the exact request |
| Permission merge rule | **Most restrictive wins**: `effective = max_severity(global, agent)` over `Allow ≺ Confirm ≺ Deny` | Safe default; global Deny is a real lockout; agents can self-restrict further |
| Approval copy | Sandbox organizes its own approval text (capability-aware) | Capability semantics are known at sandbox boundary, not at the gate |

## 4. Architecture & Decoupling Audit

### 4.1 Data flow

```
agent_loop.invoke_with_session_trace(tool_svc, session_svc, sid, ...)
      │
      ├─ emit SessionEvent::ToolCallRequested
      │
      ├─ SESSION_ID.scope(sid.clone(), tool_svc.execute(name, input))
      │                                    │
      │                                    ▼
      │                              ToolService chain (Phase 2)
      │                              Audit → Permission → ContextRule → Timeout → CoreDispatch
      │                                        │
      │                                        ├─ PermissionLayer asks LayeredPermissionResolver
      │                                        │    (global + agent → effective TrustLevel)
      │                                        │    Deny → return ToolError::PermissionDenied
      │                                        │    Confirm → ApprovalGate tool-level ask
      │                                        │    Allow → continue
      │                                        │
      │                                        ▼
      │                                   CoreDispatch → BuiltinHandler → AlephTool::call
      │                                                                      │
      │                                                                      ▼
      │                                                          (exec-class tools only)
      │                                                          sandbox.execute(SandboxCommand)
      │                                                                      │
      │                                                                      ▼
      │                                                          WorkspaceSandbox
      │                                                             │
      │                                                             ├─ lazy for_session(sid)
      │                                                             ├─ cwd validate
      │                                                             ├─ capability check
      │                                                             │    within baseline → proceed
      │                                                             │    beyond     → ApprovalGate.ask(capability)
      │                                                             │       Approved → cache grant, proceed
      │                                                             │       Denied/Timeout → CapabilityDenied
      │                                                             ├─ OsSandboxDriver.profile_for(caps, cwd)
      │                                                             ├─ OsSandboxDriver.run(program, args, ..., profile)
      │                                                             └─ capability_ledger audit log (tracing)
      │                                                                      │
      │                                                                      ▼
      │                                                          SandboxOutput → tool packs into ToolOutput
      │
      ├─ (audit layer stamps latency)
      │
      └─ emit SessionEvent::ToolResult | ToolError | ToolCallDenied
```

### 4.2 Decoupling guarantees

| Component | Phase 3 dependencies | Swappable? |
|-----------|----------------------|------------|
| `Sandbox` trait | `ApprovalGate` (shared infra), `OsSandboxDriverTrait` (own driver), task-local `SESSION_ID` (context channel, not infra) | Yes — `WorkspaceSandbox` ↔ future `ContainerSandbox` / `ProcessSandbox` / `NoOpSandbox` |
| `SessionService` | Untouched; Sandbox uses only `SessionId` value type | Yes — impls remain replaceable per Phase 1 |
| `ToolService` | Untouched at trait level; `PermissionLayer` gets real filter impl via Phase 2 backfill | Yes |
| `Harness` (future Phase 4) | Not introduced | Yes |
| `Orchestrator` (future Phase 5) | Not introduced | Yes |

**Hard invariants for the spec**:
- Sandbox **never imports** `SessionService`, `ToolService`, `Harness`, `Orchestrator`
- All five top-level traits are object-safe + `Send + Sync + 'static`
- `SESSION_ID` task-local is a context channel; no component depends on "someone sets task-local" as a lifecycle invariant — commands carry `session_id` as a field; the task-local is convenience for tools
- Exec tool → Sandbox relationship is a composition choice, not a protocol — a test Aleph can inject `NoOpSandbox` and all exec tools keep working

## 5. Sandbox trait + data types

### 5.1 Module layout

```
src/sandbox/
├── mod.rs              — trait, re-exports
├── command.rs          — SandboxCommand, SandboxOutput, NetworkPolicy
├── capabilities.rs     — SandboxCapabilities, is_within
├── context.rs          — SESSION_ID task-local + current_session()
├── workspace.rs        — WorkspaceSandbox impl
└── driver.rs           — OsSandboxDriverTrait + OsSandboxProfile
```

### 5.2 `Sandbox` trait

```rust
#[async_trait::async_trait]
pub trait Sandbox: Send + Sync + 'static {
    async fn execute(
        &self,
        command: SandboxCommand,
    ) -> Result<SandboxOutput, SandboxError>;
}
```

Single method. Everything a sandbox needs to know travels in `SandboxCommand`.

### 5.3 `SandboxCommand`

```rust
pub struct SandboxCommand {
    pub session_id: SessionId,
    pub program: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,     // additional / overrides — not a full env
    pub stdin: Option<Vec<u8>>,
    pub cwd: Option<PathBuf>,             // None = session workspace root
    pub capabilities: SandboxCapabilities,
    pub timeout: Option<Duration>,        // None = sandbox default (60s)
}
```

### 5.4 `SandboxCapabilities`

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct SandboxCapabilities {
    pub fs_read: Vec<PathBuf>,       // workspace root is always readable; these are extras
    pub fs_write: Vec<PathBuf>,      // workspace root is always writable; these are extras
    pub network: NetworkPolicy,
    pub spawn_subprocess: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NetworkPolicy {
    None,
    AllowAll,
    AllowHosts(Vec<String>),
}

impl Default for NetworkPolicy {
    fn default() -> Self { Self::None }
}

impl SandboxCapabilities {
    /// Workspace cwd read/write only, no network, no subprocess spawn.
    pub fn strict() -> Self { Self::default() }

    /// Is `self` ⊆ `baseline`? (fs_* are subset-contains, Network is ordered: None ⊆ AllowHosts ⊆ AllowAll)
    pub fn is_within(&self, baseline: &Self) -> bool { /* set-containment */ }
}
```

### 5.5 `SandboxOutput`

```rust
pub struct SandboxOutput {
    pub stdout: Vec<u8>,        // raw bytes; tool handles UTF-8 conversion
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>, // None = killed by signal
    pub signal: Option<i32>,    // Some = SIGKILL on timeout / OOM
    pub truncated: bool,        // either stream exceeded 512 KB
    pub duration_ms: u64,
}
```

### 5.6 `SandboxError`

```rust
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

Tool side maps into `ToolError` (`CapabilityDenied → PermissionDenied`, `Timeout → Timeout`, rest → `Execution`).

### 5.7 Task-local `SESSION_ID`

```rust
// src/sandbox/context.rs
tokio::task_local! {
    pub static SESSION_ID: SessionId;
}

pub fn current_session() -> Option<SessionId> {
    SESSION_ID.try_with(|sid| sid.clone()).ok()
}
```

`invoke_with_session_trace` (Phase 2 helper) gains two lines:
```rust
SESSION_ID.scope(session_id.clone(), tool_svc.execute(&name, input)).await
```

## 6. `WorkspaceSandbox` internals

### 6.1 Struct

```rust
pub struct WorkspaceSandbox {
    workspace_root: PathBuf,       // e.g., ~/.aleph/workspaces/
    sessions: Arc<RwLock<HashMap<SessionId, Arc<SessionWorkspace>>>>,
    os_driver: Arc<dyn OsSandboxDriverTrait>,
    approval_gate: Arc<ApprovalGate>,
    default_timeout: Duration,
    max_output_bytes: usize,       // 1 MB (512 KB per stream)
}

struct SessionWorkspace {
    session_id: SessionId,
    cwd: PathBuf,                  // workspace_root / session_key_to_filename(sid)
    baseline: SandboxCapabilities, // default: strict()
    granted_elevations: RwLock<HashSet<SandboxCapabilities>>,
}
```

### 6.2 Lazy provisioning

`for_session(sid)`:
1. Fast-path read lock: already exists → return
2. Upgrade to write lock, double-check, then construct
3. Create `workspace_root / session_key_to_filename(sid)/` (idempotent `tokio::fs::create_dir_all`)
4. Insert + return

### 6.3 `execute` pipeline (six steps)

```
1. resolve_session(cmd.session_id)  → SessionWorkspace
2. resolve_cwd(cmd.cwd, ws.cwd)      → PathBuf (error if outside root)
3. capability_check(cmd.capabilities, ws.baseline, ws.granted_elevations)
      beyond baseline, not already granted
        → approval_gate.ask(capability) → Approved caches grant; Denied/Timeout → Err
4. profile = os_driver.profile_for(cmd.capabilities, cwd)
5. output = os_driver.run(program, args, env, stdin, cwd, profile, timeout, max_output_bytes)
6. emit capability_ledger audit record (tracing)
```

### 6.4 `OsSandboxDriverTrait`

```rust
#[async_trait::async_trait]
pub trait OsSandboxDriverTrait: Send + Sync + 'static {
    fn profile_for(
        &self,
        capabilities: &SandboxCapabilities,
        cwd: &Path,
    ) -> Result<OsSandboxProfile, SandboxError>;

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

Concrete impl — **`OsSandboxDriver`** (renamed from `SandboxManager`). Existing macOS seatbelt logic is preserved; the rename restructures the module so `profile_for` and `run` are public trait methods rather than inherent methods.

### 6.5 `session_key_to_filename`

Given `SessionKey`, produce a filesystem-safe directory name. Strategy: `serde_json::to_string(&sid)` → hex-hash (SHA-256, first 16 bytes as hex). Avoids slashes, keeps it < 64 chars. Hash rather than raw JSON because `SessionKey` contains variants with special chars.

### 6.6 Session cleanup

Phase 3 does **not** auto-delete workspaces. A future `sandbox.cleanup(session_id)` Gateway RPC may be added on user demand.

## 7. OsSandboxDriver rename

Mechanical:
- `src/exec/sandbox/executor.rs::SandboxManager` → `OsSandboxDriver`
- Move `generate_profile` and `execute_sandboxed` methods into the `OsSandboxDriverTrait` impl
- Update all call sites in `src/exec/**` and `src/tools/**`
- Module docstring makes layering explicit: "OS-level sandbox-exec profile driver. Consumed by `WorkspaceSandbox`. Do not confuse with `src/sandbox/mod.rs::Sandbox`."

Behavior is not changed. Existing unit + integration tests continue to pass.

## 8. Agent permission system integration (Phase 2 backfill)

### 8.1 Discovery (Phase 3 Task 0)

Execute grep cascade to locate the existing **global** + **per-agent** tool permission systems (both are Deny/Confirm/Allow). Expected findings:
- Global permissions — likely in `aleph.toml` under a `[tools]`-ish section with a map `tool_name → TrustLevel`
- Per-agent permissions — in the agent / persona config with the same shape
- Session ↔ agent mapping — session stores or computes agent_id

### 8.2 `ToolPermissionResolver` trait

```rust
#[async_trait::async_trait]
pub trait ToolPermissionResolver: Send + Sync + 'static {
    async fn trust_for(&self, session_id: &SessionId, tool_name: &str) -> TrustLevel;
}
```

### 8.3 `LayeredPermissionResolver`

```rust
pub struct LayeredPermissionResolver {
    global:        Arc<ArcSwap<GlobalToolPermissions>>,
    session_agent: Arc<dyn SessionAgentResolver>,
    agents:        Arc<ArcSwap<HashMap<AgentId, AgentPermissions>>>,
}

#[async_trait::async_trait]
impl ToolPermissionResolver for LayeredPermissionResolver {
    async fn trust_for(&self, sid: &SessionId, tool: &str) -> TrustLevel {
        let global_trust = self.global.load().get(tool).unwrap_or(TrustLevel::Confirm);
        let agent_id = self.session_agent.agent_for(sid).await;
        let agent_trust = self.agents.load()
            .get(&agent_id)
            .map(|p| p.get(tool).unwrap_or(global_trust))
            .unwrap_or(global_trust);
        effective_trust(global_trust, agent_trust)
    }
}

fn effective_trust(global: TrustLevel, agent: TrustLevel) -> TrustLevel {
    match (global, agent) {
        (TrustLevel::Deny, _)    | (_, TrustLevel::Deny)    => TrustLevel::Deny,
        (TrustLevel::Confirm, _) | (_, TrustLevel::Confirm) => TrustLevel::Confirm,
        _ => TrustLevel::Allow,
    }
}
```

Default fall-back = `Confirm` (safer than Allow, friendlier than Deny).

### 8.4 `AgentPermissionFilter` wires resolver to `SmartFilter` trait

```rust
pub struct AgentPermissionFilter {
    resolver: Arc<dyn ToolPermissionResolver>,
}

#[async_trait::async_trait]
impl SmartFilter for AgentPermissionFilter {
    async fn classify(&self, tool_name: &str) -> Classification {
        let sid = current_session().unwrap_or_else(SessionId::default_ephemeral);
        match self.resolver.trust_for(&sid, tool_name).await {
            TrustLevel::Allow   => Classification::Allow,
            TrustLevel::Confirm => Classification::Confirm { reason: format!("agent/global policy for '{tool_name}'") },
            TrustLevel::Deny    => Classification::Deny    { reason: format!("tool '{tool_name}' is disabled") },
        }
    }
}
```

### 8.5 Trait signature change

`SmartFilter::classify` becomes **async** (was sync in Phase 2). Impact: update `ScriptedFilter` test mock and one `.await` in `PermissionLayer::execute`.

### 8.6 Production wiring

`build_tool_service` in `src/tools/facade.rs` changes from `None` / no-filter default to:
```rust
let resolver = Arc::new(LayeredPermissionResolver::new(global, session_agent, agents));
let filter   = Arc::new(AgentPermissionFilter::new(resolver));
let approver = Arc::new(ApprovalGate::cloned_from_app_context(...));
let perm     = Arc::new(PermissionLayer::with_policy(ctxrule, filter, approver));
```

## 9. Exec-class tools routing

### 9.1 Enumeration strategy

Exec-class = any tool that calls `Command::new(...)` (or `tokio::process::Command::new(...)`) today. Discovery:
```bash
grep -rn 'Command::new\|tokio::process::Command::new' src/builtin_tools/
```
Every hit maps to one tool to migrate. Expect ~5–10 tools.

**Non-exec tools stay untouched** — memory_*, llm_call, thinker_*, session_*, gateway_route, etc. Pure in-process operations (including `tokio::fs`) stay outside Sandbox; their policy is governed by existing `ExecSecurityGate`.

### 9.2 Migration per tool

Before:
```rust
pub struct BashExec { ... }
impl AlephTool for BashExec {
    async fn call(&self, args: BashExecArgs) -> Result<BashExecOutput> {
        let output = tokio::process::Command::new("bash").arg("-c").arg(&args.command).output().await?;
        ...
    }
}
```

After:
```rust
pub struct BashExec { sandbox: Arc<dyn Sandbox> }

impl BashExec { pub fn new(sandbox: Arc<dyn Sandbox>) -> Self { Self { sandbox } } }

impl AlephTool for BashExec {
    async fn call(&self, args: BashExecArgs) -> Result<BashExecOutput> {
        let session_id = crate::sandbox::current_session()
            .ok_or_else(|| anyhow!("bash_exec requires session context"))?;
        let cmd = SandboxCommand {
            session_id,
            program: "bash".into(),
            args: vec!["-c".into(), args.command.clone()],
            env: Default::default(),
            stdin: None,
            cwd: None,
            capabilities: args.into_capabilities(),
            timeout: args.timeout_secs.map(Duration::from_secs),
        };
        let out = self.sandbox.execute(cmd).await.map_err(sandbox_err_into_anyhow)?;
        Ok(BashExecOutput { /* unpack SandboxOutput */ })
    }
}
```

### 9.3 Tool args carry capability declarations

Example:
```rust
pub struct BashExecArgs {
    pub command: String,
    #[serde(default)] pub allow_network: bool,
    #[serde(default)] pub allow_subprocess: bool,
    #[serde(default)] pub extra_writable_paths: Vec<PathBuf>,
    pub timeout_secs: Option<u64>,
}

impl BashExecArgs {
    fn into_capabilities(&self) -> SandboxCapabilities {
        SandboxCapabilities {
            fs_read: vec![],
            fs_write: self.extra_writable_paths.clone(),
            network: if self.allow_network { NetworkPolicy::AllowAll } else { NetworkPolicy::None },
            spawn_subprocess: self.allow_subprocess,
        }
    }
}
```

LLM must explicitly request capabilities beyond baseline. Users then see approval prompts that describe exactly what the tool asked for.

### 9.4 Builtin registry wiring

`src/executor/builtin_registry/` (and its consumers in `src/bin/aleph-server/commands/start/builder/`) must pass `Arc<dyn Sandbox>` to the constructors of exec-class tools during registration. The `Arc<dyn Sandbox>` itself is constructed once at boot via `build_sandbox(approval_gate, os_driver, config)`.

## 10. Migration Strategy (Strangler)

Each step is independently shippable.

### 10.1 Phase 2 backfill — wire real permissions (preferably a single atomic commit)
- Discover global + agent permission types + session↔agent mapping
- Change `SmartFilter::classify` to async
- Implement `LayeredPermissionResolver` + `AgentPermissionFilter`
- Inject into `build_tool_service`
- 9-cell matrix tests for `effective_trust`

### 10.2 Sandbox module scaffold
- New files in `src/sandbox/` — types only, stubs for `WorkspaceSandbox` and traits
- `cargo check` green

### 10.3 Rename SandboxManager → OsSandboxDriver
- Mechanical rename across `src/exec/sandbox/**` and all callers
- Define `OsSandboxDriverTrait` and impl it for `OsSandboxDriver`
- All existing tests green

### 10.4 `WorkspaceSandbox` implementation
- Per-session cache + lazy provisioning
- Full six-step `execute` pipeline
- Unit tests: happy / cwd escape / capability denied / capability granted-via-approval / timeout

### 10.5 `SESSION_ID` task-local plumbing
- Define task-local + helper
- Extend Phase 2's `invoke_with_session_trace` with `SESSION_ID.scope(...)` wrap
- Tests: current_session Some inside scope, None outside

### 10.6 AppContext assembly
- `build_sandbox(...)` helper
- Inject `Arc<dyn Sandbox>` into boot wiring

### 10.7 Migrate exec-class tools — one commit per tool
- Discover list via grep
- For each tool: add `sandbox: Arc<dyn Sandbox>` field, migrate `call()`, update unit tests
- Integration smoke after each

### 10.8 Documentation + CHANGELOG
- New `docs/reference/SANDBOX.md`
- Update `GLOSSARY.md` (Sandbox entry → present tense)
- CHANGELOG entry

### 10.9 Exit gate
- `grep -rn 'Command::new\|tokio::process::Command' src/builtin_tools/` → zero hits (exec spawning moved to `OsSandboxDriver`)
- `grep -rn '\bSandboxManager\b' src/` → zero hits
- Phase 2 `ScriptedFilter` placeholder only appears in test fixtures; production path uses `AgentPermissionFilter`
- Capability approval flow demonstrable via integration test (bash_exec with `allow_network: true` → mock gate receives request)
- `cargo test -p alephcore --lib` matches baseline (9029 passed / 2 pre-existing failed)

## 11. Testing Strategy

### Unit tests (by module)

| Module | Representative cases |
|--------|----------------------|
| `SandboxCapabilities` | `strict()` default; `is_within` fs_* subset, Network ordered, spawn_subprocess flag |
| `SESSION_ID` task-local | scope set/get; nested scope inner wins; out-of-scope returns None; subtask spawn doesn't inherit (documented) |
| `OsSandboxDriver::profile_for` | strict → most restrictive profile; network=AllowAll → open; fs_write: [p] → profile includes p |
| `OsSandboxDriver::run` | echo happy path; non-zero exit; SIGKILL on timeout; truncation; stderr independent |
| `WorkspaceSandbox::for_session` | first call creates dir; subsequent returns same Arc; concurrent first-call creates once (race test) |
| `WorkspaceSandbox::execute` | cwd None → root; cwd inside root → allowed; cwd outside → CapabilityDenied |
| `WorkspaceSandbox::execute` | baseline request skips approval; over-baseline first time asks; grant cached; subsequent same-or-narrower request skips; broader request reasks |
| `WorkspaceSandbox::execute` | timeout triggers Timeout error with elapsed_ms |
| `LayeredPermissionResolver` | full 3×3 matrix for `effective_trust` |
| `AgentPermissionFilter::classify` | reads task-local session, queries resolver, maps to Classification correctly |

### Integration tests (`tests/sandbox_*.rs`)

1. End-to-end exec-class tool via gateway → bash_exec echo → workspace created + SessionEvent log
2. Capability escalation + approval — allow_network: true → mock gate Approved → sandbox proceeds → ledger + ToolCallApproved event
3. Capability denied — mock gate Denied → ToolCallDenied event
4. Layered permission tool-level deny — global=Allow, agent=Deny → PermissionLayer short-circuits; Sandbox never invoked
5. Cross-session isolation — session A file created in A/, session B cannot read it through sandbox
6. Task-local propagation — bash_exec via `invoke_with_session_trace` sees correct session id

### Regression

- All existing exec tool tests green
- All existing `SandboxManager` (now `OsSandboxDriver`) integration tests green
- Phase 2 `tools::middleware::*` tests green
- Phase 1 `session::*` tests green

### Performance baselines (non-blocking)

- First `execute` per session (including workspace dir creation): < 200ms
- Subsequent: < 50ms sandbox overhead (excluding actual command time)
- Capability approval cache hit: < 1ms increment

### Hard test gate

`cargo test -p alephcore --lib` → **9029+ passed / 2 failed** (the 2 pre-existing baseline failures; Phase 3 must not add any new failures).

## 12. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Existing permission types don't match the two-tier mental model (e.g. only per-agent exists, no global) | Medium | Medium | Discovery task gates everything; if global doesn't exist, resolver simplifies (agent-only); design still works |
| `SmartFilter::classify` async change breaks downstream callers we don't know about | Low | Medium | Phase 2 trait is new; grep confirms callers; small blast radius |
| macOS-only seatbelt means Linux devs can't run exec tools during development | Medium | Medium | `OsSandboxDriver` on non-macOS falls back to "no sandbox" mode with loud `tracing::warn!`; gated by config flag; tests use stubbed driver |
| `tokio::spawn` inside tool code loses task-local session_id | Medium | Low | Document prominently; `SESSION_ID.sync_scope(sid, fut)` pattern in helper for subtasks |
| Capability approval UX is confusing when tool + sandbox both ask | Medium | Low | First-tier (tool) asks "can agent use bash_exec?"; second-tier (sandbox) asks "can this call access the network?" — distinct questions, acceptable UX in v1 |
| Workspace directory bloat (never auto-cleaned) | Low | Low | Document; expose future `sandbox.cleanup(sid)` RPC; user can rm manually |
| Rename of SandboxManager breaks an out-of-tree consumer | Low | Low | `OsSandboxDriver` is purely Aleph-internal; no external consumers |
| Session ↔ agent mapping is not a pure function (agent can switch mid-session) | Low | Medium | Resolver always re-queries on each tool call; cached snapshots use ArcSwap so reads are atomic |

## 13. Open Questions (deferred to implementation)

- Exact location of global tool permissions — resolved during discovery task
- Whether `AgentPermissionFilter::classify` needs access to anything beyond `session_id` + `tool_name` (e.g., arg content for context-based rules) — answer: no in v1; Phase 4's harness rewrite can add context if needed
- Should `extra_writable_paths` be normalized to absolute paths before comparison — yes; document in `SandboxCapabilities::is_within`
- Whether non-macOS platforms ship a true stub `OsSandboxDriver` or fail at Sandbox::execute with a helpful error — defer to impl; recommend stub with warning so Aleph remains usable on Linux dev machines

## 14. Success Metrics

- `grep -rn 'Command::new\|tokio::process::Command' src/builtin_tools/` → zero hits
- `grep -rn '\bSandboxManager\b' src/` → zero hits
- Phase 2 `ScriptedFilter` appears only in test code
- Capability approval dialog demonstrable end-to-end with mock gate
- New `src/sandbox/` module ≥ 80% line coverage
- `cargo test -p alephcore --lib` baseline unchanged (9029+ passed / 2 failed)
- Daily release cadence uninterrupted

## 15. Next Action

1. User reviews this spec
2. On approval → invoke `writing-plans` skill to produce a task-level implementation plan
3. Implementation via subagent-driven-development, matching Phase 0 / 1 / 2 pattern
