# Aleph Glossary — Managed-Agents Aligned

Terminology in Aleph is aligned with Anthropic's managed-agents paradigm ([blog](https://www.anthropic.com/engineering/managed-agents)). This file is the single source of truth. If any other doc conflicts with this, this wins.

## Core terms

### Harness
**Anthropic meaning:** The loop that calls the LLM and routes tool calls to relevant infrastructure. Stateless; recoverable via `wake(session_id)` after crashes.

**Aleph today:** The Think→Act loop lives in `src/agent_loop/loop_core.rs` (pre-refactor) or `src/harness/` (post-Phase-4). No external-CLI meaning.

### Sandbox
**Anthropic meaning:** Execution environment where the agent runs code and edits files. Provisioned on-demand via `execute(name, input) → string`.

**Aleph today:** The agent-level `Sandbox` trait (post-Phase-3, `src/sandbox/`) is the workspace + capability-ledger abstraction. Implementations include `WorkspaceSandbox` (cwd + macOS seatbelt + approval gate).

**Do not confuse with:** `SandboxManager` / `ExecSecurityGate` / `ApprovalGate` — these are lower-level OS-sandbox primitives that sit *beneath* the `Sandbox` trait. Their names may change in Phase 3 for clarity.

### Session
**Anthropic meaning:** Append-only log recording everything that happened during an agent's work. Persists independently outside the harness; accessed via `getEvents()` / `emitEvent()`.

**Aleph today:** `SessionService` trait (`src/session/`), backed by an in-process tokio actor with SQLite persistence. Trait shape permits cross-process backends later. Gateway `session.*` RPC still routes through the legacy `SessionManager` during Phase 1 (migrated in Phase 6); every SessionManager append is mirrored into SessionService via a dual-write shim. See [SESSION_SERVICE.md](./SESSION_SERVICE.md).

### Tools
**Anthropic meaning:** The "hands" — custom tools, MCP servers, execution environments — all reached through one `execute()` surface. The brain is agnostic to the backing.

**Aleph today:** `ToolService` façade (post-Phase-2, `src/tools/`) unifies builtin / MCP / extension dispatch behind one `execute(name, input) → ToolOutput` call.

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
