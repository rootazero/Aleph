# Memory LLM-Wiki Evolution — Top-Level Design

> **Status**: design-approved, pending plan
> **Date**: 2026-04-14
> **Supersedes**: none (extends Spec 1/2/3 of the memory-evolution roadmap)
> **Implementation**: split into 4 sub-specs (Spec 5 / 6 / 7 / 8), each with its own plan.

## 0. Motivation

Aleph's memory system (L0 raw → L1 notes, markdown-as-source-of-truth + SQLite index, Dream Daemon offline consolidation, Spec 1 capture hooks, Spec 2 reflector, Spec 3 fencing/modes) is solid infrastructure, but compared to `hermes-agent` (Nous Research) and its **llm-wiki** pattern the following gaps remain:

| Gap | hermes / llm-wiki | Aleph today | Severity |
|---|---|---|---|
| **Orientation layer** | `SCHEMA.md` + `index.md` + `log.md` give the LLM a global map each session | `notes_index` lives in SQLite, invisible to the model | **core** |
| **Compound ingest** | One source touches 5–15 pages (entity/concept/comparison updated together) | `extract_note_updates` is a single 1→N LLM call that does not read existing pages | **core** |
| **User profile modelling** | Honcho-style dialectic model of the user | Bullet facts scattered across `personal/` | **core** |
| **Query filed-back** | Valuable Q&A archived as new knowledge | `memory_reflect` answers vanish into chat history | gap |
| **Contradiction handling** | Explicit `contradict` pathway in ingest | Only `NoteDrift` catches it, post-hoc on weekly run | partial |

This design imports the four missing capabilities while preserving Aleph's architectural choices (Rust traits, event sourcing, markdown as source of truth, LLM sovereignty). It does not copy hermes; it fuses hermes ideas with Aleph idioms.

## 1. Architecture

### 1.1 Design principles

| Principle | How it lands |
|---|---|
| **R8 LLM sovereignty** | Schema bootstrapping, compound plan, profile merge, query-file gating are all LLM outputs. Rust code does read/write/apply/validate — no heuristic replacements. |
| **P3 Open-Closed** | Four new traits (`WikiOrientation`, `CompoundIngestor`, `ProfileSynthesizer`, `QueryFiler`) injected alongside existing `NoteStore` / `NoteIndexer`. No rewrite of `NoteStore` surface. |
| **Markdown is source of truth** | `SCHEMA.md`, `index.md`, `log.md`, `USER.md` are plain markdown. `full_rebuild` continues to reconstruct every SQLite index from disk. |
| **Rust leverage** | `CompoundApplyTx` uses staged-file + batched rename for atomic multi-page writes; proptest guards plan/apply invariants; trait-object composition for testability. |
| **Progressive enhancement** | Legacy `extract_note_updates` path retained for one deprecation window; `compound_ingest_enabled` config toggles at runtime. |

### 1.2 Filesystem layout

```text
~/.aleph/memory/note/{agent_id}/
  ├── SCHEMA.md        ← NEW: LLM-maintained workflow constitution
  ├── index.md         ← NEW: category catalog with per-note one-liner (LLM-readable)
  ├── log.md           ← NEW: append-only timeline (rotates at 2000 lines)
  ├── USER.md          ← NEW: dialectic user profile
  ├── preference/ plan/ learning/ project/ personal/ tool/ lesson/ skill/ wiki/ transcript/ other/  (existing)
  ├── query/           ← NEW: filed-back query answers
  └── archive/          (existing — NoteDecay destination)
```

### 1.3 New components and trigger points

```text
                          ┌──── session_start ────┐
                          ▼                       │
Agent Loop (Spec 3 prompt builder)                │
  │                                               │
  │  LayerInput::orientation_message  ◄───────── WikiOrientation::read_snapshot()
  │  LayerInput::memory_user_message  (existing)                       │
  │  LayerInput::profile_user_message ◄───────── ProfileSynthesizer::current()
  ▼                                                                    │
LLM turn                                                               │
  │                                                                    │
  │ ingest trigger (CompressionService tick)                           │
  │   → CompoundIngestor::plan(raws, related_pages)   ◄── 2-phase LLM │
  │   → CompoundIngestor::apply(plan, tx)                             │
  │   → WikiOrientation::record_ingest(touched_paths, summary)        │
  │                                                                    │
  │ memory_reflect/memory_search returns                               │
  │   → QueryFiler::maybe_file(query, synthesis, sources)             │
  │                                                                    │
  │ session_end (Spec 1 DIGEST/RETRO)                                  │
  │   → ProfileSynthesizer::update(diff, reason)                      │
  │   → WikiOrientation::record_session_end(session_id, counts)       │
  └────────────────────────────────────────────────────────────────────┘
```

### 1.4 Relationship to existing specs

- **Spec 1 (capture hooks)** — unchanged. `PreCompress / Delegation / SessionEnd` writers keep feeding `raw_memories`. `CompoundIngestor` replaces the `extract_note_updates_for_source` single-call downstream consumer.
- **Spec 2 (reflector)** — unchanged. `QueryFiler` subscribes to `MemoryReflector::reflect` results via a fire-and-forget hook in `memory_reflect` tool impl.
- **Spec 3 (fencing/modes)** — extended. `MemoryContextProvider::build_memory_user_message` is joined by `build_orientation_user_message` and `build_profile_user_message`. All three honor `injection_mode` and share the XML escape / single-fence invariant.
- **Dream Daemon** — one new stage `IndexRefresher` (idempotent `rebuild_index`), placed before `NoteLint`. `NoteSynthesis` explicitly excludes `query/` to prevent recursion.

### 1.5 Data-flow invariants (testable)

1. `notes_index + CATEGORY_DIRS → rebuild_index → parse → round-trip lossless`
2. `log.md` strictly append-only; no rewrite of historical lines
3. `apply_plan` is atomic — any failure reverts all staged page writes
4. `SCHEMA.md` / `USER.md` writes are guarded by content-hash optimistic concurrency

## 2. Orientation Layer (§2)

### 2.1 SCHEMA.md

- **Owner**: LLM writes via `wiki_schema(action, content)` tool. Code provides atomic rename + hash-check; no direct string edits.
- **Bootstrap**: first `orient` call when file missing → one-shot LLM call, system prompt has the template, user input is optional domain hint.
- **Mutation**: only through the `wiki_schema` tool. Dream Daemon proposes changes in a weekly `schema_lint` stage; never auto-applies.
- **Format**: fixed 5 sections — Domain, Categories (fixed list), Tag Taxonomy, Page Thresholds, Update Policy.
- **Categories remain fixed** (`preference | plan | learning | project | personal | tool | lesson | skill | wiki | query | other`) because Dream Daemon stages reference them. Flexibility lives in the tag taxonomy.

### 2.2 index.md

- **Owner**: code-generated from `notes_index`. LLM reads, never writes.
- **Generation**: incremental (per-write invalidate → batched flush at end of ingest tx) + periodic full rebuild by `IndexRefresher` daily stage.
- **Format**: grouped by category; one line per note — `- [[path]] — one-line summary. (updated YYYY-MM-DD)`.
- **Summary source** (three-tier fallback): ① frontmatter `summary:` field → ② first body bullet ≤ 80 chars → ③ filename humanized. `NoteLint` stage fills missing summaries via one-shot LLM call when note body > 500 chars.
- Header metadata: `<!-- auto-generated: DO NOT EDIT --> <!-- total: N | updated: ISO8601 -->`.

### 2.3 log.md

- **Owner**: code-appended, never edited.
- **Write points**: ingest, query, lint, schema, profile, session_end.
- **Format**: `## [YYYY-MM-DD HH:MM:SSZ] <action> | <summary>` + indented detail lines.
- **Rotation**: at > 2000 lines → rename to `log-YYYY-MM-DD.md`, start fresh with continuation comment.

### 2.4 Orientation injection

`MemoryContextProvider::build_orientation_user_message(agent_id) -> Option<UnifiedMessage>` emits:

```xml
<WikiOrientation>
  <schema>...</schema>
  <index_snapshot>...</index_snapshot>
  <recent_log>...last 20 log lines...</recent_log>
</WikiOrientation>
```

Injection rules honor `injection_mode`: Context/Hybrid inject on session first turn and agent switch; Tools mode does not auto-inject but exposes `wiki_orient` tool. Budget: `OrientationConfig.max_tokens = 4000`; over-budget triggers per-category top-N sampling by recency.

### 2.5 `WikiOrientation` trait

```rust
#[async_trait]
pub trait WikiOrientation: Send + Sync {
    async fn bootstrap(&self, agent_id: &str) -> Result<()>;
    async fn read_snapshot(&self, agent_id: &str, budget: TokenBudget) -> Result<OrientationSnapshot>;
    async fn record_ingest(&self, agent_id: &str, entry: LogEntry) -> Result<()>;
    async fn record_query(&self, agent_id: &str, entry: LogEntry) -> Result<()>;
    async fn record_lint(&self, agent_id: &str, entry: LogEntry) -> Result<()>;
    async fn rebuild_index(&self, agent_id: &str) -> Result<IndexStats>;
    async fn rotate_log_if_needed(&self, agent_id: &str) -> Result<()>;
    fn invalidate(&self, agent_id: &str, path: &str);
}
```

Production implementation: `FsWikiOrientation`.

## 3. Compound Ingest (§3)

### 3.1 Pipeline

```text
Phase 1: retrieve     →  related_pages (max 15)
Phase 2: plan (LLM)   →  raws + related + SCHEMA  → IngestPlan JSON
Phase 3: apply (tx)   →  staged writes → batched rename → index.rebuild
Phase 4: record       →  log.md append + index flush
```

### 3.2 Data model

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct IngestPlan {
    pub reasoning: String,
    pub ops: Vec<PageOp>,
    pub schema_proposals: Vec<SchemaProposal>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageOp {
    Create    { note_path: String, title: String, summary: String, facts: Vec<String>, links: Vec<String>, tags: Vec<String> },
    Append    { note_path: String, new_facts: Vec<String>, new_links: Vec<String> },
    Update    { note_path: String, expected_content_hash: String, new_facts: Vec<String>, reason: String },
    Contradict{ note_path: String, new_claim: String, evidence_source_ids: Vec<String> },
    Link      { from: String, to: String },
    Supersede { old_path: String, new_path: String },
}
```

### 3.3 Phase 1 — retrieve related

`gather_related` takes the aggregated raw text, embeds once, calls `NoteStore::hybrid_search_notes` (limit 12), expands 1-hop via `get_note_index` on outgoing links (+ up to 6), dedupes, ranks by score × recency, truncates to 15. Content preview caps at 800 chars per page, total prompt budget ≤ 12 KB.

### 3.4 Phase 2 — plan (LLM)

- Base prompt: `PROMPT_COMPOUND_PLAN`.
- Source-specialised suffix: when all raws share `source`, append Spec 1's `RESCUE / LESSON / DIGEST / RETRO` template.
- Inject compacted SCHEMA.md (Tag Taxonomy + Page Thresholds + Update Policy only).
- Inject related pages list with previews.
- Output: strict JSON `IngestPlan`, parsed via `extract_json_robust`.
- Key directives: lateral multi-page thinking, explicit `Contradict` not silent `Append`, each `Create` must ship ≥ 2 `Link`s, new tags go to `schema_proposals` not into ops.

### 3.5 Phase 3 — apply (transactional)

`CompoundApplyTx` stages every write to `memory/note/{agent}/.tx/{tx_id}/{category}/{filename}.md`. `expected_content_hash` mismatch triggers single re-plan with conflict info; on success all staged files batch-rename to target (order: Create → Append/Update → Link/Supersede). Any rename failure triggers reverse-rename rollback. `Drop` impl forces sync rollback on panic paths. `raw_memories.is_processed` is set only after successful commit.

### 3.6 Phase 4 — record

- log.md line: `## [timestamp] ingest | N pages touched | session=...` + indented ops list + reasoning preview (80 chars).
- `schema_proposals` logged under `proposed-tag:` bullets; never auto-applied.
- `ApplyReport { created, appended, updated, contradicted, linked, tx_id }` returned to `CompressionService`.

### 3.7 Integration with existing components

| Component | Change |
|---|---|
| `CompressionService::extract_note_updates_for_source` | deleted, replaced by `CompoundIngestor` |
| `CompressionService::compress_default_notes` | rewritten as `retrieve → plan → apply`; public API preserved |
| `ConflictDetector` (src/memory/compression/conflict.rs) | **deleted** — R8 LLM sovereignty; Contradict is an LLM-produced op |
| Spec 1 `source_prompts::prompt_for` | retained; used as PROMPT_COMPOUND_PLAN suffix |
| `FactExtractor::extract_note_updates` | retained 2 weeks with `#[deprecated]`, then removed |
| `FactExtractor::extract_facts` / `extract_unified` | removed (already legacy per NOTES.md) |

### 3.8 Concurrency

Per-agent serialisation via `tokio::sync::Mutex<AgentId>` pool; cross-agent parallelism unchanged. Batch size stays `compression_batch_size = 50`. One LLM plan call per batch. Related gather uses `tokio::join!` for hybrid search + 1-hop expand.

### 3.9 Config

```toml
[memory.compound_ingest]
enabled = true                  # kill-switch; false → fall back to legacy extract_note_updates
max_related_pages = 15
related_preview_char_cap = 800
related_total_byte_cap = 12288  # 12 KB
replan_on_hash_conflict = 1     # retry count before aborting batch
failure_cooldown_seconds = 300  # after 3 consecutive failures
tx_residue_gc_seconds = 3600    # clean .tx/ dirs older than this on startup
```

## 4. User Profile (§4)

### 4.1 USER.md

Fixed frontmatter (`schema_version`, `updated`, `revision`, `last_session`, `confidence`). Fixed body sections: **Identity**, **Communication Style**, **Motivations**, **Current Focus**, **Stance Shifts**, **Open Questions**. Rigidity is deliberate — Honcho-style flexibility is swapped for small-surface auditability.

Rules:
- Identity / Communication / Motivations — append-only plus explicit remove.
- Current Focus — whole-section replacement allowed.
- Stance Shifts — append-only, each entry dated.
- Per-section bullet cap = 20; body ≤ 2 KB.

### 4.2 Triggers

Re-uses Spec 1 SessionEnd hooks:

| Trigger | Condition | Prompt chain |
|---|---|---|
| `SessionEnd::Disconnect` | after DIGEST extract | append profile-merge stage |
| `SessionEnd::TaskDone`   | after RETRO extract  | append profile-merge stage |
| `PreCompress` / `Delegation` | **not** triggered | avoid sub-task contamination |
| Cold-start | USER.md missing at first SessionEnd | `bootstrap_profile` LLM call |

Rate limit: `profile_min_interval_minutes = 30`.

### 4.3 `ProfileSynthesizer` trait

```rust
#[async_trait]
pub trait ProfileSynthesizer: Send + Sync {
    async fn bootstrap(&self, agent_id: &str) -> Result<UserProfile>;
    async fn current(&self, agent_id: &str) -> Result<Option<UserProfile>>;
    async fn update(&self, agent_id: &str, signal: SessionSignal) -> Result<UpdateOutcome>;
}
```

### 4.4 Merge algorithm

1. Read USER.md with content hash `H0`.
2. Build LLM prompt with `<CurrentProfile>` + `<SessionSignal>` (reason, DIGEST/RETRO text, up to 6 recent user turns).
3. `PROMPT_PROFILE_MERGE` asks for JSON `{ outcome, sections, stance_shift, confidence }`.
4. Rust validator:
   - all 5 sections present,
   - ≤ 20 bullets per section,
   - Identity/Communication/Motivations: diff must be add/remove only (no rewrite),
   - `revision = old + 1`.
5. Re-read USER.md; if hash still `H0` → atomic rename write; else single re-plan; still conflicting → abort this cycle.
6. Emit `MemoryEvent::ProfileRevised { revision, diff }`; `log.md` append.

### 4.5 Injection

`build_profile_user_message` emits:

```xml
<UserProfile>
  <revision>47</revision>
  <confidence>high</confidence>
  <body>USER.md without frontmatter, ≤ 2 KB</body>
</UserProfile>
```

Injection cadence: first turn always, then every `profile_inject_interval_turns = 10`; forced injection when revision advances between scheduled points. Tools mode disables auto-inject; `user_profile(action: "read"|"history")` exposes the surface.

### 4.6 Relation to personal/ and preference/

`USER.md` is the abstract layer (style, motivation, focus). Fine-grained facts stay in `personal/*.md` and `preference/*.md`, written by `CompoundIngestor`. The merge prompt explicitly forbids storing concrete facts in USER.md; concrete items are rerouted as `CREATE_NOTE: personal/xxx` hints, pushed back into the CompoundIngestor queue.

### 4.7 Audit

Each revision emits `MemoryEvent::ProfileRevised`. `user_profile(action: "history")` replays events to reconstruct any past revision. Dream Daemon's `NoteLint` reports serious inconsistency between USER.md claims and personal/preference notes; never auto-corrects.

### 4.8 Config

```toml
[memory.profile]
enabled = true
profile_min_interval_minutes = 30
profile_inject_interval_turns = 10
max_body_bytes = 2048
max_bullets_per_section = 20
bootstrap_on_first_session_end = true
```

## 5. Query Filed-back (§5)

### 5.1 Two-tier gating

**Cheap gate** (all must hold):
- source: `memory_reflect` only (not `memory_search`);
- `synthesis.sources.len() >= 3`;
- `synthesis.text.chars().count() >= 200`;
- `query_hash` not already filed (dedup via new `query_filed` table).

**LLM gate** (if cheap gate passes): single call to `PROMPT_QUERY_FILE_CHECK` returning `{ file, reason, proposed_title, tags, links }`. Distinguishes novel synthesis from mere fact restatement.

**Explicit override**: `query_file_note(query, file_decision="force"|"skip")` tool bypasses gating — R8 LLM sovereignty.

### 5.2 `query/` category

Added to `CATEGORY_DIRS` alongside `synthesis/`. Lazy directory creation via existing `NoteIndexer::ensure_dirs`.

### 5.3 Note format

```markdown
---
category: query
title: "..."
tags: [...]
created: "YYYY-MM-DD"
updated: "YYYY-MM-DD"
query_hash: "sha256 hex"
session_id: "..."
sources: ["path/a", "path/b", "path/c"]
summary: "≤ 120 chars one-liner"
---

## Question
> original query text

## Answer
synthesis.text

## Sources
- [[path/a]]
- [[path/b]]
- [[path/c]]
```

`sources` is dual-written (frontmatter + body wikilinks) so SQL index and `NoteStore::get_outgoing_links` stay consistent.

### 5.4 `QueryFiler` trait

```rust
#[async_trait]
pub trait QueryFiler: Send + Sync {
    async fn maybe_file(
        &self,
        agent_id: &str,
        query: &str,
        synthesis: &Synthesis,
        session_id: Option<&str>,
    ) -> Result<FileOutcome>;
}

pub enum FileOutcome {
    SkippedCheapGate { reason: CheapGateReason },
    SkippedLlmGate   { reason: String },
    Filed            { note_path: String, created_at: i64 },
    AlreadyFiled     { note_path: String },
}
```

### 5.5 Trigger hook

Inside `memory_reflect` tool impl, after `MemoryReflector::reflect` returns success, `tokio::spawn` a `maybe_file` call. Errors logged as `warn!`; reflect return path never blocks.

### 5.6 Dedup table

```sql
CREATE TABLE IF NOT EXISTS query_filed (
    id          TEXT PRIMARY KEY,
    agent_id    TEXT NOT NULL DEFAULT 'default',
    query_hash  TEXT NOT NULL,
    note_path   TEXT NOT NULL,
    session_id  TEXT,
    filed_at    INTEGER NOT NULL,
    UNIQUE(agent_id, query_hash)
);
CREATE INDEX IF NOT EXISTS idx_query_filed_agent ON query_filed(agent_id);
```

### 5.7 Dream Daemon interaction

- `NoteDecay`: `query/` treated like `wiki/skill` — archive threshold `< 0.1` (looser than default `< 0.2`).
- `NoteDrift`: normal operation.
- `NoteSynthesis` (weekly): **excludes** `query/` to prevent recursion.
- `NoteConsolidate`: allowed — semantically similar queries may MERGE/ABSORB.

### 5.8 Config

```toml
[memory.query_filer]
enabled = true
min_sources = 3
min_answer_chars = 200
llm_gate_enabled = true
```

## 6. Error Handling, Migration, Testing, Cleanup

### 6.1 Error handling

| Layer | Failure | Behavior |
|---|---|---|
| Orientation read | missing files | bootstrap / rebuild_index; log continues |
| Orientation read | corrupt frontmatter | log `lint:corrupt`, inject empty snapshot, do not abort turn |
| Compound plan LLM | invalid JSON | `extract_json_robust`; still fails → batch raws not marked processed; 3 consecutive failures → 5 min cooldown |
| Compound apply | `expected_content_hash` mismatch | single re-plan with conflict info; still fails → rollback, batch retry on next tick |
| Compound apply | rename failure | reverse-rename rollback; raws stay unprocessed |
| Profile merge | invalid LLM structure | warn + abort this cycle; USER.md untouched |
| Profile merge | concurrent hash conflict | single retry; still conflict → abort |
| QueryFiler | any failure | fire-and-forget; warn; never blocks reflect return |
| All tokio tasks | panic | wrapped in `AssertUnwindSafe + catch_unwind` |

**Lock poison**: `.lock().unwrap_or_else(|e| e.into_inner())` everywhere (project convention).

**UTF-8**: `chars().take(n).collect::<String>()`, never `&s[..n]`.

### 6.2 Migration

Zero-downtime, no migration scripts.

1. SCHEMA.md missing → `bootstrap` generates default.
2. index.md missing → `rebuild_index` from SQLite.
3. log.md missing → create empty + first log line.
4. USER.md missing → next `SessionEnd` triggers `bootstrap_profile`.
5. `query/` dir missing → `ensure_dirs` creates it (after `CATEGORY_DIRS` gains the entry).
6. `query_filed` table → `init_schema` `CREATE IF NOT EXISTS` handles it.

Deprecation window: `extract_note_updates` marked `#[deprecated]` in Spec 6 PR 1; removed in Spec 6 PR 2 (two weeks later). No feature flags — config boolean `compound_ingest_enabled` toggles runtime.

### 6.3 Testing

| Level | Framework | Coverage target |
|---|---|---|
| Unit | `#[test]` / `#[tokio::test]` | ≥ 80 % per trait impl, error paths included |
| Property | `proptest` | 4 invariants — index projection reversible, log append-only, plan-apply atomicity, USER.md schema preservation |
| Snapshot | `insta` | 5 new prompts: `PROMPT_COMPOUND_PLAN`, `PROMPT_QUERY_FILE_CHECK`, `PROMPT_PROFILE_MERGE`, `PROMPT_ORIENTATION_BOOTSTRAP`, `PROMPT_PROFILE_BOOTSTRAP` |
| Integration | tmpfs + `SqliteMemoryBackend` + `RecordingMockProvider` | ≥ 2 end-to-end scenarios per sub-spec |
| Regression | existing `NoteIndexer` / `CompressionService` / Dream tests | all green, plus explicit fallback-path assertions |
| Concurrency | `loom` (existing dev feature) | `CompoundApplyTx` commit race, profile update race |

### 6.4 Cleanup

| File / symbol | Location | Reason |
|---|---|---|
| `ConflictDetector`, `ConflictConfig` | `src/memory/compression/conflict.rs` | R8 — Contradict op replaces similarity heuristic |
| `FactExtractor::extract_facts` | `src/memory/compression/extractor.rs` | CompoundIngestor takes over |
| `FactExtractor::extract_unified`, `UnifiedExtractionResponse`, `parse_unified_response` | same | legacy entity/relationship path unused |
| `ExtractedFact`, `ExtractedEntity`, `ExtractedRelationship` | same | companions of legacy path |
| `ExtractionResponse`, `get_system_prompt` | same | replaced by `PROMPT_COMPOUND_PLAN` |
| `FactExtractor::extract_note_updates`, `extract_note_updates_for_source` | same | removed after 2-week deprecation |
| `MemoryConfig.conflict_similarity_threshold` | `src/config/types/memory.rs` | removed with `ConflictDetector` |
| Legacy `dream_reports` columns (`facts_promoted`, `clusters_found`, `drift_detected`, `nodes_decayed`, `edges_decayed`, etc.) | `src/memory/store/sqlite/schema.rs` | schema version bump, collapse to note-era fields |
| `CompressionService::conflict_detector` field + wiring | `src/memory/compression/service.rs` | removed with `ConflictDetector` |

Size estimate:
- new `src/memory/wiki/` module ≈ 2500 lines + 5 prompt snapshots
- deletions ≈ 800 lines
- net ≈ +1700 code, +200 test

### 6.5 Implementation milestones — four sub-specs

```text
Spec 5: Orientation Layer (§2)   — SCHEMA.md + index.md + log.md + orientation injection
  ↓  (foundation: every later spec writes log entries)
Spec 6: Compound Ingest (§3)     — replaces single-step extractor; deletes ConflictDetector
  ↓
Spec 7: User Profile (§4)        — USER.md + ProfileSynthesizer
  ↓
Spec 8: Query Filed-back (§5)    — query/ category + QueryFiler
```

Per-spec estimates:

| Spec | Scope | Estimate |
|---|---|---|
| 5 | Orientation layer, three new markdown files, injection | ~1 week |
| 6 | Compound ingest, transactional apply, proptest heavy | ~2 weeks |
| 7 | Profile synthesizer, re-uses Spec 1 hooks | ~1 week |
| 8 | QueryFiler, single hook, gating logic | ~0.5 week |

### 6.6 Risks and mitigations

| Risk | Mitigation |
|---|---|
| LLM plan quality low → apply loops fail | 2-week `extract_note_updates` fallback; `compound_ingest_enabled` kill switch |
| index.md blows token budget | Per-category top-N sampling; token count logged per injection |
| USER.md drift from LLM misbehavior | Fixed 5 sections + bullet cap + revision events; replay via `memory_event` tool |
| Multi-agent concurrent writes | Per-agent `Mutex`; agent-isolated files eliminate contention |
| `.tx/` residue after panic | `CompressionService` startup scans `.tx/` and cleans orphan dirs older than 1 h |

### 6.7 Observability

- `tracing` spans: `wiki.orient.read`, `wiki.ingest.plan`, `wiki.ingest.apply`, `wiki.profile.update`, `wiki.query.file`.
- Every new trait entry/exit logs `info!` with `agent_id` / `session_id` and result counts.
- `log.md` itself is the human-readable runtime log (grep-friendly by design).

## 7. Sub-spec index

Each sub-spec gets its own design + plan documents in the same directory.

- [Spec 5 — Orientation Layer](./2026-04-14-memory-llm-wiki-spec5-orientation-design.md) *(TODO)*
- [Spec 6 — Compound Ingest](./2026-04-14-memory-llm-wiki-spec6-compound-ingest-design.md) *(TODO)*
- [Spec 7 — User Profile](./2026-04-14-memory-llm-wiki-spec7-user-profile-design.md) *(TODO)*
- [Spec 8 — Query Filed-back](./2026-04-14-memory-llm-wiki-spec8-query-filed-back-design.md) *(TODO)*

Each sub-spec includes: acceptance criteria, affected files, test matrix, config additions, cleanup list, rollback path.

## 8. Approval trail

- Brainstorm started 2026-04-14 via `superpowers:brainstorming`.
- Scope chosen: Plan B (Orientation + Compound + Profile + Query; defers Skill auto-creation, nudges, cross-session search).
- All six design sections approved by user in order (§1 → §6).
- Next step: spawn `superpowers:writing-plans` for Spec 5, then Spec 6 / 7 / 8 sequentially.
