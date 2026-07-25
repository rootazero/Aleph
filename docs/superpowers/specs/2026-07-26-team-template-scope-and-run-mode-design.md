# Team Template Tool Scope + Team Run Mode Pin

Date: 2026-07-26
Scope: `src/teams/templates/builtin/*.toml`, `src/teams/templates/materialize.rs`,
`src/teams/run_mode.rs` (new), `src/teams/broadcast/mod.rs`,
`src/teams/dispatcher/runner.rs`, `src/builtin_tools/team/from_template.rs`
Follow-up to: `14b6070e2` (member tool surface) + `0863c4edf` (runtime QA note)

## 1. Why

Two items were left open when inline members gained a declared tool surface.

**Item 1 — the four built-in templates declare no scope.** `TemplateLeader` /
`TemplateMember` carry `tools` / `tools_denied` (`templates/types.rs:57,86`) and
materialization already runs `validate_toolset` (`materialize.rs:383`), but
`software-dev` / `code-review` / `research-paper` / `strategy-room` leave the
fields unset, so every member a template creates sees the full ~215-tool surface.

**Item 2 — team runs inherit the global mode.** `member_run_metadata`
(`broadcast/mod.rs:130`) and the dispatcher's metadata block
(`dispatcher/runner.rs:210`) carry no `session_mode` key, and a member's session
(`SessionKey::task(agent, "team_chat"|"team", …)`) is never stamped. So
`resolve_turn_mode` falls through `requested → stored → global` to
`[policies] mode`. That is not "always Work" — it is "always whatever the
operator set globally", which is a live defect: with `[policies] mode = "chat"`,
`SessionMode::defers_tool` hides the whole `task` and `team` families, and
`leader_prompt::build` names `task_create` / `team_delegate` / `task_review` /
`task_read_artifact` / `team_status` as the leader's four numbered duties.
`NEVER_DEFER` does not protect them.

## 2. Constraint that shapes item 1: `tools` is retain, not defer

The two narrowing mechanisms are not symmetric, and the difference decides how
generous a declaration must be:

| | mechanism | recoverable at run time? |
|---|---|---|
| session mode | `DeferredTools` — collapsed out of the initial list | **yes**, `tool_search` promotes |
| member `tools` | allowlist `retain` on the loop registry | **no**, the tool is gone |

The prior runtime QA showed a member declaring
`["task_*", "message_send", "search", "web_fetch"]` enumerating exactly those
plus `get_tool_schema` and `subagent` — `tool_search` was *not* among them. So a
too-narrow declaration has no in-run escape hatch; the only fix is recreating the
agent. **Declarations must err generous.**

(The two survivors are why `MULTI_AGENT_SYSTEM.md` already states `tools` is
attention/accident scoping, not a security boundary: `subagent` spawns a
differently-scoped agent. Nothing here changes that.)

## 3. Item 1 — declare scope on the two reasoning templates

### 3.1 Which templates, and why not the other two

Declare on **`strategy-room`** and **`code-review`**. Leave `software-dev` and
`research-paper` undeclared.

Two independent reasons line up:

1. **Fit.** `software-dev`'s backend/frontend/QA and `research-paper`'s
   experimentalist genuinely need a broad build/run surface. Writing a
   *correct* dev tool list for them is guesswork, and §2 says guessing narrow is
   unrecoverable.
2. **Blast radius.** Template member ids are **global agent ids** —
   `provision_member` (`materialize.rs:365`) reuses an existing agent by id and
   skips `tools` entirely. A declared id is therefore pinned narrow *for every
   later use of that agent*, inside teams or out. The generic ids (`lead`,
   `backend`, `frontend`, `qa`, `pi`, `reviewer`, `analyst`, `writer`,
   `experimentalist`) all live in the two templates we are not touching; the
   declared set (`moderator`, `bull`, `bear`, `contrarian`, `lead-reviewer`,
   `security-reviewer`, `perf-reviewer`, `correctness-reviewer`,
   `style-reviewer`) is role-specific enough that collision with a
   general-purpose agent is unlikely.

### 3.2 The declarations

Tool names below are verified against the registry (`const NAME` in
`src/builtin_tools/team/*.rs`, `task_manage/*.rs`, `search.rs`, `web_fetch/`,
`ctx_search.rs`, `code_check.rs`, `file_ops/*.rs`).

`team_*` is **not** globbed. The family contains `team_disband`,
`team_member_remove`, `team_create`, `team_from_template` — a `team_*` glob would
hand a bull-case analyst the ability to disband its own team, which is the
opposite of accident scoping. The two team verbs the prompts contract are
enumerated instead. `task_*` *is* globbed: the whole family (`task_create`,
`task_list`, `task_update`, `task_wait`, `task_comment`, `task_exit_journal`,
`task_read_artifact`, `task_review`, `task_submit`) is in-register for a member.

**`strategy-room`**

```toml
# moderator (leader)
tools = ["task_*", "team_status", "team_delegate", "message_send",
         "search", "web_fetch", "file_read"]

# bull / bear / contrarian
tools = ["task_*", "team_status", "message_send",
         "search", "web_fetch", "file_read"]
```

`search` / `web_fetch` stay because the prompts demand grounded claims ("State
your three best supporting facts"). `file_read` stays for the case where the
decision under debate references local documents. Everything else — bash, code
execution, all file writes, desktop/browser/media/generation — is out.

**`code-review`**

```toml
# lead-reviewer (leader)
tools = ["task_*", "team_status", "team_delegate", "message_send",
         "file_read", "search", "ctx_search", "code_check", "bash"]

# security- / perf- / correctness- / style-reviewer
tools = ["task_*", "team_status", "message_send",
         "file_read", "search", "ctx_search", "code_check", "bash"]
```

`bash` is deliberately kept: reviewing a diff requires `git diff`, and no
read-only tool produces one. That means the "read-only reviewer" framing is not
enforced — bash can write. What the declaration actually buys is removing
`file_write` / `file_edit` / `file_ops` / `apply_patch` from arm's reach, which
targets the real failure mode (a reviewer "helpfully" fixing the code it was
asked to review) rather than pretending to be a sandbox. This is exactly the
attention/accident scoping the docs already describe.

### 3.3 Contract check (must hold, enforced by `validate_toolset`)

* Worker essentials `task_submit`, `message_send` — `task_*` + explicit
  `message_send`. ✓
* Leader essentials `task_create`, `task_review`, `task_read_artifact` —
  `task_*`; `team_delegate`, `team_status` — enumerated. ✓

A mistake here fails at `team_from_template` time, before any directory is
created (`materialize.rs:383`).

### 3.4 Honest signal when a declaration is ignored

`provision_member`'s reuse branch skips `tools` silently today. With built-ins
declaring scope, a user who already has an agent named `bull` gets a team whose
members do **not** match what the template says — invisibly.

Add a structured report, mirroring the `dropped[]` pattern from the
workflow-interop round:

* `MaterializedTeam.tools_ignored_for: Vec<String>` — member ids that declared
  `tools`/`tools_denied` **and** took the reuse branch. Empty in the common case.
* Projected onto `TeamFromTemplateOutput` with
  `#[serde(skip_serializing_if = "Vec::is_empty")]`, so existing output is
  byte-identical when nothing was ignored.
* A `tracing::info` at the reuse site naming the member and the fact.

Both declared `.toml` files also get a header comment stating that member ids are
global and the declaration travels with the agent.

### 3.5 Explicitly not doing

Not namespacing template member ids per team. That would change reuse semantics
for every existing team and is a larger, separate decision.

## 4. Item 2 — pin team runs to Work

### 4.1 Shape

New `src/teams/run_mode.rs`:

```rust
/// Every team-originated agent run executes in Work — the identity partition.
pub const TEAM_RUN_MODE: SessionMode = SessionMode::Work;

/// Stamp the pinned mode onto a team run's request metadata.
pub fn stamp(metadata: &mut HashMap<String, String>);
```

Exactly two call sites, which is the complete set of team run producers
(`grep "RunRequest {" src/teams/` returns these two and nothing else):

* `broadcast::member_run_metadata` — group chat fan-out.
* `dispatcher::runner`'s metadata block — task dispatch and workflow steps.

`resolve_turn_mode` treats a request-carried mode as highest precedence and
stamps it onto the session; from the second turn on `stored == requested` and the
write short-circuits. One extra session write per member session, once.

### 4.2 Why a constant and not a literal in two places

The same reason `member_provision` exists: two copies of a rule is two chances to
change one. One constant also gives the invariant a single place to be tested.

### 4.3 The test that actually holds the line

```rust
for name in WORKER_ESSENTIAL_TOOLS ∪ LEADER_ESSENTIAL_TOOLS {
    assert!(!TEAM_RUN_MODE.defers_tool(name));
}
```

The essentials come from `member_provision` — the same two constants
`validate_toolset` uses. This binds the declaration-side contract (what a
member's `tools` must admit) to the presentation-side partition (what the mode
may defer), so they cannot drift. It fails if someone sets `TEAM_RUN_MODE` to
`Chat`, and it fails if someone adds a team verb's family to
`CHAT_DEFER_FAMILIES` while team runs still ride the global default.

### 4.4 What this fixes and what it does not

Fixes: `[policies] mode = "chat"` no longer strands the team protocol.

Does not add: any team-level mode dial. Narrowing a team's tool surface is
item 1's per-member `tools`. A session mode is a *user session* knob and a member
run is not a user session — the Panel already hides `ModePicker` in team chat for
exactly that reason (`composer/mod.rs:1114`).

### 4.5 Noted, out of scope

The same Panel `Show when=team_id.is_none()` also hides `ExecTierPicker`, so team
member runs always take the global exec tier with no per-team override. Unlike
mode, exec tier is a real permission, so pinning it is the wrong answer and it
needs its own design. Backlog.

## 5. Verification

### 5.1 Unit / integration

* `run_mode`: `stamp` writes the key; the essentials-not-deferred assertion
  (§4.3).
* `broadcast::member_run_metadata` and the dispatcher metadata both carry
  `session_mode = "work"`.
* `materialize`: both declared templates pass `validate_toolset` for their
  contract (a table test over the built-in registry, so a future edit that
  strands a member fails in CI rather than at a user's `team_from_template`).
* `materialize`: reuse branch populates `tools_ignored_for`; the fresh-create
  branch leaves it empty.

### 5.2 Runtime QA on a real daemon

The previous QA round exercised `team_create`'s inline path only. These run the
**template** path.

1. `team_from_template(template='strategy-room', …)` → probe a member with
   `chat.send{agent_id:'bull', message:'列出你能调用的全部工具'}` → expect the
   declared list plus `get_tool_schema` and `subagent`, nothing else. Restart the
   daemon and re-probe, which exercises persisted `skills` → `from_resolved` →
   `tool_whitelist`.
   (QA note from last round: the Panel agent selector does not route messages —
   `chat.send{agent_id}` is required; and `tools.effective` reads a different
   `AgentDef` registry, so it is not a valid probe.)
2. Negative: drop a user template into `~/.aleph/teams/templates/` declaring
   `tools = ["search"]` on a worker → `team_from_template` must fail naming
   `task_submit, message_send`, and leave no directory under `~/.aleph/agents/`.
3. **Decisive test for item 2**: set `[policies] mode = "chat"`, start a group
   chat, and confirm the leader's tool list still contains `task_create` and
   `team_delegate`. Run it **before** the fix first — they should be absent —
   so the pin is proven against a reproduced failure rather than assumed.

## 6. Documentation

* `MULTI_AGENT_SYSTEM.md` — built-in template scope table; the global-id caveat;
  `tools_ignored_for`.
* `MODE_SYSTEM.md` — team runs pin Work and why (mode is a user-session knob;
  teams narrow via `tools`).
* `FEATURE_LOCATOR.md` — anchors for `src/teams/run_mode.rs`.
