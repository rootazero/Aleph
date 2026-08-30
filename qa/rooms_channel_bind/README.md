# `qa/rooms_channel_bind` — a channel group conversation bound to a project room

```bash
bash qa/rooms_channel_bind/run.sh
KEEP=1 bash qa/rooms_channel_bind/run.sh        # keep the scratch dir
HOLD=1 bash qa/rooms_channel_bind/run.sh        # park the gateway for a browser
SKIP_BUILD=1 bash qa/rooms_channel_bind/run.sh
```

Real machine, real gateway, three authenticated principals, a real channel
carrying real group messages, and the real `aleph` CLI. Before this fixture,
**every claim about `projects.channel.*` rested on compile-and-unit-test
evidence** — nothing on this branch had spoken to a live server.

## It landed RED on purpose, named the defect, and is now GREEN

**Today: `52 passed, 0 failed, 0 skipped`** — `14a1ba355` onward, recorded by
**four** separate agents on this machine (the fixing agent, the task-16
implementer, and two reviewers), at least two of them building the binaries
themselves rather than reusing `SKIP_BUILD=1`.

It was committed at **`43 passed, 7 failed, 2 skipped`** and that history is the
point of this section, not a stale number to delete. All seven failures had one
cause, and the driver printed a `FINDING` block naming it rather than leaving the
next reader to decide whether the fixture was broken:

> `run_loop/inner.rs` fills the `FlowRequest`'s two scope fields from the raw
> `OWNER_META_KEY` / `SCOPE_META_KEY` metadata instead of from
> `request_scope(request)`, so the room upgrade that arm 2 computes is dropped
> at the harness spawn. The session ROW is stamped before that boundary and is
> correct; the prompt, every tool, and the speaker label are on the far side and
> run under `personal:<speaker>`.

The fixture's author measured it both ways and shipped it red rather than
shipping the fix, so the evidence would not be written by the same hand as the
change. The fix (`14a1ba355`, `01560c72f`) then went the other way round: its
author rebuilt `aleph-server` from the **pre-fix** source, reproduced
`43 / 7 / 2` exactly, and only then measured the green. Two agents,
independently, got the same red — which is what makes the green mean something.

The two `SKIP`s were the same defect one layer out. Scenario 3's positive control
(Ruling AG) failed because the room partition never received a row, so the two
"this partition gained nothing" assertions were withheld rather than passed — an
empty partition would satisfy them for exactly the wrong reason. With the fix the
control passes, so both run and both pass, and the total is 43 + 7 + 2 = **52**.
Those two assertions had never once executed before.

## What each scenario claims, and why only a live machine can settle it

| # | Claim |
|---|---|
| 1 | An **unbound** group files each speaker's turn under their own partition. The premise this round is motivated by; asserted first so a regression in it fails loudly instead of making the rest look correct. The driver stops here if it is red. |
| 2 | `aleph projects channel bind` over a live gateway upgrades the next turn to the room's partition **and** moves the conversation's existing row, which a roster member then sees in `sessions.list`. |
| 3 | A **paired** sender who is not on the roster stays in their own partition. |
| 4 | An **unpaired** sender runs with no scope at all and no `<room_context>`. |
| 5 | `<room_context>` reaches the model on a channel turn, names a member who has never spoken, and survives one `subagent` spawn into the child's own prompt. |
| 6 | `unbind` stops future turns and keeps what is already filed. |
| 7 | An `agent_switch` mints a new session key and the room survives it — the reason the binding table is keyed on the conversation rather than the key. |
| 8 / 8b | Ruling AQ evidence, two doors onto a bound-but-silent conversation, with the stored rows quoted verbatim. **Evidence, not a verdict.** |
| A | `require_operator_tier` refuses a real chat-tier connection, and it is the *tier* refusal, not an earlier one. |
| B | Both new clients, both directions, over a live wire. |
| C | A genuine store failure drives `RescopeOutcome::Unknown`, and both faces say "I cannot say". |
| E | A non-admin's `bind` comes back as `ADMIN_REQUIRED_MESSAGE`, and nothing is bound. |

## Three oracles

1. **`memory.db` → `notes_index.agent_id`.** The mock answers every group turn
   with `note_manage(create, filename=<marker>)`, and `note_manage` resolves its
   partition through `project_scope::session_write_id` — off the run's ambient
   `ScopeAttribution`. So the partition a marker's note landed in *is* the scope
   that turn ran under, read from disk rather than asked of the server.
2. **`sessions.db` → `owner_user_id` / `scope_id`.** Ruling AQ asks for the
   stored row verbatim, and a row on disk is the one thing here that cannot
   describe itself wrongly.
3. **The mock's request log.** `<room_context>` and the speaker prefix exist only
   in the request that carried them.

## Choices a reader would otherwise have to infer

- **The channel is `webhook`, not `telegram`.** The binding key is
  `(channel_id, peer_kind, peer_id)`; the mechanism knows nothing about which
  channel it is. `webhook` is the only type a fixture can drive with an HTTP
  POST and an HMAC, with no upstream service to mock. Everything the brief says
  about a Telegram group holds verbatim with `webhook` substituted.
- **The instance is named `webhook`** (i.e. after its type). `subsystems.rs`
  registers per-channel policy under the *instance* id while several factories
  hardcode the runtime id to the *type*; under any other name the
  `permission_level` below silently does nothing.
- **`permission_level = "config"` on the channel.** Without it a channel run
  carries `caller_role = "guest"`, capping the turn at `ExecTier::Ask`, and every
  `note_manage` the mock issues parks for approval — a dozen 120-second approval
  races in a fixture whose subject is attribution. The tier is orthogonal to
  everything scenarios 1–8b assert (scope comes from `pairing_store::sender_user`,
  never from the role), and the one claim that *is* about the tier gate is driven
  over a member's Panel connection instead.
- **Every group message says `@aleph`.** An unregistered channel config defaults
  to `require_mention = true`; without it every group message is refused with
  `Mention required in group` and nothing runs at all.
- **Scenario 7 asserts `coder__p-<id>`, not the brief's `main__p-<id>`.** That
  spelling assumes the agent id never changes, and an agent switch is precisely
  what changes it. The claim is that the *room* survives the switch, so the
  assertion is on the room half of the partition plus the new row's scope.
- **A second `bind` with no `--label` clears the stored label.** Recorded as a
  fact by the driver rather than asserted: a re-bind reads like an idempotent
  no-op and this half of it is not.

## Not covered

- **Addendum D — the Panel's room-settings channel section has no automated
  check here.** It is a browser claim; no shell assertion can make it. `HOLD=1`
  parks the gateway with the state already built so a browser can be pointed at
  `http://127.0.0.1:$GATEWAY_PORT/` — see the banner it prints for what to look
  at and at which widths.
- **Addendum E's first half** (a non-admin *sees* the bind/unbind controls) is
  also a render claim and lives behind the same `HOLD=1`. The write half — the
  refusal being classified rather than raw — is asserted.
