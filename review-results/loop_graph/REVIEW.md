# Loop Graph Module Review (2026-08-22)

**Module:** `src/loop_graph/` (9 files, ~6,409 LOC)
**Reviewer:** rust-doctor subagent (review-batch)
**Methodology:** Static code review + graphify cross-reference (no cargo run, no source edits)

## Summary

- **Total findings:** 22
- **critical:** 0
- **high:** 3
- **medium:** 7
- **low:** 12

The module is unusually well-instrumented: most historical bugs (capture/rowid race, body-overwrite on re-upsert, gc sweeping `owns_reference`, single-path governance walk, ownership-non-exhaustive) are pinned by named regression tests with issue-commit references. Findings below are mostly subtle correctness, API surface, or perf corners rather than known foot-guns. None of the high-severity items are unrecoverable — the worst is a SQLite write-lock window during `gc` and two untracked spawned tasks that share the daemon's lifetime.

---

## Findings

### [high] `gc()` holds the store-wide write lock for the entire transaction (blocks every concurrent writer for the full pass)

- **Location:** `src/loop_graph/store.rs:418-485`
- **Category:** concurrency / perf
- **Description:** `gc()` takes `self.lock()` at line 433, then inside that single guard opens an SQLite transaction (`conn.transaction()` at line 466) and executes one DELETE per dangling edge. The comment at line 461 calls this "Atomic delete" and is right about atomicity, but the entire duration of the transaction — for a graph with N dangling edges, N round-trips to SQLite — is spent under the store mutex that every other `upsert_node` / `upsert_edge` / `delete_*` / `lint` call must acquire.
- **Impact:** During `enable_audit`'s post-pair sweep or a large post-incident `gc`, all graph writes (and reads, since the same mutex gates both) stall. With a few thousand edges and a slow disk, this is tens of seconds of write-unavailability on the held-out layer. The doctest for `gc_is_atomic_across_multiple_dangling_edges` (`store.rs:1676-1714`) deliberately forces a failure path; the success-path latency is not tested or bounded.
- **Suggested fix:** (a) Move the report-build phase under its own read snapshot, drop the lock, then `BEGIN IMMEDIATE` for the delete-only transaction; (b) accept that the lock must be held for the transaction but batch DELETEs into a single `DELETE FROM graph_edges WHERE ...` statement with a constructed IN-tuple so the round-trip count is constant; (c) document the expected upper bound (test with N=10_000 dangling edges, assert elapsed < 1s).
- **Related:** `store.rs::lint` already follows the drop-lock-then-pure-compute pattern (`store.rs:563`); `gc` cannot easily do the same without a second write connection, but option (b) above closes most of the latency without that.

### [high] `spawn_event_persister` JoinHandle is discarded — the audit-log writer has no shutdown path or supervision

- **Location:** `src/loop_graph/mod.rs:95-115` (def); called at `src/executor/builtin_registry/builder/constructor/mod.rs:528`
- **Category:** concurrency
- **Description:** `spawn_event_persister` returns a `tokio::task::JoinHandle<()>`, but the single production caller (`constructor/mod.rs:528`) discards the return value. The doc at `mod.rs:88-92` explicitly says: "Lifecycle: deliberately NOT tracked anywhere. The daemon has no global shutdown mechanism for boot-spawned tasks, so this one shares the daemon's lifetime and exits with the process; the synchronous SQLite append per event is microseconds and never crosses an `.await`, so the task holds no lock across yield points."
- **Impact:** If the persister task panics (e.g. `unwrap` in a future change, or SQLite I/O error not caught by the `if let Err` arm), no `JoinHandle` exists to observe it. The audit log silently stops appending. `subscriber_count()` on the bus would not go to zero either, since the audit persister is the only consumer and the `Sender` stays alive even after a dead `Receiver`. There is also no graceful shutdown: on SIGTERM, any in-flight `append_event` may collide with a daemon closing the SQLite connection mid-write. The `persister_exits_when_the_channel_closes` test (`mod.rs:175-191`) only proves the loop exits cleanly when every sender drops — it does NOT prove the task is supervised in production.
- **Suggested fix:** Either (a) store the `JoinHandle` in a `OnceCell<JoinHandle<()>>` (or a small "supervisor" struct in `loop_graph` mod), so a future admin RPC or diagnostic can `.abort()` and observe panic status; (b) at minimum, log on panics by wrapping the body in `tokio::spawn(async move { ... }.in_current_span())` and registering a panic hook that warns; (c) document the assumption that "the daemon has no global shutdown mechanism" as a deliberate tech-debt line and link it to the missing-supervisor issue.
- **Related:** `topology_event_bus` consumer `recent_events` (`inspector.rs:177-185`) assumes the events table is being populated; a stuck persister goes undetected.

### [high] `gc` builds the `GcReport` before the transaction, then returns `Err` on failure — comment promises a structured report on failure that the signature cannot deliver

- **Location:** `src/loop_graph/store.rs:449-465` (report build); signature at `:418`
- **Category:** error-handling / doc-vs-code drift
- **Description:** The doc comment at `store.rs:449-451` says: *"Build the report BEFORE the transaction so a failed delete still returns a structured 'what would have been removed' instead of either an empty report or a half-truth."* But the function returns `Result<GcReport>`, and on transaction failure the `Err` is propagated at `:484` (`tx.commit()?`); the populated `report` is dropped. The caller cannot distinguish "0 dangling edges" from "100 dangling edges but all deletes failed".
- **Impact:** The failure path that the comment is written to defend against — a half-completed delete — is invisible to the caller. The audit template (`templates.rs::AUDIT_TEMPLATE`) treats `GcCompleted { retained_acl, removed }` as the evidence the next audit uses to check that the graph actually converged (`events.rs:90-92`). If `gc` failed and reported `Err`, the audit only sees the absence of a `GcCompleted` event (because the bus is published AFTER `gc` returns in `loop_graph_manage.rs:722-726`). The audit loop will retry next tick without any clue that the SAME delete set will keep failing.
- **Suggested fix:** Either (a) update the doc to honestly say "report is only returned on success"; (b) return `Result<GcReport, (GcReport, Error)>` so the caller sees both what *would have been* removed and the actual error; (c) emit a `TopologyEvent::GcCompleted { removed: 0, retained_acl }` even on failure (after rollback) so the audit loop has a uniform observation surface. Option (b) is the smallest API change.
- **Related:** `gc_never_collects_the_objective_acl` (`store.rs:1052-1080`) exercises only the success path; there is no test for failure-with-report.

### [medium] `upsert_node` overwrites `kind` unconditionally on conflict — a node can silently flip optimization classification

- **Location:** `src/loop_graph/store.rs:128-150` (SQL `ON CONFLICT ... DO UPDATE SET kind = excluded.kind, ...`)
- **Category:** correctness
- **Description:** The `INSERT ... ON CONFLICT DO UPDATE` clause sets `kind = excluded.kind` (overwrite) but uses `COALESCE` for `body` and `cadence` (preserve-on-omit). The asymmetry is deliberate for body/cadence (the doc explains it at length, `store.rs:107-126`), but `kind` is left on the overwrite path. A re-registration call with the same `(agent_id, id)` but a different `kind` therefore silently flips `is_optimization_loop()` for that node.
- **Impact:** An optimizer can be re-registered as a `Daemon` → `Frozen` and stop showing up in `lint_naked_loops`. There is no test pinning this. The `provenance_is_write_once` test (`store.rs:1589-1624`) covers `origin`, and the body-overwrite fix is covered by `label_only_reregistration_keeps_the_human_root_reference` (`store.rs:1532-1587`), but `kind` is not.
- **Suggested fix:** Either (a) reject any upsert where the existing kind ≠ the new kind with an error (`Err("loop_graph invariant: cannot change node kind after registration — delete and recreate")`); (b) extend the `kind` column write to `kind = CASE WHEN kind = excluded.kind THEN excluded.kind ELSE kind END` and add a test. Option (a) matches the discipline of `owns_reference`/`coverage_source_rejection` rules.
- **Related:** `node_kind` drives `is_optimization_loop()`, which drives `lint_naked_loops` and `coverage_source_rejection`. A silent kind flip undermines both.

### [medium] `render_session_topology` reads `list_nodes` + `list_edges` for the entire agent on every governed session turn

- **Location:** `src/loop_graph/service.rs:592-598`
- **Category:** perf
- **Description:** `render_session_topology_inner` (`service.rs:605-710`) calls `store.list_nodes(DEFAULT_AGENT)` and `store.list_edges(DEFAULT_AGENT)` — every node, every edge in the graph — to render one session's prompt block. This runs every governed session turn. The doc at `service.rs:525-534` already establishes that this is the prompt-cache-bound Dynamic layer, and bounds row size (`MAX_ROOT_BODY_CHARS = 600`), but bounds neither row count nor read cost.
- **Impact:** At N=1000 nodes / E=2000 edges, every turn of a governed session pays O(N+E) SQLite reads + Vec allocations + JSON-free serialisations. This is the very "per-turn cost" the module doc (`mod.rs:6-8`) declares it adds when the graph is populated. The prompt cache does not save these reads because the cache key is the rendered bytes; the render is the work.
- **Suggested fix:** (a) Cache the full node/edge read on a `tokio::sync::RwLock<Arc<(Vec<GraphNode>, Vec<GraphEdge>)>>` invalidated by the event bus subscription (the persister is already there; add a second subscriber); (b) push the read through `LoopGraphInspector::subgraph_for` which already builds the per-node view. Option (a) is consistent with the existing bus + persister pattern.
- **Related:** `service.rs:535-587` documents this concern as the bound issue; `render_is_bounded_against_oversized_graph_rows` (`service.rs:917-960`) bounds size but not row count (acknowledged in its own comment).

### [medium] `subgraph_for` accepts an unused `_nodes: &[GraphNode]` parameter and re-derives `by_id` from it

- **Location:** `src/loop_graph/inspector.rs:187-267` (impl); `walk_chain` signature at `:443-447`
- **Category:** api / dead code
- **Description:** `walk_chain` takes `_nodes: &[GraphNode]` (the leading underscore makes the dead-arg signal explicit), and the call sites at `:223-238` pass the same `&nodes` slice that the function already had to build `by_id` from inside `subgraph_for`. The slice is never read inside `walk_chain`. Two callers (`subgraph_for` up/down walks) and the helper itself are coupled to a parameter that does nothing.
- **Impact:** Future readers of `walk_chain` must trace back to confirm `_nodes` is genuinely unused (vs. dead-but-not-prefixed). The `#[cfg(test)]` test for chains-longer-than-MAX (`inspector.rs:847-881`) passes through the same path and benefits no functional test from the extra arg.
- **Suggested fix:** Drop the parameter from `walk_chain`'s signature and from both call sites in `subgraph_for`. Saves 1 Vec borrow per call and one source of confusion. No behavior change.
- **Related:** This is form-1 territory (visible signature with one field that doesn't earn its slot). The previous severed-wire review (`review-results/severed-wire-2026-08-17/loop_graph/REPORT.md`, sw-lg-01/02/03) flagged `subgraph_for` itself as orphaned (zero production callers); this finding reinforces that.

### [medium] `TopologySummary.governance_chain_anchored` is `false` for an empty graph — contradictory to "anchored" semantics

- **Location:** `src/loop_graph/inspector.rs:394-437` (impl); the predicate at `:434`
- **Category:** api / correctness
- **Description:** `governance_chain_anchored = unanchored_chain_count == 0 && !nodes.is_empty()`. So an empty graph (the legitimate pre-declaration state) returns `anchored = false`. The field name promises the answer to "are the chains anchored", and the obvious answer for "no chains exist" is "vacuously anchored". The `loop_graph status` test (`loop_graph_manage.rs:1967-1969`) pins `status_counts_agree_with_inspector_summary`, so flipping this predicate would touch that test.
- **Impact:** A Panel showing `governance_chain_anchored: false` for an empty graph would mislead the operator into running `enable_audit` thinking something is wrong, when the real state is "nothing declared, nothing broken". This contradicts the doctor's separate `No topology declared` (`diagnostics/checks/loop_graph.rs:107-117`) finding for exactly the same condition.
- **Suggested fix:** Drop the `&& !nodes.is_empty()` clause, or replace `governance_chain_anchored: bool` with an `Option`/`enum` distinguishing "empty graph" from "graph exists and unanchored".
- **Related:** Doctor's "No topology declared" finding (`diagnostics/checks/loop_graph.rs:107`) and inspector's `governance_chain_anchored` are about the same state and should agree on the answer.

### [medium] `TopologySummary` count relies on a substring match against the lint string `"治理链未锚定"`

- **Location:** `src/loop_graph/inspector.rs:419-421`
- **Category:** api / fragility
- **Description:** `unanchored_chain_count = findings.iter().filter(|f| f.contains("治理链未锚定")).count();`. The lint message format at `store.rs:823-830` includes the substring `治理链未锚定`, but the predicate is a literal string fragment — a future copy edit in either location silently breaks the count.
- **Impact:** A small wording change (e.g. "未锚定" → "无根") makes the dashboard number wrong without any compile error. Test `summary_counts_match_lint_findings` (`inspector.rs:814-845`) does not exercise a graph with an unanchored chain, so it would not catch the regression.
- **Suggested fix:** Lift the lint finding shape to a typed `enum LintFinding { NakedLoop, DanglingEdge, UnanchoredChain, FastOwnsSlow, ForgedCoverage }` and have `lint()` return `Vec<LintFinding>` (string-shaped at the render seam, not at the count seam). The existing tests on string content (`"裸奔优化环"`, `"悬空边"`, etc.) would just check `matches!(f, LintFinding::NakedLoop(_))`.
- **Related:** The same brittleness affects any future dashboard metric that counts lint findings (`loop_graph status` aggregates, governance metrics); fixing this once at the source benefits all consumers.

### [medium] `render_session_topology` reads from `list_nodes`/`list_edges` (fail-soft path) instead of `node_ids_present`/`owns_reference_sources` (raw-column path)

- **Location:** `src/loop_graph/service.rs:592-598`
- **Category:** correctness (fail-soft vs. fail-loud inconsistency)
- **Description:** `render_session_topology_inner` calls `store.list_edges(DEFAULT_AGENT)` and `store.list_nodes(DEFAULT_AGENT)`. Both are fail-soft (they drop rows with unparseable enum text, `store.rs:869-887` and `:890-902`). The module's other readers — `owns_reference_sources` for the objective ACL (`store.rs:330-339`) and `node_ids_with_body` for `enable_audit`'s idempotency (`store.rs:381-416`) — were deliberately migrated to raw-column reads precisely to avoid the fail-soft drop, with detailed comments (`store.rs:296-329`, `store.rs:386-417`) explaining why.
- **Impact:** A graph written by a newer build (with a `NodeKind` / `EdgeKind` enum variant added in `#[non_exhaustive]`, `types.rs:38-43` / `:110-117`) then read by this build will silently miss the new-variant rows in `render_session_topology`. A governed session will see no watchers, no `owns_reference` line, no root body — even though every row is on disk. This is the same failure mode the ACL migration closed. The render path was missed because prompt-cache determinism is the dominant constraint there and the migration seemed to threaten it; it does not (the new variant's rows are simply absent from the render, identical bytes either way).
- **Suggested fix:** Switch the `list_*` calls in `render_session_topology_inner` to a typed raw-column read for the kinds actually rendered (nodes by id+kind+label+body+cadence; edges by from+to+kind+note). The byte-determinism property is preserved by the existing `to_id`/`from_id`/`kind` filter at the render seam.
- **Related:** This is exactly the pattern `store.rs::owns_reference_sources` already implemented; the render path needs the same discipline.

### [medium] `LoopGraphStore::owns_reference_sources` returns only the first source per goal — multi-owner ambiguity is hidden from the ACL

- **Location:** `src/loop_graph/store.rs:330-339`; consumer at `service.rs:469-475`
- **Category:** api
- **Description:** `owns_reference_sources` returns `Vec<String>` ordered alphabetically by `from_id`, and the single caller `governing_owner_in` takes `.next()` (`service.rs:473-474`). If a `goal:sess-1` is governed by both `root:aleph` and `cron:steward` (a deliberate multi-steward design), the ACL sees whichever sorts first.
- **Impact:** The ACL is a write-protection gate. A multi-owner graph silently uses one of the owners' decisions. The goal-rewrite guard at `builtin_tools/goal.rs:829` and `:1041` then asks "is the agent's owner `cron:steward`?" — if `root:aleph` sorted first, the guard checks `root:aleph`'s status, which may not exist, and rejects the write. The semantics of multi-`owns_reference` are undocumented at the schema level (`types.rs:108-130` describes only the verb, not the cardinality).
- **Suggested fix:** Either (a) document at `types.rs:EdgeKind::OwnsReference` that the verb is single-owner ("two owners = bug; raise as lint"); (b) add `governing_owners(...) -> Vec<String>` to the public API and let `governing_owner_in` return all of them with the ACL's calling code deciding the merge semantics.
- **Related:** `lint_governance_chain` (`store.rs:796-836`) already correctly walks all incoming `owns_reference` edges (the round-11 fix); the ACL reader does not.

### [medium] `notify_node_settled` async-holds `cron.lock().await` for the full `run_job().await` — a slow/dead watcher stalls every other watcher poke

- **Location:** `src/loop_graph/service.rs:390-400`
- **Category:** concurrency
- **Description:** `poke_one_watcher` acquires `cron.lock().await` at `:390` and holds it across `service.run_job(job_id).await`. Multiple pokes for different watcher jobs are serialised via `poke_all_watchers`'s `join_all` (`service.rs:340-361`) because they share the same `SharedCronService` mutex. If one watcher's `run_job` is slow (e.g. a cron job that runs a heavy prompt), every other watcher's poke waits behind it.
- **Impact:** A settle against a goal with N watchers pays serialised `run_job` latency, not max. The doc comment at `service.rs:343-349` justifies the shared mutex on the grounds that "they hold separate cron mutex slots" — but they actually share the SAME `SharedCronService` mutex, which contradicts the comment.
- **Suggested fix:** Either (a) clone the `SharedCronService` per poke (already done — `cron.clone()`) and confirm whether `SharedCronService` is `Arc<Mutex<CronService>>` (single mutex) or `Arc<Scheduler>` (different shape); (b) if it IS a single mutex, comment-fix the doc; (c) add a timeout on `run_job` so a single dead watcher doesn't stall a settle indefinitely.
- **Related:** `service.rs::poke_all_watchers` comment at `:343-349` should match reality.

### [low] `TopologyEventBus::publish` silently drops the broadcast send error with `let _ = ...`

- **Location:** `src/loop_graph/events.rs:133-143`
- **Category:** error-handling
- **Description:** `publish` calls `let _ = self.inner.send(ev);`. The doc justifies this as "publishing into a quiet bus is not an error". True for `SendError(value)` (no receivers), but for any future `SendError` variant or a panic in `send`'s internals, the event vanishes with no trace. The audit persister is the only current consumer, but a slow consumer that lags past 256 events will trigger `Lagged` on the receiver side (`mod.rs:108-114`) — the publish itself does not see this.
- **Impact:** The `Lagged` log line is the only signal a publisher has that events were dropped. A burst of 257+ topology mutations (e.g. a bulk `enable_audit` fan-out) silently drops events past #256. Acceptable today but the bus capacity is a fixed constant (`events.rs:25`); there is no metric to detect "we dropped N events".
- **Suggested fix:** Add a counter (atomic) `dropped_at_publish: AtomicU64` incremented inside the `Lagged` arm of the receiver's `match`, exposed as `pub fn dropped_events(&self) -> u64`. Even if unused today, it preserves the audit's "we must never silently lose events" promise.
- **Related:** `mod.rs:108-114` already logs `Lagged(n)`; turning that into a structured counter costs one `AtomicU64`.

### [low] `BUS_CAPACITY = 256` is a bare constant with no knob

- **Location:** `src/loop_graph/events.rs:47`
- **Category:** api
- **Description:** 256 is documented as "burst writes (an `enable_audit` fan-out is one event per wired node), with ample headroom". For a graph with N>256 optimization loops, `enable_audit` (which fans one `EdgeUpserted` per wired node per the `wire_audit_edges` action) drops events before the persister sees them.
- **Impact:** Real-world graphs above 256 nodes lose audit rows during the fan-out. No operator knob to raise the ceiling.
- **Suggested fix:** Make `BUS_CAPACITY` a `pub const` plus a constructor parameter on `TopologyEventBus::with_capacity(usize)`, used by the boot sequence in `constructor/mod.rs:506`.
- **Related:** `recent_events` is the only window into the audit log; missing events are silent.

### [low] `SnapshotStore::capture` JSON-serialises the full graph on every capture — O(N+E) blocking serialisation

- **Location:** `src/loop_graph/snapshot.rs:184-226`
- **Category:** perf
- **Description:** `capture` calls `store.list_nodes` + `store.list_edges` + `serde_json::to_string(&nodes)` + `serde_json::to_string(&edges)` (4 ops, all under the graph store mutex windows) every time it is invoked. With 1000 nodes + 2000 edges, this is multi-millisecond blocking work, on the same mutex as the writer path.
- **Impact:** Concurrent writers are blocked during the read, and the capture itself is unbounded in cost by row count. The cap of 200-char labels and 200,000-byte export (`export.rs:457-484`) cover the export path but not snapshot capture.
- **Suggested fix:** Document the expected upper bound (e.g. ≤500 nodes for healthy capture <100ms). For larger graphs, switch to incremental snapshot (delta from previous) or async snapshot capture that yields to the runtime.
- **Related:** `snapshot_inherits_store_fail_soft_view` test (`snapshot.rs:789-805`) exercises capture but not the size limit.

### [low] `spawn_event_persister` does not log the Start/Stop of the task — silent lifecycle

- **Location:** `src/loop_graph/mod.rs:95-115`
- **Category:** observability
- **Description:** The body has no `tracing::info!(...)` on entry, no `tracing::info!(...)` on exit-via-closed, no metric for events processed. The only log lines are `Lagged(n)` and `failed to append event`.
- **Impact:** Operators cannot tell whether the persister is alive without observing the audit-log DB or the bus subscriber count. The `subscriber_count()` exists precisely to expose this signal (`events.rs:140-143`) but is dead.
- **Suggested fix:** At minimum, `tracing::debug!(...)` at task start and on `Closed` exit. Optionally a `tracing::info!(processed = N)` at every Nth event (e.g. every 1000).
- **Related:** `subscriber_count` is orphaned (`severed-wire-2026-08-17` sw-lg-05); wiring it to the supervisor above would close both.

### [low] `gc_never_collects_the_objective_acl` discipline is enforced in the loop, not in SQL — a future refactor of the loop can silently break the invariant

- **Location:** `src/loop_graph/store.rs:449-485` (the `if e.kind == EdgeKind::OwnsReference { retained_acl.push(...); continue; }` branch at `:459-463`)
- **Category:** api / defense-in-depth
- **Description:** The "never collect `owns_reference`" rule is enforced in the Rust loop by an early `continue`. The test `gc_never_collects_the_objective_acl` (`store.rs:1052-1080`) pins it, but nothing at the schema or transaction layer prevents a future "smart gc" (e.g. one that honours a per-edge `keep` flag) from regressing.
- **Impact:** The doc comment at `store.rs:402-416` spends 14 lines explaining why `owns_reference` survives — exactly the kind of doc-only invariant that drifts. A future contributor who refactors the loop without reading the comment can land a regression.
- **Suggested fix:** Move the rule to a constant `PROTECTED_KINDS: &[EdgeKind] = &[EdgeKind::OwnsReference]` and add an assertion in the `to_delete` filter; better, push the filter into a CHECK constraint or a pre-delete trigger that rejects `DELETE FROM graph_edges WHERE kind='owns_reference' AND ...`.
- **Related:** The same discipline gap exists for the "never delete into a present-but-unparseable node" rule (`store.rs:402-406`), enforced in `gc` by the `node_ids_present` lookup.

### [low] `GraphNode`/`GraphEdge` derive `PartialEq` but not `Hash` or `Eq`

- **Location:** `src/loop_graph/types.rs:192-208` (GraphNode), `:250-285` (GraphEdge)
- **Category:** api
- **Description:** Both structs derive `PartialEq, Eq` but not `Hash`. Consumers (`inspector.rs:223`, `:258`) use them in `HashMap<&str, &GraphNode>` keyed by id-string, never the struct itself. If a future consumer keys a HashMap by GraphNode, the compiler will reject the implicit `Hash` requirement.
- **Impact:** Low — current callers all key by string. The risk is a future caller hand-rolling a `HashMap<GraphNode, _>` and hitting an obscure compile error rather than the obvious "add `Hash`" comment.
- **Suggested fix:** Add `Hash` to the derive list, since both structs are simple value types.
- **Related:** None directly; just an API hygiene item.

### [low] `walk_chain` increments `steps` after the bound check, off-by-one with the test expectation

- **Location:** `src/loop_graph/inspector.rs:443-489`
- **Category:** correctness (test pinning)
- **Description:** `steps` is checked `if steps >= MAX_TRAVERSAL_STEPS` *before* incrementing, so the last permitted pop is iteration 1024 with `steps` going 1023 → 1024. The chain test (`inspector.rs:847-881`) builds N = MAX_TRAVERSAL_STEPS + 5 nodes and asserts the walk errors. With the current logic, iteration 1025 fires the error — works by accident: a chain of exactly MAX_TRAVERSAL_STEPS=1024 nodes walks 1024 pops and errors on the 1025th (after popping, but the start node never re-enters the frontier if it has no upstream edges).
- **Impact:** The test passes, but the bound semantics are not formally documented. A reader who tries to construct a "exactly N-step chain that succeeds" finds no comment telling them where the bound sits.
- **Suggested fix:** Add a doc comment at `MAX_TRAVERSAL_STEPS` (`:46`) explaining the inclusive/exclusive bound (currently: "a chain of N-1 edges from start is the maximum that succeeds; chain of N edges errors").
- **Related:** The `chains_longer_than_max_steps_are_rejected_not_runaway` test pins behavior; the doc pin is missing.

### [low] `LoopGraphInspector::recent_events` only exposes `limit`, no `before_id` paging

- **Location:** `src/loop_graph/inspector.rs:177-185`
- **Category:** api
- **Description:** `recent_events(limit)` hardcodes `None` for `before_id`, while `SnapshotStore::list_events(limit, before_id)` supports paging. The `loop_graph events` tool action (`loop_graph_manage.rs`) needs paging for the audit log to be useful past the first page.
- **Impact:** UI-level pagination beyond the first page requires going around the inspector. The doctor / status path is unaffected (those read counts, not rows).
- **Suggested fix:** Either (a) add `recent_events_before(limit: usize, before_id: Option<i64>)`; (b) extend the signature to take both. The current 1-arg form remains.
- **Related:** `SnapshotStore::list_events` already supports it (`snapshot.rs:340-366`).

### [low] `tokio::sync::broadcast` Lagged handling at the persister logs `dropped = n` but `n` includes events older than the receiver's `recv_from`

- **Location:** `src/loop_graph/mod.rs:108-114`
- **Category:** observability
- **Description:** `RecvError::Lagged(n)` reports the gap the receiver observed. The persister logs `tracing::warn!(dropped = n, ...)`, which is fine, but `n` is the receiver-local miss count — for a slow persister that lags once then catches up, the audit log will show a gap of N events between rows. There is no metadata (which events, what time range) to fill in the gap.
- **Impact:** Low — the audit log is advisory, not authoritative. Operators chasing a missing event cannot use the lag log to know what to look for.
- **Suggested fix:** Document the gap semantics at the log line; consider logging the timestamp at which the gap occurred (one extra atomic on the persister's `Instant::now()`).
- **Related:** `BUS_CAPACITY` finding above would compound with lag.

### [low] `service::one_line` doesn't strip `\r` from the input — only maps the char to space

- **Location:** `src/loop_graph/service.rs:209-220`
- **Category:** correctness
- **Description:** `one_line` maps `\r` (and `\u{2028}`, `\u{2029}`) to a literal space `' '`. `indented_body` (`:236-258`) does call `.trim_end_matches('\r')` per line. So a node label with a `\r\n` becomes `<label_with_trailing_space>...` — a small cosmetic issue. The render still escapes the rest correctly.
- **Impact:** Cosmetic; no security implication (`<` and `>` are escaped at the XML seam, the line seam only sees what `one_line` returns).
- **Suggested fix:** Make `one_line` also strip `\r` rather than map it to space, for consistency with `indented_body`.
- **Related:** `service.rs::indented_body` is the correct pattern.

### [low] Doc reference `Wiring is done by \`spawn_event_persister\`` is imprecise about the full boot sequence

- **Location:** `src/loop_graph/events.rs:112`
- **Category:** api / doc drift
- **Description:** The doc reads *"Wiring is done by `spawn_event_persister`"* — true in spirit (that is what connects the bus to the audit log), but the bus itself is installed by `loop_graph::init_event_bus` at `constructor/mod.rs:507`. A reader tracing the boot sequence for `TopologyEventBus` may look for the install at `mod.rs::init_event_bus` and find no doc pointer from this file. The previous severed-wire review (sw-lg-08) flagged an earlier, fully-stale `install_into_store` reference at the same location; the current wording is better but still omits the install half.
- **Impact:** Mild doc incompleteness; no runtime effect.
- **Suggested fix:** Adjust the wording to "Wiring is done at boot by `loop_graph::init_event_bus` (install) + `spawn_event_persister` (subscribe + append), `builtin_registry/builder/constructor/mod.rs`".
- **Related:** Previous review sw-lg-08 (CUT, resolved at the install_into_store half; this finding closes the remaining gap).

---

## Cross-Module Notes

- **R7 wiring is honest**: `service.rs::notify_*_settled` (3 entry points), `governing_owner`, `render_session_topology`, and `LoopGraphStore::{open,open_readonly,lint,gc,upsert_*,node_ids_with_body}` are all reached from production (`builtin_tools/goal.rs`, `gateway/execution_engine/goal_continuation.rs`, `builtin_tools/team/disband.rs`, `gateway/handlers/teams/crud.rs`, `teams/dispatcher/schedule/settle.rs`, `orchestrator/harness_bridge/prompt_build.rs`, `builtin_tools/loop_graph_manage.rs`, `diagnostics/checks/loop_graph.rs`). The previous severed-wire review's "10 low + 1 medium" findings (sw-lg-01 through sw-lg-10) are still accurate — `subgraph_for`, `loops_with_coverage`, `summary`, `TopologySummary`, `subscriber_count`, `delete_snapshot`, `AUDITOR_LACKS_CRON_MANAGE`, the stale `install_into_store` doc, the `mod.rs` re-export surface, and the `export.rs` doc-claim drift all persist.

- **Architectural boundary (R1/R7/R9)**: `loop_graph` sits in `core/` and touches no platform API. It is consumed only by `core/` callers and `builtin_tools/` (within `core/`), not by `interfaces/`/`shared/`/`desktop/`. The previous review's "no cross-crate consumers" finding still holds. Good R1 compliance.

- **Duplication with `shared/protocol`/`shared/ui_logic`**: none. `loop_graph` defines its own `NodeKind` / `EdgeKind` / `Origin` enums (`types.rs`), its own `TopologyEvent` discriminator (`events.rs`), its own `Snapshot`/`TopologyDiff` shapes (`snapshot.rs`). There is a low-cost risk that future protocol changes between `core` and `interfaces/webchat` will need a mirror, but no overlap today.

- **Failure-soft vs. fail-loud inconsistency** (cross-cutting): the codebase has two reading patterns — `list_nodes`/`list_edges` (fail-soft: drop unparseable rows) vs. `node_ids_present`/`owns_reference_sources`/`node_ids_with_body` (raw-column: ask the DB, not the parsed row). The migration is documented for the ACL (`store.rs:296-329`) and the `enable_audit` idempotency check (`store.rs:386-417`), but `render_session_topology` (medium finding above) was missed. This is the same shape of bug as the `governing_owner` fail-open on `SQLITE_BUSY` that the ACL migration closed — only the trigger condition differs.

- **`events.rs` doc honesty**: the module-level doc at `events.rs:5-32` is unusually careful about admitting which doc claims were wrong (`"an earlier version of this comment claimed they were its consumers, which was false"`, `"the earlier claim that `governance_metrics` did was aspirational"`). Worth carrying that discipline into `export.rs` (`export.rs:8-10` still claims Panel + governance_metrics consumers) and `inspector.rs` (which claims `summary()` will be the future home of status aggregates — true, but not wired).

- **`severed-wire-2026-08-17/loop_graph/REPORT.md` had no R7 violations.** Every `pub` symbol traced to either a production call site, a documented test-only consumer, or a documented future consumer. This review surfaced no new R7 violations.

---

## Out of Scope / Skipped

The following items are intentionally not flagged above because they are either pinned by tests, already documented, or outside the static-only review's reach:

- **`governance_chain_anchored` predicate literal-string match in `summary()`** is documented as a known fragility (medium finding); a full typed-lint migration is a separate PR and is the right fix but out of scope for this static review.

- **End-to-end performance of `enable_audit`'s fan-out** (one `EdgeUpserted` per wired node) under a 256-cap bus: the test `events_action_reads_the_persisted_audit_log` (`loop_graph_manage.rs` test mod) does not exercise this; only a load test would confirm whether `BUS_CAPACITY = 256` is sufficient.

- **`gc()` behavior under concurrent writers other than `upsert_node`/`upsert_edge`**: not tested for mixed workloads (e.g. `gc` concurrent with `append_event` on the snapshot store). The two stores have separate mutexes, so deadlock is impossible, but starvation is not characterised.

- **The `release_settle_notify` integration in `goal/store.rs:522`** is referenced from the loop_graph service comments as the caller contract; its interaction with the debounce stamp's `InFlight` window is tested at the unit level (`debounce_*` tests) but not at the goal-store CAS integration level.

- **The `notify_team_settled` and `notify_workflow_settled` paths** have unit coverage through `notify_node_settled` only; the team-disband and workflow-settle sweep callers (`team/disband.rs:100`, `gateway/handlers/teams/crud.rs:134`, `teams/dispatcher/schedule/settle.rs:371`) are not exercised end-to-end in tests under `src/loop_graph/`. A future integration test should cover both.

- **`LoopGraphInspector::summary` against a graph with `unanchored_chain_count > 0`**: test `summary_counts_match_lint_findings` (`inspector.rs:814-845`) only exercises naked-loop counts. The unanchored count is unreachable from the test surface, which is why the string-match fragility went unnoticed.

- **Whether `service::render_session_topology` should also bind to the event bus** (invalidate cached bytes on topology change) — this is a perf/design call and out of scope for static review.

- **Cross-platform behavior**: the module uses `tokio::sync::broadcast` (no platform-API touch) and `rusqlite` (already used elsewhere in core). No R1 risk.