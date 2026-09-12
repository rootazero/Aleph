# pi persistence / recovery scan

All anchors are against `T:\Github\pi` at HEAD `71dca871b` (2026-09-11). Read via `git archive HEAD` into the scratchpad because the working tree was being re-checked-out mid-scan (files under `packages/agent/src/harness/runtime/` showed as deleted in the tree while present in HEAD) and `graphify-out/graph.json` was built from the 2026-08-11 tree (it still points at a `reducer.ts`/`state.ts` pair that no longer exists).

## Framing finding: two stacks, one repo

- **Shipped (TUI / RPC / SDK)** = v3 `SessionManager` + non-durable `Agent` loop: `packages/coding-agent/src/core/session-manager.ts:30` (`CURRENT_SESSION_VERSION = 3`), consumed by `packages/coding-agent/src/modes/interactive/interactive-mode.ts:94` and `core/agent-session.ts:640-657`.
- **Durable harness (v4, "registers")** = `packages/agent/src/harness/{session,runtime}` (~22.8k lines incl. tests/conformance) — a full OS/DB-style design, consumed **only** by `packages/coding-agent/src/experimental/session-worker.ts:834`, `experimental/mini/worker/run.ts:66`, and `packages/evals`. Not on the production TUI path.
- `packages/agent/docs/harness.md` (1468 lines) is the v4 design spec; `packages/agent/docs/work-packages/00–09` track its build-out (WP07 marked "implemented").
- History: on 2026-08-04/05 the v4 layer was an append-only **record journal** (`operation_started` / `step_attempt` / `tool_started` … + `reduceLaneState` fold) with `AgentHarness` a scaffold throwing `HarnessNotImplemented`. By 2026-09-11 it was rewritten to the **registers + transactions** model below. Both designs are worth knowing; only the register one is live.

Everything below labels v3 vs v4.

---

## 1. The durable log

### v3 (shipped)
- JSONL, `~/.pi/agent/sessions/--<cwd-encoded>--/<ISO-ts>_<id>.jsonl`; line 1 = `{type:"session",version:3,id,timestamp,cwd,parentSession?}`; one entry per line (`session-manager.ts:32-39`).
- Append via `appendFileSync` in `_persist` (`session-manager.ts:1029-1056`). **Lazy flush: nothing hits disk until the first assistant message exists** (`:1032-1041`) — a crash before the first reply loses the user prompt entirely.
- Migrations v1→v2→v3 mutate in memory then rewrite the whole file with `openSync(path,"w")` (`:281-291`, `_rewriteFile` `:993-1003`) — non-atomic; a crash mid-rewrite truncates the session.
- Corruption: `parseSessionEntries` silently skips any unparsable line **anywhere** in the file (`:299-313`); no parent-chain or duplicate-id validation on load; `_buildIndex` (`:972-991`) only builds `byId`, labels, and leaf.
- No fsync, no rotation, no header checksum.

### v4 JSONL (`packages/agent/src/harness/session/jsonl/`)
- Header `{kind:"header",v:4,id,cwd,storageVersion,createdAt,nextSeq?,parentSessionId?,legacyParentSessionPath?}` (`codec.ts:34-47`); v3 headers are recognised as `v3-legacy` (`codec.ts:21-32,53-63`).
- **One line = one atomic transaction**: an array of committed writes (`entry` / `usage` / `value set|delete` / `list append|delete`), each stamped with a strictly increasing `seq` (`io.ts:44-78`, `storage.ts:143-149`). Single-write transactions are serialised unwrapped.
- Torn tail: an unterminated last line is dropped **whole** and the valid prefix republished via tmp-file + rename (`storage.ts:30-35`, `:108-112`, `io.ts:81-104`). A *complete* but invalid line fails closed with line number (`storage.ts:99-106`). `header.nextSeq` is a high-water mark so forks can skip sequence ranges (`:107`).
- Legacy v3 files open read-only through `LegacyV3Source` (`legacy-v3.ts`), and are upgraded to v4 **atomically on the first write** — the whole v3 content is re-emitted as v4 writes plus an imported-usage adjustment row, then the caller's write (`storage.ts:139-141`, `:154-190`). Migration is strict: duplicate ids, missing or forward parents throw (`legacy-v3.ts:401-410`, `:474-476`) — stricter than v3's own loader. v3 `model_change`/`thinking_level_change`/`session_info`/`label` entries are folded into registers and removed from the tree; `branch.tip = last entry in file` mirrors v3's in-memory-leaf semantics (`legacy-v3.ts:486-524`).
- **No `fsync`/`fdatasync` anywhere** in `packages/agent`, `packages/coding-agent`, or `packages/session-backends` (grep-verified). Durability = the OS page cache.
- Serialisation: a promise chain `commitQueue` serialises commits in-process (`storage.ts:128-136`); no cross-process guard.

### v4 SQLite (`packages/session-backends/sqlite-node/src/sqlite/`)
- Authoritative tables: `entries`, `scalar_values`, `list_values`, `usage_ledger`; `branch_entries`/`branch_meta` and stats columns are rebuildable projections (`migrations/001_initial.sql:1-8`, `:94-121`). One DB file per session by default, but every row is `session_id`-scoped so a shared container works.
- Cross-table invariants live in triggers: missing parent entry, and a **shared entry/usage id namespace** (`001_initial.sql:74-92`).
- `PRAGMA journal_mode=WAL; busy_timeout=5000` only (`repo.ts:65`). The August tree's `synchronous=FULL` and the fenced `writer_leases` table (owner_id + fence + expires_at) are **gone at HEAD**; WP07 §1.1 says explicitly: "Do not add a storage lease, filesystem lock, fencing token, heartbeat, timeout-based takeover…" — single-writer authority moved to the host process (server → one session-worker), `session-worker-manager.ts:394,697,799`.

---

## 2. Event vocabulary

### v3 entry kinds (`session-manager.ts:144-155`)
`message` (user / assistant / toolResult / bashExecution wrapped as one `AgentMessage`; thinking blocks live inside assistant content), `thinking_level_change`, `model_change`, `compaction{summary,firstKeptEntryId,tokensBefore,details,usage,fromHook}`, `branch_summary{fromId,summary,details,usage,fromHook}`, `custom{customType,data}` (state only, never in context), `custom_message{customType,content,display,details}` (in context), `label{targetId,label}`, `session_info{name}`.
**Not persisted in v3**: tool intent / start, abort requests, retry attempts, queued steer/follow-up items, slash-command invocations, skill/template identity (only the expanded user text), extension load state, MCP state, subagent spawn/result.

### v4: three stores (spec §0.3; `session/types.ts`)
- **Entries** — facts, write-once, only **four** kinds: `message{message,terminate?}`, `compaction{summary,retainedTail,tokensBefore,details,usage,fromHook}`, `branch_summary`, `custom{customType,data}` (`types.ts:16-64`). Model/thinking/tool-set changes are **no longer entries**.
- **Registers** — current mutable state, namespaced typed cells (`session/values.ts:158-195`): `pi.branch.tip/<branch>`, `pi.lane.config/<lane>` (`{model,thinkingLevel,activeToolNames}`), `pi.lane.state/<lane>` (`{currentOperationId,lastOperationId,inbox[]}`), `pi.op.meta/<op>`, `pi.op.state/<op>`, `pi.op.tool_args/<op>:<step>:<idx>`, `pi.op.tool_memo/<op>:<invocation>:<name>`, `pi.op.preparation/<op>:<task>`, `pi.pending.entry/<entryId>`, `pi.pending.tool_output/<op>:<invocation>`, `pi.pending.assistant_frames/<op>:<response>` (a list), `pi.result/<op>`, `pi.session.name`, `pi.entry.label/<entryId>`.
- **Usage ledger** — append-only rows `{id,seq,usage,entryId?,adjustment,details}` (`types.ts:379-386`).
- Derived: `branch_*` caches, stats, session listing.
- Facts vs derived: entries + ledger are immutable facts; registers are *current state* (history is not kept — an overwritten register is gone, spec §0.6); everything else is a projection.

---

## 3. Tree / branch model

- Both stacks: `id`/`parentId` chain; a branch is the path leaf→root; every entry is a potential fork point.
- **v3 leaf pointer is in-memory only**: `branch(branchFromId)` just sets `this.leafId` (`session-manager.ts:1374-1379`); `resetLeaf()` sets it to null; on reload `_buildIndex` derives leaf = **last entry in file order** (`:972-991`). Consequence: a `/tree` jump followed by a crash before the next append is silently lost; the jump becomes durable only because the next appended entry carries `parentId = jumpTarget`. `branchWithSummary` (`:1398-1420`) appends a `branch_summary` entry at the target so it *is* durable.
- **v4**: `pi.branch.tip/<branch>` is a durable register, moved in the same transaction as the placed entry (spec §2.2). Navigation is a first-class **operation** (`OperationMeta.intent = {kind:"navigation", targetId, summarize, label?, customInstructions?}` — `types.ts:75-90`) with optional LLM branch summary; it commits at `navigation.ready_to_commit` (`types.ts:308-314`) and can be aborted or crash-recovered like a run. Multiple named **lanes** share one tree (per-lane `tip/config/state` registers); `restoreSession` scans all three prefixes and classifies each name as `absent | branch (tip only) | lane` (`runtime/restore.ts:63-115`).
- What the model sees after a switch: v3 — `buildSessionContext` from the new leaf; v4 — bounded transcript read from the new tip stopping at the first compaction (`runtime/transcript.ts:50-68`); UI clients get `navigation_end` → `"rebase"` and must resnapshot (§6).
- Fork: v3 `/fork` = `createBranchedSession` writes the root→leaf path into a new file, re-chaining around label entries (`:1449+`). v4 fork copies entries then **projects registers by namespace**: `pi.session.name` kept, `pi.entry.label` only for copied entries, `pi.branch.tip` rewritten to the destination tip, `pi.lane.state` reset to idle, all `pi.op.*` / `pi.pending.*` / `pi.result` dropped, any unknown `pi.*` namespace throws (`session/fork-policy.ts:40-67`). Server-side fork may open a live worker-owned SQLite source read-only for a coherent snapshot (WP07 §1.2).

---

## 4. Reduction to run state (resume after crash)

### v3: NOT FOUND
No run state is persisted. Interrupted turns leave an assistant entry with `stopReason:"error"|"aborted"` (persisted on `message_end`, `agent-session.ts:640-657`) and possibly tool calls without results. Nothing inspects or repairs the file on resume; repair happens at the provider boundary (§5).

### v4: the durable program counter
- After every step the harness **overwrites** `pi.op.state/<opId>` with the *total* current state (no dependence on the previous value). Thirteen flat leaves (`session/types.ts:316-330`): `starting`, `checkpoint`, `assistant.ready`, `assistant.effect_pending{attempt,responseEntryId,usageId,intendedOutputLimit,contextWindow}`, `assistant.retry_wait`, `tools{batch}`, `deferred.suspended`, `deferred.effect_pending`, `summary.deciding|ready|effect_pending|retry_wait`, `navigation.ready_to_commit`. Cancellation is orthogonal: `control = running | cancel_requested{requestedAt}` (`types.ts:92-97`). Every leaf carries captured `settings`, `configuration`, `streamOptions`, `retryPolicy` inline (`types.ts:142-155`, `:189-201`) so recovery never consults live config.
- Tool batch child state machine: `ToolCall.status = planned → effect_pending{replay} → outcome_ready{terminate} → completed{terminate}` keyed by `sourceIndex` + reserved `resultEntryId` (`types.ts:157-170`).
- **Restore = point lookups, no fold, no scan**: read `branch.tip`, `lane.config`, `lane.state` (3 reads), then `op.meta` + `op.state` if an operation is open (`runtime/restore.ts:131-171`). Invariant checks (`:144-160`, `stateMatchesIntent` `:26-45`) throw `SessionInvariantError`; `createAgentHarness` wraps any such failure in `HarnessFault` and refuses to open (`runtime/harness.ts:389-406`) — **reject, never repair**. Open operations are returned to the caller as `open[]`; nothing auto-drives; the app calls `lane.resume()` (`runtime/lane.ts:1327+`).
- Uncertain-window policy (spec §4.5, code `runtime/drive/recovery.ts`, `runtime/drive/tools.ts`):
  - assistant `effect_pending` → the **committed frame prefix** (list register, `runtime/progress.ts:15-33`) is reduced via `reduceAssistantMessageFrames` into a synthetic `stopReason:"error"` message with warning text, published under the **reserved** `responseEntryId` (`recovery.ts:22-41`, `:44-84`); retries honour the captured policy's `attempt`/`maxAttempts`.
  - tool `effect_pending` → re-execute only if the stored declaration **and** the current tool declaration are both `replay:"safe"` (`tools.ts:527`); otherwise publish a synthetic `interrupted` result that carries the last `pending.tool_output` checkpoint (`tools.ts:45`, `:158-180`, `:534-539`).
  - cancelled control → synthetic `aborted` under the same reserved ids (`recovery.ts:87-126`), never retried.
  - **No tool in the repo declares `replay:"safe"`** (grep across `harness/tools`, `coding-agent/core/tools`): the safe-replay path has zero producers today.
- Streaming frames and tool progress snapshots are written through a "still owns the effect" guard so a stale writer after a takeover is a no-op (`progress.ts:35-67`, `:69-117`). Every recovery-synthesised message is flagged `recovery: true` on its events (`agent-harness.ts:389-390`).

---

## 5. Reduction to model context

- **v3**: `buildContextEntries` walks leaf→root, keeps `[latest compaction entry, entries from firstKeptEntryId…, everything after the compaction]` (`session-manager.ts:418-460`); `sessionEntryToContextMessages` turns compaction / branch_summary into synthetic messages, drops `custom`, guards null content from hand-edited files (`:383-416`). Model/thinking are re-derived from the path (`:361-378`).
- **Dangling tool calls are repaired only at the provider transform boundary**: `packages/ai/src/api/transform-messages.ts:163-229` inserts `"No result provided"` error tool results for orphaned calls and **drops every `error`/`aborted` assistant message** from what the model sees (`:195-197`). The file, the TUI transcript, and the model therefore hold three different conversations.
- **v4**: `CompactionEntry.retainedTail` copies the kept tail *into* the checkpoint (`types.ts:33-41`), so context = `[compaction summary message, …retainedTail, everything after]` with no pointer chasing (`session/context.ts:45-57`, `:75-80`); `stopReason:"deferred"` assistant entries are hidden (`:72`); `custom` entries enter context only through a registered `EntryProjector[customType]` (`:84-86`); the branch read stops at the first compaction (`runtime/transcript.ts:50-68`). Compaction itself is an operation with `summary.*` leaves and a durable `DurableStructuralPreparation` (`types.ts:360-377`), so a crash mid-summary resumes the same summarisation, and an in-run overflow compaction is tracked by `Continuation.overflowRecoveryUsed` (`types.ts:119-127`).

---

## 6. Reduction to UI

- **v3 listing**: `buildSessionInfo` streams **every session file end-to-end** on each picker open to derive `name` (last `session_info`), `firstMessage`, `allMessagesText`, `messageCount`, last activity (`session-manager.ts:688-770`, concurrency wrapper `:772+`); no index or cache. `/tree` renders from `getTree()`; the transcript is rebuilt from `buildContextEntries` + `sessionEntryToContextMessages` (`interactive-mode.ts:94`).
- **v4 listing** reads only the header line (`jsonl/repo.ts:246`, `readTextLines maxLines:1`); the session name is a register, so listing cannot show it without opening the file — a regression versus v3.
- **Live clients**: `lane.watch()` yields one coherent `LaneSnapshot{lane, transcript, tipId, lastResult?, configuration, stats, operation{id,kind,startedAt,fromTipId,status,retry?,deferred?,streamingMessage?,runningTools[]}, queues[], faulted}` (`agent-harness.ts:228-249`). The normative client fold `reduceLaneSnapshot(snapshot, event)` mutates the snapshot and returns `"rebase"` on `navigation_end`, on which the client calls `resnapshot()` without resubscribing (`runtime/reducer.ts:22-221`). Event vocabulary: `run_start|run_end|run_suspend|run_resume|turn_start|turn_end|message_start|message_update|message_end|tool_start|tool_update|tool_end|retry_scheduled|retry_start|retry_end|compaction_start|compaction_end|navigation_start|navigation_end|operation_abort|queue_update|entry_added|value_update|config_update|lane_created|usage|fault|handler_error`.
- **`ServerSnapshot.revision` / `LiveSession.connections`: NOT FOUND.** The experimental server uses per-connection attachment "demands" (`session-worker.ts:244-283`) and worker pids (`session-worker-manager.ts:686-697`), not revision numbers.

---

## 7. Extensions / skills / prompt-templates / MCP

- **v3**: `pi.appendEntry(customType, data)` → `custom` entry (`extensions/types.ts:1381`); extensions rebuild state by rescanning entries on `session_start{reason: startup|reload|new|resume|fork}` (`:565`). `custom_message` entries are the in-context channel. Loaded-extension set, tool registrations, MCP server state: **not persisted** — rebuilt from disk config on every start. A skill or prompt-template invocation persists as the **expanded user text** only (`runtime/lane.ts:529-537` for v4 too): no structured "skill X invoked with args Y" fact exists in either stack.
- **v4**: same `custom` entry + `EntryProjector`; plus per-invocation durable **memos** (`pi.op.tool_memo`, `runtime/drive/tools.ts:95-125`, deleted on replay-checkpoint clear) so a replay-safe tool can be idempotent across crashes. Hooks at HEAD: `before_run, before_drive, before_run_end, transform_context, before_request, before_payload, after_response, before_tool, after_tool, before_compaction, before_navigation` (`agent-harness.ts:430-502`) — no `before_resume`. Hook outputs become durable only in the transaction that consumes them; a crash before that may rerun the hook (spec §0.3 item 4).

---

## 8. Subagents

- **Core: NOT FOUND.** `parentSessionId` denotes fork lineage only (`session/types.ts:473-480`).
- The reference implementation is an *example extension* that spawns `pi --mode json -p --no-session` child processes (`packages/coding-agent/examples/extensions/subagent/index.ts:300`, `:346`); children are ephemeral, unlinked to any session file, killed with SIGTERM→SIGKILL on abort (`:411-424`); their result survives only inside the parent's `toolResult.details`. A crash mid-subagent leaves nothing recoverable or even nameable.
- v4's multi-lane model (per-lane `pi.lane.*` registers in one session, spec §0.4 "Slack thread" example) is the intended substitute — each lane has its own operation state and can be restored independently.

---

## 9. Recoverable side-effects

- **v3**: none. Tools run with no intent record; retries are in-process counters; `agent-loop.ts` synthesises `"Operation aborted"` tool results only for the live abort path (`agent-loop.ts:629-633`).
- **v4 "effect sandwich"** (spec §0.3 item 4, §4.5): commit intent (`op.state = …effect_pending`, `op.tool_args`, reserved `resultEntryId` + `usageId`) → do the effect → commit settlement (output + usage + next state). The reserved `resultEntryId` doubles as the `invocationId` idempotency key (`tools.ts:98`). Retry attempts are numbered inside the captured `retryPolicy` (`AssistantEffectPendingOperation.attempt`, `types.ts:263-270`); recovery uses the **captured** policy and model identity, not current config (`recovery.ts:59-62`). Exactly-once external effects and provider stream resumption are explicit non-goals (spec §0.6); the one uncertain interval is "intent durable, settlement absent" and it is handled by the §4 policy table.

---

## 10. Slash commands

- Not logged as commands in either stack.
- v3 effects that do persist (`agent-session.ts`): `/model` → `appendModelChange` (`:1687,1754,1789`); thinking level → `appendThinkingLevelChange` (`:1829`); `/compact` → `appendCompaction` (`:2053`, auto-compaction `:2379`); `/name` → `appendSessionInfo` (`:3115`); `/tree` label → `appendLabelChange` (`:2630,3297,3309`); `/tree` jump → `branch()` / `resetLeaf()` (**in-memory only**, `:3301-3304`); `/tree` with summary → `branchWithSummary` (`:3286`); `/fork` → new file. `/resume`, `/new`, `/export`, `/share`, `/session` write nothing.
- v4 turns `/compact` and tree navigation into durable **operations** (`summary.*`, `navigation.ready_to_commit`) that survive a crash mid-summary, and `/model` etc. into `lane.config` register overwrites (`values.ts:160`).

---

## Top 8 patterns worth porting to a Rust core

1. **Three stores, one invariant** — write-once entries / overwrite registers / append-only usage ledger; everything else is a rebuildable projection with no authority (`001_initial.sql:1-8`, spec §0.3).
2. **Total program-counter register** (`pi.op.state`) overwritten per step; restore is five point lookups, never a fold or journal replay (`runtime/restore.ts:92-171`). Maps directly onto Aleph's A3 "state rebuildable, trend toward pure reducer" *without* paying a replay on every open.
3. **Effect sandwich with reserved ids**: the intent commit names the ids the outcome *will* occupy; the synthetic result lands under that id, so "did it run?" and "did we record it?" can never diverge (`recovery.ts:22-41`, `tools.ts:158-180`). This is Aleph criterion §15 made mechanical.
4. **`replay: "never" | "safe"` captured at intent time and ANDed with the current tool declaration** at recovery (`tools.ts:527`) — the tool's own claim cannot be widened after the fact.
5. **Streaming frames and tool output as durable progress lists/cells** guarded by "do I still own this effect" (`progress.ts:35-117`) — a crash keeps the partial, a stale writer after takeover is a no-op, and the adaptive publisher bounds write rate (`utils/adaptive-publisher.ts`).
6. **One line = one transaction** JSONL with torn-tail-only repair and fail-closed on complete-but-invalid lines (`jsonl/storage.ts:30-35`, `:99-112`); the header carries a `nextSeq` high-water mark so forks don't collide.
7. **Fork by namespace policy** — an explicit allow/transform/drop per register namespace, with an unknown reserved namespace being an error rather than a silent copy (`session/fork-policy.ts:40-67`).
8. **Snapshot + normative client fold + `"rebase"` sentinel** for navigation (`runtime/reducer.ts:220-221`) — one code path for first paint, reconnect, and resnapshot; `recovery: true` on synthesised events lets the UI label them honestly.

## Top 5 things pi does WRONG or leaves unrecovered

1. **The shipped product still runs v3**: no intent log; leaf pointer in memory only (`session-manager.ts:1374-1379`); prompt lost if a crash precedes the first assistant reply (`:1032-1041`); in-place non-atomic migrations (`:993-1003`); silent skip of corrupt lines (`:299-313`). The durable harness has been "landing" since 2026-08-04 and is still behind `experimental/`.
2. **No fsync anywhere** in either stack, and SQLite dropped `synchronous=FULL` between August and September. "Durable" means "in the page cache" — a power loss, not just a process crash, can lose acknowledged writes.
3. **Dangling tool calls are fixed in the provider adapter, not the log** (`transform-messages.ts:163-229`): file, TUI transcript, and model each see a different conversation — criterion §1 ("two representations of one fact"), and the fix is invisible to any client that does not go through `transformMessages`.
4. **Subagents are unowned child processes** with `--no-session`; nothing links, recovers, names, or even lists them after a crash. The lane model exists but no subagent uses it.
5. **Delivered on one face only** (criterion §9): `replay:"safe"` has zero producers; v4 session listing cannot show names without opening the file; `before_resume` exists in the spec but not in the code; the spec's `OperationStatus "running"` has no producer (noted in-spec as "contract cleanup").

## Not done
- Did not run pi's tests or benchmarks; conclusions are from reading.
- Did not read `experimental/server.ts` routing or `session-worker-manager.ts` replacement logic in depth (only ownership/pid handling).
- Did not diff spec §4.4/§4.5 pseudo-code line-by-line against `runtime/lane.ts` (2012 lines) — anchors are to the implementation; the spec is cited only where the code has no prose.
- Did not verify the August record-journal design beyond noting it was replaced; its `RecordLogCorruption` reasons (`multiple_open_operations`, `record_after_finish`, `tool_call_mismatch`, …) no longer exist at HEAD.
