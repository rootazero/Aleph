# Triage Playbook — CONNECT / CUT / DECIDE

Read this before touching any candidate from phase 2. It exists because the intuitive move
— "a wire is broken, so reconnect it" — is wrong roughly half the time.

## Contents

- [The decision tree](#the-decision-tree)
- [The over-count meta-finding](#the-over-count-meta-finding)
- [Case studies: "broken wire" that was actually dead scaffolding](#case-studies)
- [Case studies: real CONNECTs](#real-connects)
- [The delete-a-file safety rule](#the-delete-a-file-safety-rule)

## The decision tree

For each candidate, in this exact order:

```
1. Grep the CONSUMER side for a LIVE caller/reader/subscriber.
   (Api::x( , rpc_call("x") , .subscribe(Bus) , an actual field read)
        │
        ├─ No live caller anywhere ──────────────► CUT
        │     The "producer" is scaffolding no one uses. Reconnecting it
        │     revives an API that then needs perpetual syncing. Delete it.
        │
        └─ Live caller exists → the wire is genuinely load-bearing.
                │
                2. Has the wire been severed a long time with ZERO
                   observed production pain, AND does an error path or a
                   newer mechanism already cover the same outcome?
                        │
                        ├─ Yes ──────────────────► CUT (painless-wire heuristic)
                        │
                        └─ No → the feature is genuinely wanted and dark.
                                │
                                3. Does reconnecting add real new coupling,
                                   or is "revive vs delete" a product call?
                                        │
                                        ├─ Yes ──► DECIDE (present trade-off, ask)
                                        │
                                        └─ No ───► CONNECT (add the one missing wire)
```

The whole tree is "read first, act last". Every branch that ends in CUT was reached by
*reading* the consumer, not by trusting the handler-side appearance.

## The over-count meta-finding

**Both buckets systematically over-count, in opposite directions:**

- The **CONNECT** bucket over-counts because a semantic/LLM audit looks at the *handler*
  side, sees a stub (`// TODO persist`, an empty registration), and concludes "broken wire,
  fix it" — without checking whether any client actually calls it. In the Aleph audit,
  **5 candidates** flagged as "broken wires to CONNECT" turned out to be dead client
  wrappers with zero callers → they were **CUT**.

- The **CUT** bucket over-counts too: a candidate that *looks* like a dead abstraction can
  have one live, non-obvious consumer. In the Aleph audit, `run_event_bus` looked like a
  dead bus but had a live `emit_run_retrying` producer broadcasting provider-retry notices
  to users (an R5 concern) → it was **kept**. And `CleanupPolicy` looked deletable with its
  tool but fed a live config field → **relocated, not deleted**.

The lesson both directions share: **the audit's own bucket labels are a hypothesis, not a
verdict.** Re-verify each by reading the real consumers before you cut or connect. In the
2026-07-15 audit this reversed on **4 separate occasions** — track it, it will happen to you
too.

## Case studies: "broken wire" that was actually dead scaffolding

These read as CONNECTs and were CUTs. Pattern to internalize: **handler-side stub + zero
client callers = dead scaffolding, not a severed feature.**

1. **`config.set` / `config.list` / `sessions.set_pinned`** (client ghosts).
   Looked like "missing handlers to add". Read the client: the webchat wrappers had **zero
   callers** repo-wide, and the server had no `pinned` field at all. → **CUT** the dead
   wrappers. (An earlier note had planned "re-point to `sessions.patch`" — that would have
   been wiring dead code to a live method. Read-before-write caught it.)

2. **`discord.save_config` / `discord.update_allowlists`** (stub far-end + no UI).
   Handlers were `// TODO persist` stubs. Read the consumer: the Discord panel view had
   **no save UI at all** (`DISCORD_FIELDS = &[]`), token came from an env var, client
   wrappers had zero callers. → **CUT**. Reviving it would be *building a feature* (add save
   UI → wire handler → persist), not repairing a wire.

3. **`invalid` tool** (the subtle one — a *real* severed wire that was still a CUT).
   Producer `InvalidTool` (187 lines + tests) existed; consumer `repair.rs` looked it up as
   a fallback (`tools.get("invalid")`); the missing wire was "register it into the tool
   map" — a genuine form-1-adjacent severance. But CONNECTing it meant every tool-server
   construction would have to register it *and* keep an `available_tools` list synced
   forever (a stale list is worse than an honest error) — fat-harness coupling. Decisive
   factor: **it had caused zero production pain**, and the error path
   (`tool_not_found_with_suggestion` + "use list_tools") already produced the same outcome.
   → **CUT** (painless-wire heuristic). Deleting the variant also exposed `was_successful()`
   as a test-only dead API removed alongside.

## Real CONNECTs

Internal wires (not cross-crate client/handler pairs) are **more likely to be genuinely
severed** — there's no dead-client-wrapper failure mode. The Aleph real CONNECTs were all
internal:

- **`[policies.metrics]` inert but live-consumed:** a `StageTimer` read a hardcoded
  `DEFAULT_WARNING_MULTIPLIER` instead of the config field. CONNECT = bridge the field into
  the live consumer. (Contrast: the sibling inert `[policies.intent]` etc. had *no*
  consumer → CUT.) The tell: **an inert config with a live reader-of-a-hardcoded-default is
  a CONNECT; an inert config with no reader is a CUT.**

- **loop `AgentBusy` exhaustion:** the far end returned `Option<(u64,u64)>`, collapsing
  "exhausted" and "re-claim" into one blurry state, so the call site skipped cleanup +
  user-notify. CONNECT = a three-state `RearmDecision{Retry/Exhausted/Drop}` enum so the
  call site can act on Exhausted. (Note the compile-time flavor: the fix *is* a stronger
  type, not just a patched call.)

- **`task_wait` never woken:** subscribed to the wrong bus. CONNECT = subscribe to the bus
  that actually carries task-completion events, drop the timeout-only fallback.

## The delete-a-file safety rule

Before `git rm` a file: **grep every exported symbol it declares, not just the headline
type.** The `CleanupPolicy` E0432 lesson: deleting `spawn_tool.rs` (grepped only for
`SessionsSpawnTool`) broke compilation because the same file also defined a `CleanupPolicy`
enum consumed by a live config field in 12 places. If a to-be-deleted symbol has a live
consumer, **relocate** it to that consumer (dropping the tool-only helpers) rather than
deleting — deleting would change a config schema. `cargo test --lib --no-run` (not
`check --lib`, which skips `#[cfg(test)]`) is what catches this class before it ships.
