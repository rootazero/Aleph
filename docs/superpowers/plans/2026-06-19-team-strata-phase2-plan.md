# Team StraTA Strategic Coordination — Phase 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the StraTA "self-audit" loop on the team group-chat path: the leader assigns coord_tasks (naming the task id), members submit deliverables that flip the owning coord_task to `WaitingReview`, the broadcaster re-triggers the leader, and the leader accepts/rejects each via a new `task_review` tool — approve→Completed (unblocks dependents), reject→InProgress (owner redoes).

**Architecture:** Reuse the existing `coord_task` status machine and the broadcaster's existing `dispatch` primitive — no new state machine, no harness change. A new `task_review` tool (modeled on the existing `WorkflowStepReviewTool`) drives `CoordTaskStore`; `TaskSubmitTool` gains an optional `CoordTaskStore` to flip status on submit; `GroupChatBroadcaster` gains a 5th read-only `CoordTaskStore` field so `run_member` can diff the member's WaitingReview tasks pre/post turn and synthetically re-dispatch to the leader. The member learns its task id purely from the leader's @-assignment text (prompt-level, R9) — no prompt-injection plumbing.

**Tech Stack:** Rust, tokio, rusqlite (SQLite), serde, schemars, async_trait. No new dependencies.

## Global Constraints

- **Locked design decisions (from user, this session):**
  1. **Task-ID flow = prompt-level.** The leader includes the `task_create` task id in its `@`-assignment message; the member reads it from the transcript (already in its prompt) and passes it to `task_submit`. No code surfaces the id; the leader/member prompt contracts mandate it (Task 6). No Phase-3 / live-kanban dependency.
  2. **Reject = `InProgress`.** `task_review` reject sets the coord_task to `InProgress` (re-queue for redo), feedback in `result` (+ a task comment). Deliberate divergence from the sibling `WorkflowStepReviewTool` (which uses `Failed`); safe because the group-chat path has no dispatcher claiming `InProgress`.
  3. **Leader authz = prompt-gate + soft ownership guard.** `task_review` is named only in the leader prompt; inside `call()` it resolves the task→team→`leader_id` and, if the caller is not the leader, **no-ops with a warning** (returns a message, mutates nothing). If the team/leader can't be resolved, it allows (prompt-gating is primary).
- **Synthetic retrigger routing (decided):** the F2-retrigger dispatch embeds `@<leader_id>` in the content with `user_triggered=false`, `leader_first=false`, `sender = member agent_id`, `chain_depth+1`, sharing the live `budget`; it is **skipped when the member is the leader** (self-review — a self-`@` is stripped by `resolve_targets`); one dispatch per newly-submitted task.
- **Redlines:** R7/R9 (no "is it done" classifier — accept/reject is the leader's LLM verdict; the soft authz guard inspects only ids, not content). R10 (`src/harness/` untouched — the submit-flip, retrigger re-dispatch, and tool calls are deterministic plumbing/state→routing, not cognition). R4 (the retrigger lives in the broadcaster, not the RPC handler). R8 (`task_review` is a tool; the leader manages acceptance through natural-language tool use).
- **Cargo frugality (project rule — overrides standard per-step TDD):** Do **NOT** run `cargo` per step. Author each test test-first (it documents intent + runs in CI), write the implementation, commit. Run exactly **one** `cargo test -p alephcore --lib --no-run` (compiles all `#[cfg(test)]` code → catches every call-site arity) **plus** one `cargo check --bin aleph-server` at the very end (Task 7). The cargo binary's `~/.cargo/bin` symlink is broken — use the rustup toolchain path (see Task 7).
- **Targeted commits:** commit per task with **explicit `git add <file>`** — never `git add -A`/`git add .` (the working tree carries unrelated dirty webchat files; also an auto-commit bot interleaves commits on `main`).
- **Branch:** single-branch dev on `main`. Do NOT push. The plan file lives under `docs/superpowers/` (git-ignored) — code commits are separate.
- **Style:** rustfmt (4-space, 100 col), `snake_case`/`PascalCase`, no `unwrap()` outside tests, preserve every Chinese prompt string byte-for-byte. Fail-soft (P7): any store miss degrades to today's behavior, never panics.

### Verified current facts (from the gather pass — trust these over any stale spec anchor)

- New tool template = **`WorkflowStepReviewTool`** (`src/builtin_tools/team/workflow_step.rs`), which already holds `Arc<dyn CoordTaskStore> + current_agent_id`, calls `record_run_review(...)` + `update_task(...)`. `TaskSubmitTool` (`task_submit.rs`) holds only `Arc<dyn ArtifactStore>` and writes an artifact — it does **not** touch coord_tasks today.
- `CoordTaskStore` (`src/agents/swarm/tasks/mod.rs`): `get_task(&str)->Result<Option<CoordTask>>`, `update_task(&str, CoordTaskUpdate)->Result<CoordTask>`, `list_tasks(CoordTaskFilter)->Result<Vec<CoordTask>>`, `get_newly_unblocked(&str)->Result<Vec<CoordTask>>`, `record_run_review(&str, ReviewVerdict, ReviewerKind, Option<&str>)->Result<()>` (default no-op), `add_task_comment(&str,&str,&str)`.
- `CoordTaskUpdate { status: Option<CoordTaskStatus>, owner: Option<AgentId>, result: Option<String>, metadata: Option<Value> }` derives `Default`. `CoordTaskFilter { team_id, status, owner }` derives `Default`. `CoordTaskStatus` has `WaitingReview`, `Completed`, `InProgress`; derives `Debug, Clone, Copy, PartialEq, Eq`; `as_str()->&'static str`. `ReviewVerdict { Approved, Rejected }`, `ReviewerKind { User, LeadAgent, Auto }`. `CoordTask { id: CoordTaskId, team_id: Option<String>, subject: String, owner: Option<AgentId>, status, ... }`.
- Type aliases: `CoordTaskId = String`, `AgentId = String` (so filter `owner`/the diff `.map(|t| t.id)` are plain `String`).
- `BuiltinToolConfig` (`config.rs`, `#[derive(Clone, Default)]`): `coord_task_store: Option<Arc<dyn CoordTaskStore>>` (:70), `artifact_store: Option<Arc<dyn ArtifactStore>>` (:84), `team_store: Option<Arc<dyn crate::teams::TeamStore>>` (:82). No new config field needed → no E0063 risk.
- `TaskSubmitTool::new` has exactly **one** call site: `collab_session_tools.rs:226`.
- `collab_session_tools.rs` already imports `crate::tool_metadata::{ToolSource, UnifiedTool}` (:16) and `info!`.
- `TeamStore = crate::teams::TeamStore`. `AlephTool` trait: `const NAME`, `const DESCRIPTION`, `type Args`, `type Output`, `async fn call`; auto `definition()`/`call_json()`.
- `GroupChatBroadcaster` (`src/teams/broadcast/mod.rs`) is `#[derive(Clone)]`, post-Phase-1: struct :80, `new()` :93 (4-arg, last = `planner_provider`), `dispatch` :173 (7-arg: `team_id, content, sender, chain_depth, user_triggered, leader_first, budget`), `run_member` :277 (params incl `leader_id`, `team_name`, `protocol`, `user_request`), `is_leader = agent_id == leader_id`, `execute()` call, the reply-recursion `self.dispatch(team_id, reply, agent_id, chain_depth + 1, false, false, budget)`, `post_system` :390. The file is 471 lines. `broadcast/mod.rs` does **not** import `CoordTask*` yet.
- `handle_chat_send` (`canvas.rs:266`) is 7-param; the `GroupChatBroadcaster::new(...)` call is at :369; the boot registration is `agent_init/mod.rs:1405` with `coord_store` (`Option<Arc<dyn CoordTaskStore>>`) in scope from :202.

---

## Task 1: `task_review` tool (Seam F1 — core)

**Files:**
- Create: `src/builtin_tools/team/task_review.rs`
- Modify: `src/builtin_tools/team/mod.rs` (add `pub mod task_review;` + `pub use`)

**Interfaces:**
- Produces: `crate::builtin_tools::team::{TaskReviewTool, TaskReviewArgs, TaskReviewOutput, ReviewDecision}`. `TaskReviewTool::new(coord_store: Arc<dyn CoordTaskStore>, team_store: Arc<dyn TeamStore>, current_agent_id: String) -> Self`. `const NAME = "task_review"`.

- [ ] **Step 1: Create the tool file** — write `src/builtin_tools/team/task_review.rs` verbatim:

```rust
//! `TaskReviewTool` — the team leader's explicit acceptance/verification tool
//! (strategy round 2, group-chat path). After reading a member's submitted
//! deliverable (`task_read_artifact`), the leader accepts or rejects the owning
//! coord_task: approve → Completed (downstream dependents unblock); reject →
//! InProgress (the owner redoes it; the leader's feedback rides along). Mirrors
//! `WorkflowStepReviewTool` but uses a flat verdict arg and carries a soft
//! leader-only guard (a non-leader caller is a no-op — prompt-gating is the
//! primary gate; this is defense-in-depth, R7/R9).

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::agents::swarm::tasks::{
    CoordTaskStatus, CoordTaskStore, CoordTaskUpdate, ReviewVerdict, ReviewerKind,
};
use crate::error::Result;
use crate::sync_primitives::Arc;
use crate::teams::TeamStore;
use crate::tools::AlephTool;

/// Accept / reject discriminator (snake_case on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Approve,
    Reject,
}

/// Arguments for `task_review`.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TaskReviewArgs {
    /// The coord_task to accept or reject — the id you handed the member when
    /// assigning, which the member echoes back on submit.
    pub task_id: String,
    /// `approve` → the task is completed and downstream dependents unblock;
    /// `reject` → the task returns to in_progress for the owner to redo.
    pub decision: ReviewDecision,
    /// Optional feedback, stored on the task (shown to the owner on a reject).
    #[serde(default)]
    pub feedback: Option<String>,
}

/// Output from `task_review`.
#[derive(Debug, Clone, Serialize)]
pub struct TaskReviewOutput {
    pub task_id: String,
    pub status: String,
    /// On approve: the subjects of tasks whose dependencies are now all
    /// satisfied, so the leader knows what to assign next. Empty on reject /
    /// when nothing newly unblocks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub newly_unblocked: Vec<String>,
    /// Human-facing note (e.g. why a non-leader call was ignored).
    pub message: String,
}

/// Map a verdict to the coord_task status it sets. `Approve` → Completed
/// (satisfies downstream deps); `Reject` → InProgress (re-queue for the owner —
/// strategy round 2 chose redo-in-place over the sibling `workflow_step_review`'s
/// terminal Failed). Pure / host-testable.
#[must_use]
fn target_status(decision: ReviewDecision) -> CoordTaskStatus {
    match decision {
        ReviewDecision::Approve => CoordTaskStatus::Completed,
        ReviewDecision::Reject => CoordTaskStatus::InProgress,
    }
}

/// Soft leader-only guard. `leader` is the resolved team leader (None when the
/// task carries no team / the team can't be read — then we can't verify, so we
/// allow and lean on prompt-gating). Pure / host-testable.
#[must_use]
fn is_authorized(caller: &str, leader: Option<&str>) -> bool {
    leader.map_or(true, |l| l == caller)
}

#[derive(Clone)]
pub struct TaskReviewTool {
    coord_store: Arc<dyn CoordTaskStore>,
    team_store: Arc<dyn TeamStore>,
    current_agent_id: String,
}

impl TaskReviewTool {
    pub fn new(
        coord_store: Arc<dyn CoordTaskStore>,
        team_store: Arc<dyn TeamStore>,
        current_agent_id: String,
    ) -> Self {
        Self {
            coord_store,
            team_store,
            current_agent_id,
        }
    }
}

#[async_trait]
impl AlephTool for TaskReviewTool {
    const NAME: &'static str = "task_review";
    const DESCRIPTION: &'static str =
        "Accept or reject a team member's submitted deliverable (leader only). \
         Call this after reading the member's artifact (task_read_artifact) for \
         a task the member submitted: decision='approve' marks the task \
         completed and unblocks dependents; decision='reject' sends it back to \
         in_progress for the owner to redo — put what to fix in `feedback`. The \
         member's result is a self-report, not a verified fact: before approving \
         claims with external side-effects (files written, requests sent, things \
         published), verify the handle it returned (path, URL, id) yourself.";

    type Args = TaskReviewArgs;
    type Output = TaskReviewOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            "task_review(task_id='task-3', decision='approve')".to_string(),
            "task_review(task_id='task-3', decision='reject', feedback='缺少错误处理,补上再交')"
                .to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Resolve the task + its team leader for the soft authz check. A missing
        // task is a graceful no-op (the id may be freeform, not a coord_task).
        let Some(task) = self.coord_store.get_task(&args.task_id).await.ok().flatten() else {
            return Ok(TaskReviewOutput {
                task_id: args.task_id,
                status: "not_found".into(),
                newly_unblocked: Vec::new(),
                message: "no coord_task with that id — nothing to review".into(),
            });
        };
        let leader = match task.team_id.as_deref() {
            Some(team_id) => self
                .team_store
                .get_team(team_id)
                .await
                .ok()
                .flatten()
                .map(|t| t.leader_id),
            None => None,
        };
        if !is_authorized(&self.current_agent_id, leader.as_deref()) {
            warn!(
                task_id = %args.task_id,
                caller = %self.current_agent_id,
                "task_review ignored: caller is not the team leader"
            );
            return Ok(TaskReviewOutput {
                task_id: args.task_id,
                status: "forbidden".into(),
                newly_unblocked: Vec::new(),
                message: "only the team leader can review tasks — ignored".into(),
            });
        }

        let status = target_status(args.decision);
        debug!(task_id = %args.task_id, ?status, "task_review verdict");

        let verdict = match args.decision {
            ReviewDecision::Approve => ReviewVerdict::Approved,
            ReviewDecision::Reject => ReviewVerdict::Rejected,
        };
        let _ = self
            .coord_store
            .record_run_review(
                &args.task_id,
                verdict,
                ReviewerKind::LeadAgent,
                Some(&self.current_agent_id),
            )
            .await;
        self.coord_store
            .update_task(
                &args.task_id,
                CoordTaskUpdate {
                    status: Some(status),
                    result: args.feedback.clone(),
                    ..Default::default()
                },
            )
            .await?;
        if let Some(fb) = args.feedback.as_deref().filter(|f| !f.trim().is_empty()) {
            let _ = self
                .coord_store
                .add_task_comment(&args.task_id, &self.current_agent_id, fb)
                .await;
        }

        let newly_unblocked = match args.decision {
            ReviewDecision::Approve => self
                .coord_store
                .get_newly_unblocked(&args.task_id)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|t| t.subject)
                .collect(),
            ReviewDecision::Reject => Vec::new(),
        };

        let message = match args.decision {
            ReviewDecision::Approve => "approved → completed".to_string(),
            ReviewDecision::Reject => "rejected → back to in_progress".to_string(),
        };
        Ok(TaskReviewOutput {
            task_id: args.task_id,
            status: status.as_str().to_string(),
            newly_unblocked,
            message,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_status_maps_verdict() {
        assert_eq!(target_status(ReviewDecision::Approve), CoordTaskStatus::Completed);
        assert_eq!(target_status(ReviewDecision::Reject), CoordTaskStatus::InProgress);
    }

    #[test]
    fn authz_allows_leader_blocks_other_allows_unknown() {
        assert!(is_authorized("alice", Some("alice")), "leader may review");
        assert!(!is_authorized("bob", Some("alice")), "non-leader is blocked");
        assert!(is_authorized("bob", None), "unverifiable team → allow (prompt-gating)");
    }

    #[test]
    fn decision_parses_snake_case() {
        let a: ReviewDecision = serde_json::from_str("\"approve\"").unwrap();
        let r: ReviewDecision = serde_json::from_str("\"reject\"").unwrap();
        assert_eq!(a, ReviewDecision::Approve);
        assert_eq!(r, ReviewDecision::Reject);
    }
}
```

- [ ] **Step 2: Export the module** — in `src/builtin_tools/team/mod.rs`, add the module declaration directly before `pub mod task_submit;` (the declarations are grouped, not strictly alphabetical):

```rust
pub mod task_review;
pub mod task_submit;
```

and add the re-export directly before the `pub use task_submit::{...}` line:

```rust
pub use task_review::{ReviewDecision, TaskReviewArgs, TaskReviewOutput, TaskReviewTool};
pub use task_submit::{TaskSubmitArgs, TaskSubmitOutput, TaskSubmitTool};
```

- [ ] **Step 3: Commit**

```bash
git add src/builtin_tools/team/task_review.rs src/builtin_tools/team/mod.rs
git commit -m "teams: add task_review tool (leader accept/reject, approve->completed reject->in_progress, soft leader guard)"
```

---

## Task 2: Register `task_review` (Seam F1 — the 6 registry sites)

Without **all** of these the leader's LLM either never sees the tool or it is never dispatched (silent runtime failure). The tool is gated on `CoordTaskStore` **and** `TeamStore` (both already in `BuiltinToolConfig`).

**Files:**
- Modify: `src/executor/builtin_registry/definitions.rs` (schema entry + result-budget arm)
- Modify: `src/executor/builtin_registry/groups.rs` (team group membership)
- Modify: `src/executor/builtin_registry/registry/struct_def.rs` (registry field)
- Modify: `src/executor/builtin_registry/registry/tool_registry_impl.rs` (dispatch arm)
- Modify: `src/executor/builtin_registry/builder/constructor/collab_session_tools.rs` (return type + construction + return tuple)
- Modify: `src/executor/builtin_registry/builder/constructor/mod.rs` (destructure + `Ok(Self{..})` field init)

**Interfaces:**
- Consumes: `crate::builtin_tools::team::TaskReviewTool` (Task 1), `config.coord_task_store`, `config.team_store`.
- Produces: a `task_review_tool: Option<TaskReviewTool>` field on `BuiltinToolRegistry`, threaded so `call_json("task_review", ...)` dispatches.

- [ ] **Step 1: Schema entry** — in `definitions.rs`, after the `task_read_artifact` `BuiltinToolDefinition` (the block that currently ends `requires_config: true,` then `},`), insert:

```rust
    BuiltinToolDefinition {
        name: "task_review",
        description: "Leader accepts/rejects a member's submitted task (approve→completed, reject→in_progress)",
        requires_config: true,
    },
```

- [ ] **Step 2: Result-budget arm** — in `definitions.rs`, immediately after the existing arm `"task_submit" | "task_read_artifact" => None,`, add:

```rust
        "task_review" => None,
```

- [ ] **Step 3: Group membership** — in `groups.rs`, in the `"team"` `ToolCategory` `tools` array, add `"task_review",` immediately after the `"task_submit",` line:

```rust
            "task_submit",
            "task_review",
            "task_read_artifact",
```

- [ ] **Step 4: Registry field** — in `registry/struct_def.rs`, immediately after the `task_read_artifact_tool` field, add:

```rust
    /// Leader task acceptance/verification (strategy round 2 — group chat).
    /// Optional because it requires both a `CoordTaskStore` and a `TeamStore`.
    pub(crate) task_review_tool: Option<crate::builtin_tools::team::TaskReviewTool>,
```

- [ ] **Step 5: Dispatch arm** — in `registry/tool_registry_impl.rs`, immediately after the `"task_read_artifact" => Box::pin(...)` arm (the one ending `tool.call_json(arguments).await }),`), add:

```rust
            "task_review" => Box::pin(async move {
                let tool = self.task_review_tool.as_ref().ok_or_else(|| {
                    AlephError::tool(
                        "task_review not available: requires CoordTaskStore + TeamStore",
                    )
                })?;
                tool.call_json(arguments).await
            }),
```

- [ ] **Step 6: Construction — return type** — in `collab_session_tools.rs`, in the `build_collab_session_tools` return-type tuple, add a new line immediately after `Option<crate::builtin_tools::team::TaskReadArtifactTool>,`:

```rust
        Option<crate::builtin_tools::team::TaskReadArtifactTool>,
        Option<crate::builtin_tools::team::TaskReviewTool>,
```

- [ ] **Step 7: Construction — block** — in `collab_session_tools.rs`, immediately after the `let (task_submit_tool, task_read_artifact_tool) = if let Some(ref artifact_store) = config.artifact_store { ... } else { (None, None) };` block, add:

```rust
        // Leader task-review tool (strategy round 2) — needs a CoordTaskStore
        // (to flip task status) AND a TeamStore (soft leader authz).
        let task_review_tool = if let (Some(coord_store), Some(team_store)) =
            (&config.coord_task_store, &config.team_store)
        {
            use crate::builtin_tools::team::TaskReviewTool;
            let tool = TaskReviewTool::new(
                Arc::clone(coord_store),
                Arc::clone(team_store),
                current_agent_id.to_string(),
            );
            {
                use crate::tools::AlephTool;
                let td = tool.definition();
                let mut ut = UnifiedTool::new(
                    format!("builtin:{}", td.name),
                    &td.name,
                    &td.description,
                    ToolSource::Builtin,
                );
                ut = ut.with_parameters_schema(td.parameters.clone());
                tools.insert(td.name.clone(), ut);
            }
            info!("Registered task_review tool (leader acceptance)");
            Some(tool)
        } else {
            None
        };
```

- [ ] **Step 8: Construction — return tuple** — in `collab_session_tools.rs`, in the function's final return tuple, add `task_review_tool,` immediately after `task_read_artifact_tool,`:

```rust
            task_submit_tool,
            task_read_artifact_tool,
            task_review_tool,
```

- [ ] **Step 9: Destructure** — in `builder/constructor/mod.rs`, in the `let ( ... ) = Self::build_collab_session_tools(...)` destructure, add `task_review_tool,` immediately after `task_read_artifact_tool,`:

```rust
            task_submit_tool,
            task_read_artifact_tool,
            task_review_tool,
```

- [ ] **Step 10: Field init** — in `builder/constructor/mod.rs`, in the `Ok(Self { ... })` field-init block, add `task_review_tool,` immediately after `task_read_artifact_tool,`:

```rust
            task_submit_tool,
            task_read_artifact_tool,
            task_review_tool,
```

- [ ] **Step 11: Commit**

```bash
git add src/executor/builtin_registry/definitions.rs \
        src/executor/builtin_registry/groups.rs \
        src/executor/builtin_registry/registry/struct_def.rs \
        src/executor/builtin_registry/registry/tool_registry_impl.rs \
        src/executor/builtin_registry/builder/constructor/collab_session_tools.rs \
        src/executor/builtin_registry/builder/constructor/mod.rs
git commit -m "registry: wire task_review through all 6 sites (schema/budget/group/field/dispatch/construct)"
```

---

## Task 3: `task_submit` flips coord_task → WaitingReview (Seam F2)

**Files:**
- Modify: `src/builtin_tools/team/task_submit.rs` (struct field + `new()` + `call()` flip)
- Modify: `src/executor/builtin_registry/builder/constructor/collab_session_tools.rs:226` (the one `TaskSubmitTool::new` call site)

**Interfaces:**
- Consumes: `config.coord_task_store` (already passed elsewhere).
- Produces: `TaskSubmitTool::new(store: Arc<dyn ArtifactStore>, coord_store: Option<Arc<dyn CoordTaskStore>>, current_agent_id: String)` (3-arg).

- [ ] **Step 1: Add a test** — in `src/builtin_tools/team/task_submit.rs`, add a `#[cfg(test)]` module at end of file (the file has none today):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // The status-flip is store-driven (integration-verified end-to-end); this
    // unit guards the public constructor arity that the registry depends on.
    #[test]
    fn new_takes_optional_coord_store() {
        fn assert_3_arg(
            f: impl Fn(Arc<dyn ArtifactStore>, Option<Arc<dyn CoordTaskStore>>, String) -> TaskSubmitTool,
        ) {
            let _ = f;
        }
        assert_3_arg(TaskSubmitTool::new);
    }
}
```

- [ ] **Step 2: Add imports** — at the top of `task_submit.rs`, after the existing `use crate::teams::artifacts::{...};` line, add:

```rust
use crate::agents::swarm::tasks::{CoordTaskStatus, CoordTaskStore, CoordTaskUpdate};
```

- [ ] **Step 3: Add the field + constructor arg** — replace the struct + `impl` block:

```rust
#[derive(Clone)]
pub struct TaskSubmitTool {
    store: Arc<dyn ArtifactStore>,
    /// Strategy round 2 (F2): when present and the submitted `task_id` is a real
    /// coord_task, the submit flips it to `WaitingReview` so the leader's
    /// `task_review` picks it up. `None` (no CoordTaskStore wired) keeps the
    /// legacy artifact-only behavior.
    coord_store: Option<Arc<dyn CoordTaskStore>>,
    current_agent_id: String,
}

impl TaskSubmitTool {
    pub fn new(
        store: Arc<dyn ArtifactStore>,
        coord_store: Option<Arc<dyn CoordTaskStore>>,
        current_agent_id: String,
    ) -> Self {
        Self {
            store,
            coord_store,
            current_agent_id,
        }
    }
}
```

- [ ] **Step 4: Flip on submit** — in `call()`, immediately after the `let artifact = self.store.create_artifact(...).await.map_err(...)?;` statement and before the final `Ok(TaskSubmitOutput { ... })`, insert:

```rust
        // F2: if this submit is against a real coord_task, flip it to
        // WaitingReview for the leader's task_review. Graceful no-op when the
        // id is freeform (not a coord_task) or no CoordTaskStore is wired (P7).
        if let Some(coord_store) = &self.coord_store {
            if coord_store.get_task(&args.task_id).await.ok().flatten().is_some() {
                let _ = coord_store
                    .update_task(
                        &args.task_id,
                        CoordTaskUpdate {
                            status: Some(CoordTaskStatus::WaitingReview),
                            ..Default::default()
                        },
                    )
                    .await;
            }
        }
```

- [ ] **Step 5: Update the construction site** — in `collab_session_tools.rs`, change the `TaskSubmitTool::new(...)` call (inside the `if let Some(ref artifact_store) = config.artifact_store` block) to pass the coord store:

```rust
                let submit = TaskSubmitTool::new(
                    Arc::clone(artifact_store),
                    config.coord_task_store.clone(),
                    current_agent_id,
                );
```

- [ ] **Step 6: Commit**

```bash
git add src/builtin_tools/team/task_submit.rs \
        src/executor/builtin_registry/builder/constructor/collab_session_tools.rs
git commit -m "teams: task_submit flips owning coord_task to waiting_review (fail-soft if not a coord_task)"
```

---

## Task 4: Broadcaster gains a read-only `CoordTaskStore` (Seam F2-retrigger — plumbing)

This threads the store through the broadcaster + handler + boot but adds **no retrigger logic yet** (Task 5 uses it). Keeping it a separate compile unit isolates the plumbing from the diff logic.

**Files:**
- Modify: `src/teams/broadcast/mod.rs` (import + 5th struct field + 5th `new()` arg)
- Modify: `src/gateway/handlers/teams/canvas.rs` (new `handle_chat_send` param + 5th arg to `GroupChatBroadcaster::new`)
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs` (capture `coord_store` + pass it)

**Interfaces:**
- Produces: `GroupChatBroadcaster::new(ctx, team_store, msg_store, planner_provider, coord_task_store: Option<Arc<dyn CoordTaskStore>>)` (5-arg); `handle_chat_send(..., event_bus, coord_task_store: Option<Arc<dyn CoordTaskStore>>)`.

- [ ] **Step 1: Import** — in `src/teams/broadcast/mod.rs`, add near the existing `use crate::teams::TeamStore;` (line 38):

```rust
use crate::agents::swarm::tasks::{CoordTaskFilter, CoordTaskStatus, CoordTaskStore};
```

- [ ] **Step 2: Struct field** — in the `GroupChatBroadcaster` struct, add a 5th field after `planner_provider`:

```rust
    planner_provider: Option<Arc<dyn crate::providers::AiProvider>>,
    /// Read-only coordination-task store (strategy round 2). `Some` lets
    /// `run_member` diff this member's WaitingReview tasks across a turn and
    /// re-trigger the leader to review fresh submissions. `None` keeps group
    /// chat exactly as before (no re-trigger).
    coord_task_store: Option<Arc<dyn CoordTaskStore>>,
}
```

- [ ] **Step 3: Constructor arg** — extend `new()`:

```rust
    pub fn new(
        ctx: Arc<GatewayContext>,
        team_store: Arc<dyn TeamStore>,
        msg_store: Arc<dyn MessageStore>,
        planner_provider: Option<Arc<dyn crate::providers::AiProvider>>,
        coord_task_store: Option<Arc<dyn CoordTaskStore>>,
    ) -> Self {
        Self {
            ctx,
            team_store,
            msg_store,
            planner_provider,
            coord_task_store,
        }
    }
```

- [ ] **Step 4: `handle_chat_send` param** — in `canvas.rs`, add a parameter at the END of `handle_chat_send`'s signature (after `event_bus`):

```rust
    event_bus: Option<Arc<crate::gateway::event_bus::GatewayEventBus>>,
    coord_task_store: Option<Arc<dyn crate::agents::swarm::tasks::CoordTaskStore>>,
) -> JsonRpcResponse {
```

- [ ] **Step 5: Pass to `new()`** — in `canvas.rs`, extend the `GroupChatBroadcaster::new(...)` call with the 5th arg:

```rust
    let broadcaster = crate::teams::broadcast::GroupChatBroadcaster::new(
        Arc::clone(&context),
        Arc::clone(&store),
        Arc::clone(&msg_store),
        team_planner_provider,
        coord_task_store,
    );
```

- [ ] **Step 6: Thread at boot** — in `agent_init/mod.rs`, inside the `if let Some(ts) = team_store.clone() { ... }` block, add a capture beside `chat_event_bus` (just before the `server.handlers_mut().register("teams.chat.send", ...)` call):

```rust
                let chat_event_bus = event_bus.clone();
                let chat_coord_store = coord_store.clone();
```

then in the closure, add a clone beside `let bus = chat_event_bus.clone();` and pass it as the new last arg to `handle_chat_send`:

```rust
                        let bus = chat_event_bus.clone();
                        let coord = chat_coord_store.clone();
                        async move {
                            alephcore::gateway::handlers::teams::handle_chat_send(
                                req,
                                store,
                                msg_store,
                                ctx,
                                provider,
                                planner,
                                Some(bus),
                                coord,
                            )
                            .await
                        }
```

- [ ] **Step 7: Commit**

```bash
git add src/teams/broadcast/mod.rs \
        src/gateway/handlers/teams/canvas.rs \
        src/bin/aleph-server/commands/start/builder/agent_init/mod.rs
git commit -m "teams: thread read-only CoordTaskStore into GroupChatBroadcaster (dormant until retrigger)"
```

---

## Task 5: F2-retrigger — `run_member` diffs WaitingReview + re-dispatches the leader

**Files:**
- Modify: `src/teams/broadcast/mod.rs` (pure diff helper + test; `run_member` pre/post snapshot + synthetic dispatch)

**Interfaces:**
- Consumes: the `coord_task_store` field (Task 4), `CoordTaskFilter`/`CoordTaskStatus` (imported in Task 4).
- Produces: `fn newly_waiting_review(pre: &[String], post: &[String]) -> Vec<String>`.

- [ ] **Step 1: Add the failing test** — in `src/teams/broadcast/mod.rs`'s `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn newly_waiting_review_is_post_minus_pre() {
        let pre = vec!["a".to_string(), "b".to_string()];
        let post = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(super::newly_waiting_review(&pre, &post), vec!["c".to_string()]);
        // nothing new
        assert!(super::newly_waiting_review(&post, &post).is_empty());
        // a task leaving WaitingReview is not "new"
        assert!(super::newly_waiting_review(&post, &pre).is_empty());
    }
```

- [ ] **Step 2: Add the pure helper** — near the top-level helpers in `src/teams/broadcast/mod.rs` (e.g. just above `impl GroupChatBroadcaster`):

```rust
/// Set-difference for the WaitingReview snapshot diff: ids present in `post`
/// but not in `pre` — i.e. tasks this member just moved into WaitingReview this
/// turn. Pure / host-testable.
#[must_use]
fn newly_waiting_review(pre: &[String], post: &[String]) -> Vec<String> {
    post.iter().filter(|id| !pre.contains(id)).cloned().collect()
}
```

- [ ] **Step 3: Pre-turn snapshot** — in `run_member`, immediately BEFORE the `let req = RunRequest { ... };` construction (after `is_leader` is computed and the `input`/`emitter` are built), insert:

```rust
        // F2-retrigger: snapshot this member's WaitingReview tasks before the
        // turn so we can detect fresh submissions afterward. Skipped for the
        // leader (a self-review needs no @leader nudge) and when no coord store
        // is wired.
        let review_pre: Vec<String> = match (&self.coord_task_store, is_leader) {
            (Some(cs), false) => cs
                .list_tasks(CoordTaskFilter {
                    team_id: Some(team_id.clone()),
                    status: Some(CoordTaskStatus::WaitingReview),
                    owner: Some(agent_id.clone()),
                })
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|t| t.id)
                .collect(),
            _ => Vec::new(),
        };
```

- [ ] **Step 4: Post-turn diff + synthetic dispatch** — in `run_member`, immediately AFTER the `execute(...)` call's error guard (the `if let Err(e) = self.ctx.execution_adapter().execute(...).await { ...; return; }` block) and BEFORE the `let Some(reply) = extract_final_response(...)` line, insert:

```rust
        // F2-retrigger: any task this member just moved into WaitingReview ⇒
        // synthetically @-nudge the leader to review it. Goes through `dispatch`
        // directly (an inert team_messages row would never re-trigger anyone),
        // carrying the live budget + depth. Skipped for the leader (self-review).
        if let (Some(cs), false) = (&self.coord_task_store, is_leader) {
            let review_post: Vec<String> = cs
                .list_tasks(CoordTaskFilter {
                    team_id: Some(team_id.clone()),
                    status: Some(CoordTaskStatus::WaitingReview),
                    owner: Some(agent_id.clone()),
                })
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|t| t.id)
                .collect();
            for new_id in newly_waiting_review(&review_pre, &review_post) {
                self.clone()
                    .dispatch(
                        team_id.clone(),
                        format!("@{leader_id} task `{new_id}` 已提交,待你 review(用 task_review 验收)。"),
                        agent_id.clone(),
                        chain_depth + 1,
                        false,
                        false,
                        budget.clone(),
                    )
                    .await;
            }
        }
```

(Note: this fires regardless of whether the member also produced a chat reply — a member may `task_submit` and say nothing. The existing reply-recursion `self.dispatch(team_id, reply, agent_id, chain_depth + 1, false, false, budget)` at the end of `run_member` is unchanged and still runs when there is a reply; both consume cloned `budget`/`team_id`, and the retrigger uses `self.clone()` so the final reply dispatch's `self` move is unaffected.)

- [ ] **Step 5: Commit**

```bash
git add src/teams/broadcast/mod.rs
git commit -m "teams: re-trigger leader on member task submission (run_member WaitingReview diff -> @leader dispatch)"
```

---

## Task 6: Prompt contracts — assign-with-id + review loop (Seam D extension)

The prompt-level task-id flow (locked decision 1): the leader is told to name the `task_id` when @-assigning and to accept/reject via `task_review`; the member is told to submit against that id.

**Files:**
- Modify: `src/teams/leader_prompt.rs` (the `build()` orchestration contract)
- Modify: `src/teams/broadcast/member_prompt.rs` (the member obey-contract branch + tests)

**Interfaces:**
- Consumes/Produces: same `build()` / `build_member_input()` signatures — only string content changes.

- [ ] **Step 1: Update + add tests** — in `src/teams/broadcast/member_prompt.rs`, update the two existing tests so the leader frame asserts `task_review` and the member frame asserts `task_submit`. Find the test asserting the leader contract and add:

```rust
        assert!(out.contains("task_review"), "leader told to accept/reject via task_review");
        assert!(out.contains("task_id"), "leader told to name the task_id when assigning");
```

and in the member-frame test add:

```rust
        assert!(out.contains("task_submit"), "member told to submit via task_submit");
```

(Keep all existing assertions — `task_create`, `不要自己闷头做完`, `团队纪律`, etc. — they must still hold.)

- [ ] **Step 2: Extend the leader contract** — in `src/teams/leader_prompt.rs`, inside the `format!` string, replace the numbered list + anti-pattern line. The **old** block (match the file's exact indentation for the `\`-continued lines):

```
作为 leader，你要：\n\
         1. 把用户需求拆解成可分配的子任务，用 `task_create` 建任务并把 owner 设为合适成员的 agent_id。\n\
         2. 必要时用 `message_send` 与成员沟通、用 `team_delegate` 直接委派给成员。\n\
         3. 成员通过 dispatcher 异步执行，产出经 `task_submit` 落为 artifact。\n\
         4. 汇总成员产出，给用户一个清晰的最终答复。\n\n\
         不要自己闷头做完所有事，也不要用通用 subagent 顶替成员——你的价值是编排团队成员与汇总。
```

becomes (preserve the leading `\`-continuation indentation exactly as the surrounding lines use it):

```
作为 leader，你要：\n\
         1. 把用户需求拆解成可分配的子任务，用 `task_create` 建任务并把 owner 设为合适成员的 agent_id。\n\
         2. 用 `@<agent_id>` 在群里把任务派给成员——消息里务必带上 `task_create` 返回的 task_id，成员要凭它提交产出。必要时用 `team_delegate` 直接委派。\n\
         3. 成员用 `task_submit`（填你给的 task_id）交回产出后，任务转入待验收：先用 `task_read_artifact` 看产出，再用 `task_review`（decision=approve 通过并解锁后续任务 / reject 退回并把要改的写进 feedback 让成员重做）。\n\
         4. 全部子任务验收通过后，汇总成员产出，给用户一个清晰的最终答复。\n\n\
         不要自己闷头做完所有事，也不要用通用 subagent 顶替成员——你的价值是编排团队成员、验收成果与汇总。
```

(Leave the warning line — `⚠️ 调用任何团队工具（task_create、team_delegate、message_send、team_status、task_submit 等）…` — UNCHANGED: `task_review` takes a `task_id`, not `team_id`, so it does not belong in that team_id-mandatory list.)

- [ ] **Step 3: Extend the member obey-contract** — in `src/teams/broadcast/member_prompt.rs`, replace the non-leader `leader_block` string (the `else` branch). The **old** string:

```
"\n\n团队纪律:你在 leader 的统筹下工作。当 leader 通过 @ 或任务把活派给你时,\
         优先接下并尽力完成,把产出交回 leader,而不是只在群里闲聊。你仍可自由 @ 其他\
         成员协作,但讨论要服务于把任务做完。"
```

becomes:

```
"\n\n团队纪律:你在 leader 的统筹下工作。当 leader 通过 @ 把任务派给你时,他会带上一个 task_id;\
         你接下后尽力完成,用 `task_submit`(填那个 task_id)把产出交回,leader 会用 task_review 验收\
         ——被 reject 就按反馈重做再交。你仍可自由 @ 其他成员协作,但讨论要服务于把任务做完,而不是只在群里闲聊。"
```

- [ ] **Step 4: Commit**

```bash
git add src/teams/leader_prompt.rs src/teams/broadcast/member_prompt.rs
git commit -m "teams: leader names task_id on assign + names task_review; member submits against assigned id"
```

---

## Task 7: Compile gate + wrap-up

**Files:** none (verification only)

- [ ] **Step 1: Single compile gate** — the `~/.cargo/bin/cargo` symlink is broken; use the rustup toolchain path with the sandbox disabled. Run once:

```bash
TC=/Users/zouguojun/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin
PATH=$TC:$PATH RUSTC=$TC/rustc RUSTUP_HOME=$HOME/.rustup CARGO_HOME=$HOME/.cargo $TC/cargo test -p alephcore --lib --no-run
```

Expected: clean (compiles all production + `#[cfg(test)]` code). If errors, fix inline — common suspects, all pre-identified:
- the 3 tuple sites in `collab_session_tools.rs` (return type) + `collab_session_tools.rs` (return value) + `constructor/mod.rs` (destructure) must each have `task_review_tool` at the **same position** (right after `task_read_artifact_tool`); a position mismatch is a type error.
- the `Ok(Self { ... })` init in `constructor/mod.rs` must include `task_review_tool,`.
- `TaskSubmitTool::new` is now 3-arg at its sole call site.
- `broadcast/mod.rs` imports `CoordTaskFilter, CoordTaskStatus, CoordTaskStore`.
- `task_review.rs` test asserts use `CoordTaskStatus`'s `PartialEq`/`Debug` (both derived — OK).

- [ ] **Step 2: Bin check** — `handle_chat_send`'s new param + the `agent_init` thread live in the bin crate:

```bash
TC=/Users/zouguojun/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin
PATH=$TC:$PATH RUSTC=$TC/rustc RUSTUP_HOME=$HOME/.rustup CARGO_HOME=$HOME/.cargo $TC/cargo check --bin aleph-server
```

Expected: clean. The only bin-crate touch is the `agent_init` `handle_chat_send(...)` call gaining the `coord` arg.

- [ ] **Step 3: Run the new unit tests** (binary already built — fast, no recompile):

```bash
TC=/Users/zouguojun/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin
PATH=$TC:$PATH RUSTC=$TC/rustc RUSTUP_HOME=$HOME/.rustup CARGO_HOME=$HOME/.cargo $TC/cargo test -p alephcore --lib -- \
  target_status_maps_verdict authz_allows_leader_blocks_other_allows_unknown decision_parses_snake_case \
  new_takes_optional_coord_store newly_waiting_review_is_post_minus_pre \
  member_prompt leader_prompt
```

Expected: all pass.

- [ ] **Step 4: Final commit (only if fixes were made)**

```bash
git add src/builtin_tools/team/task_review.rs src/builtin_tools/team/task_submit.rs \
        src/teams/broadcast/mod.rs src/executor/builtin_registry \
        src/gateway/handlers/teams/canvas.rs \
        src/bin/aleph-server/commands/start/builder/agent_init/mod.rs \
        src/teams/leader_prompt.rs
git commit -m "teams: phase-2 strata verification loop compile fixes"
```

---

## Self-Review — spec coverage

| Spec §10 Phase-2 seam | Task |
|---|---|
| F1 `task_review` tool (approve→Completed / reject→**InProgress** / soft leader authz) | Task 1 |
| F1 registration — all 6 sites (schema, budget arm, group, registry field, dispatch arm, construct+tuple+destructure+init) | Task 2 |
| F2 submit-wiring (`TaskSubmitTool` + `CoordTaskStore`, flip → WaitingReview, graceful no-op) | Task 3 |
| F2-retrigger plumbing (broadcaster 5th `CoordTaskStore` field → handler → boot) | Task 4 |
| F2-retrigger logic (`run_member` pre/post WaitingReview diff → synthetic `@leader` dispatch, skip self-review) | Task 5 |
| Task-ID flow + review loop in prompts (leader names id + `task_review`; member submits against id) | Task 6 |
| Compile gate (one lib test-compile + bin check + run new tests) | Task 7 |

**Decisions baked in:** reject→InProgress (Task 1 `target_status`); prompt-level task-id (Task 6, no code surfacing); soft leader guard (Task 1 `is_authorized`, no-op+warn). **Synthetic dispatch:** embed `@<leader_id>`, `user_triggered=false`, `leader_first=false`, `sender=agent_id`, skip when `is_leader` (Task 5).

**Out of scope (Phase 3, its own plan):** D2 live kanban (`coord_task::global()`, `ResolvedContext.team_board`, `TeamBoardLayer`). Also deferred: disband-time delete of the `team_key` strategy row (harmless disk leak — fresh UUIDs per team mean no reuse), to fold into a Phase-3 cleanup.

**Type-consistency check:** `TaskReviewTool::new(coord_store, team_store, current_agent_id)` (Task 1) matches its sole construction (Task 2 Step 7) and the registry field type (Task 2 Step 4) + dispatch arm (Step 5). `TaskSubmitTool::new`'s 3-arg form (Task 3) matches its sole call site update (Task 3 Step 5). `GroupChatBroadcaster::new`'s 5-arg form (Task 4) matches its sole caller (canvas.rs, Task 4 Step 5). `handle_chat_send`'s new trailing param (Task 4 Step 4) matches its sole caller (`agent_init`, Task 4 Step 6). `newly_waiting_review` (Task 5 Step 2) is consumed only in Task 5 Step 4. The `task_review_tool` tuple element sits at the same index (after `task_read_artifact_tool`) in the return type, return value, and destructure (Task 2 Steps 6/8/9) + the `Ok(Self)` init (Step 10).
