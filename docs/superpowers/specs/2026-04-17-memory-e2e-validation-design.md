# Memory System E2E Validation Design

**Date**: 2026-04-17
**Author**: Operator-driven (Claude Opus + user)
**Scope**: End-to-end production-grade validation of Aleph memory system on an isolated test agent, executed via webchat over Gateway WebSocket.
**Status**: Design — pending approval

---

## 1. Goal

Validate the full Aleph memory stack end-to-end against a clean, isolated test agent: L0 raw capture → L1 realtime compression → hybrid retrieval (LLM-picked + vector + BM25) → orientation layer (`SCHEMA.md`/`index.md`/`log.md`/`USER.md`) → query filed-back → Dream Daemon strategy-driven evolution (Signal → Strategy → Gate → Pipeline → Validation → EventLog).

This validation specifically exercises the recently merged Dream Daemon evolution refactor (commits `0c66840c5`..`ab7f2abff`) which replaced the hardcoded daily/weekly pipeline with three adaptive strategies: `Consolidate`, `Synthesize`, `Conserve`.

## 2. Non-goals

- Validating non-memory subsystems (Tool execution, Provider routing, Plugin lifecycle).
- Performance benchmarking (no latency/throughput targets in this run).
- Validating other gateway interfaces (Telegram, Discord, MS Teams, Feishu, WeChat).
- Modifying the real `main` agent's notes.

## 3. Architectural Surfaces Under Test

| Layer | Module | Probe Surface |
|---|---|---|
| L0 raw capture | `src/memory/store/raw_memory.rs` | `raw_memories` table (`is_processed`, `session_id`, `created_at`) |
| L1 realtime compression | `src/memory/notes/` + `CompressionService` | `~/.aleph/memory/note/{agent}/{category}/*.md`, `notes_index`, `notes_links`, `notes_fts` |
| Retrieval (hybrid) | `src/memory/retrieval/` (NoteFactRetrieval) | `recall_signals` table, WS tool events |
| Orientation layer | `src/memory/notes/orientation/` (Spec 5) | `SCHEMA.md`, `index.md`, `log.md` per agent |
| User profile | `src/memory/notes/profile/` (Spec 7) | `USER.md` per agent, `<UserProfile>` prompt envelope |
| Query filer | `src/memory/notes/query_filer/` (Spec 8) | `query_filed` table, `note/{agent}/query/*.md` |
| Dream — signal | `src/memory/dreaming/signals.rs` | `SignalSnapshot` with 4 types: Quality, Recall, Health, SkillUsage |
| Dream — strategy | `src/memory/dreaming/strategy.rs` + `selector.rs` | `DreamStrategy::{Consolidate, Synthesize, Conserve}`, `SelectionDecision.personality_adjustment` |
| Dream — gate | `src/memory/dreaming/mutation_gate.rs` | `GateDecision::{Allow, Conserve{cooldown}, Skip}` (merge cycle / oscillation / wasted distill) |
| Dream — validation | `src/memory/dreaming/validation.rs` | `DreamValidationReport` (L1 Format / L2 Consistency / L3 Semantic / L4 Retrospective) |
| Dream — event log | `src/memory/dreaming/event_log.rs` | `{agent_dir}/dream_events.jsonl` (one `DreamEvent` per cycle) |
| Dream — schedule | `src/memory/dreaming/mod.rs` | `dream_status` (singleton), `dream_reports` table, `daily_insights` |
| Gateway transport | `src/gateway/` | WebSocket `ws://127.0.0.1:18790/ws`, JSON-RPC, event topics |

## 4. Test Subject

- **Agent ID**: `test-memory-validation` (created via tool call from within the dialog — exercises R9 "Everything is a Tool")
- **Session key**: `agent:test-memory-validation:dm:operator`
- **Token**: `aleph-9976129a-407d-4893-a96c-6467b24bedac` (Gateway operator token)
- **Agent personality (system prompt)**: "I am a sandbox agent for memory-system validation. Be deliberate about what you remember, retrieve, and synthesize. Prefer using available memory tools to demonstrate them."

## 5. Probe Inventory

A single helper script `tools/memory_probe.sh` (created in Phase 0) dumps every probe to a timestamped file. Each phase runs it before and after, and we diff.

```text
~/.aleph/data/memory.db
  ├── raw_memories         (count by is_processed × session_id)
  ├── notes_index          (rows for agent='test-memory-validation')
  ├── notes_links          (outgoing edges)
  ├── notes_fts            (sample FTS terms)
  ├── recall_signals       (rows by note_path × day_bucket × channel)
  ├── query_filed          (rows by sha256(query))
  ├── dream_status         (last_run_at, last_status, last_duration_ms)
  ├── dream_reports        (latest 5 rows)
  └── daily_insights       (today's row, if any)

~/.aleph/memory/note/test-memory-validation/
  ├── SCHEMA.md, index.md, log.md, USER.md
  ├── {personal,technical,project,query,synthesis,skill,...}/*.md
  ├── archive/{category}/*.md     (NoteDecay output)
  └── dream_events.jsonl          (EventLog audit trail)
```

WebSocket subscriptions:
- `stream.*` — model output, tool start/end, agent traces
- `agent.*` — lifecycle (started, completed, error)
- `session.*` — compaction events
- `event` — generic JSON-RPC events

Tracing log filters (server stdout):
- `compression.run.{started,completed,error}`
- `dream.check.{ran,skipped}` with reason
- `dream.strategy.selected`
- `dream.gate.{allow,conserve,skip}`
- `dream.validation.tier_{1,2,3,4}.{passed,failed}`
- `dream.event_log.appended`
- `query_filer.{filed,deduped,gate_blocked}`
- `profile.synthesizer.{started,completed}`

## 6. Phase Plan

### Phase 0 — Boot & Authenticate (5 min)

1. `pkill -f "target/release/aleph-server"; pkill -f "target/debug/aleph-server"; sleep 2`
2. Verify zero residual processes (`ps aux | grep "[a]leph-server"`)
3. Backup defenses:
   - `cp ~/.aleph/data/memory.db ~/.aleph/data/memory.db.bak.$(date +%s)`
   - `tar -czf ~/.aleph/memory.note.bak.$(date +%s).tgz ~/.aleph/memory/note/`
4. `target/release/aleph-server start &` (background)
5. Wait ≤5s, then `wscat -c ws://127.0.0.1:18790/ws`:
   - First frame: `{"jsonrpc":"2.0","id":"c1","method":"connect","params":{"minProtocol":1,"maxProtocol":1,"client":{"id":"e2e-validator","version":"1.0.0","platform":"darwin"},"role":"operator","auth":{"token":"aleph-9976129a-407d-4893-a96c-6467b24bedac"}}}`
   - Second frame: `events.subscribe` with pattern `*` (capture-everything baseline)
6. Smoke test: `agent.run` to existing `agent:main:main` with one ping; confirm `stream.chunk` and `agent.completed`. Cancel.

**Pass**: single process, `connect` succeeds, ping round-trips with stream events.

### Phase 1 — Create Test Agent + L0 Capture (10 min)

1. From the WS shell (still on `main` agent), send a natural-language request: "Create a new agent called `test-memory-validation` with the system prompt: 'I am a sandbox agent for memory-system validation...' Then switch us into a DM session with that agent."
2. The LLM should call the agent-create tool. Verify via:
   - New agent config persisted under `~/.aleph/` (exact path discovered in Phase 0 by inspecting where `main` lives)
   - Subsequent `agent.run` calls accept `session_key=agent:test-memory-validation:dm:operator`
3. Inject 8 dialog turns covering 4 categories:
   - **personal** (2): name, location, current life context
   - **technical** (2): coding style preferences, tooling choices
   - **project** (2): what project the user is on, deadlines
   - **constraint** (2): hard rules (e.g., "never commit on Fridays")
4. After every turn, dump `raw_memories WHERE session_id LIKE '%test-memory-validation%'` and confirm `is_processed=0` accumulating.

**Pass**: agent created via tool call, 8 raw rows present, all `is_processed=0` (compression not yet triggered).

### Phase 2 — L1 Realtime Compression (10 min)

1. Pre-condition: `compression_turn_threshold = 20` by default. Apply temporary override via `config.patch` to `compression_turn_threshold = 8` to trigger sooner without 20 turns.
2. Continue dialog 1–2 more turns to cross the new threshold.
3. Wait for `compression.run.started` log line; then `compression.run.completed`.
4. Probe diff:
   - `~/.aleph/memory/note/test-memory-validation/` now has `{category}/*.md` files
   - `notes_index` rows for the agent grew
   - `raw_memories.is_processed` flipped to `1` for the consumed rows
5. Open one generated note manually and verify:
   - YAML frontmatter has the four required keys: `category`, `tags`, `created`, `updated` (per `NoteLint.ensure_frontmatter`)
   - Body is non-empty markdown (concrete section layout is set by the active compression prompt — verified live, not pre-asserted)
   - Wikilinks `[[other-note]]` resolve in `notes_links`

**Pass**: ≥4 notes generated across ≥3 categories, frontmatter complete, `notes_links` populated, all original raws marked processed.

### Phase 3 — Retrieval Full-Stack (15 min)

Trigger four retrieval tools indirectly through dialog (let the LLM pick):

| Prompt | Expected tool | Probe |
|---|---|---|
| "What city did I tell you I live in?" | `memory_search` | `recall_signals` row, `tool_start`/`tool_end` events with score ≥ similarity_threshold |
| "List everything you remember in the `personal` category." | `memory_browse` | tool event with category param, returned filenames match `note/test-memory-validation/personal/` |
| "Starting from my project, expand to related notes 2 hops out." | `memory_explore` | tool event with seed + depth params; result includes wikilink-traversed notes |
| "Replay the original wording of the third thing I told you in this session." | `recall_context` | tool event with session_key; returns raw text from `raw_memories` |

After all four pass, run an A/B on AI-picked vs vector-only:

5. `config.patch` → `memory.ai_retrieval_enabled = false`
6. Repeat the same four prompts, capture results
7. `config.patch` → `memory.ai_retrieval_enabled = true`
8. Compare hit counts and record both in the run report

**Pass**: all 4 tools fired by LLM autonomy, hit-at-3 ≥ 80% on factual recall prompts, `recall_signals` populated.

### Phase 4 — Orientation Layer (10 min)

1. Verify `SCHEMA.md`, `index.md`, `log.md` exist for the test agent (created on first compression run if not earlier).
2. Dialog: "Show me your current schema and the most recent activity log."
   - LLM should call `note_orient` (Tools mode) or read the prompt-injected versions (Context mode)
3. Dialog: "Add a new category called `experiments` to your schema."
   - LLM should call `note_schema` with the current content hash
   - Manually retry the same call with a stale hash → expect rejection (optimistic concurrency)
4. End the session via `agent.cancel` or session timeout to trigger `SessionEnd` raw
5. Wait for `profile.synthesizer.completed` log
6. Inspect `USER.md`:
   - `<UserProfile>` envelope present (or six section headings: Identity, Communication Style, Motivations, Current Focus, Stance Shifts, Open Questions)
   - Content reflects what was injected in Phase 1 (name, location, preferences, constraints)
7. Start a fresh session with the same agent; first model turn's prompt must include the `<UserProfile>` envelope (verified via prompt log if available, else via behavioral test: "What do you already know about me?")

**Pass**: SCHEMA.md mutation works with hash check, USER.md regenerated with all six sections populated, profile injected into next session.

### Phase 5 — Query Filed-Back (5 min)

1. Dialog: "Synthesize the common theme across everything you've remembered about me so far."
   - LLM should call `memory_reflect` (which has the QueryFiler hook)
2. Watch for `query_filer.filed` log line (cheap gate passes: ≥3 sources + ≥200 chars; LLM gate passes: novel synthesis)
3. Probe `query_filed` table → new row keyed by `sha256(query)`
4. Probe `note/test-memory-validation/query/*.md` → new file
5. Re-issue the identical query → expect `query_filer.deduped` log line, no new file

**Pass**: one query filed, identical re-issue dedupes, file content references source notes via wikilinks.

### Phase 6a — Dream Daemon Natural Cadence (5 min observation)

1. Do not modify config. Watch tracing logs:
   - `dream.check.skipped` should fire every ≤60s with reasons: `outside_window` (likely, unless it's 02:00–05:00) and/or `idle_below_threshold` (likely, since we're actively prompting)
2. Confirm `dream_status` row remains untouched (no `last_run_at` write)
3. Confirm no `dream_reports` insert
4. Confirm `dream_events.jsonl` not created or not appended

**Pass**: daemon ticks correctly, all ticks short-circuit with explicit reasons, no state mutation.

### Phase 6b — Dream Daemon Forced Full Cycle (15 min)

This is the critical validation of the recently merged evolution refactor.

1. Apply config override:
   ```toml
   [memory.dreaming]
   idle_threshold_seconds = 30
   window_start_local = "00:00"
   window_end_local    = "23:59"
   weekly_enabled      = true
   weekly_interval_days = 0
   ```
2. Stop sending dialog turns; idle for ≥35s
3. Watch for `dream.check.ran` within 60s
4. Capture full event chain (in this order):
   1. `dream.signal.collected` — `SignalSnapshot` with non-empty `signals` (4 types represented)
   2. `dream.strategy.selected` — log includes chosen strategy name + `personality_adjustment` value
   3. `dream.gate.{allow|conserve|skip}` — first cycle should be `Allow` (no merge history yet)
   4. `dream.pipeline.started` — strategy's `stage_names()` list
   5. Each stage: `dream.stage.{name}.{started|completed}`
   6. `dream.validation.tier_1.passed` (L1 Format)
   7. `dream.validation.tier_2.passed` (L2 Consistency)
   8. `dream.validation.tier_3` — only if Synthesize strategy ran
   9. `dream.validation.tier_4` — may be `not_run` if no prior cycle
   10. `dream.event_log.appended` — `dream_events.jsonl` grew by 1 line
5. Probe outputs:
   - `dream_reports` new row (`pipeline_type`, `duration_ms`, `synthesis_count`, `errors=null`)
   - `dream_status.last_status = success`
   - `dream_events.jsonl` last line parses as valid `DreamEvent` JSON with all fields populated
   - File system diff:
     - `.md.bak` files (from any merge attempt)
     - `archive/{category}/*.md` (from NoteDecay if scores < threshold)
     - Modified `.md` files with updated `## Superseded` or YAML `stale: true` markers (from NoteDrift)
   - `daily_insights` row for today (if DailyDigest ran)
6. Trigger a second cycle (idle 35s again) to verify:
   - `MutationGate.advance_cycle()` was called
   - If first cycle merged any pair, second cycle's `current_merges` starts empty
   - StrategySelector's `history` window updated (visible via second cycle's selection rationale)

**Pass**: complete pipeline ran with `status=success`, all 4 signal types present in snapshot, validation L1+L2 passed, EventLog has audit row, second cycle advances state correctly.

### Phase 7 — Restore & Report (5 min)

1. Revert config overrides via `config.patch` (back to defaults)
2. Wait one full daemon tick to confirm `dream.check.skipped` resumes
3. Generate run report at `docs/superpowers/runs/2026-04-17-memory-e2e-report.md`:
   - Per-phase pass/fail with probe diffs
   - SQL query results (raw_memories counts, recall_signals samples, dream_reports row)
   - Dream EventLog last 3 events (formatted JSON)
   - Anomalies and follow-ups
4. Backup files (`memory.db.bak.*`, `memory.note.bak.*.tgz`) retained for 7 days then pruned

## 7. Success Criteria

### Hard pass (any failure aborts the run)

| # | Criterion |
|---|---|
| H1 | Single Aleph process throughout; no `.shared_token` write contention |
| H2 | WS auth via token succeeds; `events.subscribe *` delivers events |
| H3 | Test agent created via natural-language tool call (R9 satisfied) |
| H4 | All injected raws transition to `is_processed=1` after compression |
| H5 | ≥4 generated notes pass L1 Format validation |
| H6 | ≥3 of 4 retrieval tools invoked autonomously by the LLM, all return relevant results |
| H7 | `USER.md` regenerated with all six sections after `SessionEnd` |
| H8 | Forced Dream cycle reaches `status=success` with `DreamEvent` appended to `dream_events.jsonl` |
| H9 | All four signal types present in the `SignalSnapshot` (Quality, Recall, Health, SkillUsage) |
| H10 | L1 + L2 validation tiers passed |

### Soft pass (recorded as observations, not blockers)

| # | Criterion |
|---|---|
| S1 | LLM-picked retrieval shows ≥10% precision lift over vector-only on the same prompts |
| S2 | `MutationGate` exercises a non-Allow decision (would require constructing oscillation; likely not in this run) |
| S3 | `StrategySelector` chooses a non-Consolidate strategy (would require Quality/SkillUsage signals strong enough) |
| S4 | Query filer deduplicates a re-issued query |
| S5 | NoteDecay archives at least one note (only if scores fall below threshold; corpus may be too fresh) |

## 8. Abort Conditions

Stop immediately, snapshot state, do not auto-recover:

| # | Trigger |
|---|---|
| A1 | More than one `aleph-server` process detected at any point |
| A2 | `~/.aleph/memory/note/main/` modification timestamp changes (real agent contamination) |
| A3 | Same Dream stage logged as starting ≥3 times within one cycle |
| A4 | WS connection drops ≥5 times in 10 minutes |
| A5 | `dream_reports.errors` non-null after a forced cycle |
| A6 | Validation L1 or L2 tier fails (means newly generated notes are malformed) |

On abort, the run report records the abort condition, the last successful phase, and the on-disk state at abort time.

## 9. Time Budget

| Phase | Duration |
|---|---|
| 0 — Boot & Auth | 5 min |
| 1 — Create agent + L0 | 10 min |
| 2 — L1 Compression | 10 min |
| 3 — Retrieval | 15 min |
| 4 — Orientation | 10 min |
| 5 — Query Filer | 5 min |
| 6a — Dream natural | 5 min |
| 6b — Dream forced | 15 min |
| 7 — Restore & Report | 5 min |
| **Total** | **~80 min** active operation |

If unexpected behavior requires a code fix and rebuild, the run is aborted and reported as `phase_X_blocked_on_codepath`.

## 10. Deliverables

| File | Purpose |
|---|---|
| `docs/superpowers/specs/2026-04-17-memory-e2e-validation-design.md` | This design (immutable once approved) |
| `docs/superpowers/plans/2026-04-17-memory-e2e-validation-plan.md` | Execution plan generated by writing-plans skill |
| `tests/scripts/memory_e2e_dialog.jsonl` | Replayable dialog script (one prompt per line, JSONL) |
| `tools/memory_probe.sh` | Bash helper that dumps every probe to a timestamped file |
| `docs/superpowers/runs/2026-04-17-memory-e2e-report.md` | Post-run report with probe diffs and findings |

## 11. Out of scope (explicit)

- Modifying the real `main` agent's notes
- Validating WeChat / Telegram / MS Teams gateway flows
- Performance / latency / throughput measurement
- Adversarial input testing (injection, oversize messages, malformed JSON-RPC)
- Multi-agent or sub-agent delegation paths

## 12. Open questions discovered during writing

None pending; all design choices have been confirmed with the operator (test mode = A+B mixed; baseline = isolated test agent).

---

**Approval gate**: Operator review required before proceeding to writing-plans.
