# Review Report — Batch 3 (SubagentTool pipeline)

**Scope:** `src/agents/subagent_tool/{mod,loop_tool,parse,spawn,recovery,types}.rs` (production code only; `tests.rs` excluded by instruction)
**Date:** 2026-08-10
**Reviewer:** static (4-perspective protocol)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 1 |
| Medium   | 5 |
| Low      | 3 |

## Findings

### [HIGH] parse.rs:185 — Malformed `batch_tasks` entries are silently dropped; an all-malformed array silently collapses the fan-out into a single run
**Category:** Logic
**Confidence:** High
**Description:** `batch_tasks` is parsed with `filter_map(|item| { let task = item.get("task")?.as_str()?...; if task.trim().is_empty() { return None } ... })` (parse.rs:185-209). Every entry that is not an object with a non-empty string `task` — including the very plausible LLM shape `batch_tasks: ["do X", "do Y"]` (array of strings) or `{"prompt": "..."}` — is dropped with no diagnostic. Two failure modes follow:
1. **Partial drop:** a 5-row request runs 4 children; the response's `count` / `results[].index` renumber from 0, so nothing in the output tells the model a row was lost. It reads as a complete `batch_completed`.
2. **Total collapse:** when *every* entry is malformed, `batch_tasks == Some(vec![])`, so `has_batch` is `false` (parse.rs:211) and `effective_batch` falls through to `None` (loop_tool.rs:710-723). The call then executes the **top-level `task` as one ordinary sub-agent** and returns `{"result": …}` — a fan-out request answered by a single child, reported as a normal success.

This directly contradicts this module's own stated contract: `reject_unknown_keys` exists precisely because "a near-miss … ran with a *different* meaning than the caller asked for … and reported success. Rejecting is the honest answer" (parse.rs:16-24). The rule is enforced for key *names* and abandoned for entry *shape* one level down.

**Suggested fix:** replace `filter_map` with a fallible fold that returns `Err` naming the offending index and what was wrong (mirroring the existing `"batch task {idx}: Unknown agent_type …"` message style used in loop_tool.rs:760), e.g.:
```rust
let mut rows = Vec::new();
for (idx, item) in arr.iter().enumerate() {
    let task = item.get("task").and_then(Value::as_str)
        .ok_or_else(|| format!("batch_tasks[{idx}] requires a non-empty string 'task'"))?;
    if task.trim().is_empty() { return Err(format!("batch_tasks[{idx}]: 'task' must not be empty")); }
    ...
}
```
Also make `batch_tasks` present-but-not-an-array an error rather than `None`.

---

### [MEDIUM] parse.rs:240 / :246 / :297 — Wrong-typed scalars are silently coerced to defaults while unknown keys are rejected loudly
**Category:** Logic / Security (contract integrity)
**Confidence:** High
**Description:** `timeout_secs` (`as_u64`, parse.rs:240-244 and :133-137), `run_in_background` (`as_bool`, :246-249), `synthesize` (`as_bool`, :297-300) and `proposer_models` / `request_ids` (`as_array`) all fall back to a default when the JSON type does not match, instead of erroring. Concretely: `"run_in_background": "true"` (a string — a very common provider/model emission) parses to `false`, so the call **blocks the parent turn synchronously** for up to 1500 s instead of returning a `request_id`; `"timeout_secs": 300.0` (a float) silently becomes 120 s and the child is killed at 120 s; `"synthesize": "true"` skips the MoA reduce and returns a raw batch that reads like a completed synthesis was never requested. Same defect class as the finding above and the same one `reject_unknown_keys` was written to prevent — the parser validates the *key set* strictly and the *value types* not at all.
**Suggested fix:** for each of these, distinguish "absent/null" from "present with wrong type": `match input.get(k) { None | Some(Value::Null) => default, Some(v) => v.as_bool().ok_or("'run_in_background' must be a boolean")? }`. A small helper (`req_bool` / `req_u64` / `req_array`) keeps it to one line per field and one message shape.

---

### [MEDIUM] loop_tool.rs:1660 — MoA aggregator prompt concatenates every proposal in full, unbounded and unfenced
**Category:** Quality / Efficiency
**Confidence:** High
**Description:** `build_synthesis_prompt` (loop_tool.rs:1660-1693) appends every successful proposal's **entire** `final_text` (`out.push_str(text)`, :1681) with no per-proposal or total cap; `record_batch_row` likewise clones the full text into `proposals` (:1536-1540). Child outputs are unbounded (`final_text` has no cap on the way out of `AgentRuntime`), so an N-wide fan-out of verbose children produces an aggregator task of roughly ΣN·|output|. Everything else on this path is carefully bounded — `list` rows preview at `LIST_RESULT_PREVIEW_CHARS`, the progress trail at `TRAIL_LINES`, the batch at a wall-clock share — making this the one unbounded accumulation in the pipeline. The realistic outcome is a `prompt_too_long` / provider error on the reduce, i.e. `moa_synthesis_failed` after the *entire* fan-out has already been paid for.
**Suggested fix:** cap each proposal (char-wise, UTF-8 safe — reuse `preview`-style `chars().take(n)`) with an explicit `…[truncated, M chars omitted]` marker, and cap the assembled prompt; state the truncation in the prompt text so the aggregator knows it is folding excerpts. Never truncate silently (§0 "no silent caps").

---

### [MEDIUM] loop_tool.rs:1666 — Sub-agent output is interpolated into the aggregator prompt with structural markdown delimiters and no fencing (prompt injection)
**Category:** Security
**Confidence:** High
**Description:** The aggregator prompt is built as instruction text + `## Goal` + `### Proposal {idx} (model: {m})` + raw proposal body + `## Your task\nReturn the single synthesized answer…` (loop_tool.rs:1666-1692). Proposal bodies are sub-agent outputs that routinely embed **external untrusted content** (web fetches, file reads, MCP/tool results). A proposal that emits its own `## Your task` / `### Proposal 0 (model: …)` heading forges the frame: it can re-open the instruction section after the real trailer, impersonate another proposer, or instruct the aggregator directly — and the aggregator's answer is what the parent model consumes as the batch's result. The repo already owns the countermeasure for exactly this shape (`content_sanitizer::split_external_fence` / the `<<<EXTERNAL_UNTRUSTED_CONTENT>` boundary, and the §1 rule "往 `<tag>` 里插用户/模型正文前先 escape"), and this call site uses neither.
**Suggested fix:** wrap each proposal body in the repo's external-content fence (or an XML tag with `xml_util::escape_xml`) and rewrite any interior fence/heading markers with the existing `tool_output/fence.rs::rewrite_interior` helper, so a proposal cannot terminate its own section. Keep the instruction trailer *before* the untrusted bodies, or restate it inside the trusted frame.

---

### [MEDIUM] loop_tool.rs:1310 / :1144 — A sub-agent queued on the concurrency semaphore is invisible to its own `timeout_secs`; only the sync-batch path has a queue-aware backstop
**Category:** Logic
**Confidence:** High
**Description:** `subagent_spawner::spawn` takes its permit (`sem.acquire_owned()`, spawner mod.rs:208-222) **before** arming `tokio::time::timeout(req.timeout_secs, …)` (mod.rs:658-668). The batch path knows this and compensates with `fanout_deadline` — its comment says so verbatim: "the permit wait inside the spawner happens BEFORE its `tokio::time::timeout`, so a queued child is invisible to every per-child deadline in the system" (loop_tool.rs:857-866). The **foreground single-run path** (loop_tool.rs:1297-1317) and the **MoA aggregator** (loop_tool.rs:1144-1147) got no such backstop: both `await` the runtime directly. With the default cap of 4 and four long-lived background children of the same session holding all permits (the session semaphore is deliberately shared across runs — types.rs:32-53), a `run` with `timeout_secs: 120` parks until a background child releases a permit; the only ceiling left is the 1_800_000 ms `subagent` tool budget, i.e. the parent turn can block ~30 minutes on a call that advertised 120 s. The schema tells the model `timeout_secs` is the "maximum seconds the sub-agent may run" (loop_tool.rs:120), which is then not true on the most common path.
**Suggested fix:** bound both awaits the way the batch does — `tokio::time::timeout_at(now + timeout_secs + slack, runtime.run(...))` — or (better, single source) move the queue wait inside the spawner's own clock so `Sub-agent timed out after Ns` covers queueing too, and drop the batch's separate backstop.

---

### [MEDIUM] loop_tool.rs:158 — `execute` is a ~1,190-line function in a 1,902-line file
**Category:** Quality
**Confidence:** High
**Description:** `async fn execute` runs from loop_tool.rs:158 to :1349 as one `match` arm chain with up to six levels of nesting, mixing seven distinct actions, the batch fan-out state machine, the deadline/grace/abort drain, and the MoA reduce. Both the file (1,902) and the function far exceed the repo's own 500-line split threshold (P2 / CODE_ORGANIZATION.md). The concrete cost is visible in this review: several arms `return` from inside the batch block, so invariants like "`incomplete` was snapshotted at the deadline" have to be re-established by reading ~200 lines of context, and the `default`-agent resolution below is duplicated three times inside this one function.
**Suggested fix:** extract per-action handlers (`fn handle_check_status`, `handle_wait`, `handle_cancel`, `handle_list`) and split the run path into `run_batch_sync` / `run_batch_background` / `run_single` / `run_moa_reduce` modules; `execute` becomes a dispatch table. This is mechanical (no behaviour change) and would let the existing source-level spawn guard scope itself to the fan-out module.

---

### [LOW] loop_tool.rs:940 — Sync-batch legs re-establish three of the four carried task-locals; the background path uses `CarriedAttribution` (four)
**Category:** Architecture
**Confidence:** High
**Description:** `spawn::spawn_background` carries attribution across the spawn boundary with `CarriedAttribution::capture()/reestablish()` (spawn.rs:194-217), which covers **four** task-locals: scope, project root, agent id, **and `CALLER_ROLE`**. The sync-batch legs in loop_tool.rs:940-953 hand-roll the nest with only the first three, and the file's own source guard encodes that same three-item list (`REQUIRED_TASK_LOCALS`, loop_tool.rs:1827) — so the guard cannot see the omission it was written to catch. `carried.rs:44-56` states the reason the fourth is in the carrier: an absent role is **fail-OPEN** (`role_is_operator(None) == true`), the opposite direction from the other three. Impact today is latent, not live: `run_agent_loop` does not re-establish `CALLER_ROLE` either (`with_request_scope`, run_loop/mod.rs:49-59, seeds scope + room author only), so both paths capture `None` in production and tool gating flows through `TurnContext.caller_role` instead. The defect is the duplicated ritual — exactly the failure mode `CarriedAttribution`'s doc says it exists to prevent ("Two copies of a three-line ritual is how the second variant loses it").
**Suggested fix:** replace loop_tool.rs:940-953 with `carried.reestablish(Box::pin(async move { … }))` using a `CarriedAttribution::capture()` taken at :878, delete `captured_scope`/`captured_agent`/`project_root` threading, and retarget the source guard at `CarriedAttribution::capture` / `reestablish` rather than a literal list of three combinators.

---

### [LOW] loop_tool.rs:788, :1085, :1245 — The `default`-agent fallback bypasses the spawnability predicate that every other resolution site applies
**Category:** Security (defense in depth)
**Confidence:** High
**Description:** All three sites that resolve an explicit `agent_type` go through `AgentRegistry::resolve_spawnable`, which filters `mode == SubAgent` — the filter added specifically because "`agent_type = "main"` resolved the builtin Primary def — a wildcard tool grant (`allowed_tools = ["*"]`) reachable from any sub-agent delegation" (registry.rs:140-166). The three *fallback* sites (batch row with no `agent_type`, aggregator, single run) instead call `lookup_with_overlay("default", …)` (loop_tool.rs:788, :1085, :1245), which applies no mode filter. Today the builtin `default` is `SubAgent` (registry.rs:407), the disk loader hard-codes `AgentMode::SubAgent` (loader.rs:145) and plugin defs are always SubAgent, so nothing currently exploits it — but `AgentRegistry::register` accepts any `AgentDef`, so a single future registration of a Primary-mode `default` re-opens the exact hole `resolve_spawnable` closed, on the path the model lands on when it omits `agent_type` (the most-taken path). registry.rs:142-145 states the rule this violates: the two faces of one verb "must share a predicate — branching only one of them is the same as not branching at all".
**Suggested fix:** `resolve_spawnable("default", project_root_ref)` at all three sites (it already consults the overlay via `resolve` → `lookup_with_overlay`), keeping the existing "No default agent registered" error arm for the miss.

---

### [LOW] recovery.rs:172 — `enumerate` is order-dependent while its sibling `classify` is explicitly order-independent
**Category:** Logic
**Confidence:** High
**Description:** `classify` documents and implements order-independence: "`Returned` beats `Spawned` regardless of order in the slice" (recovery.rs:91-107, returning early on `SubagentReturned`). `enumerate`'s doc claims the same shape — "a `SubagentReturned` **upgrades** the entry its `SubagentSpawned` created" (recovery.rs:136-138) — but the code is a blind last-write-wins `found.insert(request_id, upgrade)` (recovery.rs:172), so a `SubagentSpawned` record appearing after its `SubagentReturned` **downgrades** `Completed` back to `Interrupted`. `SessionService::get_events` (service.rs:41) carries no documented ordering guarantee in the trait, and the two functions read the same slice from the same call site pair. The failure is the one the module exists to prevent: `list` would label a finished child "interrupted … it never finished", inviting the model to re-run completed work.
**Suggested fix:** make the merge explicit rather than positional: `found.entry(id).and_modify(|cur| if matches!(upgrade, Recovered::Completed{..}) { *cur = upgrade.clone() }).or_insert(upgrade);` — i.e. `Completed` is absorbing, matching `classify`'s precedence in one shared rule.

---

## Cross-cutting observations

- **Field wiring is complete.** I enumerated all 29 `SubagentTool` fields (mod.rs:55-142) against `build_runtime` (spawn.rs:353-423) and the construction site (`run_loop/inner.rs:990-1195`): every inheritable field is both populated at construction and threaded into every child runtime. The three spawn paths (foreground, sync batch, background) all go through `build_runtime`, so there is no per-path drift. No finding.
- **`subagent_semaphore_for` has no double-init race** (types.rs:68-85): the `get`→`retain`→`insert` sequence is performed under one held `Mutex` guard, so two concurrent callers for one session key cannot each install a semaphore. The `with_parent_session_id` rebind (mod.rs:333-343) is likewise safe: it consumes `self` during construction, and the only cloner (`build_runtime`) does not run until `execute`, so no sibling can hold an `Arc` to the discarded `new()` semaphore. Both key angles checked and clean.
- **Cancellation coverage is consistent.** All four spawn/await sites derive through `cancel_for_child_with` (loop_tool.rs:895, :1126, :1310; spawn.rs:85) and each pairs it with a `CancelGuard` or an explicit `.cancel()`, so the bridge watcher cannot leak. The per-call harness token is a `run_cancel.child_token()` that act.rs never cancels on normal completion (act.rs:565, :905, :966), so background children correctly outlive a finished parent turn.
- **The timeout predicate is consistent across all three readers** — `runtime.rs:426`, `background_tracker.rs:98`, and the producer `subagent_spawner/mod.rs:668` all use the exact prefix `"Sub-agent timed out"`, and the cancel classifier uses byte equality on `"sub-agent failed: cancelled"`. Nothing to report.
- **No cross-session disclosure found on the by-id faces.** `progress_snapshot` (background_tracker.rs:1348) takes no scope argument, but every reachable caller is gated first: `CheckStatus` behind a scoped `running_meta`/`result_snapshot`, and the single-id `wait` timeout arm is unreachable for a foreign id because `wait_any` classifies out-of-scope ids as `NotFound` (background_tracker.rs:940-965). Worth keeping in mind if a new caller is added — the safe scoping is circumstantial, not enforced by the signature.
- **Sync fan-out seats surface as phantom `request_id`s.** Each sync-batch leg and the aggregator register a `RunningRegistration` under a fresh UUID (loop_tool.rs:904, :1132) into the process-global tracker, so a concurrent `list` from another surface in the same session shows running rows whose ids can be `check_status`'d ("running") and then vanish into "No background sub-agent found" once the leg settles. Deliberate for leader-cancel reachability, but the `list` directory has no way to mark them as non-pollable seats; a `kind: "fanout_seat"` field on the row would keep the directory honest.
- **`retryable` is `false` on every error return** in `execute`, including transient ones (`Failed to send message`, `Failed to read inbox`). Not obviously wrong given the harness's failure semantics, but it is a blanket, not a decision.
- **Argument-key drift guard is good practice worth keeping**: `ACCEPTED_ARG_KEYS` (types.rs:233) + the schema in loop_tool.rs are pinned together by a test; the two findings above are about what happens *inside* an accepted key, which that guard by construction cannot see.

## Files reviewed

| File | LOC | Notes |
|------|-----|-------|
| `src/agents/subagent_tool/mod.rs` | 390 | struct + 30 builders; full field/wiring enumeration done |
| `src/agents/subagent_tool/loop_tool.rs` | 1902 | `LoopTool::execute` pipeline, batch fan-out, MoA reduce |
| `src/agents/subagent_tool/parse.rs` | 345 | JSON → `SubagentAction` |
| `src/agents/subagent_tool/spawn.rs` | 524 | `cancel_for_child*`, `spawn_background`, `build_runtime` |
| `src/agents/subagent_tool/recovery.rs` | 643 | durable recovery (event log + sidecar) |
| `src/agents/subagent_tool/types.rs` | 451 | args types, concurrency primitives, timeout constants |

**Read for cross-file verification (not in scope, not reviewed for defects):** `src/agents/subagent_spawner/mod.rs`, `src/agents/background_tracker.rs`, `src/agents/background_persistence.rs`, `src/agents/registry.rs`, `src/agents/types.rs`, `src/agents/runtime.rs`, `src/scope/carried.rs`, `src/harness/agent/act.rs`, `src/gateway/execution_engine/run_loop/{mod,inner}.rs`, `src/gateway/execution_engine/execute.rs`, `src/tools/turn_context.rs`, `src/tools/budget.rs`.
