//! Phase 3 — transactional apply of `PageOp` sequences.
//!
//! All writes go to `memory/note/{agent}/.tx/{tx_id}/{category}/{filename}.md`
//! first. A successful commit renames the staged files to their final
//! targets in dependency order. Failures roll back by reverse-renaming
//! anything already moved.

use crate::error::AlephError;
use crate::memory::notes::canonicalize_category;
use crate::memory::notes::indexer::{NoteIndexer, CATEGORY_DIRS};
use crate::memory::notes::ingest::plan::{ApplyReport, PageOp};
use crate::memory::notes::note::{sanitize_title, KnowledgeNote, Relation};
use crate::memory::notes::store::NoteStore;
use std::collections::BTreeSet;
use std::path::PathBuf;
use uuid::Uuid;

/// Directory name, directly under an agent's note root, that holds one
/// subdirectory per in-flight apply transaction.
///
/// Single source: [`CompoundApplyTx::new`] composes the staging root from it
/// and [`sweep_tx_residue`] enumerates by it. Two literals here is how a
/// sweeper ends up scanning a directory nothing writes to.
pub const TX_DIR: &str = ".tx";

/// Delete `.tx/{id}` staging trees left behind by an apply that never reached
/// commit, rollback, or `Drop`. Returns how many were removed.
///
/// **Why this has to exist.** The transaction cleans up in three places — a
/// successful commit, a rollback, and the `Drop` impl that covers cancellation
/// — and all three run *inside the process that created the tree*. Kill the
/// server between staging and commit (`kill -9`, OOM, power loss) and the tree
/// survives every one of them, permanently. The residue is a full copy of every
/// note the batch was about to write, sitting inside the vault the product
/// tells the user to open in Obsidian, and it accumulates one tree per unlucky
/// death forever.
///
/// **Two callers, because residue outlives its owner in two ways.**
/// `full_rebuild_all` sweeps at boot, which covers the dead process — boot is
/// the first thing that happens after it. It does *not* cover the second way:
/// all three cleanup sites above `warn!` and leave the tree when
/// `remove_dir_all` itself fails, and that process keeps running.
/// `aleph-server` is a resident daemon, so "the next boot" is not a bound on
/// anything. `DefaultCompoundIngestor::try_apply` therefore sweeps before
/// staging its own tree — it wraps the only production `CompoundApplyTx::new`,
/// so "every tree we create first clears the abandoned ones beside it" holds
/// for the whole directory. Whichever comes first — the next apply in this
/// corpus or the next boot — collects the residue.
///
/// **Why an age threshold rather than "delete everything".** A live transaction
/// owns its tree while it works, and the ingest-time caller genuinely runs
/// beside one: two concurrent applies on the same corpus each sweep the other's
/// directory. `older_than_secs` is the width of the window a transaction is
/// allowed to take; an apply finishes in milliseconds, so the default hour is
/// three orders of magnitude of headroom. A tree whose mtime cannot be read is
/// **left alone** — "I could not look" is not evidence of "it is abandoned",
/// and the other branch deletes. Same rule the vault watcher applies to its own
/// stat failures.
///
/// Best-effort throughout: every failure is a named warning and the sweep
/// continues, because a residue tree that survives one boot is a disk cost, and
/// a sweep that aborts the pass around it is an index cost.
pub async fn sweep_tx_residue(
    memory_dir: &std::path::Path,
    agent_id: &str,
    older_than_secs: u64,
) -> usize {
    let tx_root = memory_dir.join(agent_id).join(TX_DIR);
    let mut entries = match tokio::fs::read_dir(&tx_root).await {
        Ok(e) => e,
        // No `.tx` directory is the normal state — this agent has either never
        // ingested or never died mid-apply. Not an error, not worth a log line.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return 0,
        Err(e) => {
            tracing::warn!(
                error = %e,
                tx_root = %tx_root.display(),
                "tx residue sweep: cannot read staging root"
            );
            return 0;
        }
    };

    let cutoff = std::time::Duration::from_secs(older_than_secs);
    let mut removed = 0usize;
    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(e)) => e,
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    tx_root = %tx_root.display(),
                    "tx residue sweep: staging root iteration failed"
                );
                break;
            }
        };
        let path = entry.path();
        let Ok(meta) = entry.metadata().await else {
            tracing::warn!(
                path = %path.display(),
                "tx residue sweep: cannot stat staging tree; leaving it"
            );
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        // `modified()` is unsupported on a handful of exotic filesystems; the
        // fail-safe direction is to keep the tree, not to delete something
        // whose age is unknown.
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(age) = modified.elapsed() else {
            // Clock moved backwards, or the tree is stamped in the future.
            // Either way its age is not a number we can act on.
            continue;
        };
        if age < cutoff {
            continue;
        }
        match tokio::fs::remove_dir_all(&path).await {
            Ok(()) => {
                removed += 1;
                tracing::info!(
                    path = %path.display(),
                    age_secs = age.as_secs(),
                    "tx residue sweep: removed abandoned staging tree"
                );
            }
            Err(e) => tracing::warn!(
                error = %e,
                path = %path.display(),
                "tx residue sweep: removal failed"
            ),
        }
    }
    removed
}

/// C2.8 origin tagging: ensure every fact line carries an inline provenance
/// marker. If the LLM already emitted one (matching the canonical regex),
/// pass through unchanged; otherwise append a permissive
/// `<!-- origin: inferred, inferred: true -->` marker so every stored fact
/// is downstream-classifiable.
/// Provenance marker for system-generated structural facts (`[title]` /
/// `[summary]` lines). They are deterministic scaffolding, not user/LLM
/// content, so they carry `origin: system` rather than falling through to
/// `Legacy`.
const SYSTEM_FACT_MARKER: &str = "<!-- origin: system, inferred: false -->";

fn ensure_origin_marker(line: &str) -> String {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"<!--\s*(?:src:[^,]+,\s*)?origin:\s*(?:raw_source|prior_note|inferred|legacy|system)\s*,\s*inferred:\s*(?:true|false)\s*-->",
        )
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap()
    });
    if RE.is_match(line) {
        line.to_string()
    } else {
        format!("{line} <!-- origin: inferred, inferred: true -->")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("hash conflict on {path}: expected {expected}, got {actual}")]
    HashConflict {
        path: String,
        expected: String,
        actual: String,
    },
    #[error("other apply error: {0}")]
    Other(#[from] AlephError),
}

struct StagedWrite {
    staged_path: PathBuf,
    target_path: PathBuf,
    category: String,
    filename: String,
    note: KnowledgeNote,
    op_label: &'static str,
}

pub struct CompoundApplyTx<'a, S: NoteStore + Send + Sync + 'static> {
    // Plain references (not `&Arc<..>`): the tx only calls methods through them,
    // never clones the Arc, so borrowing the inner value keeps the API free of
    // the caller's ownership choice. Callers holding `Arc<..>` pass `&arc` and
    // deref-coercion supplies the `&..` — existing call sites are unchanged,
    // and value-holding callers (e.g. the dream stage's `ctx.indexer`) can now
    // construct a tx to replay a deferred op.
    indexer: &'a NoteIndexer<S>,
    store: &'a S,
    agent_id: &'a str,
    memory_dir: PathBuf,
    tx_id: String,
    tx_root: PathBuf,
    staged: Vec<StagedWrite>,
    pending_links: Vec<(String, String)>,
    pending_supersedes: Vec<(String, String)>,
    batch_source_ids: Vec<String>,
    committed: bool,
}

impl<'a, S: NoteStore + Send + Sync + 'static> CompoundApplyTx<'a, S> {
    pub fn new(
        indexer: &'a NoteIndexer<S>,
        store: &'a S,
        memory_dir: impl Into<PathBuf>,
        agent_id: &'a str,
    ) -> Self {
        let memory_dir = memory_dir.into();
        let tx_id = Uuid::new_v4().to_string();
        let tx_root = memory_dir.join(agent_id).join(TX_DIR).join(&tx_id);
        Self {
            indexer,
            store,
            agent_id,
            memory_dir,
            tx_id,
            tx_root,
            staged: Vec::new(),
            pending_links: Vec::new(),
            pending_supersedes: Vec::new(),
            batch_source_ids: Vec::new(),
            committed: false,
        }
    }

    #[must_use]
    pub fn tx_id(&self) -> &str {
        &self.tx_id
    }

    /// Deterministic fallback raw-ids applied to any staged note whose op
    /// carried no `source_ids` (so the L0→L1 chain is never empty).
    #[must_use]
    pub fn with_batch_sources(mut self, ids: Vec<String>) -> Self {
        self.batch_source_ids = ids;
        self
    }

    fn resolve_sources(&self, op_ids: &[String]) -> Vec<String> {
        if op_ids.is_empty() {
            // rust-doctor-disable-next-line excessive-clone
            self.batch_source_ids.clone()
        } else {
            op_ids.to_vec()
        }
    }

    // rust-doctor-disable-next-line high-cyclomatic-complexity
    pub async fn stage(&mut self, op: &PageOp) -> Result<(), ApplyError> {
        match op {
            PageOp::Create {
                note_path,
                title,
                summary,
                facts,
                links,
                tags,
                relations,
                source_ids,
                confidence,
                severity,
            } => {
                let (category, filename) = split_path(note_path)?;
                let safe = sanitize_title(&filename)?;
                // KnowledgeNote.title is the filename (without .md), not a human title.
                // Human title + summary fold into facts so index.md picks them up.
                let mut note = KnowledgeNote {
                    // rust-doctor-disable-next-line excessive-clone
                    title: safe.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    category: category.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    tags: tags.clone(),
                    facts: facts.iter().map(|f| ensure_origin_marker(f)).collect(),
                    // rust-doctor-disable-next-line excessive-clone
                    links: links.clone(),
                    relations: relations.iter().cloned().map(Relation::clamped).collect(),
                    source_notes: self.resolve_sources(source_ids),
                    // Carry the LLM's self-assessment onto the persisted note so
                    // retrieval re-ranking (severity boost) and the governance
                    // gate see the model's judgement, not a hardcoded default.
                    confidence: confidence.clamp(0.0, 1.0),
                    severity: *severity,
                    created_at: chrono::Utc::now().timestamp(),
                    updated_at: chrono::Utc::now().timestamp(),
                    content_hash: String::new(),
                    ..Default::default()
                };
                let summary_trimmed: String = summary.chars().take(120).collect();
                if !summary_trimmed.is_empty() {
                    note.facts.insert(
                        0,
                        format!("[summary] {summary_trimmed} {SYSTEM_FACT_MARKER}"),
                    );
                }
                if !title.is_empty() && title != &safe {
                    note.facts
                        .insert(0, format!("[title] {title} {SYSTEM_FACT_MARKER}"));
                }
                // Per-fact provenance: every fact now carries a marker
                // (ensure_origin_marker stamped any the LLM omitted; the
                // synthetic title/summary lines carry origin: system).
                note.fact_provenance = note
                    .facts
                    .iter()
                    .map(|f| crate::memory::notes::note::fact_provenance_for(f))
                    .collect();
                self.push_staged(&category, &safe, note, "create").await?;
            }
            PageOp::Append {
                note_path,
                new_facts,
                new_links,
                new_relations,
                source_ids,
            } => {
                let (category, filename) = split_path(note_path)?;
                let safe = sanitize_title(&filename)?;
                let existing = self.load_existing_or_default(&category, &safe).await?;
                let mut merged = existing;
                // Route through the body-sync helpers so a prose-bearing note
                // gains the new bullets instead of losing its body on rewrite.
                let fresh_facts: Vec<String> = new_facts
                    .iter()
                    .map(|raw| ensure_origin_marker(raw))
                    .filter(|f| !merged.facts.contains(f))
                    .collect();
                merged.append_facts(&fresh_facts);
                merged.add_links(new_links);
                // Upsert typed relation edges by target: re-stated relation replaces
                // the existing one with the same `to`; new target is appended.
                for r in new_relations {
                    // rust-doctor-disable-next-line excessive-clone
                    let r = r.clone().clamped();
                    if let Some(existing) = merged.relations.iter_mut().find(|e| e.to == r.to) {
                        *existing = r;
                    } else {
                        merged.relations.push(r);
                    }
                }
                for s in self.resolve_sources(source_ids) {
                    if !merged.source_notes.contains(&s) {
                        merged.source_notes.push(s);
                    }
                }
                merged.fact_provenance = merged
                    .facts
                    .iter()
                    .map(|f| crate::memory::notes::note::fact_provenance_for(f))
                    .collect();
                merged.updated_at = chrono::Utc::now().timestamp();
                self.push_staged(&category, &safe, merged, "append").await?;
            }
            PageOp::Update {
                note_path,
                expected_content_hash,
                new_facts,
                reason: _,
            } => {
                let (category, filename) = split_path(note_path)?;
                let safe = sanitize_title(&filename)?;
                // The note is stored/indexed under the CANONICAL path
                // (`{category}/{safe}`); look up the hash there, not under the
                // raw plural/aliased `note_path`, or a canonicalized category
                // would spuriously miss the row and false-conflict.
                let canonical_path = format!("{category}/{safe}");
                let entry = self
                    .store
                    .get_note_index(&canonical_path, self.agent_id)
                    .await?;
                let actual = entry
                    .as_ref()
                    // rust-doctor-disable-next-line excessive-clone
                    .map(|e| e.content_hash.clone())
                    .unwrap_or_default();
                if &actual != expected_content_hash {
                    return Err(ApplyError::HashConflict {
                        path: canonical_path,
                        // rust-doctor-disable-next-line excessive-clone
                        expected: expected_content_hash.clone(),
                        actual,
                    });
                }
                let mut existing = self.load_existing_or_default(&category, &safe).await?;
                // Full facts replacement: the planner rewrote the bullet set,
                // so the note reverts to facts-form (legacy parity — a stale
                // verbatim body would otherwise win over the new facts).
                // rust-doctor-disable-next-line excessive-clone
                existing.facts = new_facts.clone();
                existing.body = None;
                existing.updated_at = chrono::Utc::now().timestamp();
                self.push_staged(&category, &safe, existing, "update")
                    .await?;
            }
            PageOp::Contradict {
                note_path,
                new_claim,
                evidence_source_ids,
            } => {
                let (category, filename) = split_path(note_path)?;
                let safe = sanitize_title(&filename)?;
                let mut existing = self.load_existing_or_default(&category, &safe).await?;
                let ts = chrono::Utc::now().format("%Y-%m-%d").to_string();
                let ev = if evidence_source_ids.is_empty() {
                    "".to_string()
                } else {
                    format!(" (sources: {})", evidence_source_ids.join(", "))
                };
                existing.append_facts(&[format!("[contradict {ts}] {new_claim}{ev}")]);
                existing.updated_at = chrono::Utc::now().timestamp();
                self.push_staged(&category, &safe, existing, "contradict")
                    .await?;
            }
            PageOp::Link { from, to } => {
                // Canonicalize at the queue boundary so add_link's
                // append_to_note (which re-splits with sanitize-only) writes the
                // link on the canonical note instead of a phantom plural one.
                self.pending_links
                    .push((canonicalize_note_path(from), canonicalize_note_path(to)));
            }
            PageOp::Supersede { old_path, new_path } => {
                // Canonicalize both so the read (old) and the embedded
                // `[[new_path]]` supersede pointer reference the canonical notes.
                self.pending_supersedes.push((
                    canonicalize_note_path(old_path),
                    canonicalize_note_path(new_path),
                ));
            }
        }
        Ok(())
    }

    async fn load_existing_or_default(
        &self,
        category: &str,
        filename: &str,
    ) -> Result<KnowledgeNote, ApplyError> {
        let agent_dir = self.memory_dir.join(self.agent_id);
        let disk = agent_dir.join(category).join(format!("{filename}.md"));
        if let Ok(raw) = tokio::fs::read_to_string(&disk).await {
            if let Ok(n) = KnowledgeNote::from_markdown(filename, &raw) {
                return Ok(n);
            }
        }
        Ok(KnowledgeNote {
            title: filename.to_string(),
            category: category.to_string(),
            tags: vec![],
            facts: vec![],
            links: vec![],
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
            content_hash: String::new(),
            ..Default::default()
        })
    }

    async fn push_staged(
        &mut self,
        category: &str,
        filename: &str,
        note: KnowledgeNote,
        op_label: &'static str,
    ) -> Result<(), ApplyError> {
        let staged_dir = self.tx_root.join(category);
        tokio::fs::create_dir_all(&staged_dir)
            .await
            .map_err(|e| ApplyError::Other(AlephError::other(format!("tx mkdir: {e}"))))?;
        let staged_path = staged_dir.join(format!("{filename}.md"));
        let target_path = self
            .memory_dir
            .join(self.agent_id)
            .join(category)
            .join(format!("{filename}.md"));
        let body = note.to_markdown();
        // Atomic stage: the whole apply transaction is stage-then-rename, so the
        // staged file must land atomically too — a plain write that crashes
        // mid-way leaves a half-written `.md` that `commit` then renames to the
        // target, shipping a corrupt blob as the new source-of-truth note.
        crate::utils::atomic_write::atomic_write_file(&staged_path, &body)
            .await
            .map_err(|e| ApplyError::Other(AlephError::other(format!("tx write: {e}"))))?;
        let write = StagedWrite {
            staged_path,
            target_path,
            category: category.to_string(),
            filename: filename.to_string(),
            note,
            op_label,
        };
        // Last-write-wins dedup: two ops on the same (category, filename) share
        // one staged file (the write above overwrote it). A duplicate StagedWrite
        // would make commit rename the same source twice — the second rename
        // fails with ENOENT because the first already consumed the staged file.
        if let Some(existing) = self
            .staged
            .iter_mut()
            .find(|w| w.target_path == write.target_path)
        {
            *existing = write;
        } else {
            self.staged.push(write);
        }
        Ok(())
    }

    pub async fn commit(mut self) -> Result<ApplyReport, ApplyError> {
        let mut report = ApplyReport {
            // rust-doctor-disable-next-line excessive-clone
            tx_id: self.tx_id.clone(),
            ..Default::default()
        };
        let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();

        for s in &self.staged {
            if let Some(parent) = s.target_path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    ApplyError::Other(AlephError::other(format!("mkdir target: {e}")))
                })?;
            }
            if let Err(e) = tokio::fs::rename(&s.staged_path, &s.target_path).await {
                for (from, to) in moved.iter().rev() {
                    if let Err(e) = tokio::fs::rename(to, from).await {
                        tracing::warn!(error = %e, from = %to.display(), to = %from.display(), "undo rename failed during rollback");
                    }
                }
                return Err(ApplyError::Other(AlephError::other(format!(
                    "rename {} → {}: {e}",
                    s.staged_path.display(),
                    s.target_path.display()
                ))));
            }
            // rust-doctor-disable-next-line excessive-clone
            moved.push((s.staged_path.clone(), s.target_path.clone()));

            self.store
                .index_note(&s.note, self.agent_id, &s.category)
                .await?;

            match s.op_label {
                "create" => report.created += 1,
                "append" => report.appended += 1,
                "update" => report.updated += 1,
                "contradict" => report.contradicted += 1,
                _ => {}
            }
            report
                .touched_paths
                .push(format!("{}/{}", s.category, s.filename));
        }

        for (from, to) in &self.pending_links {
            // Both directions must actually land before the link is reported.
            // `add_link` returns Ok(true) only when it appended the backlink;
            // a split_path failure, missing file, or backend error is not a
            // success — so a link that never materialized is not counted and
            // does not touch its paths. Errors stay tolerated (link failures
            // must not abort the whole commit), but they no longer masquerade
            // as successes in the report.
            let fwd = self.add_link(from, to).await.unwrap_or(false);
            let rev = self.add_link(to, from).await.unwrap_or(false);
            if fwd || rev {
                report.linked += 1;
                // rust-doctor-disable-next-line excessive-clone
                report.touched_paths.push(from.clone());
                // rust-doctor-disable-next-line excessive-clone
                report.touched_paths.push(to.clone());
            }
        }

        for (old_path, new_path) in &self.pending_supersedes {
            let _ = self.mark_superseded(old_path, new_path).await;
            report.superseded += 1;
            // rust-doctor-disable-next-line excessive-clone
            report.touched_paths.push(old_path.clone());
        }

        if let Err(e) = tokio::fs::remove_dir_all(&self.tx_root).await {
            tracing::warn!(error = %e, tx_root = %self.tx_root.display(), "tx cleanup failed");
        }

        let mut seen: BTreeSet<String> = BTreeSet::new();
        // rust-doctor-disable-next-line excessive-clone
        report.touched_paths.retain(|p| seen.insert(p.clone()));

        self.committed = true;
        Ok(report)
    }

    /// Append `link_target` as a `[[related]]` entry inside the note at
    /// `note_path`. Returns Ok(true) when the backlink was actually written,
    /// Ok(false) when the note path is unparseable or the target file does not
    /// exist (a silent skip, not an error). Callers wanting a bidirectional
    /// link call this twice with the arguments swapped — the parameter names
    /// make the direction explicit: first `(note_path: from, link_target: to)`,
    /// then `(note_path: to, link_target: from)`.
    async fn add_link(&self, note_path: &str, link_target: &str) -> Result<bool, AlephError> {
        let (category, filename) = match split_path(note_path) {
            Ok(p) => p,
            Err(_) => return Ok(false),
        };
        let safe = sanitize_title(&filename)?;
        let disk = self
            .memory_dir
            .join(self.agent_id)
            .join(&category)
            .join(format!("{safe}.md"));
        if tokio::fs::try_exists(&disk)
            .await
            .map_err(|e| AlephError::other(format!("link: stat from: {e}")))?
        {
            self.indexer
                .append_to_note(
                    self.agent_id,
                    note_path,
                    &Vec::<String>::new(),
                    &[link_target.to_string()],
                )
                .await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn mark_superseded(&self, old_path: &str, new_path: &str) -> Result<(), AlephError> {
        let (category, filename) = match split_path(old_path) {
            Ok(p) => p,
            Err(_) => return Ok(()),
        };
        let safe = sanitize_title(&filename)?;
        let disk = self
            .memory_dir
            .join(self.agent_id)
            .join(&category)
            .join(format!("{safe}.md"));
        if !tokio::fs::try_exists(&disk)
            .await
            .map_err(|e| AlephError::other(format!("supersede: stat old: {e}")))?
        {
            return Ok(());
        }
        let body = tokio::fs::read_to_string(&disk)
            .await
            .map_err(|e| AlephError::other(format!("supersede: read old: {e}")))?;
        if body.contains("## Superseded by") {
            return Ok(());
        }
        let marker = format!(
            "\n## Superseded by [[{new_path}]] ({})\n",
            chrono::Utc::now().format("%Y-%m-%d")
        );
        let combined = format!("{body}{marker}");
        // Atomic write (temp + rename) — a plain fs::write can leave a
        // truncated source-of-truth file on a crash mid-write.
        crate::utils::atomic_write::atomic_write_file(&disk, &combined)
            .await
            .map_err(|e| AlephError::other(format!("supersede: write: {e}")))?;
        if let Ok(mut n) = KnowledgeNote::from_markdown(&safe, &combined) {
            // This path calls `store.index_note` directly, bypassing the
            // indexer's normal promotion step, so promote the `## Superseded by
            // [[new]]` body heading into the `superseded_by` frontmatter list
            // ourselves — otherwise `index_note` has no list to materialize into
            // a typed edge and the supersession is never force-surfaced.
            crate::memory::notes::governance::supersession::sync_body_to_frontmatter(
                &mut n, &combined,
            );
            self.store.index_note(&n, self.agent_id, &category).await?;
        }
        Ok(())
    }

    pub async fn rollback(mut self) {
        for s in self.staged.drain(..).rev() {
            if let Err(e) = tokio::fs::remove_file(&s.staged_path).await {
                tracing::warn!(error = %e, path = %s.staged_path.display(), "rollback cleanup failed");
            }
        }
        if let Err(e) = tokio::fs::remove_dir_all(&self.tx_root).await {
            tracing::warn!(error = %e, tx_root = %self.tx_root.display(), "rollback dir cleanup failed");
        }
        self.committed = true;
    }
}

impl<'a, S: NoteStore + Send + Sync + 'static> Drop for CompoundApplyTx<'a, S> {
    fn drop(&mut self) {
        if !self.committed {
            if let Err(e) = std::fs::remove_dir_all(&self.tx_root) {
                tracing::warn!(error = %e, tx_root = %self.tx_root.display(), "drop cleanup failed");
            }
        }
    }
}

fn split_path(note_path: &str) -> Result<(String, String), ApplyError> {
    let Some((cat, name)) = note_path.split_once('/') else {
        return Err(ApplyError::Other(AlephError::other(format!(
            "invalid note_path '{note_path}' — expected 'category/filename'"
        ))));
    };
    // Canonicalize the LLM-authored category prefix (plural→singular spelling
    // merge) BEFORE sanitizing for path-safety, so `projects/foo` and
    // `project/foo` land in the same category dir instead of fragmenting the
    // graph. Ingest is the only write path that accepts a free category prefix
    // (note_manage validates against CATEGORY_DIRS), so this is the chokepoint.
    let canonical = canonicalize_category(cat);
    let safe_cat = sanitize_title(&canonical).map_err(|e| {
        ApplyError::Other(AlephError::other(format!(
            "invalid category in note_path '{note_path}': {e}"
        )))
    })?;
    if !CATEGORY_DIRS.contains(&safe_cat.as_str()) {
        return Err(ApplyError::Other(AlephError::other(format!(
            "unknown category '{safe_cat}' in note_path '{note_path}'"
        ))));
    }
    Ok((safe_cat, name.to_string()))
}

/// Canonicalize the category prefix of a full `category/filename` note path,
/// leaving the filename untouched (`projects/foo` → `project/foo`).
///
/// `split_path` canonicalizes the category it *derives*, but several op paths
/// forward the RAW `note_path` string to downstream calls that re-split it with
/// sanitize-only semantics (`append_to_note`, `get_note_index`) or embed it in a
/// wikilink marker. Canonicalizing those raw strings at the op boundary keeps
/// the ingest chokepoint complete, so a plural/aliased path never spawns a
/// phantom note in a dead category dir or dangles a supersede pointer.
fn canonicalize_note_path(note_path: &str) -> String {
    match note_path.split_once('/') {
        Some((cat, name)) => format!("{}/{}", canonicalize_category(cat), name),
        None => note_path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::notes::indexer::NoteIndexer;
    use crate::memory::store::sqlite::SqliteMemoryBackend;
    use crate::sync_primitives::Arc;

    #[test]
    fn ensure_origin_marker_idempotent_when_present() {
        let s = "- claim <!-- src: raw/x, origin: raw_source, inferred: false -->";
        assert_eq!(ensure_origin_marker(s), s);
    }

    #[test]
    fn ensure_origin_marker_patches_missing_to_inferred() {
        let s = "- bare claim";
        assert_eq!(
            ensure_origin_marker(s),
            "- bare claim <!-- origin: inferred, inferred: true -->"
        );
    }

    #[test]
    fn ensure_origin_marker_idempotent_for_inferred_origin() {
        let s = "- gist <!-- origin: inferred, inferred: true -->";
        assert_eq!(ensure_origin_marker(s), s);
    }

    async fn fresh() -> (
        tempfile::TempDir,
        Arc<SqliteMemoryBackend>,
        Arc<NoteIndexer<SqliteMemoryBackend>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = Arc::new(NoteIndexer::new(dir.path().join("note"), backend.clone()));
        (dir, backend, indexer)
    }

    #[tokio::test]
    async fn create_op_writes_file_and_indexes() {
        let (dir, backend, indexer) = fresh().await;
        let mut tx = CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
        tx.stage(&PageOp::Create {
            note_path: "learning/tokio".into(),
            title: "Tokio".into(),
            summary: "Async runtime".into(),
            facts: vec!["event-driven".into()],
            links: vec!["learning/rust-async".into()],
            tags: vec!["rust".into()],
            relations: vec![],
            source_ids: vec![],
            confidence: 1.0,
            severity: Default::default(),
        })
        .await
        .unwrap();
        let report = tx.commit().await.unwrap();
        assert_eq!(report.created, 1);
        assert!(dir.path().join("note/default/learning/tokio.md").exists());
        let listed = backend.list_notes("default").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, "learning/tokio");
    }

    #[test]
    fn canonicalize_note_path_only_touches_category_prefix() {
        assert_eq!(canonicalize_note_path("projects/foo"), "project/foo");
        assert_eq!(canonicalize_note_path("project/foo"), "project/foo");
        // Filename is untouched (even if it contains no further slash logic).
        assert_eq!(
            canonicalize_note_path("entities/my-thing"),
            "entity/my-thing"
        );
        // Pathological / prefix-less inputs pass through unchanged.
        assert_eq!(canonicalize_note_path("noslash"), "noslash");
    }

    #[tokio::test]
    async fn link_op_canonicalizes_plural_from_no_phantom_note() {
        let (dir, backend, indexer) = fresh().await;
        let mut tx = CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
        // Real note lives under the canonical singular category.
        tx.stage(&PageOp::Create {
            note_path: "project/foo".into(),
            title: "Foo".into(),
            summary: String::new(),
            facts: vec!["fact".into()],
            links: vec![],
            tags: vec![],
            relations: vec![],
            source_ids: vec![],
            confidence: 1.0,
            severity: Default::default(),
        })
        .await
        .unwrap();
        // A link referencing the PLURAL category must resolve onto the canonical
        // note — NOT spawn a phantom `projects/foo` in a dead (unscanned) dir and
        // lose the edge (the regression the review caught).
        tx.stage(&PageOp::Link {
            from: "projects/foo".into(),
            to: "reference/bar".into(),
        })
        .await
        .unwrap();
        tx.commit().await.unwrap();

        assert!(dir.path().join("note/default/project/foo.md").exists());
        assert!(
            !dir.path().join("note/default/projects/foo.md").exists(),
            "plural link `from` must not create a phantom note dir"
        );
        let listed = backend.list_notes("default").await.unwrap();
        assert!(
            listed.iter().all(|n| n.category != "projects"),
            "no row should be indexed under the dead plural category: {listed:?}"
        );
        // The canonical note actually received the link.
        let body = tokio::fs::read_to_string(dir.path().join("note/default/project/foo.md"))
            .await
            .unwrap();
        assert!(
            body.contains("[[reference/bar]]"),
            "canonical note should carry the woven link: {body}"
        );
    }

    #[tokio::test]
    async fn update_rejects_stale_hash() {
        let (dir, backend, indexer) = fresh().await;
        {
            let mut tx =
                CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
            tx.stage(&PageOp::Create {
                note_path: "learning/tokio".into(),
                title: "Tokio".into(),
                summary: "v0".into(),
                facts: vec![],
                links: vec!["learning/rust-async".into()],
                tags: vec![],
                relations: vec![],
                source_ids: vec![],
                confidence: 1.0,
                severity: Default::default(),
            })
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }

        let mut tx = CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
        let err = tx
            .stage(&PageOp::Update {
                note_path: "learning/tokio".into(),
                expected_content_hash: "deadbeef".into(),
                new_facts: vec!["v2".into()],
                reason: "test".into(),
            })
            .await
            .unwrap_err();
        match err {
            ApplyError::HashConflict { .. } => {}
            other => panic!("expected HashConflict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rollback_removes_staged_files() {
        let (dir, backend, indexer) = fresh().await;
        let mut tx = CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
        tx.stage(&PageOp::Create {
            note_path: "learning/x".into(),
            title: "X".into(),
            summary: "".into(),
            facts: vec![],
            links: vec![],
            tags: vec![],
            relations: vec![],
            source_ids: vec![],
            confidence: 1.0,
            severity: Default::default(),
        })
        .await
        .unwrap();
        let tx_id = tx.tx_id().to_string();
        tx.rollback().await;
        let tx_dir = dir.path().join(format!("note/default/.tx/{tx_id}"));
        assert!(!tx_dir.exists());
        assert!(!dir.path().join("note/default/learning/x.md").exists());
    }

    #[tokio::test]
    async fn append_merges_without_duplicates() {
        let (dir, backend, indexer) = fresh().await;
        let mut tx = CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
        tx.stage(&PageOp::Create {
            note_path: "learning/tokio".into(),
            title: "Tokio".into(),
            summary: "".into(),
            facts: vec!["fact-a".into()],
            links: vec!["learning/rust-async".into()],
            tags: vec![],
            relations: vec![],
            source_ids: vec![],
            confidence: 1.0,
            severity: Default::default(),
        })
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let mut tx = CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
        tx.stage(&PageOp::Append {
            note_path: "learning/tokio".into(),
            new_facts: vec!["fact-a".into(), "fact-b".into()],
            new_links: vec![],
            new_relations: vec![],
            source_ids: vec![],
        })
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let body = tokio::fs::read_to_string(dir.path().join("note/default/learning/tokio.md"))
            .await
            .unwrap();
        assert_eq!(body.matches("- fact-a").count(), 1);
        assert_eq!(body.matches("- fact-b").count(), 1);
    }

    #[tokio::test]
    async fn create_persists_frontmatter_relations() {
        use crate::memory::notes::note::Relation;
        let (dir, backend, indexer) = fresh().await;
        let mut tx = CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
        tx.stage(&PageOp::Create {
            note_path: "entity/alice".into(),
            title: "Alice".into(),
            summary: "".into(),
            facts: vec![],
            links: vec![],
            tags: vec![],
            relations: vec![Relation {
                to: "entity/acme".into(),
                rel_type: "works_at".into(),
                confidence: 0.9,
            }],
            source_ids: vec![],
            confidence: 1.0,
            severity: Default::default(),
        })
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let body = tokio::fs::read_to_string(dir.path().join("note/default/entity/alice.md"))
            .await
            .unwrap();
        assert!(
            body.contains("relations:"),
            "expected 'relations:' block in frontmatter"
        );
        assert!(
            body.contains("type: works_at"),
            "expected 'type: works_at' in frontmatter"
        );
    }

    #[tokio::test]
    async fn append_merges_relations_by_target() {
        use crate::memory::notes::note::Relation;
        let (dir, backend, indexer) = fresh().await;

        // Create with an initial relation: alice -knows-> bob.
        {
            let mut tx =
                CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
            tx.stage(&PageOp::Create {
                note_path: "entity/alice".into(),
                title: "Alice".into(),
                summary: "".into(),
                facts: vec![],
                links: vec![],
                tags: vec![],
                relations: vec![Relation {
                    to: "entity/bob".into(),
                    rel_type: "knows".into(),
                    confidence: 0.5,
                }],
                source_ids: vec![],
                confidence: 1.0,
                severity: Default::default(),
            })
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }

        // Append: upgrade bob edge to "colleague" and add acme edge.
        {
            let mut tx =
                CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
            tx.stage(&PageOp::Append {
                note_path: "entity/alice".into(),
                new_facts: vec![],
                new_links: vec![],
                new_relations: vec![
                    Relation {
                        to: "entity/bob".into(),
                        rel_type: "colleague".into(),
                        confidence: 0.8,
                    },
                    Relation {
                        to: "entity/acme".into(),
                        rel_type: "works_at".into(),
                        confidence: 0.9,
                    },
                ],
                source_ids: vec![],
            })
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }

        let body = tokio::fs::read_to_string(dir.path().join("note/default/entity/alice.md"))
            .await
            .unwrap();
        // bob edge should be upgraded to "colleague"
        assert!(
            body.contains("type: colleague"),
            "expected 'type: colleague' (bob edge upgraded)"
        );
        // old "knows" type should be gone (replaced by "colleague" for same `to`)
        assert!(
            !body.contains("type: knows"),
            "old 'type: knows' should have been replaced"
        );
        // new acme edge should be present
        assert!(
            body.contains("to: entity/acme"),
            "expected 'to: entity/acme' (new edge)"
        );
    }

    use proptest::prelude::*;

    fn op_strategy() -> impl Strategy<Value = PageOp> {
        let name = "[a-z][a-z0-9-]{0,8}";
        let path = (name, name).prop_map(|(c, n)| format!("{c}/{n}"));
        prop_oneof![
            path.clone().prop_flat_map(|p| {
                let p2 = p.clone();
                ("[a-z ]{3,20}", "[a-z ]{1,40}").prop_map(move |(t, s)| PageOp::Create {
                    note_path: p2.clone(),
                    title: t,
                    summary: s,
                    facts: vec![],
                    links: vec!["seed/link".to_string()],
                    tags: vec![],
                    relations: vec![],
                    source_ids: vec![],
                    confidence: 1.0,
                    severity: Default::default(),
                })
            }),
            path.clone().prop_map(|p| PageOp::Append {
                note_path: p,
                new_facts: vec!["f".into()],
                new_links: vec![],
                new_relations: vec![],
                source_ids: vec![],
            }),
            (path.clone(), path)
                .prop_filter("distinct endpoints", |(a, b)| a != b)
                .prop_map(|(from, to)| PageOp::Link { from, to }),
        ]
    }

    proptest! {
        #[test]
        fn apply_commit_produces_files_on_disk(
            ops in proptest::collection::vec(op_strategy(), 0..6)
        ) {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result: std::result::Result<(), proptest::test_runner::TestCaseError> = rt.block_on(async move {
                let (dir, backend, indexer) = fresh().await;
                let mut tx = CompoundApplyTx::new(
                    &indexer,
                    &backend,
                    dir.path().join("note"),
                    "default",
                );
                let mut expect_paths: std::collections::BTreeSet<String> =
                    std::collections::BTreeSet::new();
                for op in &ops {
                    if tx.stage(op).await.is_err() {
                        return Ok(());
                    }
                    if matches!(op, PageOp::Create { .. } | PageOp::Append { .. }) {
                        expect_paths.insert(op.primary_path().to_string());
                    }
                }
                let report = tx.commit().await;
                prop_assert!(report.is_ok(), "commit failed: {:?}", report);
                for p in expect_paths {
                    let (cat, name) = p.split_once('/').unwrap();
                    let safe_name: String = name
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                        .collect();
                    let file = dir.path().join(format!("note/default/{cat}/{safe_name}.md"));
                    prop_assert!(file.exists(), "missing file {file:?}");
                }
                Ok(())
            });
            result?;
        }
    }

    #[tokio::test]
    async fn create_populates_source_notes_from_op_ids() {
        let (dir, backend, indexer) = fresh().await;
        let mut tx = CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
        tx.stage(&PageOp::Create {
            note_path: "preference/typescript".into(),
            title: "TypeScript".into(),
            summary: "".into(),
            facts: vec!["The user prefers TypeScript.".into()],
            links: vec![],
            tags: vec![],
            relations: vec![],
            source_ids: vec!["raw-A".into(), "raw-B".into()],
            confidence: 1.0,
            severity: Default::default(),
        })
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let body =
            tokio::fs::read_to_string(dir.path().join("note/default/preference/typescript.md"))
                .await
                .unwrap();
        assert!(
            body.contains("source_notes:"),
            "frontmatter must carry source_notes"
        );
        assert!(body.contains("raw-A") && body.contains("raw-B"));

        // Order-independent assertion (no Task 6 dependency):
        let n =
            crate::memory::notes::note::KnowledgeNote::from_markdown("typescript", &body).unwrap();
        assert_eq!(
            n.source_notes,
            vec!["raw-A".to_string(), "raw-B".to_string()]
        );
    }

    #[tokio::test]
    async fn create_falls_back_to_batch_sources_when_op_ids_empty() {
        let (dir, backend, indexer) = fresh().await;
        let mut tx = CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default")
            .with_batch_sources(vec!["raw-batch-1".into()]);
        tx.stage(&PageOp::Create {
            note_path: "learning/x".into(),
            title: "X".into(),
            summary: "".into(),
            facts: vec!["fact".into()],
            links: vec![],
            tags: vec![],
            relations: vec![],
            source_ids: vec![], // LLM omitted → fall back to batch
            confidence: 1.0,
            severity: Default::default(),
        })
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let body = tokio::fs::read_to_string(dir.path().join("note/default/learning/x.md"))
            .await
            .unwrap();
        // Order-independent assertion (no Task 6 dependency):
        let n = crate::memory::notes::note::KnowledgeNote::from_markdown("x", &body).unwrap();
        assert!(
            n.source_notes.contains(&"raw-batch-1".to_string()),
            "expected source_notes to contain 'raw-batch-1', got: {:?}",
            n.source_notes,
        );
    }

    #[tokio::test]
    async fn create_stamps_system_provenance_on_title_and_summary() {
        use crate::memory::notes::note::ProvenanceOrigin;
        let (dir, backend, indexer) = fresh().await;
        let mut tx = CompoundApplyTx::new(&indexer, &backend, dir.path().join("note"), "default");
        tx.stage(&PageOp::Create {
            note_path: "learning/tokio".into(),
            title: "Tokio".into(),
            summary: "Async runtime".into(),
            facts: vec!["event-driven".into()],
            links: vec![],
            tags: vec![],
            relations: vec![],
            source_ids: vec![],
            confidence: 1.0,
            severity: Default::default(),
        })
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let body = tokio::fs::read_to_string(dir.path().join("note/default/learning/tokio.md"))
            .await
            .unwrap();
        let n = crate::memory::notes::note::KnowledgeNote::from_markdown("tokio", &body).unwrap();

        // The synthetic [title]/[summary] lines must be System origin, never Legacy.
        let system = n
            .fact_provenance
            .iter()
            .filter(|p| p.origin == ProvenanceOrigin::System)
            .count();
        assert_eq!(
            system, 2,
            "title + summary must be System origin, got {:?}",
            n.fact_provenance
        );
        assert!(
            !n.fact_provenance
                .iter()
                .any(|p| p.origin == ProvenanceOrigin::Legacy),
            "no fact should fall through to Legacy after stamping, got {:?}",
            n.fact_provenance
        );
    }
}

#[cfg(test)]
mod residue_tests {
    use super::{sweep_tx_residue, TX_DIR};
    use std::time::{Duration, SystemTime};

    /// Backdate a directory's mtime so the sweep sees it as abandoned. Uses
    /// `filetime` on the directory itself — the sweep reads `metadata.modified`
    /// of the tree root, which is what a real orphan carries: the moment its
    /// process died.
    fn backdate(path: &std::path::Path, secs: u64) {
        let when = SystemTime::now() - Duration::from_secs(secs);
        filetime::set_file_mtime(path, filetime::FileTime::from_system_time(when)).unwrap();
    }

    /// The defect this closed: cleanup lives in commit, rollback and `Drop` —
    /// all three inside the process that staged the tree. Kill that process and
    /// the tree outlives every one of them, and until this function existed
    /// nothing else in the repo so much as named `.tx`.
    #[tokio::test]
    async fn an_abandoned_staging_tree_is_removed_and_a_live_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let tx_root = dir.path().join("agent").join(TX_DIR);
        let dead = tx_root.join("dead-tx");
        let live = tx_root.join("live-tx");
        std::fs::create_dir_all(dead.join("learning")).unwrap();
        std::fs::create_dir_all(live.join("learning")).unwrap();
        std::fs::write(dead.join("learning/a.md"), "staged").unwrap();
        std::fs::write(live.join("learning/b.md"), "staged").unwrap();
        backdate(&dead, 7_200);

        let removed = sweep_tx_residue(dir.path(), "agent", 3_600).await;

        assert_eq!(removed, 1);
        assert!(!dead.exists(), "an hours-old staging tree is residue");
        assert!(
            live.exists(),
            "a tree younger than the ceiling may belong to a transaction still \
             working; deleting it would corrupt a live apply"
        );
    }

    /// No `.tx` directory is the normal state for an agent that has never died
    /// mid-ingest. It must be silent, not an error and not a warning: this runs
    /// once per corpus on every boot *and* once before every apply.
    #[tokio::test]
    async fn a_corpus_that_never_staged_anything_sweeps_to_zero() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("agent/learning")).unwrap();
        assert_eq!(sweep_tx_residue(dir.path(), "agent", 3_600).await, 0);
    }

    /// A stray file under `.tx/` is not a staging tree. Removing "everything
    /// under .tx" would be a wider promise than the one this function makes.
    #[tokio::test]
    async fn a_stray_file_under_the_staging_root_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let tx_root = dir.path().join("agent").join(TX_DIR);
        std::fs::create_dir_all(&tx_root).unwrap();
        let stray = tx_root.join("notes.txt");
        std::fs::write(&stray, "not a tx").unwrap();
        backdate(&stray, 7_200);

        assert_eq!(sweep_tx_residue(dir.path(), "agent", 3_600).await, 0);
        assert!(stray.exists());
    }

    /// The staging root the constructor composes and the one the sweep walks
    /// must be the same directory. Two literals is how a sweeper ends up
    /// cleaning a path nothing writes to — reporting zero forever while the
    /// residue piles up next door. Source-level because at runtime a sweep of
    /// the wrong directory and a sweep of an empty one are the same reading.
    ///
    /// Comment lines are stripped first: a doc comment naming the directory is
    /// documentation, not a second spelling, and a scanner that cannot tell
    /// them apart reports on prose.
    #[test]
    fn the_directory_name_is_spelled_exactly_once() {
        let src = include_str!("apply.rs");
        let production = src.split("#[cfg(test)]").next().unwrap();
        let code: String = production
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !t.starts_with("//") && !t.starts_with("*")
            })
            .collect::<Vec<_>>()
            .join("\n");

        let decl = "const TX_DIR: &str = \".tx\";";
        assert!(
            code.contains(decl),
            "self-guard: the scan must find the declaration it is counting \
             against, or a rename makes this test pass vacuously"
        );
        assert_eq!(
            code.matches("\".tx\"").count(),
            1,
            "`.tx` may appear once — on TX_DIR's declaration. Every other site \
             (the constructor, the sweep) must go through the constant."
        );
        assert!(
            code.contains("join(TX_DIR)"),
            "the constructor must compose its staging root from TX_DIR"
        );
        assert!(
            code.contains("join(agent_id).join(TX_DIR)"),
            "…and the sweep must walk that same root"
        );
    }
}
