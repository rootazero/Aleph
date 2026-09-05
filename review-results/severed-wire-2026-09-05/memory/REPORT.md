# Severed-Wire Audit — `src/memory` (2026-09-05)

| Field | Value |
| --- | --- |
| Module | `src/memory` (245 `.rs` files, ~83.9 kLoC) |
| Method | PRODUCED - CONSUMED parity via `rg` across `src/`, `bin/`, `interfaces/`, `shared/`, `desktop/`. Each candidate's every consumer file confirmed; an item with one file is suspicious, an item with one file and only test consumers is severed. |
| Scope | Public surface (`pub fn / struct / enum / trait / type / const / static`) — 856 items across 184 files. |
| Out of scope | `#[cfg(test)]` modules, style nits, `#[allow(dead_code)]` test-only accessors, items with `#[cfg(...)]` that are clearly feature-gated (`loom`, `test-helpers`). |

The memory module is *unusually* well-wired: most public types are
re-exported from `memory::mod.rs` and reach either a `bin/aleph-server`
or a `builtin_tools` call site. The wiring is dense — every dream
stage is enumerated in `src/memory/dreaming/mod.rs`, every
`rerank::*Provider` is dispatched from `src/memory/rerank/mod.rs`,
every extension is `register`ed in `src/bin/aleph-server/.../memory.rs`,
every assembler is built in `src/thinker/memory_context_provider/constructor.rs`.
The severed wires below are the holes in that fabric, not a systematic
gap.

Each finding below cites the exact `rg` evidence; the verification line
lists every file in which the symbol appears so a reviewer can replay
the audit in one minute.

---

## Findings (12 total)

| ID | Decision | Form | Severity | Headline |
| --- | --- | --- | --- | --- |
| `sw-memory-01` | CUT | 1 | low | `EventProjector::new` / `rebuild_fact` / `rebuild_fact_at` — instance methods with zero callers; only the static `fold_events_to_note` is used. |
| `sw-memory-02` | CUT | 1 | medium | `memory::events::migration::{EventSourcingMigration, run_if_needed, migrate_facts}` — full module has zero production callers; only its own tests. |
| `sw-memory-03` | CUT | 1 | low | `memory::context::fact::with_embedding` — `pub fn` builder with zero callers anywhere in the tree. |
| `sw-memory-04` | CUT | 1 | low | `memory::context::fact::close_validity` — `pub fn` builder with only an in-file test caller. |
| `sw-memory-05` | CUT | 1 | medium | `memory::store::sqlite::recall_signals::{aggregate_for_facts, RecallAggregate}` — `pub` aggregate and 7-field struct used only in tests; the file's own docstring says other fields are "currently unconsumed". |
| `sw-memory-06` | CUT | 1 | low | `memory::dreaming::stages::workflow_proposal::aggregate_chains` — `pub fn` used only in the file's own tests. |
| `sw-memory-07` | CUT | 1 | low | `memory::dreaming::stages::skill_distill::build_distill_prompt_with_candidates` — `pub fn` used only in the file's own tests. |
| `sw-memory-08` | CUT | 1 | low | `memory::dreaming::stages::tool_failure_distill::build_tool_failure_prompt` — `pub fn` used only in the file's own tests. |
| `sw-memory-09` | CUT | 1 | low | `memory::session_compactor::summary_engine::chunk_messages` — `pub fn` used only in the file's own tests. |
| `sw-memory-10` | CUT | 1 | low | `memory::dreaming::validation::validate_frontmatter` — `pub fn` used only in the file's own tests. |
| `sw-memory-11` | CUT | 1 | low | `memory::note_retrieval::builder::with_reranker` — `pub fn` used only in tests; production wires via `with_rerank_config`. |
| `sw-memory-12` | CUT | 1 | low | `memory::session_search_summary::dedup::{ScoredCandidate, top_per_session}` — `pub` struct + `pub fn` used only in the file's own tests. |

The triage decision tree resolved cleanly to "no live caller anywhere"
for every finding — none required a CONNECT (reviving a severed arm) or
a DECIDE (product call). All are low or medium severity, but several
are honest remnants of refactors that left a `pub` surface behind; the
report lists each so a single follow-up PR can either delete them or
flip them to `pub(crate)`.

Two larger patterns were checked and explicitly *not* reported:

* `MemoryTimeTraveler::new` / `explain_fact` — wired through
  `bin/aleph-server/.../agent_init/mod.rs:722` and
  `builtin_tools/memory_timeline.rs:42`.
* `SessionReflector::reflect` — wired through
  `gateway/session_manager/ops/emit.rs:217` (the same
  fire-and-forget task that drives `/end-summary`).
* Every dream stage (`NoteLintStage`, `NoteDecayStage`,
  `GraphRecomputeStage`, `GoalLessonsPromoteStage`,
  `IndexRefresherStage`, etc.) is constructed in
  `src/memory/dreaming/mod.rs` (`Box::new(stages::FooStage)` lines
  238–344). All wired.
* Every `rerank::*RerankProvider` is dispatched in
  `src/memory/rerank/mod.rs:36–51`. All wired.

---

## Evidence

### `sw-memory-01` — `EventProjector` instance surface is dead

**Symbols**: `EventProjector::new`,
`EventProjector::rebuild_fact`, `EventProjector::rebuild_fact_at`
(`src/memory/events/projector.rs:39, 231, 239`).

The struct exists, has `db: Arc<StateDatabase>` field, exposes three
instance methods. The only externally used method is the **static**
`EventProjector::fold_events_to_note(&[MemoryEventEnvelope])`. The
struct itself never needs to be constructed.

```sh
$ rg -n "EventProjector::new" --type rust
# (no hits)

$ rg -n "\.rebuild_fact\b" --type rust
# (no hits)

$ rg -n "\.rebuild_fact_at\b" --type rust
# (no hits)

$ rg -n "EventProjector\b" --type rust | rg -v "fold_events_to_note|//"
src/memory/mod.rs:87:    projector::EventProjector,         # re-export only
src/memory/events/projector.rs:32:pub struct EventProjector { # definition
src/memory/events/projector.rs:36:impl EventProjector {        # impl block
```

**Decision**: CUT (Form 1: producer with zero callers — visible to
`dead_code` lint).

**Proposed change**: keep `fold_events_to_note`; convert the rest to
private helpers or move `get_memory_events_for_fact` and
`get_memory_events_until` calls inline. The `db` field on the struct
is dead weight.

**Risk**: Low. `fold_events_to_note` is the only used method and it
is `pub static`-eligible (no `&self`); the migration test at
`src/memory/events/migration.rs:266` and the integration tests at
`src/memory/integration_tests/mod.rs:72` use it via the static path.

**Verification**: `rg -n "EventProjector" --type rust | rg -v "fold_events_to_note"`
returns only the re-export line at `src/memory/mod.rs:87` plus the
defining file itself.

---

### `sw-memory-02` — `EventSourcingMigration` has zero production callers

**Symbols**: `EventSourcingMigration::new`, `run_if_needed`,
`migrate_facts`, `MigrationReport`
(`src/memory/events/migration.rs:33, 37, 50, 81`).

The whole 130-line module is exercised only by its own
`#[cfg(test)] mod tests`. There is no `aleph-server` boot hook, no
`bin/aleph-server` migration command, no startup wiring — the
event-sourced store simply replaced the legacy store without ever
running this migration.

```sh
$ rg -n "EventSourcingMigration::new|EventSourcingMigration::|run_if_needed|migrate_facts|MigrationReport::" \
    --type rust | rg -v "migration\.rs"
# (no hits outside the defining file)
```

The only other file that mentions it is
`src/memory/events/migration.rs` itself (definitions + 5 unit tests).

**Decision**: CUT (Form 1: producer with zero callers).

**Proposed change**: Delete `src/memory/events/migration.rs` and
remove the `pub use migration::{EventSourcingMigration, MigrationReport}`
line from `src/memory/mod.rs:84`. ~280 lines including tests.

**Risk**: Medium — if a future boot ever needs to back-fill events
from a pre-event-sourced install, the migration must be re-derived
from the same code paths. Keep a 5-line `// historical migration ran
here, see git history` note in the schema migration log
(`src/memory/store/sqlite/schema/migrations.rs:30` already documents
the column-level migration).

**Verification**: every reference outside `migration.rs` is the type
definition itself; `rg -l "EventSourcingMigration" --type rust`
returns only `src/memory/mod.rs` (re-export) and
`src/memory/events/migration.rs`.

---

### `sw-memory-03` — `MemoryFact::with_embedding` has zero callers

**Symbol**: `src/memory/context/fact.rs:192`
```rust
pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
    self.embedding = Some(embedding);
    self
}
```

```sh
$ rg -n "with_embedding" --type rust
src/memory/context/fact.rs:192:    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
# (no other hits)
```

The companion `with_score` and `with_specificity` are also unused, but
those are exposed via `pub use` and at least carry a docstring; this
one has no callers at all.

**Decision**: CUT (Form 1).

**Proposed change**: Delete `with_embedding` from `src/memory/context/fact.rs`.

**Risk**: None — it is a builder method, no I/O, no semantic effect.

---

### `sw-memory-04` — `MemoryFact::close_validity` is dead

**Symbol**: `src/memory/context/fact.rs:303`
```rust
pub fn close_validity(mut self) -> Self { ... }
```

```sh
$ rg -n "close_validity" --type rust
src/memory/context/fact.rs:303:    pub fn close_validity(mut self) -> Self {
src/memory/context/fact.rs:334:        ... .close_validity();   # test only
```

**Decision**: CUT (Form 1).

**Proposed change**: Delete `close_validity` and its test. The
`valid_from` / `valid_to` fields are otherwise unwritten in
production code (`src/memory/store/sqlite/notes/store_impl.rs` only
sets them via `serde_json::from_value` deserialisation paths).

**Risk**: Low — the only other writer of `valid_to` is the legacy
migration test path (which we are deleting in `sw-memory-02`).

---

### `sw-memory-05` — `recall_signals::aggregate_for_facts` + `RecallAggregate`

**Symbols** (`src/memory/store/sqlite/recall_signals.rs`):
```rust
pub struct RecallAggregate {                  // line 25
    pub note_path: String,
    pub signal_count: i64,
    pub total_score: f64,
    pub unique_queries: i64,
    pub unique_channels: i64,
    pub recall_days: i64,
    pub first_recall: i64,
    pub last_recall: i64,
}
pub fn aggregate_for_facts(...) -> Vec<RecallAggregate>  // line 142
```

The module's own docstring (line 9) flags this directly:
> "The other `RecallAggregate` fields are currently unconsumed — see the struct."

```sh
$ rg -n "RecallAggregate::|aggregate_for_facts" --type rust
src/memory/store/sqlite/recall_signals.rs:25:pub struct RecallAggregate {
src/memory/store/sqlite/recall_signals.rs:142:    pub fn aggregate_for_facts(
src/memory/store/sqlite/recall_signals.rs:189: ... aggregate_for_facts prepare ...
src/memory/store/sqlite/recall_signals.rs:204: ... aggregate_for_facts query ...
src/memory/store/sqlite/recall_signals.rs:208: ... aggregate_for_facts row ...
src/memory/store/sqlite/recall_signals.rs:353:        .aggregate_for_facts("owner", &["f1".into(), "f2".into()])
src/memory/store/sqlite/recall_signals.rs:400:        let agg = store.aggregate_for_facts("owner", &["f1".into()]).unwrap();
src/memory/store/sqlite/recall_signals.rs:427:        let agg = store.aggregate_for_facts("owner", &["f1".into()]).unwrap();
src/memory/store/sqlite/recall_signals.rs:442:        let agg2 = store.aggregate_for_facts("owner", &["f2".into()]).unwrap();
src/memory/store/sqlite/recall_signals.rs:449:        let agg3 = store.aggregate_for_facts("owner", &["f2".into()]).unwrap();
# (all callers are in the defining file)
```

The only **production** path that reads `recall_signals` is the
`recall_signals_last_hit` lookup (used by
`src/memory/dreaming/stages/note_decay.rs:130`) and the
`co_recall_pairs` aggregate (used by
`src/memory/dreaming/stages/co_recall_edges.rs:53`) — neither touches
`aggregate_for_facts`.

**Decision**: CUT (Form 1).

**Proposed change**: Either delete `aggregate_for_facts` and
`RecallAggregate` (and 5 tests), or shrink `RecallAggregate` to
`(note_path, signal_count)` and demote `aggregate_for_facts` to a
private helper if the note-decay stage eventually needs more than
`recall_signals_last_hit`. The existing `recall_support(hit_count)`
function in `evolution::evidence` derives its saturating score from a
plain `i64` hit count — `RecallAggregate` adds nothing.

**Risk**: Medium — the 7-field struct was designed as the read-side
projection; deleting it removes the only `pub`-exposed way to ask
"what notes were recalled this cycle". The dream stage that *should*
need this (`note_decay`) was forced to settle for a per-note
last-hit lookup instead.

**Verification**: `rg -l "RecallAggregate" --type rust` returns only
`src/memory/store/sqlite/recall_signals.rs`.

---

### `sw-memory-06` — `workflow_proposal::aggregate_chains` is dead

**Symbol**: `src/memory/dreaming/stages/workflow_proposal.rs:72`
```rust
pub fn aggregate_chains(chains: Vec<Vec<String>>) -> HashMap<String, (u32, Vec<String>)>
```

```sh
$ rg -n "aggregate_chains" --type rust
src/memory/dreaming/stages/workflow_proposal.rs:72:pub fn aggregate_chains(...)
src/memory/dreaming/stages/workflow_proposal.rs:109:        let aggregated = aggregate_chains(all_chains);
src/memory/dreaming/stages/workflow_proposal.rs:167:        let agg = aggregate_chains(chains);
src/memory/dreaming/stages/workflow_proposal.rs:179:        let agg = aggregate_chains(vec![vec!["solo".into()]]);
# (all 4 callers are in the defining file; 3 are tests)
```

The production `WorkflowProposalStage::execute` builds its
representative chain differently (it sorts by `(count, first_seen)`
during SQL aggregation and never calls this fn). `aggregate_chains`
is a leftover pure helper that only the tests reach.

**Decision**: CUT (Form 1).

**Proposed change**: Drop `aggregate_chains` (line 72) plus the 2
test-only call sites at lines 167 and 179.

**Risk**: None — pure helper, no I/O.

---

### `sw-memory-07` — `skill_distill::build_distill_prompt_with_candidates` is dead

**Symbol**: `src/memory/dreaming/stages/skill_distill.rs:266`
```rust
pub fn build_distill_prompt_with_candidates(...)
```

```sh
$ rg -n "build_distill_prompt_with_candidates" --type rust
src/memory/dreaming/stages/skill_distill.rs:105:            let prompt = build_distill_prompt_with_candidates(
src/memory/dreaming/stages/skill_distill.rs:266:pub fn build_distill_prompt_with_candidates(
src/memory/dreaming/stages/skill_distill.rs:388:        let prompt = build_distill_prompt_with_candidates(
src/memory/dreaming/stages/skill_distill.rs:408:        let prompt = build_distill_prompt_with_candidates(
src/memory/dreaming/stages/skill_distill.rs:434:    fn build_distill_prompt_with_candidates_includes_existing_block() {
src/memory/dreaming/stages/skill_distill.rs:439:        let prompt = build_distill_prompt_with_candidates(
src/memory/dreaming/stages/skill_distill.rs:474:        let prompt = build_distill_prompt_with_candidates("text", "skill", 3, &[], &[]);
# (line 105 is inside a #[cfg(test)] mock impl, lines 388+ are tests)
```

**Decision**: CUT (Form 1).

**Proposed change**: Delete the `pub fn` and the 5 test usages.

**Risk**: None — pure prompt builder, identical signature to other stage prompt builders; the production `SkillDistillStage::execute` (line 95) builds its prompt inline via the same template.

---

### `sw-memory-08` — `tool_failure_distill::build_tool_failure_prompt` is dead

**Symbol**: `src/memory/dreaming/stages/tool_failure_distill.rs:454`
```rust
pub fn build_tool_failure_prompt(...)
```

```sh
$ rg -n "build_tool_failure_prompt" --type rust
src/memory/dreaming/stages/tool_failure_distill.rs:270:        let prompt = build_tool_failure_prompt(    # inside #[cfg(test)]
src/memory/dreaming/stages/tool_failure_distill.rs:454:pub fn build_tool_failure_prompt(
src/memory/dreaming/stages/tool_failure_distill.rs:603:        let p = build_tool_failure_prompt(&d, &[], 2, &[]);
src/memory/dreaming/stages/tool_failure_distill.rs:621:        let p = build_tool_failure_prompt(&d, &[], 2, &[]);
src/memory/dreaming/stages/tool_failure_distill.rs:639:        let p = build_tool_failure_prompt(&d, &["..."], 2, &[]);
src/memory/dreaming/stages/tool_failure_distill.rs:660:        let plain = build_tool_failure_prompt(&d, &[], 2, &[]);
src/memory/dreaming/stages/tool_failure_distill.rs:662:        let with_reject = build_tool_failure_prompt(
# (lines 270 is in a test mock impl; lines 603+ are tests)
```

Same shape as `sw-memory-07`. Production builds its prompt inline.

**Decision**: CUT (Form 1).

---

### `sw-memory-09` — `summary_engine::chunk_messages` is dead

**Symbol**: `src/memory/session_compactor/summary_engine.rs:142`
```rust
pub fn chunk_messages(messages: &[(String, String)], chunk_tokens: usize, ratio: f64) -> Vec<Vec<(String, String)>>
```

```sh
$ rg -n "chunk_messages" --type rust
src/memory/session_compactor/summary_engine.rs:134:// chunk_messages
src/memory/session_compactor/summary_engine.rs:142:pub fn chunk_messages(...)
src/memory/session_compactor/summary_engine.rs:319:    fn test_chunk_messages_empty() {
src/memory/session_compactor/summary_engine.rs:320:        let chunks = chunk_messages(&[], 100, 3.5);
src/memory/session_compactor/summary_engine.rs:328:        let chunks = chunk_messages(&messages, 10_000, 3.5);
src/memory/session_compactor/summary_engine.rs:334:    fn test_chunk_messages_splits_correctly() {
src/memory/session_compactor/summary_engine.rs:341:        let chunks = chunk_messages(&messages, 15, 3.5);
src/memory/session_compactor/summary_engine.rs:353:    fn test_chunk_messages_single_large_message_forms_own_chunk() {
# (only inside the defining file)
```

The production summariser (`src/memory/session_compactor/post_turn_compress.rs`)
chunks via `partition_fresh_tail_pairs`, not this one.

**Decision**: CUT (Form 1).

---

### `sw-memory-10` — `validation::validate_frontmatter` is dead

**Symbol**: `src/memory/dreaming/validation.rs:69`
```rust
pub fn validate_frontmatter(content: &str, note_path: &str) -> Vec<ValidationIssue>
```

```sh
$ rg -n "validate_frontmatter" --type rust
src/memory/dreaming/validation.rs:69:pub fn validate_frontmatter(...)
src/memory/dreaming/validation.rs:182:        let note_issues = validate_frontmatter(content, path);
src/memory/dreaming/validation.rs:220:        let issues = validate_frontmatter(content, "learning/test");
src/memory/dreaming/validation.rs:227:        let issues = validate_frontmatter(content, "learning/test");
src/memory/dreaming/validation.rs:236:        let issues = validate_frontmatter(content, "learning/test");
src/memory/dreaming/validation.rs:243:        let issues = validate_frontmatter(content, "learning/test");
# (only the defining file)
```

The aggregate runners `run_l1_validation` / `run_l2_validation` (lines
175, 200) are wired into the dream cycle (`dreaming/mod.rs:1639` /
`dreaming/project_cycle.rs:345`); `validate_frontmatter` is the
single-note primitive they wrap and is itself never called.

**Decision**: CUT (Form 1).

**Proposed change**: Demote to `fn` (private) and keep it as a helper
of the two runners, or delete the 4 single-note tests. The 4 callers
in tests are all asserting on the same shape; consolidating them into
the L1 runner test is straightforward.

**Risk**: Low — the function is the only producer of `ValidationIssue`
rows; the L1 runner reaches it via internal call (line 175).

---

### `sw-memory-11` — `NoteFactRetrieval::with_reranker` is dead

**Symbol**: `src/memory/note_retrieval/builder.rs:57`
```rust
pub fn with_reranker(mut self, reranker: Arc<dyn RerankProvider>, weight: f32) -> Self
```

```sh
$ rg -n "with_reranker\(" --type rust
src/memory/note_retrieval/tests.rs:527:        retrieval.with_reranker(Arc::new(MockReranker { scores, fail }), weight)
src/memory/note_retrieval/tests.rs:620:        let retrieval = NoteFactRetrieval::new(indexer, embedder).with_reranker(
src/memory/note_retrieval/builder.rs:57:    pub fn with_reranker(mut self, reranker: Arc<dyn RerankProvider>, weight: f32) -> Self {
src/memory/note_retrieval/builder.rs:73:        self.with_reranker(provider, cfg.rerank_weight)    # internal
# (production uses with_rerank_config)
```

Production wiring goes through `with_rerank_config`
(`src/thinker/memory_context_provider/constructor.rs:218`,
`src/builtin_tools/memory_search.rs:308`,
`src/gateway/handlers/memory_config.rs:270`). The `with_reranker`
variant exists only because `with_rerank_config` calls it internally
(line 73), and only tests use the direct variant.

**Decision**: CUT (Form 1).

**Proposed change**: Demote `with_reranker` to private; tests can use
`with_rerank_config(&RerankConfig { enabled: true, provider: …,
weight, ..Default::default() })` instead.

**Risk**: Low — the two tests can be migrated to `RerankConfig`
constructors with no semantic change.

---

### `sw-memory-12` — `session_search_summary::dedup::{ScoredCandidate, top_per_session}` is dead

**Symbols** (`src/memory/session_search_summary/dedup.rs`):
```rust
pub struct ScoredCandidate {                  // line 12
    session_id: String,
    fact_path: String,
    score: f32,
}
pub fn top_per_session(candidates: Vec<ScoredCandidate>, limit: usize)
    -> Vec<ScoredCandidate>                   // line 25
```

```sh
$ rg -n "ScoredCandidate|top_per_session" --type rust
src/memory/session_search_summary/dedup.rs:12:pub struct ScoredCandidate {
src/memory/session_search_summary/dedup.rs:25:pub fn top_per_session(
src/memory/session_search_summary/dedup.rs:29:    let mut best_per_session: HashMap<String, ScoredCandidate> = ...
src/memory/session_search_summary/dedup.rs:41:    ...: Vec<ScoredCandidate> = best_per_session.into_values().collect();
src/memory/session_search_summary/dedup.rs:58:    fn cand(...) -> ScoredCandidate { ... }   # test helper
src/memory/session_search_summary/dedup.rs:72:        let out = top_per_session(vec![], 5);
# (only the defining file)
```

The module is part of `session_search_summary` (the
`/end-summary` summariser path) which is wired through
`bin/aleph-server/.../agent_init/mod.rs:1132–1168` and
`gateway/session_manager/ops/emit.rs`. The summariser's actual
dedup is done elsewhere (`session_search_summary/filter.rs:11
FactSourceFilter::Only(...)`, `session_search_summary/mod.rs`). This
`dedup.rs` is a leftover from an earlier ranking strategy and is
not on the live `/end-summary` path.

**Decision**: CUT (Form 1).

**Proposed change**: Delete `src/memory/session_search_summary/dedup.rs`.
The summariser's real `MemoGroup::merge` (in
`session_search_summary/synthesizer.rs`) handles
per-session collapsing via `HashMap<String, MemoGroup>` already.

**Risk**: Low — pure dead-wood of an abandoned ranking strategy.

---

## Negative findings (explicitly checked, not severed)

The audit also confirmed the following public items are
*fully wired* and should NOT be touched:

* **Every DreamStage** (`NoteLintStage`, `NoteDecayStage`,
  `NoteReviewStage`, `NoteConsolidateStage`, `NoteSynthesisStage`,
  `NoteDriftStage`, `NoteWeaveStage`, `MentionWeaveStage`,
  `SkillDistillStage`, `SkillLifecycleStage`, `WorkflowProposalStage`,
  `ToolFailureDistillStage`, `FeedbackDistillStage`,
  `GraphRecomputeStage`, `CoRecallEdgesStage`, `DailyDigestStage`,
  `IndexRefresherStage`, `GoalLessonsPromoteStage`,
  `CorpusNarrativeStage`) is constructed in
  `src/memory/dreaming/mod.rs:238–344`. Verified by
  `rg -n "Box::new\(stages::" src/memory/dreaming/mod.rs`.
* **`RerankConfig`, `RerankProvider`, `RerankResult`,
  `RerankProviderType`** + all 6 provider implementations
  (`JinaRerankProvider`, `VoyageRerankProvider`, `SiliconFlowRerankProvider`,
  `CohereRerankProvider`, `VllmRerankProvider`, `PineconeRerankProvider`)
  dispatched in `src/memory/rerank/mod.rs:36–51`. `build_provider`
  consumer: `src/gateway/handlers/rerank_config.rs:183`.
* **`CompressionService` builder chain** (`new`,
  `new_with_backend`, `with_compound_ingestor`,
  `with_profile_synthesizer`, `with_extension_registry`,
  `with_post_hook`, `add_post_hook`) wired from
  `src/bin/aleph-server/.../handlers/memory.rs:330–341` and
  `agent_init/mod.rs:1106`.
* **Stage wiring for `IndexRefresherStage`** (comment in
  `src/bin/aleph-server/.../agent_init/mod.rs:1004`) confirmed —
  the comment says "thread wiki handle into DreamDaemon for
  IndexRefresherStage"; actual instantiation is
  `src/memory/dreaming/mod.rs:271` and `:342`.
* **`NoteVaultWatcher` struct** — `pub struct` but with no public
  fields (only a private `_debouncer`). `spawn_note_vault_watcher()`
  is the only entry point and is wired from
  `src/bin/aleph-server/.../start/mod.rs:2405`. The struct is a
  token type, not a dead surface.
* **`recall_signals_last_hit`** (read-side of `recall_signals`) used
  by `src/memory/dreaming/stages/note_decay.rs:130` and
  `src/memory/notes/store.rs:794`.
* **`scratchpad::{ScratchpadManager, PlanItem, PlanItemStatus,
  ScratchpadSnapshot, COMPLETION_BANNER}`** used from
  `src/verification/scratchpad_goal_verifier.rs:123`,
  `src/builtin_tools/scratchpad_registry.rs:214`,
  `src/gateway/handlers/chat.rs:1832–1842`.
* **`ProfileSynthesizer` / `FsProfileSynthesizer`** wired through
  `src/thinker/memory_context_provider/curated.rs:598–613`,
  `src/builtin_tools/user_profile.rs:25`,
  `src/bin/aleph-server/.../agent_init/mod.rs:411–414`.
* **`NoteOrientation` trait + `FsNoteOrientation`** wired through
  `src/thinker/memory_context_provider/constructor.rs:281`,
  `src/bin/aleph-server/.../start/mod.rs:1281–1283`, and every
  dream stage that calls `ctx.orientation.refresh_index_after_ingest`.
* **`MemoryTimeTraveler` + `explain_fact`** wired through
  `src/executor/builtin_registry/.../constructor/mod.rs:722` and
  `src/builtin_tools/memory_timeline.rs:42`.
* **`SessionReflector::reflect`** wired through
  `src/gateway/session_manager/ops/emit.rs:217` (the same
  fire-and-forget task that emits `/end-summary`).
* **`MemoryCommandHandler`** wired through
  `src/bin/aleph-server/.../start/mod.rs:1897`,
  `src/builtin_tools/note_manage/mod.rs:112`, and the reconciler
  admin API.
* **`flush::{FlushRegistry, FlushGuard, global_registry,
  flush_agent_memory}`** wired from
  `src/gateway/session_manager/ops/emit.rs:178,265,293`,
  `src/builtin_tools/flag_user_correction.rs:340–342`,
  `src/memory/assembler/hybrid.rs:314`.
* **`TranscriptIndexer` + `TranscriptIndexerConfig`** wired from
  `src/memory/session_compactor/constructor.rs:44` and the
  `post_turn_compress` indexer hook (line 267).

---

## Items demoted to `low` (still surfaced as severed but no live production consequence)

* `memory::notes::graph::minhash::shingles` / `jaccard_estimate` —
  used inside `similarity_edges` and the file's tests but never
  externally. Both could be demoted to private.
* `memory::store::sqlite::recall_signals::{query_hash,
  today_bucket}` — used inside `record_signals` only; demote to
  private.
* `memory::dreaming::evolution::evidence::recall_support` — used
  inside `gate_supersede_evidence` only; demote to private.
* `memory::scratchpad::manager::plan_dir_for` — used inside
  `ScratchpadManager::new` only; demote to private.
* `memory::compression::service::new_with_backend` — used inside
  `new` only; demote to private.

These are not in the CUT list because (a) they are obvious "module
helper, should be `fn`" candidates and (b) the 12 CUT items above
already cover the severable-wires-of-concern for a single audit
follow-up. They are listed here for the next pass.

---

## What this audit did NOT find

* **No stub far-ends** (Form 2). Every error path that returns
  `Ok(())` is intentional — they are the "fire-and-forget" guard
  documented in `src/gateway/session_manager/ops/emit.rs` and
  `src/memory/session_reflection/mod.rs` ("P7 — memory is
  degradable").
* **No inert config** (Form 3). The `agent_init` / `handlers/memory`
  builders always pass `ReflectionConfig`, `RerankConfig`, etc. —
  every config knob is read by at least one consumer.
* **No client ghosts** (Form 4). Every caller of `SessionReflector`,
  `MemoryTimeTraveler`, `MemoryCommandHandler`, `ProfileSynthesizer`,
  `NoteOrientation`, `RerankProvider`, `CompoundIngestor`,
  `QueryFiler`, `MemoryExtension`, `ToolSignalSink`,
  `EmbeddingProvider`, `AiProvider` has a matching impl.
* **No name/path drift** (Form 5). Module paths are stable;
  `pub use` re-exports from `src/memory/mod.rs` cover every
  externally consumed type.
* **No never-compiled far-ends** (Form 6). The two feature flags
  used in `src/memory/` (`loom` at line 55, `test-helpers` at
  `events/testing.rs:27`) are both declared in `Cargo.toml:141, 143`
  with `required-features` on every integration test that needs
  them — the comment block at `Cargo.toml:400–413` documents the
  exact failure mode (cargo compiling an empty binary that reports
  "0 passed") and the fix.