# Dream Daemon

> Offline consolidation of the notes layer during user-idle windows.

## 1. Purpose

The Dream Daemon is the **offline** counterpart to the realtime compression pipeline. Where `CompressionService` promotes raw memory rows into notes as they arrive, the Dream Daemon performs maintenance that should not run on the hot path: merging near-duplicates, detecting contradictions between linked notes, synthesizing weekly insights, repairing broken wikilinks, and archiving low-activity notes. It is **not** a memory writer — every stage reshapes the existing Notes (L1) corpus (markdown files + `notes_index` / `notes_links` / `notes_fts` + embeddings), never adding new knowledge. The daemon runs only during a user-configurable idle window (default `02:00`–`05:00` local) after a minimum idle period (default 15 min); every stage is interruption-aware.

## 2. Scheduling

`DreamDaemon` lives in `src/memory/dreaming/mod.rs`. A `tokio::time::interval` ticks every `DEFAULT_CHECK_INTERVAL_SECONDS = 60` seconds, running `check_and_run` which bails out unless all preconditions pass. `ensure_dream_daemon(database, config, provider, command_handler)` is the entry point: it uses `once_cell::sync::OnceCell` to guarantee one daemon per process and no-ops in `cfg!(test)`, when memory / dreaming is disabled, when already initialized, or when no Tokio runtime is available. On success it calls `DreamDaemon::start_background_task_with_handle`.

`LAST_ACTIVITY_TS: AtomicI64` is updated by `record_activity()`; `idle_seconds()` must exceed `config.idle_threshold_seconds` (default 900 s). `is_within_window()` checks local time against `window_start_local` / `window_end_local` (defaults `02:00` / `05:00`), with explicit midnight-wrap. `determine_run_type()` returns `DreamRunType::Weekly` if `weekly_enabled` and `(now - last_run_at) / 86400 >= weekly_interval_days` (default 7); otherwise `Daily`. `check_and_run` consults `dream_status.last_run_at` and short-circuits if a `success` row exists for today; a successful run claims an `AtomicBool` (`is_running`). The run is wrapped in `tokio::time::timeout(max_duration_seconds)` (default 600 s); `last_status` transitions `running → success | error | timeout | cancelled`.

## 3. `DreamGate`

`src/memory/dreaming/gate.rs` provides a three-level cheap→expensive gate chain.

```rust
/// Configuration for the DreamGate chain.
#[derive(Debug, Clone)]
pub struct DreamGateConfig {
    /// Minimum hours that must have elapsed since the last successful
    /// consolidation before the gate will pass.  Default: 6.0
    pub min_hours: f64,
    /// Minimum number of pending facts required for consolidation to proceed.
    /// Default: 20
    pub min_pending_facts: usize,
    /// Minimum average cosine distance across pending facts required to proceed.
    /// Values below this indicate the memory landscape hasn't drifted enough.
    /// Default: 0.3
    pub drift_threshold: f32,
    /// Suggested polling interval for the background scheduler.  Default: 4 h
    pub background_interval: Duration,
}

pub enum GateResult { Pass, Blocked(BlockReason) }

pub enum BlockReason {
    TooRecent         { hours_since: f64 },    // hours_since < min_hours
    InsufficientFacts { count: usize },        // pending_facts < min_pending_facts
    LowDrift          { avg_distance: f32 },   // avg_distance < drift_threshold
    AlreadyRunning,                            // is_running.load() is true
}
```

`DreamGate::evaluate(pending_facts, avg_drift)` runs the four checks in cheap→expensive order and, on `Pass`, atomically claims `is_running`. Lifecycle: `mark_complete()` records timestamp + releases lock; `mark_failed()` releases lock only (retry fires sooner); `record_consolidation()` updates the timestamp manually.

## 4. Core Types

### 4.1 `DreamContext`

Verbatim from `src/memory/dreaming/mod.rs`:

```rust
/// Metadata for a single note in the dream pipeline.
#[derive(Debug, Clone)]
pub struct NoteEntry {
    pub path: String,
    pub category: String,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_accessed_at: Option<i64>,
    pub content_hash: String,
}

/// Context passed through the dream pipeline stages.
pub struct DreamContext {
    pub notes: Vec<NoteEntry>,
    /// Lazy-loaded note contents: path → markdown body.
    pub note_contents: HashMap<String, String>,
    pub agent_id: String,
    pub database: MemoryBackend,
    pub indexer: NoteIndexer<SqliteMemoryBackend>,
    pub provider: Arc<dyn AiProvider>,
    pub embedder: Arc<dyn EmbeddingProvider>,
    pub report: DreamReport,
    /// "daily" or "weekly"
    pub pipeline_type: String,
    /// Activity checker: returns true if user activity has been detected.
    pub activity_checker: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Run metadata for scheduling and reporting.
    pub run_metadata: DreamRunMetadata,
}
```

`DreamContext::load_content(&mut self, path)` lazy-loads markdown from `indexer.memory_dir()/{agent_id}/{category}/{filename}.md` into a `HashMap` cache — each path hits disk at most once across stages.

### 4.2 `DreamPipeline`

```rust
pub struct DreamPipeline { stages: Vec<Box<dyn DreamStage>> }

impl DreamPipeline {
    pub fn new(stages: Vec<Box<dyn DreamStage>>) -> Self { ... }
    pub fn daily()  -> Self { ... }
    pub fn weekly() -> Self { ... }
    pub async fn run(&self, mut ctx: DreamContext) -> Result<DreamReport, AlephError> { ... }
}
```

`run()` iterates stages in order. `stage.should_run(&ctx).await == false` skips silently. Before each `execute`, `(ctx.activity_checker)()` — on true, `status = Interrupted`, `interrupted_at_stage` names the aborting stage, and the pipeline returns. Otherwise the stage runs, `ctx` flows forward, and the stage name is appended to `stages_executed`.

### 4.3 `DreamStage` Trait

From `src/memory/dreaming/stages/mod.rs`:

```rust
/// A single stage in the dream pipeline.
#[async_trait]
pub trait DreamStage: Send + Sync {
    /// Human-readable name of this stage (used for logging and reports).
    fn name(&self) -> &'static str;

    /// Whether this stage should run given the current context.
    /// Returning `false` skips this stage without error.
    async fn should_run(&self, _ctx: &DreamContext) -> bool { true }

    /// Execute the stage, consuming and returning the context.
    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError>;
}
```

## 5. Stages

### 5.1 NoteConsolidate

`src/memory/dreaming/stages/note_consolidate.rs`. Merges near-duplicate notes within a category. `should_run` returns `ctx.notes.len() >= 2`. Notes are grouped by `category`; per category: `indices.len() <= 20` → `batch_consolidate_candidates` sends up to 400 chars of each note in one LLM prompt, which returns lines like `MERGE: i j`, `ABSORB_A: i j`, `ABSORB_B: i j`, or `NONE`. `indices.len() > 20` → `title_heuristic_candidates` (no LLM): a pair qualifies if one filename is a prefix of the other (shorter ≥ 70% of longer) or their common prefix covers ≥ 75% of the longer filename.

`consolidate_pair` sends both notes' first 600 chars asking for one label:

```text
MERGE      — combine both into a single note (content is too similar/redundant)
COEXIST    — keep both as separate notes (content is distinct enough)
ABSORB_A   — keep A, absorb B's unique content into A, then delete B
ABSORB_B   — keep B, absorb A's unique content into B, then delete A
```

Unrecognised replies default to `COEXIST`.

**Execution.** `execute_merge` copies both files to `.md.bak` first; notes are parsed with `KnowledgeNote::from_markdown`; unique facts / links / tags merge into the keeper; `updated_at` bumps; `NoteIndexer::write_note` + `index_file` persist and re-index. The absorbed note's index row is removed via `NoteStore::remove_note_index`, its file deleted, backups removed best-effort. On `MERGE` the earlier `created_at` wins. `DreamReport::notes_consolidated` increments per success.

### 5.2 NoteDrift

`src/memory/dreaming/stages/note_drift.rs`. Detects contradictions and stale information between wikilink-connected notes. `should_run` returns true only if some note has `updated_at > now - 7 * 86_400`. For each recently-updated note, `NoteStore::get_outgoing_links(path, agent_id)` returns raw wikilink targets; `resolve_link_path` maps each to a `ctx.notes` entry (exact path first, then filename-segment match). Self-links and unresolved targets are skipped. Both bodies (first 500 chars each) go to the LLM asking for one word:

```text
CONSISTENT    — no contradictions
CONTRADICTORY — they contain conflicting information
STALE         — Note B contains outdated information that Note A has superseded
```

`CONTRADICTORY` → `mark_contradictory` appends a `## Superseded` section to the linked note (idempotent: skipped if `## Superseded` already present). `STALE` → `mark_stale` inserts `stale: true` on a new line immediately after the opening `---` of the YAML frontmatter (idempotent: skipped if `stale:` already present or no frontmatter). `CONSISTENT` is a no-op. Every write invalidates `ctx.note_contents` for that path. Counters: `contradictions_found`, `notes_marked_stale`.

### 5.3 NoteSynthesis (Weekly)

`src/memory/dreaming/stages/note_synthesis.rs`. Weekly-only: `should_run` returns `ctx.pipeline_type == "weekly" && ctx.notes.len() >= 5`. Notes are grouped by category, **excluding `category == "synthesis"`** so output doesn't feed itself; only categories with ≥ 3 notes are synthesized. Up to 15 notes per category (300 chars each) concatenate into an LLM prompt asking for "cross-cutting themes … connections between different notes … key takeaways", with instruction to use `[[wikilinks]]` back to source paths. Output is written under the `synthesis/` category — a directory **not** in `CATEGORY_DIRS`, so the stage calls `tokio::fs::create_dir_all(memory_dir/{agent_id}/synthesis/)` before writing. Title `"{category} Synthesis"`, `tags = [<category>, "synthesis"]`, `links = [<every source path>]`, `facts = [<synthesis text>]`. `NoteIndexer::write_note` persists and indexes; `synthesis_count` increments per success.

**DBSCAN helper.** `src/memory/dreaming/stages/types.rs` ships `dbscan(points, eps, min_pts)` using cosine distance; `DreamingConfig` exposes `cluster_dbscan_eps` (default `0.3`) / `cluster_dbscan_min_samples` (default `2`). The current synthesis stage groups by category; the helper is a reusable primitive for future stages.

### 5.4 NoteLint

`src/memory/dreaming/stages/note_lint.rs`. Always runs.

**Frontmatter.** Required keys: `category`, `tags`, `created`, `updated`. `ensure_frontmatter` does string-level detection: no opening `---` → prepend minimal defaults; opening `---` without closing → malformed, skip; all four present → no-op; any missing → append missing keys with today's date, rewrite, re-index, evict cache. `format_fixed` increments per repair.

**Broken links.** For each note, `NoteStore::get_outgoing_links` returns raw targets; each is checked with `find_by_filename`. Empty → `broken_links_found` increments, then fuzzy repair: `list_notes` lists every note; case-insensitive filename comparison; exactly one match → `rewrite_wikilinks(content, old, new)` rewrites, `index_file` re-indexes, cache evicted, `links_repaired` increments. Ambiguous (≥ 2) or no matches are logged only.

### 5.5 NoteDecay

`src/memory/dreaming/stages/note_decay.rs`. Archives low-activity notes. Never deletes.

**Protection rules** (score not computed, `notes_protected` increments): (1) `now - note.created_at < 7 * 86400`; (2) `incoming_count >= 3`.

**Activity score.** For unprotected notes, `compute_score` returns:

```text
score          = access_weight * 0.4 + recency_weight * 0.3 + link_weight * 0.3
access_weight  = 1.0 if last_accessed_at.is_some() else 0.0
recency_weight = 1.0 / (1.0 + days_since_update / 30.0)
link_weight    = min(incoming_count / 3.0, 1.0)
```

`incoming_count` comes from `NoteStore::get_incoming_links(filename, agent_id)` — the links table stores raw filenames as targets, so the stage passes the filename extracted from `category/filename`.

**Threshold + destination.** `reference` / `skill` archive when `score < 0.1`; every other category when `score < 0.2`. Destination: `memory_dir/{agent_id}/archive/{category}/{filename}.md`. `tokio::fs::create_dir_all` creates the dir; `tokio::fs::rename` moves the file atomically; on success the index row is removed via `NoteStore::remove_note_index`, cache evicted, `notes_archived` increments.

**Recall-signal dependency.** `last_accessed_at` + `incoming_count` distinguish "dormant but valuable" from "abandoned"; the `recall_signals` table (§8) persists every retrieval and keeps `last_accessed_at` fresh. Forward-link: [Notes (L1)](NOTES.md) §8.

### 5.6 DailyDigest

`src/memory/dreaming/stages/daily_digest.rs`. `should_run` returns true if any note has `updated_at > now - 86400 || created_at > now - 86400`. Every note changed in the last 24 h contributes a 200-char preview; previews concatenate into a bulleted list sent to the LLM as "a concise daily activity summary (3-5 sentences) … key themes, decisions, and learnings. Write in third person." Output becomes a `DailyInsight { date, content, source_memory_count, created_at }` upserted via `DreamStore::upsert_daily_insight` (keyed by date; same-day re-runs overwrite).

## 6. Pipelines

Verbatim from `src/memory/dreaming/mod.rs`:

```rust
/// Build the standard daily pipeline.
pub fn daily() -> Self {
    Self::new(vec![
        Box::new(stages::NoteConsolidateStage), // merge first to reduce volume
        Box::new(stages::NoteDriftStage),        // detect contradictions
        Box::new(stages::NoteLintStage),         // format fixes
        Box::new(stages::NoteDecayStage),        // cleanup low-value
        Box::new(stages::DailyDigestStage),      // generate daily report
    ])
}

/// Build the weekly pipeline (daily + deep synthesis).
pub fn weekly() -> Self {
    Self::new(vec![
        Box::new(stages::NoteConsolidateStage),
        Box::new(stages::NoteDriftStage),
        Box::new(stages::NoteSynthesisStage), // weekly-only: deep synthesis
        Box::new(stages::NoteLintStage),
        Box::new(stages::NoteDecayStage),
        Box::new(stages::DailyDigestStage),
    ])
}
```

### 6.1 Daily (5 Stages)

```text
[Consolidate] -> [Drift] -> [Lint] -> [Decay] -> [DailyDigest]
```

Consolidation runs first so every downstream stage operates on a strictly smaller, de-duplicated set. Drift precedes Lint so stale markers land before frontmatter rewrites; Decay precedes Digest so the digest reflects survivors.

### 6.2 Weekly (6 Stages)

```text
[Consolidate] -> [Drift] -> [Synthesis] -> [Lint] -> [Decay] -> [DailyDigest]
                            (weekly)
```

`NoteSynthesisStage` at position 3 — after Drift (so synthesis never ingests contradicted content), before Decay (so archivable notes are still synthesizable).

## 7. `DreamReport` Schema

Verbatim from `src/memory/dreaming/report.rs`:

```rust
/// The type of dream run (daily vs weekly).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DreamRunType { Daily, Weekly }

/// Metadata about a dream pipeline run.
#[derive(Debug, Clone)]
pub struct DreamRunMetadata {
    pub run_type: DreamRunType,
    pub run_date: String,
    pub run_start_ts: i64,
}

/// Status of a completed dream pipeline run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DreamReportStatus { Completed, Interrupted, Failed }

/// Report generated by dream pipeline execution.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DreamReport {
    pub pipeline_type: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub duration_ms: u64,
    pub notes_consolidated: u32,
    pub contradictions_found: u32,
    pub notes_marked_stale: u32,
    pub synthesis_count: u32,
    pub format_fixed: u32,
    pub broken_links_found: u32,
    pub links_repaired: u32,
    pub notes_archived: u32,
    pub notes_protected: u32,
    pub errors: Option<String>,
    /// Stages that were executed during this run.
    #[serde(skip)]
    pub stages_executed: Vec<String>,
    /// Stage at which the pipeline was interrupted (if any).
    #[serde(skip)]
    pub interrupted_at_stage: Option<String>,
    /// Overall run status.
    #[serde(skip)]
    pub status: DreamReportStatus,
}
```

`DreamReportStatus` defaults to `Completed`; `DreamPipeline::run` overwrites it with `Interrupted` on activity detection (`Failed` is reserved for external error paths). `stages_executed` appends after every successful `execute`, so on interruption it lists exactly the completed stages and `interrupted_at_stage` names the stage that would have run next. Both are `#[serde(skip)]` — for logging and scheduler decisions, not the persisted row.

## 8. Persistence

All DDL from `src/memory/store/sqlite/schema.rs`; `init_schema` is idempotent.

**`dream_status`** — one-row singleton:

```sql
CREATE TABLE IF NOT EXISTS dream_status (
    id               INTEGER PRIMARY KEY CHECK (id = 1),
    last_run_at      INTEGER,
    last_status      TEXT,
    last_duration_ms INTEGER
);
```

The `CHECK (id = 1)` enforces singleton semantics. `last_status` transitions `running → success | error | timeout | cancelled` (§2).

**`dream_reports`** — one row per run (legacy columns `facts_*` / `nodes_*` / `edges_*` are holdovers from the pre-notes schema; current writers populate only `pipeline_type`, `started_at`, `finished_at`, `duration_ms`, `synthesis_count`, `errors` — all legacy columns default to 0):

```sql
CREATE TABLE IF NOT EXISTS dream_reports (
    id                    TEXT PRIMARY KEY,
    pipeline_type         TEXT NOT NULL,
    started_at            INTEGER NOT NULL,
    finished_at           INTEGER NOT NULL,
    duration_ms           INTEGER NOT NULL,
    facts_collected       INTEGER NOT NULL DEFAULT 0,
    clusters_found        INTEGER NOT NULL DEFAULT 0,
    drift_detected        INTEGER NOT NULL DEFAULT 0,
    drift_summary         TEXT,
    candidates_evaluated  INTEGER NOT NULL DEFAULT 0,
    facts_promoted        INTEGER NOT NULL DEFAULT 0,
    promotion_details     TEXT,
    facts_decayed         INTEGER NOT NULL DEFAULT 0,
    facts_pruned          INTEGER NOT NULL DEFAULT 0,
    nodes_decayed         INTEGER NOT NULL DEFAULT 0,
    edges_decayed         INTEGER NOT NULL DEFAULT 0,
    synthesis_count       INTEGER NOT NULL DEFAULT 0,
    errors                TEXT,
    namespace             TEXT NOT NULL DEFAULT 'owner'
);
CREATE INDEX IF NOT EXISTS idx_dream_reports_started ON dream_reports(started_at);
```

**`daily_insights`** — one row per date:

```sql
CREATE TABLE IF NOT EXISTS daily_insights (
    date                 TEXT PRIMARY KEY,
    content              TEXT NOT NULL,
    source_memory_count  INTEGER NOT NULL DEFAULT 0,
    created_at           INTEGER NOT NULL
);
```

**`recall_signals`** — retrieval telemetry feeding `NoteDecay`. The `fact_id` column was renamed to `note_path` by `migrate_recall_signals_note_path`, which runs inside `init_schema`:

```sql
CREATE TABLE IF NOT EXISTS recall_signals (
    id          TEXT PRIMARY KEY,
    note_path   TEXT NOT NULL,
    query_hash  TEXT NOT NULL,
    query_text  TEXT NOT NULL,
    channel     TEXT NOT NULL DEFAULT 'unknown',
    score       REAL NOT NULL,
    session_id  TEXT,
    namespace   TEXT NOT NULL DEFAULT 'owner',
    created_at  INTEGER NOT NULL,
    day_bucket  TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_recall_dedup
    ON recall_signals(note_path, query_hash, day_bucket, channel);
CREATE INDEX IF NOT EXISTS idx_recall_note_path ON recall_signals(note_path);
CREATE INDEX IF NOT EXISTS idx_recall_day_bucket ON recall_signals(day_bucket);
```

Per-day bucketing plus `(note_path, query_hash, day_bucket, channel)` dedup keeps the table bounded under repeated retrievals. Forward-link: [Notes (L1)](NOTES.md) §8.

## 9. Safety

- **Archive, never delete.** `NoteDecay` uses `tokio::fs::rename` into `archive/{category}/`; content stays recoverable by moving it back.
- **Backup before merge.** `NoteConsolidate::execute_merge` copies both files to `.md.bak` before any write; removed best-effort on success, retained on failure.
- **Interruption on activity.** `DreamPipeline::run` invokes `(ctx.activity_checker)()` before every stage; any user activity aborts at the stage boundary with `status = Interrupted`.
- **Idempotent markers.** `mark_contradictory` / `mark_stale` check for prior application before writing; re-runs do not thrash files.
- **Index-level safety net.** Every write stage re-indexes via `NoteIndexer::index_file` and evicts `ctx.note_contents` so later stages see on-disk truth.
- **Audit trail.** `dream_status` + `dream_reports` record what ran, when, how long, with what outcome.

## 10. Configuration

From `src/config/types/memory.rs`. Every key is `#[serde(default = "...")]`; defaults shown.

```toml
[memory.dreaming]
enabled                       = true       # Master on/off switch
idle_threshold_seconds        = 900        # 15 min user-idle required before run
window_start_local            = "02:00"    # Local HH:MM — wraps midnight if start > end
window_end_local              = "05:00"    # Local HH:MM
max_duration_seconds          = 600        # tokio::time::timeout wrapping the run
weekly_enabled                = true       # Promote one run/week to weekly pipeline
weekly_interval_days          = 7          # Days between weekly runs
cluster_dbscan_eps            = 0.3        # DBSCAN cosine-distance threshold (shipped helper)
cluster_dbscan_min_samples    = 2          # DBSCAN minimum samples per cluster
drift_similarity_threshold    = 0.85       # Reserved for embedding-based drift pairing
drift_max_pairs_per_run       = 20         # Cap on drift pairs per run
synthesis_min_cluster_size    = 3          # Minimum cluster size for a synthesis note
synthesis_max_insights        = 10         # Maximum synthesis notes per weekly run
```

`enabled` gates `ensure_dream_daemon`; `idle_threshold_seconds` gates entry into `run_dream`; `window_*_local` supports midnight-wrap; `max_duration_seconds` is the outer `tokio::time::timeout` (expiration → `last_status = "timeout"`); `weekly_*` control `determine_run_type`; `cluster_dbscan_*` feed the `types::dbscan` helper (§5.3); `drift_*` is reserved surface for future embedding-based pairing — the current `NoteDriftStage` walks the wikilink graph (§5.2); `synthesis_*` bound synthesis output.

## See Also

- [Notes (L1)](NOTES.md) §7 — the `NoteStore` trait the dream pipeline reshapes.
- [Raw Memory (L0)](RAW_MEMORY.md) §7.1 — realtime distillation vs offline maintenance here.
- [Retrieval](RETRIEVAL.md) §1 — downstream beneficiary of a de-duplicated, lint-clean, link-repaired note corpus.
