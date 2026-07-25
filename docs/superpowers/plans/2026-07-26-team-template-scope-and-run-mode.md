# Team Template Tool Scope + Team Run Mode Pin — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Declare a narrowed tool surface on the two reasoning-oriented built-in team templates, and pin every team-originated agent run to `SessionMode::Work` so a global `[policies] mode` can never defer the tools the team prompts contract members to call.

**Architecture:** Two independent changes sharing one theme — a team member's tool surface. Item 1 fills in `tools` on `strategy-room` / `code-review` `.toml` files (the fields and their fail-fast validator already exist) and adds an honesty field reporting when a declaration was silently dropped by the agent-reuse branch. Item 2 introduces one constant, `teams::run_mode::TEAM_RUN_MODE`, stamped into request metadata by the two — and only two — team run producers, guarded by a test derived from the same essentials constants the toolset validator uses.

**Tech Stack:** Rust (alephcore), TOML templates, `cargo test -p alephcore --lib`.

Spec: `docs/superpowers/specs/2026-07-26-team-template-scope-and-run-mode-design.md`

## Global Constraints

- **`tools` is `retain`, not defer.** A tool excluded by a member's allowlist is removed from the loop registry; `tool_search` **cannot** promote it back. There is no in-run recovery from a too-narrow declaration. Err generous.
- **Never glob `team_*`.** That family contains `team_disband`, `team_create`, `team_from_template`, `team_member_remove`. Enumerate `team_status` / `team_delegate` instead. `task_*` **is** safe to glob.
- **Tool names are verified**, do not invent: `task_create`, `task_list`, `task_update`, `task_wait`, `task_comment`, `task_exit_journal`, `task_read_artifact`, `task_review`, `task_submit`, `team_status`, `team_delegate`, `message_send`, `search`, `web_fetch`, `file_read`, `file_write`, `file_edit`, `file_ops`, `apply_patch`, `ctx_search`, `code_check`, `bash`.
- **Do NOT run bare `cargo fmt`** in this repo — it reformats the whole tree even when given a path. Use `rustfmt --check --edition 2021 <file>` and fix by hand with Edit.
- **No `src/harness/` changes.** R10's 12-file / line ratchet is untouched by this work.
- Commit messages: `<scope>: <description>`, English, e.g. `teams: pin team-originated runs to Work`.
- Verify with `cargo test -p alephcore --lib <filter>` and `cargo clippy -p alephcore --all-targets -- -D warnings`.

---

### Task 1: `teams::run_mode` — the pinned mode and its invariant

**Files:**
- Create: `src/teams/run_mode.rs`
- Modify: `src/teams/mod.rs:7-22` (module list, alphabetical — `run_mode` goes between `plans` and `sessions`)
- Test: inline `#[cfg(test)] mod tests` in `src/teams/run_mode.rs`

**Interfaces:**
- Consumes: `crate::config::types::policies::{SessionMode, MODE_SESSION_KEY}`; `crate::teams::member_provision::{WORKER_ESSENTIAL_TOOLS, LEADER_ESSENTIAL_TOOLS}` (both `&[&str]`).
- Produces: `pub const TEAM_RUN_MODE: SessionMode` and `pub fn stamp(metadata: &mut std::collections::HashMap<String, String>)`. Task 2 calls `stamp`.

- [ ] **Step 1: Write the failing test**

Create `src/teams/run_mode.rs` with the tests only (no implementation yet, so it fails to compile — that is the RED signal here):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::teams::member_provision::{LEADER_ESSENTIAL_TOOLS, WORKER_ESSENTIAL_TOOLS};

    #[test]
    fn stamp_writes_the_pinned_mode() {
        let mut metadata = HashMap::new();
        stamp(&mut metadata);
        assert_eq!(
            metadata.get(MODE_SESSION_KEY).map(String::as_str),
            Some("work")
        );
    }

    /// The invariant this module exists for. A team member's launch prompt
    /// contracts it to call these verbs; the mode its run executes in must not
    /// defer them out of the initial tool list. The names come from the same
    /// two constants `member_provision::validate_toolset` checks a declared
    /// `tools` list against, so the declaration-side contract and the
    /// presentation-side partition cannot drift.
    #[test]
    fn pinned_mode_never_defers_a_team_protocol_essential() {
        for name in WORKER_ESSENTIAL_TOOLS
            .iter()
            .chain(LEADER_ESSENTIAL_TOOLS.iter())
        {
            assert!(
                !TEAM_RUN_MODE.defers_tool(name),
                "`{name}` is contracted by the team prompts but deferred by mode `{}`",
                TEAM_RUN_MODE.id()
            );
        }
    }

    /// Chat would strand the protocol — this asserts the guard above actually
    /// has teeth rather than passing vacuously because nothing is ever
    /// deferred.
    #[test]
    fn the_guard_would_catch_a_chat_pin() {
        assert!(
            SessionMode::Chat.defers_tool("task_create"),
            "chat must still defer the task family, else the guard is vacuous"
        );
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p alephcore --lib run_mode`
Expected: compile error — `cannot find function 'stamp'` / `cannot find value 'TEAM_RUN_MODE'`. (The module is not yet declared in `mod.rs` either, so it may report the file is unused first; declare it in Step 3 and re-run.)

- [ ] **Step 3: Write the implementation**

Prepend to `src/teams/run_mode.rs` (above the test module):

```rust
//! The usage mode every team-originated agent run executes in.
//!
//! A member run is not a user session. It has no composer, no mode pill, and
//! nobody watching it choose a register — the Panel hides `ModePicker` in team
//! chat for exactly that reason. Left unset, `resolve_turn_mode` falls through
//! to the global `[policies] mode`, which means an operator who switches their
//! own sessions to chat silently defers the `task` and `team` families out of
//! every team member's tool list — the same verbs `teams::leader_prompt` names
//! as the leader's four numbered duties.
//!
//! So team runs declare their mode instead of inheriting one. The narrowing
//! knob for a team is the per-member `tools` declaration
//! (`teams::member_provision`), not a session mode.

use std::collections::HashMap;

use crate::config::types::policies::{SessionMode, MODE_SESSION_KEY};

/// The mode every team-originated run executes in. `Work` is the identity
/// partition: it defers nothing and subtracts nothing from the core set, so a
/// team run's surface is exactly the registry minus whatever the member itself
/// declared.
pub const TEAM_RUN_MODE: SessionMode = SessionMode::Work;

/// Stamp [`TEAM_RUN_MODE`] onto a team run's request metadata.
///
/// `ExecutionEngine::resolve_turn_mode` treats a request-carried mode as
/// highest precedence and persists it onto the session; from the second turn on
/// `stored == requested` and the write short-circuits.
///
/// Called by both team run producers and nowhere else:
/// `teams::broadcast::member_run_metadata` (group chat fan-out) and
/// `teams::dispatcher::runner::task_run_metadata` (task dispatch / workflow
/// steps).
pub fn stamp(metadata: &mut HashMap<String, String>) {
    metadata.insert(
        MODE_SESSION_KEY.to_string(),
        TEAM_RUN_MODE.id().to_string(),
    );
}
```

Then add the module to `src/teams/mod.rs`, keeping the list alphabetical:

```rust
pub mod plans;
pub mod run_mode;
pub mod sessions;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib run_mode`
Expected: 3 passed.

- [ ] **Step 5: Lint and format check**

Run: `rustfmt --check --edition 2021 src/teams/run_mode.rs && cargo clippy -p alephcore --lib -- -D warnings`
Expected: no output from rustfmt, clippy clean. Fix any rustfmt complaint by hand with Edit — do not run bare `cargo fmt`.

- [ ] **Step 6: Commit**

```bash
git add src/teams/run_mode.rs src/teams/mod.rs
git commit -m "teams: declare the usage mode team runs execute in

A member run is not a user session, so it has no mode pill and inherits
the global [policies] mode by default -- which means an operator on chat
silently defers the task/team families out of every member's tool list,
including the verbs leader_prompt names as the leader's duties. Pin the
mode in one constant instead, guarded by an assertion derived from the
same essentials constants validate_toolset checks declarations against."
```

---

### Task 2: Wire both team run producers to the pin

**Files:**
- Modify: `src/teams/broadcast/mod.rs:130-139` (`member_run_metadata`)
- Modify: `src/teams/dispatcher/runner.rs:210-236` (extract the inline metadata block into a named function)
- Test: `src/teams/broadcast/mod.rs` tests module (starts line 774); `src/teams/dispatcher/runner.rs` tests module (starts line 485)

**Interfaces:**
- Consumes: `crate::teams::run_mode::stamp` from Task 1.
- Produces: `fn task_run_metadata(team_id: &str, task_id: &str, think_level: Option<&str>, worktree: Option<&WorktreeHandle>) -> HashMap<String, String>` in `dispatcher/runner.rs` (private, test-visible via `use super::*`).

- [ ] **Step 1: Write the failing tests**

In `src/teams/broadcast/mod.rs`, inside the existing `mod tests`, next to `member_metadata_tags_webchat_platform`:

```rust
    /// A member run must declare its mode rather than inherit the global
    /// `[policies] mode` — chat defers the `task`/`team` families the member
    /// and leader prompts contract these agents to call.
    #[test]
    fn member_metadata_pins_the_team_run_mode() {
        let metadata = member_run_metadata("team-1", 0);
        assert_eq!(
            metadata
                .get(crate::config::types::policies::MODE_SESSION_KEY)
                .map(String::as_str),
            Some("work")
        );
    }
```

In `src/teams/dispatcher/runner.rs`, inside the existing `mod tests`:

```rust
    #[test]
    fn task_run_metadata_pins_the_team_run_mode() {
        let m = task_run_metadata("t1", "task-9", None, None);
        assert_eq!(
            m.get(crate::config::types::policies::MODE_SESSION_KEY)
                .map(String::as_str),
            Some("work")
        );
        assert_eq!(m.get("team_id").map(String::as_str), Some("t1"));
        assert_eq!(m.get("task_id").map(String::as_str), Some("task-9"));
        assert!(
            !m.contains_key(crate::agents::thinking::THINK_LEVEL_SESSION_KEY),
            "an undeclared effort must not write a think-level key"
        );
    }

    /// The per-step `effort` override still rides the same metadata map.
    #[test]
    fn task_run_metadata_carries_a_declared_think_level() {
        let m = task_run_metadata("t1", "task-9", Some("high"), None);
        assert_eq!(
            m.get(crate::agents::thinking::THINK_LEVEL_SESSION_KEY)
                .map(String::as_str),
            Some("high")
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib member_metadata_pins && cargo test -p alephcore --lib task_run_metadata`
Expected: the broadcast test FAILS with `left: None, right: Some("work")`; the runner tests fail to compile with `cannot find function 'task_run_metadata'`.

- [ ] **Step 3: Implement — broadcast**

In `src/teams/broadcast/mod.rs`, `member_run_metadata` becomes:

```rust
fn member_run_metadata(
    team_id: &str,
    chain_depth: u32,
) -> std::collections::HashMap<String, String> {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("team_id".to_string(), team_id.to_string());
    metadata.insert("chain_depth".to_string(), chain_depth.to_string());
    metadata.insert("platform".to_string(), "webchat".to_string());
    crate::teams::run_mode::stamp(&mut metadata);
    metadata
}
```

Append this paragraph to that function's existing doc comment (after the `UNATTENDED_KEY` paragraph):

```rust
/// Also carries the pinned usage mode (`teams::run_mode`). Without it the run
/// falls through to the global `[policies] mode`, and a chat-mode install
/// defers the `task`/`team` families this member's prompt tells it to call.
```

- [ ] **Step 4: Implement — dispatcher**

In `src/teams/dispatcher/runner.rs`, add this function immediately above `execute_member_task` (line ~139):

```rust
/// Build the request metadata for one dispatched member task.
///
/// Named (rather than inline) so the pinned usage mode is assertable: this and
/// `broadcast::member_run_metadata` are the only two team run producers, and
/// both must stamp it — see `teams::run_mode`.
///
/// Deliberately carries no `UNATTENDED_KEY`, unlike cron / heartbeat / A2A /
/// goal continuations: a member run has no channel, so a confirm-gated tool
/// resolves through `OperatorApprovalRequester` to a Panel card that the user
/// who dispatched the team can answer. The marker would auto-deny that working
/// human-in-the-loop path.
fn task_run_metadata(
    team_id: &str,
    task_id: &str,
    think_level: Option<&str>,
    worktree: Option<&WorktreeHandle>,
) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("team_id".to_string(), team_id.to_string());
    m.insert("task_id".to_string(), task_id.to_string());
    crate::teams::run_mode::stamp(&mut m);
    // Per-step effort override (workflow `effort`): the execution engine's
    // `resolve_turn_think_level` reads this request-carried key first, so the
    // member run thinks at the step's declared depth.
    if let Some(level) = think_level {
        m.insert(
            crate::agents::thinking::THINK_LEVEL_SESSION_KEY.to_string(),
            level.to_string(),
        );
    }
    if let Some(handle) = worktree {
        m.insert(
            "team_worktree_path".to_string(),
            handle.path().display().to_string(),
        );
        // Lets `run_agent_loop` build a rebasing `FsScope::worktree` so
        // parent-repo absolute paths are redirected into the checkout,
        // matching the subagent spawner's isolation semantics.
        m.insert(
            "team_worktree_repo_root".to_string(),
            handle.repo_root().display().to_string(),
        );
    }
    m
}
```

Then replace the inline `let metadata = { … };` block (currently lines ~210-236, the block that starts with the `// Deliberately carries no UNATTENDED_KEY` comment and ends with `};`) — including that comment, which now lives on the function — with:

```rust
    let metadata = task_run_metadata(
        team_id,
        task_id,
        think_level.as_deref(),
        worktree_handle.as_ref(),
    );
```

Confirm `use std::collections::HashMap;` is present at the top of `runner.rs`; add it if the inline block was the only user and the import was scoped.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib member_metadata && cargo test -p alephcore --lib task_run_metadata`
Expected: all pass, including the pre-existing `member_metadata_tags_webchat_platform`.

- [ ] **Step 6: Run the wider team suite for regressions**

Run: `cargo test -p alephcore --lib teams`
Expected: no new failures. Note any pre-existing failures explicitly rather than assuming they are yours.

- [ ] **Step 7: Lint and format check**

Run: `rustfmt --check --edition 2021 src/teams/broadcast/mod.rs src/teams/dispatcher/runner.rs && cargo clippy -p alephcore --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add src/teams/broadcast/mod.rs src/teams/dispatcher/runner.rs
git commit -m "teams: stamp the pinned run mode on both team run producers

Group-chat fan-out and task dispatch are the only two producers of a
team-originated RunRequest; both now carry session_mode explicitly. The
dispatcher's inline metadata block becomes a named task_run_metadata so
the pin is assertable there too, matching broadcast's shape."
```

---

### Task 3: Declare `tools` on the two reasoning templates

**Files:**
- Modify: `src/teams/templates/builtin/strategy-room.toml`
- Modify: `src/teams/templates/builtin/code-review.toml`
- Test: `src/teams/templates/materialize.rs` tests module (starts line 580)

**Interfaces:**
- Consumes: `MemberToolset::new`, `validate_toolset`, `MemberContract` (already imported at `materialize.rs:22-23`); `crate::teams::templates::TemplateRegistry`.
- Produces: nothing new in code — the built-in `.toml` bodies gain `tools` keys that `TemplateLeader.tools` / `TemplateMember.tools` already deserialize.

- [ ] **Step 1: Write the failing tests**

Add to `src/teams/templates/materialize.rs`'s `mod tests`. Add `use std::path::Path;` and `use crate::teams::templates::TemplateRegistry;` to that module's imports.

```rust
    /// Built-ins only: pointing discovery at a directory that does not exist
    /// skips the user-override pass.
    fn builtins() -> TemplateRegistry {
        TemplateRegistry::discover(Path::new("/nonexistent/aleph-builtin-only"))
    }

    /// Every built-in role must satisfy the launch-prompt contract it runs
    /// under. A future template edit that strands a member fails here instead
    /// of at a user's `team_from_template`.
    #[test]
    fn builtin_templates_satisfy_their_member_contracts() {
        let registry = builtins();
        let mut checked = 0;
        for name in [
            "software-dev",
            "code-review",
            "research-paper",
            "strategy-room",
        ] {
            let tpl = registry
                .get(name)
                .unwrap_or_else(|| panic!("built-in `{name}` missing from the registry"));
            let leader = MemberToolset::new(tpl.leader.tools.clone(), tpl.leader.tools_denied.clone());
            if let Err(missing) = validate_toolset(&leader, MemberContract::Leader) {
                panic!("`{name}` leader hides contracted verbs: {missing:?}");
            }
            checked += 1;
            for m in &tpl.members {
                let ts = MemberToolset::new(m.tools.clone(), m.tools_denied.clone());
                if let Err(missing) = validate_toolset(&ts, MemberContract::Worker) {
                    panic!("`{name}` member `{}` hides contracted verbs: {missing:?}", m.id);
                }
                checked += 1;
            }
        }
        assert_eq!(checked, 18, "expected 4 leaders + 14 members to be checked");
    }

    /// Which built-ins narrow and which stay full is a decision, not an
    /// accident: the two reasoning templates declare a surface, the two
    /// build/run templates deliberately do not (their members need a broad dev
    /// surface, and their member ids are the generic ones — `backend`, `qa`,
    /// `writer`, `analyst` — that a declaration would pin narrow for every
    /// later use of that global agent id).
    #[test]
    fn only_the_reasoning_templates_declare_a_surface() {
        let registry = builtins();
        for name in ["strategy-room", "code-review"] {
            let tpl = registry.get(name).expect("built-in present");
            assert!(
                tpl.leader.tools.is_some(),
                "`{name}` leader must declare a surface"
            );
            for m in &tpl.members {
                assert!(
                    m.tools.is_some(),
                    "`{name}` member `{}` must declare a surface",
                    m.id
                );
            }
        }
        for name in ["software-dev", "research-paper"] {
            let tpl = registry.get(name).expect("built-in present");
            assert!(
                tpl.leader.tools.is_none(),
                "`{name}` leader must keep the full surface"
            );
            for m in &tpl.members {
                assert!(
                    m.tools.is_none(),
                    "`{name}` member `{}` must keep the full surface",
                    m.id
                );
            }
        }
    }

    /// The point of narrowing code-review: a reviewer must not have edit tools
    /// within reach. (`bash` stays — reading a diff needs `git diff` — so this
    /// is accident scoping, not enforcement.)
    #[test]
    fn code_review_roles_carry_no_edit_tools() {
        let registry = builtins();
        let tpl = registry.get("code-review").expect("built-in present");
        let surfaces = std::iter::once((
            "lead-reviewer",
            tpl.leader.tools.as_ref().expect("declared"),
        ))
        .chain(
            tpl.members
                .iter()
                .map(|m| (m.id.as_str(), m.tools.as_ref().expect("declared"))),
        );
        for (id, tools) in surfaces {
            for banned in ["file_write", "file_edit", "file_ops", "apply_patch"] {
                assert!(
                    !tools.iter().any(|t| t == banned),
                    "`{banned}` must stay out of reviewer `{id}`'s surface"
                );
            }
        }
    }

    /// `team_*` must never be globbed: the family contains `team_disband`,
    /// `team_create`, `team_from_template` and `team_member_remove`, so a glob
    /// would let a bull-case analyst disband its own team.
    #[test]
    fn no_builtin_globs_the_team_family() {
        let registry = builtins();
        for name in ["strategy-room", "code-review"] {
            let tpl = registry.get(name).expect("built-in present");
            let all = std::iter::once(tpl.leader.tools.as_ref().expect("declared"))
                .chain(tpl.members.iter().map(|m| m.tools.as_ref().expect("declared")));
            for tools in all {
                assert!(
                    !tools.iter().any(|t| t == "team_*"),
                    "`{name}` must enumerate team verbs, not glob the family"
                );
            }
        }
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib templates::materialize`
Expected: `only_the_reasoning_templates_declare_a_surface` FAILS ("`strategy-room` leader must declare a surface"); `code_review_roles_carry_no_edit_tools` and `no_builtin_globs_the_team_family` FAIL on `.expect("declared")`; `builtin_templates_satisfy_their_member_contracts` PASSES already (undeclared is always valid).

- [ ] **Step 3: Declare the strategy-room surface**

In `src/teams/templates/builtin/strategy-room.toml`, add to the header comment block:

```toml
# Tool surface: this template declares `tools` on every role. Note member ids
# are GLOBAL agent ids — an agent created here keeps this surface for every
# later use, inside a team or out. `tools` is an allowlist `retain`, not a
# deferral: an excluded tool cannot be recovered with `tool_search`.
```

Add `tools` to `[leader]` (after `role = "leader"`):

```toml
tools = ["task_*", "team_status", "team_delegate", "message_send", "search", "web_fetch", "file_read"]
```

Add to each of the three `[[members]]` (`bull`, `bear`, `contrarian`), after their `role = "analyst"`:

```toml
tools = ["task_*", "team_status", "message_send", "search", "web_fetch", "file_read"]
```

- [ ] **Step 4: Declare the code-review surface**

In `src/teams/templates/builtin/code-review.toml`, add to the header comment block:

```toml
# Tool surface: every role declares `tools`. Reviewers keep read + inspect
# tools and `bash` (reading a diff needs `git diff`) but NOT file_write /
# file_edit / file_ops / apply_patch — the failure mode being scoped out is a
# reviewer "helpfully" editing the code it was asked to review. Since bash can
# write, this is attention scoping, not an enforcement boundary. Member ids are
# GLOBAL agent ids and keep this surface outside the team too.
```

Add to `[leader]` (after `role = "leader"`):

```toml
tools = ["task_*", "team_status", "team_delegate", "message_send", "file_read", "search", "ctx_search", "code_check", "bash"]
```

Add to each of the four `[[members]]` (`security-reviewer`, `perf-reviewer`, `correctness-reviewer`, `style-reviewer`), after their `role = …` line:

```toml
tools = ["task_*", "team_status", "message_send", "file_read", "search", "ctx_search", "code_check", "bash"]
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib templates`
Expected: all five tests pass. If `builtin_templates_satisfy_their_member_contracts` now fails, a declaration is missing a contracted verb — the panic names which; add it rather than removing the assertion.

- [ ] **Step 6: Lint**

Run: `cargo clippy -p alephcore --all-targets -- -D warnings` and `rustfmt --check --edition 2021 src/teams/templates/materialize.rs`
Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/teams/templates/builtin/strategy-room.toml src/teams/templates/builtin/code-review.toml src/teams/templates/materialize.rs
git commit -m "teams: declare a tool surface on the two reasoning templates

strategy-room's analysts and code-review's reviewers are read-and-reason
roles; narrowing them removes the edit tools a reviewer should not have at
hand and trims a large surface nobody in those teams uses. software-dev and
research-paper stay full: their members genuinely need a broad build/run
surface, and their generic member ids (backend, qa, writer, analyst) are
global agent ids a declaration would pin narrow everywhere.

Tests pin the contract for every built-in role, that only these two declare,
that no reviewer carries an edit tool, and that nothing globs team_* (which
would admit team_disband)."
```

---

### Task 4: Report declarations the reuse branch drops

**Files:**
- Modify: `src/teams/templates/materialize.rs:49-59` (`MaterializedTeam`), `:110-156` (leader + member resolution), `:271-278` (construction), `:358-372` (`provision_member` signature and reuse branch)
- Modify: `src/builtin_tools/team/from_template.rs:46-67` (`TeamFromTemplateOutput` + `From` impl)
- Test: new `#[cfg(test)] mod tests` in `src/builtin_tools/team/from_template.rs`

**Interfaces:**
- Consumes: `MaterializedTeam` from Task 3's file, `MemberToolset::is_unrestricted` (existing).
- Produces: `MaterializedTeam.tools_ignored_for: Vec<String>` and `TeamFromTemplateOutput.tools_ignored_for: Vec<String>`; private `struct ProvisionedMember { agent_id: String, tools_ignored: bool }` in `materialize.rs`.

- [ ] **Step 1: Write the failing tests**

Add a new test module at the end of `src/builtin_tools/team/from_template.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn materialized(tools_ignored_for: Vec<String>) -> MaterializedTeam {
        MaterializedTeam {
            team_id: "team-1".into(),
            team_name: "n".into(),
            leader_id: "lead".into(),
            member_ids: vec!["bull".into()],
            task_ids: vec![],
            message: "ok".into(),
            tools_ignored_for,
        }
    }

    /// The common case must not churn the tool's output shape.
    #[test]
    fn an_empty_report_is_omitted_from_the_output() {
        let out = TeamFromTemplateOutput::from(materialized(vec![]));
        let v = serde_json::to_value(&out).expect("serializes");
        assert!(
            v.get("tools_ignored_for").is_none(),
            "an empty report must not appear in the output"
        );
    }

    /// When a template's declaration was dropped, the caller must be told
    /// which member it happened to — otherwise the team silently does not
    /// match what the template says.
    #[test]
    fn a_dropped_declaration_reaches_the_caller() {
        let out = TeamFromTemplateOutput::from(materialized(vec!["bull".into()]));
        let v = serde_json::to_value(&out).expect("serializes");
        assert_eq!(v["tools_ignored_for"], serde_json::json!(["bull"]));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p alephcore --lib from_template`
Expected: compile error — `struct MaterializedTeam has no field named tools_ignored_for`.

- [ ] **Step 3: Implement — `MaterializedTeam` gains the field**

In `src/teams/templates/materialize.rs`, add to the struct (after `task_ids`):

```rust
    /// Member ids whose template `tools` / `tools_denied` declaration had no
    /// effect — an agent with that id already existed and was reused (or the
    /// leader is `self`), and an existing agent keeps its own surface.
    ///
    /// Silence here would be a lie: the caller asked for a team of narrowed
    /// members and got something else. Empty in the common case.
    pub tools_ignored_for: Vec<String>,
```

- [ ] **Step 4: Implement — `provision_member` reports**

Add above `provision_member`:

```rust
/// Outcome of resolving one template member to a live agent id.
struct ProvisionedMember {
    agent_id: String,
    /// The spec declared a tool surface that had no effect, because an
    /// existing agent was reused.
    tools_ignored: bool,
}
```

Change the signature's return type to `Result<ProvisionedMember, TeamTemplateError>`, and replace the reuse branch (currently `materialize.rs:365-372`) with:

```rust
    // Reuse existing agent when present.
    if deps.registry.get(&member.id).await.is_some() {
        if let Some(addendum) = &member.prompt_addendum {
            let rendered = substitute(addendum, vars);
            let role = member.role.as_deref().unwrap_or("worker");
            inject_role_prompt(deps, &member.id, role, &rendered).await;
        }
        let declared =
            !MemberToolset::new(member.tools.clone(), member.tools_denied.clone()).is_unrestricted();
        if declared {
            info!(
                member = %member.id,
                "team_template: reusing an existing agent; the template's `tools` \
                 declaration does not apply (an existing agent keeps its own surface)"
            );
        }
        return Ok(ProvisionedMember {
            agent_id: member.id.clone(),
            tools_ignored: declared,
        });
    }
```

At the end of the function, wherever it currently returns the created id, return:

```rust
    Ok(ProvisionedMember {
        agent_id: member.id.clone(),
        tools_ignored: false,
    })
```

- [ ] **Step 5: Implement — collect at both call sites**

In `materialize_template`, declare the accumulator before the leader block:

```rust
    let mut tools_ignored_for: Vec<String> = Vec::new();
```

The `self`-leader branch (`materialize.rs:110-123`) also drops a declaration — `id = "self"` means the caller is an existing agent that keeps its own surface. Inside that branch, after the addendum injection, add:

```rust
        if !MemberToolset::new(tpl.leader.tools.clone(), tpl.leader.tools_denied.clone())
            .is_unrestricted()
        {
            info!(
                leader = %req.current_agent_id,
                "team_template: leader is `self`; the template's `tools` declaration \
                 does not apply (the calling agent keeps its own surface)"
            );
            tools_ignored_for.push(req.current_agent_id.clone());
        }
```

The non-self leader branch becomes:

```rust
        let provisioned = provision_member(
            deps,
            &pseudo_member,
            &req.current_agent_id,
            &vars,
            MemberContract::Leader,
        )
        .await?;
        if provisioned.tools_ignored {
            tools_ignored_for.push(provisioned.agent_id.clone());
        }
        provisioned.agent_id
```

The member loop becomes:

```rust
    for m in &tpl.members {
        let provisioned = provision_member(
            deps,
            m,
            &req.current_agent_id,
            &vars,
            MemberContract::Worker,
        )
        .await?;
        if provisioned.tools_ignored {
            tools_ignored_for.push(provisioned.agent_id.clone());
        }
        enrolled_members.push(provisioned.agent_id);
    }
```

And the final construction (`materialize.rs:271`) gains `tools_ignored_for,`.

- [ ] **Step 6: Implement — project onto the tool output**

In `src/builtin_tools/team/from_template.rs`, add to `TeamFromTemplateOutput` (after `task_ids`):

```rust
    /// Members whose template `tools` declaration had no effect because an
    /// agent with that id already existed. Omitted when empty so the common
    /// case's output is unchanged.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools_ignored_for: Vec<String>,
```

and to the `From<MaterializedTeam>` body:

```rust
            tools_ignored_for: m.tools_ignored_for,
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p alephcore --lib from_template && cargo test -p alephcore --lib templates`
Expected: all pass.

- [ ] **Step 8: Full build and lint**

Run: `cargo test -p alephcore --lib && cargo clippy -p alephcore --all-targets -- -D warnings`
Expected: no new failures. Report any pre-existing failures rather than absorbing them.

- [ ] **Step 9: Commit**

```bash
git add src/teams/templates/materialize.rs src/builtin_tools/team/from_template.rs
git commit -m "teams: report template tool declarations the reuse branch drops

provision_member reuses an existing agent by id and skips the template's
tools declaration -- correct (an existing agent keeps its own surface) but
silent, so a team could quietly not match the template it came from. Now
materialization collects those member ids and team_from_template surfaces
them, omitted from the output when empty so the common case is unchanged."
```

---

### Task 5: Documentation

**Files:**
- Modify: `docs/reference/MULTI_AGENT_SYSTEM.md` (the "Member Tool Surface" section, after the "Not derived from `role`" paragraph at ~line 470)
- Modify: `docs/reference/MODE_SYSTEM.md` (near line 104, where team-chat pill hiding is already described)
- Modify: `docs/reference/FEATURE_LOCATOR.md` (§4.5 打磨话术, line ~518)

**Interfaces:**
- Consumes: everything from Tasks 1-4.
- Produces: nothing consumed by later tasks.

- [ ] **Step 1: MULTI_AGENT_SYSTEM.md — built-in template scope**

Insert after the "**Not derived from `role`.**" paragraph:

```markdown
**Built-in templates: two declare, two do not.** `strategy-room` (moderator +
bull/bear/contrarian) and `code-review` (lead-reviewer + four lens reviewers)
declare a surface; `software-dev` and `research-paper` deliberately do not.
Two reasons line up. Fit: the build/run roles genuinely need a broad dev
surface, and §"Not exhaustive" means guessing narrow is unrecoverable — a
`tools` list is a `retain`, so an excluded tool cannot be promoted back with
`tool_search` the way a mode-deferred one can. Blast radius: template member
ids are **global agent ids**, and the generic ones (`lead`, `backend`,
`frontend`, `qa`, `pi`, `reviewer`, `analyst`, `writer`) all live in the two
undeclared templates.

`team_*` is never globbed in a declaration — the family contains
`team_disband` / `team_create` / `team_from_template` / `team_member_remove`.
Enumerate `team_status` and `team_delegate`. `task_*` is safe to glob.

For code-review the surface keeps `bash` (reading a diff needs `git diff`) and
drops `file_write` / `file_edit` / `file_ops` / `apply_patch`. Since bash can
write, that is attention scoping — it targets the reviewer who "helpfully"
edits the code it was asked to review, not an attacker.

**When a declaration is dropped.** `provision_member` reuses an existing agent
by id and skips `tools` entirely (an existing agent keeps its own surface), and
a `self` leader is the caller's own agent. Both cases are reported:
`MaterializedTeam.tools_ignored_for` → `TeamFromTemplateOutput.tools_ignored_for`,
omitted from the output when empty. Guards live in `templates/materialize.rs`
tests: every built-in role satisfies its contract, only the two reasoning
templates declare, no reviewer carries an edit tool, nothing globs `team_*`.
```

- [ ] **Step 2: MODE_SYSTEM.md — team runs pin Work**

Append to the line-104 area (the paragraph noting the two pills are hidden in team chat):

```markdown
**团队 run 钉 Work，不继承全局。** 成员 run 不是用户会话（无 composer、无 pill），
留空会落到全局 `[policies] mode`——`chat` 档把 `task`/`team` 整族 defer 掉，正好是
`teams::leader_prompt` 点名要 leader 调的四步动词。所以两个团队 run 产地
（`broadcast::member_run_metadata` 群聊、`dispatcher::runner::task_run_metadata`
任务派发/workflow step）都显式写入 `teams::run_mode::TEAM_RUN_MODE`。
不变量测试从 `member_provision` 的 `WORKER_ESSENTIAL_TOOLS` /
`LEADER_ESSENTIAL_TOOLS` 取数——声明侧契约与呈现侧分区因此不可能各说各话。
**团队要收窄工具面用每成员 `tools` 声明，不是用 mode。**
```

- [ ] **Step 3: FEATURE_LOCATOR.md — §4.5 打磨话术**

Append to the §4.5 打磨话术 paragraph (line ~518), after the existing `tools` 声明 sentence:

```markdown
‘团队 run 跑在什么 mode’＝**恒 Work，显式钉的**（`src/teams/run_mode.rs::TEAM_RUN_MODE`，两个产地 `broadcast::member_run_metadata` / `dispatcher::runner::task_run_metadata` 都 stamp）——**别让它去继承全局 `[policies] mode`**：chat 档 defer 掉 `task`/`team` 整族，leader prompt 点名要的 `task_create`/`team_delegate`/`task_review` 全不在初始列表里。收窄团队工具面走每成员 `tools`，不走 mode。‘内置模板哪些声明了工具面’＝**只有 `strategy-room` + `code-review`**（纯推理/只读角色），`software-dev`/`research-paper` 刻意保持全量——**别顺手给它们加**：模板成员 id 是全局 agent id，那两个模板占着 `backend`/`qa`/`writer`/`analyst` 这些泛名，一旦声明就把该 agent 在团队之外也钉窄了。**`tools` 里别写 `team_*` glob**（族里有 `team_disband`/`team_create`），枚举 `team_status`+`team_delegate`。‘我声明了模板 tools 但成员还是全量’＝那个 id 的 agent 已存在走了复用分支，看返回里的 `tools_ignored_for`。
```

- [ ] **Step 4: Verify the markdown renders and links resolve**

Run: `grep -n "tools_ignored_for" docs/reference/MULTI_AGENT_SYSTEM.md docs/reference/FEATURE_LOCATOR.md && grep -n "TEAM_RUN_MODE" docs/reference/MODE_SYSTEM.md docs/reference/FEATURE_LOCATOR.md`
Expected: each file reports its new anchors.

- [ ] **Step 5: Commit**

```bash
git add docs/reference/MULTI_AGENT_SYSTEM.md docs/reference/MODE_SYSTEM.md docs/reference/FEATURE_LOCATOR.md
git commit -m "docs: record built-in template scope and the team run mode pin

MULTI_AGENT_SYSTEM gets the two-declare/two-don't decision with both
reasons (retain-not-defer, global member ids) and the tools_ignored_for
report; MODE_SYSTEM records that team runs pin Work rather than inherit
the global mode; FEATURE_LOCATOR 4.5 gets the lookup phrasing."
```

---

### Task 6: Runtime QA on a live daemon (manual, human-run)

**Files:** none — this task changes no code. It is the acceptance gate.

**Interfaces:**
- Consumes: a built binary containing Tasks 1-4.

This task requires a real daemon and a browser/Panel session. Do not mark it complete from unit tests. Before starting, confirm the browser you drive is the local one (`list_connected_browsers` → `isLocal: true`) — a session that drifts to another machine's Chrome produces confusing "service is broken" false positives.

- [ ] **Step 1: Reproduce the item-2 defect BEFORE the fix is running**

On a daemon built from `main` **without** Task 1-2 (e.g. stash or a pre-fix binary), set `[policies] mode = "chat"` in `~/.aleph/config.toml`, then open a team group chat and ask the leader to list every tool it can call.
Expected (the defect): `task_create` and `team_delegate` are **absent**.
If they are present, stop and report — the premise of Task 1-2 is wrong and the plan needs revisiting.

- [ ] **Step 2: Confirm the pin holds after the fix**

Rebuild with Tasks 1-4, restart the daemon, keep `[policies] mode = "chat"`, repeat the same group-chat probe.
Expected: `task_create` and `team_delegate` are present again.
Then restore `[policies] mode` to its prior value.

- [ ] **Step 3: Verify the template path narrows a member**

```
team_from_template(template='strategy-room', team_name='qa-strategy', goal='评估是否自建搜索索引')
```
Then probe a member directly — the Panel's agent selector does **not** route messages, so use the RPC:
`chat.send { agent_id: "bull", message: "列出你能调用的全部工具" }`

Expected: exactly `task_*` (create/list/update/wait/comment/exit_journal/read_artifact/review/submit), `team_status`, `message_send`, `search`, `web_fetch`, `file_read` — plus `get_tool_schema` and `subagent`, which are registered after the allowlist filter and are documented as always present. Anything else means a declared name is wrong; anything missing means a typo silently stripped it.

- [ ] **Step 4: Verify it survives a restart**

Restart the daemon and repeat the Step 3 probe. This exercises the persisted `AgentDefinition.skills` → `from_resolved` → `tool_whitelist` path rather than the in-boot config.
Expected: identical list.

- [ ] **Step 5: Verify fail-fast on a stranding declaration**

Write `~/.aleph/teams/templates/qa-strand.toml`:

```toml
description = "QA fixture: a worker declaration that hides its hand-off verbs"

[leader]
id = "qa-strand-lead"
role = "leader"

[[members]]
id = "qa-strand-worker"
role = "worker"
tools = ["search"]

[[tasks]]
key = "only"
subject = "noop"
owner = "qa-strand-worker"
```

Then `team_from_template(template='qa-strand', team_name='qa-strand-team', goal='noop')`.
Expected: an error naming `task_submit, message_send`, and `ls ~/.aleph/agents/qa-strand-worker` reports no such directory. Delete the fixture afterwards.

- [ ] **Step 6: Verify the reuse report**

With `bull` now existing from Step 3, materialize `strategy-room` a second time under a different team name.
Expected: the result carries `tools_ignored_for` listing the reused member ids, and the daemon log has the matching `team_template: reusing an existing agent` line.

- [ ] **Step 7: Record the outcome**

Append the observed tool counts and any surprises to the spec's §5.2 as a "QA result" note, and commit that doc change. If any step failed, file it as a finding rather than silently adjusting the assertion.

---

## Self-Review

**Spec coverage.** §3.1 (which templates) → Task 3 + its `only_the_reasoning_templates_declare_a_surface` test. §3.2 (the declarations) → Task 3 Steps 3-4. §3.3 (contract check) → Task 3's `builtin_templates_satisfy_their_member_contracts`. §3.4 (honest signal) → Task 4. §3.5 (not namespacing ids) → nothing to do, recorded in Task 5's doc text. §4.1-4.2 (constant + two call sites) → Tasks 1-2. §4.3 (the invariant test) → Task 1 Step 1. §4.4-4.5 (scope boundaries) → Task 5 docs. §5.1 → Tasks 1-4 test steps. §5.2 → Task 6. §6 → Task 5.

One deliberate widening over the spec: §3.4 scoped the report to the reuse branch; Task 4 Step 5 also reports the `self`-leader branch, which drops a declaration for the same reason. The field's doc comment says so.

**Placeholders.** None — every step carries the literal code or command.

**Type consistency.** `stamp(&mut HashMap<String, String>)` is defined in Task 1 and called in Task 2 with `&mut metadata` / `&mut m`. `task_run_metadata(&str, &str, Option<&str>, Option<&WorktreeHandle>)` is defined in Task 2 Step 4 and called in Task 2 Step 4's replacement block with `think_level.as_deref()` / `worktree_handle.as_ref()`, matching the tests in Step 1. `ProvisionedMember { agent_id, tools_ignored }` is defined in Task 4 Step 4 and destructured in Step 5. `tools_ignored_for: Vec<String>` has the same name on `MaterializedTeam` and `TeamFromTemplateOutput`, and Task 4's test constructs `MaterializedTeam` with every field the struct will have after Step 3.
