# Seam Catalog — where wires get severed, and the 6 forms

Read this before phase 1 (scan). It tells each scan lens what a severed wire looks like at
that seam, so the grep-diff has a target. The concrete cases are from the 2026-07-15 Aleph
audit and are kept as illustrations — the **patterns** generalize to any codebase.

## Contents

- [The 6 forms of a severed wire](#the-6-forms)
- [Seam types (scan lenses)](#seam-types)
- [Why one lens is never enough](#why-multiple-lenses)

## The 6 forms

A "wire" is: producer output → (registration / dispatch / subscription / call / config
read) → consumer input. It can be severed in six distinct ways. Only **form 1** is visible
to `dead_code` lints; the rest compile clean.

| # | Form | What it looks like | Lint sees it? |
|---|------|--------------------|---------------|
| 1 | **No consumer** | A producer (type/fn/tool) with zero callers | ✅ yes (dead_code) |
| 2 | **Stub far-end** | The wire connects, but the other end is a fake: a `// TODO persist` handler that validates then returns `success` without doing anything | ❌ no |
| 3 | **Inert config** | A config section/field is defined and parsed, but no non-test code path ever *reads* it (a dead knob) | ❌ no |
| 4 | **Client ghost** | A client/caller invokes a method name that has **no handler registered** → runtime NOT_FOUND | ❌ no (different crates) |
| 5 | **Name / path drift** | Two ends agree in spirit but disagree on the literal key: `session.delete` (singular) vs `sessions.delete` (plural); a destructive method that lands in the loose rate-limit bucket because the classifier matches the wrong spelling | ❌ no |
| 6 | **Never-compiled far-end** | A whole test/impl block behind a feature flag that isn't a real feature, so it never compiles and never guards the wire (Aleph: a BDD suite under `#[cfg(feature="gateway")]` where `gateway` was never a declared feature — which is *why* the unwired tool was never caught) | ❌ no |

**Key consequence:** because forms 2–6 are invisible to the compiler, you cannot audit by
"does it build". You must audit by **DEFINED − CONSUMED symbol parity** at each seam.

## Seam types

Each seam is one scan lens. Fan them out as independent parallel read-only passes. The
Aleph audit used seven; the generalized names are on the left.

1. **Registration parity (tool/plugin/command catalog).**
   `DEFINED` = every `const NAME = "x"` (or equivalent registration key).
   `CONSUMED` = every dispatch arm / match on that name.
   Severed = a tool the model can never call though it's fully implemented+tested.
   *Aleph:* `vision`, `sessions_spawn`, `invalid` — defined tools with no dispatch arm.

2. **Call-vs-handler parity (RPC / API methods).**
   `DEFINED` = every client `rpc_call("x")` and every classifier that names `"x"`.
   `CONSUMED` = every registered server handler for `"x"`.
   Severed = **client ghost** (form 4): `config.set` / `sessions.set_pinned` / `skills.add`
   called by the client, no handler → NOT_FOUND.

3. **Classifier-vs-handler parity (security/routing tables).**
   `DEFINED` = every method a rate-limiter / lane / permission classifier scores.
   `CONSUMED` = every method with a real handler.
   Severed = a **ghost classification** (scoring a method that doesn't exist) OR a **name
   drift** (form 5) putting a real destructive method in the wrong bucket.
   *Aleph:* `session.delete` (singular ghost) vs `sessions.delete` (real handler in the
   loose bucket) — a genuine security gap.

4. **Event emit-vs-subscribe parity.**
   `DEFINED` = every event variant emitted on a bus.
   `CONSUMED` = every subscriber that matches that variant.
   Severed = an emitter with no live subscriber, or a subscriber waiting on a bus that
   never carries the variant it needs.
   *Aleph:* `task_wait` subscribed to `AgentMessageBus`, but task-completion events were
   broadcast only to `GlobalBus` → the waiter woke only on a 300s timeout fallback.

5. **Config-reader parity (inert config, form 3).**
   `DEFINED` = every config section/field type.
   `CONSUMED` = every non-test read of that field.
   Severed = a knob nobody reads. *Then* decide CONNECT (something reads a hardcoded
   default and should read the field) vs CUT (nobody wants it).
   *Aleph:* `[policies.metrics]` was inert but had a live consumer reading a hardcoded
   `DEFAULT_WARNING_MULTIPLIER` → **CONNECT**. `[policies.{intent,keyword,...}]` were inert
   with no consumer → **CUT**.

6. **Path / route parity.**
   `DEFINED` = every path/route/name on one side of a boundary.
   `CONSUMED` = the matching set on the other side.
   Severed = a spelling/pluralization/casing mismatch across the seam (form 5).

7. **Stub sweep (form 2).**
   Grep for `// TODO`, `unimplemented!`, `todo!`, handlers that `return Ok(success)` with
   no side effect, `&[]` empty field tables. Each is a wire whose far end is a fake person.
   *Aleph:* `discord.save_config` validated then returned success without persisting;
   `gateway/handlers/generation.rs` was four empty `// TODO` stubs with no registration.

## Why multiple lenses

No single grep angle finds everything, and the grep-diff guard finds what the LLM sweep
misses. Two proven bonus catches from Aleph, where the mechanical `DEFINED − CONSUMED`
diff caught wires the semantic (LLM) audit had dropped:

- **`memory.store`** — in the rate-limiter's RpcWrite list, but `memory` only registers
  `search/stats/delete/clear/facts/trace`; no `store` handler exists. Pure grep-diff catch.
- **`AiRetrievalPolicy`** — an inert config type nested under the memory group; the
  semantic sweep skipped it, the config parity guard flagged it.

This is the core reason phase 5 exists: **LLM lenses provide judgment, grep-diff provides
completeness.** Run both.
