# src/memory (assembler/compression/context/context_comptroller/curated/dreaming/events/extensions/flush) — occams-r5 Review

## Summary
- 19 findings: 1 critical / 5 high / 8 medium / 5 low
- Files reviewed:
  - src/memory/assembler/ (13 files, ~2 970 lines): mod.rs, envelope.rs, error.rs, fallback.rs, feedback_floor.rs, gather.rs, hybrid.rs, hydration.rs, profile.rs, render.rs, rerank.rs, tests.rs, context_block.rs
  - src/memory/compression/ (5 files, ~1 816 lines): mod.rs, scheduler.rs, service.rs, source_prompts.rs, source_prompts/
  - src/memory/context/ (7 files, ~1 148 lines): mod.rs, compression.rs, enums.rs, fact.rs, paths.rs, tests/
  - src/memory/context_comptroller/ (4 files, ~196 lines): mod.rs, comptroller.rs, config.rs, types.rs
  - src/memory/curated/ (7 files, ~1 594 lines): mod.rs, budget.rs, format.rs, legacy.rs, snapshot.rs, store.rs, tests.rs
  - src/memory/dreaming/ (15 + 19 stage files, ~22 232 lines including stages)
  - src/memory/events/ (6 files, ~5 137 lines): mod.rs, commands.rs, handler.rs, projector.rs, testing.rs, traveler.rs
  - src/memory/extensions/ (9 files, ~2 745 lines): mod.rs, first_party.rs, insert_helper.rs, manifest.rs, mcp_adapter.rs, registry.rs, scheduler.rs, traits.rs, types.rs
  - src/memory/flush/ (2 files, ~548 lines): mod.rs, registry.rs

---

## Findings

### [CRITICAL] Test file references removed fields — `cargo test --lib` fails to compile
- **Location**: src/memory/integration_tests/mod.rs:20-22, 25-26, 50-52
- **Category**: dead-code / regression / build-broken
- **Description**: The integration test file (compiled under `#[cfg(test)]`) constructs `ComptrollerConfig { similarity_threshold: 0.95, token_budget: 1000, fold_threshold: 0.2 }` and reads `config.similarity_threshold` / `config.token_budget` / `config.fold_threshold` — but commit `7bf481dba` (occams-r5 round-close) emptied `ComptrollerConfig` into a `pub struct ComptrollerConfig {}` precisely by deleting those three fields. `cargo check --lib --tests` fails with `E0560: struct ... has no field named 'similarity_threshold' / 'token_budget' / 'fold_threshold'` (×9 errors). The sibling `RippleConfig` test (line 26-32) still works because `RippleConfig` retained its fields. This is a regression from the same review round that introduced the deletion — the test file was missed by the round-close sweep.
- **Evidence**: `rg -n "similarity_threshold|token_budget|fold_threshold" src/memory/integration_tests/mod.rs` returns 9 hits on the missing fields; `cargo check -p alephcore --lib --tests` reproduces all 9 errors (plus 2 unrelated `hub/official_skills.rs` errors). `ComptrollerConfig` is declared `pub struct ComptrollerConfig {}` in `src/memory/context_comptroller/config.rs:13` with only `Default`.
- **Suggested fix**: Either (a) delete the integration test bodies — they assert only that the structs can be constructed, which the test itself proves — or (b) drop the three field accesses and assert `ComptrollerConfig::default()` equals `ComptrollerConfig::default()` (a smoke test only). The test predates the field deletion and adds no value past what `tests/memory_modes_integration.rs` and `tests/memory_reflect_integration.rs` already exercise end-to-end.
- **Risk**: none — the test is already non-compiling, so deleting or stubbing it cannot regress anything.

### [HIGH] `dreaming/mod.rs` is 3464 lines; `run_dream` is a 419-line function
- **Location**: src/memory/dreaming/mod.rs:1-3464 (file), :1348-1767 (`run_dream`)
- **Category**: oversize / control-flow
- **Description**: `src/memory/dreaming/mod.rs` carries the daemon wiring (`ensure_dream_daemon`, `daemon_status`, `DREAM_DAEMON` slot), the `DreamDaemon` struct, the `DreamPipeline` stage composer, `DreamContext`, `DreamReport`, `DreamStatus`, `MutationGate`, `StrategySelector`, `SignalSnapshot`, `RawMetrics`, `compute_raw_metrics`, `l1_over_corpus`, `feedback_rules_landed`, `should_skip_scheduled_run`, `last_activity_timestamp`, `idle_seconds`, `record_activity`, `try_run_now`, `run_dream`, `check_and_run`, `run_now`, `parse_window`, and 1300+ lines of tests. `run_dream` alone is 419 lines spanning seven phases (rehydrate history → compute metrics → gate → select → build context → run pipeline + per-corpus fan-out → validate → evolution gate → solidify event log). The function's narrative has been kept coherent only by very long comments; the actual logic is one continuous `if let Some(provider) / Some(embedder) { ... } else { ... }` block whose control flow lives entirely inside that one arm.
- **Evidence**: `awk '/^    async fn run_dream\(/,/^    }$/' src/memory/dreaming/mod.rs | wc -l` → 419; file size 3464 lines (the 700-line ceiling is exceeded by ~5×).
- **Suggested fix**: Split the file into the responsibilities it conflated — a top-level `mod.rs` with `pub use` re-exports, plus submodules `daemon.rs` (wiring + lifecycle), `dream_cycle.rs` (`run_dream` broken into the 7 phases, each a named helper), `gate.rs` (MutationGate + StrategySelector already exist separately, but `compute_raw_metrics` and `should_skip_scheduled_run` should move), and `event_log.rs` for the JSONL event reader. The fan-out per-corpus logic should sit in `project_cycle.rs` (it already lives there as `run_namespace_cycle`) and `run_dream` should call it for the base agent too — see CRIT 4 below.
- **Risk**: medium — the file has tests in `#[cfg(test)] mod tests` that reach into private items (`DREAM_DAEMON`, `should_skip_scheduled_run`, `DreamStatus`, `feedback_rules_landed`); refactoring needs to keep those paths visible to the test module (move them to `pub(crate)` where required).

### [HIGH] `run_dream` and `run_namespace_cycle` re-implement the same 7-phase cycle
- **Location**: src/memory/dreaming/mod.rs:1348-1767 (`run_dream`) vs src/memory/dreaming/project_cycle.rs:175-456 (`run_namespace_cycle`)
- **Category**: duplication
- **Description**: Both functions walk the same 7 phases — (1) read event-log history, (2) compute raw metrics from the note index + recall signals, (3) evaluate the MutationGate, (4) select strategy, (5) build DreamContext and run the pipeline, (6) build the L1/L2 validation report, (7) evaluate the evolution gate, (8) persist decision + append event log entry. Phases 5–8 are largely byte-identical: the same `l2_pairs` collection, the same `validation_report` construction, the same evolution-gate evaluation with `get_best_health` / `set_best_health`, the same `CycleDecision` assembly, the same `report.is_vacuous_interruption` skip, the same `DreamEvent` struct construction. Differences are confined to (a) the agent_id source (`DEFAULT_AGENT_ID` vs the per-corpus arg), (b) the `retain_project_stages()` call in the namespace path, and (c) the Conserve-only distillation gate that the namespace path adds. The drift risk is real and already partially realized: the namespace path has its own local copy of the `is_vacuous_interruption` skip, and the two paths have historically diverged on best-health handling (the base cycle keeps an in-process `Mutex<f64>`, the namespace path round-trips through the DB every cycle).
- **Evidence**: side-by-side phase markers at `dreaming/mod.rs:1391-1393` (read history), `:1417-1419` (compute metrics), `:1439-1442` (gate), `:1446-1449` (select), `:1461-1612` (run pipeline + fan-out), `:1622-1641` (validate), `:1650-1696` (evolution gate), `:1707-1744` (solidify event log), versus `project_cycle.rs:188-216` (read history), `:246-248` (compute metrics), `:268-269` (gate), `:272-275` (select), `:281-318` (run pipeline), `:328-345` (validate), `:347-403` (evolution gate), `:406-431` (solidify). 281 vs 419 lines; >70% of the bodies are character-for-character the same.
- **Suggested fix**: Extract the cycle body into a single `pub(super) async fn run_one_cycle(deps, agent_id, strategy, retain_project) -> Result<DreamCycleOutcome, AlephError>`. Both `run_dream` (base) and `run_namespace_cycle` (per-corpus) become thin wrappers that compute their agent-specific inputs (history fetch, index fetch, gate stage, distill gates) and delegate the rest. The base-cycle fan-out becomes a separate `pub(super) async fn fan_out_corpora(deps, base) -> ...` that calls `run_one_cycle` per corpus.
- **Risk**: medium — `run_dream` keeps the cycle's `Arc<DreamDaemon>` borrow which `run_one_cycle` cannot hold; the extraction needs to pass an `Arc` of pre-cloned collaborators instead.

### [HIGH] Six `MemoryCommandHandler` write methods re-implement the same 6-step structure
- **Location**: src/memory/events/handler.rs:248 (create_fact), :291 (update_content), :337 (invalidate_fact), :374 (restore_fact), :408 (record_access), :459 (consolidate_facts), :512 (delete_fact)
- **Category**: duplication
- **Description**: All seven write methods follow the same shape: (1) compute `partition` from `cmd.agent` (or `cmd.note_path` lookup, or source-partition BTreeSet for consolidate), (2) compute `seq = latest_seq + 1`, (3) build a `MemoryEvent` variant, (4) wrap it in a `MemoryEventEnvelope`, (5) `self.append(envelope, partition)`, (6) `self.project_to_notes(fact_id)`, with the exact same `tracing::error!` on dual-write failure. The 9-line `tracing::error!` block (with the four-line message "Notes dual-write failed; event log is persisted but the notes filesystem is now divergent. A future background reconciler must scan memory_events vs the notes/ directory and replay divergent events. Until then, the note file is stale.") is copy-pasted in all six sites with no variation. The non-trivial per-method work is the event-variant construction (different fields per command), which is the only place where the methods should differ.
- **Evidence**: side-by-side reading of lines 248-289, 291-335, 337-372, 374-406, 408-457, 459-510, 512-543; each method body opens with `let partition = ...` and closes with `if let Err(e) = self.project_to_notes(...) { tracing::error!(fact_id = ..., error = ..., "Notes dual-write failed; ..."); }`. `rg "Notes dual-write failed"` returns 7 hits in handler.rs.
- **Suggested fix**: Extract a `async fn write_event(&self, partition: Option<String>, envelope: MemoryEventEnvelope) -> Result<(), AlephError>` that handles seq + append + project_to_notes + error logging. Each public method shrinks to: build `MemoryEvent` variant → `self.write_event(partition, MemoryEventEnvelope::new(...))`.
- **Risk**: low — the public signatures stay identical, the per-method logic stays identical, only the seq/append/project body is shared.

### [HIGH] `parse_distill_response` is duplicated byte-for-byte across two stages
- **Location**: src/memory/dreaming/stages/feedback_distill.rs:573-588 vs src/memory/dreaming/stages/skill_distill.rs:302-321
- **Category**: duplication
- **Description**: Both files define `pub fn parse_distill_response(text: &str) -> Vec<DistillAction>` with the same body — find first `{`, rfind `}`, the `if end <= start` panic guard, the `serde_json::from_str::<DistillResponse>(json_str)` parse, returning `Vec::new()` on any failure. `DistillResponse` is a private `#[derive(serde::Deserialize)] struct { actions: Vec<DistillAction> }` defined in skill_distill.rs; `feedback_distill.rs` shadows it with its own private copy. Both are reached only by tests in their own module.
- **Evidence**: diff the bodies line-by-line — identical modulo whitespace. `rg "pub fn parse_distill_response"` returns both hits. The `DistillResponse` struct is also duplicated.
- **Suggested fix**: Move both `pub fn parse_distill_response` and the `DistillResponse` struct to `stages/mod.rs` (or a new `stages/parse.rs` helper module) and have both stages call the shared helper. The shared helper would also be the right home for a future `pub fn build_distill_prompt(...)` if the prompt builders ever converge.
- **Risk**: none — both copies return identical output for identical input.

### [HIGH] Oversize files in `dreaming/stages/` (six files over 700 lines)
- **Location**: src/memory/dreaming/stages/feedback_distill.rs:1237, note_weave.rs:1156, note_decay.rs:1077, tool_failure_distill.rs:927, note_lint.rs:805, note_consolidate.rs:737
- **Category**: oversize
- **Description**: Six stage files exceed the 700-line ceiling; five of them are dominated by a single `execute` function (followed by their tests). `feedback_distill.rs::execute` is 200+ lines and includes an inline LLM call, a watermark read, a quorum check, a candidate-fetch, a prompt build, the action loop with skill_gate / recall-evidence gate / evolution-budget gate, and the apply call. `note_decay.rs::execute` walks the entire note index in one pass with phase markers (protection-rule 0/1/2/3, scoring, archive threshold, archive move, C2.7 confidence pass). `tool_failure_distill.rs::execute` mirrors feedback_distill's shape but for a different RawMemorySource variant. Three of the six stage files (`feedback_distill`, `tool_failure_distill`, `skill_distill`) share near-identical control flow — fetch → quota → prompt → LLM → apply loop with shared gates — and a real refactor would extract a `pub(super) async fn distill_skill_like(...)` shared helper.
- **Evidence**: `wc -l` on the six files (see summary); `rg "    pub async fn execute\("` shows one `execute` per stage file.
- **Suggested fix**: For `feedback_distill` / `tool_failure_distill` / `skill_distill`, extract a single `distill_skill_like` driver into `stages/mod.rs` taking `&dyn DistillSource` (an internal trait with `watermark_key: &str`, `read_inputs(agent)`, `build_prompt(...)`) and the existing gate/apply plumbing. For `note_decay`, split `execute` into `score_notes` and `apply_archive_moves`. For `note_lint`, split into `phase1_frontmatter` and `phase2_links`.
- **Risk**: medium — the inline LLM-call setup and the post-action DistillActionRecord bookkeeping is identical but uses stage-specific strings ("feedback_distill" / "skill_distill" / "tool_failure_distill") that must not be conflated; a `DistillSource` trait parameterizes the strings.

### [HIGH] `events/handler.rs` is 2043 lines with one chunk (`reconcile_once`) at 130 lines
- **Location**: src/memory/events/handler.rs:1-2043, reconcile_once at :654-782
- **Category**: oversize / control-flow
- **Description**: The handler file owns the public mutation API (create_fact / update_content / invalidate_fact / restore_fact / record_access / consolidate_facts / delete_fact — see CRIT 4), the audit-trail helpers (log_note_created / log_note_updated / log_note_deleted — three more entries in the same shape), the projection helpers (project_to_notes at :88, append at :73, append_note_event at :627), the reconciler (reconcile_once at :654, spawn_reconciler_daemon at :823), the runtime sentinel types (ReconcileReport at :876, DivergentFact at :894), and a 700-line test module. `reconcile_once` is 130 lines that fold every distinct fact_id, project events, sanitize titles, build the missing/stale divergent-fact lists, then publish to the `last_reconcile` slot — the entire filesystem scan and reporting logic in one function.
- **Evidence**: `wc -l src/memory/events/handler.rs` → 2043; `awk '/^    pub async fn reconcile_once\(/,/^    }$/' src/memory/events/handler.rs | wc -l` → 130.
- **Suggested fix**: Move the reconciler into `src/memory/events/reconciler.rs`: types `ReconcileReport` / `DivergentFact` plus `MemoryCommandHandler::reconcile_once` and `spawn_reconciler_daemon`. Keep the handler file focused on the write API + projection helpers. The reconciler types are already standalone — only `MemoryCommandHandler`'s two methods need to be moved.
- **Risk**: low — the reconciler is `pub`-exposed at the handler level but only via re-exports; `mod.rs:81` already re-exports `DivergentFact` / `ReconcileReport`, so moving the impl re-uses the same surface.

### [MEDIUM] Dead `CompressionService::new_with_backend` and `compress_default_notes`
- **Location**: src/memory/compression/service.rs:119-127 (`new_with_backend`), :239-244 (`compress_default_notes`)
- **Category**: dead-code
- **Description**: `new_with_backend` is `pub` and takes an extra `_memory_backend: Option<MemoryBackend>` parameter that the body never uses (the field is unused — the `Self::database` is the only backend stored). Its only caller is the file-local forwarder `Self::new(...)` at line 115. `compress_default_notes` is a private `async fn` that is just `self.compress_to_notes(workspace_id).await`; its comment explicitly says "this layer no longer constructs one". Both functions are unreachable through the public API and dead through the forwarder.
- **Evidence**: `rg "new_with_backend\("` returns only the forwarder and its own definition (no external callers); `rg "compress_default_notes"` returns only the definition (no callers).
- **Suggested fix**: Delete `new_with_backend` (and inline its body into `new`); delete `compress_default_notes` and have the one caller at line 215 (`compress`) call `compress_to_notes` directly.
- **Risk**: none — the two functions have no callers outside their own file.

### [MEDIUM] `MemoryExtensionScheduler::with_tick_duration` is never called
- **Location**: src/memory/extensions/scheduler.rs:36-39
- **Category**: dead-code
- **Description**: `with_tick_duration` is a `pub const fn` builder setter. It exists, but no caller uses it — `MemoryProducerScheduler::new` already takes a `Duration` would require taking one; today `tick_duration` is set from `DEFAULT_TICK_SECONDS` and the builder does not override it.
- **Evidence**: `rg "with_tick_duration\("` returns only the definition; `MemoryProducerScheduler::new` does not accept a tick.
- **Suggested fix**: Either (a) delete the setter, or (b) take `tick: Duration` as a second `new` argument with `DEFAULT_TICK_SECONDS` as a default. Option (a) is the smaller change.
- **Risk**: none.

### [MEDIUM] `MemoryCommandHandler::with_note_indexer` is only used in tests; production never wires it
- **Location**: src/memory/events/handler.rs:52-55; production construction at src/bin/aleph-server/commands/start/builder/handlers/memory.rs:360-374
- **Category**: dead-code / architecture
- **Description**: `with_note_indexer` enables the `project_to_notes` notes-filesystem write path. The production wiring in `init_command_handler` constructs the handler with `MemoryCommandHandler::new(state_db)` and never calls `with_note_indexer`. The only callers of `with_note_indexer` are the test module inside `events/handler.rs` itself and `events/testing.rs`. With no indexer, every `create_fact` / `update_content` / `invalidate_fact` / `restore_fact` / `consolidate_facts` / `delete_fact` call writes to the event log and `project_to_notes` returns `Ok(())` without touching the notes filesystem. This is the documented behavior of the dual-write architecture — the event log is the source of truth — but the field's `Option<>` plus the dual implementation creates two parallel write paths that the production init has chosen to ignore.
- **Evidence**: `rg "with_note_indexer\("` returns only the definition (line 52) plus the test module at line 1025 and `events/testing.rs:58`; `init_command_handler` at `handlers/memory.rs:360` calls `MemoryCommandHandler::new(Arc::clone(state_db))` with no indexer. The `note_indexer` field is declared at line 29 as `Option<Arc<NoteIndexer<SqliteMemoryBackend>>>`.
- **Suggested fix**: Either (a) document the production posture (event-log-only writes) and remove `note_indexer` + `project_to_notes` to eliminate the misleading dual-write path; or (b) wire the indexer at `init_command_handler` to actually project every mutation to the notes filesystem. Option (a) is consistent with the current production state and removes ~120 lines + an entire error path.
- **Risk**: medium — option (b) would silently start writing notes from every memory event, which may or may not be intended; option (a) removes a documented behavior path.

### [MEDIUM] `FeedbackFloorLoader::load` is only used by tests
- **Location**: src/memory/assembler/feedback_floor.rs:50-52
- **Category**: dead-code
- **Description**: `load(agent_id: &str)` is the single-partition convenience over `load_many(&[agent_id])`. Production's only call site (`gather.rs:85`) calls `load_many(&feedback_floor_ids)`. The test module at lines 193, 205, 225 calls `load("default")`. Both behaviors live in one type; production never uses the simple form.
- **Evidence**: `rg "feedback_floor\.load\b"` returns only test code; production is `load_many`.
- **Suggested fix**: Delete `load` and have the test module call `load_many(&["default".to_string()])`. Three test calls, each one a small change.
- **Risk**: none.

### [MEDIUM] `SnapshotReader::load_latest` is only used by tests
- **Location**: src/memory/session_resume/reader.rs:69-77 (plus its body to line 80)
- **Category**: dead-code
- **Description**: `load_latest(agent_id, exclude_session_id)` reads the most recent snapshot, excluding a given session. Production uses the partition-aware `load_latest_in_partition(agent_id, partition, exclude_session_id)` instead (called from `gather.rs:243`). The simple form has no production caller — only tests in `reader.rs:154, 173, 191, 212, 215`.
- **Evidence**: `rg "load_latest\("` returns only the test module in reader.rs; production uses `load_latest_in_partition` exclusively.
- **Suggested fix**: Delete `load_latest`; tests can use `load_latest_in_partition("agent", "agent", "exclude")` since the partition argument is the agent id when the room/agent scopes are not yet in play.
- **Risk**: none.

### [MEDIUM] `assembler/feedback_floor.rs` has 240 lines; `load_many` mixes merge, sort, dedup, truncation logic
- **Location**: src/memory/assembler/feedback_floor.rs:76-95
- **Category**: oversize / control-flow
- **Description**: `load_many` does four distinct jobs in one function — (1) extend into `out` from each partition's `load_partition`, (2) sort by severity asc + updated_at desc, (3) dedup by path with a HashSet `retain`, (4) truncate to `FLOOR_CAP`. The nested `load_partition` body (lines 98-150) does its own scan, mtime sort, truncation to `SCAN_CAP`, parse, and severity filter. The function is ~80 lines but interleaves four concerns with non-trivial interleavings (e.g. dedup happens AFTER merge, which is the correct order, but truncation is on the merged list, not per partition — both comments justifying that ordering live inside the function body).
- **Evidence**: `wc -l src/memory/assembler/feedback_floor.rs` → 240; `awk '/^    pub async fn load_many/,/^    }$/' src/memory/assembler/feedback_floor.rs | wc -l` → 22 (the function itself is short; the surrounding module carries the helpers).
- **Suggested fix**: Extract `merge_and_rank(out: &mut Vec<FeedbackFloorEntry>, cap: usize)` from the body of `load_many`. The phase ordering is non-trivial and the function would read top-to-bottom if the merge step were named.
- **Risk**: low.

### [MEDIUM] Duplicated `strip_frontmatter` across three call sites
- **Location**: src/memory/assembler/profile.rs:32-41, src/memory/assembler/feedback_floor.rs:164-173, src/memory/dreaming/stages/note_weave.rs:424-433
- **Category**: duplication
- **Description**: Three implementations of the same "strip leading YAML frontmatter" routine. The `assembler/profile.rs` and `assembler/feedback_floor.rs` versions are byte-identical except for the return type (`String` vs `String`) and one line; the `note_weave.rs` version returns `&str` and uses a slightly different leading pattern (`\n§\n`-style end delimiter vs `\n---\n`). The comment at `feedback_floor.rs:162` explicitly says "Mirrors `profile::strip_frontmatter` (kept local — two trivial call sites do not warrant a shared util per the rule of three)". The third site (`note_weave.rs`) crosses the rule-of-three threshold — the duplicated work should now be extracted.
- **Evidence**: `rg "fn strip_frontmatter" src/memory/` → 3 hits. Two are byte-identical; the third differs in return type and delimiter.
- **Suggested fix**: Add a `pub fn strip_frontmatter(s: &str) -> &str` to `crate::utils::text_format` (the existing text-formatting module) that handles the leading `---` / trailing `\n---\n` case. The `note_weave.rs` version differs in end-marker shape (`---` vs `§`) and would stay local if its use is genuinely different — but the `---` form is shared.
- **Risk**: low.

### [MEDIUM] Dreaming-stage `execute` functions share watermark/ quorum / apply plumbing
- **Location**: src/memory/dreaming/stages/feedback_distill.rs:144-410 (execute), tool_failure_distill.rs:332-450 (execute), skill_distill.rs:155-300 (execute)
- **Category**: duplication
- **Description**: The three "distill" stages follow the same shape: read watermark → fetch input rows from `get_raw_by_path_prefix_since` → quorum check → build prompt with existing candidates → call LLM → parse response → for each action, run skill_gate / recall-evidence gate / evolution-budget gate → apply → record DistillActionRecord. The watermark keys, the candidate-fetch SQL, and the gate plumbing differ only by stage name string and source variant. A real shared driver would take a stage-specific `WatermarkKey`, `SourceVariant`, and `PromptFn` and walk the rest of the shape.
- **Evidence**: side-by-side reading of the three `execute` functions; each opens with `let watermark = match store.get_dream_watermark(WATERMARK_CONSUMER, &ctx.agent_id)` and closes with `Ok(ctx)` after iterating actions. `rg "store.get_raw_by_path_prefix_since\("` returns 2 hits (feedback + tool_failure).
- **Suggested fix**: Extract `pub(super) async fn distill_skill_like(ctx, source: &dyn DistillSource, prompt: &dyn Fn(&[RawMemory], &[String], u32) -> String, ...)`. Each stage provides its source-fetch, watermark key, and prompt function; the driver handles quorum, gates, apply, and bookkeeping.
- **Risk**: medium — the three stages also have stage-specific filtering (`feedback_distill` filters by severity-urgent, `skill_distill` filters by candidate count, `tool_failure_distill` filters by failure signature). A trait-object approach would need to encapsulate those filters.

### [LOW] `ComptrollerConfig {}` is an empty placeholder struct kept only for serde compatibility
- **Location**: src/memory/context_comptroller/config.rs:13
- **Category**: quality / visibility
- **Description**: `ComptrollerConfig` is a `pub struct` with zero fields after commit `7bf481dba` dropped `similarity_threshold`, `token_budget`, and `fold_threshold`. Its doc-comment says "kept as a serialized config handle so callers (and downstream consumers of the serialized form) do not churn; once those knobs are actually implemented, add them back here with `#[serde(default)]`". The struct's `config: ComptrollerConfig` field in `ContextComptroller` is annotated `#[allow(dead_code)]` because it is never read. The single production call site (`builtin_tools/memory_search.rs:318`) passes `ComptrollerConfig::default()` into `ContextComptroller::new(...)` and then never reads it.
- **Evidence**: `rg "ComptrollerConfig"` outside test/integration tests → 1 hit (`builtin_tools/memory_search.rs:50` import + `:318` use).
- **Suggested fix**: Either (a) delete `ComptrollerConfig` and `ComptrollerConfig::default()` entirely; update `ContextComptroller::new` to take `()`; or (b) keep the struct but document that it is a serde-only type alias (`pub struct ComptrollerConfig(Vec<u8>)` of serialized bytes) so the placeholder is honest. Option (a) is the smaller change.
- **Risk**: low — the integration tests file is already non-compiling on the missing-field accesses; clearing them up is the same fix as CRIT 1.

### [LOW] `from_str_or_default` convenience helpers on five enums have no production callers
- **Location**: src/memory/context/enums.rs:224 (FactSource), :287 (MemoryLayer), :387 (MemoryCategory), :446 (FactSpecificity), :500 (TemporalScope)
- **Category**: dead-code
- **Description**: Each of these enums has both a `FromStr` impl (returning `Result<Self, String>`) and a `from_str_or_default(s: &str) -> Self` helper. The `FromStr` is exercised by serde deserialization throughout the codebase; the `from_str_or_default` helper has only test callers. `NoteType::from_str_or_other` (line 110) is the only one with a production caller (`notes/search_result.rs:32` and `builtin_tools/note_manage/mod.rs:137`) because its default is `Other` rather than a non-`Other` value, and callers legitimately want that fallback.
- **Evidence**: `rg "from_str_or_default\("` outside tests → 0 hits in production code.
- **Suggested fix**: Delete the five `from_str_or_default` helpers. If callers want lenient parsing, they use `s.parse::<T>().unwrap_or_default()`.
- **Risk**: none — only tests reference them.

### [LOW] `SignalType` (dreaming/signals.rs:20) is `pub enum` but only consumed internally
- **Location**: src/memory/dreaming/signals.rs:20-26
- **Category**: visibility
- **Description**: `SignalType { Recall, Health, SkillUsage }` is a `pub enum` exported in the `dreaming` module's surface, but consumers (`StrategySelector` at `selector.rs:178`, `MutationGate` in `mutation_gate.rs`) look up signals by `name` string, not by the typed enum. The only places that construct a `DreamSignal { signal_type: SignalType::X }` are inside `signals.rs` itself. A typed enum over an enum nobody else reads is over-exposure.
- **Evidence**: `rg "SignalType::" src/memory/` → all 6 hits are inside `signals.rs`.
- **Suggested fix**: Demote `SignalType` to `enum SignalType { ... }` (private) or fold it into `DreamSignal` as `pub name: String` only.
- **Risk**: none — no external consumer.

### [LOW] `DreamStrategy` matches are exhaustive by compiler; no `unreachable!()` site
- **Location**: src/memory/dreaming/strategy.rs:11-23 (definition), all consumers
- **Category**: quality (carryover from batch-2 finding)
- **Description**: The previous batch-2 finding flagged `DreamStrategy` matched without a fallthrough as a forward-risk: a future variant added without updating the match would compile-error at the call site, which is the right Rust behavior but the previous concern was that "the selector switches on strategy; new variants added without updating the match produce a `match _ => unreachable!()` style site". Today the matches are exhaustive (`Consolidate | Synthesize | Conserve` in `DreamPipeline::from_strategy`); the compiler enforces every new variant at every call site. The runtime panic risk only materializes if someone writes `match s { DreamStrategy::A => ... }` (no catch-all) and adds a new variant — the compiler catches that, not runtime.
- **Evidence**: `rg "match .* DreamStrategy" src/memory/dreaming/` shows the exhaustive arm lists in `mod.rs::from_strategy`, `selector.rs`, `project_cycle.rs`.
- **Suggested fix**: Leave as-is. Document the compile-time exhaustive invariant in `DreamStrategy`'s doc-comment so a future maintainer adding a variant knows the compile-error contract.
- **Risk**: none.

### [LOW] `MemoryExtension` 6-method trait requires consumers to spell out all 6 methods
- **Location**: src/memory/extensions/traits.rs:16-77
- **Category**: trait-erosion (carryover from batch-4 finding)
- **Description**: Every method has a default no-op body, so a consumer needing only `on_capture` has to spell out the trait with `on_capture` overridden and the other five defaulted. The previous batch-4 finding flagged this as a developer-experience issue. The proposed split (`Capture + Retrieve + PreCompress + Delegation + SessionSwitch` supertrait pattern) was not adopted. Five of the seven registered first-party / test extensions implement only `name()` + one hook, paying the cost of typing `async_trait` for the rest.
- **Evidence**: `rg "impl MemoryExtension for" src/memory/extensions/` shows 7+ impls each spelling out only the methods they need.
- **Suggested fix**: Either (a) split into 5 sub-traits and have a type alias `pub trait MemoryExtension: Capture + Retrieve + PreCompress + Delegation + SessionSwitch {}`, or (b) keep the mega-trait but drop `async_trait` in favor of associated `BoxFuture` types so consumers pay no macro cost for defaulted methods.
- **Risk**: low — the public API is `dyn MemoryExtension`, which is invariant; a split into supertraits is source-compatible for object-safe uses.

---

## Cross-cutting observations

- **The integration test file is a regression trap.** `src/memory/integration_tests/mod.rs:20-22, 50-52` still constructs `ComptrollerConfig` with the three fields that commit `7bf481dba` (occams-r5 round-close) deleted. `cargo check --lib --tests` fails with 9 errors; the integration_tests are never run (the file is marked `#[ignore]`), so the regression is silent. The same review round that introduced the field deletion should have updated or removed the test. A simple `rg "similarity_threshold|token_budget|fold_threshold"` gate added to the round-close's pre-merge checklist would catch this.
- **The dreaming pipeline has the same phase shape at three call depths** — `run_dream` (base agent), `run_namespace_cycle` (per corpus), and (partially) `reconcile_once` (event-log / filesystem divergence). Extracting a shared `run_one_cycle` driver would close three duplication sites at once and let the best-health checkpoint, validation report construction, and event-log append live in one place.
- **The `MemoryCommandHandler` write API is one routine repeated seven times.** Six of the seven public methods (`create_fact`, `update_content`, `invalidate_fact`, `restore_fact`, `record_access`, `consolidate_facts`, `delete_fact`) follow the exact same 6-step shape (partition + seq + envelope + append + project_to_notes + error log). Extracting a `write_event` helper shrinks each method to its variant-specific event-construction, removes 9 copies of the `tracing::error!` block, and centralizes the dual-write error contract.
- **The dreaming/stages/ files are oversized single-`execute`-function monoliths.** Six files exceed 700 lines; the dominant reason is a single 200–400 line `execute` method that interleaves data fetch, LLM call, gating, and apply. Three of the six (`feedback_distill`, `tool_failure_distill`, `skill_distill`) share an additional shape — a shared `distill_skill_like` driver would close the gap.
- **The "spelling a struct literally" pattern is everywhere** — the comment "kept local — two trivial call sites do not warrant a shared util per the rule of three" at `feedback_floor.rs:162` now has its third caller (`note_weave.rs:424`). Time to extract.

## Out of scope (intentionally not flagged)

- The 17 CJK / byte-vs-char findings in batch-6 (`assembler/hybrid.rs::hydrate`) have all been fixed — `truncate_chars` is now used and the budget is denominated in characters. The regression tests at `hybrid.rs:683-723` pin the new behavior.
- The `curated::legacy` "no callers" finding in batch-6 is wrong on this branch — `curated/store.rs:12, 224, 412` actively uses `legacy::load_body` and `ParsedLoad`. The previous review's grep was scoped to a subset of the codebase.
- `NoteType` doc-list drift (batch-6: "doc lists 4 layers but enum has 3") — the current code lists all four `CognitiveLayer` variants (`Working / Episodic / Semantic / Raw` at `context/enums.rs:286-310`) and the comment matches.
- The `DreamStrategy` exhaustiveness finding (batch-2) is moot — Rust's exhaustive `match` on a `pub enum` enforces the contract at compile time, so no `unreachable!()` style site can exist without intentional effort.
- The `MemoryExtension` 6-method trait split (batch-4) was not adopted but is low-priority; the practical impact is `async_trait` boilerplate on test extensions, which the codebase already accepts.
- The `MentionWeaveStage` `unsafe { ... }` blocks (batch-2) live in test setup code, guarded by `// SAFETY: ...` comments. Acceptable as scoped to test-only env-mutation.
- The assembler `hydrate` invariants (`batch-4`) — every trim/break/empty-retain branch is now pinned by `hybrid.rs:599-737` regression tests.
- The curated store `tokio::sync::Mutex<()>` I/O gate (batch-6) has been replaced by an `fs2::FileExt` advisory lock + `tokio::sync::Mutex<()>` in-process gate (`store.rs:124-150`), with the documented rationale.
- `ComptrollerConfig`'s serialized form is intentionally stable (the struct exists as a serde placeholder per its own doc-comment); the regression is in the test file, not the struct shape.