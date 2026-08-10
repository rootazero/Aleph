# Review Report — Batch 2 (Background tracking + trace infrastructure)

**Scope:**
- `src/agents/background_tracker.rs` (2869 LOC)
- `src/agents/background_persistence.rs` (1068 LOC)
- `src/agents/forwarding_trace_sink.rs` (310 LOC)
- `src/agents/subagent_tree_events.rs` (38 LOC)

Cross-file verification also read (not in scope for findings unless the seam breaks):
`src/agents/subagent_tool/spawn.rs`, `src/agents/subagent_tool/loop_tool.rs`,
`src/agents/subagent_tool/recovery.rs`, `src/agents/subagent_tool/types.rs`,
`src/gateway/subagent_announce.rs`, `src/builtin_tools/process_journal.rs` (the twin
sidecar), `src/utils/atomic_io.rs`, `src/bin/aleph-server/commands/start/mod.rs`.

**Date:** 2026-08-10
**Reviewer:** static (4-perspective protocol)

## Summary

| Severity | Count |
|----------|-------|
| Critical | 0 |
| High     | 1 |
| Medium   | 6 |
| Low      | 6 |

The tracker itself is in good shape: the running/completed transition ordering, the
`Notify`-armed-before-read wait loop, the in-loop scope re-check, the presence-only
split, poison-tolerant locks and the two bounds (`BACKGROUND_RESULT_TTL` +
`MAX_COMPLETED_RESULTS`) all check out. Every finding below except B2-09/B2-10/B2-11/B2-12
lives on the **cross-process sidecar seam** (`background_persistence` ↔ `subagent_announce`
↔ `background_tracker::mark_consumed`), which is where the two halves of one fact —
*"did it finish"* vs *"does anyone know"* — are still disagreeing.

---

## Findings

### [HIGH] src/agents/background_persistence.rs:357 — The recovered orphan report is discarded by the announce path in exactly the case it exists for
**Category:** Logic (cross-file wire contract)
**Confidence:** High

**Description:**
`init_and_announce_orphans` packs the whole recovery payload — every orphan's task, outcome
and masked partial result, built by `summarize_orphans` (`:382-434`) — into
`SubAgentCompletionEvent.summary`, and sets `success: interrupted == 0` (`:358`) with a
one-line `error` (`:359-363`). The only consumer, `subagent_announce::announce_one`, selects
the body by `success`:

```rust
// src/gateway/subagent_announce.rs:136-143
let detail = if result.success {
    result.summary.clone()
} else {
    result.error.clone().unwrap_or_else(|| "(no error detail)".to_string())
};
```

So whenever **any** run was genuinely interrupted (`interrupted > 0` — the primary W24 case),
`summary` is dropped on the floor and the parent turn receives only
`"N background sub-agent(s) were interrupted by a daemon restart"`. Every partial result
this module writes, masks, retains for 7 days and re-reads at boot never reaches the model.
Worse for a mixed batch: three interrupted + two finished-unannounced children ⇒ `success=false`
⇒ the "this work is DONE, do not repeat it" paragraph for the finished two is also discarded,
so the model is primed to re-delegate work whose answer is already on disk.

The two producers of this event type disagree about the contract: `spawn.rs:266-280` uses
`summary` for success and `error` for failure (the shape `announce_one` assumes);
`background_persistence.rs:351-369` uses `summary` for the report and `error` for a headline.

**Failure scenario:** daemon is `SIGKILL`ed with 2 background sub-agents in flight → next boot
tombstones both, builds a report containing both partial results → parent session is told only
"2 background sub-agent(s) were interrupted by a daemon restart", with no request_ids beyond the
first (via `child_session_id`), no partial progress, and no instruction not to repeat finished work.

**Suggested fix:** stop overloading `success`/`error` for a grouped notice. Either (a) always put
the rendered block in `summary` and have `announce_one` render `summary` when non-empty regardless
of `success` (`error` becomes a headline appended to it), or (b) give the grouped restart notice its
own event variant whose renderer is not the per-run success/failure switch. Add a test that asserts
the announce *input string* contains an orphan's `partial_result` when `success == false` — the
current tests assert on the event, not on what `announce_one` renders.

---

### [MEDIUM] src/agents/background_tracker.rs:1035 — `mark_consumed` on a *running* entry never stamps `record_announced`, so a cancelled child is re-announced after the next restart
**Category:** Logic
**Confidence:** High

**Description:**
`mark_consumed` stamps the durable "the parent knows" bit only in its **completed** branch
(`:1024-1033`). The running branch (`:1039-1041`) sets `consume_on_completion` and returns —
no `record_announced`. That branch is not hypothetical: it is the documented reason the field
exists, and `loop_tool.rs:528` calls it on the cancel path while the child is still running.

Sequence: model cancels → `consume_on_completion = true`, sidecar still `announced: false` →
child unwinds → `spawn.rs:329` `record_settled` writes `phase: Settled` (leaving `announced`
false) → `mark_completed` births the entry `consumed: true` → `announce_one` returns early at
`subagent_announce.rs:103-110` *without* stamping. Disk record is now `Settled && !announced`,
which is precisely the predicate `init_and_reconcile:279` treats as "finished but the parent
was never told".

**Failure scenario:** parent cancels a background sub-agent, daemon restarts an hour later. Boot
announces a full parent turn: *"1 background sub-agent(s) FINISHED before the daemon stopped, but
the completion notice never reached you… this work is done, do not repeat it"* — about a child the
parent itself killed, whose `outcome` reads `cancelled`. This is the exact spurious-announce turn
`consume_on_completion` was introduced to prevent, resurfacing across a restart boundary.

**Suggested fix:** call `background_persistence::record_announced(request_id)` from the running
branch too (it is a no-op for ids the sidecar does not know, and idempotent via the
`if record.announced { return }` guard) — or, better, from `mark_completed` when
`born_consumed == true`, so the stamp lands at the moment the completed entry is born consumed
regardless of which side set the intent.

---

### [MEDIUM] src/agents/background_persistence.rs:387 — The "FINISHED … do not repeat it" bucket is keyed on `phase == Settled`, which includes failed / timed-out / cancelled runs
**Category:** Logic
**Confidence:** High

**Description:**
`summarize_orphans` partitions on `r.record.phase == RunPhase::Settled` (`:387-389`) and tells the
model the whole bucket *"FINISHED before the daemon stopped … this work is done, do not repeat it"*
(`:425-430`). But `RunPhase::Settled` means only "reached a terminal outcome": `spawn.rs:318-329`
writes it with `label` ∈ `{completed, failed, timed_out, cancelled}`. The label survives in
`record.outcome` and is rendered as one line (`:397-399`), but the authoritative prose above it
asserts the work succeeded.

**Failure scenario:** a background sub-agent fails with a provider 503, the announce path returns
without stamping (`subagent_announce.rs:233-241`, agent unregistered / non-busy error / retries
exhausted), the daemon restarts. The parent is told the run "FINISHED … this work is done, do not
repeat it" with a one-line `outcome: failed` underneath — the opposite of the action the model
should take.

**Suggested fix:** partition on the *outcome*, not the phase: `Settled && outcome == Some("completed")`
into the "done, do not repeat" bucket; every other terminal label into a third bucket whose prose says
the run ended without producing a usable result and names the label.

---

### [MEDIUM] src/agents/background_persistence.rs:285 — The one-shot `announced` stamp / tombstone is spent before the notification is confirmed delivered
**Category:** Logic
**Confidence:** High

**Description:**
`init_and_reconcile` writes the durable one-shot marks *before* anything is delivered: the
`Abandoned` tombstone at `:267` and `announced: true` at `:281-285`. The delivery happens later,
in `init_and_announce_orphans` (`:370-377`), and is itself best-effort — `announce_one` gives up
without any durable record on an unparseable session key (`:112-119`), an unregistered agent
(`:122-129`), a non-busy execution error (`:233-241`), or after the whole 0/30/120 s retry ladder
(`:245-249`). Because the record now reads `Settled && announced` (or `Abandoned`, which the
reconcile never revisits at all), **no future boot will ever retry it.**

This is the repo's own criterion "一次性的章不能在动作确认之前花掉 / 要么事后盖，要么必须可归还"
(CLAUDE.md §0) applied to the very mechanism that exists to stop a promise being silently withdrawn.

**Failure scenario:** daemon restarts, orphans are tombstoned and stamped, then the parent agent is
not yet in the registry (boot ordering across agent registration) → `announce_one` warns and returns
→ the parent is never told, on this boot or any later one.

**Suggested fix:** move the stamp to the delivery's success arm (`announce_one` already calls
`record_announced` there for the live path — reuse it), i.e. hand `init_and_announce_orphans` the
recovered set with the record still un-stamped, and stamp per-run after `adapter.execute` returns
`Ok`. For the `Abandoned` tombstone, keep writing it (it is the phase verdict, not the notice) but
add a separate `announced` bit to it so an undelivered orphan notice can be retried on the next boot.

---

### [MEDIUM] src/agents/background_persistence.rs:391 — The grouped orphan summary is unbounded: N runs × 8 KiB each into one parent-turn prompt
**Category:** Quality (unbounded growth)
**Confidence:** High

**Description:**
`summarize_orphans`'s `render` closure (`:391-409`) walks **every** recovered run of a session and
inlines the full `partial_result` — each bounded at `PARTIAL_RESULT_TAIL_BYTES` (8 KiB, `:79`) but
with no cap on the number of runs and no cap on the total. The result becomes the `input` of a real
parent run (`subagent_announce.rs:144-152`, `:197-209`). The sibling face for the same data
deliberately does the opposite: `recovery::to_list_row` previews at `LIST_RESULT_PREVIEW_CHARS`
(200) and `loop_tool.rs:642-682` caps rows at `MAX_LISTED_COMPLETED` with an explicit
anti-silent-truncation note.

**Failure scenario:** a session running a 30-way background fan-out is orphaned by a restart →
~240 KiB of trail text is concatenated into one prompt, blowing the parent's context budget (or a
large chunk of it) on a single boot notification.

**Suggested fix:** cap the rendered group (e.g. the N most recent by `last_activity_ms`, plus a
per-run preview rather than the full tail) and add the same "showing X of Y; the rest are retrievable
by request_id via check_status" note the list face already carries. The full text is one `check_status`
away — the notice does not need to carry it.

---

### [MEDIUM] src/agents/background_persistence.rs:669 — Blocking file I/O (including `fsync`) on tokio worker threads, once per progress event and once per `wait` consume
**Category:** Quality
**Confidence:** High

**Description:**
Every write entry point does synchronous `std::fs` work with no `spawn_blocking`, and all of them
are reached from async contexts:

- `record_activity` → `append_trail` (`:669-688`, `create_dir_all` + `OpenOptions::open` + `write_all`)
  is called from `background_tracker::push_progress:1337`, which is called from
  `ForwardingTraceSink::on_trace:132` — i.e. **once per tool call and once per Think transition** of
  every background sub-agent, inline on the harness's async loop.
- `record_announced` (`:511-525`) → `write_state` → `atomic_io::write_atomic`, which is
  tempfile + `sync_all()` (**an fsync**) + rename. It is called from
  `background_tracker::mark_consumed:1031`, which `wait_any:949` calls from inside an `async fn`.
- `record_start` / `record_settled` are likewise fsync-per-call inside the spawned run task.

The twin sidecar built from the same template explicitly avoids this shape:
`process_journal` rewrites in place instead of appending, batches through
`spawn_partial_flusher` (`process_journal.rs:548-567`), and keeps a `written` dirty-map whose stated
purpose is that rewriting an unchanged file "would cost a `fsync` per idle job"
(`process_journal.rs:590-593`). Two sibling subsystems answering the same question two different
ways is the divergence CLAUDE.md §0 calls out ("两个子系统是孪生时，一边修好的判据要主动搬过去").

**Failure scenario:** a background sub-agent making rapid tool calls on a busy/slow filesystem stalls
a tokio worker for the duration of each append; a parent parked in `wait_any` pays an fsync on the
runtime thread the moment a child completes.

**Suggested fix:** at minimum route `record_announced` / `record_settled` / `record_start` through
`tokio::task::spawn_blocking` (they are fire-and-forget already), and batch the activity trail the way
`process_journal` does — an in-memory tail plus a periodic flusher — rather than one `open`+`write`
per progress event.

---

### [MEDIUM] src/agents/background_persistence.rs:576 — `list_for_scope` fully reads and re-renders every retained record's trail on every `subagent list`; the trail file itself has no size cap
**Category:** Quality (efficiency / unbounded growth)
**Confidence:** High

**Description:**
`list_for_scope` (`:576-600`) calls `read_trail` for **every** in-scope record in `INDEX`, and
`INDEX` now deliberately retains terminal records for `RECORD_RETENTION_MS` = 7 days (`:73`,
`:100-115`). `read_trail` (`:704-752`) reads the whole file with `read_to_string`, splits it, joins
**all** lines into a second full-size `String` (`:723-727`), and only then slices the last 8 KiB.
Peak transient memory is ~2× the file, and the caller (`recovery.rs:434`, from `loop_tool.rs:640`)
discards everything but a 200-char preview of at most `MAX_LISTED_COMPLETED` rows.

The trail file is append-only with no cap: one line per progress event (`append_trail:675`), each up
to `MAX_LINE_CHARS` = 4000 chars, for the whole life of a background run. The twin module rejects the
append-only shape for exactly this reason ("the reader only ever shows the last `OUTPUT_TAIL_BYTES`
anyway, so there is nothing to accumulate", `process_journal.rs:507-509`).

**Failure scenario:** a week-old daemon holding ~300 retained records → each `subagent list` action
performs 300 blocking full-file reads plus 300 full-string joins on the async thread, to produce at
most `MAX_LISTED_COMPLETED` 200-char previews.

**Suggested fix:** (a) read only the tail (seek to `len - PARTIAL_RESULT_TAIL_BYTES` and drop the
first partial line) instead of `read_to_string` + join; (b) give `list_for_scope` a bound/ordering
argument so it only materialises the rows the caller will render; (c) cap the trail file (truncate-
and-rotate, or adopt the twin's rewrite-in-place tail).

---

### [LOW] src/agents/background_persistence.rs:486 — `record_settled` appends to the trail *before* the index check, creating run directories the retention sweep can never prune
**Category:** Quality
**Confidence:** High

**Description:**
`record_settled` (`:486-502`) calls `append_trail` at `:489` — which does `create_dir_all` + creates
`result.txt` — and only afterwards checks `index.get_mut(request_id)`, returning early for an unknown
id (`:493-495`) without ever writing `state.json`. `read_all` (`:754-775`) skips any directory with no
readable `state.json`, so the retention sweep in `init_and_reconcile` (`:251-258`) never sees or removes
such a directory. The twin does it in the safe order: `process_journal::record_settled` checks
`index_lock().contains_key(&id)` first (`process_journal.rs:620`).

Reachable when persistence is toggled or the index is replaced between `record_start` and
`record_settled` (`init_and_reconcile:299` overwrites `INDEX` wholesale), and in tests via
`disable_for_test` / `enable_for_test`.

**Suggested fix:** move the index check above the `append_trail` call, matching `record_activity`
(`:477-479`) and the twin.

---

### [LOW] src/agents/background_tracker.rs:490 — TOCTOU on `consume_on_completion` between the running snapshot and the completed insert
**Category:** Logic
**Confidence:** High (race is real; window is narrow)

**Description:**
`mark_completed` reads `already_consumed` at `:490`, snapshots the running entry (including
`consume_on_completion`) under a read lock at `:510-545`, inserts the completed entry at `:552`, and
only removes the running entry at `:590-596`. A `mark_consumed` landing in the window between the
running snapshot and the completed insert takes the *running* branch (the completed entry does not
exist yet), writes `consume_on_completion = true` onto an entry that is about to be removed, and the
flag is lost — the completed entry is born un-consumed.

**Failure scenario:** the model issues `cancel` (`loop_tool.rs:517-528`) at the same moment the child
settles → the `Err("sub-agent failed: cancelled")` reaches `subagent_announce` un-consumed and spends
a full parent turn reporting the failure of a sub-agent the parent just killed — the exact bug
`consume_on_completion` exists to prevent, just at a narrower window.

**Suggested fix:** re-read the consumed intent while holding the `completed` write lock (i.e. fold
`consume_on_completion` into the same critical section that inserts), or take the running write lock
for the whole transition instead of read-then-write.

---

### [LOW] src/agents/background_tracker.rs:575 — The `MAX_COMPLETED_RESULTS` eviction is process-global, so one session can evict another session's unread results
**Category:** Logic
**Confidence:** High

**Description:**
The count cap (`:575-585`) sorts and evicts oldest-by-`completed_at` across the whole map, which is
process-global and cross-session (`GLOBAL_TRACKER`, `:47`). A session that completes 256+ background
sub-agents evicts other sessions' entries, including ones whose parent has neither polled nor been
announced to — those ids then answer "unknown request_id" from every by-id face
(`result_snapshot` / `wait` / `unknown_ids`).

The sidecar covers the gap when persistence is enabled, but the tracker's own bound should not be
another session's problem in the first place.

**Suggested fix:** apply the cap per `meta.root_session` (evict the oldest of the *inserting* session)
rather than globally; the map stays bounded by `sessions × MAX_COMPLETED_RESULTS`, which is what
`subagent_snapshot` already reports per-session anyway.

---

### [LOW] src/agents/background_tracker.rs:936 — `wait_any` / `unknown_ids` clone the full `CompletedSnapshot` just to test presence
**Category:** Quality (efficiency)
**Confidence:** High

**Description:**
`result_snapshot` (`:818-836`) clones `outcome` (the child's entire final text) and
`progress_tail` (up to 10 structs). `wait_any` calls it for every id on **every lap** (`:936`) —
including ids it immediately discards as already-delivered (`:937-938`) — and every process-wide
completion wakes every parked waiter into another lap. `unknown_ids` (`:1192-1200`) does the same for
a pure existence test, and `loop_tool` folds `unknown_ids` into every outcome.

**Suggested fix:** add a private `completed_state(id, scope) -> Option<bool /*consumed*/>` (or
`contains_completed` + `is_consumed`) that answers under the read lock without cloning, and only build
the snapshot for the one id actually being returned.

---

### [LOW] src/agents/background_tracker.rs:486 — Stale invariant comment: "no code path ever holds two of this struct's locks at the same time" is false
**Category:** Architecture (documented invariant vs code)
**Confidence:** High

**Description:**
`mark_completed`'s comment at `:485-487` states the invariant as fact, but `subagent_snapshot`
(`:760-767`) holds the `running` read guard and the `completed` read guard simultaneously for the
whole body. It is not a deadlock today (every other site releases `completed` before touching
`running`, so no path establishes the reverse order), but the comment is the only thing recording the
ordering rule, and it currently records a rule the code does not follow — a future
`completed`-then-`running` acquisition would be a genuine inversion with nothing to catch it.

**Suggested fix:** restate the invariant as a *lock order* ("`running` before `completed`, never the
reverse") on the struct, and note `subagent_snapshot` as the one site that holds both.

---

### [LOW] src/agents/forwarding_trace_sink.rs:109 — `render_tool_result_preview` serializes the entire tool output to keep 200 characters
**Category:** Quality (efficiency)
**Confidence:** High

**Description:**
`serde_json::to_string(output)` (`:112`) materialises the full JSON of every successful tool result
before `.chars().take(200)` (`:116`), and then walks the whole string again for
`raw.chars().count()` (`:117`). This runs on every `ToolCallCompleted` of every background
sub-agent — including `read_file` / `bash` / search results that can be megabytes.

**Suggested fix:** short-circuit on the common shapes (`Value::String` / small objects) or use a
bounded serializer (`serde_json::to_writer` into a capped sink) and replace the second full scan with
a `chars().nth(200).is_some()` check.

---

## Cross-cutting observations

1. **Two halves of one addressing rule agree; two halves of one *delivery* rule do not.**
   `BackgroundAgentTracker::addressable` and `background_persistence::addressable` are now
   consistently strict (the empty-`root_session` fail-open was fixed and is documented at
   `background_persistence.rs:551-563`). The *delivery* fact ("does the parent know") is still split
   across three places with three different producers — `CompletedAgent.consumed`,
   `RunningAgent.consume_on_completion`, and `PersistedRun.announced` — and B2-02 is the seam where
   they disagree. A single `mark_consumed` chokepoint that always stamps all three would remove the
   whole class.

2. **`subagent_tree_events.rs` is correct.** `now_ms` saturates rather than panicking, and
   `emit_tree_event` short-circuits on an empty session id before `tokio::spawn`. The documented
   requirement ("must be called from within a Tokio runtime") holds for every caller I traced
   (`spawn.rs:121/295` inside the run task, `forwarding_trace_sink.rs:133` inside the harness loop);
   `flush()` — the one method a shutdown/Drop path might call outside a runtime — does not emit. If a
   non-async emitter is ever added, `Handle::try_current()` (the pattern
   `process_journal::spawn_partial_flusher` already uses) would make that safe by construction.

3. **`ForwardingTraceSink` pointer/drop semantics are clean.** It holds `Arc<dyn TraceSink>` +
   `Arc<BackgroundAgentTracker>` by value, takes no locks of its own, always forwards to `inner`
   (both `on_trace` and `flush`), and takes the tracker lock only inside `push_progress` — which is
   dropped before the persistence write (`background_tracker.rs:1331`) and before
   `emit_tree_event`. No reentrancy into the tracker from the inner sink is possible because the
   guard is released first. A `push_progress` arriving after `mark_completed` returns `None` and
   correctly suppresses the tree event rather than emitting a `Progress` for a settled node.

4. **Semaphore ordering vs. tracker registration is deliberate and consistent.**
   `spawn_background` registers with the tracker and the sidecar *before* the child takes its
   permit (`subagent_spawner/mod.rs:208`), so a child cancelled while queued still has both a
   running entry and a disk record — and `background_persistence.rs:568-574` names that as the reason
   the `list` face exists. `subagent_semaphore_for` is keyed per top-level session with `Weak` +
   opportunistic prune, so the cap outlives the per-request `SubagentTool`, matching the
   tracker's own engine-lifetime correction. No issue found here.

5. **Parent/child attach semantics are honest but flat.** `spawn_background` always sets
   `parent_id: None` (`spawn.rs:100-105`) with a comment saying every background sub-agent attaches
   under the session root, so `running_children_of` only ever matches presence-only registrations
   (team-chat fan-out). Consistent with `flat_nodes`' documentation — noting it because the "tree"
   the panel renders for background sub-agents is structurally always depth-1 under the root.

6. **No panic sites found in the production paths of these four files.** All lock acquisitions use
   `unwrap_or_else(|e| e.into_inner())`; the only `expect`/`unwrap` calls are under `#[cfg(test)]`;
   integer conversions use `try_from(..).unwrap_or(u64::MAX)`; string truncation is char-indexed in
   both `preview_from_outcome` and `read_trail`'s tail slice (the `take_while(..).last()` fix at
   `background_persistence.rs:740-746` is correct — the boundary is the smallest satisfying index).
   `Instant::now() + timeout` in `wait_any:916` would panic on a `Duration` near `MAX`, but every
   caller clamps against `MAX_WAIT_TIMEOUT_SECS`.

7. **Not reported, but worth knowing:** `SubagentNode.result_preview` (`background_tracker.rs:1249`)
   carries up to 200 chars of the child's raw final text to the panel over `subagent.tree` **without**
   passing through `SecretMasker`, while the disk sidecar masks unconditionally and documents why
   (§5.1). The panel RPC is session-scoped and was an existing egress before this field was added, so
   I did not raise it as a finding — but the asymmetry between the two egresses of the same bytes is
   the shape §5.1 warns about, and is worth a deliberate decision rather than an accident.

8. **`register_with_meta` silently overwrites an existing entry** with the same `request_id`
   (`insert_running:458`), dropping the previous `CancellationToken` and making that run
   permanently uncancellable. Not reported because every production id is a fresh UUID
   (`spawn.rs:84`), but a debug assertion or a `warn!` on collision would be cheap insurance for the
   day an id becomes caller-supplied.

## Files reviewed

| File | LOC | Findings |
|------|-----|----------|
| `src/agents/background_tracker.rs` | 2869 | 4 (B2-02, B2-09, B2-10, B2-11, B2-12 → 5 anchors, 4 unique bugs + 1 doc) |
| `src/agents/background_persistence.rs` | 1068 | 7 |
| `src/agents/forwarding_trace_sink.rs` | 310 | 1 |
| `src/agents/subagent_tree_events.rs` | 38 | 0 |
| **Total in scope** | **4285** | **13** |
