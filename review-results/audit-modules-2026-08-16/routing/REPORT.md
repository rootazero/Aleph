# Routing Module — Static Code Review (2026-08-16)

## Scope

- **Module:** `src/routing/` (9 files, 3,828 LOC)
- **Reviewer:** static review (seam / logic / architecture lenses)
- **Working tree:** `.worktrees/audit-2026-08-16-modules/`
- **Constraint:** review only; no edits. Skip issues already addressed in `8c537fb74`, `4671057d4`, `b14c28152`, `6cb10a0ce`.

Files reviewed:

| File | LOC |
|---|---|
| `mod.rs` | 291 |
| `config.rs` | 276 |
| `identity_links.rs` | 269 |
| `observer.rs` | 289 |
| `experience_store.rs` | 188 |
| `recall.rs` | 393 |
| `overlay.rs` | 200 |
| `resolve.rs` | 820 |
| `session_key.rs` | 1,102 |

Recent rebase context (already fixed, not re-reported):

- `8c537fb74 routing: rustfmt normalization`
- `4671057d4 routing: document send-seam DM/group key drift and identity_links fallback wiring gap`
- `b14c28152 routing: wire binding workspace to executor, unify DM fallback keys, delegate AgentRouter to resolve_route, guard inert account_id`
- `6cb10a0ce routing: cut dead re-exports, warn on recall failure, document identity_links scope`

## Summary

| Severity | Count |
|---|---|
| Critical | 0 |
| High | 3 |
| Medium | 5 |
| Low | 4 |
| **Total** | **12** |

By category: `logic` 5 · `architecture` 4 · `quality` 2 · `security` 1.

---

## Findings

### [High] src/routing/recall.rs:48–52 — Empty `api_key` is treated as configured & available

**Category:** logic / security
**Confidence:** High

`provider_availability_from_config` only checks `c.api_key.as_ref().is_some()`. A provider
configured with `api_key = ""` (empty string — a common operator typo or a TOML pattern
where the value is omitted but the key is preserved) is treated as `ProviderStatus::Available`,
which in the recall block means the entry is rendered with no `[UNAVAILABLE]` tag and the
LLM is free to pick a model that has no working credential.

```rust
let available = providers
    .get(provider)
    .and_then(|c| c.api_key.as_ref())
    .is_some()      // ← Some("") counts as available
```

Symmetrically the vault path looks up `ai:{provider}`, which is correct; the bug is only
on the config-side predicate. The recall block is the only place a misconfigured provider
would be silently recommended.

**Suggested fix:** change to `c.api_key.as_deref().is_some_and(|s| !s.trim().is_empty())`.

---

### [High] src/routing/experience_store.rs:11, 30–40 — Inert retention knob (form-3)

**Category:** architecture
**Confidence:** High

`RoutingExperienceStore::retention_cap` is set to `DEFAULT_ROUTING_RETENTION_CAP = 5000`
in `::new()` and never reconfigurable — the field is private, there is no setter, and
the constructor has no parameter. The `prune_routing_experiences` call is wired, but the
cap is a constant. Today this is a hidden assumption (5000 rows per agent); tomorrow's
deployment that needs 50k or 500 has no way to express it.

This is severed-wire form 3 (inert config): a piece of plumbing that does real work
(calls into `prune_routing_experiences`) but whose value is hardcoded.

**Suggested fix:** add `pub fn with_retention_cap(self, cap: usize) -> Self` (or accept
the cap in `::new`), and let `subsystems.rs` read it from `config` (or a sensible
default). Or, if the constant is intentional, drop the `retention_cap` field and inline
the constant at the call site — there is no reason to thread it through `self` if it
never varies.

---

### [High] src/routing/resolve.rs:140–155 — Empty `agent_id` in a binding silently collapses to default agent

**Category:** logic
**Confidence:** High

`binding_problems` (config.rs:91) reports a binding with `agent_id == ""` at boot, but the
runtime `build` closure in `resolve_route` still accepts it and routes to
`normalize_agent_id("")` → `DEFAULT_AGENT_ID = "main"`. The result is two failure modes
that look identical from the operator's seat:

1. A binding with an empty `agent_id` and a channel scope silently routes to `main`.
2. No binding at all routes to `main`.

When the operator looks at the bootstrap log and sees `binding_problems` flagging it,
then sees the same channel routed to `main` in production, they may think the warning
was a false alarm. The runtime path should honor the boot-time verdict: an empty
`agent_id` should produce a `None` match (fall through to default via the standard
`None` arm) so the source is indistinguishable from "no binding matched" — *not* from
"a binding matched and pointed at the default".

```rust
let agent_id = if trimmed.is_empty() {
    normalize_agent_id(trimmed)   // ← returns "main"; looks like a deliberate match
} else {
    trimmed.to_string()
};
```

**Suggested fix:** in the `match matched` arm, when `b.agent_id.trim().is_empty()`, treat
it as `None` (or, equivalently, skip the binding when the field is empty). The
boot-time `binding_problems` already warns the operator; the runtime path then degrades
the same way as a missing binding.

---

### [Medium] src/routing/recall.rs:94–95 — `attribution.task_emb.set` result discarded; first-wins silently

**Category:** logic
**Confidence:** High

`build_routing_experience_message` writes the recall-side embedding to the per-run
`RoutingAttribution` with `let _ = attribution.task_emb.set(task_emb.clone());`. The
`OnceLock` `set` returns `Err` if a value is already present. If the same
`RoutingAttribution` ever flows through recall twice (operator `/recall`, retry,
bug, hot-reload that reuses the handle), the second embedding is silently discarded and
the *observer* will record using the *first* embedding — breaking the D6 record/recall
symmetry that the whole VESR design rests on.

The two current producers (`runner_impl.rs:333` and `subagent_spawner/mod.rs:523`) each
construct a fresh `RoutingAttribution` per run, so a second set is unlikely today. But
the silent drop is the kind of latent failure that bites when the construction
discipline changes.

**Suggested fix:** on the `Err` arm, log a `warn!` with the session id and the new
embedding's model — the operator needs to know the recall is not idempotent. Or use
`OnceLock::get_or_init` instead of `set` and accept the first call's embedding; the
behavior is the same but the failure mode is documented.

---

### [Medium] src/routing/config.rs:38–49 — `identity_links` only honored on the configured-bindings path; fallback path silently drops it

**Category:** architecture
**Confidence:** High

This is documented in the field's docstring and called out in commit `4671057d4`. The
gap is still real: a deployment that ships `[[session]] identity_links` and *no*
`[[bindings]]` table gets a zero-config fallback that ignores identity links, so
cross-channel DM continuity never engages for the bulk of inbound traffic. The
`boot/builder/subsystems.rs:586` snapshot threads `session_cfg.dm_scope` into the
fallback-side `RoutingConfig` but not `identity_links`.

The wiring already exists on the configured-bindings path (`resolve_session_key_with_agent`
→ `session_keys_for` → `build_session_key` consults `identity_links`). The fix is to
push the same field through the fallback path. Until then, the docstring is doing the
work the code should be doing.

**Suggested fix:** add an `identity_links: HashMap<…>` field to
`gateway/routing_config::RoutingConfig`, mirror it in `subsystems.rs:586`, and have
`agent_resolver::resolve_session_key_with_agent` consult it (mirroring `build_session_key`).
Or, since the field is rarely configured, document the deployment expectation in
`config.search.example.toml` and the operator docs.

---

### [Medium] src/routing/observer.rs:93–133 — `OutcomeObserver` ignores every event except `SessionCompleted`

**Category:** architecture
**Confidence:** Medium

`on_trace` only matches `LoopTraceEvent::SessionCompleted`. All other variants
(`TurnStarted`, `ToolInvoked`, `TurnCompleted`, `ContextUpdated`, …) are forwarded to
`inner.on_trace(event)` unchanged but produce no `RoutingOutcome`. That is intentional
— the design is "record on session end" — but the asymmetry is invisible to anyone
reading the trait without context. The tracing logs `recording routing experience` are
emitted iff the `task_emb` was set, which makes "we never recorded anything" look
identical to "we recorded zero rows because the store was empty" on the operator side.

The dead-code lint does not catch this (the `LoopTraceEvent` match is non-exhaustive
on purpose — it's an `_` implicit), and the test suite covers the positive path only.

**Suggested fix:** consider emitting a `debug!` (or `trace!`) on non-`SessionCompleted`
events so the absence of "recording routing experience" logs is debuggable. Or document
the "session-end only" contract on the `OutcomeObserver::new` docstring.

---

### [Medium] src/routing/session_key.rs:760–770 — `[main_key]` parse arm silently excludes `peer | dm | ephemeral`

**Category:** logic
**Confidence:** High

`parse_rest` for the single-segment rest arm rejects `main_key` values of `peer`, `dm`,
and `ephemeral`. This is necessary (without the guard, `agent:id:dm:s1` would parse as
a `Main{main_key:"dm"}` after the epoch strip), but the constraint is silent — a
`Main` key whose `main_key` is any of those three strings will not round-trip through
`parse(key.to_key_string())`.

Today no built-in constructor produces such a key (the `project_room` constructor goes
through `sanitize_component`, which would mangle `dm` into `dm` only if the project id
literally is `dm` — in that case the room key would still be `Main{main_key:"dm"}`, the
intended `Main` variant, but parsing would return `DirectMessage` and the round-trip
would break).

The compound failure: a misbehaving future caller that chooses `main_key = "dm"` for a
room gets a key that *looks* main but is secretly a DM after storage. The `is_interactive`
gate would still be `true`, but the session store would not match.

**Suggested fix:** add a unit test that round-trips `SessionKey::Main{main_key: "dm"}` and
asserts the parse result, then either reject the input in `project_room` (panic /
saturate) or add a `None` arm to `parse_rest` so the symptom is a parse failure rather
than a silent re-type.

---

### [Medium] src/routing/session_key.rs:178–185 — `SessionKey::task` panics on reserved task type

**Category:** quality
**Confidence:** High

`SessionKey::task` panics when `task_type` normalizes to `peer`, `dm`, or `ephemeral`.
A library panic on a `String` input is a code smell: the caller has no `Result` to
match, no docstring nudge (the docstring says "must not be" but does not say the
function panics), and there is no shared `is_reserved_task_type` helper to centralise
the rule. The same reservation exists in `parse_rest` as a silent re-route, so the two
sides disagree on what the error surface is.

**Suggested fix:** either return `Result<Self, ReservedTaskType>` from `task()` and let
the caller decide, or expose `pub const fn is_reserved_task_type(s: &str) -> bool` so the
guard is grep-able and the panic has a documented entry point.

---

### [Low] src/routing/experience_store.rs:79–85 — `context_tokens` / `context_window` columns hardcoded to 0

**Category:** quality
**Confidence:** High

The two fields are documented as "stays in the schema for row compatibility" and the
type deliberately omits them from `RoutingOutcome`. The hardcoded `0` write is dead
code that pretends to be data. The recall renderer (`recall.rs::render_aggregates`)
does not display them, so a future renderer that picks them up will see zerod values
that look like real measurements.

**Suggested fix:** either default to `NULL` in the column (so reads can distinguish
"unmeasured" from "measured zero") or remove the columns entirely. The current state
is the worst of both worlds: the schema says "always zero" while the type says "not
measurable".

---

### [Low] src/routing/resolve.rs:135–137 — `agent_id` is reported verbatim, but the session key is normalised

**Category:** architecture
**Confidence:** Medium

The `build` closure in `resolve_route` returns `agent_id` as the trimmed config string
(e.g. `Work_Bot`), but `session_key` is built via `session_keys_for`/`build_session_key`
which normalises via `normalize_agent_id` (→ `work_bot`). The split is documented and
*intentional* (registry lookup uses `Work_Bot`, filesystem-safe key uses `work_bot`),
but the asymmetry is easy to misread. The split also means a binding-targeted
`agent_registry.contains(agent_id)` call must use the raw id, while a session-store
lookup must use the normalised id — future authors will get this wrong unless the
naming difference is repeated in every docs comment.

The relevant test (`agent_id_is_reported_as_configured_while_the_key_stays_normalised`)
pins the contract, but the wording "**while** the key stays normalised" is a load-bearing
naming convention, not a property.

**Suggested fix:** add a `/// verified-verbatim-id` field constructor / accessor on
`ResolvedRoute` so the boundary is encoded in the type, or rename the field to
`agent_id_verbatim` to make the contract grep-able.

---

### [Low] src/routing/session_key.rs:267–270 — `format_dm_base` has a `PerPeer if channel.is_empty()` branch that is reachable only through legacy callers

**Category:** quality
**Confidence:** Medium

The `PerPeer if channel.is_empty()` arm produces `agent:{agent_id}:peer:{peer_id}`. The
only producer of an empty-channel DM today is `SessionKey::peer(agent, peer_id)` (the
legacy compatibility alias), which routes through `dm(agent, "", peer_id, PerPeer)`.
The arm is reachable and the tests assert it (`test_to_key_string_peer`).

Not dead code today, but the comment ("legacy compatibility alias") and the arm's
existence are now load-bearing: if `SessionKey::peer` is ever removed, the `is_empty()`
branch becomes unreachable. The kind of code that survives a refactor by accident.

**Suggested fix:** add a `#[cfg(test)]` round-trip test that pairs every `SessionKey::peer`
construction with `parse(peer.to_key_string())`, so the legacy path stays
under-test after the constructor is pruned.

---

### [Low] src/routing/recall.rs:42–52 — `ProviderStatus::Deconfigured` vs `Unknown` is inverted by the vault path

**Category:** logic
**Confidence:** Medium

The predicate combines `config api_key` and `vault ai:{provider}` with `||`. That is
correct on the "available" side. But the next branch is `if providers.contains_key(provider)`
→ `Deconfigured`, otherwise `Unknown`. A provider that is *only* in the vault (not in
the config map) — e.g. an A2A handler registered at runtime — is treated as `Unknown`,
which `fail_opens` the recall block tag into "no `UNAVAILABLE` warning". This is the
intended behavior of `Unknown` (don't penalize what we cannot identify), but the
mental model the operator builds from `ProviderConfig` is "every provider that *could*
answer is in this map". A vault-only provider is refused with `ProviderStatus::Unknown`
and the model is recommended as if it were a real, working config — the LLM will
discover the misfire on the first call, not in the recall block.

**Suggested fix:** document the "vault-only providers are unobservable from the gate"
contract on the `ProviderStatus::Unknown` docstring, or wire the vault predicate into
the `Deconfigured` arm (vault present but config absent → `Unknown` is a confusing
label).

---

## Out-of-scope but flagged

The following wires are severed *intentionally* and are documented in `config.rs`,
`session_key.rs`, and the call sites — they are not findings, but they are the
load-bearing assumptions the routing module makes about the rest of the codebase:

- `identity_links` does not flow into the zero-config fallback (`agent_resolver.rs::resolve_session_key_with_agent`).
- `multi-account` is `None` in the inbound `RouteInput` (commented `// TODO: multi-account support` on line 100 of `agent_resolver.rs`).
- `RouteInput::account_id` is normalised to `"default"` in `resolve_route` when empty; the only current caller does not pass it.

---

## What was NOT done

- No code was modified (review-only constraint).
- Runtime / integration tests were not executed; only the in-module `#[cfg(test)]` and
  `integration_tests` *were read*. Behavioural grounds are noted where the test prints
  pin a contract.
- The `subsystems.rs` wiring was *read* but not audited under the seam lens in full
  — the `with_route_bindings` call (subsystems.rs:891) was confirmed to pass
  `session_cfg.clone()` (including `identity_links`); the fallback path
  (`with_dm_scope`) was confirmed to drop it. This is the basis for finding #2 (Medium),
  not a new seam.
- `graphify` was not invoked — the cross-module graph is not in this worktree. The
  RE-export fan-out from `mod.rs` was hand-checked via `grep`.
- The `binding_problems` precondition (binding_problems runs at boot, never at hot-reload)
  was *not* audited — the runtime hot-reload path is in `subsystems.rs` and outside the
  stated scope.
- The `aggregate_by_model` SQL query (routing_experience.rs:334) was not reviewed line-by-line;
  the wiring is correct (it is called from `recall.rs:100` and the result is rendered).
