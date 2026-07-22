---
name: severed-wire-audit
description: >-
  Find and resolve "severed wires" in a codebase — a producer and a consumer both
  exist and are fully implemented, but the connecting registration / dispatch arm /
  event subscription / call site between them is missing, so a feature looks broken
  though both ends are done. dead-code lints CANNOT catch this: the producer isn't
  dead (it has tests) and the consumer connects to a stub or reads an inert config.
  Use when a feature "looks unwired / 没接上 / 两端都在但连线断 / 明明写了却调不到"; after a
  refactor or middleware removal, to verify producers still reach consumers; when
  asked to "审接缝 / 找断线 / seam audit / wire audit / connection audit"; or when a
  method returns NOT_FOUND, a tool the model can't call, an event that never fires,
  or a config field nothing reads. Triages each finding into CONNECT (fix the wire) /
  CUT (delete the dead abstraction) / DECIDE (ask the human), enforces
  read-before-write so dead scaffolding is deleted not reconnected, and installs a
  grep-diff guard against regression.
---

# Severed Wire Audit

## The problem this solves (病灶)

Across a codebase, features repeatedly **look missing** when the truth is narrower:
a **producer** exists, a **consumer** exists, and only the **wire between them is cut**.
两端都在，连线断。

The trap: `dead_code` / unused-import lints only catch **form 1** (a producer with no
consumer). They are blind to the four forms that matter more — a wire whose far end is a
**stub**, a config nobody **reads**, a client call with no **handler**, a name/path that
**drifted**. So the compiler stays green while the feature is dark.

The cure is to **audit seams, not modules**, then triage each severed wire — and the
single most expensive mistake is assuming a severed wire should be *reconnected*. Very
often it should be **cut**.

## The five-phase workflow

Run these in order. Do not skip phase 3's read-first rule — it is where most of the value (and most of the avoided damage) lives.

### 1. Scan seams (multi-lens, parallel, read-only)

A seam is a join point where one side's output must reach the other side's input. Sweep
each seam type as an independent grep-diff of **PRODUCED/DEFINED symbols** minus
**CONSUMED/DISPATCHED symbols**.

**Scope first.** Audit one subsystem's seams when the ask is "审计 X 子系统"; go whole-repo
when it's "find all severed wires" or after a broad refactor / middleware removal.

**Fan out one read-only agent per seam type.** They don't interfere, and one grep angle
always misses what another catches (the 2026-07-15 audit ran seven in parallel). Give each a
prompt shaped like: *"Under `<path>`, list every `<DEFINED pattern>` and every `<CONSUMED
pattern>`; return the set difference with a file:line for both ends. Read-only — do not fix
or edit anything."* Collect every agent's diff into the phase-2 list.

The seam types and the "6 forms of severed wire" (with concrete cases) are in
**[references/seam-catalog.md](references/seam-catalog.md)** — read it before scanning so
you know what each lens targets.

### 2. Enumerate candidates into one list

Collect every `DEFINED − CONSUMED` mismatch into a single flat list with a file:line
anchor for **both** ends (or "no consumer found"). Do not fix anything yet. The list is
almost always longer than the real defect count — that's expected, phase 3 prunes it.

### 3. Triage each candidate — READ-FIRST, then CONNECT / CUT / DECIDE

This is the heart of the skill. **Both** the CONNECT and the CUT buckets systematically
**over-count**, in opposite directions, and the only defense is to read the actual
consumers/callers before touching anything.

- **The read-before-write rule (non-negotiable):** the first action on any candidate is
  to grep for a **live caller** on the *consumer* side (`Api::x(`, `rpc_call("x")`,
  `.subscribe(`, an actual field read). A "missing handler" is very often a **dead client
  wrapper** with zero callers → that's a **CUT**, not a CONNECT. Reconnecting dead
  scaffolding is worse than useless: it revives an API that then needs perpetual syncing.

- **The painless-wire heuristic:** a wire that has been severed a long time with **zero
  observed production pain** — 一根断了却没人喊疼的线 — is a candidate to **CUT**, not
  reconnect. Especially if an error path or a newer mechanism already covers the same
  outcome.

- **DECIDE** is for genuine feature calls where reconnecting means real new coupling, or
  where "delete vs revive" is a product judgment. Present the trade-off and ask; don't
  silently pick.

The full decision tree, the "systematic over-count" meta-finding, and the real
case studies (5 candidates that read as "broken wires" but were dead scaffolding) are in
**[references/triage-playbook.md](references/triage-playbook.md)**.

### 4. Fix — surgically, and grep every symbol before deleting a file

- **CONNECT:** add the one missing wire (register the tool, add the dispatch arm, wire the
  subscription, point the client at the real method). Nothing else.
- **CUT:** delete the dead producer *and* its now-orphaned consumer stub, tests, docs, and
  imports — but **grep every exported symbol of a file before deleting the file**, not just
  the headline type. A file named for a dead tool can still export an enum a live config
  field consumes (the `CleanupPolicy` E0432 lesson: deleting the tool broke the config
  schema). If a to-be-deleted symbol has a live consumer, **relocate** it to that consumer
  rather than deleting.
- Keep diffs minimal and match surrounding style (see the user's global rules).
- **Verify with a pass that compiles TEST code**, not just a fast type-check — a CUT can
  break a `#[cfg(test)]` consumer that `cargo check --lib` never compiles (use
  `cargo test --no-run`, or the equivalent in your language). Then re-run the phase-5 guard:
  green means nothing severed beyond the baseline.

### 5. Guard — turn the runtime convention into a compile-time or CI check

A severed wire recurs because the same fact is copied into ≥3 unsynchronized lists with
**no single source of truth**. "No omission" is only real when the convention is
*enforced*, not documented:

- **Best — compile-time:** one enum/type as the single true source, with the other lists
  *derived* from it, so an unrouted method fails to compile. (In Aleph: RPC method single
  enum; a tool-registration completeness test; a config-reader completeness test.)
- **Good — CI grep-diff guard:** a script computing `DEFINED − CONSUMED` against a
  `KNOWN_SEVERED` baseline, wired into the test target. **Fix one → remove one from the
  baseline.** A new severed wire then turns the check red.

The grep-diff guard is a reusable, parametrized script:
**[scripts/wiring_audit.py](scripts/wiring_audit.py)** — point it at your DEFINED pattern +
glob and your CONSUMED pattern + files. The design rationale, baseline discipline, and the
"a guard is a **detector, not a judge**" rule (it flags inert wires; CONNECT-vs-CUT still
needs a human) are in **[references/guard-scripts.md](references/guard-scripts.md)**.

## The four non-negotiable rules (checklist)

Create one todo per rule when running an audit:

1. **Read before write.** Grep for a live *caller/consumer* before deciding CONNECT vs CUT.
   No live caller ⇒ default CUT.
2. **Grep every exported symbol before deleting a file** — not just the headline type.
   Relocate symbols that have a live consumer.
3. **A painless severed wire is a CUT candidate.** Absence of production pain is evidence,
   not a coincidence to preserve.
4. **A guard is a detector, not a judge.** Grep-diff catches what LLM audits miss
   (in Aleph it caught `memory.store` and `AiRetrievalPolicy` that the semantic sweep
   dropped) — but whether to CONNECT or CUT each flagged wire is a human/read decision.

## Why not just trust dead-code lints or an LLM sweep?

- Lints see only form 1. Stubs, inert config, client-ghost, and name-drift stay green.
- An LLM semantic sweep over-trusts the *handler* side ("there's a stub → it's a broken
  wire to fix") and under-checks the *client* side. It also silently misses entries — the
  grep-diff guard is what makes the audit exhaustive. Use both: LLM lenses for judgment,
  grep-diff for completeness.
