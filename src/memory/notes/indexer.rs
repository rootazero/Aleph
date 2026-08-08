//! `NoteIndexer` — file I/O, full rebuild, incremental update, and rename cascade.
//!
//! Scans `memory_dir/{agent_id}/{category}/*.md` files, parses them into
//! `KnowledgeNote`s, and maintains the `SQLite` index via a `NoteStore` implementation.

use crate::sync_primitives::Arc;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::fs;

use crate::error::AlephError;
use crate::memory::dreaming::distill_action::DistillAction;
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::wikilink::{rewrite_relation_targets, rewrite_wikilinks};
use crate::memory::notes::{sanitize_title, KnowledgeNote, Severity};
use crate::utils::atomic_write::atomic_write_file;

/// All valid category subdirectories under `memory/{agent_id}/`.
///
/// This is the *single source of truth* for every indexable note category:
/// `full_rebuild`/`ensure_dirs` scan it, `note_manage::validate_category` and
/// dream `validation.rs` accept it, and the rename cascade rewrites wikilinks
/// across it. `archive/` is deliberately absent — `NoteDecay` moves cold notes
/// there and they must stay OUT of the active index.
pub const CATEGORY_DIRS: &[&str] = &[
    "preference",
    "plan",
    "learning",
    "project",
    "personal",
    "tool",
    "lesson",
    "goal-lessons", // GoalLessonsPromoteStage: per-goal lessons appended by the dream stage
    "skill",
    "reference",
    "feedback", // user-taught corrections distilled by FeedbackDistill
    "transcript",
    "subagent-run",
    "subagent-session",
    "subagent-checkpoint",
    "subagent-transcript",
    "contradiction", // Phase C2.6: note_drift conflict pages
    "entity",        // ingest entity-graph pages (`entity/<slug>`) — the ingest
    // prompt instructs the LLM to create these; without registration here
    // full_rebuild silently dropped the entire entity graph on an index rebuild
    // and dream L1 flagged every entity note "invalid category".
    "synthesis", // NoteSynthesisStage cross-note synthesis pages — likewise
    // written to disk but previously unscanned/unvalidated.
    "other",
    "query", // Spec 8: filed-back query answers
];

/// Known singular/plural (and spelling) variants → their single canonical
/// category. Explicit allow-list, NOT a generic depluralizer, so intentionally
/// plural or hyphenated categories (`goal-lessons`, the `subagent-*` family) are
/// never mangled. See [`canonicalize_category`].
const CATEGORY_ALIASES: &[(&str, &str)] = &[
    ("projects", "project"),
    ("preferences", "preference"),
    ("entities", "entity"),
    ("learnings", "learning"),
    ("lessons", "lesson"),
    ("plans", "plan"),
    ("tools", "tool"),
    ("skills", "skill"),
    ("references", "reference"),
    ("personals", "personal"),
    ("transcripts", "transcript"),
    ("contradictions", "contradiction"),
    ("queries", "query"),
    ("synthesis-notes", "synthesis"),
];

/// Canonicalize a raw, LLM-authored category string to its single canonical
/// spelling before it becomes a note-path prefix.
///
/// This is deterministic **path hygiene** (morphological normalization), NOT
/// semantic classification — it only collapses known singular/plural spelling
/// variants of the *same* category so `project`/`projects` (or
/// `workflow`/`workflows`) can never split the graph into two fragmented
/// clusters that break type-affinity relatedness, per-category synthesis
/// thresholds, distill dedup, and orientation rendering. Unknown categories
/// pass through unchanged (the LLM keeps category sovereignty — R7); we only
/// merge spellings observed to coexist in practice.
///
/// Applied at every category write chokepoint (ingest `split_path`,
/// `note_manage` create). Path-traversal sanitizing still happens downstream via
/// `sanitize_title`; this only fixes spelling. Case-insensitive on match; a
/// non-aliased category is returned byte-identical except for trimming.
#[must_use]
pub fn canonicalize_category(raw: &str) -> String {
    let trimmed = raw.trim();
    let key = trimmed.to_ascii_lowercase();
    for &(variant, canonical) in CATEGORY_ALIASES {
        if key == variant {
            return canonical.to_string();
        }
    }
    trimmed.to_string()
}

/// Statistics from an indexing operation.
#[derive(Debug, Clone, Default)]
pub struct IndexStats {
    pub indexed: usize,
    pub skipped: usize,
    pub errors: usize,
    /// Index rows removed because their backing `.md` file no longer exists on
    /// disk (orphans from a rename / deletion / agent-id relocation).
    pub pruned: usize,
    /// Notes whose vector is missing or was computed from an older version of
    /// the note (see `NoteStore::stale_vector_paths`).
    ///
    /// Embed-on-write logs and swallows its failures on purpose, so this is the
    /// only place the resulting drift becomes visible. A count equal to the
    /// note total means the vector leg is not working at all — the shape a
    /// misconfigured embedding dimension produces, which otherwise shows up
    /// only as "semantic search finds nothing".
    pub stale_vectors: usize,
}

/// Aggregate outcome of [`NoteIndexer::full_rebuild_all`].
///
/// `failed` is carried rather than folded into `total.errors` because the two
/// answer different questions: `errors` counts files that would not parse
/// (normal on a hand-edited vault), `failed` names corpora whose reconcile did
/// not run at all — the only shape that can leave a whole namespace unmaintained
/// while the summary line still looks healthy.
#[derive(Debug, Clone, Default)]
pub struct RebuildAllStats {
    /// How many corpora were attempted.
    pub corpora: usize,
    /// Sum of the per-corpus [`IndexStats`].
    pub total: IndexStats,
    /// `(corpus, error)` for each corpus whose reconcile returned `Err`.
    pub failed: Vec<(String, String)>,
}

/// What `index_one_file` actually wrote, for the callers that owe the file
/// further side-effects.
///
/// `Skipped` means the on-disk content hash already matched the index — the
/// single reason a self-write (which indexed the file the moment it wrote it)
/// costs nothing when a rebuild or the vault watcher sees it again.
enum IndexOutcome {
    /// The index row was (re-)written. Carries the file text and the parsed
    /// aliases so the caller can run the vector / inbound-link legs without
    /// reading and reparsing the file a second time.
    Indexed {
        content: String,
        aliases: Vec<String>,
    },
    Skipped,
}

/// Index a single markdown file, hashing its contents and skipping the write
/// if the existing index entry already has the same hash.
///
/// Free function (rather than a method on `NoteIndexer`) so it can be cheaply
/// called from inside spawned `tokio` tasks that own only an `Arc<S>` clone of
/// the store — avoids forcing `&self` capture into `'static` futures.
///
/// **This is the index leg only.** It deliberately does not embed and does not
/// backfill inbound links: `full_rebuild` calls it once per file across a whole
/// corpus and *reports* vector drift rather than paying to repair it, then does
/// the link pass once globally (`relink_unresolved`). Callers reconciling a
/// single file want the opposite trade — see [`NoteIndexer::index_file`].
async fn index_one_file<S: NoteStore + Send + Sync>(
    path: &Path,
    agent_id: &str,
    category: &str,
    store: Arc<S>,
) -> Result<IndexOutcome, AlephError> {
    let content = fs::read_to_string(path)
        .await
        .map_err(|e| AlephError::config(format!("index_one_file read {path:?}: {e}")))?;
    let title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| AlephError::config(format!("invalid filename {path:?}")))?;
    let hash = sha2_hash(&content);
    let key_path = format!("{category}/{title}");

    if let Some(existing) = store.get_note_index(&key_path, agent_id).await? {
        if existing.content_hash == hash {
            return Ok(IndexOutcome::Skipped);
        }
    }
    let mut note = KnowledgeNote::from_markdown(title, &content)?;
    crate::memory::notes::governance::supersession::sync_body_to_frontmatter(&mut note, &content);
    store.index_note(&note, agent_id, category).await?;
    Ok(IndexOutcome::Indexed {
        aliases: note.aliases,
        content,
    })
}

/// Indexes markdown note files into a `NoteStore`.
///
/// Generic over `S: NoteStore` so tests can swap in any backend.
pub struct NoteIndexer<S: NoteStore> {
    memory_dir: PathBuf,
    store: Arc<S>,
    /// Optional embedding provider. When present, every full note write
    /// (`write_note`, `write_note_raw`, `rename_note`) refreshes the note's
    /// vector so it becomes searchable immediately — instead of only after the
    /// next background `reembed_all` sweep. `None` is a graceful no-op (the
    /// FTS leg still works, and interactive tools that own their own embedder
    /// remain unaffected).
    embedder: Option<Arc<dyn crate::memory::embedding_provider::EmbeddingProvider>>,
}

impl<S: NoteStore> NoteIndexer<S> {
    /// Create a new indexer for the given memory directory and store.
    ///
    /// `memory_dir` should point to `~/.aleph/data/memory/` (the parent of
    /// all agent directories).
    pub fn new(memory_dir: PathBuf, store: Arc<S>) -> Self {
        Self {
            memory_dir,
            store,
            embedder: None,
        }
    }

    /// Attach an embedding provider so every full note write refreshes the
    /// note's vector on the spot (embed-on-write), making it immediately
    /// visible to hybrid/vector recall. Without it, freshly written notes are
    /// FTS-only until the next `reembed_all` sweep.
    #[must_use]
    pub fn with_embedder(
        mut self,
        embedder: Arc<dyn crate::memory::embedding_provider::EmbeddingProvider>,
    ) -> Self {
        self.embedder = Some(embedder);
        self
    }

    /// Embed `content` and upsert the note's vector so it is searchable
    /// immediately after a write. Best-effort: a missing embedder is a no-op,
    /// and an embed/upsert failure is logged but never fails the write — the
    /// note is already on disk and the `reembed_all` sweep is the safety net.
    async fn refresh_embedding(&self, agent_id: &str, category: &str, title: &str, content: &str) {
        let Some(embedder) = &self.embedder else {
            return;
        };
        if content.trim().is_empty() {
            return;
        }
        match embedder.embed(content).await {
            Ok(embedding) => {
                let dim = embedding.len() as u32;
                let note_path = format!("{category}/{title}");
                // Record which version of the note this vector came from. The
                // basis is the full file text — the same string `index_note`
                // hashes into `content_hash` — so `stale_vector_paths` compares
                // like with like.
                let embedded_hash = sha2_hash(content);
                if let Err(e) = self
                    .store
                    .upsert_embedding(&note_path, agent_id, &embedding, dim, &embedded_hash)
                    .await
                {
                    tracing::warn!(path = %note_path, error = %e, "indexer: embedding upsert failed");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "indexer: embed-on-write failed (vector index stays stale)");
            }
        }
    }

    /// Shared post-write pipeline for a full note write: reparse the on-disk
    /// markdown, sync the SQLite index, and refresh the vector
    /// (embed-on-write). Every full-write entry point (`write_note`,
    /// `write_note_raw`) funnels through here so the index / vector
    /// side-effects stay in lockstep and no writer can silently forget one.
    async fn finalize_write(
        &self,
        agent_id: &str,
        category: &str,
        safe_title: &str,
        content: &str,
    ) -> Result<(), AlephError> {
        let mut reparsed = KnowledgeNote::from_markdown(safe_title, content).map_err(|e| {
            AlephError::other(format!("reparse after write {category}/{safe_title}: {e}"))
        })?;
        // Promote body-embedded supersession (`## Superseded by [[X]]`) into
        // `superseded_by` frontmatter before indexing — mirrors `index_file` /
        // `index_one_file`, so a raw/panel-edited body reflects the relation
        // immediately instead of waiting for the next `full_rebuild`.
        crate::memory::notes::governance::supersession::sync_body_to_frontmatter(
            &mut reparsed,
            content,
        );
        self.store.index_note(&reparsed, agent_id, category).await?;

        self.finalize_side_effects(
            agent_id,
            category,
            safe_title,
            content,
            &reparsed.aliases,
            true,
        )
        .await;
        Ok(())
    }

    /// Post-index side-effects shared by every note writer: resolve other
    /// notes' dangling links that now point at this note
    /// (`backfill_inbound_links`, P7 best-effort, always applied) and — when
    /// `reembed` is set — refresh this note's vector so it is immediately
    /// searchable (embed-on-write; a no-op without an injected embedder). The
    /// append family routes through here too so appended / dream-distilled
    /// knowledge becomes vector-searchable without waiting for a rare
    /// `reembed_all` sweep. Callers pass `reembed = false` for metadata-only
    /// edits (link/relation weaving on an already-embedded note) so the
    /// zero-cost structural dream stages (`NoteWeave`) don't issue an embedding
    /// call per woven edge. Callers keep their own `index_note`; this covers
    /// only the link/vector legs.
    async fn finalize_side_effects(
        &self,
        agent_id: &str,
        category: &str,
        safe_title: &str,
        content: &str,
        aliases: &[String],
        reembed: bool,
    ) {
        // This write may resolve other notes' dangling links to this note
        // (create / recreate-after-delete) — targeted by to_raw.
        let mut keys: Vec<String> =
            vec![safe_title.to_string(), format!("{category}/{safe_title}")];
        keys.extend(aliases.iter().cloned());
        if let Err(e) = self.store.backfill_inbound_links(agent_id, &keys).await {
            tracing::warn!(error = %e, "finalize_side_effects: inbound backfill failed (non-fatal)");
        }
        if reembed {
            self.refresh_embedding(agent_id, category, safe_title, content)
                .await;
        }
    }

    /// Getter for the memory directory.
    #[must_use]
    pub fn memory_dir(&self) -> &Path {
        &self.memory_dir
    }

    /// Getter for the underlying store.
    #[must_use]
    pub const fn store(&self) -> &Arc<S> {
        &self.store
    }

    /// Ensure all category subdirectories exist for the given agent.
    pub async fn ensure_dirs(&self, agent_id: &str) -> Result<(), AlephError> {
        let agent_dir = self.memory_dir.join(agent_id);
        for cat in CATEGORY_DIRS {
            fs::create_dir_all(agent_dir.join(cat))
                .await
                .map_err(|e| AlephError::ConfigError {
                    message: format!("Failed to create {}/{cat}: {e}", agent_dir.display()),
                    suggestion: None,
                })?;
        }
        Ok(())
    }

    /// Provision an agent's category scaffold, then reconcile its index with
    /// disk. See [`Self::reconcile_corpus`] for the reconcile half.
    ///
    /// Scaffolding is a *write*: `ensure_dirs` is the only thing in the repo
    /// that materialises the 21 category directories, and it is what makes an
    /// agent's vault look provisioned in a file browser. That is right for the
    /// agent the operator is running and wrong to fan over every namespace a
    /// disk scan happens to turn up — which is why the fan-out
    /// ([`Self::full_rebuild_all`]) splits the two.
    pub async fn full_rebuild(&self, agent_id: &str) -> Result<IndexStats, AlephError>
    where
        S: 'static,
    {
        self.ensure_dirs(agent_id).await?;
        self.reconcile_corpus(agent_id).await
    }

    /// Reconcile one corpus' index with the `.md` files on disk: scan every
    /// category dir, (re-)index what changed, prune rows whose file is gone,
    /// sweep orphan vectors, report vector drift, and retry dangling links.
    ///
    /// Skips files whose `content_hash` matches the existing index entry.
    /// Creates nothing — it only ever reads the tree it is reconciling.
    ///
    /// Phase B B3: each category is scanned in its own `tokio` task so the
    /// directory walks and `SQLite` reads/writes overlap. Concurrency is bounded
    /// by `std::thread::available_parallelism()` (falling back to 1 on probing
    /// failure) — a runtime probe that avoids pulling in `num_cpus`.
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    pub async fn reconcile_corpus(&self, agent_id: &str) -> Result<IndexStats, AlephError>
    where
        S: 'static,
    {
        let parallelism = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .max(1);
        let sem = Arc::new(tokio::sync::Semaphore::new(parallelism));
        let mut set: tokio::task::JoinSet<Result<IndexStats, AlephError>> =
            tokio::task::JoinSet::new();

        for category in CATEGORY_DIRS {
            let agent_id = agent_id.to_string();
            let category = (*category).to_string();
            // rust-doctor-disable-next-line excessive-clone
            let memory_dir = self.memory_dir.clone();
            // rust-doctor-disable-next-line excessive-clone
            let store = self.store.clone();
            // rust-doctor-disable-next-line excessive-clone
            let sem = sem.clone();

            set.spawn(async move {
                let _permit = sem
                    .acquire_owned()
                    .await
                    .map_err(|e| AlephError::config(format!("full_rebuild semaphore: {e}")))?;
                let dir = memory_dir.join(&agent_id).join(&category);
                let mut local = IndexStats::default();

                let mut entries = match fs::read_dir(&dir).await {
                    Ok(rd) => rd,
                    Err(_) => return Ok(local),
                };

                while let Some(entry) = entries
                    .next_entry()
                    .await
                    .map_err(|e| AlephError::config(format!("full_rebuild read_dir: {e}")))?
                {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("md") {
                        continue;
                    }
                    // rust-doctor-disable-next-line excessive-clone
                    match index_one_file(&path, &agent_id, &category, store.clone()).await {
                        Ok(IndexOutcome::Indexed { .. }) => local.indexed += 1,
                        Ok(IndexOutcome::Skipped) => local.skipped += 1,
                        Err(e) => {
                            tracing::warn!(
                                path = %path.display(),
                                error = %e,
                                "full_rebuild: file failed"
                            );
                            local.errors += 1;
                        }
                    }
                }
                Ok(local)
            });
        }

        let mut total = IndexStats::default();
        while let Some(joined) = set.join_next().await {
            match joined.map_err(|e| AlephError::config(format!("full_rebuild join: {e}")))? {
                Ok(s) => {
                    total.indexed += s.indexed;
                    total.skipped += s.skipped;
                    total.errors += s.errors;
                }
                Err(e) => return Err(e),
            }
        }

        // Reconcile: the index must reflect disk. The scan above only adds or
        // updates rows (keyed by content_hash) — it never removed rows whose
        // backing file is gone, so entries left by a rename, deletion, or
        // agent-id relocation lingered forever (the source of the orphan rows
        // that broke reembed and surfaced as duplicates). Drop any index row
        // for this agent whose `<category>/<file>.md` no longer exists.
        //
        // Non-destructive: only rows with NO file on disk are removed — never a
        // file, never a row that still has content. Scoped to scannable
        // CATEGORY_DIRS so out-of-scope rows (e.g. `archive/`) are untouched.
        // `remove_note_index` takes the note's vector with it, so this prune
        // leaves no orphan behind; the sweep below is for the ghosts that
        // predate that behaviour.
        let agent_dir = self.memory_dir.join(agent_id);
        for entry in self.store.list_notes(agent_id).await? {
            if !CATEGORY_DIRS.contains(&entry.category.as_str()) {
                continue;
            }
            let safe_cat = sanitize_title(&entry.category).unwrap_or_else(|_| "other".to_string());
            let file =
                agent_dir
                    .join(safe_cat)
                    .join(crate::memory::notes::store::note_md_filename(
                        &entry.filename,
                    ));
            if fs::metadata(&file).await.is_err() {
                self.store.remove_note_index(&entry.path, agent_id).await?;
                total.pruned += 1;
            }
        }

        // Sweep embedding rows orphaned by historical deletes (delete paths
        // now clear vectors, but pre-existing ghosts kept occupying KNN
        // slots). Best-effort: a sweep failure must not fail the rebuild.
        match self.store.prune_orphan_vectors(agent_id).await {
            Ok(n) if n > 0 => {
                tracing::info!(pruned = n, "full_rebuild: removed orphan note vectors");
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "full_rebuild: orphan-vector sweep failed");
            }
        }

        // Report vector drift. Best-effort and read-only: the sweep never
        // repairs anything here (re-embedding a whole corpus is a cost the
        // operator asks for via `reembed_all`), it only makes the drift
        // countable instead of invisible.
        match self.store.stale_vector_paths(agent_id).await {
            Ok(paths) => {
                total.stale_vectors = paths.len();
                if !paths.is_empty() {
                    tracing::info!(
                        stale = paths.len(),
                        "full_rebuild: notes whose vector is missing or outdated"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "full_rebuild: vector staleness probe failed");
            }
        }

        // Re-resolve cross-note wikilinks. The category scan above runs each
        // category in its own task, so a note can be indexed before the note it
        // links to exists in `notes_index` — the links resolver then dangles the
        // raw wikilink text (`status = 'dangling'`). Now that every note is
        // indexed, retry those dangling edges so links resolve regardless of
        // which category task ran first.
        self.store.relink_unresolved(agent_id).await?;

        Ok(total)
    }

    /// Reconcile **every** note corpus on disk, plus `always_include` whether or
    /// not it exists yet.
    ///
    /// [`Self::full_rebuild`] answers "is this one corpus' index consistent with
    /// its files". Nothing answered "is *memory* consistent with disk": the boot
    /// pass called `full_rebuild(default_agent_id)` and stopped there, so for
    /// every other corpus — each project namespace (`{base}__proj-…`), each
    /// session scope (`{base}__u-…` / `{base}__p-…`), and every non-default
    /// agent — the index row left behind by a rename never got pruned, a
    /// `[[wikilink]]` that dangled only because of scan order never got retried,
    /// and vector drift was never even counted. Those corpora are not exotic:
    /// `project_scope::list_note_corpora` is the same enumeration the dream
    /// daemon fans its nightly maintenance over (`dreaming::maintenance_corpora`,
    /// which is this list minus the base agent — the base runs the full pipeline
    /// itself). One enumeration, two passes, so neither can cover a different set
    /// than the other. This sentence was a lie until 2026-08-08: the daemon
    /// enumerated only `{base}__…` siblings, and only behind a config flag that
    /// is off by default.
    ///
    /// `always_include` (the default agent) is unioned in and is the **only**
    /// corpus that gets scaffolded: a fresh install still gets its category
    /// directories created by `ensure_dirs`, exactly as the single-agent boot
    /// pass did, while a namespace merely discovered on disk is reconciled and
    /// not written to. A discovered corpus exists because something already
    /// wrote a note into it, and `write_note` creates the category directory it
    /// needs; materialising all 21 in every project namespace would be a new
    /// visible side-effect of what is otherwise a repair pass.
    ///
    /// Corpora are reconciled **sequentially** — each reconcile already fans out
    /// across categories up to `available_parallelism()`, so an outer fan-out
    /// would only multiply peak SQLite contention on one connection.
    /// A corpus that fails is recorded and the pass continues: one unreadable
    /// namespace must not decide whether the others get reconciled.
    pub async fn full_rebuild_all(&self, always_include: &str) -> RebuildAllStats
    where
        S: 'static,
    {
        let mut corpora = crate::memory::project_scope::list_note_corpora(&self.memory_dir);
        if !corpora.iter().any(|c| c == always_include) {
            corpora.push(always_include.to_string());
            corpora.sort();
        }

        let mut out = RebuildAllStats {
            corpora: corpora.len(),
            ..RebuildAllStats::default()
        };
        for corpus in corpora {
            let reconciled = if corpus == always_include {
                self.full_rebuild(&corpus).await
            } else {
                self.reconcile_corpus(&corpus).await
            };
            match reconciled {
                Ok(s) => {
                    out.total.indexed += s.indexed;
                    out.total.skipped += s.skipped;
                    out.total.errors += s.errors;
                    out.total.pruned += s.pruned;
                    out.total.stale_vectors += s.stale_vectors;
                }
                Err(e) => {
                    tracing::warn!(corpus = %corpus, error = %e, "full_rebuild_all: corpus failed");
                    out.failed.push((corpus, e.to_string()));
                }
            }
        }
        out
    }

    /// Reconcile a single file on disk into the index — the entry point for
    /// every writer that changed a note's bytes *without* going through
    /// [`Self::write_note`] / [`Self::write_note_raw`] (the dream stages that
    /// patch frontmatter, repair a wikilink, or stamp a supersession banner),
    /// and for the vault watcher picking up an edit made outside Aleph.
    ///
    /// Returns `Ok(true)` if the file was (re-)indexed, `Ok(false)` if skipped
    /// because the content hash is unchanged.
    ///
    /// **Runs the same post-write side-effects a first-class write does.** It
    /// used to be the index leg alone, while three call sites' comments claimed
    /// it reconciled "notes_index/FTS/embedding/tags" — so every dream cycle
    /// that lint-fixed or supersede-banner'd a note left that note's vector
    /// describing the *pre-edit* text, and the drift was visible only as a
    /// `stale_vectors` count at the next boot. A file that arrives from outside
    /// (Obsidian, an editor, a `git checkout` of the vault) has the same two
    /// needs: its vector must match its bytes, and a note that only now exists
    /// must resolve other notes' dangling `[[wikilink]]`s to it. Both legs are
    /// best-effort by contract (`finalize_side_effects` logs and continues), and
    /// both are skipped entirely on the unchanged-hash path, so a self-write
    /// that already indexed itself still costs one hash comparison.
    pub async fn index_file(
        &self,
        agent_id: &str,
        category: &str,
        path: &Path,
    ) -> Result<bool, AlephError> {
        // rust-doctor-disable-next-line excessive-clone
        let outcome = index_one_file(path, agent_id, category, self.store.clone()).await?;
        let IndexOutcome::Indexed { content, aliases } = outcome else {
            return Ok(false);
        };
        let title =
            path.file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| AlephError::ConfigError {
                    message: format!("Invalid filename: {path:?}"),
                    suggestion: None,
                })?;
        self.finalize_side_effects(agent_id, category, title, &content, &aliases, true)
            .await;
        Ok(true)
    }

    /// Write a `KnowledgeNote` to disk as a markdown file.
    ///
    /// The file is written to `{memory_dir}/{agent_id}/{category}/{title}.md`.
    /// Returns the path of the written file.
    ///
    /// The title is sanitized to prevent path traversal.
    pub async fn write_note(
        &self,
        agent_id: &str,
        category: &str,
        note: &KnowledgeNote,
    ) -> Result<PathBuf, AlephError> {
        let safe_title = sanitize_title(&note.title)?;
        let path = self
            .memory_dir
            .join(agent_id)
            .join(category)
            .join(format!("{safe_title}.md"));

        // Ensure parent dir exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| AlephError::ConfigError {
                    message: format!(
                        "Failed to create parent directory {}: {e}",
                        parent.display()
                    ),
                    suggestion: None,
                })?;
        }

        let content = note.to_markdown();
        let content = crate::memory::notes::governance::supersession::ensure_supersession_section(
            &content, note,
        );
        atomic_write_file(&path, &content).await?;

        // Sync index + vector immediately so callers don't have to
        // wait for full_rebuild / reembed_all.
        self.finalize_write(agent_id, category, &safe_title, &content)
            .await?;
        Ok(path)
    }

    /// Write RAW markdown content to a note file verbatim, then sync the index.
    ///
    /// Unlike [`write_note`], this does NOT reconstruct the file from a
    /// `KnowledgeNote` (which is lossy — `to_markdown` only re-emits frontmatter
    /// plus bullet facts plus a `Related:` line, dropping prose / headings /
    /// code blocks). The caller-supplied `content` is the full markdown
    /// (frontmatter + arbitrary body) and is written byte-for-byte, so
    /// hand-edited content survives a round-trip. Backs the panel node editor's
    /// `graph.update_note` RPC.
    ///
    /// `title` is sanitized to prevent path traversal. Returns the written path.
    pub async fn write_note_raw(
        &self,
        agent_id: &str,
        category: &str,
        title: &str,
        content: &str,
    ) -> Result<PathBuf, AlephError> {
        let safe_title = sanitize_title(title)?;
        let safe_agent = sanitize_title(agent_id)
            .unwrap_or_else(|_| crate::routing::DEFAULT_AGENT_ID.to_string());
        let safe_category = sanitize_title(category).unwrap_or_else(|_| "other".to_string());
        let path = self
            .memory_dir
            .join(safe_agent)
            .join(safe_category)
            .join(format!("{safe_title}.md"));

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| AlephError::ConfigError {
                    message: format!(
                        "Failed to create parent directory {}: {e}",
                        parent.display()
                    ),
                    suggestion: None,
                })?;
        }

        atomic_write_file(&path, content).await?;

        // Sync index + vector immediately so the graph reflects
        // the edit without waiting for the next full_rebuild. Index under the
        // path `category` (the file's physical location), mirroring `write_note`.
        self.finalize_write(agent_id, category, &safe_title, content)
            .await?;
        Ok(path)
    }

    /// Append facts and links to an existing note, or create a new one.
    ///
    /// `note_path` is `"category/filename"` (e.g. `"preference/Editor Preferences"`).
    /// Deduplicates links, bumps `updated_at`, then writes and indexes.
    pub async fn append_to_note(
        &self,
        agent_id: &str,
        note_path: &str,
        new_facts: &[String],
        new_links: &[String],
    ) -> Result<(), AlephError> {
        let (category, filename) =
            note_path
                .split_once('/')
                .ok_or_else(|| AlephError::ConfigError {
                    message: format!(
                        "Invalid note_path (expected 'category/filename'): {note_path}"
                    ),
                    suggestion: None,
                })?;
        let safe_cat = sanitize_title(category).unwrap_or_else(|_| "other".to_string());

        let safe_title = sanitize_title(filename)?;
        let file_path = self
            .memory_dir
            .join(agent_id)
            .join(&safe_cat)
            .join(format!("{safe_title}.md"));

        let existed = file_path.exists();
        let mut note = if existed {
            let content =
                fs::read_to_string(&file_path)
                    .await
                    .map_err(|e| AlephError::ConfigError {
                        message: format!("Failed to read {file_path:?}: {e}"),
                        suggestion: None,
                    })?;
            KnowledgeNote::from_markdown(filename, &content)?
        } else {
            // Ensure parent dir exists
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent).await.ok();
            }
            KnowledgeNote {
                title: filename.to_string(),
                // rust-doctor-disable-next-line excessive-clone
                category: safe_cat.clone(),
                tags: vec![],
                facts: vec![],
                links: vec![],
                created_at: chrono::Utc::now().timestamp(),
                updated_at: chrono::Utc::now().timestamp(),
                content_hash: String::new(),
                confidence: 1.0,
                severity: Severity::Low,
                source_notes: Vec::new(),
                ..Default::default()
            }
        };

        // Append facts + links through the body-sync helpers so the verbatim
        // body (prose notes) is extended rather than silently dropped.
        note.append_facts(new_facts);
        note.add_links(new_links);

        // Bump updated_at
        note.updated_at = chrono::Utc::now().timestamp();

        // Recompute hash from the new markdown content
        let md = note.to_markdown();
        note.content_hash = sha2_hash(&md);

        // Write file + index. Use the atomic helper (write-temp + rename) like
        // every other note writer (`write_note`, `write_note_raw`,
        // `merge_source_notes_into_note`) — a plain `fs::write` can leave a
        // truncated/partial markdown file (the source of truth) on a crash or
        // be observed half-written by a concurrent reader.
        atomic_write_file(&file_path, &md).await?;
        self.store.index_note(&note, agent_id, &safe_cat).await?;

        // Re-embed only when searchable content actually changed: a brand-new
        // note (created here) or real appended facts. A link-only append on an
        // already-embedded note (e.g. NoteWeave orphan-linking) skips the
        // embedding call so the zero-cost structural weave stays cost-free.
        let reembed = !existed || !new_facts.is_empty();
        self.finalize_side_effects(
            agent_id,
            &safe_cat,
            &safe_title,
            &md,
            &note.aliases,
            reembed,
        )
        .await;
        Ok(())
    }

    /// Merge typed relations into an existing note's frontmatter (deduped by
    /// (to, rel_type)), bump updated_at, rewrite + re-index. No-op when every
    /// relation already exists.
    pub async fn append_relations(
        &self,
        agent_id: &str,
        note_path: &str,
        relations: &[crate::memory::notes::Relation],
    ) -> Result<(), AlephError> {
        let (category, filename) =
            note_path
                .split_once('/')
                .ok_or_else(|| AlephError::ConfigError {
                    message: format!(
                        "Invalid note_path (expected 'category/filename'): {note_path}"
                    ),
                    suggestion: None,
                })?;
        let safe_cat = sanitize_title(category).unwrap_or_else(|_| "other".to_string());
        let safe_title = sanitize_title(filename)?;
        let file_path = self
            .memory_dir
            .join(agent_id)
            .join(&safe_cat)
            .join(format!("{safe_title}.md"));
        let content = fs::read_to_string(&file_path)
            .await
            .map_err(|e| AlephError::config(format!("append_relations read: {e}")))?;
        let mut note = KnowledgeNote::from_markdown(filename, &content)?;
        let mut added = false;
        for r in relations {
            if !note
                .relations
                .iter()
                .any(|x| x.to == r.to && x.rel_type == r.rel_type)
            {
                // rust-doctor-disable-next-line excessive-clone
                note.relations.push(r.clone().clamped());
                added = true;
            }
        }
        if !added {
            return Ok(());
        }
        note.updated_at = chrono::Utc::now().timestamp();
        let md = note.to_markdown();
        note.content_hash = sha2_hash(&md);
        atomic_write_file(&file_path, &md).await?;
        self.store.index_note(&note, agent_id, &safe_cat).await?;
        // Relations are frontmatter metadata on an already-existing (already
        // embedded) note — searchable prose is unchanged, so skip the re-embed
        // and only run the cheap inbound-link backfill.
        self.finalize_side_effects(agent_id, &safe_cat, &safe_title, &md, &note.aliases, false)
            .await;
        Ok(())
    }

    /// Delete a note: remove its index rows (including any embedding) and the
    /// markdown file. Sanitizes both path segments.
    ///
    /// Index removal runs first — if it fails the file stays put and the two
    /// remain consistent; if the file delete fails afterwards, `full_rebuild`
    /// re-indexes the surviving file (self-healing in the safe direction).
    /// A file already missing on disk is not an error (idempotent delete).
    pub async fn delete_note(
        &self,
        agent_id: &str,
        category: &str,
        filename: &str,
    ) -> Result<(), AlephError> {
        let safe_cat = sanitize_title(category).unwrap_or_else(|_| "other".to_string());
        let safe_title = sanitize_title(filename)?;
        let note_path = format!("{safe_cat}/{safe_title}");
        let file_path = self
            .memory_dir
            .join(agent_id)
            .join(&safe_cat)
            .join(format!("{safe_title}.md"));

        self.store.remove_note_index(&note_path, agent_id).await?;
        match fs::remove_file(&file_path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(AlephError::config(format!(
                    "delete_note: failed to remove {file_path:?}: {e}"
                )))
            }
        }
        Ok(())
    }

    /// Rename a note: rename file, rewrite wikilinks in all other notes,
    /// remove old index entry, and re-index affected files.
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    pub async fn rename_note(
        &self,
        agent_id: &str,
        old_title: &str,
        new_title: &str,
    ) -> Result<(), AlephError> {
        let safe_old = sanitize_title(old_title)?;
        let safe_new = sanitize_title(new_title)?;

        // Find the old note to determine its category
        let old_paths = self
            .store
            .find_by_filename(old_title, agent_id)
            .await
            .unwrap_or_default();
        let category = if let Some(first_path) = old_paths.first() {
            first_path.split('/').next().unwrap_or("other").to_string()
        } else {
            "other".to_string()
        };
        let category =
            crate::memory::notes::sanitize_title(&category).unwrap_or_else(|_| "other".to_string());

        let cat_dir = self.memory_dir.join(agent_id).join(&category);
        let old_path = cat_dir.join(format!("{safe_old}.md"));
        let new_path = cat_dir.join(format!("{safe_new}.md"));

        // Rename the file
        fs::rename(&old_path, &new_path)
            .await
            .map_err(|e| AlephError::ConfigError {
                message: format!("Failed to rename {old_path:?} → {new_path:?}: {e}"),
                suggestion: None,
            })?;

        // Remove old index entries
        for old_p in &old_paths {
            self.store.remove_note_index(old_p, agent_id).await?;
        }

        // Scan all category dirs and rewrite [[old_title]] → [[new_title]]
        let agent_dir = self.memory_dir.join(agent_id);
        for cat in CATEGORY_DIRS {
            let dir = agent_dir.join(cat);
            let mut entries = match fs::read_dir(&dir).await {
                Ok(e) => e,
                Err(_) => continue,
            };

            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                // Skip the renamed file itself — we'll index it separately below.
                if path == new_path {
                    continue;
                }

                let content = match fs::read_to_string(&path).await {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                // Rewrite BOTH body `[[old]]` wikilinks AND frontmatter typed
                // relations (`- to: old`) — the latter are bare scalars the
                // wikilink regex cannot see, so without this they dangle after
                // a rename. Composing both means the `!= content` guard below
                // also fires (and re-indexes) on a relation-only change.
                let rewritten = rewrite_relation_targets(
                    &rewrite_wikilinks(&content, old_title, new_title),
                    old_title,
                    new_title,
                );
                if rewritten != content {
                    // Write the updated content atomically — a plain fs::write
                    // can leave a truncated source-of-truth file on a crash
                    // (same rationale as `append_to_note`).
                    if let Err(e) = atomic_write_file(&path, &rewritten).await {
                        tracing::warn!(path = %path.display(), error = %e, "Failed to rewrite wikilinks");
                        continue;
                    }
                    // Re-index the affected file
                    if let Err(e) = self.index_file(agent_id, cat, &path).await {
                        tracing::warn!(path = %path.display(), error = %e, "rename: re-index after wikilink rewrite failed (index left stale)");
                    }
                }
            }
        }

        // Index the renamed file. A failure here leaves the new path unindexed
        // (the old path's row was already removed), so surface it rather than
        // swallowing it — the file is on disk and full_rebuild will recover.
        if let Err(e) = self.index_file(agent_id, &category, &new_path).await {
            tracing::warn!(path = %new_path.display(), error = %e, "rename: re-index of renamed note failed (index left stale)");
        }

        // Re-embed under the new path: `remove_note_index` dropped the old
        // path's vector, and `index_file` does not embed, so without this the
        // renamed note falls out of vector search until the next reembed sweep.
        // Guard on the embedder to skip the disk read in FTS-only deployments.
        if self.embedder.is_some() {
            if let Ok(content) = fs::read_to_string(&new_path).await {
                self.refresh_embedding(agent_id, &category, &safe_new, &content)
                    .await;
            }
        }

        // Backfill: the new name may resolve other notes' dangling links that
        // pointed at it before it existed under this title (its own aliases
        // are unchanged by a rename, and the link-rewrite cascade above
        // already re-pointed every other note's body) — P7 best-effort.
        let keys = vec![safe_new.clone(), format!("{category}/{safe_new}")];
        if let Err(e) = self.store.backfill_inbound_links(agent_id, &keys).await {
            tracing::warn!(error = %e, "rename_note: inbound backfill failed (non-fatal)");
        }

        Ok(())
    }

    /// Each newly-corroborating source fact lifts a note's confidence by this
    /// step, saturating at 1.0. Small enough that the score grows monotonically
    /// without overshooting on a single high-corroboration round.
    const STRENGTHEN_STEP: f32 = 0.05;

    /// Merge new `source_notes` into an existing note on disk and bump its
    /// confidence monotonically. Used by both `DistillAction::Strengthen`
    /// (no floor lift) and the `New`-with-collision demotion path
    /// (`confidence_floor = new_action.confidence` lifts the note's
    /// confidence to at least the LLM's latest judgment).
    ///
    /// Confidence formula:
    ///   `confidence = min(1.0, max(existing, floor) + STRENGTHEN_STEP * newly_added_notes)`
    async fn merge_source_notes_into_note(
        &self,
        agent_id: &str,
        existing_note_path: &str,
        new_source_notes: &[String],
        confidence_floor: f32,
    ) -> Result<(), AlephError> {
        let (cat, filename) =
            existing_note_path
                .split_once('/')
                .ok_or_else(|| AlephError::ConfigError {
                    message: format!(
                        "merge_source_notes: invalid note_path '{existing_note_path}' \
                     (expected 'category/filename')"
                    ),
                    suggestion: None,
                })?;
        let safe_cat = sanitize_title(cat).unwrap_or_else(|_| "other".to_string());
        let safe_title = sanitize_title(filename)?;
        let file_path = self
            .memory_dir
            .join(agent_id)
            .join(&safe_cat)
            .join(format!("{safe_title}.md"));
        if !file_path.exists() {
            return Err(AlephError::other(format!(
                "merge_source_notes: target missing on disk: {existing_note_path}"
            )));
        }
        let content =
            fs::read_to_string(&file_path)
                .await
                .map_err(|e| AlephError::ConfigError {
                    message: format!("merge read {file_path:?}: {e}"),
                    suggestion: None,
                })?;
        let mut note = KnowledgeNote::from_markdown(filename, &content)?;

        let mut added_count: u32 = 0;
        for f in new_source_notes {
            if !note.source_notes.contains(f) {
                // rust-doctor-disable-next-line excessive-clone
                note.source_notes.push(f.clone());
                added_count += 1;
            }
        }

        // Lift to the floor first (covers the New-collision case where the
        // LLM's new confidence may exceed the existing value), then add a
        // proportional bump for genuinely-new corroborating facts.
        let base = note.confidence.max(confidence_floor);
        note.confidence = (base + Self::STRENGTHEN_STEP * (added_count as f32)).min(1.0);

        note.updated_at = chrono::Utc::now().timestamp();
        let md = note.to_markdown();
        note.content_hash = sha2_hash(&md);
        atomic_write_file(&file_path, &md).await?;
        self.store.index_note(&note, agent_id, &safe_cat).await?;
        // Merges source-provenance + confidence (frontmatter metadata) into an
        // already-embedded note — searchable prose is unchanged, so skip the
        // re-embed and only run the cheap inbound-link backfill.
        self.finalize_side_effects(agent_id, &safe_cat, &safe_title, &md, &note.aliases, false)
            .await;
        Ok(())
    }

    /// Apply a `DistillAction` emitted by a Distill stage (`SkillDistill`, `FeedbackDistill`).
    ///
    /// Pure plumbing — the LLM already judged what to do; this method just executes
    /// the I/O. Phase 2 Decision 2: the candidate selection happens upstream
    /// (`find_similar_notes` → injected into the LLM prompt → LLM emits the action).
    ///
    /// `category` is the destination/source category (e.g. `"skill"` for `SkillDistill`).
    /// For `Strengthen` and `Supersede`, the category is parsed from the embedded
    /// note path so cross-category deletes work correctly.
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    pub async fn apply_distill_action(
        &self,
        agent_id: &str,
        category: &str,
        action: &DistillAction,
    ) -> Result<(), AlephError> {
        match action {
            DistillAction::New {
                title,
                rule,
                confidence,
                severity,
                source_facts,
            } => {
                // Write-time collision guard: if a note with the same safe
                // filename already exists in this (agent, category), demote
                // to Strengthen semantics rather than silently overwriting
                // (which would lose existing source_facts and stale the
                // confidence to whatever the new action emitted).
                let safe_title = sanitize_title(title)?;
                let candidate_path = format!("{category}/{safe_title}");
                if self
                    .store
                    .get_note_index(&candidate_path, agent_id)
                    .await?
                    .is_some()
                {
                    tracing::info!(
                        note_path = %candidate_path,
                        "DistillAction::New collided with existing note — demoting to Strengthen"
                    );
                    return self
                        .merge_source_notes_into_note(
                            agent_id,
                            &candidate_path,
                            source_facts,
                            *confidence,
                        )
                        .await;
                }

                let now = chrono::Utc::now().timestamp();
                let note = KnowledgeNote {
                    // rust-doctor-disable-next-line excessive-clone
                    title: title.clone(),
                    category: category.to_string(),
                    tags: vec![],
                    facts: vec![rule.clone()],
                    links: vec![],
                    created_at: now,
                    updated_at: now,
                    content_hash: String::new(),
                    confidence: *confidence,
                    severity: *severity,
                    // rust-doctor-disable-next-line excessive-clone
                    source_notes: source_facts.clone(),
                    ..Default::default()
                };
                self.write_note(agent_id, category, &note).await?;
            }
            DistillAction::Strengthen {
                existing_note_path,
                source_facts,
            } => {
                // Strengthen does not carry a confidence value — pass 0.0 as
                // floor so the bump is purely additive against the existing
                // note's confidence.
                self.merge_source_notes_into_note(agent_id, existing_note_path, source_facts, 0.0)
                    .await?;
            }
            DistillAction::Supersede {
                old_note_path,
                title,
                rule,
                confidence,
                severity,
                source_facts,
            } => {
                let (old_cat, old_filename) =
                    old_note_path
                        .split_once('/')
                        .ok_or_else(|| AlephError::ConfigError {
                            message: format!(
                                "Supersede: invalid old_note_path '{old_note_path}' \
                             (expected 'category/filename')"
                            ),
                            suggestion: None,
                        })?;
                let safe_old = sanitize_title(old_filename)?;
                // Sanitize the category segment too: `old_note_path` is
                // LLM-generated, and an unsanitized `old_cat` (e.g. "..")
                // would let the remove_file below escape the agent directory.
                // Mirrors `merge_source_notes_into_note` / `append_to_note`.
                let safe_old_cat = sanitize_title(old_cat).unwrap_or_else(|_| "other".to_string());
                let old_file = self
                    .memory_dir
                    .join(agent_id)
                    .join(&safe_old_cat)
                    .join(format!("{safe_old}.md"));
                if old_cat != category {
                    // Stages must validate `old_note_path` against their
                    // candidate set before getting here (see `referenced_path`
                    // in distill_action.rs). A cross-category supersede
                    // arriving here is either a legitimate user intent or a
                    // bypassed validation — either way it deserves visibility.
                    tracing::warn!(
                        old_path = %old_note_path,
                        old_cat,
                        new_cat = category,
                        "Supersede crosses category boundary"
                    );
                }
                if old_file.exists() {
                    fs::remove_file(&old_file)
                        .await
                        .map_err(|e| AlephError::ConfigError {
                            message: format!(
                                "Supersede: failed to remove old file {old_file:?}: {e}"
                            ),
                            suggestion: None,
                        })?;
                }
                self.store
                    .remove_note_index(old_note_path, agent_id)
                    .await?;

                let now = chrono::Utc::now().timestamp();
                let note = KnowledgeNote {
                    // rust-doctor-disable-next-line excessive-clone
                    title: title.clone(),
                    category: category.to_string(),
                    tags: vec![],
                    facts: vec![rule.clone()],
                    links: vec![],
                    created_at: now,
                    updated_at: now,
                    content_hash: String::new(),
                    confidence: *confidence,
                    severity: *severity,
                    // rust-doctor-disable-next-line excessive-clone
                    source_notes: source_facts.clone(),
                    ..Default::default()
                };
                self.write_note(agent_id, category, &note).await?;
            }
            DistillAction::Skip {
                source_fact,
                reason,
            } => {
                tracing::debug!(
                    source_fact = %source_fact,
                    reason = %reason,
                    "DistillAction::Skip"
                );
            }
        }
        Ok(())
    }
}

/// Compute SHA-256 hex digest.
///
/// `pub(crate)` so content-identity comparisons elsewhere in the memory layer
/// (e.g. the dream cycle's synthesis-churn digest) agree with the note index's
/// own `content_hash` rather than inventing a second hasher.
pub(crate) fn sha2_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests;
