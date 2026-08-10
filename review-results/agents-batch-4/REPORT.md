# Review Report — Batch 4 (Subagent spawner + supporting modules)

**Scope:**
- `src/agents/subagent_spawner/mod.rs` (1015 LOC)
- `src/agents/sub_agents/mod.rs` (8 LOC)
- `src/agents/sub_agents/traits.rs` (128 LOC)
- `src/agents/allowlist_tool_service.rs` (330 LOC)
- `src/agents/teammates.rs` (80 LOC)
- `src/agents/thinking.rs` (233 LOC)
- `src/agents/tool_sets.rs` (132 LOC)
- `src/agents/progress.rs` (39 LOC)

**Date:** 2026-08-10
**Reviewer:** static (4-perspective protocol)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 3 |
| Medium   | 3 |
| Low      | 1 |

## Findings

### [HIGH] src/agents/subagent_spawner/mod.rs:896 — `parent_session_id_of` JSON-parses a value that is always a key-string, so `SubagentSpawned` / `SubagentReturned` are never emitted

**Category:** Logic
**Confidence:** High

**Description:**
`parent_session_id_of` reads the parent session id with `serde_json::from_str::<SessionId>(raw).ok()`. `SessionId` is `routing::session_key::SessionKey`, an **internally tagged** enum (`#[serde(tag = "type", rename_all = "snake_case")]`, `session_key.rs:31-33`) — deserializing it requires a JSON *object*. The one production wiring site supplies the flat key-string instead:

```
src/gateway/execution_engine/run_loop/inner.rs:1054
    .with_parent_session_id(request.session_key.to_key_string())   // e.g. "agent:main:main"
```

`"agent:main:main"` is not a JSON document at all, so `from_str` errors and `.ok()` yields `None` on every production spawn. Both `emit_event` call sites are guarded by that `Some(..)`:

- `mod.rs:412-427` — `SessionEvent::SubagentSpawned` into the parent log
- `mod.rs:689-704` — `SessionEvent::SubagentReturned` into the parent log

so neither durable event is ever written. `subagent_tool::recovery` (`recovery.rs:186-190`) resolves the same string through the *same* function, so the entire event-log recovery path (`Recovered::Completed` / `Recovered::Interrupted`) is unreachable in production — exactly the "reader finds an empty log and reports unknown forever, a silent failure with no error path" outcome the doc comment on `parent_session_id_of` warns about. Only the `background_persistence` sidecar path still recovers anything. No test covers `parent_session_id_of`; the one spawner test that sets `parent_session_id` (`tests.rs:824`) asserts only the `RawMemory(Delegation)` row, which consumes the raw string and is therefore unaffected.

Note the same field is consumed correctly as a raw key-string two other places in this file (`mod.rs:581-591` `ToolResultStore::for_session`, `mod.rs:709-723` delegation emit), which confirms the string's actual shape.

**Failure scenario:** A background sub-agent finishes, the daemon restarts, the model calls `subagent(check_status, request_id=…)`. The tracker is process memory and is empty; recovery scans the parent session log, finds no `SubagentReturned`, and the completed work + its output are reported as unknown/re-runnable.

**Suggested fix:**
```rust
pub fn parent_session_id_of(raw: &str) -> Option<SessionId> {
    // Callers pass `SessionKey::to_key_string()` (see run_loop/inner.rs).
    SessionKey::parse(raw)
}
```
(`SessionKey::parse` at `session_key.rs:383` is the existing string reader; `gateway/orphan_notice.rs:37` already uses it for the same kind of value.) Add a round-trip guard that feeds `SessionKey::Main{..}.to_key_string()` through `parent_session_id_of` and asserts `Some`, and an integration assertion that a spawn with `parent_session_id` set writes `SubagentSpawned` into the parent log.

---

### [HIGH] src/agents/subagent_spawner/mod.rs:231 — `IsolationMode::Worktree` anchors on the server process cwd, not the run's project root

**Category:** Logic / Security (isolation boundary)
**Confidence:** High

**Description:**
The worktree repo root is taken from the process's current directory:

```rust
// mod.rs:231-239
let repo_root = tokio::task::spawn_blocking(std::env::current_dir).await…;
let handle = crate::sandbox::worktree::create(&repo_root, label, …).await…;
```

`aleph-server` is a long-lived daemon; its cwd is wherever it was launched (`/` under the Tauri shell / launchd, the operator's shell cwd otherwise) and has no relationship to the run's project. The main run path anchors its `FsScope` on `request.workspace_override` / `agent.workspace()` (`gateway/execution_engine/run_loop/mod.rs:204-225`), and this very function reads the run's project root 390 lines later (`mod.rs:621`, `crate::projects::current_project_root()`), so the correct anchor is available in scope and simply not used here.

Two distinct failure modes:
1. **cwd is not a git repo** — `worktree::create` returns `NotAGitRepo` (`sandbox/worktree.rs:141-143`) and the spawn hard-fails with `"sub-agent failed: worktree create: …"`. Every `isolation: worktree` agent is unusable.
2. **cwd is a *different* git repo** — the child is checked out from, and `FsScope::worktree(wt, repo)` (`mod.rs:296`) rebases parent-repo absolute paths into, the wrong tree. The isolated child then reads/writes the wrong repository while reporting success, and `WorktreeSandbox` (`mod.rs:306-310`) executes its commands there too.

**Failure scenario:** Operator authors `~/.aleph/agents/refactorer.md` with `isolation: {kind: worktree}` and delegates a refactor while working in project `~/code/svc-a`. The daemon was started from `~/code/tooling`; the child silently branches, edits and builds `~/code/tooling`, and reports the refactor as done.

**Suggested fix:** Prefer the run's project root and only fall back to cwd:
```rust
let repo_root = match crate::projects::current_project_root() {
    Some(root) => root,
    None => tokio::task::spawn_blocking(std::env::current_dir).await…?,
};
```
and add a guard asserting the provisioned worktree's `repo_root()` equals the run's project root when one is published.

---

### [HIGH] src/agents/subagent_spawner/mod.rs:483-489 — inline MCP servers are spawned and advertised to the child, but no execution path exists

**Category:** Architecture / Logic
**Confidence:** High

**Description:**
When `agent_def.mcp_servers` contains an `Inline` spec, `spawn()` provisions a real child process via `McpScope::provision` (`mod.rs:248-267` → `mcp_registrar.rs:346-370`), snapshots its `tools/list`, and layers those registrations into the child's tool surface:

```rust
// mod.rs:483-489
Some(scope) => Arc::new(crate::tools::mcp_scope_view::McpScopedToolService::new(
    base.parent_tools.clone(), scope.tools(),
)),
```

`McpScopedToolService` surfaces the extras in `list()`, `dispatchable_list()`, `describe()` and `metadata_schema()` — i.e. they reach the child LLM's tool catalog — but its execute arms forward **only** to the parent:

```rust
// src/tools/mcp_scope_view.rs:66-68
// Parent first. Stage I MVP: extras execution deferred to Task 12.
self.parent.execute(name, input).await
```

The parent `ScopedToolService` has no knowledge of the inline process (it was never registered in `PluginRegistry` — `provision` explicitly *rejects* names that collide with it, `mcp_registrar.rs:194-198`), so every call the child makes to an inline tool bounces to tool-not-found. The routing the registrar's doc promises ("the agent_spawner wiring which routes `inline:<server>:<tool>` calls back to the matching `InlineMcpHandle`", `mcp_registrar.rs:243-247`) does not exist anywhere in the tree. `call_concurrency_claim` has the same gap (`mcp_scope_view.rs:113-121`).

**Failure scenario:** An agent def declares an inline MCP server. Every spawn starts the server process, pays its handshake, hands the child a catalog entry it can see and cannot call, and the child burns its iteration budget retrying a tool that will never resolve — then the process is torn down.

**Suggested fix:** Either (a) wire execution: hold the `McpScope` (or an `Arc<InlineMcpHandle>` map) inside `McpScopedToolService` and route `name` whose registration carries `plugin_id == "inline:<server>"` to `handle.process.call_tool(...)`, mirroring the claim in `call_concurrency_claim`; or (b) until that lands, stop advertising what cannot be dispatched — filter `inline_tools` out of `McpScope::tools()` and fail `provision` loudly for `Inline` specs, so the gap is visible at spawn instead of at the model's first tool call.

---

### [MEDIUM] src/agents/subagent_spawner/mod.rs:770-781 — `context_summary` is silently discarded for every `Fresh`-mode agent, including the three shipped delegates

**Category:** Logic
**Confidence:** High

**Description:**
`build_effective_task` prepends the parent's summary only when the child's declared `context_mode` is `Summary`:

```rust
Some(summary) if context_mode == ContextMode::Summary => { … }
_ => task.to_string(),
```

`ContextMode::default()` is `Fresh` (`agents/types.rs:85-91`) and the loader only sets it when frontmatter declares it (`loader.rs:172-174`). Of the shipped built-ins, `explore` (`registry.rs:344`), `coder` (`registry.rs:380`) and `researcher` (`registry.rs:393`) declare none — only `default`, `plan` and `verify` set `Summary`. Meanwhile the `subagent` tool schema advertises the parameter unconditionally and without qualification:

```json
"context_summary": { "type": "string",
  "description": "A summary of the parent agent's context to pass to the sub-agent." }
```
(`subagent_tool/loop_tool.rs:128-131`)

So the model composes a context summary, passes it to the three most-used roles, receives no error and no signal, and the child starts from the bare task. This is the "two statements of one fact, only one updated" shape called out in `CLAUDE.md` §0 — and the copy that lies is the one shipped to the model in the tool `DESCRIPTION`.

**Failure scenario:** Parent delegates "continue the migration described above" to `explore` with a 2 KB `context_summary`. The child receives only the task string, has no idea what "the migration" is, and returns an off-target answer that the parent has no way to attribute to a dropped parameter.

**Suggested fix:** Either honour the caller's explicit summary regardless of `context_mode` (making `context_mode` the *default* rather than a veto), or keep the veto and make it observable: return the dropped-summary fact in the tool result / annotate the schema description with "ignored unless the target agent declares `context_mode: summary`", and set `Summary` on `explore` / `coder` / `researcher`.

---

### [MEDIUM] src/agents/subagent_spawner/mod.rs:738-761 — worktree / MCP cleanup runs only on success; the error path falls to a blocking `Drop`

**Category:** Quality / Logic
**Confidence:** High

**Description:**
Both explicit teardowns are gated on `result.is_ok()`:

```rust
if result.is_ok() { if let Some(scope) = mcp_scope { scope.shutdown().await … } }
if result.is_ok() { if let Some(h) = worktree_handle { h.cleanup().await … } }
```

Timeout, cancellation and harness error are the *expected* terminations for long research children, not exceptional ones — and on all of them the async teardown is skipped even though the code is still inside an async context and could run it. The consequences of deferring to `Drop`:

- `WorktreeHandle::drop` (`sandbox/worktree.rs:86-133`) runs `std::process::Command::…status()` — a **synchronous** `git worktree remove --force` — on whatever thread drops the handle, which here is a tokio worker. On a large checkout this blocks a runtime worker for seconds.
- Every ordinary timeout emits `tracing::error!("WorktreeHandle leaked …")` and a `WorktreeCleanedUp { leaked: true }` trace event, so the telemetry that is supposed to flag genuine leaks fires on the routine path and becomes unusable as a signal.
- `InlineMcpHandle::drop` (`mcp_registrar.rs:76-110`) spawns an OS thread plus a fresh current-thread runtime per handle to close the connection.

**Failure scenario:** A worktree-isolated research child hits its 120 s wall clock. `spawn` returns `Err`, the handle drops on a tokio worker, `git worktree remove --force` blocks that worker while the operator's chat run stalls — and the trace stream reports a leak that did not happen.

**Suggested fix:** Run both teardowns unconditionally before returning (`scope.shutdown().await` / `h.cleanup().await` outside the `is_ok` guard), logging failures; keep `Drop` strictly as the panic/abort safety net.

---

### [MEDIUM] src/agents/teammates.rs:25 — `ensure_team` check-then-create races into duplicate teams; the error fallback is inert because `teams.name` has no UNIQUE constraint

**Category:** Logic
**Confidence:** High

**Description:**
`ensure_team` does a `get_team_by_name` → `create_team` sequence and handles the race by re-querying on `create_team` error. That recovery only works if a concurrent create *fails*, and the schema has no uniqueness on the name:

```sql
-- src/teams/store.rs:174-182
CREATE TABLE IF NOT EXISTS teams (
    id   TEXT PRIMARY KEY,
    name TEXT NOT NULL,        -- no UNIQUE
    …
);
```

Two concurrent `ensure_team("analysis", …)` calls therefore both see `None`, both `INSERT` successfully, and the database ends up with two rows named `analysis`. `get_team_by_name` (`store.rs:462-469`) is a bare `SELECT … WHERE name = ?1` + `query_row`, so subsequent lookups return whichever row SQLite yields first, with no ordering guarantee. The tool faces that call this (`subagent_tool/loop_tool.rs:199`, `:256` — `send_message` and `read_inbox`) can then land on different team ids for the same name.

Secondary defect in the same query: it does not filter `status`, so a **disbanded** team is returned as an existing team and `ensure_team` will never recreate it — messages are addressed to a dead team.

**Failure scenario:** A turn fans out two subagent tool calls that both `send_message` to team `analysis` (the Act phase dispatches concurrent-safe calls in parallel). Two teams named `analysis` are created; the follow-up `read_inbox` resolves to the other one and reports an empty inbox for messages that were delivered.

**Suggested fix:** Add `CREATE UNIQUE INDEX IF NOT EXISTS teams_name_unique ON teams(name)` (as a migration that de-duplicates existing rows first) so the existing error-fallback becomes real, or make the create an `INSERT … ON CONFLICT(name) DO NOTHING` + re-read inside one transaction. Separately, decide explicitly whether `get_team_by_name` should skip `status = 'disbanded'` and encode it in the query.

---

### [LOW] src/agents/subagent_spawner/mod.rs:185 — `spawn()` is ~580 lines in a 1015-line file

**Category:** Quality
**Confidence:** High

**Description:**
`pub async fn spawn` spans lines 185-764. The `HarnessDeps` literal alone (534-645) is 110 lines of mostly-commented field wiring, and the function additionally owns semaphore admission, worktree provisioning, FsScope/sandbox derivation, MCP scope provisioning, session seeding, prompt building, provider decoration, tool-service layering, budget construction, run + timeout + panic isolation, event emission and two teardown paths. This exceeds the ≤500-line file guideline in `CODE_ORGANIZATION.md` / P2 and makes the exit-path audit in the two Medium findings above harder than it should be.

**Suggested fix:** Extract three cohesive helpers already latent in the body — `provision_isolation(&req, &base) -> (Option<WorktreeHandle>, Option<FsScope>, Option<Arc<dyn Sandbox>>)`, `build_child_deps(...) -> HarnessDeps`, and `teardown(worktree, mcp_scope)` — leaving `spawn()` as the ~120-line orchestration it is meant to be. `build_context_triple` is already the model for this.

## Cross-cutting observations

- **Allowlist enforcement is sound.** I specifically probed the two bypasses in the brief and could not reproduce either. `AllowlistToolService` overrides all six `ToolService` methods (no default-impl leak), and the tool-name repairer (`tools/name_repair.rs`) is driven from `self.deps.tools.dispatchable_list()` in `harness/agent/act.rs:392-393` — i.e. the *already filtered* set — so case/separator/fuzzy repair can never synthesize a denied name. `tools/server/repair.rs`'s looser case-folding repairer has **no** callers outside its own module, so it is not on the subagent path. `is_tool_allowed`'s ordering (recursion guard → deny → sets → allow) is correct, deny genuinely beats allow, and `call_concurrency_claim` fails closed to `global()` for denied names.
- **Everything in `SpawnerBase` is plumbed.** I checked all 21 fields against `AgentRuntime`'s builders and `execute_via_harness` (`runtime.rs:486-565`): every one has a setter or a `new()` seed and every one reaches the `SpawnerBase` literal. No dangling `with_*`.
- **`thinking.rs` and `tool_sets.rs` are clean.** `normalize_think_level` is total, trims, folds case, rejects unknown input rather than defaulting, and both production callers (`gateway/handlers/agent.rs:832`, `workflow/compile.rs:153`) honour the reject contract. `ThinkLevel::id()` round-trips through the normalizer and matches the `rename_all = "lowercase"` serde form. Tool sets have no set math to get wrong (they are three flat literal slices consulted as an OR-list before the flat allowlist) and are pinned to canonical builtin names by a regression test.
- **`progress.rs` verified.** Pure domain types; the 200-char preview that feeds it truncates with `chars().take(200)` (`forwarding_trace_sink.rs:116`), so no UTF-8 slice panic.
- **`sub_agents/traits.rs` round-trips** — all non-`Option` fields are always populated by the constructors and serde defaults missing `Option` fields to `None`, so `SubAgentRequest`/`SubAgentResult` survive a JSON round trip. There is **no schema version field**; if these ever cross a process or version boundary (they are the A2A delegation shapes, `a2a/sub_agent.rs`) an additive-only evolution policy should be written down or a `version` field added. Not raised as a finding since today's use is intra-version.
- **Per-agent MCP scope is additive, not restrictive.** Declaring `mcp_servers: [Reference{x}]` does not confine the child to `x` — `McpScopedToolService` merges extras on top of the *full* parent tool list and `AllowlistToolService` is the only narrowing layer. This is documented behaviour (`mcp_scope_view.rs:1-3`), but the name "scope" invites the opposite reading; worth a doc note on `AgentDef.mcp_servers`.
- **`SubagentReturned.summary` can be empty by construction.** `extract_run_result` deliberately clears `final_text` when the last assistant message is pure tool_use (`mod.rs:814-824`), and the emit at `mod.rs:688` does `unwrap_or_default()`. Recovery then renders `Completed { summary: "" }` alongside the note "the text above … is the sub-agent's actual output — do NOT re-run the task" (`recovery.rs:212-217`). Moot today because of the High finding above, but it becomes live the moment that emission is fixed — worth handling in the same change.
- **Cancellation coverage is good where it matters.** The semaphore wait is `biased`-selected against the cancel token with a byte-exact error string that `background_tracker::lifecycle_from_outcome` matches by equality; the harness run observes the same token. The only unguarded window is the `subagent_semaphore: None` path (tests / direct callers), which does no cancel check at all before provisioning.

## Files reviewed

| File | LOC | Notes |
|------|-----|-------|
| `src/agents/subagent_spawner/mod.rs` | 1015 | 5 findings (3 High, 1 Medium ×2, 1 Low) |
| `src/agents/sub_agents/mod.rs` | 8 | clean (re-export only) |
| `src/agents/sub_agents/traits.rs` | 128 | clean; no schema versioning (noted) |
| `src/agents/allowlist_tool_service.rs` | 330 | clean; bypass probes negative |
| `src/agents/teammates.rs` | 80 | 1 Medium |
| `src/agents/thinking.rs` | 233 | clean |
| `src/agents/tool_sets.rs` | 132 | clean |
| `src/agents/progress.rs` | 39 | clean |

Supporting files read for verification (not in scope, not reviewed for their own defects):
`src/agents/runtime.rs`, `src/agents/types.rs`, `src/agents/registry.rs`, `src/agents/loader.rs`,
`src/agents/subagent_tool/{mod,spawn,loop_tool,recovery,types}.rs`, `src/agents/background_persistence.rs`,
`src/agents/forwarding_trace_sink.rs`, `src/tools/{mcp_scope_view,name_repair,fs_scope,service}.rs`,
`src/tools/server/repair.rs`, `src/harness/agent/{act,think}.rs`, `src/sandbox/worktree.rs`,
`src/extension/registrar/mcp_registrar.rs`, `src/routing/session_key.rs`, `src/teams/store.rs`,
`src/gateway/execution_engine/run_loop/{mod,inner}.rs`.
