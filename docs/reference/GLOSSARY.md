# Aleph Glossary — Managed-Agents Aligned

Terminology in Aleph is aligned with Anthropic's managed-agents paradigm ([blog](https://www.anthropic.com/engineering/managed-agents)). This file is the single source of truth. If any other doc conflicts with this, this wins.

## Core terms

### Harness
**Anthropic meaning:** The loop that calls the LLM and routes tool calls to relevant infrastructure. Stateless; recoverable via `wake(session_id)` after crashes.

**Aleph today:** The Think→Act loop lives in `src/agent_loop/loop_core.rs` (pre-refactor) or `src/harness/` (post-Phase-4). No external-CLI meaning.

### Sandbox
**Anthropic meaning:** Execution environment where the agent runs code and edits files. Provisioned on-demand via `execute(name, input) → string`.

**Aleph today:** The agent-level `Sandbox` trait (`src/sandbox/mod.rs`) is the workspace + capability-ledger abstraction. Production boot wires `WorkspaceSandbox` (cwd + macOS seatbelt + approval gate). See [SANDBOX.md](./SANDBOX.md) for the pipeline, capabilities, and testing pattern.

**Do not confuse with:** `OsSandboxDriver` / `ExecSecurityGate` / `ApprovalGate` — these are lower-level primitives that sit *beneath* the `Sandbox` trait. `OsSandboxDriver` is the macOS `sandbox-exec` driver invoked by `WorkspaceSandbox`; `ExecSecurityGate` is the pre-exec filesystem guard for `file_write`/`file_edit`; `ApprovalGate` is the user-facing prompt path for capability elevation.

### WorkspaceSandbox
**Aleph-specific:** Concrete `Sandbox` implementation (`src/sandbox/workspace.rs`). Provisions `~/.aleph/workspaces/{hash(session_id)}/` lazily on first exec-class call, enforces a strict per-session capability baseline, escalates out-of-baseline requests through `ApprovalGate`, caches per-session grants, and delegates OS isolation to an `OsSandboxDriverTrait` implementation. Implements the six-step execute pipeline described in [SANDBOX.md](./SANDBOX.md).

### OsSandboxDriver
**Aleph-specific:** macOS `sandbox-exec` driver (`src/exec/sandbox/executor.rs`). Implements `OsSandboxDriverTrait` so `WorkspaceSandbox` can generate SBPL profiles and run subprocesses under seatbelt. Formerly named `SandboxManager`; renamed in Phase 3 Task 4 to reflect its OS-level role and free the name for Anthropic's agent-level Sandbox meaning.

### OsSandboxDriverTrait
**Aleph-specific:** The seam between `WorkspaceSandbox` and OS-level seatbelt (`src/sandbox/driver.rs`). Two methods: `profile_for(caps, cwd) -> OsSandboxProfile` and `run(program, args, env, stdin, cwd, profile, timeout, max_output_bytes) -> SandboxOutput`. Lets tests substitute a fake driver without invoking the real `sandbox-exec` binary.

### SandboxCapabilities
**Aleph-specific:** What a subprocess is allowed to do (`src/sandbox/capabilities.rs`): `fs_read`, `fs_write`, `network` (`None`/`AllowHosts`/`AllowAll`), `spawn_subprocess`. `::strict()` is the workspace baseline (no fs outside cwd, no network, no spawn). `is_within(&baseline)` enforces monotonic subset semantics: prefix-subset for paths, ordered `None ⊆ AllowHosts ⊆ AllowAll` for network, and `false ⊆ any` for spawn.

### LayeredPermissionResolver
**Aleph-specific:** Concrete `SmartFilter` implementation (`src/tools/middleware/permission/resolver.rs`) backed by a merged two-tier `ToolPermissionsConfig` (global + per-agent, most-restrictive-wins). Classifies each tool call as `Allow` / `Confirm` / `Deny` by consulting `PermissionAction::Allow/Ask/Deny`. Live-reloadable via `ArcSwap`. Backfills the Phase 2 placeholder filter with real policy.

### AgentPermissionFilter
**Aleph-specific:** Convenience builder (`src/tools/middleware/permission/agent_filter.rs`) that takes a global + per-agent `ToolPermissionsConfig`, merges them, and returns an `Arc<dyn SmartFilter>` ready to hand to `PermissionLayer::set_smart_filter`. Used by orchestrator paths that know which agent is running.

### Session
**Anthropic meaning:** Append-only log recording everything that happened during an agent's work. Persists independently outside the harness; accessed via `getEvents()` / `emitEvent()`.

**Aleph today:** `SessionService` trait (`src/session/`), backed by an in-process tokio actor with SQLite persistence. Trait shape permits cross-process backends later. Gateway `session.*` RPC still routes through the legacy `SessionManager` during Phase 1 (migrated in Phase 6); every SessionManager append is mirrored into SessionService via a dual-write shim. See [SESSION_SERVICE.md](./SESSION_SERVICE.md).

### Tools
**Anthropic meaning:** The "hands" — custom tools, MCP servers, execution environments — all reached through one `execute()` surface. The brain is agnostic to the backing.

**Aleph today:** `ToolService` trait (`src/tools/service.rs`), backed by
`CoreDispatch` + `ArcSwap`-backed `ToolRegistry`, with a five-layer decorator
chain (Audit / Permission / ContextRule / Timeout / Core). Three handler sources
(`BuiltinHandler`, `McpHandler`, `ExtensionHandler`) adapt existing tools without
changing their author-side `AlephTool` trait. Gateway `tools.*` RPC still routes
through the legacy `AlephToolServer` in Phase 2 (future phase migrates).
See [TOOL_SYSTEM.md](./TOOL_SYSTEM.md).

### Orchestrator
**Anthropic meaning:** Infrastructure managing session state, sandbox provisioning, and routing between brains and hands.

**Aleph today:** `src/orchestrator/` module (post-Phase-5). Owns session lifecycle + Harness dispatch + Sandbox provisioning + `FlowSpec` composition.

## Adapter terms (not Anthropic)

### AcpAdapter
**Aleph-specific:** A Rust adapter that bridges an external CLI tool (claude-code, codex, gemini-cli, opencode, …) to the Agent Client Protocol. Formerly called `AcpHarness`; renamed in Phase 0 to free "Harness" for its Anthropic meaning.

Defined in `src/acp/adapter.rs` (trait) and `src/acp/adapters/` (implementations).

### Brain / Hands
**Anthropic shorthand, used informally:**
- **Brain:** LLM + Harness
- **Hands:** Sandbox + Tools

## Phase reference

This glossary's forward-looking terms align with the 6-phase refactor roadmap: `docs/superpowers/specs/2026-04-18-managed-agents-refactor-roadmap.md`.
