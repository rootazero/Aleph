# Security System

> Shell execution safety, approval workflows, and per-channel tool permissions

---

## Overview

Aleph's security system provides:
- **Permission Model**: single-tier Gateway-token auth — loopback is operator zero-config; a remote presents the shared token (or is walled). Authorized == full local-equivalent authority (see below)
- **Tool Permission Enforcement**: Per-channel tool permissions via `ScopedToolService` — governs *what an agent may do*, orthogonal to *who may connect*
- **Exec Tier**: the one user-facing dial over tool permissions — `Ask` / `Auto` / `Full`, metadata-driven, fail-closed for unknown tools (see below)
- **Exec Approval**: Human-in-the-loop, keyed on the *action* (the actual command / arguments), not the tool name
- **Command Analysis**: Static analysis of command risk
- **Allowlist/Blocklist**: Fine-grained command control
- **Output Masking**: Sensitive data protection

**Location**: `src/gateway/security/` (crypto + vault: `crypto.rs`, `shared_token.rs`, `store/`, `token_readonly.rs`), `src/tools/scoped/`, `src/config/types/policies/exec_tier.rs`, `src/sandbox/exec_approval/`, `src/exec/`

---

## Permission Model (network boundary + Gateway token)

The trust boundary is the network boundary, gated by a single shared
**Gateway token** (see [Trust model](#auth-ux)). The Panel connects over plain
WS (same-origin HTTP) — **not** the channel pipeline — so a thin-shell App
reaching a LAN core behaves exactly like a browser opening the core's IP:

- **Loopback** (the local desktop App / same machine): always authorized as
  **operator**, no token (single-machine zero-config).
- **Remote** (LAN): must present a credential in the `connect` handshake.
  `connect::resolve_connect_auth` accepts three, in priority order — a
  **device token** (`aleph-dt-*`, long-lived, bound to one paired device), a
  **bootstrap ticket** (`aleph-bt-*`, single-use, minutes-long, exchanged
  in-handshake for a device token), or the legacy **shared Gateway token**
  (`aleph-<uuid>`, provisioned at boot by `SharedTokenManager`). Any valid
  credential grants the **same** operator authority as local — there is no
  Chat/Config sub-tier. Nothing valid ⇒ the connection stays behind a
  **login wall** (`connect` is the only method it may call).
- **Revocation**, three granularities, all effective immediately:
  - `gateway.token.rotate` — regenerates the shared token, revokes **every**
    paired device, and closes every remote socket (`TokenRotated`).
    ⚠️ It does **not** reach an outstanding bootstrap ticket: a ticket is not a
    remote yet, so "revoke all remotes" leaves it fully redeemable. Cut those
    with the verbs below.
  - `gateway.ticket.list` / `gateway.ticket.revoke {ticket_id}` — the
    **outstanding**, still-redeemable bootstrap tickets. Minting one has had two
    faces since P0 (the Panel's Security page and `aleph-server pair`);
    cancelling one had **none** until 2026-09-03, so a minted-and-forgotten code
    was a live credential nothing could reach. The listing addresses rows by a
    non-secret `ticket_id` and **never carries the code**; a redeemed, revoked
    or unknown code all fail with one indistinguishable refusal, so an
    unauthenticated exchanger learns nothing from which one it hit. Clients:
    `aleph-server pair --list` / `--revoke`, shipped in the same commit as
    the RPC (a capability with no client is not delivered). No Panel face yet.
  - `gateway.devices.revoke {device_id}` — one device: its live sessions are
    dropped to the login wall synchronously, then their sockets are closed
    (`DeviceRevoked`, WS 4001). `gateway.devices.list` is the inventory, with a
    live `connected` flag. Both are scoped to `device_type = 'panel'` and never
    touch cluster nodes.

Three ways to authorize a device, all equivalent to a browser login:

- **QR / link** — `Settings → Security → Pair a new device` mints a ticket and
  shows `http(s)://<ip>:<port>/?bt=<ticket>`. **The URL is resolved by the
  server** (`gateway.ticket.create` → `urls`, from
  `tls::discover_interface_ips`), not by the browser: a Panel building it from
  its own `window.location` emits `http://127.0.0.1:<port>/…` whenever the
  operator generates it from the local desktop App.
- **Typed pairing code** — the same ticket, read off the QR and typed into the
  Panel's authorize box. The only path when a phone cannot scan.
- **Shared token** — recovery / manual entry. It never expires and doubles as
  the secret vault's master key, so it must **never** ride a URL or QR; the
  ticket flow exists precisely to keep long-lived credentials out of browser
  history, `Referer` headers, and access logs.

Headless cores mint a ticket with `aleph-server pair` (opens the 0600
`security.db` directly, WAL — the daemon need not be running).
`aleph-server bootstrap-token` prints the shared token for recovery.

### Enforcement

- **Login wall** (`server::handler` + `handlers::connect::connect_authorized`):
  the WS dispatch refuses every method except `connect` to an unauthorized
  connection. The handshake computes the verdict — loopback, or a valid token
  via `SharedTokenManager::validate` — stamps `ConnectionState.caller_role`
  (`operator` / `guest`), and echoes `role` / `authorized` / `needs_token` so
  the Panel renders the wall or unlocks the full app.
- **Channel tool gate** (`tools/scoped/dispatch.rs` +
  `method_authz::tool_requires_operator`): governs **channels only**. The
  inbound router stamps each channel run's `caller_role` from its
  `ChannelPermissionLevel` (default `Chat` ⇒ `guest`), and the tool-dispatch
  chokepoint refuses self-config tools to a chat-tier channel (e.g. a default
  Telegram bot). An authorized Panel is always operator, so this gate is a
  no-op for it — it is not a Panel sub-tier.

  ⚠️ **OPEN RULING (raised 2026-09-03, deliberately NOT implemented
  unattended).** That vocabulary has two values and `channel_run_identity`
  takes only the channel id, so a `permission_level = "config"` channel stamps
  `caller_role = "operator"` for **every** admitted sender — including a paired
  principal whose `users` row says Member. It is also the one authority axis a
  demotion never reaches (`restamp_live_connections` rewrites WS
  `ConnectionState` rows only). The fix is to COMPOSE the two axes to the more
  restrictive rather than let the channel word stand alone, and the reason it
  was not done unattended is that it changes a **shipped opt-in** (pinned by
  `executor.rs`'s `config_tier_channel_maps_to_operator`) and would make a
  paired member MORE restricted than an unpaired stranger on the same channel.
  Recommendation on record, awaiting a user ruling: compose to most-restrictive
  for KNOWN principals (`Config`+`Member` ⇒ `member`) and leave `Chat` ⇒
  `guest` **byte-unchanged** — that clause is load-bearing, because
  `channel_permission_level_from_role` returns `None` for `"member"` and
  `turn_permissions.rs` back-derives the channel tier from this same string, so
  overwriting a Chat channel's `"guest"` would DELETE the Full→Auto clamp on
  the default surface. The unpaired-sender residual is a separate product
  decision.

### What was removed

The role-based guest / invitation permission *engine* was deleted in the
LAN-trust revert:

- **`PolicyEngine`** (`src/gateway/security/policy_engine.rs`) — the
  per-tool `Owner` / `Guest` / `Anonymous` permission checker. Gone.
- **`InvitationManager`** (`src/gateway/security/invitation_manager.rs`)
  and the `aleph guests invite / list / revoke` CLI — guest invitation
  lifecycle. Gone.

`src/gateway/security/` now holds only crypto + vault plumbing
(`crypto.rs`, `shared_token.rs`, `store/`, `token_readonly.rs`).

### What survives (as inert types)

The identity *types* still exist in the protocol crate
(`shared/protocol/src/auth.rs`): `Role` (`Owner` / `Guest` / `Anonymous`),
`GuestScope`, and `IdentityContext`. `SessionManager` still resolves a
session's `IdentityContext` from stored metadata
(`src/gateway/session_manager/ops/identity.rs`) — but with the invitation
path gone, **every session falls back to `SessionIdentityMeta::owner`**;
nothing creates `Guest` or `Anonymous` sessions any more. Those branches
are unreachable legacy code, kept only as the audit-snapshot shape until a
later cleanup folds them away.

### Where tool permissions actually live

Limiting *what an agent may do* (as opposed to *who may connect*) is the
job of the per-channel tool-permission layer, **`ScopedToolService`**
(`src/tools/scoped/`). It merges a `ToolPermissionsConfig` across three
tiers — global → agent → channel, most-restrictive wins — and does not
read `IdentityContext`. This is orthogonal to connection trust and is
unchanged by the LAN-trust revert.

#### The agent tier is a permission set, so who may select it is a permission (2026-08-10)

The middle tier above is **per agent**, which means choosing an agent chooses
a permission set. `agent_id` arrives on `chat.send` / `agent.run` from the
caller, and until 2026-08-10 nothing checked it: a caller denied a tool under
one agent could name another on the same request and inherit its allowances.
The run-start guard that sat there — `visibility::existing_session_is_visible`
— answers a different question ("is this SESSION mine") and structurally
cannot answer this one: naming a different agent mints a *brand-new* session
key, which that predicate admits by design.

`AgentDefinition.allowed_users` closes it, with the same "unset or empty means
everyone" convention as its neighbour `allowed_links` — so single-user installs
and every pre-existing config are unchanged. One rule
(`config::types::agent_admits_user`), one enforcement point
(`handlers::agent::build_run_request`, shared by all three run-start paths,
taking the agent config as a **required** parameter so a new surface cannot
opt out), and one twin on the delegation face (`sessions_send`, because "use an
agent you are allowed and have it delegate" is otherwise two legal steps that
add up to the same thing). Refusal is an honest `PERMISSION_DENIED` naming the
agent — `agents.list` already returns every agent to every authenticated
caller, so there is no existence secret to protect, and a puzzle is what pushes
an operator who forgot to list themselves into widening the setting.

`agent_update` writes that list, and is therefore operator-gated: a gate must
cover the verb that removes the gate. Its RPC twin `agents.update` — what the
Panel's per-agent access picker calls — is covered by the `agents.` prefix in
`method_admin.rs` (only `list` / `get` are carved out for members).

**A revocation binds on the refused caller's next turn**, with no restart
(2026-08-10). Both faces install the new list on the runtime registry through
one method, `AgentRegistry::set_allowed_users`, because that registry entry is
what the run-start guard reads; a face that wrote only `config.toml` would
report a revocation as done while the refused person kept running until the
next boot. The claim is earned, not assumed: `takes_effect` (tool) and
`allowed_users_applied_live` (RPC) say `Live` only when that write returned
`true`, the same downgrade rule as `config::live_apply::classify_verified`.
`name` / `description` / `model` deliberately stay restart-only.

`agent.resume` used to be exempt from this guard; since 2026-08-10 it is not.
It never reached `build_run_request`, so it never asked the agent question —
and it is member-open, which made it the one way to put an agent back to work
without being asked. The exemption stood on two legs and the load-bearing one
("a revocation needs a restart anyway") died the day the live write above
landed. The gate now runs inside `resume_named_session`, after the visibility
gate and before the coordinator, refusing with an honest `PERMISSION_DENIED`
that shares its wording with `BuildRunError::AgentForbidden`. See
`handlers/resume.rs` and [AGENT_IDENTITY.md](AGENT_IDENTITY.md) §6 ①.

Full model and threat analysis in [AGENT_IDENTITY.md](AGENT_IDENTITY.md); the
seam-level landmines are in `src/gateway/CLAUDE.md`.

Shell-command execution safety (risk analysis, approval, allowlist,
output masking) is a separate subsystem — see **Exec Kernel** below.

---

## Exec Tier (Plan / Ask / Auto / Full)

**Location**: `src/config/types/policies/exec_tier.rs` (rules + the single
precedence composition point), `src/tools/scoped/` (enforcement),
`src/gateway/execution_engine/turn_permissions.rs` (per-turn resolution).

The exec tier is the **one user-facing dial** over tool permissions. It is not a
second policy engine and not a second enforcement mechanism: it is a rule
consulted at the chokepoint every tool call already funnels through, whenever no
explicit `[policies.tool_permissions]` entry names the tool — with exactly one
exception, `Plan`'s refusal, which is a **floor above** that entry rather than a
rule beneath it (see the lattice below).

| Tier | What it asks about | Notes |
|------|--------------------|-------|
| `Plan` *(session only)* | nothing — it **refuses** | read-only planning. Every non-idempotent tool is denied outright, not carded: an approval per mutating call turns "hold everything until the plan is reviewed" into "review the plan one edit at a time". Three carve-outs, each circular without it: `ask_user`, `scratchpad` (the plan file **and** the handoff), `subagent` (a child runs on the parent's own gated service). See **Plan → build handoff** below |
| `Ask` | every mutating / side-effecting tool | read-only tools stay allowed, so the model can still investigate |
| `Auto` *(default)* | the irreversible tail only | `*_delete`, `vault_*`, `team_disband`, an MCP server's `destructiveHint`, and `file_ops` `delete` / `move` (argument-level) |
| `Full` | nothing *(the tier asks nothing — see the two floors below)* | the command-policy floor and each tool's own `requires_confirmation` declaration both survive |

`Plan` is strictest, so it composes through `most_restrictive` as the floor of
the lattice: the channel clamp and the non-operator ceiling both leave it alone.
It is **not** offerable as the install-wide `[policies] exec_tier` — an install
sitting at `plan` would have nothing to hand an approved plan back to, so
`config.update_tool_permissions` validates against `builtin_tiers()` (three) and
the composer pill offers `session_tiers()` (four).

### The lattice (who wins)

```
a tier verdict of Deny — today `Plan`     (rung 0: a floor, nothing outranks it)
        ↓  (the tier said Ask, or had nothing to say)
explicit [policies.tool_permissions] entry   (exact name > glob)
        ↓  (nothing named this tool)
configured `default`   TIGHTENED BY   the tier's verdict
        ↓  (restrictive_min — the tier can only raise, never widen)
tool-declared confirmation gate              (CONFIRMATION_REQUIRED_TOOLS + MCP destructiveHint)
        ↓  (read by check_confirmation_gate independently of the tier)
[sandbox.command_policy] hardline floor      (no tier can lower it — not even Full)
```

⚠️ **Rung 0 exists because a floor an entry can outrank is a default.** `Ask` /
`Auto` / `Full` only ever raise a tool to *ask*, so an operator who named the
tool has answered the very question the tier was about to pose — for those three
"more specific wins" is right, and they are byte-identical to before rung 0
existed. `Plan` poses no question: its promise, restated to the model every turn,
is that nothing runs. Under "more specific wins", one `"bash" = "allow"` written
months earlier for an unrelated reason would retract that promise silently, and
only on installs whose operator configured tool permissions at all — the ones
with the most to lose. Rung 0 is keyed on the **verdict** (`rule_for` answering
`Deny`), not on `ExecTier::Plan`: `rule_for`'s contract is that a tier yields at
most `Ask`, so a future refusing tier inherits the floor without a name table.
It fires only on the tier's *own* `Deny`, never on a composed one, so
`default = "deny"` is still a baseline an explicit `allow` outranks, and the
`Plan` carve-outs plus every idempotent tool fall straight through to rung 1 —
an operator's explicit `deny` on `ctx_search` still denies while planning.
This deliberately **inverts** the tier's shipped behaviour, whose test
`an_explicit_entry_still_outranks_the_plan_tier` asserted the opposite; the
three replacement tests in `exec_tier.rs` pin the scope of the inversion.

⚠️ **`Full` means "the tier gates nothing", not "nothing is gated."** The
second-from-bottom row is easy to miss because it is not part of
`effective_permission`'s lattice at all: `ScopedToolService::check_confirmation_gate`
consults `requires_confirmation(name)` **independently of the tier and of any
explicit `allow`**. So `vault_store` / `agent_delete` / `team_disband` /
`skill_install` and any MCP tool carrying `destructiveHint` still raise a card
under `Full` — and in an **unattended** run (goal / loop / cron continuation)
still auto-deny, because unattended is fail-closed. That is deliberate: these are
the operations whose blast radius does not shrink because an operator set a
permissive tier. The variant doc on `ExecTier::Full` and the model-facing
`approval_prompt_line` both used to say "nothing pauses for confirmation", which
was the same statement told three times and false in all three.

`effective_permission(permissions, tier, facts)` is the **only** place this
precedence exists. Both consumers — `ScopedToolService::permission_for` (the
loop) and the gateway slash-command fast path — call it, so the two surfaces
cannot drift apart. Consulting the tier *before* the default (the pre-2026-07-14
shape) inverted a `default = "deny"` install into ask-by-default for exactly the
tools the tier meant to guard.

### Plan → build handoff

A conversation at `Plan` is read-only until a **human** approves its plan, and
then it starts building **in the same turn** — the next tool call, not the next
message. Nothing in `src/harness/` learns that plan mode exists.

```
tier resolves to Plan          turn_permissions mints a PlanGate
        ↓                      (AtomicBool + the tier to restore to)
model explores, writes the     file_read / grep / find / file_ops list|search /
plan to `scratchpad`           ctx_search / search / web_fetch / memory_* /
                               ask_user / subagent
        ↓                      everything else: PermissionDenied(GateRule::PlanMode)
scratchpad(request_approval)   the persisted plan → clarification::ask → a person
        ↓  "approved"
PlanGate::release()            the ONE chokepoint now reads the restore tier
        ↓                      + the session's `exec_tier` override is cleared
next tool call builds
```

Four properties are load-bearing:

- **The model cannot let itself out.** Everything under `request_approval`
  refuses without a person: the action errors when no approval transport is
  wired, and `clarification::ask` errors on an unattended run and on any turn
  with no channel to deliver to. There is no branch that reaches `approved`
  without one having been picked off a menu.
- **One gate per turn, shared by children.** The `Arc<PlanGate>` rides on
  `TurnContext`, and a run builds its tool service twice — for itself and as
  the parent view handed to spawned sub-agents. Both hold the same gate, which
  is precisely why `subagent` is admissible while planning: a child runs on the
  parent's gated service, so one approval releases both, and a child spawned
  before the approval can never outrun it.
- **The restore tier is derived, not remembered.** `plan_restore_tier` re-runs
  the same resolution with the plan picks dropped, so both clamps still apply —
  a member cannot come out of a plan above the install's posture. The cost,
  stated plainly: entering plan mode *replaces* a per-conversation tier rather
  than suspending it.
- **Plan-created denials are named and still listed.** `GateRule::PlanMode`
  points the model at the two-call exit rather than at a
  `[policies.tool_permissions]` entry nobody wrote, and the tools stay in the
  model's list (a plan another agent could implement needs the vocabulary of
  what can be done). An operator's real `deny` is unaffected on both counts —
  `denied_only_by_plan` is a difference, not a name table.

An unattended run whose session sits at `Plan` **stalls read-only**: there is
nobody to approve, so it cannot build. That is the correct direction — the user
put this conversation into read-only planning and an autonomous continuation
must not act for them — but it is a dead end whose only exit is a person moving
the tier back.

### A `/btw` side question composes `Plan`, then revokes two of its named-tool carve-outs

A `/btw` side question (see
[FEATURE_LOCATOR §4.14](FEATURE_LOCATOR.md#414-btw-侧问派生的只读侧会话-side-questions--2026-08-20))
runs on a session derived from the conversation it was typed in, and it is
read-only whatever that conversation's own tier says. The ceiling is **composed,
not assigned**: `resolve_turn_permissions` mints `side_question` from the request
stamp (`execution_engine/turn_permissions.rs:291` — one minting site, never
re-derived downstream from the key's shape) and folds it in with
`ExecTier::most_restrictive(tier, ExecTier::Plan)` (`:303`) — the same rule the
non-operator ceiling composes through (`:183`), and the same only-ever-tighten
discipline the channel clamp keeps by hand (`clamp_tier_for_channel`,
`gateway/channel_policy.rs:24`, is a two-arm match, not a `most_restrictive` call). So it can
only tighten: a conversation already at `Plan` is byte-identical, and no tier, no
explicit `[policies.tool_permissions]` entry and no request-carried pick can
raise it, because `Plan`'s refusal is rung 0 rather than a default. It rides to
the tool layer on `TurnContext::side_question` (`tools/turn_context.rs:85`).

**Both `PLAN_REACHABLE_TOOLS` carve-outs are revoked** — a side question is
strictly narrower than planning — at
`ScopedToolService::permission_for`'s rung −1 (`tools/scoped/builder.rs:250`),
above the tier verdict, because neither is removable by configuration nor
resolvable by approving a plan, which are the two repairs everything beneath it
points at. The reasons differ and both are load-bearing:

- **`scratchpad`** writes the **main** session's execution list. A side question
  is by definition not the main turn's work; letting it edit that file is a write
  into the very conversation the side session exists to stay out of.
- **`subagent`** can spawn a child that **outlives the side session**, with no
  surface able to enumerate it — the side key is derived, so nothing on the
  user's screen names the session the child is parented to.

The revocation is read directly off the same `PLAN_REACHABLE_TOOLS` constant
`ExecTier::rule_for` reads for the carve-out itself, not restated as a second
list, so a third member added there is denied to side questions with no edit
here. `Plan`'s **third named-tool** carve-out is untouched: `ask_user` lives in
`HUMAN_CONTACT_TOOLS`, not in this constant, and asking the user a clarifying
question is as circular to deny here as it is while planning.

⚠️ **It cannot ride on the allow set.** The narrowing that reads most naturally
— drop `subagent` from `allowed` — is a **no-op for exactly `subagent`**:
`ScopedToolService::is_allowed` (`builder.rs:443`) returns `true`
unconditionally for the attached `SubagentTool`, because that tool is appended
to listings independently of `allowed` (it is not in the builtin registry) and
would otherwise vanish from the model's list on every real gateway path. The
exemption is correct and predates this; what matters is that a rule written to
narrow *through* that filter never runs for the member it was written for — and
it *does* run for every other tool, so a test written with any other tool is
green. Narrow where the verdict is actually made.

Two further consequences:

- **No `PlanGate` is minted** (`turn_permissions.rs:339`). The gate is the one
  thing that can move a turn's tier mid-run, and a side question's ceiling is not
  the conversation asking to plan — it is a bound on this single turn with
  nothing to hand back to when the turn ends. Withholding it makes the bound
  immovable by construction rather than by the fact that `scratchpad`, the only
  thing that flips a gate, happens to be revoked. ⚠️ Argued at the source and
  **not pinned by a test of its own** — restoring the gate for a side question
  would go unnoticed until something else made `scratchpad` reachable again.
- **The refusal names itself and then keeps its promise**
  (`a_side_question_refusal_names_itself_not_the_plan_handoff`,
  `a_side_question_admits_the_read_arm_of_a_multiplexer_and_refuses_the_rest`,
  `the_side_question_refusal_promises_reads_that_really_run`).
  `GateRule::SideQuestion` is checked **ahead of** `PlanMode` and `PolicyDeny`
  (`tools/scoped/gate_chain.rs:433`), because both of those name a repair that
  cannot work here (approve the plan; edit the policy). Its sentence tells the
  model the side question "can read and search" — so the per-call read
  re-admission in `dispatch.rs:238` is keyed on **`PlanMode | SideQuestion`**,
  the property rather than the reporting rule. Keyed on the rule name alone, a
  side question missed that arm entirely and `file_ops list/search/stats`,
  `doctor`, `note_schema read`, `a2a_agents list` and `inbox_read peek` — the
  exploration a side question is mostly made of — were refused by a sentence
  promising the opposite.
  `denied_only_by_plan` still does the scoping, and it does **not** consult rung
  −1, so the two revoked carve-outs stay refused.

### The rules read declared metadata, never the tool's name

`ToolFacts { name, idempotent, requires_approval }` is filled from the tool's own
`ToolDefinition`:

- `idempotent` ← `LoopTool::is_idempotent()` — the builtin pure-read allowlist
  (`tools/retry.rs::is_idempotent_builtin_name`, which delegates to the single
  `READ_ONLY_TOOLS` list in `tools/adapters/registry_adapter.rs` — read-only ⇒
  idempotent, one source for the concurrency claim, auto-retry, and this tier
  rule) or an MCP server's
  `readOnlyHint` / `idempotentHint` (`mcp/protocol.rs::is_idempotent`).
  Anything that declares nothing is `false`.
- `requires_approval` ← `ToolDefinitionMetadata` (an MCP server's
  `destructiveHint`).

Hence: **not idempotent = mutating**, and **an unknown tool is non-idempotent**,
so `Ask` is fail-closed for every tool Aleph has never heard of. A table of name
globs cannot do this — MCP tools register as `{server_id}__{tool}`
(`github__delete_repo`), and any glob table silently lets whole families through
the gate it claims to hold.

The one exception is argument-level: `file_ops` multiplexes `list` and `delete`
behind a single name, so `ExecTier::asks_for_arguments` reads the `operation`
field. That is a deterministic safety hard filter (explicitly permitted by R7),
not a judgement about intent.

### Where the tier comes from, per turn

`resolve_turn_permissions` resolves it once per run: **request > session >
global**.

- **request** — the Panel composer pill sends the tier *with* the first message
  (`chat.send`), so it governs the very turn it was armed for.
- **session** — persisted to `identity_meta.custom["exec_tier"]` via
  `sessions.patch` (the same carrier as `project_root`). `null` = follow global.
  `sessions.patch` validates the value against `ExecTier::from_id`, exactly as
  `chat.send` does.
- **global** — `[policies] exec_tier` in config, read live per turn (no restart).
- A **non-operator** caller is clamped after resolution — twice, and the two
  clamps cover different callers:
  - **the channel clamp** (`clamp_tier_for_channel`) — a chat-tier channel
    cannot run at `Full`. It keys on `caller_role`, mapping only `"guest"` and
    `"operator"`.
  - **the non-operator ceiling** (2026-08-08) — a caller whose role is not an
    operator spelling resolves to at most the global `[policies] exec_tier`.
    Strictly tighter is always allowed; looser never is.

  The ceiling exists because the channel clamp **never fired for `"member"`** —
  it returns `None` for every role it does not map — while this document and
  `resolve_exec_tier`'s own doc both named it as the thing bounding member tool
  execution. `chat.send { exec_tier: "full" }` is a plain per-request parameter
  any member can send, and it landed in the rung that outranks everything, so a
  member could turn the tier off entirely.

  It reuses the existing global dial rather than adding `member_max_exec_tier`:
  `[policies]` is a server-global section, so setting it to `Full` is already an
  install-wide statement that this axis gates nothing here. **Cost, stated:** an
  operator who raises the global tier for their own convenience raises every
  member's ceiling with it. "Full for me, Auto for everyone else" needs that
  second knob, which layers on top of this clamp cleanly.

  An unrecognized role string is ceilinged like a member. `None` (no role at
  all — loopback, CLI, cron) means local/internal and is trusted; a role STRING
  nobody recognizes means a caller we cannot place, and the two must not share
  an answer.

### The three bypasses that were closed (do not re-open them)

Every one of these was a surface that could execute a tool **without passing
through `ScopedToolService`**. When you add a new such surface, this is the
question to ask first.

1. **Slash-command fast path** (`execution_engine/slash_command.rs`). `/bash`,
   `/file_ops` etc. dispatched straight into `BuiltinToolRegistry` — no tier, no
   `tool_permissions`, no `requires_confirmation`, no operator gate — and the
   path is reachable **from channels**. Now `slash_gate_reason()` returns the
   existing `ExecutionError::Fallthrough` for any gated call, routing it into the
   fully-gated agent loop; ungated slash commands keep their deterministic fast
   path.
2. **`tools.invoke` RPC** (`gateway/handlers/tools_invoke.rs`). Its denylist
   (`security/dangerous_tools.rs`) named 7 tools that **do not exist in this
   repo** — it had been inert its entire life. It now names the real ones and
   also refuses confirmation-gated tools (`is_confirmation_gated`, reading
   `CONFIRMATION_REQUIRED_TOOLS`). *Known limitation*: argument-level asks (an
   `Auto`-tier `file_ops delete`) are still ungated on this direct-invoke
   surface — closing that needs an approval transport for the RPC.
3. **Background runs** (goal / loop continuations, cron, heartbeat, a2a). A
   continuation used to drop both `caller_role` and the channel's
   `tool_permissions` layer — a chat-tier Telegram session escalated itself to
   local-operator authority by continuing. `carry_policy_metadata` now forwards
   exactly those two keys (and deliberately *not* `channel_id` /
   `conversation_id`, which would make an unattended run's approvals look
   deliverable).

### Unattended = fail closed

A run with no human attached is stamped `UNATTENDED_KEY`
(`execution_engine/mod.rs`) at every headless producer: cron **when its approval
is not routable** (a job carrying both a source channel and a conversation has a
real `/approve` path — stamping it would auto-deny a working HITL flow),
heartbeat and a2a always. `continuation_metadata` inserts the marker **last**, so
an inherited key can never demote a continuation to attended. The flag also
rides `TurnContext.unattended`, so `session_send` stamps **wait-mode** children
of a headless parent too (`fire_and_forget || parent.unattended`) — previously
only fire-and-forget children were stamped and a headless parent's wait-mode
child hung the full 120 s per gated tool before refusing.

`ScopedToolService` then denies confirm-gated tools immediately instead of
publishing an approval card into the void and blocking for the 120s timeout. The
model is told the run is unattended.

Teams (dispatcher / broadcast) are deliberately **not** stamped: a member run's
approvals resolve to a Panel card, and the user who dispatched the team is the
operator watching it.


**But "the user who dispatched the team" is one person only until the room has
two (2026-08-25).** In a project room the person who *started* the team and the
person whose message triggered this member run are routinely different, and the
card used to go to the room's owner regardless. It now follows the **speaker**:
the same `author_user_id` the transcript labels the turn with, carried into the
approval request, so the card lands with the person whose action is waiting on
it. Two consequences worth stating: the room owner is no longer guaranteed to
see every card in their room (they see the ones their own turns raise, plus
whatever their operator tier grants them), and a turn with no author at all —
a channel-driven or scheduled run — still routes the old way, because there is
no speaker to route to. Falling back to the owner there is a wrong-but-honest
addressee, not a widened one: it is the same principal the run would have
reached before.

**And the wire it rides on was cut, for every card, since before any of this
(found by `qa/teamchat_rooms`, fixed 2026-08-25).** `TURN_ORIGINATOR` is scoped
**once per run tree** at `run_agent_loop`, while `TURN_CONTEXT` is re-scoped at
the tool chokepoint by `ScopedToolService::execute`. Between the two sits
`orchestrator::dispatch`'s harness `tokio::spawn`, which hand-rolled a list of
task-locals to re-establish — agent id, project root, scope, room author — and
that list never learned about the originator. So `TURN_CONTEXT` survived the
boundary and `TURN_ORIGINATOR` did not, and every
`ExecApprovalRecord::originator_user_id` raised from inside a run was `None`.
That silently disarmed **both** consumers: the channel button-callback gate in
`ManagerCallbackSink::handle_callback` ("only the human who asked may press the
button" — the group-chat bypass it exists to close) and the room narrowing in
`approval_addressable_by_caller`. Neither degraded loudly; both fell back to
the pre-existing "any paired user / any session owner" rule, which is what they
were written to replace. **A lost task-local and a run that never had one are
the same `None`**, which is why no in-process test could see it and a
two-process real-machine run could: `run_agent_loop` logged `Some(u-…)` and
`OperatorApprovalRequester` logged `None`, one spawn apart. Guarded at source by
`dispatch.rs::the_harness_spawn_reestablishes_the_run_tree_originator`.
**刻意不做 · session grants do NOT survive into an unattended continuation
(评估于 2026-08-07, 用户裁定)。** In `confirm_with_memory` the `if self.unattended`
auto-deny sits **before** the session-grant short-circuit, so an action a human
approved with "本会话批准" is refused again on the next `goal`/`loop` continuation
of the same `SessionKey` — even though the grant is fingerprinted on
(tool + normalized args) and lives in that session's own bucket. Moving the two
blocks would make "approve once, the loop stops asking" work, and was
considered.

It is **not** being changed. The order is the trust boundary, not an accident:
it is what keeps the evidence for executing something with nobody watching a
*present* decision rather than a remembered click from earlier in the session.
CLAUDE.md 判据清单 §0 states the rule this instantiates — 「按状态做的闸，`Err`
必须是拒绝不能是放行」. The cost is bounded and visible: the auto-deny carries an
actionable hint telling the model to call `goal(action='update',
status='blocked')`, and the decision is filed on both durable trails, so the run
reports and hands back rather than stalling silently. Do not "fix" this by
reordering; if the ask returns, the answer is to make the continuation
attended (give it a routable approval channel), not to widen the gate.

**The ruling is now pinned by a test (2026-08-11).** Until then it lived only in
prose here and in FEATURE_LOCATOR §5.3 — one two-block move away from being
undone by someone repairing "my session grant stopped applying", which is
precisely how the reorder gets proposed. `tools::scoped::tests::`
`a_session_grant_does_not_survive_into_an_unattended_run` grants the action on an
attended service, proves the grant *does* suppress the re-prompt while a human
is attached (so the test is about the flag and nothing else), then asserts the
same action on an `unattended` service is refused. A reorder turns that
`unwrap_err()` into a panic. The comment above the block in
`confirm_with_memory` names the ruling and the test.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                       Security System                            │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                    Exec Kernel                             │  │
│  │  ┌────────────┐  ┌────────────┐  ┌────────────┐          │  │
│  │  │  Command   │  │   Risk     │  │  Approval  │          │  │
│  │  │  Parser    │→ │ Analyzer   │→ │  Manager   │          │  │
│  │  └────────────┘  └────────────┘  └────────────┘          │  │
│  └──────────────────────────────────────────────────────────┘  │
│                           │                                      │
│                           ▼                                      │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                   Approval Flow                            │  │
│  │  ┌────────────┐  ┌────────────┐  ┌────────────┐          │  │
│  │  │  Allowlist │  │   Human    │  │   Auto     │          │  │
│  │  │  Check     │  │  Approval  │  │  Approve   │          │  │
│  │  └────────────┘  └────────────┘  └────────────┘          │  │
│  └──────────────────────────────────────────────────────────┘  │
│                           │                                      │
│                           ▼                                      │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                   Execution & Masking                      │  │
│  │  ┌────────────┐  ┌────────────┐  ┌────────────┐          │  │
│  │  │  Execute   │  │   Output   │  │   Audit    │          │  │
│  │  │  Command   │  │   Masker   │  │    Log     │          │  │
│  │  └────────────┘  └────────────┘  └────────────┘          │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Exec primitives (`src/exec/`)

There is **no** monolithic `ExecKernel` with a single `execute()` pipeline, and
no separate `RiskAnalyzer` / `Allowlist` subsystem — earlier revisions of this
doc described a design that was never built. Shell-command safety is enforced by
two real layers documented above:

- **`[sandbox.command_policy]`** — the catastrophic hardline floor
  (`src/sandbox/command_policy/`), applied before every sandboxed exec on every
  tier. This is the only *blocking* command classifier.
- **Exec tier + approval gate** — the metadata-driven tier
  (`config/types/policies/exec_tier.rs`) enforced at `src/tools/scoped/`, which
  raises the action-aware approval card via `src/sandbox/exec_approval/`.

> **The prompt gets exactly one approval voice, and it is the enforced one.**
> The tier the gate will apply is surfaced to the model as `Approval mode: …`
> (`ExecTier::approval_prompt_line`, rendered by `OperatingEnvelopeLayer` @1758 —
> **Dynamic**, because the tier is a per-turn pill; see
> [FEATURE_LOCATOR §2.3](FEATURE_LOCATOR.md#23-context-模式-context-mode--codex-风格)).
> `SecurityContext`'s paradigm-derived `ElevatedPolicy` note answers the **same
> question** and is **not** enforced anywhere, so it was split out into
> `SecurityContext::elevated_policy_note()` and now renders only when no tier was
> resolved. Before that split, a Telegram turn at the default `exec_tier = auto`
> was told both "Approval mode: auto — routine tool calls run without
> interruption" and "Elevated Operations: Require user approval before
> execution", three bullets apart, with the unenforced one last (recency wins).
> If you add another approval-shaped sentence, delete one of these two first.

`src/exec/` holds the supporting primitives these layers use — none of them
enforce on their own:

| Item | Location | Role |
|------|----------|------|
| `analyze_shell_command` → `CommandAnalysis` | `src/exec/parser.rs` | Parse a shell string into program/args/segments — used to *render* the approval card summary, not to gate. |
| `SecretMasker` | `src/exec/masker.rs` | Redact secrets in a string for display (approval summaries, logs). |
| `SecurityKernel::assess_custom` | `src/exec/kernel.rs` | Advisory-only custom-pattern layer over the user's `[security]` patterns; wired as a `SandboxBeforeHook`. Deliberately does **not** re-enforce the built-in floor. |
| `RiskLevel {Safe, Caution, Danger, Blocked}` | `src/exec/risk.rs` | The advisory scale `assess_custom` returns. It is **not** the fictional `{Low, Medium, High, Critical}` an older doc showed. |
| `ExecApprovalManager` | `src/exec/manager.rs` | Pending/resolve pairing for the approval gate (see below). |

---

## Command analysis (approval-summary rendering)

**Location**: `src/exec/parser.rs` (`analyze_shell_command → CommandAnalysis`)

`analyze_shell_command` splits a shell string into its executables and segments.
Its output feeds the **approval card summary** (so a human approving a `bash`
call sees the real command, not just the word `bash`) — it is a rendering aid,
not an enforcement gate. The catastrophic floor that actually refuses commands
is `sandbox::command_policy`, whose real hardline rules
(`command_policy/rules.rs::hardline_rules`) cover the never-legitimate shapes:
fork bomb, bare-root `rm -rf /` (in any operand position and in every spelling
POSIX resolves to the root — `//`, `/.`, `/..`, `/./`), `dd`/`mkfs`-family/
redirect-or-`tee` to a raw block device,
on macOS the `diskutil`/`asr` disk-destruction verbs, and on Windows a
drive/hive-root recursive delete, `format`, and the destruction chain (shadow
copies, backup catalog, boot recovery, raw disk).
A `powershell -EncodedCommand` payload is decoded before matching **and put
through the same normalisation pipeline as the text that carried it**, so
neither encoding a script nor obfuscating it inside the encoding removes it
from the floor's view — see
[SANDBOX.md](SANDBOX.md) § "command-policy hard-filter" for the normalisation
contract.

---

## Approval Manager

**Location**: `src/exec/manager.rs` (`ExecApprovalManager` — pending/resolve
pairing), `src/sandbox/exec_approval/` (the gate: `gate.rs`, `action.rs`,
`session_memory.rs`, `denial_ledger.rs`).

The gate is **action-aware**: it is asked to approve *this call*, not *this tool*.

```rust
// src/sandbox/exec_approval/action.rs
pub struct ApprovalAction {
    pub tool_name: String,
    pub summary: String,             // the ACTUAL call, redacted + capped
    pub cwd: Option<String>,
    pub analysis: Option<CommandAnalysis>,
    pub reason: String,
}

// src/sandbox/exec_approval/gate.rs
trait ApprovalRequester {
    async fn request_approval(&self, action: &ApprovalAction) -> ApprovalResponse;
}
pub struct ApprovalResponse {
    pub outcome: ApprovalOutcome,
    pub deny_reason: Option<String>, // the human's own words on a /deny <reason>
}
```

`summary` renders what will actually happen — the command for `bash` /
`code_exec` (fed through the real `exec::parser::analyze_shell_command`),
`operation=delete path=…` for `file_ops`, `k=v` otherwise — then passes through
`SecretMasker`, is newline-flattened, and is capped at 200 chars on a
`char_indices()` boundary. **A confirmation gate that hides what it is gating
trains the user to click Approve**, which converts the whole tier system into
theater; every surface (Panel card, plain-text channel prompt, cluster node over
reverse RPC) now shows the action.

### Grants key on the action, not the tool

`confirm_with_memory` computes `grant_fingerprint(tool, &args)` once — over the
**raw canonical arguments**, not the redacted summary (redaction collapses
distinct secrets to one placeholder, so a grant on one credential would cover
another) — and uses it for **both** the session memory and the denial ledger.

Consequence: "Allow for this session" authorizes **that exact call**. Under
`Ask`, `bash` re-prompts per distinct command instead of whitelisting arbitrary
argv after one approval (codex's `ApprovalStore` semantics). Noisier, deliberately.

The same fingerprint keys the persistent tier, so "Always allow" authorizes that
exact call and nothing else either — see *Standing grants* below.

### Approval decision

```rust
// src/exec/socket.rs
pub enum ApprovalDecisionType {
    AllowOnce,    // this call
    AllowSession, // this call, for the rest of the session (by fingerprint)
    AllowAlways,  // this call, until revoked, across restarts — ONLY if the card offered it
    Deny,
}
```

`AllowAlways` was a legacy wire value narrowed to `AllowSession` everywhere
until 2026-08-11; it is now the persistent tier of the grant store, and the
narrowing is **conditional on the card**: `clamped_for(&record.allowed_decisions)`
(see *Standing grants* below).

- **Timeout ⇒ refusal.** `DEFAULT_APPROVAL_TIMEOUT_MS` = 120s;
  `ApprovalOutcome::is_approved` excludes `Timeout`. A timeout is deliberately
  **not** written to the denial ledger — an expired card is not a decision.
  There is no fail-open path.
- **Orphans cannot hijack a card.** `PendingEntry::is_live()` (not expired ∧
  receiver not closed) filters both `resolve_for_session` and `list_pending`, so
  a cancelled run's zombie can no longer win the `/approve` FIFO or render.
- **Denial is terminal** for that call, and is returned to the model as an
  in-context instruction not to retry it, rewrite it, or achieve the same result
  by other means. Three **consecutive** denials trip the session pause in
  `denial_ledger.rs` — see *The denial breaker recovers* below.
- **A denial can carry the human's reason.** `/deny wrong directory, use /tmp`
  (channels) or `exec.approval.resolve {reason}` (RPC) stamps
  `ExecApprovalRecord.deny_reason`; the gate renders it verbatim in the
  model-facing error (`The user said: "…"`) so the model re-plans on the actual
  objection. Display-layer only — the ledger still keys on the fingerprint.

### The card names the rule that gated the call (2026-08-11)

`src/tools/scoped/gate_chain.rs` is the one enumeration of *why* a call stops.
`ScopedToolService::confirmation_rule` returns the first matching `GateRule`, and
its `reason()` renders the sentence that goes to the human card, the model's
refusal, and the ledger detail — from one source, so the two audiences cannot be
told different stories about the same gate.

| rule (`id`) | what fired | what the reader can change |
|---|---|---|
| `tool_declared` | `LoopTool::requires_confirmation` (`CONFIRMATION_REQUIRED_TOOLS` / MCP `destructiveHint`) | **nothing** — this survives every tier, `full` included, and an explicit `allow` |
| `destructive_arguments` | `ExecTier::asks_for_arguments` on *this call*'s args | name the tool exactly in `overrides` |
| `policy_ask` | an explicit `[policies.tool_permissions]` entry, quoted verbatim (exact name or the glob that matched) | edit that entry |
| `tier_raised` | nobody named the tool; the tier read its declared metadata | change the tier |
| `policy_deny` | the merged policy denies (refuses rather than asks); quotes the entry, or `default` | edit that entry |

Order is chosen by one criterion: **a reason must never mislead the reader about
what would change the outcome.** `tool_declared` therefore outranks a
same-tool `policy_ask`, because reporting the softer rule sends the operator to a
setting that cannot help them — the same class of claim as the three now-corrected
copies of "`Full` gates nothing". Two of the pairs are mutually exclusive by
construction (an exact entry stands the argument filter down), so the ordering
only actually arbitrates around the declared floor;
`gate_chain::tests::the_chain_and_the_chokepoint_never_disagree` pins that the
chain classifies exactly the calls `permission_for` gates — no gate without a
reason, no reason without a gate.

Before this, all three ask-arms produced the identical sentence "Tool `x`
requires your confirmation to run."

### Standing grants: two tiers, one store, and a way back (2026-08-11)

`src/sandbox/exec_approval/grants.rs` is the one store of "what did a human say
yes to, and for how long". Two scopes:

| scope | lives in | created by | ends |
|---|---|---|---|
| `Session` | memory, keyed by `SessionKey` | any card offering it | the process, or a revoke |
| `Always` | `~/.aleph/approval-grants.json` | **only** a card that offered the tier | a revoke |

Both key on the same action fingerprint, so a grant covers the call that was
shown and nothing else. Every grant also stores the **redacted summary the human
read**, plus who granted it and when: a revocation list of SHA-256 prefixes is
not a revocation list, and that field is the whole reason the store holds
records instead of a `HashSet<String>`.

**Who may create a persistent grant.** `exec::allowed_decisions::for_confirm_gate`
adds `AllowAlways` only when both hold:

1. the requesting turn is **operator-tier** (`TurnContext::caller_is_operator`,
   the same predicate the operator tool gate uses). A persistent grant is
   install-wide — the per-call sibling of a `[policies.tool_permissions]`
   `allow` entry — so a member creating one would silently authorize everybody
   else's identical call. On an operator *escalation* card the requester is by
   construction not operator-tier, so answering "always" there cannot
   permanently strip a gate a member has to pass; and
2. the gate is **not** the tool's own declared floor (`tool_declared`). That
   card's own sentence says nothing switches the gate off, including `full` and
   an explicit `allow`. A button that switched it off would make that sentence
   false at the moment it is read.

**The offer is enforced, not merely drawn.** The set rides the record
(`ExecApprovalRecord::allowed_decisions`) and `ExecApprovalManager::resolve*`
clamps the incoming decision against it. This matters because "no surface offers
the button" was never a control: `allow-always` has always been an accepted wire
value on `exec.approval.resolve`, and every renderer that hardcoded three
buttons was one RPC client away from irrelevant. A renderer may still draw
*fewer* buttons than the record allows — that direction is safe — which is why
`ApprovalDecisionType::to_outcome_within` takes the offered set and has no
argument-free variant: a surface that never offers a standing grant has to say
so, in code, at the point it maps the decision.

**One derivation, both directions.** A card that may not *create* a persistent
grant is also never *satisfied* by one (`GrantStore::granted_within`). Without
that clause an operator's "always" on their own card would silently retire the
operator-escalation card a MEMBER trips on the byte-identical call — the
escalation would stop being raised for everybody, from one click on an unrelated
surface, and the person who clicked was never asked that question. The same
clause covers a tool that starts declaring a confirmation floor after a grant
was taken (an MCP server adding `destructiveHint`).

And the gate narrows on the way in as well as on the way out: an
`ApprovalRequester` returns an `ApprovalOutcome` **directly**, so the manager's
clamp only covers the implementations that route through it — and that trait has
several (channel bridge, operator, cluster centre, guardian, fallback, and a
debug auto-approver). `confirm_with_memory` therefore records an
`ApprovedAlways` as a session grant, with a warning, when the card it raised did
not offer the tier: the gate does not trust the outcome it is handed.

**Unattended runs are unaffected.** The `if self.unattended` auto-deny in
`confirm_with_memory` still precedes *both* tiers. A persistent grant is wider
than a session grant, so the 2026-08-07 ruling applies a fortiori: executing
something with nobody watching rests on a present decision, and "remembered from
last month" is further from present than "remembered ten minutes ago", not
closer. Pinned by
`tools::scoped::tests::a_persistent_grant_does_not_survive_into_an_unattended_run`,
the twin of the session-grant guard.

**And there is a way back.** `exec.grants.list` / `exec.grant.revoke`
(`gateway/handlers/exec_grants.rs`, rendered under Settings → Policies) show
every standing grant with the summary that was approved, and take one back. Both
are scoped exactly like their `exec.approvals.*` siblings: an unrestricted
caller sees everything; a member sees only session grants for sessions already
visible to them, and no install-wide grant at all; a grant that is not
addressable is refused exactly as an unknown fingerprint is. Revocation only
ever narrows authority — the worst outcome of an over-permissive revoke is one
extra approval card — which is what makes the write verb safe to carve open to
members while the rest of the `exec.` family stays operator-only.

Before this, the subsystem was asymmetric in the direction that matters: the
*negative* half had an exit (the denial breaker below cools down and half-opens)
and the positive half had none. A user who clicked "allow for this session"
could neither see the grant nor take it back.

### The denial breaker recovers (2026-08-11)

`DenialLedger`'s session pause has the same three states as its in-repo twin,
the guardian judge's `GuardianBreaker`: `Closed` / `Open` / `HalfOpen`, with
`SESSION_PAUSE_COOLDOWN` = 300 s.

- The counter is **consecutive**, reset by `DenialLedger::record_approval`,
  called from both gates that can obtain a live yes (the tool confirm gate and
  the sandbox elevation gate — they share one session bucket).
- After the cooldown, one probe is let through. A refusal there re-opens
  immediately (no fresh countdown, and no re-firing of the one-shot
  tool-result-cache purge); an approval closes it.
- Per-intent stickiness is untouched: an approval of action A says nothing about
  the refusal of action B.
- **A blind retry does not spend the probe** (2026-08-11 follow-up). Stickiness
  is checked first, against a non-advancing read of the breaker, because a
  repeat of an already-refused intent reaches nobody — the next branch refuses
  it — and consuming the `Open` → `HalfOpen` transition on it left the breaker
  recovered on paper and unasked in fact. An agent looping on the action it was
  just refused could keep every cooldown to itself while the user saw nothing.
  While the pause is genuinely in force the reported reason is still
  `ThresholdExceeded` (the stronger, more actionable statement); only after the
  cooldown elapses does the per-intent refusal report itself.

It used to be neither. `total` only ever went up and nothing cleared it, so the
word "consecutive" — in the constant's doc, the module doc, and the product spec
all three quote — described something the code never did; and `paused` was set
once and never cleared by anything. A user who declined three *different*
suggestions across an hour of productive review permanently paused every confirm
gate in that conversation, including the operator gate a chat-tier device needs
to get anything authorized at all. The countermeasure built for a brute-force
loop fired hardest on the most attentive user, and the documented escape was to
widen `exec_tier` — a gate that pushes people onto the least safe setting has
inverted its own purpose. The bound on an actual attacker is unchanged: one
prompt per cooldown, and their refusal re-opens it.

### A refusal nobody made (2026-08-12)

`ApprovalOutcome` had one word for two facts. **Four** production paths answered
`Denied` without any card ever reaching a person:

| path | when |
|---|---|
| `ApprovalGate` with no requester wired | boot ordering, headless installs |
| `ChannelApprovalBridgeAdapter` with no route | cron / webhook / non-channel turns |
| `ChannelApprovalBridge` delivery `Some(false)` | **a Telegram send that failed** |
| `ChannelApprovalBridge` delivery `None` | a channel with no approval capability |

Every one of them was filed by `DenialLedger::record_denial` as
`DenialReason::UserRejected`, which means:

1. the intent became **sticky for the rest of the session** — the identical call
   was auto-refused from then on, never asked again;
2. it advanced the **brute-force breaker**, so three transient delivery failures
   paused every confirm gate in the conversation for five minutes (including the
   operator gate a chat-tier device needs); and
3. the model was told *"The user already declined this exact action this
   session"* — a sentence it relays to the person it is talking to, about a
   decision they never made.

The ledger already drew this exact line for `DenialReason::Timeout` ("a timeout
is not a decision"), and its doc says the rule lives at the ledger "so a third
caller cannot reintroduce it by forgetting" — but the ledger can only apply a
rule to reasons it can tell apart, and these arrived wearing a person's label.

Now: `ApprovalOutcome::Unavailable` is the fourth refusal outcome, mapped by the
one derivation `DenialReason::for_refusal` (shared by the tool confirm gate and
the sandbox elevation gate, which had each written the mapping out inline and
each written it the same wrong way — `Timeout => Timeout, _ => UserRejected`).
`record_denial` records **only** `DenialReason::is_a_human_decision`, and both
gates' model-facing sentences are built from `ConfirmDenial::lead` /
`reason_kind.is_a_human_decision()` instead of a hardcoded "the user did not
approve". Security posture is unchanged — `is_approved()` is false, everything
still fails closed. What changed is that a transport failure stops being
recorded as a person's refusal.

**The unattended tax now has both gates.** `ApprovalGate::request_approval_for_action`
— the chokepoint serving sandbox capability elevation *and* the failover route
escalation — refuses with `Unavailable` when `TurnContext.unattended`, instead of
parking for the full approval timeout **per escalation** and being refused at the
end of it anyway. The tool confirm gate has charged this since 2026-07-14; these
two gates have now been found divergent three times (the ledger key, the
approval that closes the breaker, and this), so the tax sits on the shared
chokepoint rather than at a third call site.

### One "no", one strike, and it reaches the cards beside it (2026-08-12)

Two halves of the same defect, in the concurrent-card case the `AllowSession`
cascade was built for (parallel subagents, a teams broadcast).

- **`Deny` now cascades** to every *live* pending record in the same session
  with the same `grant_key`, carrying the human's `/deny <reason>` with it. It
  did not, on the stated grounds that "a deny is about THIS call, not the
  action" — but `DenialLedger` two files over answers the same question the
  other way, and auto-refuses the *next* identical call with no card. So whether
  one "no" covered the identical call parked beside it was decided by a **race**
  between the sibling's ledger check and the human's click. `AllowOnce` still
  never cascades. Narrower than opencode, which rejects the whole session's
  pending queue: keying on the action means a cascade can never refuse something
  the user did not read.
- **The breaker counts intents, not cards.** Each dismissed sibling was another
  `record_denial` on the *same fingerprint*, so a three-way fan-out tripped the
  three-strike pause on a single refusal — the countermeasure for an agent
  *guessing* firing on a user answering a fan-out the system raised itself.
  `consecutive` now advances only on a fingerprint not already refused this
  session; `counts` still sees every card, and three **distinct** refusals still
  trip it. A blind retry never reaches that line at all (`is_blocked`
  short-circuits it), so nothing but double-counting is given up.

### The third floor: the write that retires the gates (2026-08-12)

`ExecTier::floor_asks_for_arguments` — a `self_config` write whose path
intersects `policies.tool_permissions` / `policies.exec_tier`, or a
`rollback_config` — now raises a card under **every** tier, `full` included.

`self_config_touches_the_gate` was added (round 11, §4.12) precisely because
"write the override, then act freely" is two legal steps whose composition
equals the gated one-step write. It was written as an `Auto`-only rule, and
`asks_for_arguments` opened with `if self != Auto { return false }` — leaving the
composition open on the tier where it costs least. The asymmetry that makes this
worse than it looks: **the tier is a per-request knob** (`chat.send{exec_tier}`,
one message) and **the config change is install-wide and permanent**. One
message at `full` bought a gate removal that outlived it, in every later session
at every later tier.

The rule is narrow (ordinary `self_config` work is untouched) and still stands
down for an explicitly-named `[policies.tool_permissions]` entry — that entry is
a decision a person wrote, and the write that creates one cards through this
very rule. Three copies of the sentence were updated in the same change: the
`ExecTier::Full` variant doc, `approval_prompt_line(Full)` (**the one the model
reads**, now "Three floors"), and the card's own `GateRule::GateRemoval::reason`.
Its card may not offer a persistent grant — `allowed_decisions::FLOOR_RULES`
replaced the single `rule_id != DECLARED_FLOOR_RULE` equality, pinned to
`GateRule::is_floor` from both ends by
`every_floor_variant_is_declared_a_floor_on_both_sides`.

> **2026-08-30 update, left as a pointer rather than a rewrite**: this section is
> a changelog entry for when the mechanism landed (round 11 / 2026-08-12) and
> stays historical — the two paths it names above were the complete list *then*.
> The list has since grown a third member, `policies.terminal` (see "内嵌终端"
> below); the two names above are no longer complete. `GATE_DECIDING_CONFIG_PATHS`
> (`config::types::policies::exec_tier`) is the current, authoritative list —
> read that, not the two names in the paragraph above.

### The chain names every gate, and the trail hears it (2026-08-12)

`GateRule` gained `OperatorRequired` and `HookRequested`: the two gates that
raise a card outside `confirmation_rule` and had their prose hand-written at
their call sites. And `GateRule::id` — which the chain's own module doc has
always described as "the stable token the ledger and the tests key on" — now
actually reaches the ledger: `ApprovalAction::gated_by` carries it to
`record_approval_decision`, which appends `[gate: <rule_id>]` to the signed
`detail`. (Appended to `detail` rather than given a field of its own: the
identity ledger's signed preimage is append-ordered, so a new optional field
would invalidate every existing chain — see AGENT_IDENTITY.md.) An auditor
reading an `ApprovalGranted` row can now tell a tier-raised card from a floor
the operator could not have removed.

---

### 内嵌终端（`pty.*`） {#embedded-terminal}

- **两面 operator-only**：RPC 面在 `method_admin::ADMIN_PREFIXES`，订阅面在 `event_scope::default_rules`。
- **cwd jail 只管起点**。终端内部的 `cd` 不受约束 —— 命令粒度的闸在交互式字节流上不可表达（`vim` 里的回车不是命令）。
  它买到的是**"每个终端的起点可枚举、可审计"，不是"终端不能离开工作区"**。别把它当成隔离来引用。
- **PTY 不经 `[sandbox.command_policy]` 也不经 exec tier**（`method_admin.rs` 的注释自陈 "strictly more dangerous"）。
  会话粒度的开关 `[policies.terminal] enabled` 是这一层唯一说得出口的谓词；关掉它会杀掉在飞的会话。
- **终端历史住在服务器上**（每会话 `scrollback_lines` 行，默认 1000），因此对诊断与审计面可见。
- **同一装机的所有 operator 共享 `["*"]` 作用域**，能互相看见并 attach 彼此的会话。这是单层信任模型的有意结果，不是疏漏。
- **打开这道闸本身不举卡就是一次绕闸**：`[policies.terminal] enabled` 在
  `config::types::policies::exec_tier::GATE_DECIDING_CONFIG_PATHS` 上，所以经 `self_config`
  写它在**每个执行档位**——包括 `Full`——都停下来问。这张卡不提供"始终允许"，原因不是它例外，
  恰恰相反：`GateRule::GateRemoval` **本来就在** `FLOOR_RULES` 上，`for_confirm_gate` 正是
  因为这条地板判定才收回持久授权按钮——同一套地板机制覆盖名单上的每个成员，`policies.terminal`
  不是特例（`is_floor()` 自己的注释警告过"地板"在这个子系统里有两层含义，此处说的是"这张卡不许
  提供持久授权"那一层，不要在这句话之外再造第三种读法）。它上榜的理由属于该名单的两类之一：写它
  不是**退掉**一张已有的卡，而是**交出**一个此前没有任何卡在看守的执行面（`pty.*` 本身不经
  command policy、不经 exec tier）——这正是"写配置，再自由行动"能把两个各自合法的步骤拼成的
  一步免卡动作，也是该常量存在的理由（名单里另一类成员的写法是退掉一张已有的卡，具体成员与理由
  见 `GATE_DECIDING_CONFIG_PATHS` 自己的 doc，不在此重复——那份名单会变，这句话不该跟着腐烂）。
  一条精确点名 `self_config` 的 `[policies.tool_permissions]` 条目仍能站下这张卡（那是人写的
  决定，创建它的那次写入自己已经经过这条规则举过卡），这是有意的。
- **第三张脸：`terminal` 工具（2026-09-04 增量，仍然只读）**。同一份数据现在有三个入口——`pty.*` RPC、
  `pty.*`/`runtime.*` 事件、以及给 LLM 的 `terminal` 工具——所以**三张脸各关一次**（判据 §9）。
  工具面五个动词全部只读：`list`（有哪些会话）· `read`（当前可见屏幕，无 scrollback）· `status`
  （每个会话被检测出的 agent 状态）· `wait`（阻塞到某个状态出现，靠 `RuntimeAgents` 的变更 watch 唤醒，
  **不轮询屏幕、不碰会话**）· `explain`（哪条 manifest 规则、在哪个 manifest 版本、基于哪段屏幕文本判的）。
  **没有写动词**：PTY 不经 `[sandbox.command_policy]` 也不经 exec tier，把写入给 LLM 就是给它一个绕过全部
  命令闸的 shell——那是授权架构的决定，不是一个功能（第 2 期 spec §7.1 记着「若裁定要做」的形状）。
  闸：`method_authz::OPERATOR_TOOLS` 的成员 **＋** `builtin_tools/terminal.rs` 里的内联复查，两道。
- **零身份的调用方看不见任何有归属的会话（有意的 fail-closed）**。归属过滤在工具面只有**一个**谓词
  （`builtin_tools/terminal.rs::terminal_admits`，`list` 直接用它、`read`/`wait`/`explain` 经
  `owner_record_admits` + `PtyManager::owner_of` 用它），所以五张镜头不可能对「哪些行你能看」给出不同答案。
  对**有身份**的调用方它逐字节等于 `pty::owner_admits`；对 `actor == None` 它**刻意更窄**——
  `owner_admits` 那条臂答"不受限"，这里只放行 `created_by == None` 的会话。
  ⚠️ 后果要说清：**生产上每一次 spawn 都盖 actor**（loopback 的 operator 在 `connect` 就解析成
  `OWNER_USER_ID`），所以一个真的没有调用者的运行（cron / A2A / 内部接线）**一个会话都看不见**。
  这是**有意的**：一次没有身份的自动运行不该继承 operator 的终端。要重新打开它，开口在**身份系统**
  （让那次运行带上身份），不在这个工具——把这条臂放宽是一行策略改动，但那一行会把"未知"读成"全体"。
  不属于你的会话一律答 `no such session`，与"不存在"逐字节同形（否则每个动词都成了枚举别人 session id
  的 oracle）。守卫 `an_actorless_caller_sees_only_unowned_sessions` ·
  `a_loopback_operator_is_not_an_actor_less_caller` · `read_of_someone_elses_session_is_refused_like_unknown`。
- **⚠️ 一个记录在案、尚未修的闸缺口**（不是本轮引入）：在 `ScopedToolService` 路径上，chat 档位 / member
  调用方会触发 `OPERATOR_TOOLS` 的审批卡，**而即使人批准了，内联复查仍然拒绝**——批准只翻转派发管线的
  `authorized`，没有任何东西重盖 `TurnContext`。决定的修法是一条**接缝**（把 `check_operator_gate` 已经算出的
  `approved_by_operator_gate` 传下来），那条缝还不存在。**别用「删掉内联检查」来修**：审批卡的文案今天写着
  "…which changes Aleph's own configuration"，对一个只读工具是假的，删掉内联检查等于让一张贴错标签的卡
  真的授出一次读别人终端屏幕的权限。全文在 `builtin_tools/terminal.rs` 的模块 doc 与
  `method_authz.rs` 的 `terminal` 条目 → [TERMINAL_RUNTIME.md](TERMINAL_RUNTIME.md) §4

---

## Command allowlist

There is **no** `src/exec/allowlist.rs` and no `exec.allowlist` / `exec.blocklist`
config table — an older revision of this doc invented one. There is a **grant
registry** (`~/.aleph/approval-grants.json`, see *Standing grants* above), and it
is deliberately not the same thing: it holds per-CALL exceptions a human created
by answering a card, keyed on `(tool, canonical arguments)`, never a command
glob or a pattern language. Nothing writes it but a human answering an approval,
and it can only ever exempt calls that were shown to somebody.

"Which commands run" is otherwise decided by the two real layers already
described:

- What is **always refused**: the `[sandbox.command_policy]` hardline floor plus
  any user `deny` in `[policies.tool_permissions]` (an explicit entry beats the
  tier).
- What runs **without asking**: read-only tools under the exec tier (via declared
  `idempotent` metadata), plus anything the tier's verdict allows for the current
  `Ask` / `Auto` / `Full` setting. There is no per-command glob allowlist with
  `autoApprove` flags.

The only command-shape allowlist in the tree is `READ_ONLY_TOOLS`
(`src/tools/adapters/registry_adapter.rs`, consumed via
`tools/retry.rs::is_idempotent_builtin_name`), the pure-read builtins the tier
treats as safe.

---

## Output masking

**Location**: `src/exec/masker.rs` (`SecretMasker` — for displayed strings),
`src/sandbox/scrub.rs` (`scrub_and_gate_output` — for sandbox command output).

Two distinct paths, by consumer:

- **`SecretMasker`** redacts secrets in strings shown to a human or written to
  logs (e.g. the approval-card summary — `ApprovalAction` runs its command
  summary through it).
- **`sandbox::scrub::scrub_and_gate_output`** is the single source of truth for
  a finished command's stdout/stderr: it redacts secrets at the byte level,
  strips invisible/bidi control characters, and returns a **block-class** verdict.
  A block-class hit (a PEM private key — `leak_detector::BLOCK_CLASS_SECRETS`)
  makes the sandbox fail closed rather than return the surrounding context. Both
  `WorkspaceSandbox` and `WorktreeSandbox` route their output through it, so the
  floor cannot diverge between execution paths.

The secret pattern catalogs live in `src/secrets/leak_detector.rs` and
`src/exec/secret_patterns.rs`; they are kept in sync (the private-key regex is
`-----BEGIN[A-Z ]*PRIVATE KEY-----` in both, so bare PKCS#8 headers cannot slip
one catalog but not the other).

### The guardrail chain settles on "was it rewritten", not on "did anyone warn" (2026-08-28)

**Location**: `src/guardrails/registry.rs` — `evaluate_input` / `evaluate_output`
/ `evaluate_tool_call`, all three settling through `settle_chain`.

A guardrail chain runs each registered guardrail in order, feeding the next one
the **rewritten** payload, so a `Sanitize` followed by a `Block` cannot leak.
Only `Block` short-circuits. `Warn` reasons accumulate (deduplicated). What the
chain returns is then decided in exactly one place, and the predicate is
**`rewritten`** — did any guardrail ask for a rewrite — not `warns.is_empty()`.

The distinction is the whole contract. `warns.is_empty()` was the predicate for
one commit, and under it a guardrail that rewrote the payload without *also*
warning returned `Allow` and its rewrite was discarded: the original text went
to the provider. That is not an edge case — it is the shape **every redaction
guardrail has**. A PII scrubber does not warn; warning about a secret is one way
to write it into the audit log. So for the class of guardrail this layer exists
to serve, sanitization was a complete no-op, with no error and no log line, and
the only observable difference was the secret on the wire.

Two consequences follow, and both are load-bearing:

- **A rewrite that does not parse back as JSON is surfaced, not swallowed.**
  `evaluate_tool_call` must rebuild a `Value` from the guardrail's replacement
  text. When that fails there is no payload left to continue the chain with, so
  the chain ends there and returns the unparseable `Sanitize` **verbatim**.
  `AgentHarness::apply_tool_call_guardrail` then fails its own reparse and
  blocks the call — which is what its fail-closed arm is for. Downgrading the
  failure to a warn and keeping the previous args instead hands the tool the
  guardrail's **original, un-sanitized** arguments, and does so by making the
  harness's fail-closed arm unreachable.
- **The `source` string may not claim a warn that did not fire.** It is audit
  copy: `guardrails (sanitized)` / `guardrails (sanitized; warn: …)` /
  `guardrails (warn: …)`, chosen from the two facts rather than formatted from
  one of them.

The regression test is `all_three_chains_surface_a_warn_free_rewrite`, and it
asserts the three surfaces **agree** rather than asserting any one surface's
answer: the failure mode here was divergence — the tool-call twin had the
correct predicate while its two siblings did not, twenty lines apart in the same
file. `a_warn_only_chain_still_carries_its_reasons_unchanged` is the floor in
the other direction, so "stop dropping the rewrite" cannot be over-corrected
into "`Allow` unless rewritten", which would discard every warn reason instead.

### Asking a human FOR a secret is a third path, and masking is not its answer

`ask_user` questions carry a `secret` flag (codex `isSecret`). It is **not** a
display property. A reply typed into Telegram / Slack / iMessage is a permanent
message in a third party's datastore, and nothing Aleph does afterwards reaches
it — masking our copy would protect the one place that was never the exposure.

So the flag changes **transport**, not rendering
(`src/clarification/ask.rs`):

- when the turn's channel is a **registered** `ChannelRegistry` transport, the
  secret questions are **withheld** — dropped from the request before it is
  registered, so they never reach the renderer, the channel, or the pending
  registry. Their ids come back on `AskUserOutput.withheld` with an actionable
  reason (have the user set the value through the Panel or configuration, and
  do not re-ask here);
- otherwise — the Panel's `gui:chat` pseudo-channel, which is never registered
  — the question goes **only** onto the gateway event bus, where the Panel and
  TUI render a masked input.

The rule is per **question**, not per call. It began as a per-call refusal,
which threw away the three ordinary questions that happened to travel with a
fourth asking for a token: the human never saw them, and the model got an error
instead of three answers. A call whose questions are *all* secret still fails
outright — withholding every question would park the tool on a request no reply
can complete.

Two consequences worth stating rather than discovering: the model still
receives the answer (it asked for a value it needs, and withholding it would
make the tool pointless), and the answer still lands in the session event log
like any other tool result. The property bought here is precisely one — **a
credential never travels over a third-party channel** — and it is structural:
it follows from the shape of the delivery ladder, not from a caller remembering
to check.

**That covers the return leg too**, which is the half worth checking rather
than assuming — this codebase has been bitten before by a value that was masked
on one leg and shipped in clear on another (`RedactingEmitter` wrapped the
trace sink while the same run's text still went out through
`OriginFanoutEmitter`). Here the two directions are governed by the *same*
predicate: a secret question is only ever registered when
`ChannelRegistry::get(turn.channel_id)` is `None`, so on any turn that could
carry the answer outward there is no answer to carry. And the fanout mirrors
only `RunComplete.final_response` — model-authored prose — never tool results.
Pinned by `clarification::ask::tests::a_secret_among_others_withholds_only_itself`,
which asserts that nothing marked `secret` reaches the pending registry on a
turn with a registered channel.

---

## Permission System

There is no `src/permission/` module and there never was one. Limiting *what an
agent may do* is exactly three things, and they compose in this order:

1. **`[policies.tool_permissions]`** — per-tool `allow` / `ask` / `deny`, exact
   name or glob, merged global → agent → channel, most-restrictive wins
   (`src/config/types/policies/tool_permissions.rs`).
2. **The exec tier** — `Ask` / `Auto` / `Full`, which tightens the configured
   `default` for tools nobody named (see [Exec Tier](#exec-tier-ask--auto--full)).
3. **`[sandbox.command_policy]`** — the hardline command floor, which no tier and
   no permission entry can lower (`src/sandbox/command_policy/`).

All three are enforced at one chokepoint, `src/tools/scoped/` — `Deny` hides the
tool from the model *and* refuses the call; `Ask` routes to the approval gate.

The action-type approval engine (`src/approval/`) is a separate, older axis over
*capability domains* (desktop / browser / system / pim / automation), not tool
names; its `Ask` hands `approval_required` back to the LLM rather than running a
deterministic HITL gate (R7/R9).

---

## Audit Logging

**The live trail is the session event log.** Every approval decision at the
enforcement chokepoint is recorded by
`tools/scoped/dispatch.rs::record_approval_decision`, which writes
`ToolCallApproved` / `ToolCallDenied` session events. Query it through the
session service, alongside every other event of the run that produced it.

**Deleted (2026-07-14), do not go looking for it**: `src/exec/approval/storage.rs`
+ `audit.rs` and the `aleph-server audit` CLI command. They queried three SQLite
tables (`~/.aleph/approval_audit.db`) whose **only writers were test helpers** —
an operator running `audit` got zeros and concluded nothing had happened, while
the real trail sat in the session event log. That is worse than dead code.

Deleted in the same sweep, all zero-consumer:
`src/exec/approval/{escalation,binding,path_canonicalize}.rs` (path-escalation,
binding-compliance and sensitive-directory checks that nothing ever called),
`benches/approval_performance.rs`, and
`exec/allowed_decisions.rs::{decisions_for_risk, assess_command_decisions,
risk_segments}`.

Security *events* (not approvals) still log through
`src/security/audit.rs`; SSRF has its own trail (see below).

### Reading the security trail — `aleph audit` (2026-08-21)

`security_audit_log` had **five producers and no reader**. `AuthFailure`,
`RateLimited`, `CommandPolicy`, `ScopedContentRead` and `AuthorityChange` were
all persisted, indexed by timestamp and event type, and purged behind a 30-day
horizon — and nothing in the repository ever ran a `SELECT` against them outside
the drain task's own tests. `AuthorityChange` was added (round-5) specifically so
that *what authority changed, in order* is answerable with one `WHERE` clause,
and there was no surface from which that clause could be run.

- **Store**: `SecurityStore::query_audit_entries` — SQL-side filters (event type,
  actor, relative window), newest first with `id` breaking same-second ties, and
  one row over-fetched so a full page can be distinguished from a truncated one.
- **Wire**: `security.audit.query`, contract in
  `aleph_protocol::audit`. Admin-gated by the new `security.` prefix in
  `method_admin.rs` (prefix, not method, so a future `security.audit.export`
  arrives gated). Census entry pins the ruling.
- **Client**: `aleph audit [--type] [--actor] [--since 90s|5m|2h|7d] [--limit]`.

Two honesty properties are carried on **every** response rather than left to the
reader: `retention_secs`, because an empty window otherwise means *nothing
happened* / *nothing matched* / *it was deleted and you cannot tell which*; and
`truncated`, because "the window is clean" and "I stopped counting" must not
render the same.

⚠️ **This is not the `aleph-server audit` deleted on 2026-07-14 coming back.**
That command queried `approval_audit.db`, three tables **whose only writers were
test helpers** — it reported zeros about a trail that never existed, while the
real approval trail sat in the session event log. This is the opposite defect:
a table with five real producers and no way to read it. The deletion rule that
removed the old command ("a reader with no writers is worse than dead code")
and the connection rule that added this one ("a writer with no reader is a
capability that was never delivered") are the same rule seen from its two ends.

### Signed agent ledger (2026-07-25, hardened 2026-07-26 … 2026-08-10)

The session event log answers *what the run did*. It does not answer *who,
provably*: its only actor identity is the `agent_id` inside the session key, and
nothing stops a stored row from being edited afterwards.

`src/identity/` adds the second half. Every agent holds an Ed25519 keypair
(public half + fingerprint in `security.db`, private half in the existing
encrypted vault), and every **mutating** tool call, every refusal and every
approval decision is appended to that agent's own hash-chained, **signed**
chain — from the same chokepoint, `tools::scoped::dispatch::execute_inner`, that
already enforces every gate. `record_approval_decision` writes both trails: the
ledger first, because it needs only the turn's agent identity and therefore
covers surfaces the ambient `CallIdentity` cannot reach.

This is the first production consumer of `gateway/security/crypto.rs`'s Ed25519
helpers, which had none (the `devices.public_key BLOB NOT NULL` column every
writer fills with empty bytes is the visible half of that).

**Each record answers "who" twice (2026-08-10).** `agent_id` is the identity
that acted; `principal` is the person who drove it (`users.user_id`). Recording
only the first meant non-repudiation held for agents and not for people: on a
multi-user install, agent `main`'s chain proved a command ran and was silent on
whether the operator or a member asked for it. `principal` is resolved at the
same chokepoint from `visibility::ambient_actor()` — never from tool arguments,
the same provenance fence `agent_id` has — and is `None` only where there
genuinely is no person (cron, heartbeat, a continuation, an A2A delegation, a
chain's own opening record). It is **inside the preimage**, so an adversary with
database write access can neither blame someone else nor erase the attribution:
both edits move the digest the signature covers. Chains written before the
column keep verifying — the field is committed last and a `None` emits no bytes
at all, so a principal-less row hashes byte-for-byte as it did before.

**Read it with `agent_identity` (tool, operator-gated) or `aleph-server
identity` (CLI, read-only, no runtime and no instance lock — verification must
not have to ask the process that wrote the records).** Shipping the reader in
the same change is deliberate: the deleted store above is what happens when it
is not.

**A delegated sub-agent is a principal here, not a line on its parent's chain.**
It holds its own key and signs its own work. That is not free: a subagent runs
on its *parent's* `ScopedToolService`, inherits the parent's `TURN_CONTEXT`, and
`SessionKey::Subagent::agent_id()` resolves to the parent — all three roads led
to the spawner. The acting role is injected by the one layer that knows it
(`AllowlistToolService`, which the spawner builds from the child's `AgentDef`),
and it has to be injected *there* because the harness spawns a task per tool
call. Any future delegation or isolation path that does not go through that
wrapper must open its own `identity::as_actor` scope, inside the spawn, or its
work will be signed by whoever started it.

**Key lifecycle is inside the chain, not beside it.** `agent_keys.retired_at`
and `agent_identities.revoked_at` are ordinary mutable columns; on their own
they mean an attacker with database write access can un-revoke an identity or
erase a rotation and still get a clean `verify`. Every chain therefore opens
with a signed `identity_created`, and each rotation and revocation appends a
signed record to the affected agent's own chain — the revocation signed by the
key it retires.

**What a clean `verify` proves**: no stored record was edited, reordered,
transplanted between agents, prefix-deleted, tail-truncated or forged without
**that** agent's private key — "that" being enforced, not assumed: a row naming
a key this installation minted for a *different* agent is a `ForeignSigner`
fault even when its signature is arithmetically valid. Without that check the
guarantee would only ever have been "some agent's private key", and every
delegated role that now holds a key enlarges that set. **What it does not
prove**: anything against an adversary holding `~/.aleph` (vault, master key and
database share a disk); anything about in-process impersonation (`agent_id` is
still a caller-supplied string on `chat.send` — see AGENT_IDENTITY.md §6); and
anything about records that were never written, which is why `failed_appends` is
returned next to every `ok` — and why that counter is **durable**, not
process-local: the offline verifier, the one surface you reach for when you do
not trust the daemon, runs in a different process and would otherwise always
read zero.

Revocation is a mark, not an execution gate — nothing in this subsystem stops a
revoked agent from running, so its actions keep being recorded (under the
retired key). Refusing to sign would delete the evidence without preventing the
act.

Full model, threat analysis and the buzz gap-analysis table:
[AGENT_IDENTITY.md](AGENT_IDENTITY.md).

---

## Spend Ceilings (per-principal USD budget · 2026-08-22)

**Location**: `src/spend/` (`Principal`/`Spent`/`Delta`/`Limit`/`Verdict`, the
`SpendLedger` trait, the two process-global handles, the two principal
resolvers, `check`/`check_with` — the single admission predicate),
`src/spend/sqlite.rs` (durable backend on the `SecurityStore`),
`src/spend/period.rs` (local-calendar period boundaries + retention),
`src/config/types/policies/spend.rs` (`[policies.spend]`),
`src/providers/metering.rs` (the floor arm),
`src/gateway/execution_engine/run_loop/` (the admission arm, `deny_if_over_spend`),
`src/gateway/handlers/spend.rs` (`spend.query`), `interfaces/cli` (`aleph spend`).
Boot wiring — `src/bin/aleph-server/commands/start/mod.rs`, right after
`initialize_vault`.

`[policies.spend]` sets a per-principal ceiling (`per_user_usd`) and/or a
machine-total ceiling (`total_usd`) over a rolling `Day` or `Month`
local-calendar period. Two arms read the same predicate:

- **The floor arm** — `MeteringProvider::enforce_spend_ceiling`, the first
  statement inside every provider call's boxed future, before it is
  awaited. A principal already over the ceiling gets no further LLM calls.
- **The admission arm** — `run_loop::deny_if_over_spend`, checked when a run
  is first admitted, so an over-budget principal cannot even start a new
  run.

### It is a ceiling, not a hard cap

The floor arm checks *before* the call and records spend *after* it
completes — by design. Reserving an estimate up front would need a refund
on every error path, and a single missed refund would silently shrink
someone's ceiling forever; a bounded overshoot is the cheaper failure to
accept. The bound is exact: a principal can exceed the ceiling by at most
the cost of whatever calls were already in flight the instant they crossed
it. What this control promises is "no *new* call is admitted once you are
over budget, and the ledger reflects reality within one in-flight call's
worth of drift" — not "spend will never cross the configured number".

### A missing price is never a gate

`pricing::estimate` can answer `CostStatus::Unknown` for a provider/model
pair the static price table does not carry. That maps to
`spend::Delta::Unpriced`, a deliberately fieldless variant with no `usd`
field to attach a figure to — an unpriced call is *counted* (so an operator
can see how much traffic the ledger cannot price) but never denies anything
and never contributes to either ceiling. A source-level guard beside the
one production site that performs this mapping
(`providers::metering::MeteringProvider::record_spend_with`) pins that
`CostStatus::Unknown` can never reach `Delta::Usd`/`Delta::Partial`, so this
stays true even if `pricing::estimate` someday grows a heuristic guess for
unknown models. See `pricing.rs`'s module doc.

### Two coverage boundaries, stated rather than left for a reader to infer

- **Provider health probes are unmetered.** `providers::probe::probe_provider`
  calls `.process()` directly on the raw provider, bypassing the
  `MeteringProvider` wrapper entirely — a probe accumulates no spend and is
  denied by no ceiling. This is correct (a health check is not the user's
  spend), but it means "every model call is metered" is the wrong claim;
  the accurate one is "every call made on a principal's behalf is metered".
- **Unattributable spend has a name, and it is not a person.** Spend
  produced where no principal can be resolved
  (`principal_from_metadata`/`ambient_principal`'s fallback arm) lands on
  `Principal::Unattributed`, the `"@unattributed"` row `spend.query` reports
  alongside every real principal's row. It is the ledger's honest account
  of spend nobody could be billed to — not a user who is somehow both
  anonymous and over budget, and it is never itself subject to
  `per_user_usd` (there is no per-principal ceiling to check it against).

### What it does not buy

Exceeding a ceiling denies further calls; it does not retroactively undo
anything already billed. There is deliberately no `spend.reset`: zeroing a
ledger row is, after the fact, indistinguishable from a write that never
happened, so raising the ceiling in `[policies.spend]` (live-appliable, see
below) is the reversible way to say the same thing. `spend.query`/
`aleph spend` — admin-gated, same `spend.`-prefix reasoning
`security.audit.query` documents for its own prefix, since a spend report
names every principal on the machine and their dollar figures — are the
only read faces today; there is no Panel surface, matching the CLI-only
pattern `users.*`/`aleph audit` already established for admin-gated
controls.

### Retention

The ledger is swept once at boot, right after the durable ledger is
installed, down to `spend::period::RETENTION_PERIODS` (3 — current + 2
prior periods). This is a period *count*, not the 30-day *duration*
[`security::audit::DEFAULT_RETENTION_SECS`](#audit-logging) uses for the
audit trail — the two are not the same policy in a different unit, and the
ledger's own doc says so explicitly. The reason is structural: the ledger
is keyed by a calendar boundary (`period_start_ms`), not by an elapsed-time
cutoff, so a seconds-based horizon would still have to be converted into
"which period boundary does that land on" before it could sweep anything —
and that conversion is exactly where a fixed-duration off-by-one deletes
the *current* period instead of an old one. `retention_cutoff_ms` walks
calendar boundaries backward in period-sized steps instead.

### `[policies.spend]` is live-appliable, boot installation is not optional

`spend::install_policy`/`spend::install_ledger` install the two
process-global handles this whole control reads; both must run at boot
(`start/mod.rs`), unconditionally — including when no ceiling is
configured. `spend::update_policy` (the live-apply path for
`[policies.spend]`) returns `false`, and the live-apply verdict honestly
downgrades to `Restart`, when no handle has been installed yet; skipping
installation on an unconfigured box would mean an operator could turn a
ceiling *off* live but never turn one *on* live — the direction that
matters less. A source-level census in `start/mod.rs`'s own test module
(`boot_installs_the_spend_policy_and_the_spend_ledger`) pins that both calls
exist in production, because nothing else can see their absence: it is not
a compile error, no other test goes red, and a server missing either call
runs indistinguishably from a correctly-configured one with no ceiling set.

---

## IPC Security

**Location**: `src/exec/bridge.rs` + `src/exec/socket.rs`

Secure communication for approval requests:

```rust
pub struct ApprovalBridge {
    socket: UnixSocket,
}

impl ApprovalBridge {
    pub async fn request_approval(
        &self,
        cmd: &ParsedCommand,
        risk: &RiskAssessment,
    ) -> ApprovalDecision {
        // Send request to UI/CLI
        let request = ApprovalRequest {
            command: cmd.to_string(),
            risk_level: risk.level,
            reasons: risk.reasons.clone(),
        };

        self.socket.send(&request).await?;
        self.socket.recv().await
    }
}
```

---

## Security Best Practices

### For Developers

1. **Never bypass the one chokepoint** - Every tool that can execute must go
   through `src/tools/scoped/` (the exec-tier + approval gate); every sandboxed
   command through `WorkspaceSandbox` / `WorktreeSandbox` (the command-policy
   floor + output scrub). A new surface that skips either is a bypass.
2. **Validate inputs** - Sanitize all user-provided command arguments
3. **Read declared metadata, not names** - The gate keys on `ToolFacts`
   (`idempotent` / `requires_approval`), never a tool-name glob
4. **Log everything** - All security decisions should be audited
5. **Principle of least privilege** - Request minimal permissions

### For Users

1. **Review approval requests** - Read commands before approving
2. **Use session scope** - Avoid permanent approvals for risky commands
3. **Check audit logs** - Regularly review what commands were executed
4. **Update allowlists** - Keep allowlists minimal and current

---

## Configuration

```json5
{
  "security": {
    "exec": {
      "enabled": true,
      "defaultPolicy": "ask",  // ask, allow, deny
      "sessionApprovals": true,
      "auditLog": true
    },
    "permissions": {
      "defaultEffect": "deny",
      "rules": [
        {
          "resource": "file://~/.aleph/*",
          "action": "read",
          "effect": "allow"
        }
      ]
    },
    "masking": {
      "enabled": true,
      "patterns": ["apiKey", "password", "secret", "token"]
    }
  }
}
```

---

## SSRF Protection (Outbound Request Security)

> Server-Side Request Forgery defense: DNS pinning, redirect chain validation, IP classification, legacy literal blocking

**Location**: `src/security/ssrf/`

### Overview

Aleph's SSRF engine validates ALL outbound HTTP requests before they leave the server. It protects against:
- Accessing private networks (10.x, 172.16.x, 192.168.x) via user-provided URLs
- Cloud metadata endpoint attacks (169.254.169.254)
- DNS rebinding (TOCTOU) attacks
- Redirect chain pivoting (public URL → private redirect target)
- Legacy IPv4 literal bypass (octal, hex, decimal encoding)
- IPv6 embedded IPv4 bypass (NAT64, 6to4, Teredo)
- URL credential obfuscation (http://evil.com@127.0.0.1)

### Architecture

```
src/security/ssrf/
├── mod.rs        — Public API: validate_url_async, safe_fetch, SsrfError
├── policy.rs     — SsrfPolicy configuration struct
├── ip.rs         — IPv4/IPv6 classification against blocked ranges
├── hostname.rs   — Hostname blocklist, allowlist, legacy IP detection
├── dns.rs        — Async DNS resolution with address pinning
└── fetch.rs      — safe_fetch() with redirect chain validation
```

### SsrfPolicy

```rust
pub struct SsrfPolicy {
    pub enabled: bool,                    // Master switch (default: true)
    pub allow_private_network: bool,      // Allow private IPs (default: false)
    pub allowed_hosts: Vec<String>,       // Allowlist: ["*.corp.internal"]
    pub blocked_hosts: Vec<String>,       // Blocklist: ["*.malware.com"]
    pub max_redirects: u8,                // Redirect limit (default: 5)
    pub strip_auth_on_cross_origin: bool, // Strip Auth/Cookie on cross-origin redirects (default: true)
}
```

### IP Classification (`ip.rs`)

**Blocked IPv4 ranges:**

| Range | Purpose |
|-------|---------|
| `0.0.0.0/8` | Current network (unspecified) |
| `10.0.0.0/8` | RFC1918 private |
| `100.64.0.0/10` | Carrier-grade NAT |
| `127.0.0.0/8` | Loopback |
| `169.254.0.0/16` | Link-local (includes cloud metadata) |
| `172.16.0.0/12` | RFC1918 private |
| `192.0.2.0/24` | TEST-NET-1 |
| `192.168.0.0/16` | RFC1918 private |
| `198.18.0.0/15` | Benchmark testing |
| `198.51.100.0/24` | TEST-NET-2 |
| `203.0.113.0/24` | TEST-NET-3 |
| `224.0.0.0/4` | Multicast |
| `240.0.0.0/4` | Reserved + broadcast |

**Blocked IPv6 ranges:**

| Range | Purpose |
|-------|---------|
| `::1` | Loopback |
| `::` | Unspecified |
| `fe80::/10` | Link-local |
| `fc00::/7` | Unique local address |
| `ff00::/8` | Multicast |

**IPv6 embedded IPv4 extraction** — extracts inner IPv4 and validates:

| Format | Example | Extraction |
|--------|---------|------------|
| IPv4-mapped | `::ffff:127.0.0.1` | Direct mapping |
| NAT64 | `64:ff9b::7f00:1` | Last 32 bits |
| 6to4 | `2002:7f00:0001::` | Segments 1-2 |
| Teredo | `2001:0000::80ff:fffe` | Last 32 bits XOR `0xFFFFFFFF` |
| IPv4-compatible | `::127.0.0.1` | Last 32 bits |

### Hostname Blocking (`hostname.rs`)

**Hardcoded blocklist:**
- `localhost`, `localhost.localdomain`
- `metadata.google.internal`, `metadata.internal`
- Suffixes: `.localhost`, `.local`, `.internal`

**Legacy IPv4 literal detection** — blocks non-standard IP formats that bypass naive parsing:
- Octal: `0177.0.0.1` → 127.0.0.1
- Hexadecimal: `0x7f000001` → 127.0.0.1
- Decimal: `2130706433` → 127.0.0.1
- Short-form: `127.1` → 127.0.0.1

**URL credential obfuscation** — detects `http://evil.com@127.0.0.1:8080/` patterns.

### DNS Pinning (`dns.rs`)

Prevents DNS rebinding (TOCTOU) attacks:

```
1. Async DNS resolution via tokio::net::lookup_host
2. Validate ALL returned IPs against policy
3. Return first valid SocketAddr
4. Caller pins via reqwest::Client::builder().resolve(host, validated_addr)
5. No TOCTOU window — reqwest connects to pre-validated IP
```

### safe_fetch (`fetch.rs`)

**Single entry point for all outbound HTTP requests.** Replaces direct `reqwest::Client` usage.

```rust
pub async fn safe_fetch(
    url: &str,
    policy: &SsrfPolicy,
    request: SafeFetchRequest,
) -> Result<SafeFetchResponse, SsrfError>
```

**Execution flow:**

```
1. URL format + scheme validation (http/https only)
2. Legacy IPv4 literal rejection
3. URL credential obfuscation detection
4. Hostname blocklist/allowlist check
5. IP literal check OR async DNS resolve + validate all IPs
6. DNS pinning via reqwest resolve()
7. Send request (redirect::Policy::none())
8. If 3xx → redirect loop:
   a. Extract Location header
   b. Repeat steps 1-6 for new URL
   c. Cross-origin → strip Authorization/Cookie/Proxy-Authorization
   d. Redirect counter + loop detection (URL set dedup)
   e. Exceeds max_redirects → SsrfError::TooManyRedirects
9. Return final response
```

### Callers

All outbound HTTP requests go through the SSRF engine:

| Caller | File | Method |
|--------|------|--------|
| Web fetch tool | `builtin_tools/web_fetch.rs` | `safe_fetch()` |
| Webhook delivery | `tasks/cron/webhook_target.rs` | `safe_fetch()` |
| Media downloader | `gateway/pipeline/media_download.rs` | `safe_fetch()` |
| MCP HTTP transport | `mcp/transport/http.rs` | `safe_fetch()` |
| MCP SSE transport | `mcp/transport/sse.rs` | `validate_url_with_pinned()` |
| MCP preflight probe | `mcp/preflight.rs` | `validate_url_async()` + pinned `resolve()` |
| A2A push webhook | `a2a/service/notification.rs` | `validate_url_async()` at register, `safe_fetch()` at send |
| Fetch providers (crawl4ai/firecrawl) | `builtin_tools/web_fetch/mod.rs` | `validate_url_async()` before handing the URL to the provider |
| Media pipeline URLs | `media/pipeline.rs` | `validate_url_async()` |
| Browser navigation | `browser/network_policy.rs` | `validate_url_async()` via `BrowserSsrfGuard` |

### Browser SSRF Guard

**Location**: `src/browser/network_policy.rs`

Thin wrapper over the core SSRF engine with browser-specific features:

```rust
pub struct BrowserSsrfGuard {
    config: SsrfConfig,  // block_private, blocked_domains, allowed_domains,
                         // block_secrets_in_url, redact_secrets_in_content,
                         // block_secrets_in_input
}

impl BrowserSsrfGuard {
    pub fn check_url(&self, url: &str) -> Result<(), PolicyViolation> {
        // Delegates to ssrf::validate_url_async() with converted policy
        // Adds browser-specific allowlist-only mode
    }
}
```

The browser boundary has **four layers** (all default-on; policy knobs under
`[browser.policy]`):

1. **Navigation-in** — `check_navigation` vets every agent-initiated `open`/`goto`
   target: SSRF policy plus `block_secrets_in_url` (rejects URLs embedding a
   Critical-severity credential, raw or percent-encoded — anti-exfiltration).
2. **Post-landing** — `src/browser/post_nav.rs::verify_landed_url` re-checks the
   URL the tab actually landed on after `open_tab`/`navigate` (HTTP redirects can
   cross origins); a violation closes the tab (quarantine) and fails the call.
   Interaction/history ops are deliberately not re-checked here.
3. **Input-side** — `block_secrets_in_input` scans text typed into pages
   (`browser_type` / `browser_fill_form` / `browser_select` / dialog prompts) for
   Critical-severity secrets and refuses the action before it runs, so a
   credential in the model's context cannot be typed into a form on an allowed
   host. Cookie `set` values are intentionally exempt (a cookie value legitimately
   IS a credential).
4. **Content-out** — every page-derived read (snapshot / console / network /
   evaluate / cookies / tab list) passes bound → `redact_secrets_in_content` →
   prompt-injection wrapping; content reads additionally re-validate the tab's
   CURRENT URL (`make_backend_and_tab_guarded`) so a JS/redirect navigation to a
   blocked origin cannot be read out.

Secret patterns are NOT duplicated in the browser layer: the `Critical`-severity
PII rules (`src/pii/rules`) are the single source of truth for layers 1, 3 and 4.

### Content Sanitization

**Location**: `src/security/content_sanitizer.rs`

Wraps fetched external content with boundary markers to prevent prompt injection:

```rust
pub fn wrap_external_content(content: &str, source: ContentSource) -> String
```

**⚠️ 围栏是结构，不是内容 (the boundary is structure, not content)。** 两行标记
（`<<<EXTERNAL_UNTRUSTED_CONTENT id="…">` / `<<<END_…>`）是**唯一**告诉模型"以下不可信、
到此为止"的东西，因此**任何重写文本的下游 stage 都不许碰它**。2026-08-04 之前，§3.14 的
ingress 清洗**整体替换字段**，于是被清洗过的 `web_fetch.content` / 浏览器抓取正文 / MCP
结果的标记随之消失——或者更糟，只剩开头那条（`reduce_log` 的 `KEEP_HEAD` 恰好会留下它），
模型读到一个**没有终点的不可信区**。而它只在这些载荷**大到触发清洗**时才发生，也就是最该
有围栏的时候。

三条纪律：

1. **重写走 `tool_output::fence::rewrite_interior`**，只改内部、标记逐字节重发；不配对
   就整体弃权。
2. **解析走单一源 `split_external_fence`**，判据严格：开/闭标记**各恰好一次**、都在行首、
   id 配对。两段拼接的围栏、被截断的一半，一律拒绝——重新缝在错误的边界上比不缝更糟。
   围栏**之外**的自有文本（`web_fetch` 的 `[fetch_focus: …]` 行）按 prefix/suffix 原样保留。
3. **把一个大围栏拆成若干小围栏时，必须回答"落在小围栏之间的字节还被谁覆盖"**。
   `wrap_external_content` 做两件事：加标记 **和** 归一化/剥不可见字符/转义/清洗
   chat-template 标记。后者对短元数据同样必要，故拆分方使用
   `sanitize_external_text`（＝同一份变换，不加标记）。MCP adapter 逐 text 块围栏时
   就是这么保住覆盖面的；`data` / `blob` 刻意不碰（base64 要能解码，且它的字母表表达不出
   chat-template 标记）。

详见 [FEATURE_LOCATOR §3.14](FEATURE_LOCATOR.md)。

### Audit Logging

**Location**: `src/security/audit.rs`

SSRF blocks are logged with context, hostname, and rejection reason for security monitoring.

### Panel Configuration

Users configure SSRF settings via the Panel UI (Settings → Security → Outbound Request Protection):

| Setting | Config Key | Default |
|---------|-----------|---------|
| Enable SSRF Protection | `security.ssrf.enabled` | `true` |
| Allow tools to access LAN | `security.ssrf.allow_tool_private_network` | `false` |
| Allow webhooks to access LAN | `security.ssrf.allow_webhook_private_network` | `false` |
| Max redirects | `security.ssrf.max_redirects` | `5` |
| Trusted host allowlist | `security.ssrf.allowed_hosts` | `[]` |
| Blocked host denylist | `security.ssrf.blocked_hosts` | `[]` |

**Config file** (`~/.aleph/config.toml`):

```toml
[security.ssrf]
enabled = true
allow_tool_private_network = false
allow_webhook_private_network = false
max_redirects = 5
allowed_hosts = ["*.corp.internal"]
blocked_hosts = ["*.malware.com"]
```

**RPC**: `security_config.get` / `security_config.update` — read/write via Gateway JSON-RPC.

### Security Guarantees

1. **Fail-closed** — malformed URLs, unresolvable hosts, and unknown IP formats are rejected
2. **DNS pinning** — eliminates TOCTOU race conditions between validation and connection
3. **Every hop validated** — redirect targets are validated with the same rigor as the initial URL
4. **Header isolation** — Authorization/Cookie headers stripped on cross-origin redirects
5. **Policy-aware** — `allow_private_network=true` still blocks loopback and cloud metadata
6. **Unified engine** — single implementation prevents coverage gaps between callers

### For Developers

1. **Always use `safe_fetch()`** for outbound HTTP requests — never use `reqwest::Client` directly
2. **Use `validate_url_async()`** for URL-only validation without fetching (e.g., browser navigation)
3. **Construct caller-specific policies** — tools and webhooks may have different `allow_private_network` settings based on user configuration
4. **Add new callers** — any new outbound HTTP code must go through `safe_fetch()` or `validate_url_async()`
5. **Test coverage** — add tests for new IP ranges or bypass vectors in `ip.rs` and `hostname.rs`

---

## Trust model: network boundary + Gateway token {#auth-ux}

The trust boundary is the network boundary, gated by a **Gateway-token login
wall**. Loopback is the implicit operator (zero-config, no credential). A
remote connection must present a valid credential at the `connect` handshake,
resolved by `src/gateway/handlers/connect.rs::resolve_connect_auth` in
priority order: (1) loopback ⇒ operator; (2) **device token** (`aleph-dt-*`,
long-lived, bound to a paired device, SHA-256-hashed at rest); (3) **bootstrap
ticket** (`aleph-bt-*`, 5-min single-use, exchanged during onboarding for a
fresh device token); (4) the legacy shared **Gateway token** (`aleph-<uuid>`,
`SharedTokenManager`, HMAC-hashed, constant-time verified). A valid credential
= full operator authority (single tier, identical to local); a missing /
invalid one is walled (the WS dispatch refuses every method but `connect`, and
a flood guard closes a connection that keeps probing). Revocation is token
rotation (`gateway.token.rotate` — regenerates the shared token, revokes all
paired Panel devices, and force-closes live remote sockets) or per-device
revoke (`gateway.devices.revoke` — drops that device's live sessions to the
login wall, then closes their sockets; the roster `gateway.devices.list` marks
which devices are connected right now). Both take effect immediately rather
than at the next handshake. Rejected remote connects and flood-guard
closes are recorded in the security audit log (`AuthFailure` / `RateLimited`).

### 多用户角色层（P0）{#multi-user-roles-p0}

The trust boundary above answers "is this connection authorized at all";
this layer answers "authorized as **whom**, and with **what** authority."
Landed as the P0 identity foundation
(`docs/superpowers/plans/2026-08-04-p0-identity-foundation.md`), it stays
strictly additive to the single-tier model — a single-machine deployment
sees byte-identical behavior before and after.

- **Users table.** `src/gateway/security/store/users.rs` adds a `users` table
  (schema v14): `user_id`, `display_name`, `role ∈ {admin, member}`,
  `status ∈ {active, deactivated}`, `created_at`. `role` drives the
  admin/member boundary below; `status = deactivated` walls every connection
  bound to that user, immediately (see deactivation below).
- **Device / pairing linking.** Two independent binding paths feed the same
  `users` table, both with identical COALESCE semantics ("an unbound rebind
  never clobbers an existing binding; a still-unbound row after the write
  defaults to the owner"):
  - **Panel devices** (`devices.user_id`): `gateway.ticket.create` can bind a
    bootstrap ticket to a `user_id`; the device that exchanges it
    (`DeviceTokenManager::exchange_bootstrap_ticket`) inherits the binding via
    `upsert_device`'s `COALESCE(excluded.user_id, devices.user_id)`, and
    `set_device_user_if_unbound` defaults a brand-new unbound pairing to the
    owner.
  - **Channel senders** (`pairing_store.approved_senders.user_id`): approving
    a channel sender (`PairingStore::approve`) can bind the same way;
    `sender_user(channel, sender_id)` resolves it with the same
    bound-is-sticky / unbound-defaults-to-owner semantics.

    ⚠️ **This half had no producer until 2026-08-09.** The column, the SQL's
    `COALESCE`, and the consumer (`inbound_router::executor` stamps
    `ScopeAttribution::personal` from `sender_user` on every inbound turn) all
    shipped with P0/P1 — while `handlers::pairing::handle_approve` passed
    `None`, with a comment saying P1 would thread a real id through. P1 landed
    and did not. Every approved sender on every channel therefore resolved to
    the owner: a member's Telegram turns were stamped `personal:u-owner`, so
    their session rows were filed under the operator, their messages ingested
    into `main__u-owner`, and the OWNER's curated `MEMORY.md` / `USER.md`
    injected into their prompts.

    `channel.pairing.approve` now takes an optional `user_id`, **validated
    against the users table before the approval commits** (a binding to a
    dangling id gives the sender an identity nobody can grant, revoke or list,
    and surfaces much later as "the wrong person's memory" — same reason
    `pair --user` validates), and echoes it in the response so the approving
    surface can say who this sender now is. Omitted still means the owner.
    Client: the Panel's channel settings, which renders the picker only when a
    second principal exists.

    **Still unbound by design:** `DmPolicy::Allowlist` / `allow_from` senders
    never enter `approved_senders` at all, so they stay owner-scoped. If
    per-user allowlist entries are ever wanted, that is a second producer, not
    a widening of this one.
- **Resolved per connection**, not just per credential:
  `handlers/connect.rs::resolve_connection_identity` turns an authorized
  connection into `(Option<user_id>, role)` — loopback and any
  authorized-but-unbound credential (legacy shared token, a pre-v14 device row
  with no `user_id`) still resolve to the implicit owner as `"operator"` (the
  zero-change guarantee); a device bound to an `admin`-role user resolves to
  `"operator"`, one bound to a `member`-role user resolves to `"member"`; a
  device bound to a **deactivated** user, or whose `user_id` points at a row
  no longer in `users` (dangling reference), fails **closed** to
  `("guest", None)` — a lookup that could not be performed, or a link known to
  be broken, must never silently grant full authority. The same fail-closed
  rule covers the no-store degrade: no store **and** a presented `device_id`
  ⇒ guest (a binding lookup that could not be performed is not "unbound"),
  while no store and no device keeps the pre-P0 owner fallback.
  The pair rides `CALLER_ROLE` / `CALLER_USER` task-locals
  (`src/gateway/caller_identity.rs`) scoped around every `process_request`
  call, and it is echoed back to the client in the `connect` response
  (`role` / `authorized` / `needs_token`) — the **resolved** role, not the
  credential-only verdict, so a member renders a member UI and a deactivated
  user's still-valid device gets the ordinary walled response instead of a
  false `operator` + a dead UI.
- **Two gates, not one.** The login wall (`wall_admits`,
  `src/gateway/server/handler.rs`) is the *guest* wall and admits both
  authorized roles — `"operator"` and `"member"` — for every method; a walled
  connection may only send `connect`. The admin/member split is the *separate*,
  deeper gate below, at the `process_request` chokepoint. Conflating them
  refuses real members everything and then flood-guard-kicks them as abusers.
- **Admin / member method boundary** (spec §4.6). `method_admin.rs`'s
  `method_requires_admin` classifies RPC **methods** by prefix — sibling of
  the pre-existing `method_authz.rs`, which classifies **tools** for the
  channel chat-tier gate; the two are separate axes (method vs. tool) and
  don't substitute for each other. A prefix match gates the whole family by
  default (fail-closed for privilege); a short allowlist re-opens member-safe
  reads inside an otherwise-admin family. The table below is a **summary**;
  the authoritative classification is `method_admin.rs`'s `ADMIN_PREFIXES` +
  `ADMIN_METHODS` + `MEMBER_CARVE_OUTS` themselves, whose module doc records
  the mechanical sweep (**74 method families**) and the reasoning for every
  non-obvious open ruling. That file is both the enforcement point and the
  audit trail — there is no separate report artifact to consult.

  ⚠️ **That "audit trail" is prose, not a test** — and FEATURE_LOCATOR §5.22
  round-4 「刻意未做 ⑥」 records why that matters: the only enumerating test
  covers `workspace.` alone, everything else is a curated literal tripwire, and
  `HandlerRegistry::new()` holds only phase-1 registrations, so even a runtime
  census over `methods()` would be blind to the families registered later via
  `register_handler!(server, …)`. Four member-reachable server-global verbs
  lived in that blind spot until 2026-08-13.

  **`ADMIN_METHODS` (2026-08-13) is a floor, not a default.** A prefix gates a
  family, which works when the family is uniformly one thing. `memory.` is not:
  it is deliberately open because `memory.search` / `memory.stats` really are
  per-caller reads, while `memory.compress` / `memory.reembed` /
  `memory.reembed.cancel` sweep every corpus on disk. Those three are named
  individually and checked **before** `MEMBER_CARVE_OUTS`, so a name here
  cannot be re-opened from the other side of the file; the two tables naming
  the same method fails `no_admin_method_is_also_a_carve_out` rather than
  resolving to whichever `if` was written first. A per-caller *filter* cannot
  express this — there is no caller-owned subset of "recompress the whole
  install" — which is also why `method_visibility.rs` records `memory.reembed`
  as having no filter to apply. That ruling is about visibility and is correct;
  this table is the admission half it was silent on.

  | Family | Verdict | Why |
  |---|---|---|
  | `gateway.*`, `users.*`, `cluster.*`, `environments.*`, `services.*` | **admin** | Trust-boundary credentials/tokens/devices, principal management, fleet membership, server process control. `environments.list` is the fleet's READ face and lived outside the `cluster.` prefix until 2026-08-07; its delivery-side twin is `event_scope.rs`'s `node.` rule, since `node.connected`/`node.disconnected` carry the same ids |
  | `providers.*`, `embedding_providers.*`, `generation_providers.*`, `channels.*`, `channel.*`, `discord.*` | **admin** | Server-global provider/channel credentials & config |
  | `config.*`, `secrets.*`, and 11 Settings-page `*_config.*` families (`security_config.` … `route_config.`), `routing_rules.*`, `logs.*` | **admin** | Server configuration surfaces (Settings page) |
  | `extensions.*`, `mcp.*`, `mcp_config.*`, `skills.*`, `bundled.*`, `plugins.*`/`plugin.*`, `hooks.*`, `runtimes.*` | **admin** | Install-class capability surfaces |
  | `agents.*` (carve-outs `agents.list`/`agents.get`), `identity.*`, `moa.*`, `acp.*` | **admin** | Server-global persona/shared config, not per-user |
  | `cron.*`, `heartbeat.*` (carve-outs `.list`/`.get`/`.runs`) | **admin** | Scheduled automation — mirrors `method_authz.rs`'s existing tool-tier ruling, so the RPC surface isn't a lower-privilege bypass of it |
  | `daemon.*`, `wizard.*`, `diagnostics.*`, `pty.*`, `exec.*` | **admin** | Fleet lifecycle, raw interactive shell, exec-approval gate resolution |
  | `dreaming.*` (carve-out `dreaming.list_insights`) | **admin** | `dreaming.run_now` force-drives a full cycle of the global daemon over `maintenance_corpora` — every `main__u-*` and `main__p-*` partition on disk — an LLM-driven lint / consolidate / synthesize / **archive** pass over every other principal's notes, with no visibility predicate anywhere in `src/memory/dreaming/`. **This family had no prefix at all until 2026-08-13**, so it fell through to the default-open tail while three separate places in the tree called it "the admin RPC": a gate three documents asserted and no code performed. The read face stays open, narrowed by its own `partition_visible` |
  | `memory.compress`, `memory.reembed`, `memory.reembed.cancel` | **admin** (`ADMIN_METHODS`) | Server-global maintenance inside a deliberately open family — see the `ADMIN_METHODS` note above |
  | `tools.*` | **admin** | `tools.invoke` dispatches straight off the raw `ToolRegistry`, so none of the loop's gates run there — including the per-tool operator gate (`method_authz.rs`'s `OPERATOR_TOOLS`: `cron_manage`, `hooks_manage`, `agent_identity`, …), which its own hard floor does not cover. An RPC surface must not be a lower-privilege bypass of an existing tool-tier decision, and via `cron_manage` a member could schedule a run that executes as trusted-internal. The family is gated whole (E2E-oriented surface by its own module doc); a member-safe read carve-out is a P1 call |
  | `workspace.*` | **admin** | The second face of the same `agent_envs` table `agents.*` writes. Gated whole on 2026-08-08 after a real-machine QA in which a member renamed and archived a workspace the operator had created; `partition_visible` cannot close that, because a workspace id encodes no owner. Both clients are operator surfaces — the CLI, and (since 2026-08-09) the Panel's `/settings/workspaces`, which reports this refusal rather than rendering an empty roster. **This row said "open" until 2026-08-09**: the family moved into `ADMIN_PREFIXES` a day earlier and the summary table was not moved with it, so this document described a member carve-out that the code had already withdrawn |
  | `connect`, `chat.*`, `sessions.*`, `memory.*`, `projects.*`, `artifacts.*`, `fs.*`, `teams.*`, `voice.*`, `graph.*` | **open** | Member daily / caller's-own-data surfaces; per-user *visibility* filtering is P1's job, not this gate's |
  | `users.me`, `users.list`, `agents.list`, `agents.get`, `heartbeat.list`/`.get`/`.runs` | **open** | Member-safe reads, carved out of otherwise-admin families |

  Enforced at **one chokepoint** inside `process_request`
  (`src/gateway/server/handler.rs`) — both WS dispatch stations (the
  `do_lane_dispatch` closure and the idempotency `Proceed` arm) scope
  `CALLER_ROLE` around `process_request`, so this single check covers both. A
  `"member"` role hitting an admin-classified method is refused with the same
  error code the login wall uses for non-`connect` methods on walled
  connections. `None` (cron/internal) and `"operator"` pass every method; a
  `"guest"` connection never reaches this check for non-`connect` methods
  because the login wall above already refuses it first.
- **Deactivation kicks live Panel sessions.** `users.update { status:
  "deactivated" }` revokes every live **Panel device** bound to that user
  through the same `revoke_device_and_kick` pipeline `gateway.devices.revoke`
  uses (demote the connection to guest, then close the socket) — not a second
  implementation. See `src/gateway/CLAUDE.md`'s revocation landmines for the
  ordering / single-source discipline that pipeline depends on.
  **Scope, precisely (corrected 2026-08-13, extended 2026-09-03):**
  deactivation withdraws the credentials a principal is bound through —
  `devices.user_id` (WS/Panel), `approved_senders.user_id` (channel senders,
  via `PairingStore::revoke_for_user`), and the principal's **outstanding
  bootstrap tickets** (`SecurityStore::revoke_bootstrap_tickets_for_user`,
  schema v18's `revoked_at`). Ad-hoc cutoff of a single sender is still
  `channel.pairing.revoke`; of a single ticket, `gateway.ticket.revoke`.

  ⚠️ **The ticket leg exists because `exchange_bootstrap_ticket` checks no
  user status at all** — the status gate sits only on the two MINTING faces. So
  mint → deactivate → redeem produced a brand-new, un-revoked device **after**
  the sweep had finished, while the reactivation receipt still asserted "the
  devices stay revoked — mint a fresh ticket". The count reports **live
  credentials cut**, so an already-expired ticket is excluded (it is inert;
  counting it would inflate both the receipt and the audit row), and a revoked
  ticket's redemption is deliberately indistinguishable from an unknown or
  already-consumed one. Unbound tickets (`user_id IS NULL`) are untouched by
  this leg — they belong to no principal, and they are the higher-authority
  half; `gateway.ticket.revoke` is what reaches them.

  The paragraph that stood here until 2026-08-13 said channel senders were
  *not* revoked, on the reasoning that "`sender_user()` has no consumer yet, so
  the link is recorded but carries no authority to withdraw". That was
  contradicted ~95 lines above in this same file, and by the code since round-3
  connected the producer: `inbound_router::executor` stamps
  `ScopeAttribution::personal` from `sender_user` on **every inbound turn**, so
  the link decides which session rows a person gets, which memory partition
  they read and write, and whose curated `MEMORY.md` / `USER.md` is injected
  into their prompts. A deactivated member therefore kept messaging the bot
  under their own identity — and could call
  `goal(action='update', status='active')` / `loop(action='resume')` to undo the
  freeze `users.update` had just applied.

  ⚠️ **Removal, not a status check inside `sender_user`.** That resolver's
  `None` does not mean "refused"; it means *unlinked*, and its consumer reads
  unlinked as legacy owner semantics — it stamps no attribution and the run is
  adopted by the operator. Answering a deactivated member with `None` would
  have upgraded them to the OWNER's scope, memory and sessions: strictly more
  authority than the leak it was meant to close. A refusal folded into a value
  that already means something else inverts into a grant.

  ⚠️ **Still unbound by design, unchanged:** `DmPolicy::Allowlist` / `allow_from`
  senders never enter `approved_senders`, so they stay owner-scoped and this
  sweep does not reach them. On an `Open`-policy channel, anyone at all is
  owner-scoped — that is the pre-existing product property of an open channel,
  not something deactivation can withdraw.
- **Role changes take effect on live connections.** `users.update { role }`
  re-stamps `caller_role` on the user's already-open Panel connections
  (`restamp_live_connections`), because the wire role is latched into
  `ConnectionState` at the `connect` handshake and read from there on every
  later frame — a store-only write would leave a demoted admin holding admin
  authority on its open tab until it happened to reconnect. Promotion and
  demotion both; a connection already walled at `"guest"` (revoked device /
  deactivated user) is never promoted this way — only a fresh `connect` lifts
  the wall.
- **Implicit owner, zero migration.** `ensure_bootstrap_owner` runs at every
  store open: if `users` is empty it mints `u-owner` (`admin`, `active`) and
  adopts every un-owned **panel** device (`devices.user_id IS NULL AND
  device_type = 'panel'`; shared cluster-node rows are machines, never
  adopted). Every pre-existing single-user deployment therefore ends up with
  exactly one user, owning every device it already had — loopback and legacy
  credentials keep resolving to that same owner as full operator, so the
  single-user experience is unchanged.

### 多用户数据隔离层（P1）{#multi-user-isolation-p1}

The P0 layer above answers "authorized as whom"; this layer answers "can
that identity SEE this particular row of data." Landed as the P1 data
isolation plan (`docs/superpowers/plans/2026-08-05-p1-data-isolation.md`),
it is partition-key composition (spec §3): a new `src/scope/` vocabulary
rides the *existing* `project_scope.rs` suffix mechanism for memory, new
`owner_user_id`/`scope_id` columns on sessions and background-work stores,
one ambient `ScopeAttribution` task-local seeded at gateway dispatch and at
every `tokio::spawn` run boundary, and a single predicate family every
scoped-data RPC handler and the WS event-delivery filter both consume.
Legacy rows (no owner field) read as owner-owned — adoption by absence, zero
backfill migration; the single-user experience is byte-identical before and
after (verified by `single_user_fixture_is_byte_identical_after_upgrade`,
`src/gateway/isolation_acceptance.rs`).

- **Scope vocabulary** (`src/scope/mod.rs`). `ScopeId::{Org, Personal(user_id),
  Project(project_id)}` and `ScopeAttribution { owner_user_id, scope }`.
  `Org`/`Personal`/`Project` render to `"org"` / `"personal:<id>"` /
  `"project:<id>"` and compose directly with `project_scope::scoped_agent_id`'s
  suffix grammar — the `proj-*` (legacy project-directory feature) / `u-*`
  (personal) / `p-*` (project, P2) suffix families are siblings, never
  nested. Carried by a `tokio::task_local!` (`with_scope`/`current_scope`),
  scoped around every dispatch by `server::handler::
  dispatch_with_caller_context` exactly like P0's `CALLER_USER`/
  `CALLER_ROLE` — and, like those, does NOT cross a `tokio::spawn` boundary:
  any run-work spawn must re-seed it explicitly (see the
  `src/gateway/CLAUDE.md` landmine below).

  **Two facts about that re-seeding, both learned the expensive way:**

  1. **Re-establishing the task-local is only half.** `run_loop` rebuilds the
     run's scope from `request.metadata` and nothing else, and
     `ensure_session_under_request_scope` stamps the session row from the same
     map — so a spawn that restores the task-local and does not also
     `scope::stamp_metadata` still writes a NULL/NULL row. The team fan-out did
     neither for a full round.
  2. **The carrier is one type, not a three-line ritual.**
     `scope::CarriedAttribution` (`src/scope/carried.rs`) captures scope +
     project root + agent id + `caller_role` before a spawn and re-establishes
     all four inside it. It lives in `src/scope/` because `agents` and `teams`
     both depend on `scope` and not on each other. `caller_role` joined it
     because it fails in the same place with the OPPOSITE direction: the other
     three are fail-quiet (work lands in the wrong partition), while
     `turn_context::role_is_operator(None)` is `true` by design, so a lost role
     is fail-OPEN — the run skips both the exec-tier ceiling and the
     operator-tool gate. Census:
     `run_loop::tests::scope_stamping_producers_are_all_accounted_for`.

  **Which actor a predicate is handed is a separate question from which
  predicate it is.** `scope::ambient_owner()` answers "who owns this row"; in a
  project room it names the room's CREATOR, identically for every member —
  exactly the mechanism that makes members share one memory partition. A
  predicate asking "who is ASKING" must use `visibility::ambient_actor()` (the
  turn's speaker in a room, byte-identical to `ambient_owner` everywhere else).
  Four tool-face predicates acquired the wrong one independently, because three
  places in the repo stated the relationship backwards. Pinned by
  `visibility::AMBIENT_OWNER_CENSUS`: a new `ambient_owner()` call site must
  declare which of the two questions it answers.

  **A writer's composition is a contract on its readers.** Every memory/note
  writer composes the session scope into the partition id through
  `project_scope::session_write_id`, so a scoped run writes `main__u-alice` /
  `main__p-room`. Nine readers did not compose and silently read the empty base
  partition — never a multi-user-only defect, because a zero-config loopback
  Panel session is already `Personal(u-owner)`. The tool face's single answer is
  `BuiltinToolRegistry::caller_memory_partition`, with
  `caller_profile_partition` as the twin that must REFUSE inside a room (a room
  holds more than one human, so there is no single profile to return).
  `memory_reflect`'s query filer was the last writer outside that contract
  (2026-09-03): it composed from the **base** persona id, so a note synthesised
  out of one member's private memory landed in the org partition `main`, which
  is unioned into every principal's read set — the read side had always been
  right, only the write side was wrong.

  ⚠️ **A model-writable field that becomes a partition key must refuse a
  COMPOSED id, not merely a traversing one.** `note_manage.agent_id` is
  exposed on the JsonSchema the model writes and is used directly as the
  partition key, and until 2026-09-03 it validated only path traversal — so
  `agent_id: "main__u-alice"` addressed another principal's vault byte for
  byte. What stopped it from being exploited was luck:
  `partition_visible_to(_, None)` answers **true**, so the visibility gate is
  open to exactly the callers (tool face, cron, A2A) that most need it.
  `resolve_agent_id` — the single chokepoint all eleven call sites pass
  through — now refuses any id the caller was never handed, and the refusal
  text is **identical whether or not that vault exists**; a refusal that varies
  is an existence oracle.
- **Visibility chokepoint** (`src/gateway/visibility.rs`). `effective_owner`
  is the ONE place "who owns this row" is decided: a session's own
  `owner_user_id`, or `OWNER_USER_ID` for a legacy/pre-P1 row with none
  (adoption by absence). `session_visible` and `partition_visible` turn that
  into a boolean for a session row / a `<base>__<suffix>` memory partition
  id respectively; `visible_owner_filter` is `None` for an unrestricted
  (internal/cron/A2A) caller — the zero-change guarantee for
  single-user/internal callers — or `Some(caller)` for a scoped one.
  `not_found_response` is the single, byte-identical `RESOURCE_NOT_FOUND`
  response every addressed-key denial returns — see "NOT_FOUND over
  forbidden" below. Any handler that writes its own `meta.owner_user_id ==
  caller` comparison instead of calling one of these predicates, or filters
  `sessions.list` without setting `SessionFilter::owner_visible_to`, is
  exactly the bypass this module exists to prevent.
- **Registry + regression net** (`src/gateway/method_visibility.rs`). NOT a
  dispatch gate — a durable table pairing every scoped-data RPC method with
  its enforcement shape (`KeyChecked` / `PartitionChecked` / `ListFiltered`)
  and a pin test that fails loudly if a method's enforcement call is ever
  removed. Sibling of P0's `method_admin.rs` (same shape, different
  question: that one asks "does this method need operator role," this one
  asks "does this method's answer depend on who's asking, and is that
  enforced"). Covers `sessions.*`/`session.*`/`chat.*`,
  `memory.*`/`artifacts.*`/`clarification.*`/`subagent.tree`/`graph.query`
  and (since 2026-08-06) every `teams.*` method that addresses a record or
  filters a list, `teams.chat.cancel` included since 2026-08-07 — see that
  file's module doc for the full per-method breakdown and for the two
  siblings deliberately left out of the table. The literal count that used to
  stand here is gone on purpose — it disagreed with the table by one; the
  table is the source.
- **Team ownership** (`src/teams/scoped.rs`). P1 originally shipped `teams.*`
  as org-shared: `Team` had no owner field, so there was nothing to check
  without first inventing an ownership model. That was overturned by human
  ruling on 2026-08-06. `Team` now carries `owner_user_id` on the same
  adoption-by-absence terms as a session, stamped inside
  `SqliteTeamStore::create_team` so every creation path (RPC, `team_create`,
  `team_from_template`, template materialization) lands owned.
  **Enforcement is a `TeamStore` decorator, not a per-call-site check** —
  teams are reachable from the gateway AND from ~30 `team_*` builtin tools a
  model calls mid-run, and putting the predicate on the one path both cross
  is what makes the tool half enforced rather than "the Panel half enforced
  and the chat half wide open". `ScopedTeamStore::wrap` is applied at the
  single construction site (`builder::agent_init::coord_stores`); publishing
  the raw store anywhere else is the bypass.
  - The resolver is `visibility::ambient_actor()` — the SPEAKER in a room,
    falling back to the gateway `CALLER_USER` / run-seeded `ScopeAttribution`
    outside one — and the rule body is the same one sessions use
    (`visibility::owner_and_scope_visible_to`): since 2026-08-18 (round-5)
    `Team` also carries `scope_id`, so a team created inside a project room
    is stamped `project:<id>` and is visible to the room's whole roster,
    while personal/legacy rows fall through to owner-equality exactly as
    before. `CALLER_USER` alone is dead inside a spawned run, so a team
    predicate built on `visible_owner_filter()` would be fail-open for every
    tool call.
  - The gateway still gates explicitly
    (`handlers::teams::visibility::{gate_team, gate_task}`) for the two things
    a decorator cannot do: produce the byte-identical `not_found` response,
    and reach the ~20 methods that address a team through the `coord_tasks`
    DAG — a different database the team store cannot see. A task with no team
    reads as an unstamped record (the legacy owner's), never as public.
  - `teams.chat.cancel` is a third shape: its key is a fan-out tree
    `run_id`, which neither store can resolve. `register_fanout` — the single
    point a tree run id is minted — records `run_id → team_id` in a bounded
    index (`teams::broadcast::team_of_fanout_run`), and the handler resolves
    through it into the same `gate_team`. It was open until 2026-08-07 on the
    reasoning that a run id is an unguessable capability; that made this
    handler's safety depend on the event plane's classification table, which
    was in fact broadcasting every user's tree run ids to everyone. A gate
    that is really another subsystem's invariant is not a gate.
  - Six `team_*` tools are the tool-side twin of that second case
    (`team_task_control`, `workflow_step_review`, `task_comment`,
    `task_exit_journal`, `task_submit`, `team_workflow_canvas`): they address
    a task or team through `CoordTaskStore` alone, so they call
    `teams::task_team_reachable` after their own lookup. `team_workflow_canvas`
    is gated despite being read-only — its `export` enumerates every task in a
    team, and it is the one face that hands out ids for the other five.
    **Any new tool that reaches a coord task by id owes the same call**; the
    decorator will not catch it.
- **Event delivery** (`src/gateway/event_visibility.rs`). The event-bus
  analogue of the RPC chokepoint above: `EventScopeGuard` (P0) is
  role-based and default-allow for ordinary session/run events, so without
  this every connected member would receive every OTHER user's live run
  stream. `EventVisibilityIndex` is the 4th `&&` term in `server::handler`'s
  `should_forward` filter chain — it classifies each delivered frame's
  identity (`session_identity_of`: by session key directly, by `run_id`
  through a seeded run→session cache, by `team_id` for the `team.<id>.*`
  plane, or `Global` for org-level infrastructure) and then resolves that
  identity to an owner through the same predicates the RPC path uses
  (`visibility::owner_and_scope_visible_to` for a session — so a project
  room's frames follow its roster, not its creator — and
  `visibility::owner_or_legacy` for a team). Fails closed: an unresolvable
  `run_id` (cache miss), an unresolvable `team_id`, or a walled
  `caller_user: None` connection is denied, never admitted by default.
  The `team.<id>.*` half needs its own arm because `publish_team_event`
  emits a raw `{topic, data}` envelope with no `GatewayEventFrame` variant
  behind it, so the compile-anchored `every_frame_variant_is_classified`
  pin cannot see it — a SOURCE-level pin
  (`no_published_team_topic_suffix_classifies_as_global`) covers that
  producer shape instead, and the id is extracted structurally so a suffix
  added later is scoped rather than broadcast. ⚠️ Resolving through a store
  makes delivery depend on that store's HANDLE being installed; it currently is
  not, on one supported degraded boot — see known gap #3 below.
  - **One frame is PROJECTED rather than admitted or denied** (2026-08-07).
    `stream.running_set_changed` carries `{seq, running: Vec<String>}` — every
    in-flight session KEY in the process, spanning every user (keys only; the
    claim that it also carries `run_id`s was wrong). Neither boolean is right
    for it: forwarding it whole hands every member everyone else's live
    session keys, and gating the topic operator-only extinguishes each
    member's OWN sidebar red dot, which this frame is the authoritative feed
    for. So it stays `Global` and `EventVisibilityIndex::project_for` narrows
    its ARRAY per connection through the same `session_admits` every other
    frame uses. Two invariants: the frame is still SENT when the array comes
    back empty (the Panel drops any frame with `seq <= server_seq`, so a
    suppressed one latches a stale dot for the rest of the connection), and an
    element whose owner cannot be resolved is DROPPED, never passed through.
    The same set has a second producer — `gateway.metrics.run_concurrency`,
    the Panel's cold-load seed for those dots — filtered by the same rule and
    carved out of the admin gate in the same change, because a fallback that
    is admin-gated does not work for the population the filtering is for.
    ⚠️ The drop rule interacts with WHEN the frame is published: the claim
    broadcasts before the session row exists, so a new conversation's first
    turn resolved to nothing and was dropped from both producers. Fixed at the
    resolution step rather than in the drop rule — see the CLOSED gap #2
    («The running-set projection raced a new session's row into existence»),
    which carries the resolution at the end of its own item. *(This paragraph
    called it "an open defect" until 2026-08-30, pointing at an entry that had
    already recorded itself resolved — a cross-reference is a second statement
    of the same fact, and closing the target does not update the pointer.)*
- **Background-work ownership.** `goal::Goal` and `looping::LoopState` both
  carry the same `owner_user_id`/`scope_id` pair, stamped once at creation
  from `scope::current_scope()` (`with_owner_scope`) and preserved across
  updates (e.g. `GoalStore::commit_field_update`'s status CAS never
  clobbers it). **Deactivation freeze** (spec §10): `users.update { status:
  "deactivated" }` freezes background work owned by that user — **four legs,
  one per subsystem that runs work on a principal's behalf**:
  `GoalStore::pause_all_owned_by`, `LoopRegistry::pause_all_owned_by`,
  (since 2026-08-13) `CronService::pause_all_owned_by`, and (since
  2026-09-03) `tasks::heartbeat::service::ops::pause_all_owned_by`. The first
  three are reached through process-global handles because the freeze is a free
  function with no injected dependencies; cron was the one it could not reach.

  ⚠️ **The heartbeat leg is passed in, not read from a global, and it reports
  `Option<usize>`.** A process-global is install-once, so a test that installs
  a service can never afterwards observe the not-installed arm — the leg
  therefore takes its handle as a parameter and the three older legs' shape is
  the one that could not be tested. And "the leg did not run" is a different
  answer from "the leg froze nothing": an absent field on the receipt says the
  service was declined at boot (the capability slot carries the reason
  verbatim), where `0` would be a claim. The other three still report `0` on a
  store failure, which is criterion #8's shape; **one ruling across all four is
  owed, not four.**

  Until 2026-09-03 `HeartbeatTask` carried neither `owner_user_id` nor
  `scope_id` at all — nine files, zero hits — so a deactivated second admin's
  monitors kept firing and kept delivering, and every beat's L2 run executed
  with no scope and billed `@unattributed`. Both columns now carry cron's
  byte-identical serde attributes, both creation faces share one
  `stamp_current_scope()`, and an incoherent `(owner, scope)` pair stamps
  **no** scope rather than inventing one. The service is enabled by default.

  One-way, no auto-resume on reactivation (spec silent on the reverse;
  recorded as a deliberate P1 scope boundary, not an oversight).

  ⚠️ **Cron was excluded until 2026-08-13 on a premise `handlers/users.rs`
  itself falsifies**: "`cron.*` is admin-gated, so a deactivated MEMBER owns
  none by construction". The owner guard in that file pins only
  `OWNER_USER_ID`, and `users.create` accepts `role: "admin"` — so a SECOND
  admin is fully deactivatable and certainly owns cron jobs, whose schedules
  kept firing afterwards with their attribution rehydrated into every run
  (`build_cron_metadata`). The escape hatch the comment watched for ("if cron
  creation is ever opened to non-admin members") was not the one that fired.

  ⚠️ **The sweep is set-to-disabled, never toggle.** A sweep must be idempotent
  — a repeated `users.update{status:"deactivated"}` or a retry must not ENABLE
  an already-disabled job, which is what a toggle does. It is also scoped to
  the principal (another admin's schedules survive) and leaves pre-P1 jobs with
  no owner alone.

  ⚠️ **This is an enumerated sweep at WRITE time, and that is its weakness.**
  It covers exactly the legs enumerated on the day it was written; a fourth
  subsystem added later will be forgotten in the same way cron was. The
  stronger shape — re-asking owner liveness at FIRE time, in the one chokepoint
  each scheduler already has — is Aleph's own 「界限要在执行时刻成立」 criterion
  applied to identity rather than to time, and is recorded as deliberately not
  done in FEATURE_LOCATOR §5.22 round-4 「刻意未做 ④」 rather than half-built.
- **Scope is immutable for a session's lifetime** (spec §10), **with exactly
  one exception, added 2026-08-30.** `owner_user_id`/`scope_id` are stamped
  once, at session creation
  (`SessionMetadata::stamp_attribution`, the CREATE branch only — reading an
  EXISTING row, even as its owner, never (re)stamps it;
  `single_user_fixture_is_byte_identical_after_upgrade` pins this directly).
  This is also why the curated-memory envelope can stay in the prompt's
  Stable (cacheable) zone per session (CLAUDE.md §2.18): per-user bytes are
  per-session stable.

  **The exception is `SessionStore::rescope_attribution`**, which moves an
  existing row's scope into a project room when an operator binds a channel
  conversation to that room. It exists because binding is a decision a human
  makes, with a reason, recorded in the audit log — whereas
  `backfill_attribution` can only heal rows that were never stamped at all.
  Without it a bound conversation splits in two: the RUN takes the room scope
  while the ROW keeps `personal:<first speaker>`, so every other member's
  `session_visible_to` says false and the group stays invisible in their
  session list.

  **Three narrowings, and all three live in the verb rather than in a rule
  each caller has to remember:**
  1. **Only a conversation.** `key` must be a `SessionKey::Group`; anything
     else is refused inside the verb via `require_conversation_key`, so
     "only a conversation may be rescoped" is a property of the method and
     has one author across every backend. A DM has one human on the far side
     and no roster to grant visibility to.
  2. **Only into a project.** It takes a `project_id`, **not** a rendered
     `scope_id`. There is no such thing as rescoping a conversation into
     someone's personal scope or into org scope, so the parameter's *type*
     carries "which kind of scope" instead of it being a value the caller can
     get wrong. The runtime check for that was **deleted**, not centralised —
     there is no longer a rendered-personal-scope string a caller could pass.
     (`backfill_attribution` legitimately takes a `&str` for all three kinds,
     because it heals a NULL row to whatever the run's ambient attribution
     was. The divergence is deliberate; do not "harmonise" it back.)
  3. **Only the scope.** `owner_user_id` still names whoever spoke first. For
     a project-scoped row visibility is decided by the roster, so overwriting
     the owner would buy nothing and lose the byline.

  Its single caller is pinned by name in
  `session_store::caller_census::SOLE_CALLERS`, and the RPC that reaches it
  (`projects.channel.bind`) is in `method_admin::ADMIN_METHODS`.
- **NOT_FOUND over forbidden.** Every addressed-key visibility denial
  (`sessions.history`, `artifacts.read_text`, `sessions.new` on a foreign
  key, …) returns the EXACT SAME `RESOURCE_NOT_FOUND` response a genuinely
  missing key would — never a distinct "forbidden"/"not authorized" shape.
  Confirming existence to an unauthorized caller is itself a leak;
  `visibility::not_found_response` is the single byte-identical response
  every one of these sites returns, and its own test serializes both cases
  and compares the bytes.
- **The §11 honesty boundary, restated.** This layer is **privacy-grade
  isolation** — it protects against ACCIDENTAL cross-user exposure between
  cooperating users on one server (the stated goal: two users cannot see
  each other's sessions, memory, artifacts, or live events). It is
  explicitly **NOT malicious-member-grade**: a member is still trusted code
  execution inside the same process, sandbox, and filesystem as the owner.
  The hardening below (member default exec tier `Ask`, an explicit
  `tools.invoke` allowlist starting at `team_from_template`, `memory_search`
  denied to members) raises the cost of a hostile member; it does not
  remove that trust assumption. `role-aware per-tool tool_permissions` was
  considered and dropped as YAGNI (R10) in favor of this narrower set.
- **The operator is a privileged content reader by design, and those reads
  are audited** (human ruling, 2026-08-07). The predicates above are about
  IDENTITY: nobody, operator included, gets a session they do not own out of
  `sessions.list`, `chat.history` or the event bus — which is what this
  layer's acceptance test means when it says "the operator is not exempt from
  session ownership" (`isolation_acceptance.rs`), and it is a statement about
  those surfaces, not about the operator's total reach. The operator ALSO has
  a debugging surface those predicates do not answer for: `trace.list` /
  `trace.get` return any run's persisted prompts, tool inputs and tool outputs,
  admin-gated instead of owner-scoped, and that is deliberate — an operator who
  cannot read a failing member's run cannot support it. The half that was
  missing was accountability, not authority: such a read now emits one
  `AuditEventType::ScopedContentRead` into `security_audit_log`, naming the
  actor (`actor_user`, the column this ruling added), the session read and the
  run — never the content. Reading your OWN trace is not an event, so a
  single-user box records nothing; the predicate is
  `visibility::session_visible_to`, not owner-equality, so a project room's
  members reading their own room's runs are not filed as cross-user readers
  either. Pinned by `trace_replay.rs`'s
  `trace_get_of_another_users_run_is_served_and_audited` (which asserts the
  bytes still arrive — this is a ratification, not a new denial) and its three
  negative siblings. There is deliberately **no query API and no separate
  retention policy** for these rows: the drain's existing horizon applies and
  the table is read with SQL, per R10.
- **A caller-supplied key is a gate on WRITES too** (2026-08-07). Four
  surfaces took an addressed key and never asked whose it was; each is now
  closed with the predicate its own in-file neighbour already used —
  `sessions.set_topic` and `chat.context_estimate` through
  `visibility::session_visible` / `existing_session_is_visible`, and
  `workspace.{create,update,archive}` through `partition_visible`. Two
  descriptions are corrected in passing, because they are the reason these sat
  open: `sessions.set_topic` is a cross-user WRITE, not a read, and
  `chat.context_estimate` is not "a token-count-only read" — it resolves the
  addressed session's pinned model to return `window_tokens` (a model-identity
  oracle on someone else's session) and its `Some`-vs-`None` answer is an
  existence oracle, the exact thing `not_found_response` exists to deny. Read
  the workspace half narrowly, exactly as its handlers do: a workspace id is a
  user-chosen name encoding no owner and `agent_envs` has no owner column, so
  the gate buys defence in depth against partition-composed ids
  (`main__u-alice`) and nothing more. **The 2026-08-08 real-machine QA
  exercised that residual and it is a WRITE, not only a read**: a member
  renamed and then archived a workspace the operator had just created, both
  returning `ok`. Earlier wording here and in the handler named only
  "read another's `env_vars`", understating it by a verb class.
  **Closed 2026-08-08 — at the admin gate, not with an owner column.** The
  wording immediately above used to say closing it needed a schema change and
  a product decision; re-scouting the family found it needed neither. The
  `workspace.` family has exactly one client and that client is already
  operator (`aleph workspace list|create|archive`, over loopback); the Panel
  has none — `interfaces/webchat/src/api/workspace.rs` records that its
  `workspace.list` call was removed long ago — and `workspace.update` /
  `workspace.get` have no client anywhere.
  *(Client census as of 2026-08-08. Since 2026-08-09 there are two clients:
  `get`/`update` gained CLI subcommands, and the Panel gained
  `/settings/workspaces`. **The ruling is unchanged** — what carries it is that
  no client is a member surface, not that there is only one. That page renders
  for anybody, calls, and reports this gate's refusal through
  `components::admin_refusal` instead of showing a member an empty roster. The
  file named above is now `api/agent_binding.rs`; `api/workspace.rs` is the
  real family.)*
  Nor was the read half worth what it
  looked like: `env_vars`, `system_prompt_override` and `allowed_tools` have
  no writer on `agent_envs` at all, so cross-user "reads" returned empty
  columns; the write half was the whole of the real residual. So the family
  joined `method_admin::ADMIN_PREFIXES` **whole, with no carve-out** — a
  `MEMBER_CARVE_OUTS` entry for the reads would have been a zero-consumer
  opening. An owner column would have built a permission model for per-user
  workspaces, which no surface currently lets a member use; it can layer on
  top of this gate if that product ever lands.
  Two consequences worth stating, because each is the kind of thing that
  drifts: **(1)** the five methods **left `method_visibility::SCOPED_METHODS`**
  in the same change — that table's contract is per-user filtering on surfaces
  a *member* reaches, so keeping a claim there would have gone stale, and the
  same ruling already applies to `trace.list`/`trace.get`/`agents.teams`. The
  absence is pinned in both directions (`treatment_of == None` **and**
  `method_requires_admin == true`), so reopening the family to members fails a
  test by name and forces the entries back. **(2)** the handlers **keep**
  calling `partition_visible` and it is not dead code: a second
  `UserRole::Admin` principal connects with `CALLER_USER = Some(their own
  id)`, not `OWNER_USER_ID`, so `main__u-alice` is still refused to an
  operator who is not alice.
- **Session `owner_user_id` / `scope_id` were never stamped on any run path
  until 2026-08-08, and the P1/P2 predicates that read them were therefore
  inert.** `SessionMetadata::stamp_attribution` reads `scope::current_scope()`
  on `get_or_create`'s CREATE branch, and every producer of a run hands the
  request to a `tokio::spawn`ed task, so the ambient scope was `None` exactly
  where the row was written — even though `build_run_request` had already
  resolved the attribution into `request.metadata`. Every session persisted
  with both columns NULL and was adopted as owner-owned. Two consequences worth
  keeping: the member-facing symptoms all read as *missing features* rather
  than as a leak (own sessions absent from `sessions.list`, own session
  "not found" to `sessions.set_topic`), and the operator-read audit above could
  never fire, because `caller_could_reach` compared against an `effective_owner`
  that was always the operator. Fixed by
  `run_loop::ensure_session_under_request_scope`, which re-derives the scope
  from the metadata (not from a captured task-local — `current_scope()` is
  `None` in the gateway dispatch loop too, so the metadata map is the only
  place the resolved attribution exists).
- **`OPEN_LOOPS.md`'s `proj-` handling is settled, not an open gap.**
  `session_reflection::open_loops_path` resolves through
  `memory::project_scope::session_write_id(agent_id, false, None)`: the legacy
  project-DIRECTORY feature is deliberately not threaded onto this write path,
  because there is no live `current_project_root()` task-local at session-close
  time to resolve it with. The READ side (`capture_curated` via
  `resolve_storage_id`) is pinned to the same `false`/`None`, so the two agree
  and a non-personal session falls through to the base id exactly as it did
  before P1 — personal scope, which is P1's actual mandate, still applies on
  top. This has been re-triaged as an isolation gap more than once; it is not
  one. Widening it is a feature decision about the legacy project-directory
  namespace and needs a persisted project root on the session-close path first.
- **Round 2 (2026-08-08) — the faces that had no predicate on them.** The full
  per-item record is FEATURE_LOCATOR §5.22; what belongs here is the shape and
  the three rulings a future change must not undo.

  The organising question was **"which face was never asked?"**, because P0–P2
  got the predicates right and installed them somewhere not every path crosses:

  - the login wall was on the request arm only — a connection has two
    directions, and the event-forward arm had four terms, none of them
    authentication. Combined with `pty.output` classifying as `Global`, an
    unauthenticated LAN socket received the operator's raw shell bytes while
    `"pty."` had been in `ADMIN_PREFIXES` all along. (`pty.output` no longer
    exists: the embedded-terminal work replaced the raw-byte topic with the
    row-diff `pty.screen`. The name is kept here because it is what the defect
    happened to — grep `PTY_SCREEN_TOPIC` for the topic that carries terminal
    output today, and note the shape is unchanged: it is still a topic whose
    payload is the operator's shell.)
  - `partition_visible` had only the task-local way of naming an actor, which is
    dead inside a spawned run — so every tool reaching for it would have got a
    silent always-true. It now has an explicit-actor twin
    (`partition_visible_to` / `project_visible_to`), like
    `session_visible` / `session_visible_to`;
  - `users.create` / `users.update` had only a server half. **No shipped surface
    could create a second person**, which made every predicate P0–P2 built
    unreachable in practice. Closed by `aleph users` + `pair --user`.

  Three rulings to preserve:

  1. **The approval gate is member-reachable, on purpose.** Closing both its
     faces did not restrict members — it pushed them past the gate: a blocked
     run died at the 120s timeout and the recorded workaround was
     `exec_tier: "full"`. `exec.approvals.pending` / `exec.approval.resolve` are
     carved out and filtered by `session_visible`; a fleet approval (empty
     `session_key`, raised by a cluster node) stays operator-only on both faces.
     Re-closing them without also removing the member's ability to start a
     gated run recreates the inversion.
  2. **`approval.` has no `EventScopeGuard` prefix rule, and must not get one
     back.** That table keys on the topic prefix and the family carries two
     kinds of frame; the decision moved to `event_visibility`, where the payload
     is visible. Re-adding a prefix rule re-closes the member half without the
     fleet half noticing.
  3. **The Panel does not gate on role.** §5.22 ⑥'s ruling stands (a role is
     latched at connect and `restamp_live_connections` changes it silently, so a
     UI gate can never be an enforcement point). What changed is that a client
     which will not gate is now required to **report accurately**:
     `interfaces/webchat/src/components/admin_refusal.rs` tells a refusal apart
     from an empty answer, keyed on the same `ADMIN_REQUIRED_MESSAGE` the server
     emits.

- **Known gaps — all four CLOSED 2026-08-09; kept here with what closing each
  one actually turned out to require:**
  0. **The exec-tier id CATALOG.** ✅ Closed — and it turned out to be closed
     already at the wire: `config.get_tool_permissions` is in
     `method_admin::MEMBER_CARVE_OUTS` and the handler narrows the response
     rather than refusing it, so a member receives all four id enumerations
     (`exec_tier`/`tiers`/`mode`/`modes`). What was missing was a guard and two
     stale docs claiming the opposite.
     `aleph_protocol::tool_permissions::OPERATOR_ONLY_KEYS` is defined by
     REMOVAL. ⚠️ **The sentence that stood here until 2026-09-03 — "the four
     fields are protected by nothing but that list staying short; adding
     `"tiers"` to it would compile and pass every test" — is MEASURED false,
     and it was already false when it was written.** Moving `"tiers"` into
     `OPERATOR_ONLY_KEYS` reddens `config::tests::
     a_member_still_receives_both_dials_and_both_catalogues`; emitting a field
     named in neither shared list reddens
     `the_emitted_key_sets_are_exactly_the_declared_wire_contract`. Those two
     tests are the protection, not the length of the table. Defined-by-removal
     also means a new field arrives **member-VISIBLE** and someone has to rule
     it withheld — the opposite of what an earlier version of this entry said.
     The local `MEMBER_WITHHELD_KEYS` mirror in `handlers/config.rs` is gone
     (one spelling, in the crate that owns it), together with the doc that cited
     a guard `git log --all -S` proves never existed.
  0b. **`surface.approval` (the R5 banner).** ✅ Closed by giving the frame the
     `session_key` it is derived from. It was `Global` + role-gated not because
     a banner is fleet-level but because `r5_router::approval_for` dropped the
     field on the way through, leaving nothing to scope by. It now classifies
     exactly like its three siblings (`BySessionKeyOrAdmin` / `OperatorOnly` on
     an empty key) and its `EventScopeGuard` prefix rule is **gone** — keeping
     both would have re-closed the member half, which is the shape recorded as
     mine K. Symptom it fixes: the person whose own tool call is parked received
     the decision card and never the interrupt whose job is to fetch them to it.
     **Honest scope of the fix:** the frame carries `audience: ["desktop"]`, so
     `audience_allows` still restricts it to connections whose surface kind is
     `Desktop` — the shell's OS-notification bridge, never a browser Panel — and
     that bridge hard-codes `ws://127.0.0.1:18790/ws`. Every consumer alive
     today is therefore a loopback connection, which resolves to operator before
     any of this is consulted, so the classification change moves nothing on a
     shipped install. It is correct rather than useful: the previous state was
     "gated by role because the payload lost its owner", and that is the state
     that would have quietly excluded a member the moment the shell learned to
     point at a remote core. **The real remaining gap is one level out** — there
     is no member-reachable interrupt surface at all, because the only R5 banner
     transport is a loopback desktop bridge. Closing *that* is a product
     decision (an in-Panel banner, or a shell that can address a remote core),
     not a visibility fix.
  0c. **Legacy room attribution.** ✅ Backfilled. The predicate was exactly
     derivable — `ProjectStore::claim_session_key` is the sole writer of
     `projects.current_session_key` and writes only `WHERE current_session_key
     IS NULL`, so a legacy room's session key is DECLARED in a second table
     rather than guessed. `SessionStore::backfill_attribution` (both backends)
     writes the pair only `WHERE owner_user_id IS NULL AND scope_id IS NULL` —
     the guard is in the statement, not the caller, so it can never re-scope a
     session somebody owns. Driven once at boot by
     `projects::backfill_legacy_room_attribution`, before any handler is
     reachable. Of the two columns only `scope_id` mattered: `owner_user_id`
     NULL already reads as the default owner, while `scope_id` NULL reads as
     "org-era personal session", so `project_visible` never fired and the room's
     own conversation was invisible to its whole roster. Personal pre-P1
     sessions remain genuinely unrecoverable.
  0d. **Secret masking beyond the vendor-pattern class.** ✅ Closed by
     `[[security.mask_patterns]]`, installed onto the **type**
     (`exec::masker::install_operator_patterns`) rather than threaded to a
     construction site — `SecretMasker::new()` has seven production call sites
     and configuring one would have redacted one leg while six spelled the
     secret out, with no test able to see it. Invalid regexes are logged at
     `error`, never dropped silently: a redaction pattern that failed to compile
     looks exactly like an operator who never configured one. `[security]` is
     not a live-reload section, and `ReloadImpact::classify` already says so.
  1. `chat.send`'s Simulated-execution fallback path (used only when no LLM
     provider is configured — `AgentRunManager::start_run`, which has no
     `SessionStore` dependency) is not covered by the real-provider path's
     `existing_session_is_visible` check.
  2. **The running-set projection raced a new session's row into existence,
     and the self-heal its own comment promised had no producer** (was high; a
     regression this round introduced, and it fired on a single-user loopback
     box too — **resolved**, see the end of this item).
     `SessionRunRegistry::try_claim` is the FIRST statement of
     `ExecutionEngine::admit_run` and it broadcasts `RunningSetChanged`; the
     session row is not written until `agent.ensure_session(...)`, at least two
     await points later. `EventVisibilityIndex::project_for` resolves each
     element through `session_admits`, which returns false on `Ok(None)` —
     deliberately, since "I could not work out whose this is" must never mean
     "everyone's" — so the brand-new key is DROPPED and the socket receives
     `{seq: N, running: []}`. `SessionMap::set_server_running` records seq N
     with an empty set, and the next `RunningSetChanged` is the RELEASE at run
     end, which also excludes the key; nothing else re-fetches
     (`seed_server_running` is cold-load-on-mount only, and the running dot is
     documented as purely server-authoritative). Net: the sidebar dot and the
     "active" counter stay dark for the entire first turn of every NEW
     conversation — behaviour that worked before this round, when the frame was
     forwarded whole. The RPC twin `visible_running_keys` drops the key for the
     same reason, so the cold-load seed does not cover it either, and
     `gateway_metrics.rs`'s "it self-heals on the next
     `stream.running_set_changed`" was false whenever there is only one
     in-flight run.
     **Resolved (2026-08-07) by giving the promised self-heal a producer, not
     by relaxing the drop rule** (an element whose owner does not resolve is
     still dropped — widening that would admit another user's imminent run, and
     the key string itself names an agent/peer). `execute.rs` calls
     `SessionRunRegistry::republish_running_set()` immediately after
     `agent.ensure_session(...)`: the same set, re-published at a FRESH seq now
     that the key resolves. The bump is load-bearing —
     `SessionMap::set_server_running` discards any frame whose seq is `<=` the
     one it holds, so a re-publish at the claim's seq would have been inert.
     The RPC twin `visible_running_keys` is unchanged and still drops a key
     polled inside that window; it needs no change because the repair frame
     arrives on the event plane and outranks the cold-load seed
     (`seed_server_running` applies only while `server_seq == 0`). Pinned by
     `session_run_registry.rs::a_new_sessions_key_reaches_its_owner_once_its_row_exists`
     (asserts the projected payload and the seq ordering) plus a source-level
     pin that `execute.rs` re-publishes AFTER the row is created — the full
     `ExecutionEngine::execute` has no unit-level harness, so without it the
     wire could be deleted with every test still green.
  3. **The team event plane's resolver was installed under a narrower condition
     than the one that produces the frames it classifies** (resolved in-round,
     kept here for the criterion it teaches). `event_visibility::event_admits`
     resolves `team.<id>.*` through `ConnectionContext`'s `TeamStore`, and
     `teams: None` denies. That handle was installed by
     `server.set_team_store(...)` inside `start/mod.rs`'s
     `if let (Some(ref ts), Some(ref cs)) = (&agent_result.team_store,
     &agent_result.coord_task_store)`, while the PRODUCER — the
     `teams.chat.send` registration driving `GroupChatBroadcaster` /
     `publish_team_event` — sits under `builder/agent_init/mod.rs`'s
     `if let Some(ts) = team_store.clone()`, i.e. `team_store` ALONE. The two
     stores open two different databases from two different files (`teams.db`
     vs `coord.db`), and coord init returns `(None, None)` on any of three
     warn-and-continue paths (data-dir resolve, `open_sqlite_safe`, `migrate`).
     So a corrupt, locked or unwritable `coord.db` — an explicitly supported
     degraded boot that prints "Task coordination tools disabled" and carries
     on — left team chat publishing frames that EVERY connection, operator
     included, denied: no error, no log line at the denial, zero live frames.
     Note the direction was fail-CLOSED, which is why nothing surfaced it: the
     leak this round closed stayed closed, and the cost was a total silent
     outage instead. `set_team_store` now hangs on `agent_result.team_store`
     alone (`register_teams_handlers` stays under the two-store tuple — it
     genuinely needs both), pinned at source level by
     `event_visibility.rs::the_team_resolver_gate_is_no_narrower_than_its_frame_producers_gate`,
     which reads the enclosing `if let` at both sites and asserts the resolver's
     `*_store` set is a SUBSET of the producer's. **The durable criterion: a
     classifier's resolution handle must not be gated more narrowly than the
     frames it classifies are produced** — narrower means the whole feature goes
     dark in the fail-closed direction, for everyone, with nothing logged. It
     generalises past this one handle: any `ConnectionContext` dependency a
     visibility predicate DENIES on is a second, independent switch that can
     turn a feature off from a wiring site nobody associates with it.
  4. `slash_command.rs::execute_direct_tool` (the `/toolname` L0 fast path) was
     recorded here as "bypasses `ScopedToolService` entirely, with no
     allowlist". That description was wrong and is corrected rather than
     carried forward. `slash_gate_reason` builds the SAME `ToolFacts`
     `ScopedToolService` builds and runs the SAME `effective_permission`, then
     applies the argument-keyed destructive filter, the declared
     `requires_confirmation` gate (at every tier, `Full` included), and — for a
     non-operator caller — `method_authz::tool_requires_operator` plus the
     dangerous-tool floor. A call that trips any clause is not run ungated: it
     returns `ExecutionError::Fallthrough` into the full agent loop, where
     `ScopedToolService` re-decides it with the real facts and can raise the
     approval card. The residual is narrower and different in kind: this path
     CONSTRUCTS its own `ToolFacts`, a second statement of one fact, so a third
     input added to that struct in `tools/scoped/builder.rs` alone leaves the
     fast path deciding on a stale shape — silently, and only for slash
     commands. Converging the two constructions is the open work.
  5. `gateway.metrics.run_concurrency`'s `per_agent` was the one un-narrowed
     identity array in a response this round made member-reachable (was low —
     **resolved**). The handler narrowed `running_sessions` and
     `busy_queue.per_session` but passed `ConcurrencySnapshot::per_agent`
     (`Vec<AgentSlotUsage>`, i.e. `{agent_id, in_use}`) through verbatim, so a
     member learned which agent personas have live runs right now — the same
     class of fact ("who is doing something at this moment") the session-key
     narrowing removed. `agents.list`/`agents.get` are themselves member
     carve-outs, so the ids were not new, only the live-activity correlation.
     **Resolved (2026-08-07)**: the handler now removes `per_agent` from the
     response whenever `visibility::visible_owner_filter()` is `Some` — the
     same predicate that narrows the two session arrays — and an unrestricted
     (internal / operator) caller still receives the whole snapshot. What a
     member gets is COUNTERS only (`global_in_use` / `global_total` /
     `per_agent_cap` / `waiting` / `busy_queue.total_waiting`). The carve-out's
     justification in `method_admin.rs` was corrected to say so rather than
     carrying an exception in prose, which is how the next reviewer gets told
     to skip it. Pinned by
     `gateway_metrics.rs::the_per_agent_breakdown_is_withheld_from_a_scoped_caller`,
     which proves the "present unless dropped" premise in the same test.
  6. The no-existence-oracle property of the `workspace.*` writes was stated but
     not pinned (resolved in-round; the shape is worth keeping).
     `the_workspace_writes_deny_a_foreign_partition_composed_id` used to archive
     the same composed id twice as the same non-owner and compare the two
     serialized responses — but both calls took the identical
     `partition_visible` deny branch before touching the store, so the assertion
     was literally `f(x) == f(x)`: it could not fail, and the id it called
     "something that never existed" had been created earlier in the same test.
     It now archives a genuinely absent composed id, creates it, archives again,
     and compares — `f(x, absent) == f(x, present)`, which an existence oracle
     inserted into the deny branch actually breaks. **The criterion: a
     byte-equality assertion is only a no-oracle proof if the two sides differ
     in the state being probed.** Two calls that take the same early-return
     branch prove nothing, and they read exactly like a real guard.
  7. The admin gate's refusal wording is now ONE constant, not a Panel copy of
     a server literal (resolved). `settings/network/cluster.rs` used to
     transcribe the server's refusal string and claim a doc comment's worth of
     drift protection it could not deliver: `aleph-panel` does not depend on
     `alephcore`, and its test fed its OWN constant into `fleet_error_label`, so
     rewording `handler.rs` would have stranded every member who opens the
     cluster page on the raw English protocol string with both crates' tests
     still green. The wording moved to
     `aleph_protocol::jsonrpc::ADMIN_REQUIRED_MESSAGE` — the one crate both
     sides already depend on — and both the emit site (`server/handler.rs`, via
     `gateway::protocol`'s re-export beside `AUTH_REQUIRED`) and the match site
     read it, so there is no reword that moves one side without the other. The
     Panel test now feeds that shared constant in, which is what makes it able
     to fail: drift the Panel's recognition away from the server's words and the
     refusal falls through to the raw string, tripping its `assert_ne!`. The
     residual is the ordinary one — someone re-inlining a literal at either site
     — and no test catches that.
- **Explicitly out of scope for P1**: pushing routing/notifications TO
  members (spec §8, P3).

### 项目房间层（P2）{#project-rooms-p2}

P1 answers "can this identity see this row"; this layer adds the first
SHARED scope with more than one legitimate reader-writer. Landed as the P2
project-rooms plan (`docs/superpowers/plans/2026-08-06-p2-project-rooms.md`):
`projects` promoted to a SQLite entity with `owner_user_id` + a
`project_members` roster, sessions openable in `ScopeId::Project`, memory
routed to the room partition (`p-*` suffix family), per-message author
attribution, and a bound workspace as the room's default cwd.

- **Membership IS the authorization model — there are no per-resource
  grants.** One roster row answers every question for that project: session
  visibility, memory-partition visibility, event delivery, RPC access. There
  is no per-session ACL, no per-note sharing, no capability tokens inside a
  room. Adding a member grants everything the room contains at once;
  removing them revokes it **across every one of those four predicates at
  once**, immediately — the roster projection (`src/projects/roster.rs`) is
  published by the store inside its own write lock, so the next predicate
  evaluation already excludes them. Pinned by
  `visibility.rs::removing_a_member_revokes_visibility_immediately`.
  Anything needing finer sharing than "in the room / not in the room" is a
  different feature, not a variation of this one.

  **Corollary, and it is not obvious: deleting the roster does not delete the
  room, it hides it from everyone.** Because membership is the entire
  predicate, dropping `project_members` leaves every session scoped to that
  project unreachable by every principal alive — the operator and the room's
  creator included. Not deleted: the rows stay on disk, and the `main__p-<id>`
  memory partition keeps being enumerated and nightly-maintained by
  `list_scoped_agent_ids`, which reads the filesystem and cannot know the room
  is gone. `ProjectStore::remove` did exactly this (its semantics predate P2 —
  "drop a folder from the recents list"), so since 2026-08-09 it **refuses a
  project that has a claimed session** and points the caller at
  [`archive`](#), which keeps the roster and therefore keeps the conversation
  reachable. A pre-P2 catalogue entry — no claimed session — removes exactly as
  before. The refusal is `INVALID_PARAMS`, not not-found: the caller owns the
  project and named it, so there is no existence to leak, and a refusal that
  names the working verb is the only kind that is actionable.

  **The comparison that produced this** (2026-08-09, vs rowboat's
  `DeleteProjectUseCase`): rowboat answers "who can still reach this after the
  row is gone" with an explicit cascade over ten repositories — api keys, data
  sources, Qdrant vectors, conversations, jobs, two kinds of job rule, members,
  the project. Aleph adopts the QUESTION and rejects the shape: a cascade would
  become a second source of truth for what a room contains, which is precisely
  what one-roster-answers-everything exists to avoid. Making the verb
  unavailable for rooms answers the same question with no second source.

- **There is no per-user permission state, anywhere.** The `users` table is
  `(user_id, display_name, role, status, created_at)` and nothing else; the
  role enum plus the project roster is the complete authorization model.
  `EventScopeGuard`'s rules named four fine-grained permissions (`pairing`,
  `guest.manager`, `exec.approver`, `config.viewer`) that no producer could
  ever mint — `scope_for_role` returns `["*"]` or `[]` — which cost nothing at
  runtime and a great deal to any reader who concluded a grant system existed
  and designed a narrow permission against it. Removed 2026-08-09; pinned by
  `event_scope::tests::no_rule_names_a_permission_nothing_can_grant`. Adding a
  real grant axis means adding its **producer first**.

  **Those four questions are the whole list, and BACKGROUND WORK is
  deliberately not one of them** (human ruling, 2026-08-07). A loop, goal, cron
  job or group-chat session started inside a room stays visible to the person
  who started it, never to the roster: `visibility::stamped_owner_visible` /
  `ambient_owner_visible` are owner-only by design — **a room does not own its
  members' background work.** The scope fact needed to answer otherwise is
  already persisted and deliberately unread on this path
  (`LoopState::scope_id`, `Goal::scope_id`, `CronJob::scope_id` all carry
  `project:<id>`), so it reads like a severed wire and is not one; the
  predicates' own doc says so at the site. The direction is fail-CLOSED, so
  "the room can't see my loop" is a decision, not a bug — widening it is a
  product change and needs a ruling first, and the shape would then be ONE
  scope-aware sibling delegating to the same `roster::is_member`, never a
  second inlined membership check at the call sites.

  **The promise is scoped to the predicates, and there is exactly one other
  ingress.** `/artifact/<cap>/<id>/<file>`
  (`src/gateway/server/artifact_route.rs`) is a **bearer** byte route: its
  eight guards contain no identity check, and the capability names a SESSION,
  not a user, so removal has nothing to revoke there. An URL an ex-member
  minted through a then-legitimate `artifacts.list` / `session.export_html`
  keeps serving those bytes for the remainder of the capability's 8-hour TTL
  (`security::artifact_caps::CAP_TTL`); the same applies to a `users.update`
  deactivation, whose kick covers `devices.user_id` bindings (WS/Panel), not
  a URL already copied. That bounded bearer window is an accepted boundary,
  not a gap: the capability is session-wide, so revoking it on member removal
  would also break the REMAINING members' already-rendered `<img src>` URLs
  until their next `artifacts.list` re-mints. Pinned as a stated fact by
  `artifact_route.rs::a_removed_members_minted_artifact_url_survives_until_ttl`
  — a red test is how anyone reversing this decision will learn it was one.

  **Confirmed end-to-end on 2026-08-08, through the production mint path.**
  That test builds its `SessionMetadata` by hand and calls
  `ArtifactCapabilities::mint` directly, so it never shows that
  `artifacts.list`'s own `deny_unless_visible` admitted the holder — the gap a
  real-machine run had to close. It now has: a member chatted in a room, the
  model called `artifact_publish`, and the member read the capability URL out
  of their own `artifacts.list` response. Removing them from the roster
  produced **two different and both-correct answers**: the RPC face shut
  immediately (`artifacts.list` and `artifacts.read_text` each `-32009`,
  byte-identical to a session that does not exist) while the already-minted
  byte URL kept serving — the bearer window above. Fresh minting was
  impossible, because the only mint site is the `artifacts.list` that just
  refused. If a SECOND mint site is ever added, that last sentence stops being
  true and this paragraph has to be re-derived.
- **`owner_user_id` means CREATOR, not "the one who can see it."** The P1
  vocabulary (`effective_owner`, adoption-by-absence) keeps working for
  personal rows, but for a project row the owner column only decides
  owner-only verbs (rename, archive, roster mutation, workspace binding).
  Visibility is the roster, full stop. Any new predicate that reaches for
  `owner_user_id` to answer a can-see question re-opens the bug class P2's
  roster predicates (`projects::roster::is_member`, reached through
  `visibility::project_visible` / `session_visible_to` / `partition_visible`
  — and, since 2026-08-07, event delivery reaches the roster through the very
  same `owner_and_scope_visible_to` body rather than the owner-equality copy it
  had kept from P1)
  exist to prevent.
- **`not_found` vs `forbidden` — the boundary is visibility, not
  politeness.** A caller who cannot SEE the project (not on the roster) gets
  the byte-identical `RESOURCE_NOT_FOUND` of P1 — confirming existence is a
  leak (`gate_project`). A caller who can see it but lacks the ROLE for an
  owner-only verb gets an honest `PERMISSION_DENIED` (`require_owner`) —
  they already know the room exists, so "forbidden" leaks nothing and is
  actionable ("ask the owner"). Pinned by
  `a_stranger_binding_gets_not_found_not_permission_denied`.
- **"Admin-gated" here means NOT-A-MEMBER, not IS-AN-OPERATOR.** The
  predicate is `caller_identity::caller_is_member()`, which is literally
  `current_caller_role() == Some("member")` — so a caller with **no** role at
  all (`None`: cron, A2A, in-process) passes every one of these gates. That is
  the house rule across the repo and it is deliberate: the login wall handles
  the unauthenticated half, and an unrestricted internal caller is not a
  principal being judged. Say it precisely when reading any "admin-gated"
  claim in this document — a gate written as "is operator" and a gate written
  as "is not a member" agree on every human and disagree on every machine
  caller, and only the second one is what the code does.
- **The workspace binding is a privilege: every writer of `workspace_path`
  that CHOOSES a directory is gated, and exactly one is exempt by
  invariant.** *(This bullet used to carry the count itself, and it went
  stale exactly the way a count does: it read "four writers — three gated"
  from P2 until 2026-08-30, silently wrong from the day
  `project_manage(bind_workspace)` landed calling
  `ProjectStore::bind_workspace` directly rather than through the RPC. The
  number is therefore no longer repeated here — the authoritative census is
  `gateway::handlers::projects`'s module doc, and a copy of a count is a
  trap whether or not it happens to be right today.)* Turning
  `workspace_path` into the room's
  runtime cwd (a dormant display field waking up) retroactively made every
  writer of that column a directory-choice authority: `projects.add`,
  `projects.create_blank`, and `projects.bind_workspace` all carry the same
  `caller_identity::caller_may_choose_directory()` gate (config-tier OR
  loopback) — the same predicate `agent.run`'s explicit `project_root`
  param enforces. Without the write-side gate, "register a folder, then
  chat in it" is a two-step route to an arbitrary server directory in which
  both steps are individually legal.

  **The tool face is gated by a different predicate, on purpose.**
  `project_manage(bind_workspace)` uses
  `require_operator_tier()` — `TurnContext::caller_is_operator()` — rather
  than the ambient `caller_may_choose_directory()`, because the ambient
  task-local is dead inside a run (that is the very defect round-9's
  `546984c2b` fixed, and the reason this action could not ship before it).
  `TurnContext` is per **turn**, not per connection, and the gateway has
  already resolved the originating connection's authority into it by the
  time any tool executes. ⚠️ **Bound**: `require_operator_tier()` is
  `current_turn_context().is_none_or(|t| t.caller_is_operator())` — an
  ABSENT context reads as operator. That branch is unreachable today (both
  production call sites of `build_request_tool_service`,
  `run_loop/inner.rs:945` and `:1228`, pass `Some(turn_context)`, and
  `ScopedToolService::execute` scopes `TURN_CONTEXT` only in the `Some`
  arm) — but **nothing pins it**, so the sentence above holds for
  gateway-originated turns, not for "any tool execution" in general.
  Note the deliberate asymmetry: the check fires
  only when a path is actually NAMED, so **releasing** a binding stays
  reachable from a session that a bad binding broke.

  The one exempt writer is
  `execution_engine::run_loop::inner`, which auto-registers a run's
  `workspace_override` into the catalogue so a CLI/programmatic cwd appears
  in the picker; it is exempt because it never *introduces* a directory — it
  records the one the run is already executing in, granting no reach, only
  visibility in a list. That moves the authority question upstream to
  whichever producer set `workspace_override` (a gated `project_root` param,
  a gate-written room binding, a channel's configured `default_workspace`, a
  resumed or inherited workspace), so the rule has two halves: a new writer
  of `workspace_path` that *chooses* a directory owes the gate, and a new
  *source* of `workspace_override` owes a gate at its own choice point.
  Unbinding is a de-escalation and is
  deliberately exempt — it must stay reachable from the connection that got
  stuck. Members do NOT need the gate to *use* the room's binding: the
  owner chose the directory through a gated verb; the member only inherits
  that choice. The full census lives in
  `gateway::handlers::projects`'s module doc, with a back-reference at the
  exempt writer — and that doc also records that both it and this bullet sat
  at "four / three gated" for the whole time there were five, because a count
  of members is prose and prose has no compiler. Do not restore a count here:
  read the census, or grep `workspace_path` and `bind_workspace`.
- **Author attribution is display-grade, not signature-grade.** The
  `[name]` speaker labels a room prompt carries come from
  `SessionEvent::UserMessage.author_user_id`, stamped server-side from the
  authenticated caller — a member cannot forge the LABEL. There is
  deliberately no request parameter anywhere in the chain:
  `build_run_request` reads `caller_identity::current_caller_user()` into
  `AUTHOR_USER_KEY`, and every emission site takes the label from that key —
  `scope::room_author_from_metadata` for the four sites that hold the request,
  and `scope::ambient_room_author` for the three `session_seed` sites, which
  hold neither the request nor `CALLER_USER` and read a task-local instead.
  A turn that carries no author at all — a legacy row, or a channel-driven run
  whose inbound router stamps the scope but not the speaker — falls back to the
  room's own `owner_user_id`; that is a wrong-but-honest label on a turn
  nobody claimed, not a forgeable one.

  **The failure mode to watch for is the label degrading to that fallback on a
  turn that DID name its author**, because it degrades silently and the wrong
  answer is plausible: every member's run in a room carries the ROOM's
  attribution (that is what shares the memory partition), so the fallback names
  the session's creator on everyone's message. It has one cause — the author
  task-local being dropped at a boundary the scope survives. `with_room_author`
  is therefore seeded at exactly the two places `with_scope` is
  (`run_loop::with_request_scope`, and inside `orchestrator::dispatch`'s
  `tokio::spawn`), and any new spawn between a seeding point and an emission
  site owes the same capture-and-re-seed pair. Pinned across the real dispatch
  spawn by `tests/gateway_chat_room_author_across_spawn.rs`; a test that nests
  the two task-locals in one task cannot see this class of break. But message
  BODIES are unauthenticated prose: a member can still type
  `\n[someone-else]: …` inside their own message. Deliberate (recorded at
  `speaker_label`): room members are same-server operators under the
  single-layer trust model; rewriting user prose to defend against peers of
  equal privilege costs more than it buys.
- **The room's WORKSPACE tab is read-only, and that is a boundary, not a
  simplification.** `projects.workspace.list` / `.read` let a member browse
  and read the folder the room is bound to. Three properties hold it:
  reads are roster-gated exactly like every other room predicate; a path
  that escapes the bound root and a path that does not exist return the
  **same** refusal, so the tab cannot be used to probe the server's
  filesystem outside the room; and reads are capped (8 KB, binary refused
  rather than lossily decoded). What it deliberately does NOT have is a
  write verb — a room member editing the owner's checkout through a browse
  UI is a different privilege from reading it, and there is no request that
  needs it today.

  **The threat this tab makes concrete, which the binding gate did not:**
  before it, binding a room to an over-broad directory (`~`, a repo root
  holding secrets) only affected what an *agent run* could reach in that
  room. Now it is directly readable by every person on the roster, one
  click, no model in the loop. The gate on the binding
  (`caller_may_choose_directory()`) is unchanged and still correct — the
  owner chose the directory through a gated verb — but the consequence of
  choosing badly got larger. Bind rooms to the project folder, not to a
  home directory.
- **The room's member list reaches the model, and it is display-grade for
  the same reason the speaker labels are.** `RoomRosterLayer` renders
  `<room_context>` naming the roster and marking the owner. The names come
  from `nudges::speaker_label` — the same seam the transcript labels use, so
  a member cannot spell a display name that forges a second speaker in
  either place, and there is exactly one sanitiser rather than two spellings
  of one rule. It is bounded (24 names, then a count) because the block
  rides on every request for the whole conversation and the list is
  something another member can grow. A session that is not a project room
  renders nothing at all.
- **Model-facing room management goes through the same authorization as the
  RPC face, and stops short of the one verb that grants reach.**
  `project_manage` (R8) shares `projects::authz` with `handlers/projects.rs`
  — the same `project_for` (not-found for a non-member) and `is_owner`
  (owner or org admin) — so a room renamed in words and one renamed by a
  click pass the same gates. ⚠️ **That sentence was true of the module doc and
  false of exactly two rules until 2026-09-03**: the owner-removal protection
  and `require_known_user` lived only on the RPC face, so one tool call could
  empty a roster down to nobody — a room no one, not even an org admin, could
  see. Both are now predicates in `projects::authz`
  (`may_remove_member` / `is_active_principal` + `unknown_user_refusal`) that
  **both faces call**; sharing a criterion means sharing a derivation, not
  writing the same conclusion twice. A store error resolves to REFUSE on both
  (a failed lookup is not evidence of permission), and that arm has its own
  test because no other test reached it.

  Two consequences shipped with it. **A roster verb now reports what it
  changed**: `ProjectStore::remove_member` discarded the DELETE's affected-row
  count and always answered `Ok(())`, so a removal that removed nobody still
  wrote an `AuthorityChange` row and still pushed `affected_user` to every
  watcher; it returns `Result<bool>` and both faces write only on a real
  change. The tool face had **zero** audit rows before this, and the
  `AUTHORITY_PRODUCERS` census could not see the absence because both of its
  directions key on "this file already contains `authority_change(`" — a census
  keyed on presence is structurally blind to a silent face. **And the model can
  now name a member**: the only roster view a model has is
  `speaker_label` — names, no ids — so `member_add` / `member_remove` accept
  `user_id` **or** `user` (exactly one; supplying both is refused, not
  guessed), resolved through `authz::principal_id_for_name` over the
  status-aware `aleph_protocol::users::resolve`. An ambiguous name **refuses**
  and names every candidate, and resolution happens **after** the ownership
  check — the ambiguity message names principals, so resolving first would let
  a refused caller read the directory. `bind_workspace` is **absent from the tool on
  purpose**: it is a writer of `workspace_path`, the gate it needs is
  `caller_may_choose_directory()`, and that predicate is fail-OPEN for a
  caller with no connection role — which on a tool face is a model running
  under cron or A2A. Writing a stricter predicate just for this face would
  be a second answer to "may this actor name a server directory", and the
  two answers would drift.
- **The §11 honesty boundary applies unchanged.** Project isolation is
  privacy-grade, exactly like P1 — it prevents ACCIDENTAL cross-room and
  cross-user exposure between cooperating users. All three §11 hard
  boundaries hold for rooms too: members share one process and one OS
  account; the vault is org-level, not per-room; org-tier memory remains
  org-shared. A room does not partition the sandbox, the filesystem, or the
  credential store — two rooms with bound workspaces are two directories,
  not two trust domains.
- **"Is this my own update?" is answered by run, not by channel.** A room is
  the first surface where two different Panel connections legitimately watch
  one session, and the frame that tells a Panel to re-hydrate
  (`stream.session_updated`) used to be judged by `origin_channel`. That field
  is a surface CLASS — every Panel connection hardcodes the literal
  `"gui:chat"` (`api/chat.rs`) — so reading it as "mine" said "mine" for a
  second tab of the same user and for every other member of the room, and
  their turns never appeared until someone reselected the session. The frame
  therefore also carries `origin_run_id`: the run that caused the update,
  stamped at all five publish sites (`ExecutionEngine::publish_session_updated`
  and its `SimpleExecutionEngine` twin; the sixth publisher,
  `SessionManager::emit_session_updated`, is a topic/title edit no run caused
  and carries neither field). A client re-hydrates iff the run is one its own
  `chat.send` did not return. This buys no new exposure: the frame is
  classified `BySessionKey`, the same audience `RunAccepted` already hands the
  identical run id to. It is deliberately NOT a per-connection or per-device
  id — a run id fixes two tabs of one user as well, and mints no new identity
  concept.
- **Known gaps (deliberate, recorded, not silently dropped).** ⚠️ **A
  known-gap record is itself a claim a later fix can quietly invalidate — two
  entries below had already rotted when this table was last rewritten
  (2026-08-30). Re-verify an entry in the code before acting on it.**
  1. **`projects.channel.bind`/`unbind` have no tool surface (R8 gap).** A
     channel conversation can be bound to a room over RPC (Panel) or
     `aleph projects channel bind`, but not by conversation.
     *(This entry used to read "`projects.*` has no tool surface" and was
     false from 2026-08-25, when round-8 shipped `project_manage`; round-9
     then added its ninth action, `bind_workspace`. Only `channel.*` is
     left.)* Two reasons it is not a simple addition: `bind`/`unbind` are in
     `method_admin::ADMIN_METHODS`, and the tool face runs **inside a run**
     where the ambient role predicate is dead — `bind_workspace` could only
     ship once it had `require_operator_tier()` to read `TurnContext`
     instead. `channel.*` needs the equivalent answer, and inventing a
     second, stricter predicate for one face would be a second answer to
     "may this caller point the server at something".
  2. **Channels into rooms: the INBOUND half is delivered (round-9); three
     things at its boundary are not.** A channel group conversation bound to
     a room now carries that room's scope all the way to the harness — memory
     partition, `<room_context>`, speaker label — and `rescope_attribution`
     moves the conversation's existing rows in so roster members see it in
     `sessions.list`. What remains:
     - **No room → group push.** Only the inbound direction exists. A turn
       started from the Panel inside the room does not reach the bound group.
     - **An unpaired speaker takes no scope at all, and lands in the BARE
       base partition.** Measured, not inferred
       (`qa/rooms_channel_bind` scenario 4): the note goes to `main`, not to
       the room and not to any principal. That is not "nowhere":
       `project_scope::session_read_ids` puts the bare base in **every**
       scoped principal's read set, so an unpaired stranger's turn in a bound
       group is readable by every member's recall. Pre-existing
       adoption-by-absence shape, unchanged by this round; recorded here
       because binding a group is what makes strangers reach it.
     - **A re-bind with no `--label` silently clears the stored label.**
       `bind` otherwise reads as an idempotent no-op; this half is not. The
       fixture records it as a fact rather than asserting it.
  3. `resume_coordinator::retrigger` does not re-check the binding: a
     resumed room run whose folder vanished degrades to the agent workspace
     (background sweep, nobody to tell) where `build_run_request` refuses
     loudly (a human is there). The asymmetry is deliberate and documented
     at both sites.
  4. `[projects] allowed_roots` (the `fs.*` browse fence) is NOT layered
     onto the directory-choosing binding writers (whichever ones the census
     currently lists — this line said "the three" while there were four) —
     the tier gate above is the only fence. Layering it on would change
     existing picker behaviour; a separate product decision.
  5. Teams created inside a room BEFORE the `teams.scope_id` column existed
     (2026-08-18, round-5) keep their pre-column semantics: the row cannot
     say whether it was made in a room, so it is not backfilled and stays
     creator-owned. New room-created teams carry `project:<id>` and belong
     to the roster. If a legacy room team must be shared, recreate it from
     inside the room.
  6. **A session listing can come back short, and the receipt built on it
     asserts an absence.** `projects.channel.bind`'s `NothingToMove` says no
     transcript row was found for the conversation — a confident factual
     claim, and only as honest as `list_sessions` is complete. Both backends
     can under-report, and they do it differently:
     - **SQLite (opt-in: `[general] session_store_backend = "sqlite"`; the
       shipped default is `"file"`, see
       `config::types::general::default_session_store_backend`, dispatched in
       `bin/aleph-server/commands/start/helpers.rs::initialize_session_store`)**
       — `session_manager::ops::query::list_sessions` collects with
       `rows.filter_map(|r| r.ok())`. A row whose column mapping fails is
       dropped in **complete silence**, so one damaged `sessions` row yields
       `NothingToMove`. **Deliberately not fixed in round-9**: it is
       pre-existing, has other callers, and re-classifying it is its own
       task. **More severe in kind** -- it drops the row with no diagnostic at
       all, where the file backend at least warned on one arm -- but
       **smaller in blast radius, because it is opt-in**. Round-9 fixed the
       half a stock install actually runs.
     - **File backend** — the same class, now loud on both arms. An
       unparseable `metadata.json` was already skipped with a named `warn!`;
       an **unreadable** one was skipped in silence six lines above that
       argument, which applied to it verbatim. Fixed in round-9
       (`8e6134398`): `ErrorKind::NotFound` still skips silently — the
       `exists()` check above it raced a delete, so that really is an
       absence — and every other kind now warns by path. ⚠️ **No test
       covers either `Err` arm**; the change is compile-verified and argued,
       not asserted. Whoever next touches this function owes it one.
       ⚠️ **And the silent half is narrowed, not closed**: the "it really is
       an absence" argument rests on the `exists()` check above the read,
       and `Path::exists()` is `metadata(path).is_ok()` — it answers `false`
       for **any** error. A session directory the process can stat but not
       traverse is skipped there and never reaches either arm, silently.
       That is the shape root `CLAUDE.md` §8 names («我看不了» answered as
       «那里没有东西»); closing it belongs to the `exists()` call.
     - **Inherent to any snapshot** — a turn whose routing resolved before
       the bind committed, but whose session row is created after the
       listing, is stamped `personal:<speaker>` and never seen.
       `stamp_attribution` is create-only and `backfill_attribution` heals
       only NULL rows. The window is milliseconds and the remedy is free:
       re-running `bind` on the same room is a documented no-op that re-runs
       the scan.
  7. **Single-project RPC responses have no typed envelope** (Ruling BD).
     `projects.list` gained `ProjectListResult`; the single-project
     responses did not — the server writes the literal
     `json!({ "project": … })` in **6** places and the Panel reads
     `.get("project")` in **5**, all of the latter in
     `interfaces/webchat/src/api/projects.rs`. The criterion that motivated
     the list change applies verbatim (信封也是 wire key，而且它通常是最后一个
     没被类型化的部分). Deferred because 11 sites across two crates is a task
     rather than a nit, and it is pre-existing. **Counts re-verified
     2026-08-30 and unmoved; re-verify again before acting.**
  8. **`AuthorityChange` is used more broadly than its own doc describes**
     (Ruling BJ). `daemon.shutdown` is logged as an `AuthorityChange`, while
     that variant's doc says it covers "changes who can do what". The ruling
     is **record, do not act, and the likelier defect is the doc**: a
     shutdown ends every connected session, which revokes everyone's ability
     to do anything by any reading not purely about grants. The census row
     is accurate and stays; narrowing prose beside a broader practice is
     this repo's most common drift shape.

### Network boundary = reachability

- **Default — loopback only.** `aleph-server` binds `127.0.0.1`
  (`GatewayServerConfig::default`), so only processes on the same machine
  can connect. A single-machine desktop install needs zero configuration and
  is auto-authorized as operator.
- **LAN opt-in.** Set `[gateway] host = "0.0.0.0"` in
  `~/.aleph/config.toml` to listen on every interface. A remote device on the
  LAN can then reach the socket, but is **walled until it presents a valid
  credential** (device token / bootstrap ticket / shared Gateway token) — so
  exposure of the socket no longer equals control of the agent. The server
  logs a one-line warning at startup when it binds a non-loopback interface.
  Still, treat any accepted credential as a key to everything (an authorized
  remote has full operator authority, including PTY / shell): share it only
  over a trusted channel, and rotate it if it may have leaked.
- **Beyond the LAN.** To reach Aleph across the internet, encrypt the
  transport — either front it with your own TLS-terminating reverse proxy
  (recommended) or enable Aleph's native in-process TLS. See
  **[Remote-connection transport encryption](#remote-tls)** below. Plaintext
  to a remote client is now refused by default (boot gate + per-connect gate).

### Remote-connection transport encryption (TLS) {#remote-tls}

Reachability (above) controls *who can open the socket*; TLS controls *whether
the bytes on the wire are readable*. The two are independent: the Gateway token
authenticates, TLS encrypts. A `host = "0.0.0.0"` deployment without TLS ships
the token and every message in cleartext, sniffable on any hop between client
and server.

**Enforcement (off-by-default; loopback always exempt).** Loopback
(`127.0.0.1` / `::1`) stays plaintext `ws://` — the zero-config desktop / CLI
/ same-host-proxy hop is unchanged. For a **non-loopback** bind Aleph is now
fail-closed on plaintext:

- **Boot gate** (`check_network_exposure`): a config that binds a non-loopback
  `host` with no native TLS, no trusted proxy, and `allow_insecure_remote =
  false` **refuses to start** with an actionable error. (This is the one
  intentional breaking change — a previously-working `host = "0.0.0.0"`
  plaintext config now must pick a remedy below.)
- **Per-connect gate** (`refuse_insecure_remote`): a remote client whose leg is
  unencrypted is rejected at the WS upgrade with `426 Upgrade Required`, even if
  the boot gate passed on a permissive combo. "Encrypted" means native TLS
  terminated in-process, or a trusted proxy that set `X-Forwarded-Proto: https`.

Three ways to satisfy it:

#### Tier ① — TLS reverse proxy (recommended; needs a domain)

Aleph stays bound to **loopback**; a same-host Caddy/nginx terminates TLS and
forwards to it. Keep the proxy config trivial — all the robustness lives in
Aleph.

`~/.aleph/config.toml`:

```toml
[gateway]
host = "127.0.0.1"                       # aleph stays loopback; the proxy is same-host
allowed_origins = ["https://your.domain.com"]   # domain Host is not auto-allowed (DNS-rebind guard)

[gateway.trusted_proxy]
enabled = true                            # honor the proxy's X-Forwarded-For / -Proto
# trusted_ips defaults to ["127.0.0.1", "::1"] — correct for a same-host proxy
```

`Caddyfile` (the entire file — Caddy auto-provisions Let's Encrypt):

```
your.domain.com {
    reverse_proxy 127.0.0.1:18790
}
```

> **Why `trusted_proxy` is security-critical here, not just cosmetic.** The
> proxy connects to Aleph over loopback, so *without* `trusted_proxy` every
> remote client would appear to Aleph as `127.0.0.1` — i.e. **auto-authorized
> as loopback operator**, a full auth bypass. With `trusted_proxy = true` Aleph
> reads the real client IP from `X-Forwarded-For` (spoof-safe: only a peer in
> `trusted_ips` is believed), so a remote client is correctly seen as remote
> and must present a Gateway-token credential, and per-IP rate-limit / cap /
> audit key on the real client. Enabling the proxy **without** setting
> `trusted_proxy` is a mistake.

> **`trusted_proxy` assumes a same-host proxy.** The default `trusted_ips` is
> loopback, matching a co-located Caddy whose hop to Aleph never leaves the
> machine. If you put the proxy on a *different* host you must (a) bind Aleph to
> a LAN interface, (b) add the proxy's IP to `trusted_ips`, and (c) accept that
> the proxy→Aleph hop and Aleph's non-`/ws` routes (Panel assets, `/health`,
> `/metrics`) travel plaintext on that LAN segment. The Gateway token only ever
> rides `/ws`, but prefer a same-host proxy so nothing sensitive is on the wire.

#### Tier ② — Native self-signed TLS (no domain; weaker)

No domain, no proxy — Aleph generates and persists a self-signed cert to
`~/.aleph/data/tls/` on first boot and logs its SHA-256 fingerprint. Its SAN
**auto-covers loopback plus every non-loopback interface IP of the box**
(e.g. a VPS public IP on `eth0`), so connecting by that IP passes TLS hostname
validation; add hostnames or a NAT'd public IP via `[gateway.tls] san = [...]`.
Clients still get a browser cert warning (accept-once, or pin the fingerprint) —
encryption is real, the trust anchor is manual. A newly-appearing address
regenerates the cert (new fingerprint ⇒ re-trust once); a `sans.txt` sidecar
tracks coverage so churn stays minimal. Note the SAN enumerates *every* local
interface IP — including private/LAN and Docker-bridge addresses — so anyone who
inspects the cert learns the box's interface map; harmless for a personal server,
but list only what you need via `san` + a loopback bind if that matters.

```toml
[gateway]
host = "0.0.0.0"

[gateway.tls]
enabled = true          # empty cert/key paths ⇒ auto self-signed
# san = ["vps.example.com"]   # optional: hostnames or a NAT'd IP not on any local interface
```

The Panel hard-codes `wss://` for any non-loopback host and refuses a plaintext
socket to a remote gateway, so remote Panels connect over TLS automatically.

#### Tier ③ — Native TLS with a real cert (domain, no proxy)

Point Aleph at operator-provided PEM files (e.g. certbot output). Aleph
terminates TLS itself.

```toml
[gateway]
host = "0.0.0.0"

[gateway.tls]
enabled = true
cert_path = "/etc/letsencrypt/live/your.domain.com/fullchain.pem"
key_path  = "/etc/letsencrypt/live/your.domain.com/privkey.pem"
```

#### Escape hatch — `allow_insecure_remote`

```toml
[gateway]
host = "0.0.0.0"
allow_insecure_remote = true    # DANGER: plaintext to remote clients
```

Restores pre-hardening LAN-plaintext behavior (boot gate + per-connect gate
both stand down). Only for a trusted, isolated LAN where you knowingly accept
cleartext. Never on a public interface.

#### Debian production recipe (Tier ①, the user's ColoCrossing box)

```bash
# 1. aleph stays loopback + trusted_proxy on (config above), restart aleph-server
# 2. install Caddy, drop in the one-line Caddyfile above, `systemctl reload caddy`
# 3. firewall: expose only the proxy + SSH; aleph's 18790 is loopback-only already
ufw allow 22/tcp && ufw allow 80/tcp && ufw allow 443/tcp && ufw enable
# 4. rotate the Gateway token after enabling remote access
#    (RPC: gateway.token.rotate — regenerates the shared token, revokes paired devices)
```

Verify: a remote browser gets a green-lock `wss://your.domain.com/ws`; a direct
`http://<public-ip>:18790` attempt fails (loopback bind, not routable); the
security audit log shows the **real** client IP, not the proxy's.

### The one remaining guardrail: WS Origin check

Browsers attach an unforgeable `Origin` header to every WebSocket upgrade
and cross-origin `fetch`. A malicious public web page the user happens to
visit can still *reach* `ws://127.0.0.1:18790/ws` (loopback is not
firewalled from the browser), so without an origin check it could open a
control channel to the local daemon — the cross-origin-WebSocket
confused-deputy (and its DNS-rebinding variant). The origin gate
(`src/gateway/origin_policy.rs`) is therefore retained as the **only**
validation on the browser surface. It guards against the public internet,
**not** against LAN neighbours.

`OriginPolicy::is_allowed` decides:

| Origin | Verdict | Why |
|--------|---------|-----|
| **absent / empty** | allow | Native clients (CLI, bots, bridges, `tokio-tungstenite`) send none; only browsers do. |
| **loopback** (`127.0.0.0/8`, `::1`, `localhost`, `*.localhost`) | allow | Same-machine UI. |
| **`tauri:` scheme** | allow | The desktop shell's own webview origin, unspoofable by a remote page. |
| **exact allow-list match** (`[gateway] allowed_origins`) | allow | Operator-configured extra origins for split panel/API deployments. |
| **same-origin** (Origin authority == request `Host`) | allow **only if the `Host` is an IP literal or loopback** | LAN deployments reached by IP (`http://10.10.10.6:18790`) work without config; an IP literal cannot be DNS-rebound. A **domain** `Host` is *not* auto-allowed — the deployment must add its origin to `allowed_origins` (see DNS-rebinding note below). |
| anything else (public web domain) | **deny** | |

Note the gate does **not** auto-allow arbitrary private-LAN IPs by range —
a cross-origin browser request from another LAN host is only accepted when
it is same-origin with the gateway's `Host`, in the allow-list, or
loopback/`tauri:`. Native LAN clients carry no `Origin` and pass freely.

> **DNS-rebinding — defended.** A classic DNS-rebinding attack rebinds a
> domain (`evil.com`) to the gateway's own address so the victim's page
> carries `Origin == Host == evil.com` and would slip past a naive
> same-origin check. Aleph closes this by gating the same-origin branch on
> the `Host`: same-origin is auto-allowed **only when the `Host` is an IP
> literal or loopback** (`127.0.0.0/8`, `::1`, `localhost`, `*.localhost`, or
> a bare IPv4/IPv6 address). A rebinding attack must use a domain name (the A
> record is what gets rebound), and a domain `Host` no longer passes
> same-origin — it falls through to deny. The trade-off: a zero-config
> **domain** deployment (serving the panel from `aleph.example.com` with no
> `allowed_origins`) is now rejected and must add its origin to
> `[gateway] allowed_origins`. LAN deployments reached by IP and loopback
> access are unaffected.

**Escape hatch — `allow_any_origin`.** Set `[gateway] allow_any_origin =
true` to trust every Origin unconditionally
(`OriginPolicy::allow_any`). Intended only for deployments that front the
gateway with their own reverse proxy / auth layer; it leaves the agent
drivable by any web page the user's browser visits, so keep it `false`
unless you know why.

### Migration from the pre-revert auth model

The original heavyweight device system (silent bootstrap, `/pair` 6-digit
codes, `/login` form, `?token=` URLs, `aleph auth …` CLI) was removed in the
LAN-trust revert. It was later **replaced by a leaner device model** — device
tokens (`aleph-dt-*`) issued via one-time bootstrap tickets (`aleph-bt-*`,
scanned as a `?bt=` QR), managed by `gateway.devices.*` RPC rather than a CLI.
Long-lived credentials never ride in a URL or QR (only the single-use ticket
does), which closes the old `?token=` leak vector. For operators upgrading
from the pre-revert build:

- **Old `[gateway.auth]` config is ignored, not rejected.** The config
  root has no `deny_unknown_fields`, so dead keys (`require_auth`,
  `enable_pairing`, `[gateway.auth]`, `[gateway.bootstrap]`, …) load
  without error and are silently dropped
  (`GatewayConfig::from_toml` legacy-config test). **One gotcha:** a
  `allowed_origins` list that lived under the legacy `[gateway.auth]`
  table is **not** migrated — move it up to the `[gateway]` root yourself.
- **`aleph auth *` CLI subcommands are gone.** There is no
  `aleph auth show-token` / `debug show-token` / `devices` / `pairing`
  command any more. "Open in Browser" survives as a desktop-shell menu
  item that hands the plain gateway URL (loopback, or the configured
  remote) to the system browser — no nonce, no token.
- **Orphaned `~/.aleph` token / device data is left in place.** The server
  ignores any leftover device/token rows on startup and never deletes user
  data; you may remove them by hand if you wish.

---

## Gap analysis: exec tiers vs codex / hermes-agent / pi {#exec-tier-gap-analysis}

> Same convention as the **openclaw 对照映射表** in [CLUSTER.md](CLUSTER.md):
> per-dimension verdicts, anchored to real code. **Read this before re-comparing
> Aleph's permission model against another agent's — the comparison has been
> done.** Verdicts: **aligned** (same shape, no action) · **aleph-superior**
> (keep, protect by test) · **gap** (closed in the 2026-07-14 round unless said
> otherwise) · **deliberately-not-ported** (do not "helpfully" add it).

| Dimension | codex | hermes / pi | Aleph | Verdict |
|---|---|---|---|---|
| Policy axes | 2 live axes: `AskForApproval` × `PermissionProfile` | hermes: `approvals.mode {manual\|smart\|off}` × gate stack. pi: declined a sandbox entirely | 1 axis: `ExecTier {Plan,Ask,Auto,Full}`, orthogonal to `[sandbox.command_policy]` (an undisableable floor). codex models its plan phase as a separate `ModeKind` axis; here it is the strictest rung of the one axis, so both existing ceilings compose with it unchanged | **aligned** |
| Tier semantics | 4 approval values, but 2 of the 3 shipped presets share one — the axis that really moves is the sandbox profile | hermes: 3 modes + a HARDLINE floor under the top one. pi: 3-valued project TRUST, gating config *loading* | 3 tiers with honest semantics; the floor holds under all three | **aligned** |
| Gate input | name-based argv allowlist + a Starlark rules engine over the command string | hermes: 47 regex `DANGEROUS_PATTERNS` over argument content. pi: tools declare *no* risk metadata; every gate re-derives danger from regex | **metadata-driven**: `ToolFacts` read off the declared `ToolDefinition`, never the name; one argument-level filter (`file_ops`) | **aleph-superior** — do not regress toward regex-on-shell-strings |
| Safe-read bypass | `is_known_safe_command` argv allowlist, compositional over `&& \|\| ; \|` | hermes: permanent glob `command_allowlist`. pi: patterns exist only in an example | `READ_ONLY_TOOLS` (pure-read builtins, single source for claim + retry + tier) + MCP `readOnlyHint`; default-deny for anything unlisted | **aligned** |
| Memory key | `{env, CANONICALIZED argv, cwd, sandbox perms}` | hermes: keys on a *pattern*, so "always" on `rm -r*` allowlists every future one. pi: no memoization at all | `grant_fingerprint(tool, canonical args)`, shared by session memory **and** the denial ledger | **gap → closed** (was: the bare tool name) |
| Escalation *axis* (model-declared) | model-declared `SandboxPermissions` on the exec tool | hermes: plugin may re-escalate into the same gate | **present**: `bash_exec` / `code_exec` declare `allow_network` / `allow_subprocess` / `extra_writable_paths` + a `justification`, mapped to `SandboxCapabilities`, arbitrated by `ApprovalGate` (`format_capability_request`), and OS-enforced by the driver | **aligned** (arguably superior — it carries the justification to the approver) |
| Escalation *retry ladder* | **`ToolOrchestrator`'s harness-side ladder**: on `SandboxErr::Denied`, pick a recovery strategy and re-run with elevated perms | pi: "approve-with-modification" mutates `event.input` in place, with no re-validation | none — denial is terminal and returned to the model to re-plan | **deliberately-not-ported** (R10 5th 不 / A2) — see below |
| Timeout / orphans | approvals block forever; turn death drops the sender → fails closed | hermes: timeout ⇒ deny ("silence is not consent"). pi: `select` timeout returns `undefined`, so a gate written `if choice === "No"` **fails open** | 120s ⇒ refusal everywhere; timeout is *not* ledgered (an expired card is not a decision); `is_live()` evicts orphan waiters | **aleph-superior** |
| Runtime switching UX | `/permissions` → 3 presets, session-scoped, admin-lockable | hermes: `/yolo` per session; shift-click writes it globally. pi: restrictions persist to the session transcript and survive resume | composer pill (per-session, rides the first message) + Settings→Policies (global, live per turn) | **gap → closed** (Panel lost the display on select/reload while the server kept enforcing) |
| Unknown-tool default | MCP: an unannotated tool requires approval | hermes: documented fail-open hole in headless non-gateway contexts. pi: no metadata, so "unknown" is meaningless | fail-closed by construction: unknown ⇒ non-idempotent ⇒ mutating ⇒ `Ask` holds | **aligned** — protect by test |
| Per-tool override | per-server *and* per-tool approval mode; two-tier memory | hermes: `approvals.deny` globs that survive yolo; per-rule grain | `[policies.tool_permissions]` exact + glob, 3-tier merge, most-restrictive-wins; explicit beats the tier | **gap → closed** (the tier used to *widen* a `default = "deny"`) |
| Background inheritance | approval store lives on shared session services | hermes: background writes must stage; **cron gets its own axis** because "ask" is meaningless with no human attached | subagents inherit correctly; continuations now carry `caller_role` + channel permissions; headless producers stamp `unattended` ⇒ fail closed | **gap → closed** |
| Audit trail | telemetry on the orchestrator path | hermes: observability hooks, everything redacted before display | live: `ToolCallApproved` / `ToolCallDenied` session events | **gap → closed** by deleting the dead SQLite trail that reported zeros |
| Cryptographic actor identity | none — the caller is a process, not a principal | hermes / pi: none | per-agent Ed25519 keypair, **delegated sub-agents included**; vault-held private half, fingerprint + public half in `security.db`; key lifecycle recorded inside the agent's own chain | **ahead** — no reference implementation in this set has one (buzz does, but keyless and with no lifecycle; see AGENT_IDENTITY.md) |
| Record integrity | append-only intent, no chain | none | per-agent hash chain, Ed25519-signed, anchored head, first-row genesis check | **ahead** — detects edit / reorder / mid-delete / transplant / prefix-delete / tail-truncate, each located to a `seq` |
| Verifier | none | none | `agent_identity(action="verify")` + `aleph-server identity verify` (offline, daemon-independent) | **ahead** — shipped with the chain, not after it |
| Floor beneath the top tier | under `Never`, dangerous commands are Forbidden — **but only when the sandbox profile is Managed**; with it off, the top tier is unbounded | hermes: `HARDLINE_PATTERNS` + a user-editable `approvals.deny` floor that survives yolo | `[sandbox.command_policy]` holds under every tier including `Full` (unit-pinned); a `deny` override also beats the tier | **aligned** — better placed than codex's |
| What the human SEES | full argv + cwd + the model's own justification | hermes: the whole command, redacted, with all findings merged into one prompt. pi: typed per-tool event | the redacted **action summary** (the command / `operation=delete path=…`), on every surface | **gap → closed** — this was the sharpest defect of round 1 |
| Windows command parsing | `shell-command/src/command_safety/`: a **resident PowerShell AST subprocess** (`powershell_parser.ps1`, id-tagged request protocol) + shlex/operator splitting for cmd — feeding *approval escalation*, not a hard block | hermes / pi: no Windows-specific command surface at all | `RegexSet` hard-filter over a normalised copy; codex's AST **deliberately not ported** (see below), its three *semantics* were: same-segment gaps (`seg!()`), order-free verb/flag/target, full `Remove-Item` alias set | **aligned on semantics** — deliberately different on mechanism |
| Encoded-payload visibility | AST parser sees the real script; codex owns the encoding side | none | `-EncodedCommand` (and `-e`/`-ec`/`-enc`) base64/UTF-16LE decoded in `normalize.rs` and appended to the scan text, bounded (64 KiB × 8 payloads × 2 nesting rounds) and text-gated; decoding precedes the tier split so `enforcement = "off"` cannot restore the blind spot | **gap → closed 2026-07-27** — the floor was one base64 away from being off on Windows |

### Deliberately not ported (do not add these)

- **codex's sandbox-escalation retry ladder.** On a sandbox denial, codex's
  `ToolOrchestrator` selects a recovery strategy and re-runs with elevated
  permissions. That is precisely R10's 5th 不 (不做错误恢复策略选择) and precisely
  what clause **A2** forbids: *let the model see and heal the error = yes; let the
  harness pick the recovery strategy for it = no.* Aleph compresses the denial
  into context and the model re-plans. A future round that "helpfully" adds a
  retry matrix is reverting an architectural decision, not fixing an omission.
- **codex's resident PowerShell AST parser** (`command_safety/powershell_parser.ps1`
  + a cached child process per executable, id-tagged request/response over
  stdio). It is the right tool for codex's job — deciding whether an argv is
  *safe enough to auto-approve* — and it buys real precision: it can tell
  `echo del /f x` from `del /f x`, which a regex cannot. Aleph does not take it,
  for three reasons that are unlikely to change: it puts a PowerShell round-trip
  on **every** sandboxed exec (R3 core minimalism, R10 thin harness), it makes
  the catastrophic floor **depend on PowerShell being installed and healthy** on
  a path whose whole point is to fail closed, and Aleph's approval-side analogue
  (`exec_approval/` + exec tier) is a different layer from this one. What *was*
  ported is the semantics — same-segment matching, order-free verb/flag/target,
  the complete `Remove-Item` alias set. The accepted cost is the
  `echo del /s C:\` class of false positive; that is a **known trade**, recorded
  in FEATURE_LOCATOR §3.8, not an omission to fix later.
- **pi's approve-with-modification** (the gate mutates `event.input` in place).
  Tempting, but pi does **no re-validation after mutation**, and Aleph has no
  consumer for a third "allow-if-rewritten" state that a tier enum cannot express.

### Closed in round 3 (2026-07-14, sandbox × tier seam)

- **`session_send` delegation escalation** (was critical): the delegated run
  carried only `project_root`, so a guest channel became operator +
  unclamped-tier by delegating. Now forwards **both** restrictive keys the
  `carry_policy_metadata` continuations carry — `caller_role` and the channel
  `tool_permissions` deny layer (`CHANNEL_TOOL_PERMISSIONS_KEY`, plumbed onto
  `TurnContext`) — and stamps `unattended` on fire-and-forget
  (`build_sub_metadata`). Without the channel layer a guest could still bypass
  its own `deny` override (e.g. `web_fetch = deny`) by delegating.
- **`WorktreeSandbox` floor bypass**: subagent worktree isolation ran commands
  with no hooks — the hardline command-policy floor and secret scrub never ran.
  Now shares both (the before-hook + `scrub::scrub_and_gate_output`) with
  `WorkspaceSandbox`.
- **PKCS#8 block-class miss**: the private-key regex required an algorithm word,
  so `-----BEGIN PRIVATE KEY-----` slipped the block floor. Both catalogs now use
  `-----BEGIN[A-Z ]*PRIVATE KEY-----`.
- **Justification poisoning the fingerprint**: the model's `justification` rode
  into both approval fingerprints; now excluded from the key on both gates.
- **`file_ops` on `tools.invoke`**: its destructive ops are argument-level and
  this surface can't honor them, so `file_ops` is now on the gateway denylist.

### Closed in round 4 (2026-07-17, permission-hierarchy hardening)

- **`merge()` erased the explicitness bit** (correctness): the 3-tier merge
  dropped any override whose merged value equalled the merged default as a
  "compression". Two casualties: a same-layer exact-name carve-out inside a
  glob family (`github__* = ask` + `github__list_issues = allow` — the `allow`
  vanished and the glob re-captured the tool), and the *explicitness* of an
  `allow` — after merge, `resolve_explicit` could no longer tell "the operator
  decided Allow" from silence, so the exec tier re-gated tools the operator
  had deliberately named. Merge now keeps every named key; pinned end-to-end
  (`explicit_allow_survives_merge_and_beats_the_tier`).
- **Operator gate + confirm gate double-prompt**: `vault_store` / `agent_delete`
  sit in both `OPERATOR_TOOLS` and `CONFIRMATION_REQUIRED_TOOLS`; an operator's
  `AllowOnce` (which writes nothing into session memory) fell through to the
  confirmation gate, which re-prompted the very call the operator just read.
  The operator-approved call now skips the confirm gate for that call only;
  operator-tier callers (who pass the config gate without an approval) still
  hit it.
- **Wait-mode `session_send` children of a headless parent** now inherit
  `unattended`: `TurnContext` carries the flag (fed from `UNATTENDED_KEY` at
  run start) and `build_sub_metadata` stamps `fire_and_forget || parent
  .unattended` — an instant fail-closed deny instead of a 120 s hang.
- **Free-text `/deny <reason>` back to the model** (hermes parity): the reason
  rides `ExecApprovalRecord.deny_reason` → `ResolvedDecision` →
  `ApprovalResponse` (the `ApprovalRequester` trait now returns outcome +
  reason; transports that cannot carry one use `From<ApprovalOutcome>`), and
  the dispatch gate renders it verbatim — `The user said: "…"` — in the
  model-facing error on the confirm, config-sudo, hook-Ask and sandbox
  elevation paths. `exec.approval.resolve` accepts an optional `reason`
  param; the cluster reverse-RPC response carries an optional `deny_reason`
  (older nodes ignore it). Deliberately NOT ledgered — the denial ledger keys
  on the fingerprint, and the reason is display-layer.
- **Entropy**: deleted the dead `ChannelPolicy` trait island
  (`ChannelPolicy` / `WhatsAppPolicy` / `PolicyDecision`, zero consumers —
  WhatsApp's live policy is `wa_policy/`, which consumes the
  `ChannelAccessConfig` data types directly). `matches_glob` now compiles
  through a bounded process-wide regex cache (it sits on the per-tool
  `resolve_explicit` hot path).

### Still open (honest)

- `tools.invoke` has no *general* argument-level tier parity (needs an approval
  transport on the RPC surface). `file_ops` — the one destructive multiplexer —
  is now denied outright there, but the general gap remains for any future
  argument-gated tool.
- The `FullRead` / `FullWrite` / `ProxyOnly` sandbox-policy variants and the
  managed per-host proxy subsystem remain dead (no producer sets them); pruning
  them is deferred (cross-platform driver match-arm surgery — track with the
  network-proxy decision).
- No user-editable floor under `Full` in hermes' sense (an `approvals.deny` glob
  that survives yolo). `[policies.tool_permissions]` `deny` overrides already
  cover ~80% of it, since an explicit entry beats the tier.
- ~~The Panel's approval card has no reason input yet~~ **closed**: the card
  now has a "Deny with reason…" entry (inline input, Enter/confirm submits
  `reason` on `exec.approval.resolve`), matching kimi-cli's approval option 4.
  The TUI overlay still sends a bare deny — it resolves by decision index and
  has no free-text input mode.

---

## Cross-Cutting Security Module Index

| Module | Location | Purpose |
|--------|----------|---------|
| SSRF Engine | `src/security/ssrf/` | Outbound request validation |
| HTTP Headers | `src/security/headers.rs` | Security response headers |
| Content Sanitizer | `src/security/content_sanitizer.rs` | Prompt injection defense — tokenizer/format-marker scrubbing (openclaw-parity literal set + `reserved_special_token_NN` family), forged-boundary-marker replacement, bounded sanitized truncation |
| Safe Regex | `src/security/safe_regex.rs` | `bounded_builder` — 1 MiB compiled-size cap for untrusted (config/operator-supplied) patterns. Rust's `regex` crate matches in linear time, so the ONLY residual risk is compile-time automaton expansion, not backtracking; every config-pattern compile site routes through it |
| Audit Logger | `src/security/audit.rs` | Security event logging |
| Browser Guard | `src/browser/network_policy.rs` | Browser navigation SSRF |
| Crypto + Vault | `src/gateway/security/` | Secrets vault, shared-token crypto |
| Tool Permissions | `src/tools/scoped/` | Per-channel tool permission merge + the enforcement chokepoint |
| Exec Tier | `src/config/types/policies/exec_tier.rs` | Plan / Ask / Auto / Full — the rules and the one precedence composition point |
| Plan gate | `src/tools/plan_gate.rs` | The plan → build handoff: a human `approved` lifts the read-only planning tier mid-turn |
| Approval Gate | `src/sandbox/exec_approval/` | Action-aware confirmation, grant fingerprint, denial ledger |
| Gateway-surface denylist | `src/security/dangerous_tools.rs` | What `tools.invoke` refuses outright |
| Exec primitives | `src/exec/` | Command parse for approval summaries (`analyze_shell_command`), `SecretMasker`, advisory custom-pattern `SecurityKernel` — support code, not a standalone enforcement kernel |

---

## Swift helper isolation

**The Swift `AlephBridge` helper process must not read or write any vault path,
including `~/.aleph/data/`, `~/.aleph/.shared_token`, or any session or memory
storage directory.** The vault's file lock is owned and held exclusively by the
Rust core. Concurrent writes from any second writer — including the bridge
helper — corrupt the encrypted vault and unrecoverably destroy all stored API
keys, OAuth tokens, and embedding keys.

This rule has already caused a production incident (the `.shared_token` event
documented in CLAUDE.md). It must be treated as a hard constraint, not a
guideline.

Audit procedure: whenever a new handler is added to `AlephBridge`, the code
reviewer must confirm the handler does not open, read, or write any file under
`~/.aleph/`. The handler source file must include a comment of the form:

```swift
// Vault access: none. This handler only reads [describe what it reads].
```

Any handler that cannot include this comment must be redesigned before merging.

## Cross-process safety guarantees (Spec C, 2026-05-02)

The original `.shared_token` corruption incident (documented elsewhere
in this file) had a structural cause: nothing prevented two
`aleph-server` processes from writing the same `~/.aleph/data/` files.
Spec C closes that gap with four layered protections:

- **Singleton lock**: `aleph-server start` acquires
  `~/.aleph/data/aleph.lock` via `flock()` in `main()` before any
  other state. The OS releases the lock automatically on process
  exit (graceful, panic, SIGKILL). A second `start` exits cleanly
  with code 64 + a stderr diagnostic naming the holder PID.
- **Vault writes**: every write to `secrets.vault` goes through
  `alephcore::utils::vault_io::VaultIo`, which combines an fcntl
  exclusive lock (defense-in-depth even if the singleton ever fails)
  with `tempfile + persist` rename for atomicity.
- **JSON state writes**: `acp_sessions.json` and equivalent
  hand-managed JSON files use `alephcore::utils::atomic_io::write_atomic`,
  which is also temp + rename via `tempfile`.
- **SQLite connections**: every connection under `~/.aleph/data/`
  is opened via `alephcore::utils::sqlite_open::open_sqlite_safe`,
  which sets `journal_mode=WAL`, `busy_timeout=5000`, and
  `synchronous=NORMAL`. Read-only callers use `open_sqlite_readonly`.
  Multi-reader + single-writer is now safe by construction.
- **CLI dispatch**: every CLI subcommand declares a
  `CommandPolicy` (NoLock / LockOnly / LockOrIpc) and routes
  through `alephcore::cli::policy::with_policy` or `run_no_lock`.
  When the server holds the singleton lock, write subcommands
  forward to `/v1/admin/*` IPC endpoints (token rotation
  self-heals via a single 401 retry). When no server is running,
  the CLI takes the lock locally and operates directly.

Reverse-regression: `scripts/spec_c_regression.sh` enforces the four
invariants (no direct rusqlite open, no direct fs ops on vault/acp
files, every CLI subcommand annotated, no leftover
`acquire_instance_lock` callers). Run before any commit that touches
`src/utils/`, `src/cli/`, `src/gateway/admin_api/`, or
`src/bin/aleph-server/commands/`.

## See Also

- [Architecture](ARCHITECTURE.md) - System overview
- [Feature Locator](FEATURE_LOCATOR.md) - §5.2 permission hierarchy, §5.3 approval gate, §5.12 exec tier (code anchors + what was deleted)
- [Tool System](TOOL_SYSTEM.md) - How bash_exec works
- [Gateway](GATEWAY.md) - Security RPC methods
- [Desktop Bridge](DESKTOP_BRIDGE.md) - Bridge isolation invariants
