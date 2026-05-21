# Agent Files Directory Fix

**Date:** 2026-03-19
**Status:** Approved
**Scope:** Bug fix — agent identity files written to wrong directory

## Problem

Agent identity/bootstrap files (SOUL.md, AGENTS.md, MEMORY.md, etc.) are written to
`~/.aleph/workspaces/{agent_id}/` instead of `~/.aleph/agents/{agent_id}/`.

The design intent is:
- `~/.aleph/agents/{id}/` — identity files + sessions (agent state)
- `~/.aleph/workspaces/{id}/` — tool output, project files (work products)

Currently both directories contain duplicate SOUL.md files. The Panel's "agent files" API
also reads from the wrong directory (workspaces instead of agents).

## Root Cause

`initialize_workspace()` is called with `workspace_path` (pointing to workspaces/) at three
call sites. `AgentManager::workspace_files.rs` operations use `self.workspace_root` instead
of `self.agents_root`.

Additionally, `create.rs` (AgentCreateTool) calls `initialize_workspace` AND then
`AgentManager::create()` calls it again — double initialization writing to wrong dir.

## Design

### Directory Semantics (unchanged, just enforced)

| Directory | Contains | Purpose |
|-----------|----------|---------|
| `~/.aleph/agents/{id}/` | SOUL.md, IDENTITY.md, AGENTS.md, TOOLS.md, MEMORY.md, HEARTBEAT.md, BOOTSTRAP.md, sessions/ | Agent identity & state |
| `~/.aleph/workspaces/{id}/` | output/, .tool_output/, project files | Tool execution working dir |

### Changes

#### 1. Rename `workspace_files.rs` → `agent_files.rs`

**File:** `src/config/agent_manager/workspace_files.rs` → `agent_files.rs`

- All `self.workspace_root` → `self.agents_root`
- Error messages: "workspace" → "agent"
- `mod.rs`: `mod workspace_files` → `mod agent_files`

#### 2. Fix `initialize_workspace()` call sites

Three call sites pass the wrong path:

1. **`agent_resolver.rs:235`** — `initialize_workspace(&workspace_path, ...)` → `initialize_workspace(&agent_dir, ...)`
   - Also update misleading comment on line 234: "Workspace files...go in workspace_path" → "Identity files...go in agent_dir"
2. **`crud.rs:228-235`** — `initialize_workspace(&ws_dir, ...)` where `ws_dir = self.workspace_root.join(...)` → use `agent_state_dir` (already defined as `self.agents_root.join(...)` on line 218)
   - Remove the separate `ws_dir` variable for identity files; keep workspace dir creation for tool output
3. **`create.rs:231-312`** — hardcoded `~/.aleph/workspaces` path for identity files → use `~/.aleph/agents`
   - Lines 232-235: change `workspaces_dir` to agents dir for `initialize_workspace` call
   - Lines 258, 275, 298, 308: subsequent writes to SOUL.md, IDENTITY.md, TOOLS.md, AGENTS.md must also use agents path
   - Still create workspace dir for `AgentInstanceConfig.workspace` (tool execution)

#### 3. Fix `reconcile_orphan_workspaces`

**File:** `crud.rs:75`

Current: scans `self.workspace_root` and reads IDENTITY.md from there.
Fix: scan `self.agents_root` instead (identity files will only exist there after fix).

#### 4. Update tests

- `test_create_agent` (tests.rs ~line 97): change `mgr.workspace_root.join("researcher").join("SOUL.md")` → `mgr.agents_root.join(...)`
- `test_creates_dual_directories` (tests.rs ~line 228): change `mgr.workspace_root.join("dual").join("SOUL.md")` → `mgr.agents_root.join(...)`
- All other test assertions checking for identity files in workspace_root
- Workspace file CRUD tests (list_files, read_file, write_file, delete_file) should now operate on agents_root

### Not Changed

- `ResolvedAgent.workspace_path` — stays pointing to workspaces/ (tool execution dir)
- `AgentInstanceConfig.workspace` — stays as workspaces/ (run_loop working dir)
- `thinker/workspace_files.rs` — separate module for prompt injection, takes arbitrary path
- `default_workspace_root()` / `default_agents_root()` — defaults unchanged
