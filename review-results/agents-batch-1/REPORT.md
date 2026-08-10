# Review Report — Batch 1 (Top-level agents module: types / registry / loader / runtime)

**Scope:** `src/agents/mod.rs`, `src/agents/types.rs`, `src/agents/registry.rs`, `src/agents/loader.rs`, `src/agents/runtime.rs`, `src/agents/run_context.rs`
**Date:** 2026-08-10
**Reviewer:** static (4-perspective protocol)
**Worktree:** `/tmp/aleph-review-agents` (branch `review/agents`)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 3 |
| Medium   | 3 |
| Low      | 3 |

The three High findings compose into one chain: a three-line agent markdown file dropped
into the directory the server happens to be started from becomes a **globally registered,
spawnable, wildcard-tool agent that can claim the id `main`** — and the prompt catalog keeps
advertising the builtin's read-only description for it.

---

## Findings

### [HIGH] src/agents/loader.rs:154 — An explicit `allowed_tools: []` (and an omitted key) silently becomes the wildcard `["*"]`

**Category:** Security / Logic
**Confidence:** High

**Description:**
`AgentDef::new` seeds `allowed_tools = vec!["*"]` (`types.rs:163`). The loader only overrides
it when the parsed list is **non-empty**:

```rust
// loader.rs:154
if !fm.allowed_tools.is_empty() {
    def = def.with_allowed_tools(fm.allowed_tools);
}
```

So an author who writes the natural deny-all form — `allowed_tools: []` — gets the exact
opposite: every tool. The value is enforced verbatim by `AllowlistToolService::execute`
(`allowlist_tool_service.rs:39`), so this is the sub-agent's real permission surface, not a
cosmetic default. `allowed_tool_sets` accidentally masks the bug when present (the builder
clears the wildcard, `types.rs:206`), which is why it never shows up in the set-based
builtins.

The existing loader test cannot catch it — it asserts a disjunction that is true either way:

```rust
// loader.rs:378
assert!(def.allowed_tools.is_empty() || def.allowed_tools == vec!["*"]);
```

**Failure scenario:** `~/.aleph/data/agents/scratch.md` with
`allowed_tools: []` and no `allowed_tool_sets` → the agent is spawnable and `is_tool_allowed`
returns `true` for `bash`, `file_write`, `file_ops`, every MCP tool, etc.

**Suggested fix:** make the field provenance-explicit and reject the ambiguous case:

```rust
#[serde(default)]
allowed_tools: Option<Vec<String>>,   // None = key absent, Some(vec![]) = deny-all
...
match fm.allowed_tools {
    Some(list) => def = def.with_allowed_tools(list),   // empty list stays empty
    None => {}                                          // documented "absent = inherit default"
}
```
and change the test to assert the two cases separately (`absent → ["*"]`, `[] → []`).

---

### [HIGH] src/agents/registry.rs:196 (+ loader.rs:145, loader.rs:253) — Builtin agent ids are not reserved: a user/project file named `main.md` re-opens the escalation `resolve_spawnable` was added to close

**Category:** Security
**Confidence:** High

**Description:**
`resolve_spawnable` (registry.rs:159-166) exists precisely to stop `agent_type = "main"` from
resolving the Primary wildcard def, and its doc states: *"the builtin `main` is the only
definition this filter can reject."* That claim holds only while `main` **is** Primary.

Nothing reserves builtin ids. `load_agents` seeds the map with builtins (loader.rs:253-255)
and then lets any user/project file overwrite them by id (`insert_with_shadow`,
loader.rs:282); `parse_file` hard-codes `AgentMode::SubAgent` for every disk-loaded def
(loader.rs:145); `register_from_dirs` then registers it into the live registry, where
`register` is an unconditional `HashMap::insert` (registry.rs:84).

Net effect of a file at `<dir>/.aleph/agents/main.md` containing only the three required
fields:

```yaml
---
id: main
description: Primary agent that responds directly to user
when_to_use: anything
---
```

1. It passes the `ForbiddenSystemField{mode}` gate — it never sets `mode`, the loader sets
   `SubAgent` for it.
2. Combined with **HIGH #1**, its `allowed_tools` is `["*"]`.
3. `resolve_spawnable("main", …)` now returns `Some` (mode is `SubAgent`), and
   `spawnable_agent_ids()` advertises `main` to the model.
4. The primary run's own definition (`prompt_build.rs:327`, `agent_registry.get(agent_id)`)
   is now this file's, not the builtin's.

The two steps are individually legal and jointly equivalent to the thing the gate forbids —
the mode filter checks a field the file is allowed to change indirectly.

**Suggested fix:** reject collisions with builtin ids at load time (or force them to lose):

```rust
// loader.rs, in parse_file or insert_with_shadow
const RESERVED: &[&str] = /* derive from builtin_agents() ids where mode == Primary */;
if RESERVED.contains(&fm.id.as_str()) {
    return Err(LoaderError::ForbiddenSystemField { path: …, field: "id" });
}
```
Deriving `RESERVED` from `builtin_agents()` (not a literal list) keeps it from drifting.

---

### [HIGH] src/agents/registry.rs:190 vs src/bin/aleph-server/commands/start/orchestrator_init.rs:91 — `register_from_dirs`'s doc says boot passes `project_dir = None`; the only boot caller passes the process CWD

**Category:** Security / Architecture
**Confidence:** High

**Description:**
`register_from_dirs`'s doc comment states:

> *"Boot wiring passes `project_dir = None` (the desktop daemon has no active project at
> boot). Per-run project overlay is exposed via `lookup_with_overlay` … without reloading the
> global registry."*

The actual and only boot call site does the opposite:

```rust
// orchestrator_init.rs:90-93
let project_dir = std::env::current_dir().ok();
…
match agent_registry.register_from_dirs(home, project_dir.as_deref()) {
```

So `<cwd>/.aleph/agents/*.md` is loaded into the **process-global, long-lived** registry at
startup — not scoped to runs in that project, not re-read, and visible to every session,
every agent and every channel for the process lifetime. That is the exact blast-radius the
doc claims is avoided, and it turns the directory the operator happened to `cd` into before
`aleph-server start` into a permanent agent-definition source. Chained with the two findings
above, `git clone <repo> && cd <repo> && aleph-server start` is enough for the repo to define
a globally spawnable wildcard `main`.

Per 判据清单 §0 ("同一事实的两份表述，只改一份就是静默说谎"), one of the two is wrong; the
security-relevant reading is that the code is.

**Suggested fix:** decide and pin it. If the documented design is intended, pass `None` at
boot and rely on `lookup_with_overlay` for per-run project agents. If CWD loading is
intended, fix the doc **and** gate it (opt-in config, or restrict registered project defs to
the run's project rather than the global registry) — plus a test that pins the boot call's
argument so the doc/code pair cannot drift again.

---

### [MEDIUM] src/agents/registry.rs:225 vs src/orchestrator/harness_bridge/prompt_build.rs:458 — the advertised agent catalog is built without the project overlay that the spawn path applies

**Category:** Security / Architecture (判据清单 §0 "一个动词有 N 个面时，谁能看要在每个面用同一个推导")
**Confidence:** High

**Description:**
Two faces of one verb, two different predicates:

| Face | Source | Overlay applied? |
|---|---|---|
| `<available_agents>` catalog (what the model reads) | `builtin_agents()` ∪ `agent_registry.list_subagents()` ∪ `plugin_subagents()` — prompt_build.rs:452-464 | **No** |
| Current agent's own def in the prompt | `agent_registry.get(agent_id)` — prompt_build.rs:327 | **No** |
| Spawn resolution | `resolve_spawnable` → `resolve` → `lookup_with_overlay` — registry.rs:159/264/225 | **Yes** |

`<project>/.aleph/agents/explore.md` therefore replaces the read-only `explore` at spawn time
(different `allowed_tools`, `denied_tools` reset to empty, different `max_iterations`) while
the model is still told *"Read-only codebase exploration specialist"* and *"search, read, or
understand code without modifying anything"*. The loader blocks only `mode` and `source`
(loader.rs:117-128) — nothing about tool grants — so the shadowing def is free to be
write-capable. There is also no `ShadowEvent` on this path: `load_project_overlay` bypasses
`insert_with_shadow` entirely, so unlike the boot tier this substitution is never logged.

**Suggested fix:** build the catalog through the same predicate as the gate — give
`prompt_build` the run's `project_root` and fold `load_project_overlay(root)` in with the same
precedence `resolve` uses, or expose one `fn delegatable_defs(&self, project_root) ->
Vec<AgentDef>` on the registry and make both faces call it. A test asserting
"every id the catalog advertises resolves to a def byte-identical to what `resolve_spawnable`
returns for the same `project_root`" pins it.

---

### [MEDIUM] src/agents/runtime.rs:719-752 — the subagent transcript store has exactly one writer and zero readers, and the writer runs a destructive GC

**Category:** Architecture (R10 "零现有消费者的抽象立即删除/撤回") / Quality
**Confidence:** High

**Description:**
`persist_transcript` writes `<config>/data/transcripts/<chain_id>/<agent_id>.json` on every
subagent completion. `data/transcripts` occurs exactly once in the whole repo — this write.
Nothing reads the directory, no RPC serves it, no tool exposes it, `SubagentTranscript` is
deserialized only in this file's own round-trip tests (runtime.rs:990, 1012). The durable
subagent record consumers actually use is the `SubagentSpawned`/`SubagentReturned` session
event pair.

Two consequences beyond the dead code itself:

1. Every successful write calls `cleanup_old_transcripts` (runtime.rs:747), which
   `remove_dir_all`s directories under the transcripts root — a delete path maintained for
   data no one consumes.
2. The data is wrong anyway on the paths that would matter most: the error/timeout arm
   hard-codes `iterations: 0, tokens_used: 0` (runtime.rs:436-437) even though a run that
   timed out after N minutes burned both, and `key_findings` is forced empty (runtime.rs:408).

**Suggested fix:** CUT — delete `persist_transcript`, `cleanup_old_transcripts`,
`MAX_TRANSCRIPT_DIRS` and the detached `spawn_blocking` at runtime.rs:461. If observability
is genuinely wanted, route it through the existing `TraceSink` (already wired on this struct,
runtime.rs:137) instead of a second, unread store. Do not "fix" the zeroed metrics first —
that reconnects a dead wire (severed-wire triage: read-before-write).

---

### [MEDIUM] src/agents/types.rs:204-210 — `with_allowed_tool_sets` uses a value check where it needs a provenance check, silently dropping an author's explicit `["*"]`

**Category:** Logic
**Confidence:** High

**Description:**

```rust
// types.rs:204-210
pub fn with_allowed_tool_sets(mut self, sets: Vec<String>) -> Self {
    self.allowed_tool_sets = sets;
    if self.allowed_tools.len() == 1 && self.allowed_tools.first().is_some_and(|s| s == "*") {
        self.allowed_tools = vec![];   // "still at its constructor default"
    }
    self
}
```

The doc says *"If `allowed_tools` is still at its constructor default `["*"]`"* — but the code
cannot distinguish the constructor default from an author who deliberately wrote `["*"]`.
The loader applies the flat list first (loader.rs:154) and the sets second (loader.rs:185),
so a frontmatter carrying **both** `allowed_tools: ["*"]` and any `allowed_tool_sets` has its
wildcard silently deleted, with no warning and no way to express "wildcard plus a set".
(The direction is fail-closed, so this is a correctness/UX bug, not an escalation — but it is
invisible: `agent_manage info` will report the narrowed list as if the author wrote it.)

**Suggested fix:** track provenance instead of inspecting the value — e.g. a private
`allowed_tools_explicit: bool` set by `with_allowed_tools`, or have the loader decide
(it knows whether the key was present) and never let the builder guess. The doc comment's
"Callers wanting both should chain `with_allowed_tools` after this method" is not reachable
from the loader, which has a fixed order.

---

### [LOW] src/agents/registry.rs:132-138 — `spawnable_agent_ids` re-derives the spawnability predicate and omits the mode filter for plugin agents

**Category:** Security (latent) / Architecture
**Confidence:** High

**Description:**
`resolve_spawnable` filters `mode == AgentMode::SubAgent` for *every* source (registry.rs:165).
`spawnable_agent_ids` applies it only to the registry half (via `list_subagents`) and then
extends with **every** published plugin id unfiltered:

```rust
// registry.rs:133-135
let mut ids: Vec<String> = self.list_subagents().into_iter().map(|a| a.id).collect();
ids.extend(plugin_subagents().iter().map(|a| a.id.clone()));
```

Today this is benign because `extension/mod.rs:205` always constructs plugin defs as
`AgentMode::SubAgent`. But the gate and the disclosure surface derive the predicate
independently — the exact failure mode `resolve_spawnable`'s own doc warns about ("they are
the two faces of one verb and must share a predicate"). The first plugin def built as
`Primary` yields a list that advertises an id every spawn attempt then rejects.

**Suggested fix:**

```rust
ids.extend(
    plugin_subagents().iter()
        .filter(|a| a.mode == AgentMode::SubAgent)
        .map(|a| a.id.clone()),
);
```

---

### [LOW] src/agents/loader.rs:130-143 — agent ids are validated only against the file stem; no charset/reserved-name check

**Category:** Security (defense in depth)
**Confidence:** High

**Description:**
The only id validation is `stem == fm.id`. That blocks `/` and `\` (a stem cannot contain
them) but not everything: a file named `...md` yields `file_stem() == ".."`, and ids with
spaces, control characters, XML metacharacters or arbitrary Unicode pass unchanged.

The id then flows into: the transcript path (sanitized — runtime.rs:722-726), the per-agent
config dir (validated and rejected — `utils::paths::get_agent_config_dir`, paths.rs:732-741),
the memory partition key (`session_write_id`, subagent_spawner/mod.rs:620), the signed
identity ledger actor (`identity::as_actor`, allowlist_tool_service.rs:45) and the
`<available_agents>` prompt text. Downstream defenses catch the path cases today, so there is
no live traversal — but validation belongs at the parse boundary (P7 "系统边界校验"), not
spread across four consumers each of which must remember.

**Suggested fix:** in `parse_file`, before anything else:

```rust
if fm.id.is_empty() || fm.id == "." || fm.id == ".."
    || !fm.id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
{
    return Err(LoaderError::InvalidValue { path: …, message: format!("invalid agent id '{}'", fm.id) });
}
```

---

### [LOW] src/agents/runtime.rs:686-715 — `MAX_TRANSCRIPT_DIRS` is documented "per session" but enforced across the whole transcripts root, and degenerates if the session component is empty

**Category:** Logic / Quality
**Confidence:** High

**Description:**
The constant is documented as *"Maximum transcript directories to retain per session"*
(runtime.rs:685), but `cleanup_old_transcripts` walks `base_dir.parent()` — i.e. the shared
`<config>/data/transcripts` root — and deletes the oldest directories beyond 50 across **all**
sessions/chains (runtime.rs:689-714). A busy day therefore silently deletes older sessions'
transcripts, which is the opposite of what the name promises.

Separately, the GC's anchor is derived by string surgery rather than being passed in: if
`safe_session` were ever empty, `base` collapses to `<config>/data/transcripts/` whose
`parent()` is `<config>/data` — the enumerate-and-`remove_dir_all` loop would then target
Aleph's other state directories. `chain_id` is always non-empty in production
(`generate_chain_id`, harness/chain_context.rs:12) and `ChainContext`'s fields being `pub`
is the only way to get there, so this is a latent hazard rather than a live bug — but the
function should not be able to walk above the directory it owns.

**Suggested fix:** if the store survives finding #5 at all, pass the root explicitly
(`cleanup_old_transcripts(&transcripts_root)`) instead of deriving it with `.parent()`, and
either rename the constant to `MAX_TRANSCRIPT_SESSIONS` or scope the retention to `base`.

---

## Cross-cutting observations

- **`AgentDef` has no validating constructor.** Every guard in this module is a check some
  *caller* remembers to run: `mode` is guarded in the loader, ids in `utils::paths`, tool
  grants nowhere. `AgentDef` is `pub` with `pub` fields and `Deserialize`, so any future
  deserialization site (an RPC that accepts an agent def, a plugin bridge, a config reload)
  gets the unvalidated version for free. A `AgentDef::try_from_untrusted(...)` chokepoint
  would collapse findings #1, #2 and #7 into one place.
- **Two different `current_agent_id()` functions.** `crate::agents::current_agent_id`
  (run_context.rs:38, run-scoped identity for `~/.aleph/agents/<id>/skills`) and
  `crate::tools::turn_context::current_agent_id` (turn_context.rs:134, derived from
  `session_key.agent_id()`). Same name, different task-locals, different lifetimes, both
  `pub`. This is the shape 判据清单 §0 flags for `ambient_owner` vs `ambient_actor`; today no
  caller in the reviewed files mixes them up, but nothing stops one. Worth a doc census that
  says which question each answers.
- **`run_context.rs` itself is clean.** The task-local is correctly scoped by the future
  (`CURRENT_AGENT_ID.scope`), `try_with(...).ok().flatten()` cannot panic outside a scope, and
  the documented "capture before `tokio::spawn`" rule is actually honoured by the spawn sites
  (`CarriedAttribution::capture`/`reestablish`, scope/carried.rs:58-102, which carries
  `agent_id` among its four task-locals). No leak or race found.
- **Timeout classification is sound.** `e.starts_with("Sub-agent timed out")` (runtime.rs:426)
  matches the spawner's only producer of that string (subagent_spawner/mod.rs:668); the
  sibling arms use distinct prefixes (`sub-agent panicked:`, `sub-agent failed:`), so a
  model- or provider-controlled message cannot reach the `Timeout` arm. No spoofing path.
- **`resolve_spawn_route` (runtime.rs:612-673) reads correct** — precedence documented and
  tested, HashMap iteration made deterministic by the `min_by` tie-break (runtime.rs:659),
  blank-half rejection in `split_provider_prefix`. No findings.
- **File length / P2.** `runtime.rs` is 1017 lines (≈750 production) and `registry.rs` is 911
  (≈450 production); both exceed the 500-line split guideline. `AgentRuntime` carries 25
  fields with 20 `with_*` builders — a pure config carrier that would read better as a
  `SpawnerBase`-shaped struct the runtime holds, since `execute_via_harness` copies all 25
  into `SpawnerBase` one-to-one (runtime.rs:529-563).
- **`Some("")` conflation in the hook payloads.** `self.parent_session_id.unwrap_or_default()`
  (runtime.rs:383, 470) feeds an empty `session_id` into `HookContext` when the parent session
  is unknown. Impact here is a mislabelled hook env var, not routing — flagged only because
  the repo has been bitten by this shape before.
- **`plugin_subagents()` is consulted per resolve, per catalog build.** It's a cheap
  `Arc::clone` under an `RwLock`, and poisoning is handled (`PoisonError::into_inner`,
  registry.rs:33/43). Lock handling across the whole registry is consistent and correct.

## Files reviewed

| File | LOC | Notes |
|---|---|---|
| `src/agents/mod.rs` | 52 | Re-export surface only; no findings |
| `src/agents/types.rs` | 644 | 1 Medium (#6); recursion guard verified correct |
| `src/agents/registry.rs` | 911 | 2 High (#2, #3), 1 Medium (#4), 1 Low (#7) |
| `src/agents/loader.rs` | 510 | 1 High (#1), 1 Low (#8) |
| `src/agents/runtime.rs` | 1017 | 1 Medium (#5), 1 Low (#9) |
| `src/agents/run_context.rs` | 63 | Clean |
| **Total** | **3197** | |

Consulted for verification (not in scope, not reviewed): `src/agents/allowlist_tool_service.rs`,
`src/agents/subagent_spawner/mod.rs`, `src/agents/subagent_tool/spawn.rs`,
`src/orchestrator/harness_bridge/prompt_build.rs`, `src/bin/aleph-server/commands/start/orchestrator_init.rs`,
`src/utils/paths.rs`, `src/scope/carried.rs`, `src/harness/chain_context.rs`, `src/extension/mod.rs`,
`src/extension/hooks/mod.rs`, `src/tools/turn_context.rs`.
