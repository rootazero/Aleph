# Memory System E2E Validation — Partial Run Report

**Date**: 2026-04-17 (run window 21:07 → 23:41 local)
**Spec**: [docs/superpowers/specs/2026-04-17-memory-e2e-validation-design.md](../specs/2026-04-17-memory-e2e-validation-design.md)
**Plan**: [docs/superpowers/plans/2026-04-17-memory-e2e-validation-plan.md](../plans/2026-04-17-memory-e2e-validation-plan.md)
**Aleph build**: `target/release/aleph-server` (binary mtime 2026-04-17 20:11; source HEAD `ab7f2abff`)
**Outcome**: **Partial pass — terminated at Phase 1** per spec §9 abort condition (codepath blockage)

---

## 1. Executive summary

The validation surfaced **3 real bugs** and **1 high-priority behavioural finding** before completing Phase 1. Phase 0 (boot, auth, gateway smoke test) passed end-to-end after fixing the connect/subscribe/agent.run RPC schemas in the test client. Phases 1–6 could not proceed because the L0 raw-memory store stayed empty after every dialog turn and after an explicit `session.delete` attempt — without raw rows, the entire L1/orientation/retrieval/dream chain has no corpus to operate on.

Helper artifacts (`tools/memory_probe.sh`, `tests/scripts/ws_send.py`, `tests/scripts/memory_e2e_dialog.jsonl`) are committed and reusable for the next attempt.

## 2. Per-phase results

| Phase | Plan task | Result | Evidence |
|---|---|---|---|
| 0 — Pre-flight | Task 0 | ✅ PASS | env ok, backups at `~/.aleph/backups/2026-04-17-pre-validation/` |
| Helper script | Task 1 | ✅ PASS | `tools/memory_probe.sh` committed `e7fec13ac`, dual-review APPROVED |
| Helper scripts | Task 2 | ✅ PASS | `tests/scripts/{memory_e2e_dialog.jsonl,ws_send.py}` committed `a8651acfe`, dual-review APPROVED |
| 0 — Boot + auth + ping | Task 3 | ✅ PASS | server PID 71312 single, ws auth ok, `pong 🏓` round-trip via stream.response_chunk |
| 1 — L0 capture | Task 4 | 🚨 BLOCKED | 0 rows in `raw_memories` after 1 successful dialog turn (`teal` color memory) and a `session.delete` attempt; aborted per spec §8 / §9 |
| 2 — L1 compression | Task 5 | ⏭ NOT RUN | depends on Phase 1 |
| 3 — Retrieval | Task 6 | ⏭ NOT RUN | depends on Phase 2 |
| 4 — Orientation | Task 7 | ⏭ NOT RUN | depends on Phase 1+2 |
| 5 — Query filer | Task 8 | ⏭ NOT RUN | depends on Phase 3 |
| 6a — Dream natural | Task 9 | ⏭ NOT RUN | could in principle run independently; deferred |
| 6b — Dream forced | Task 10 | ⏭ NOT RUN | depends on having a non-empty corpus |

## 3. Hard-pass checklist (spec §7)

| # | Criterion | Status |
|---|---|---|
| H1 | Single Aleph process throughout | ✅ PID 71312 only (after first start failed on Bug #1 and was cleanly restarted) |
| H2 | WS auth via token | ✅ shared_token path returns signed device token + permissions=`["*"]` |
| H3 | Test agent created via natural-language tool call (R9) | ⚠️ partial — created via direct `agents.create` RPC (not via LLM tool selection); the LLM-tool path was not exercised because it would have required dialog through `main`, which itself depended on L0 working |
| H4 | All injected raws transition to is_processed=1 | ❌ no raws ever inserted |
| H5 | ≥4 generated notes pass L1 Format | ❌ no notes generated |
| H6 | ≥3 retrieval tools auto-invoked | ❌ not exercised |
| H7 | USER.md regenerated with all six sections | ❌ not exercised |
| H8 | Forced Dream cycle reached status=success | ❌ not exercised |
| H9 | All four signal types in SignalSnapshot | ❌ not exercised |
| H10 | L1 + L2 validation tiers passed | ❌ not exercised |

## 4. Bugs discovered (high-value byproducts)

### Bug #1 — `init_schema` migration ordering (Medium)

**File**: `src/memory/store/sqlite/schema.rs`
**Symptom**: server fails to start on any database that pre-dates the `recall_signals.fact_id → note_path` rename, with:

```
Error: Failed to initialize memory backend: Configuration/Database error:
Failed to create recall_signals table: no such column: note_path in
CREATE INDEX IF NOT EXISTS idx_recall_note_path
    ON recall_signals(note_path);
```

**Root cause**: `init_schema` (line 366) executes `RECALL_SIGNALS_DDL` — which contains `CREATE INDEX … ON recall_signals(note_path)` — *before* calling `migrate_recall_signals_note_path` (line 413). On a database where `recall_signals` still has the old `fact_id` column, the index creation references a column that does not yet exist.

**Workaround applied during run**: manual `ALTER TABLE recall_signals RENAME COLUMN fact_id TO note_path; DROP INDEX IF EXISTS idx_recall_fact_id; DROP INDEX IF EXISTS idx_recall_dedup;` against the live db, then restart.

**Suggested fix**: move `migrate_recall_signals_note_path(conn)?;` to run *before* `conn.execute_batch(RECALL_SIGNALS_DDL)?;`. Add an integration test that opens a v1-schema fixture and asserts the migration succeeds.

### Bug #2 — `sessions` table missing `state` column on legacy databases (Low–Medium)

**File**: gateway session_store/sqlite_backend (legacy migration path)
**Symptom**: server log emits during boot:

```
Warning: Session migration failed: Configuration/Database error: Prepare failed:
no such column: state in SELECT key, agent_id, session_type, created_at,
last_active_at, message_count, total_tokens, auto_reset_at, state, metadata,
label, input_tokens, output_tokens, model, model_provider, parent_session_key,
compaction_count FROM sessions at offset 108
```

The legacy SQLite session migration to the file backend silently fails — historical sessions are not migrated. New sessions created post-boot work fine (they go to the file backend).

**Suggested fix**: precede the SELECT with a `PRAGMA table_info(sessions)` check, or add a defensive `ALTER TABLE sessions ADD COLUMN state TEXT` migration step ahead of the read.

### Bug #3 — `ws_send.py` initial RPC schemas were misaligned with actual handlers (resolved in-flight)

The plan-as-written assumed the connect/subscribe/agent.run shapes from the spec doc, which did not match the actual handlers:

| RPC | Wrong shape | Correct shape |
|---|---|---|
| `connect` | `params.auth.token` | `params.shared_token` (top-level) for the gateway-token style |
| `events.subscribe` | `params.pattern: "*"` | `params.topics: [str]` |
| `agent.run` | `params.message` | `params.input` |

Also: the streaming loop's `--timeout-seconds` was per-frame; `system.tick` heartbeats arriving every 10s reset the deadline indefinitely. Fixed via `--break-on` (commit `657d1a43d`).

This is not a server bug, but the plan / spec material referenced wrong field names. Recommend updating the doc next to the gateway protocol reference (`docs/reference/GATEWAY.md`) so future validations don't re-discover.

## 5. High-priority behavioural finding — L0 capture path under WS dialog

**Symptom**: across one ping turn (Phase 0) and one Phase-1 turn (with explicit memory-shaped content "My favorite color is teal"), `raw_memories` total stays at 0. An explicit `session.delete` RPC after the turn also did not produce a row. With `compression_enabled=true` and `compression_turn_threshold=20`, no inserts happened either.

**Wiring observed**: `src/bin/aleph-server/commands/start/mod.rs:425` does call `SessionManager::with_raw_memory_writer(memory_db.clone() as Arc<dyn RawMemoryStore>)`. The compactor wiring at `start/builder/agent_init.rs:1149` is also present. The trigger paths (set_state→stopped, SessionCompactor pre-compress hook, TranscriptIndexer per-turn) all exist in source but none fired during the test run.

**Possible explanations** (un-verified in remaining time budget):
1. The session.delete RPC may not transition the session into the `stopped` state that `ops.rs:587` watches — `session.delete` may not even be the canonical RPC name (the plan grepped but did not confirm).
2. The session-compactor's pre-compress hook only fires above a token threshold that 1–2 short turns do not approach.
3. `TranscriptIndexer` is wired to the SessionCompactor but **not** to the per-turn agent loop — it is not a per-turn capture path under the gateway WS flow.
4. There may be a config flag (`memory.capture_enabled`, an extension toggle) not present in the active `~/.aleph/config.toml`.

**Suggested follow-up before re-running validation**:
- Add an integration test: send N dialog turns through the WS RPC and assert `raw_memories` grows.
- Document the canonical entry point that fills `raw_memories` per turn (or per session-end) so the validation plan's Phase 1 has a concrete trigger.
- If Spec 1 G3-A's "session-end digest extraction" is the only WS-path capture, the spec's "raw count grows after each turn" assumption needs revising to "raw count grows on session-end / compaction".

## 6. Side-effects observed

### `main` agent **was** contaminated (spec §8 A2)

Diff of `~/.aleph/memory/note/main/` baseline vs after run:

```diff
+SCHEMA.md
+index.md
+log.md
+query/
```

These four orientation-layer artifacts were auto-bootstrapped against the `main` agent during the Phase 0 ping (because the ping went to `agent:main:dm:operator`, which the server normalised to `agent:main:peer:operator`). The orientation layer fired even though no notes existed, and created the empty scaffolding under `main`.

This is **not destructive** (no existing files modified, no notes overwritten) but it does mean Phase 0 should have used a throwaway agent, not `main`. Update the spec/plan to use `test-memory-validation` for the Phase 0 ping too.

The pre-run backup at `~/.aleph/backups/2026-04-17-pre-validation/note.tgz` retains the original state if the operator wants to roll back the four added scaffolding files.

### Created agents

- `~/.aleph/agents/test-memory-validation/` — created via direct `agents.create` RPC. Standard skeleton (AGENTS.md, HEARTBEAT.md, IDENTITY.md, MEMORY.md, SOUL.md, TOOLS.md, sessions/). Identity description was passed in but IDENTITY.md still shows the template (the RPC may store the description elsewhere). Retain or delete with `agents.delete` per operator preference.

### Database mutations during run

- `recall_signals` schema migrated manually: `fact_id` → `note_path` rename; old `idx_recall_fact_id` and `idx_recall_dedup` indexes dropped to allow the new ones. Server then created the new indexes correctly.
- No notes added/modified for `test-memory-validation`.
- No rows added to `notes_index`, `notes_links`, `recall_signals`, `query_filed`, `dream_reports`, `dream_status`, `daily_insights` for either agent.

### Process state at report-write time

- `aleph-server` PID 71312 is still running with the patched schema and overridden config.
- 0 zombie ws_send.py processes (cleaned up earlier).
- Gateway listening on `ws://127.0.0.1:18790/ws`.

## 7. Commits this session

| SHA | Subject |
|---|---|
| `4bb360d60` | tools: add memory_probe.sh for e2e validation snapshots |
| `e7fec13ac` | tools(memory_probe): validate agent_id, split stderr, safer find/process handling |
| `a8651acfe` | tests: add memory e2e dialog script and WS RPC client |
| `502ccf052` | tests(ws_send): align connect/subscribe/run params with actual gateway schema |
| `657d1a43d` | tests(ws_send): wall-clock timeout + structured break-on markers |

All five commits are isolated to the helper-tool surface and touch only files outside `src/` — they do not modify the Rust server code. The only server-side change applied during the run was the **manual** SQL migration on `recall_signals`, which is uncommitted (it lives in the live database file).

## 8. Probe snapshots retained

```
/tmp/aleph-probes/
  smoke_test_20260417T211040          # Task 1 implementer self-test
  fix_v2_20260417T215444              # Task 1 quality-fix re-test
  specreview_20260417T211322          # Task 1 spec reviewer
  quality_recheck_20260417T215620     # Task 1 quality re-review
  phase0_pre_20260417T222848          # Phase 0 baseline
  phase0_post_20260417T224837         # after Phase 0 ping
  phase_final_20260417T234131         # at report-write time
```

Plus `/tmp/aleph-server.log` (server boot + first ping + diagnostic queries) and `/tmp/phase0_ping_v3.jsonl` (the successful ping JSONL frame stream).

## 9. Recommended next steps

1. **Fix Bug #1 (1-line move + integration test)** — unblocks any operator with an old database and prevents future "freshly-cloned dev box won't start" reports.
2. **Investigate the L0 capture trigger path** — confirm whether per-turn / per-session-end WS conversation should populate `raw_memories`, or whether it is by design that L0 fills only via session compaction at high token counts. If by design, update [MEMORY_SYSTEM.md](../../reference/MEMORY_SYSTEM.md) §3 to be explicit; if it's a wiring gap, fix it.
3. **Fix Bug #2** — small migration shim to add `state` column before reading legacy sessions.
4. **Re-run this validation** once #2 is resolved. Re-use the existing scripts (`tests/scripts/ws_send.py`, `tests/scripts/memory_e2e_dialog.jsonl`) and the probe (`tools/memory_probe.sh`) — no rewrite needed. Estimated re-run cost: ~60 min once L0 is observed populating.
5. **Update the spec** to:
   - point the Phase 0 ping at the test agent (not `main`)
   - replace `--params message` with `--params input` everywhere
   - document the actual `events.subscribe` / `connect` shapes
6. **Optional**: clean up the four scaffolding files added to `~/.aleph/memory/note/main/` if you want to restore the exact pre-run state. The backup tarball under `~/.aleph/backups/` has the originals.

## 10. Cleanup performed by report-writer

- ✅ Backups retained at `~/.aleph/backups/2026-04-17-pre-validation/` (delete after 7 days per spec §10)
- ✅ All zombie test processes killed
- ⚠️ `aleph-server` PID 71312 left **running** with applied schema patch and the temporary config overrides from earlier phase attempts; stop with `pkill -f "target/release/aleph-server"` when you want a clean state. Or restart fresh once Bug #1 is fixed in code.
- ⚠️ Test agent `test-memory-validation` retained for re-run; delete via the `agents.delete` RPC if you'd rather start clean next time.
