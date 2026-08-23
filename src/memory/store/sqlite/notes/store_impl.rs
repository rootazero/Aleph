//! `NoteStore` trait implementation for `SqliteMemoryBackend`.
//!
//! This is a single indivisible trait `impl` block (>1000 lines) — a Rust
//! trait impl cannot be split across files, so it is kept whole here per the
//! mechanical-split scope. Free helper functions live in `super::helpers`.

#![allow(unused_imports)]

use async_trait::async_trait;
use rusqlite::params;
use rusqlite::OptionalExtension;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::error::AlephError;
use crate::memory::notes::graph::{GraphEdge, GraphNode, GraphSnapshot};
use crate::memory::notes::store::{
    GraphEdgeRow, NoteIndexEntry, NoteStore, OutgoingLinkRow, ReviewArchiveRow, ReviewQueueRow,
};
use crate::memory::notes::{
    extract_wikilinks_with_alias, FactProvenance, KnowledgeNote, ProvenanceOrigin,
};

use super::super::vec;
use super::super::SqliteMemoryBackend;

use super::helpers::{
    body_text_sha256, collect_edges_between, load_note_content_from_disk,
    provenance_origin_from_str, provenance_origin_to_str, row_to_entry,
};

macro_rules! lock_conn {
    ($self:expr) => {{
        // Recover from a poisoned mutex instead of failing permanently: a panic
        // while some other note-store call held the lock must not brick every
        // subsequent note operation (the poison flag is sticky). The connection
        // is still usable — the panicking op simply didn't commit. Mirrors the
        // `.unwrap_or_else(|e| e.into_inner())` recovery used elsewhere (P7).
        // Wrapped in `Ok` so the existing `lock_conn!(self)?` call sites keep
        // their fallible shape without a per-site change. The recovery is
        // logged so a poison event is visible to operators (a silent
        // into_inner() is the exact failure mode that hid the half-committed
        // statement bug for years). The explicit Ok::<_, AlephError> keeps
        // the return type pinned through the match arms; the per-site `?`
        // relies on the From<Mutex<...>> impl, which is non-unique here.
        match $self.conn.lock() {
            Ok(g) => Ok::<_, AlephError>(g),
            Err(poisoned) => {
                tracing::warn!(
                    caller = stringify!($self),
                    "notes store: SQLite mutex was poisoned by a prior panic; \
                     recovering (this should be rare)"
                );
                Ok::<_, AlephError>(poisoned.into_inner())
            }
        }
    }};
}

#[async_trait]
impl NoteStore for SqliteMemoryBackend {
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    async fn index_note(
        &self,
        note: &KnowledgeNote,
        agent_id: &str,
        category: &str,
    ) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;

        // Store titles/filenames extensionless: a title carrying ".md" would
        // otherwise produce a doubled "*.md.md" path on disk reads.
        let title = crate::memory::notes::store::strip_md_ext(&note.title);
        let path = format!("{category}/{title}");
        let filename = title.to_string();

        let tags_json = serde_json::to_string(&note.tags).unwrap_or_else(|_| "[]".to_string());
        let aliases_json =
            serde_json::to_string(&note.aliases).unwrap_or_else(|_| "[]".to_string());

        use crate::memory::notes::links::{self, LinkStatus};

        let resolve_ctx = super::helpers::build_resolve_context(&conn, agent_id)
            .map_err(|e| AlephError::config(format!("index_note resolve ctx: {e}")))?;

        // Body labels: raw target → display label from `[[target|label]]`.
        let labels: HashMap<String, String> = note
            .body
            .as_deref()
            .map(|b| {
                extract_wikilinks_with_alias(b)
                    .into_iter()
                    .filter_map(|(t, l)| l.map(|l| (t, l)))
                    .collect()
            })
            .unwrap_or_default();

        /// Desired row value per to_note key.
        struct DesiredEdge {
            to_raw: String,
            relation: Option<String>,
            confidence: f32,
            resolved_by: Option<&'static str>,
            status: &'static str,
            label: Option<String>,
        }

        // to_note -> DesiredEdge. Body wikilinks first; typed relations
        // override on the same resolved target (unchanged precedence).
        let mut desired: HashMap<String, DesiredEdge> = HashMap::new();
        for raw_target in &note.links {
            let r = links::resolve(raw_target, &resolve_ctx);
            let (to_note, status) = match &r.target {
                // rust-doctor-disable-next-line excessive-clone
                Some(t) => (t.clone(), LinkStatus::Active.as_str()),
                // rust-doctor-disable-next-line excessive-clone
                None => (raw_target.clone(), LinkStatus::Dangling.as_str()),
            };
            desired.entry(to_note).or_insert_with(|| DesiredEdge {
                // rust-doctor-disable-next-line excessive-clone
                to_raw: raw_target.clone(),
                relation: None,
                confidence: r.confidence,
                resolved_by: r.resolved_by.map(|s| s.as_str()),
                status,
                label: labels.get(raw_target).cloned(),
            });
        }
        for rel in &note.relations {
            let r = links::resolve(&rel.to, &resolve_ctx);
            let (to_note, status) = match &r.target {
                // rust-doctor-disable-next-line excessive-clone
                Some(t) => (t.clone(), LinkStatus::Active.as_str()),
                // rust-doctor-disable-next-line excessive-clone
                None => (rel.to.clone(), LinkStatus::Dangling.as_str()),
            };
            // Typed relation overrides a plain wikilink to the same target;
            // its confidence is the LLM/tool judgement, not the resolver tier.
            desired.insert(
                to_note,
                DesiredEdge {
                    // rust-doctor-disable-next-line excessive-clone
                    to_raw: rel.to.clone(),
                    // rust-doctor-disable-next-line excessive-clone
                    relation: Some(rel.rel_type.clone()),
                    confidence: rel.confidence.clamp(0.0, 1.0),
                    resolved_by: r.resolved_by.map(|s| s.as_str()),
                    status,
                    label: None,
                },
            );
        }

        // Structural-strong supersession edges. The `superseded_by:` /
        // `supersedes:` frontmatter lists — and their `## Superseded by [[X]]`
        // body form, promoted into the lists by `sync_body_to_frontmatter`
        // before every index — are materialized as typed `notes_links` edges so
        // retrieval's `surface_relations` force-surfaces them (the
        // `STRUCTURAL_STRONG` correctness guarantee in `note/relation.rs`).
        // Without this, a note superseded via the body/list path — the form
        // ingest's `mark_superseded` and the orientation prompt both write —
        // never became an edge and was silently NOT force-surfaced; only the
        // `relations:`-block encoding worked. An explicit `relations:` entry to
        // the same target wins (more specific); a plain body wikilink to the
        // target is upgraded to the typed supersession edge.
        for (targets, rel) in [
            (&note.superseded_by, "superseded_by"),
            (&note.supersedes, "supersedes"),
        ] {
            for raw_target in targets {
                let r = links::resolve(raw_target, &resolve_ctx);
                let (to_note, status) = match &r.target {
                    // rust-doctor-disable-next-line excessive-clone
                    Some(t) => (t.clone(), LinkStatus::Active.as_str()),
                    // rust-doctor-disable-next-line excessive-clone
                    None => (raw_target.clone(), LinkStatus::Dangling.as_str()),
                };
                // Precompute so neither entry closure borrows `r`.
                let resolved_by = r.resolved_by.map(|s| s.as_str());
                // rust-doctor-disable-next-line excessive-clone
                let to_raw = raw_target.clone();
                desired
                    .entry(to_note)
                    .and_modify(|e| {
                        if e.relation.is_none() {
                            e.relation = Some(rel.to_string());
                            e.confidence = 1.0;
                        }
                    })
                    .or_insert_with(|| DesiredEdge {
                        to_raw,
                        relation: Some(rel.to_string()),
                        confidence: 1.0,
                        resolved_by,
                        status,
                        label: None,
                    });
            }
        }

        // Existing edges: to_note -> (to_raw, relation, confidence, resolved_by, status, label).
        let existing: HashMap<
            String,
            (
                String,
                Option<String>,
                f32,
                Option<String>,
                String,
                Option<String>,
            ),
        > = {
            let mut stmt = conn
                .prepare(
                    "SELECT to_note, to_raw, relation, confidence, resolved_by, status, label \
                     FROM notes_links WHERE agent_id = ?1 AND from_note = ?2",
                )
                .map_err(|e| AlephError::config(format!("index_note links scan prep: {e}")))?;
            let rows = stmt
                .query_map(params![agent_id, path], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, f32>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, Option<String>>(6)?,
                    ))
                })
                .map_err(|e| AlephError::config(format!("index_note links scan: {e}")))?;
            rows.filter_map(|r| r.ok())
                .map(
                    |(to_note, to_raw, relation, conf, resolved_by, status, label)| {
                        (
                            to_note,
                            (to_raw, relation, conf, resolved_by, status, label),
                        )
                    },
                )
                .collect()
        };

        // DELETE targets no longer desired.
        for to_note in existing.keys() {
            if !desired.contains_key(to_note) {
                conn.execute(
                    "DELETE FROM notes_links \
                     WHERE agent_id = ?1 AND from_note = ?2 AND to_note = ?3",
                    params![agent_id, path, to_note],
                )
                .map_err(|e| AlephError::config(format!("index_note links delete: {e}")))?;
            }
        }

        // UPSERT new or changed targets; skip unchanged rows (no write storm).
        for (to_note, d) in &desired {
            let prior = existing.get(to_note);

            // `relation` is DB-only enrichment OWNED BY THE DREAM STAGES, not by
            // markdown: `NoteWeave` stamps 'semantic' / 'related' / '<keyword>' via
            // `add_link_with_relation`, and nothing writes those to frontmatter. But
            // `desired` is rebuilt from markdown, where a body wikilink carries no
            // relation at all — so it came back as `None`, and writing that `None`
            // straight through NULLed every label the weave had stamped.
            //
            // That fired on the next re-index of the source note for ANY reason:
            // NoteDecay's per-cycle frontmatter patch, an ingest `append_to_note`, a
            // panel edit — and even inside NoteWeave's own loop, where writing a
            // second pair for the same peer re-indexed it and wiped the relation
            // stamped moments earlier. There is no re-stamp path (NoteWeave only
            // visits orphans, and by then the note has links), so the labels were
            // gone for good and the graph silently degraded to untyped edges.
            //
            // Markdown is the source of truth for the EDGE, not for its LABEL. When
            // markdown says nothing (`None`), keep what is already there. An explicit
            // frontmatter `relations:` entry still wins, because it sets
            // `relation: Some(..)` when `desired` is built above.
            let relation = match (&d.relation, prior.and_then(|(_, rel, ..)| rel.as_ref())) {
                // rust-doctor-disable-next-line excessive-clone
                (None, Some(kept)) => Some(kept.clone()),
                // rust-doctor-disable-next-line excessive-clone
                (from_markdown, _) => from_markdown.clone(),
            };

            let unchanged = prior.is_some_and(|(er, erel, econf, eresolved, estatus, elabel)| {
                er == &d.to_raw
                    && erel == &relation
                    && (econf - d.confidence).abs() < f32::EPSILON
                    && eresolved.as_deref() == d.resolved_by
                    && estatus.as_str() == d.status
                    && elabel.as_deref() == d.label.as_deref()
            });
            if unchanged {
                continue;
            }
            conn.execute(
                "INSERT INTO notes_links \
                   (agent_id, from_note, to_note, to_raw, relation, confidence, resolved_by, status, label) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
                 ON CONFLICT(agent_id, from_note, to_note) \
                 DO UPDATE SET to_raw = excluded.to_raw, relation = excluded.relation, \
                               confidence = excluded.confidence, resolved_by = excluded.resolved_by, \
                               status = excluded.status, label = excluded.label",
                params![
                    agent_id, path, to_note, d.to_raw, relation, d.confidence, d.resolved_by,
                    d.status, d.label
                ],
            )
            .map_err(|e| AlephError::config(format!("index_note links upsert: {e}")))?;
        }

        // Rebuild notes_sources from the note's `source_notes` (mirrors the
        // links replace semantics): clear the prior rows then re-insert the
        // current set, so the materialized graph snapshot sees exactly the
        // sources declared in frontmatter.
        conn.execute(
            "DELETE FROM notes_sources WHERE agent_id=?1 AND note_path=?2",
            params![agent_id, path],
        )
        .map_err(|e| AlephError::config(format!("index_note clear notes_sources: {e}")))?;
        for src in &note.source_notes {
            conn.execute(
                "INSERT OR IGNORE INTO notes_sources (agent_id, note_path, source_ref) \
                 VALUES (?1, ?2, ?3)",
                params![agent_id, path, src],
            )
            .map_err(|e| AlephError::config(format!("index_note insert notes_sources: {e}")))?;
        }

        // Skip notes_fts rebuild when the body text is unchanged — frontmatter-only
        // edits (e.g. updated_at bump, link reorder, tag rotation) are common and
        // each FTS5 DELETE+INSERT cascades to ~14 shadow-table writes
        // (`_data`/`_idx`/`_docsize`/`_content`). The body hash is tracked in a
        // sibling `notes_fts_meta` row so the gate survives across processes.
        //
        // Use `body_text_for_fts()` (Phase C2.2) so inline `<!-- src: ... -->`
        // provenance markers don't end up in the FTS index. For legacy notes
        // without markers this is identical to `body_text()`, so the existing
        // `notes_fts_meta` rows remain valid; notes that gain markers will
        // trigger one rewrite — the correct one-time migration cost.
        let body = note.body_text_for_fts();
        let body_hash = body_text_sha256(&body);

        let prev_body_hash: Option<String> = conn
            .query_row(
                "SELECT content_hash FROM notes_fts_meta WHERE agent_id = ?1 AND path = ?2",
                params![agent_id, path],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| AlephError::config(format!("index_note fts meta lookup: {e}")))?;

        if prev_body_hash.as_deref() != Some(&body_hash) {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| AlephError::config(format!("index_note fts tx begin: {e}")))?;
            tx.execute(
                "DELETE FROM notes_fts WHERE path = ?1 AND agent_id = ?2",
                params![path, agent_id],
            )
            .map_err(|e| AlephError::config(format!("index_note delete fts: {e}")))?;
            tx.execute(
                "INSERT INTO notes_fts (path, filename, content, agent_id) VALUES (?1, ?2, ?3, ?4)",
                params![path, filename, body, agent_id],
            )
            .map_err(|e| AlephError::config(format!("index_note insert fts: {e}")))?;
            // Mirror into the trigram companion (CJK substring search) under the
            // same transaction + same body-hash skip-gate, so it never diverges
            // from notes_fts.
            tx.execute(
                "DELETE FROM notes_fts_trigram WHERE path = ?1 AND agent_id = ?2",
                params![path, agent_id],
            )
            .map_err(|e| AlephError::config(format!("index_note delete fts_trigram: {e}")))?;
            tx.execute(
                "INSERT INTO notes_fts_trigram (path, filename, content, agent_id) VALUES (?1, ?2, ?3, ?4)",
                params![path, filename, body, agent_id],
            )
            .map_err(|e| AlephError::config(format!("index_note insert fts_trigram: {e}")))?;
            tx.execute(
                "INSERT INTO notes_fts_meta (agent_id, path, content_hash) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(agent_id, path) DO UPDATE SET content_hash = excluded.content_hash",
                params![agent_id, path, body_hash],
            )
            .map_err(|e| AlephError::config(format!("index_note fts meta upsert: {e}")))?;
            tx.commit()
                .map_err(|e| AlephError::config(format!("index_note fts tx commit: {e}")))?;
        }

        // Persist per-fact provenance for governance / review (Phase C2.9.2).
        // Inlined under the existing connection guard rather than calling
        // `self.upsert_provenance(...)` so we don't drop and re-acquire the
        // connection mutex mid-write. An empty `fact_provenance` (legacy notes)
        // is fine: the DELETE clears any stale rows and the loop is a no-op.
        conn.execute(
            "DELETE FROM notes_provenance WHERE agent_id = ?1 AND note_path = ?2",
            params![agent_id, path],
        )
        .map_err(|e| AlephError::config(format!("index_note prov delete: {e}")))?;
        let now_ts = chrono::Utc::now().timestamp();
        for (idx, p) in note.fact_provenance.iter().enumerate() {
            let origin_str = provenance_origin_to_str(&p.origin);
            let source_kind: Option<&str> = match p.origin {
                ProvenanceOrigin::RawSource => Some("raw"),
                ProvenanceOrigin::PriorNote => Some("note"),
                _ => None,
            };
            conn.execute(
                "INSERT INTO notes_provenance \
                 (agent_id, note_path, fact_idx, origin, source_kind, source_id, inferred, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    agent_id,
                    path,
                    idx as i64,
                    origin_str,
                    source_kind,
                    p.source_id,
                    i64::from(p.inferred),
                    now_ts,
                ],
            )
            .map_err(|e| AlephError::config(format!("index_note prov insert: {e}")))?;
        }

        // Upsert notes_index LAST. Its `content_hash` is the skip-gate that
        // `full_rebuild` / `index_file` check to skip an unchanged file. Writing
        // it only after links + notes_sources + FTS + provenance have all landed
        // (each autocommits in order) means a crash mid-write leaves the OLD hash
        // advertised, so the next scan re-processes this file and self-heals —
        // instead of skipping it forever on a hash whose derived rows never got
        // written.
        conn.execute(
            "INSERT OR REPLACE INTO notes_index \
             (path, filename, agent_id, category, tags_json, aliases_json, created_at, updated_at, content_hash) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                path,
                filename,
                agent_id,
                category,
                tags_json,
                aliases_json,
                note.created_at,
                note.updated_at,
                note.content_hash,
            ],
        )
        .map_err(|e| AlephError::config(format!("index_note insert: {e}")))?;

        Ok(())
    }

    // rust-doctor-disable-next-line high-cyclomatic-complexity
    async fn remove_note_index(&self, path: &str, agent_id: &str) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;

        // Wrap all four DELETEs in a single transaction so a crash mid-removal
        // cannot leave an orphan `notes_fts_meta` row. Without this, recreating
        // a note with the same body would match the stale hash and skip the
        // FTS rewrite, silently making the recreated note search-invisible.
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AlephError::config(format!("remove_note_index tx begin: {e}")))?;

        tx.execute(
            "DELETE FROM notes_index WHERE path = ?1 AND agent_id = ?2",
            params![path, agent_id],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index index: {e}")))?;

        tx.execute(
            "DELETE FROM notes_links WHERE from_note = ?1 AND agent_id = ?2",
            params![path, agent_id],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index outgoing links: {e}")))?;

        // D1 tombstone semantics: inbound rows are marked, never destroyed —
        // the linking note's body keeps its [[link]] text and the row revives
        // via backfill_inbound_links if a same-name note is recreated.
        tx.execute(
            "UPDATE notes_links SET status = 'tombstone' WHERE to_note = ?1 AND agent_id = ?2",
            params![path, agent_id],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index tombstone inbound: {e}")))?;

        tx.execute(
            "DELETE FROM notes_fts WHERE path = ?1 AND agent_id = ?2",
            params![path, agent_id],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index fts: {e}")))?;

        tx.execute(
            "DELETE FROM notes_fts_trigram WHERE path = ?1 AND agent_id = ?2",
            params![path, agent_id],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index fts_trigram: {e}")))?;

        tx.execute(
            "DELETE FROM notes_fts_meta WHERE agent_id = ?1 AND path = ?2",
            params![agent_id, path],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index fts meta: {e}")))?;

        tx.execute(
            "DELETE FROM notes_sources WHERE agent_id = ?1 AND note_path = ?2",
            params![agent_id, path],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index sources: {e}")))?;

        // Fact-level provenance rows are keyed by (agent_id, note_path); a
        // permanent delete must clear them too, otherwise every removed note
        // leaks orphan provenance rows (an unbounded growth over time).
        tx.execute(
            "DELETE FROM notes_provenance WHERE agent_id = ?1 AND note_path = ?2",
            params![agent_id, path],
        )
        .map_err(|e| AlephError::config(format!("remove_note_index provenance: {e}")))?;

        // Clear the embedding too: an orphan vector for a deleted note keeps
        // occupying KNN slots forever (retrieval skips it on the missing file,
        // but it still displaces real notes from the top-K). The vec0 virtual
        // tables are per-dimension; the map row tells us the shared rowid.
        let vec_rowid: Option<i64> = tx
            .query_row(
                "SELECT rowid FROM notes_vec_map WHERE path = ?1 AND agent_id = ?2",
                params![path, agent_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AlephError::config(format!("remove_note_index vec map lookup: {e}")))?;
        if let Some(rowid) = vec_rowid {
            for table in vec::all_notes_vec_tables() {
                // Table name comes from an internal static allowlist (`EMBEDDING_DIM_TABLES`).
                // rust-doctor-disable-next-line sql-injection-risk
                tx.execute(
                    &format!("DELETE FROM {table} WHERE rowid = ?1"),
                    params![rowid],
                )
                .map_err(|e| AlephError::config(format!("remove_note_index {table}: {e}")))?;
            }
            tx.execute("DELETE FROM notes_vec_map WHERE rowid = ?1", params![rowid])
                .map_err(|e| AlephError::config(format!("remove_note_index vec map: {e}")))?;
        }

        tx.commit()
            .map_err(|e| AlephError::config(format!("remove_note_index tx commit: {e}")))?;
        Ok(())
    }

    async fn get_note_index(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Option<NoteIndexEntry>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT n.*, \
                 (SELECT COUNT(*) FROM notes_links WHERE from_note = n.path AND agent_id = n.agent_id) AS link_count \
                 FROM notes_index n WHERE n.path = ?1 AND n.agent_id = ?2",
            )
            .map_err(|e| AlephError::config(format!("get_note_index prepare: {e}")))?;

        let result = stmt
            .query_row(params![path, agent_id], row_to_entry)
            .optional()
            .map_err(|e| AlephError::config(format!("get_note_index query: {e}")))?;

        Ok(result)
    }

    async fn list_notes(&self, agent_id: &str) -> Result<Vec<NoteIndexEntry>, AlephError> {
        let agent_id = agent_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT n.*, \
                     (SELECT COUNT(*) FROM notes_links WHERE from_note = n.path AND agent_id = n.agent_id) AS link_count \
                     FROM notes_index n \
                     WHERE n.agent_id = ?1 \
                     ORDER BY n.updated_at DESC",
                )
                .map_err(|e| AlephError::config(format!("list_notes prepare: {e}")))?;

            let rows = stmt
                .query_map(params![agent_id], row_to_entry)
                .map_err(|e| AlephError::config(format!("list_notes query: {e}")))?;

            let mut entries = Vec::new();
            for row in rows {
                entries.push(row.map_err(|e| AlephError::config(format!("list_notes row: {e}")))?);
            }
            Ok(entries)
        })
        .await
    }

    async fn count_all_notes(&self) -> Result<i64, AlephError> {
        let conn = lock_conn!(self)?;
        conn.query_row("SELECT COUNT(*) FROM notes_index", [], |row| row.get(0))
            .map_err(|e| AlephError::config(format!("count_all_notes failed: {e}")))
    }

    async fn count_notes(&self, agent_id: &str) -> Result<i64, AlephError> {
        let conn = lock_conn!(self)?;
        conn.query_row(
            "SELECT COUNT(*) FROM notes_index WHERE agent_id = ?1",
            params![agent_id],
            |row| row.get(0),
        )
        .map_err(|e| AlephError::config(format!("count_notes failed: {e}")))
    }

    async fn community_ids(
        &self,
        agent_id: &str,
    ) -> Result<std::collections::HashMap<String, i64>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut stmt = conn
            .prepare("SELECT node_path, community_id FROM notes_graph_cache WHERE agent_id = ?1")
            .map_err(|e| AlephError::config(format!("community_ids prepare: {e}")))?;
        let rows = stmt
            .query_map(params![agent_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })
            .map_err(|e| AlephError::config(format!("community_ids query: {e}")))?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (path, cid) =
                row.map_err(|e| AlephError::config(format!("community_ids row: {e}")))?;
            map.insert(path, cid);
        }
        Ok(map)
    }

    async fn get_outgoing_links(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Vec<String>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare("SELECT to_note FROM notes_links WHERE from_note = ?1 AND agent_id = ?2")
            .map_err(|e| AlephError::config(format!("get_outgoing_links prepare: {e}")))?;

        let rows = stmt
            .query_map(params![path, agent_id], |row| row.get::<_, String>(0))
            .map_err(|e| AlephError::config(format!("get_outgoing_links query: {e}")))?;

        let mut links = Vec::new();
        for row in rows {
            links
                .push(row.map_err(|e| AlephError::config(format!("get_outgoing_links row: {e}")))?);
        }
        Ok(links)
    }

    async fn get_incoming_links(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Vec<String>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare("SELECT from_note FROM notes_links WHERE to_note = ?1 AND agent_id = ?2")
            .map_err(|e| AlephError::config(format!("get_incoming_links prepare: {e}")))?;

        let rows = stmt
            .query_map(params![path, agent_id], |row| row.get::<_, String>(0))
            .map_err(|e| AlephError::config(format!("get_incoming_links query: {e}")))?;

        let mut links = Vec::new();
        for row in rows {
            links
                .push(row.map_err(|e| AlephError::config(format!("get_incoming_links row: {e}")))?);
        }
        Ok(links)
    }

    async fn get_incoming_links_any(
        &self,
        path: &str,
        filename: &str,
        agent_id: &str,
    ) -> Result<Vec<String>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT from_note FROM notes_links \
                 WHERE to_note IN (?1, ?2) AND agent_id = ?3",
            )
            .map_err(|e| AlephError::config(format!("get_incoming_links_any prepare: {e}")))?;

        let rows = stmt
            .query_map(params![path, filename, agent_id], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| AlephError::config(format!("get_incoming_links_any query: {e}")))?;

        let mut links = Vec::new();
        for row in rows {
            links.push(
                row.map_err(|e| AlephError::config(format!("get_incoming_links_any row: {e}")))?,
            );
        }
        Ok(links)
    }

    async fn add_link_with_relation(
        &self,
        agent_id: &str,
        from_note: &str,
        to_note: &str,
        relation: &str,
    ) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;
        conn.execute(
            "INSERT INTO notes_links (agent_id, from_note, to_note, to_raw, relation, status) \
             VALUES (?1, ?2, ?3, ?3, ?4, 'active') \
             ON CONFLICT(agent_id, from_note, to_note) \
             DO UPDATE SET relation = excluded.relation",
            params![agent_id, from_note, to_note, relation],
        )
        .map_err(|e| AlephError::config(format!("add_link_with_relation: {e}")))?;
        Ok(())
    }

    async fn get_typed_relations(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Vec<(String, String)>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut stmt = conn
            .prepare(
                "SELECT to_note, relation FROM notes_links \
                 WHERE from_note = ?1 AND agent_id = ?2 AND relation IS NOT NULL",
            )
            .map_err(|e| AlephError::config(format!("get_typed_relations prepare: {e}")))?;
        let rows = stmt
            .query_map(params![path, agent_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| AlephError::config(format!("get_typed_relations query: {e}")))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| AlephError::config(format!("get_typed_relations row: {e}")))?);
        }
        Ok(out)
    }

    async fn search_notes_fts(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<NoteIndexEntry>, AlephError> {
        // An empty / whitespace-only query would build a bare `MATCH '""'`,
        // which FTS5 can reject as a syntax error. Nothing can match it, so
        // short-circuit to "no results" before touching the connection.
        if query.split_whitespace().next().is_none() {
            return Ok(Vec::new());
        }

        let query = query.to_string();
        let agent_id = agent_id.to_string();
        self.with_conn(move |conn| {
            // FTS5 reserves `[`, `]`, `^`, `*`, `:`, `(`, `)`, `"` and operator
            // keywords (`AND`, `OR`, `NOT`, `NEAR`). Raw user/transcript queries
            // routinely contain brackets (`[user]`, code snippets, JSON), which
            // makes the unquoted MATCH binding raise `fts5: syntax error near "["`.
            // Each whitespace-separated term becomes its own FTS5 phrase (embedded
            // quotes doubled per FTS5 escaping) joined with OR — binding the WHOLE
            // query as one phrase required the exact token sequence, so multi-word
            // natural-language queries matched nothing. `ORDER BY rank` (bm25)
            // then puts notes matching more/rarer terms first.
            let phrase = |t: &str| format!("\"{}\"", t.replace('"', "\"\""));
            let terms: Vec<String> = query.split_whitespace().map(phrase).collect();
            let match_expr = if terms.len() <= 1 {
                phrase(query.trim())
            } else {
                terms.join(" OR ")
            };

            // Run one FTS table (unicode61 primary or trigram companion). `table`
            // is a compile-time constant, so interpolating it is injection-safe.
            let run_fts = |table: &str, expr: &str| -> Result<Vec<NoteIndexEntry>, AlephError> {
                let sql = format!(
                    "SELECT n.*, \
                     (SELECT COUNT(*) FROM notes_links WHERE from_note = n.path AND agent_id = n.agent_id) AS link_count \
                     FROM {table} f \
                     JOIN notes_index n ON n.path = f.path AND n.agent_id = f.agent_id \
                     WHERE {table} MATCH ?1 AND f.agent_id = ?2 \
                     ORDER BY rank \
                     LIMIT ?3"
                );
                let mut stmt = conn
                    .prepare(&sql)
                    .map_err(|e| AlephError::config(format!("search_notes_fts prepare: {e}")))?;
                let rows = stmt
                    .query_map(params![expr, agent_id, limit as i64], row_to_entry)
                    .map_err(|e| AlephError::config(format!("search_notes_fts query: {e}")))?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(
                        row.map_err(|e| AlephError::config(format!("search_notes_fts row: {e}")))?,
                    );
                }
                Ok(out)
            };

            let mut entries = run_fts("notes_fts", &match_expr)?;

            // CJK substring recall: `unicode61` indexes a run of CJK ideographs as a
            // single token, so the CJK word `记忆` never matches inside `记忆管理`. For CJK-bearing
            // queries also consult the trigram companion. Build the MATCH from
            // per-term OR (mirroring the unicode61 leg), keeping only terms of ≥3
            // chars — the trigram tokenizer's minimum — so a multi-word CJK query
            // (e.g. `记忆管理 系统运维`) still substring-matches each word; a single
            // whole-phrase MATCH would fail across the interior spaces. New hits are
            // merged in, capped at `limit`. ASCII-only queries skip this entirely and
            // keep byte-identical behaviour.
            if query
                .chars()
                .any(crate::memory::notes::links::mentions::is_cjk)
            {
                let tri_terms: Vec<String> = query
                    .split_whitespace()
                    .filter(|t| t.chars().count() >= 3)
                    .map(phrase)
                    .collect();
                if !tri_terms.is_empty() {
                    let tri_expr = tri_terms.join(" OR ");
                    let seen: std::collections::HashSet<String> =
                        // rust-doctor-disable-next-line excessive-clone
                        entries.iter().map(|e| e.path.clone()).collect();
                    for entry in run_fts("notes_fts_trigram", &tri_expr)? {
                        if entries.len() >= limit {
                            break;
                        }
                        if !seen.contains(entry.path.as_str()) {
                            entries.push(entry);
                        }
                    }
                }
            }
            Ok(entries)
        })
        .await
    }

    async fn get_graph_data(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<(Vec<NoteIndexEntry>, Vec<GraphEdgeRow>), AlephError> {
        let conn = lock_conn!(self)?;

        // Top notes by link_count (outgoing) + recency
        let mut stmt = conn
            .prepare(
                "SELECT n.*, \
                 (SELECT COUNT(*) FROM notes_links WHERE from_note = n.path AND agent_id = n.agent_id) AS link_count \
                 FROM notes_index n \
                 WHERE n.agent_id = ?1 \
                 ORDER BY link_count DESC, n.updated_at DESC \
                 LIMIT ?2",
            )
            .map_err(|e| AlephError::config(format!("get_graph_data nodes prepare: {e}")))?;

        let rows = stmt
            .query_map(params![agent_id, limit as i64], row_to_entry)
            .map_err(|e| AlephError::config(format!("get_graph_data nodes query: {e}")))?;

        let mut entries = Vec::new();
        let mut visible: HashSet<String> = HashSet::new();
        for row in rows {
            let entry =
                row.map_err(|e| AlephError::config(format!("get_graph_data node row: {e}")))?;
            // rust-doctor-disable-next-line excessive-clone
            visible.insert(entry.path.clone());
            entries.push(entry);
        }

        // Edges between visible nodes
        let edges = collect_edges_between(&conn, &visible, agent_id)?;

        Ok((entries, edges))
    }

    // rust-doctor-disable-next-line high-cyclomatic-complexity
    async fn get_neighbors(
        &self,
        center: &str,
        agent_id: &str,
        depth: u8,
        limit: usize,
    ) -> Result<(Vec<NoteIndexEntry>, Vec<(String, String)>, bool), AlephError> {
        let conn = lock_conn!(self)?;

        // BFS from center
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, u8)> = VecDeque::new();

        visited.insert(center.to_string());
        queue.push_back((center.to_string(), 0));

        while let Some((node, d)) = queue.pop_front() {
            if d >= depth {
                continue;
            }
            if visited.len() >= limit {
                break;
            }

            // Outgoing
            let mut out_stmt = conn
                .prepare("SELECT to_note FROM notes_links WHERE from_note = ?1 AND agent_id = ?2")
                .map_err(|e| AlephError::config(format!("get_neighbors out prepare: {e}")))?;
            let out_rows = out_stmt
                .query_map(params![node, agent_id], |row| row.get::<_, String>(0))
                .map_err(|e| AlephError::config(format!("get_neighbors out query: {e}")))?;
            for r in out_rows {
                let n = r.map_err(|e| AlephError::config(format!("get_neighbors out row: {e}")))?;
                // rust-doctor-disable-next-line excessive-clone
                if visited.insert(n.clone()) {
                    queue.push_back((n, d + 1));
                }
                if visited.len() >= limit {
                    break;
                }
            }

            // Incoming
            let mut in_stmt = conn
                .prepare("SELECT from_note FROM notes_links WHERE to_note = ?1 AND agent_id = ?2")
                .map_err(|e| AlephError::config(format!("get_neighbors in prepare: {e}")))?;
            let in_rows = in_stmt
                .query_map(params![node, agent_id], |row| row.get::<_, String>(0))
                .map_err(|e| AlephError::config(format!("get_neighbors in query: {e}")))?;
            for r in in_rows {
                let n = r.map_err(|e| AlephError::config(format!("get_neighbors in row: {e}")))?;
                // rust-doctor-disable-next-line excessive-clone
                if visited.insert(n.clone()) {
                    queue.push_back((n, d + 1));
                }
                if visited.len() >= limit {
                    break;
                }
            }
        }

        // The BFS caps on `visited.len()` (which counts dangling endpoints too),
        // so this — not the indexed-entry count below — is the authoritative
        // truncation signal: if we filled the frontier to `limit`, the graph may
        // hold neighbors we never explored.
        let truncated = visited.len() >= limit;

        // Fetch index entries for visited nodes
        let mut entries = Vec::new();
        for path in &visited {
            let mut stmt = conn
                .prepare(
                    "SELECT n.*, \
                     (SELECT COUNT(*) FROM notes_links WHERE from_note = n.path AND agent_id = n.agent_id) AS link_count \
                     FROM notes_index n WHERE n.path = ?1 AND n.agent_id = ?2",
                )
                .map_err(|e| AlephError::config(format!("get_neighbors entry prepare: {e}")))?;

            if let Some(entry) = stmt
                .query_row(params![path, agent_id], row_to_entry)
                .optional()
                .map_err(|e| AlephError::config(format!("get_neighbors entry query: {e}")))?
            {
                entries.push(entry);
            }
        }

        // Edges between visited nodes (kind discarded — neighbors keeps its
        // untyped `(from, to)` shape; only graph.query surfaces edge kind).
        let edges = collect_edges_between(&conn, &visited, agent_id)?
            .into_iter()
            .map(|e| (e.from, e.to))
            .collect();

        Ok((entries, edges, truncated))
    }

    // rust-doctor-disable-next-line high-cyclomatic-complexity
    async fn find_path(
        &self,
        from: &str,
        to: &str,
        agent_id: &str,
        max_depth: u8,
    ) -> Result<(Option<Vec<(String, String, Option<String>)>>, bool), AlephError> {
        // A note is connected to itself by the empty path.
        if from == to {
            return Ok((Some(Vec::new()), false));
        }

        // Bound total work so a pathological hub can't turn one query into a
        // full-graph scan (mirrors get_neighbors' visit discipline).
        const MAX_VISITS: usize = 10_000;

        let conn = lock_conn!(self)?;

        // BFS over notes_links in both directions, remembering the edge that
        // first discovered each node so the path can be reconstructed.
        // `came_from`: discovered_node -> (predecessor, connecting-edge relation).
        let mut came_from: HashMap<String, (String, Option<String>)> = HashMap::new();
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, u8)> = VecDeque::new();

        visited.insert(from.to_string());
        queue.push_back((from.to_string(), 0));
        let mut truncated = false;
        let mut found = false;

        'bfs: while let Some((node, d)) = queue.pop_front() {
            if d >= max_depth {
                // A frontier node left unexpanded: an unreached target may still
                // be reachable beyond the depth cap.
                truncated = true;
                continue;
            }
            if visited.len() >= MAX_VISITS {
                truncated = true;
                break;
            }

            // Outgoing (from=node → to=nbr) then incoming (from=nbr → to=node);
            // the connection graph is undirected, so both discover neighbours.
            for sql in [
                "SELECT to_note, relation FROM notes_links \
                 WHERE from_note = ?1 AND agent_id = ?2",
                "SELECT from_note, relation FROM notes_links \
                 WHERE to_note = ?1 AND agent_id = ?2",
            ] {
                let mut stmt = conn
                    .prepare(sql)
                    .map_err(|e| AlephError::config(format!("find_path prepare: {e}")))?;
                let rows = stmt
                    .query_map(params![node, agent_id], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                    })
                    .map_err(|e| AlephError::config(format!("find_path query: {e}")))?;
                for r in rows {
                    let (nbr, rel) =
                        r.map_err(|e| AlephError::config(format!("find_path row: {e}")))?;
                    // rust-doctor-disable-next-line excessive-clone
                    if visited.insert(nbr.clone()) {
                        // rust-doctor-disable-next-line excessive-clone
                        came_from.insert(nbr.clone(), (node.clone(), rel));
                        if nbr == to {
                            found = true;
                            break 'bfs;
                        }
                        queue.push_back((nbr, d + 1));
                    }
                }
            }
        }

        if !found {
            return Ok((None, truncated));
        }

        // Reconstruct the walk by following predecessors from `to` back to
        // `from`, then reverse to get `from → to` order.
        let mut path: Vec<(String, String, Option<String>)> = Vec::new();
        let mut cursor = to.to_string();
        while cursor != from {
            let (pred, rel) = match came_from.get(&cursor) {
                // rust-doctor-disable-next-line excessive-clone
                Some(v) => v.clone(),
                None => break, // defensive: unreachable once `to` is in came_from
            };
            // rust-doctor-disable-next-line excessive-clone
            path.push((pred.clone(), cursor.clone(), rel));
            cursor = pred;
        }
        path.reverse();
        Ok((Some(path), false))
    }

    async fn get_outgoing_link_rows(
        &self,
        path: &str,
        agent_id: &str,
    ) -> Result<Vec<OutgoingLinkRow>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT to_note, to_raw, relation, label, confidence, resolved_by, status \
                 FROM notes_links WHERE from_note = ?1 AND agent_id = ?2",
            )
            .map_err(|e| AlephError::config(format!("get_outgoing_link_rows prepare: {e}")))?;

        let rows = stmt
            .query_map(params![path, agent_id], |row| {
                Ok(OutgoingLinkRow {
                    to_note: row.get::<_, String>(0)?,
                    to_raw: row.get::<_, String>(1)?,
                    relation: row.get::<_, Option<String>>(2)?,
                    label: row.get::<_, Option<String>>(3)?,
                    confidence: row.get::<_, f32>(4)?,
                    resolved_by: row.get::<_, Option<String>>(5)?,
                    status: row.get::<_, String>(6)?,
                })
            })
            .map_err(|e| AlephError::config(format!("get_outgoing_link_rows query: {e}")))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(
                row.map_err(|e| AlephError::config(format!("get_outgoing_link_rows row: {e}")))?,
            );
        }
        Ok(out)
    }

    async fn find_by_filename(
        &self,
        filename: &str,
        agent_id: &str,
    ) -> Result<Vec<String>, AlephError> {
        let filename = filename.to_string();
        let agent_id = agent_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare("SELECT path FROM notes_index WHERE filename = ?1 AND agent_id = ?2")
                .map_err(|e| AlephError::config(format!("find_by_filename prepare: {e}")))?;

            let rows = stmt
                .query_map(params![filename, agent_id], |row| row.get::<_, String>(0))
                .map_err(|e| AlephError::config(format!("find_by_filename query: {e}")))?;

            let mut paths = Vec::new();
            for row in rows {
                paths.push(
                    row.map_err(|e| AlephError::config(format!("find_by_filename row: {e}")))?,
                );
            }
            Ok(paths)
        })
        .await
    }

    async fn prune_orphan_vectors(&self, agent_id: &str) -> Result<usize, AlephError> {
        let conn = lock_conn!(self)?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AlephError::config(format!("prune_orphan_vectors tx begin: {e}")))?;

        // Collect map rowids whose path has no notes_index row.
        let orphan_rowids: Vec<i64> = {
            let mut stmt = tx
                .prepare(
                    "SELECT m.rowid FROM notes_vec_map m \
                     WHERE m.agent_id = ?1 \
                       AND NOT EXISTS (SELECT 1 FROM notes_index n \
                                       WHERE n.path = m.path AND n.agent_id = m.agent_id)",
                )
                .map_err(|e| AlephError::config(format!("prune_orphan_vectors prepare: {e}")))?;
            let rows = stmt
                .query_map(params![agent_id], |row| row.get::<_, i64>(0))
                .map_err(|e| AlephError::config(format!("prune_orphan_vectors query: {e}")))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| AlephError::config(format!("prune_orphan_vectors rows: {e}")))?
        };

        // Bound each round-trip: 5 000 rowids × 6 statements (5 vec tables + 1
        // map) per batch = 30 k round-trips in the worst case. The previous
        // per-rowid loop made it N × 6, which on a 50 k-note vault held the
        // connection mutex for many seconds and starved concurrent recall.
        // SQLite's SQLITE_MAX_VARIABLE_NUMBER is 32 766 by default; 5 000 leaves
        // generous headroom for other bind usage in the same statement.
        const BATCH_SIZE: usize = 5_000;
        for chunk in orphan_rowids.chunks(BATCH_SIZE) {
            for table in vec::all_notes_vec_tables() {
                // Table name comes from an internal static allowlist
                // (`EMBEDDING_DIM_TABLES`), so the format! here is safe.
                // rust-doctor-disable-next-line sql-injection-risk
                let sql = format!("DELETE FROM {table} WHERE rowid IN (");
                let placeholders = std::iter::repeat_n("?", chunk.len())
                    .collect::<Vec<_>>()
                    .join(",");
                let sql = format!("{sql}{placeholders})");
                let binds = rusqlite::params_from_iter(chunk.iter().copied());
                tx.execute(&sql, binds).map_err(|e| {
                    AlephError::config(format!("prune_orphan_vectors {table}: {e}"))
                })?;
            }
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("DELETE FROM notes_vec_map WHERE rowid IN ({placeholders})");
            let binds = rusqlite::params_from_iter(chunk.iter().copied());
            tx.execute(&sql, binds)
                .map_err(|e| AlephError::config(format!("prune_orphan_vectors map: {e}")))?;
        }

        tx.commit()
            .map_err(|e| AlephError::config(format!("prune_orphan_vectors commit: {e}")))?;
        Ok(orphan_rowids.len())
    }

    async fn upsert_embedding(
        &self,
        path: &str,
        agent_id: &str,
        embedding: &[f32],
        dim: u32,
        content_hash: &str,
    ) -> Result<(), AlephError> {
        let table = vec::notes_vec_table_for_dim(dim)?;
        let conn = lock_conn!(self)?;
        let now = chrono::Utc::now().timestamp();

        // Upsert the mapping row to get a stable numeric rowid, recording which
        // version of the note this vector was computed from. The freshness
        // columns are updated on conflict too — a re-embed of an existing note
        // must move its provenance forward, or the row would keep claiming the
        // version it was first embedded at.
        conn.execute(
            "INSERT INTO notes_vec_map (path, agent_id, embedded_hash, embedded_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(agent_id, path) \
             DO UPDATE SET embedded_hash = excluded.embedded_hash, \
                           embedded_at = excluded.embedded_at",
            params![path, agent_id, content_hash, now],
        )
        .map_err(|e| AlephError::config(format!("upsert_embedding map insert: {e}")))?;

        let rowid: i64 = conn
            .query_row(
                "SELECT rowid FROM notes_vec_map WHERE path = ?1 AND agent_id = ?2",
                params![path, agent_id],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("upsert_embedding map lookup: {e}")))?;

        // Delete any existing embedding for this rowid across ALL dimension
        // tables, not just the current one: a re-embed at a new dimension (e.g.
        // 768 -> 1024) would otherwise orphan the old-dim row at this same rowid,
        // leaving a stale vector permanently occupying a KNN slot in the other
        // table. Mirrors the sweep in `remove_note_index`.
        for t in vec::all_notes_vec_tables() {
            // Table name comes from an internal static allowlist (`EMBEDDING_DIM_TABLES`).
            // rust-doctor-disable-next-line sql-injection-risk
            conn.execute(&format!("DELETE FROM {t} WHERE rowid = ?1"), params![rowid])
                .map_err(|e| AlephError::config(format!("upsert_embedding delete vec {t}: {e}")))?;
        }

        // Insert new embedding
        let blob = vec::embedding_to_blob(embedding);
        // Table name is validated by `vec::notes_vec_table_for_dim` against a static allowlist.
        // rust-doctor-disable-next-line sql-injection-risk
        conn.execute(
            &format!("INSERT INTO {table}(rowid, embedding) VALUES (?1, ?2)"),
            params![rowid, blob],
        )
        .map_err(|e| AlephError::config(format!("upsert_embedding insert vec: {e}")))?;

        Ok(())
    }

    async fn stale_vector_paths(&self, agent_id: &str) -> Result<Vec<String>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut stmt = conn
            .prepare(
                "SELECT n.path FROM notes_index n \
                 LEFT JOIN notes_vec_map m ON m.path = n.path AND m.agent_id = n.agent_id \
                 WHERE n.agent_id = ?1 \
                   AND (m.path IS NULL OR m.embedded_hash != n.content_hash) \
                 ORDER BY n.path",
            )
            .map_err(|e| AlephError::config(format!("stale_vector_paths prepare: {e}")))?;
        let rows = stmt
            .query_map(params![agent_id], |row| row.get::<_, String>(0))
            .map_err(|e| AlephError::config(format!("stale_vector_paths query: {e}")))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| AlephError::config(format!("stale_vector_paths row: {e}")))?);
        }
        Ok(out)
    }

    async fn hybrid_search_notes(
        &self,
        embedding: &[f32],
        query_text: &str,
        agent_id: &str,
        dim_hint: u32,
        limit: usize,
    ) -> Result<crate::memory::notes::store::HybridSearchOutcome, AlephError> {
        use std::collections::HashMap;

        let vec_results = self
            .vector_search(embedding, dim_hint, agent_id, limit * 2)
            .await?;
        let fts_entries = self
            .search_notes_fts(query_text, agent_id, limit * 2)
            .await?;

        // RRF fusion. `rrf_k` is the standard Reciprocal Rank Fusion
        // constant; FTS (lexical) matches get an extra `bm25_bonus_weight`
        // multiplicative lift so operators can bias toward keyword hits.
        let k = self.tuning.rrf_k as f32;
        let bm25_lift = 1.0 + self.tuning.bm25_bonus_weight;
        let mut scores: HashMap<String, f32> = HashMap::new();

        for (rank, (path, _score)) in vec_results.iter().enumerate() {
            let rrf = 1.0 / (k + (rank as f32) + 1.0);
            // rust-doctor-disable-next-line excessive-clone
            *scores.entry(path.clone()).or_insert(0.0) += rrf;
        }

        for (rank, entry) in fts_entries.iter().enumerate() {
            let rrf = (1.0 / (k + (rank as f32) + 1.0)) * bm25_lift;
            // rust-doctor-disable-next-line excessive-clone
            *scores.entry(entry.path.clone()).or_insert(0.0) += rrf;
        }

        let mut sorted: Vec<(String, f32)> = scores.into_iter().collect();
        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        sorted.truncate(limit);

        // Resolve index rows first (they share one connection, so they are
        // serial by construction), then read the bodies concurrently.
        let mut entries = Vec::with_capacity(sorted.len());
        for (path, score) in sorted {
            if let Some(entry) = self.get_note_index(&path, agent_id).await? {
                entries.push((entry, score));
            }
        }
        let rows: Vec<NoteIndexEntry> = entries.iter().map(|(e, _)| e.clone()).collect();
        let contents = super::helpers::load_note_contents_from_disk(&rows, agent_id).await;

        let results = entries
            .into_iter()
            .zip(contents)
            .map(
                |((entry, score), content)| crate::memory::notes::NoteSearchResult {
                    path: entry.path,
                    filename: entry.filename,
                    category: entry.category,
                    tags: entry.tags,
                    content,
                    score,
                    created_at: entry.created_at,
                    updated_at: entry.updated_at,
                },
            )
            .collect();

        Ok(crate::memory::notes::store::HybridSearchOutcome {
            results,
            vector_candidates: vec_results.len(),
            fts_candidates: fts_entries.len(),
        })
    }

    async fn vector_search_notes_with_content(
        &self,
        embedding: &[f32],
        agent_id: &str,
        dim_hint: u32,
        limit: usize,
    ) -> Result<Vec<crate::memory::notes::NoteSearchResult>, AlephError> {
        let pairs = self
            .vector_search(embedding, dim_hint, agent_id, limit)
            .await?;

        let mut entries = Vec::with_capacity(pairs.len());
        for (path, score) in pairs {
            if let Some(entry) = self.get_note_index(&path, agent_id).await? {
                entries.push((entry, score));
            }
        }
        let rows: Vec<NoteIndexEntry> = entries.iter().map(|(e, _)| e.clone()).collect();
        let contents = super::helpers::load_note_contents_from_disk(&rows, agent_id).await;

        Ok(entries
            .into_iter()
            .zip(contents)
            .map(
                |((entry, score), content)| crate::memory::notes::NoteSearchResult {
                    path: entry.path,
                    filename: entry.filename,
                    category: entry.category,
                    tags: entry.tags,
                    content,
                    score,
                    created_at: entry.created_at,
                    updated_at: entry.updated_at,
                },
            )
            .collect())
    }

    async fn get_notes_with_content(
        &self,
        agent_id: &str,
        paths: &[String],
    ) -> Result<Vec<crate::memory::notes::NoteSearchResult>, AlephError> {
        let mut entries = Vec::with_capacity(paths.len());
        for path in paths {
            if let Some(entry) = self.get_note_index(path, agent_id).await? {
                entries.push(entry);
            }
        }
        let contents = super::helpers::load_note_contents_from_disk(&entries, agent_id).await;
        Ok(entries
            .into_iter()
            .zip(contents)
            .map(|(entry, content)| crate::memory::notes::NoteSearchResult {
                path: entry.path,
                filename: entry.filename,
                category: entry.category,
                tags: entry.tags,
                content,
                score: 0.0,
                created_at: entry.created_at,
                updated_at: entry.updated_at,
            })
            .collect())
    }

    async fn get_notes_by_category(
        &self,
        agent_id: &str,
        category: &str,
        limit: usize,
    ) -> Result<Vec<NoteIndexEntry>, AlephError> {
        let all = self.list_notes(agent_id).await?;
        Ok(all
            .into_iter()
            .filter(|n| n.category == category)
            .take(limit)
            .collect())
    }

    async fn get_embedding(
        &self,
        path: &str,
        agent_id: &str,
        dim_hint: u32,
    ) -> Result<Option<Vec<f32>>, AlephError> {
        let conn = lock_conn!(self)?;

        // Look up rowid via notes_vec_map
        let rowid: Option<i64> = conn
            .query_row(
                "SELECT rowid FROM notes_vec_map WHERE path = ?1 AND agent_id = ?2",
                params![path, agent_id],
                |row| row.get(0),
            )
            .ok();

        let Some(rowid) = rowid else {
            return Ok(None);
        };

        let table = vec::notes_vec_table_for_dim(dim_hint)?;

        let sql = format!("SELECT embedding FROM {table} WHERE rowid = ?1");
        let blob: Option<Vec<u8>> = conn.query_row(&sql, params![rowid], |row| row.get(0)).ok();

        Ok(blob.map(|b| {
            // Decode strictly: a blob whose byte length is not a multiple of 4
            // (or whose decoded length disagrees with the table's dimension)
            // is corrupt, not a vector — returning a wrong-length Vec<f32>
            // would silently poison downstream cosine similarity. `chunks_exact`
            // drops a trailing partial f32 silently, so validate first.
            if b.len() % 4 != 0 {
                tracing::warn!(
                    path = %path,
                    blob_len = b.len(),
                    "get_embedding: stored embedding blob is not a multiple of 4 bytes; \
                     treating as missing"
                );
                return Vec::new();
            }
            let decoded: Vec<f32> = b
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect();
            if decoded.len() != dim_hint as usize {
                tracing::warn!(
                    path = %path,
                    expected_dim = dim_hint,
                    actual_dim = decoded.len(),
                    "get_embedding: stored embedding dimension disagrees with table; \
                     treating as missing"
                );
                return Vec::new();
            }
            decoded
        }))
    }

    async fn vector_search(
        &self,
        embedding: &[f32],
        dim: u32,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<(String, f32)>, AlephError> {
        let table = vec::notes_vec_table_for_dim(dim)?;
        let conn = lock_conn!(self)?;
        let blob = vec::embedding_to_blob(embedding);

        // Overshoot k to account for agent_id post-filtering. Bound the result
        // set BEFORE building the `IN (...)` placeholder list below: SQLite's
        // SQLITE_MAX_VARIABLE_NUMBER is 32 766 by default, and every rowid is
        // one placeholder — an unclamped `limit` (an RPC parameter) could push
        // the statement past the limit and fail with a misleading "too many
        // SQL variables" error. 5 000 leaves generous headroom for the extra
        // `agent_id` bind (mirrors the BATCH_SIZE in `prune_orphan_vectors`).
        const MAX_KNN_K: usize = 5_000;
        let k = limit.saturating_mul(3).max(limit).min(MAX_KNN_K);

        // Step 1: KNN search on the notes vec0 table alone (sqlite-vec requirement)
        let knn_results = {
            let mut knn_stmt = conn
                .prepare(&format!(
                    "SELECT rowid, distance FROM {table} WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2"
                ))
                .map_err(|e| AlephError::config(format!("vector_search knn prepare: {e}")))?;

            let rows = knn_stmt
                .query_map(params![blob, k as i64], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
                })
                .map_err(|e| AlephError::config(format!("vector_search knn query: {e}")))?;

            let mut results = Vec::with_capacity(k);
            for row in rows {
                let pair =
                    row.map_err(|e| AlephError::config(format!("vector_search knn row: {e}")))?;
                results.push(pair);
            }
            results
        };

        if knn_results.is_empty() {
            return Ok(Vec::new());
        }

        // Step 2: Look up paths and filter by agent_id via the map table
        let rowid_placeholders: Vec<String> =
            (1..=knn_results.len()).map(|i| format!("?{i}")).collect();
        let agent_param_idx = knn_results.len() + 1;

        let sql = format!(
            "SELECT rowid, path FROM notes_vec_map \
             WHERE rowid IN ({}) AND agent_id = ?{agent_param_idx}",
            rowid_placeholders.join(", ")
        );

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AlephError::config(format!("vector_search prepare: {e}")))?;

        // Build distance lookup
        let distance_map: std::collections::HashMap<i64, f64> =
            knn_results.iter().copied().collect();

        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = knn_results
            .iter()
            .map(|(rowid, _)| Box::new(*rowid) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        param_values.push(Box::new(agent_id.to_string()));

        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(params_ref.as_slice(), |row| {
                let rowid: i64 = row.get(0)?;
                let path: String = row.get(1)?;
                Ok((rowid, path))
            })
            .map_err(|e| AlephError::config(format!("vector_search query: {e}")))?;

        let mut results: Vec<(String, f32)> = Vec::with_capacity(limit);
        for row in rows {
            let (rowid, path) =
                row.map_err(|e| AlephError::config(format!("vector_search row: {e}")))?;
            if let Some(&dist) = distance_map.get(&rowid) {
                results.push((path, dist as f32));
            }
        }

        // Sort by distance and truncate to limit
        results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);

        Ok(results)
    }

    async fn relink_unresolved(&self, agent_id: &str) -> Result<usize, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT id, to_raw FROM notes_links \
                 WHERE agent_id = ?1 AND status = 'dangling'",
            )
            .map_err(|e| AlephError::config(format!("relink prep: {e}")))?;

        let rows: Vec<(i64, String)> = stmt
            .query_map(params![agent_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| AlephError::config(format!("relink scan: {e}")))?
            .filter_map(|r| r.ok())
            .collect();

        // One prefetched context for every dangling row (was per-row double
        // query in the old resolve_target-mirroring code — this fixes the N+1).
        let resolve_ctx = super::helpers::build_resolve_context(&conn, agent_id)
            .map_err(|e| AlephError::config(format!("relink resolve ctx: {e}")))?;

        let mut updated = 0usize;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AlephError::config(format!("relink transaction: {e}")))?;
        for (id, raw) in rows {
            let r = crate::memory::notes::links::resolve(&raw, &resolve_ctx);
            if let Some(target) = &r.target {
                // `OR IGNORE`: two dangling variants on the same from_note can
                // both resolve to the same target once it exists (tier-4
                // normalized matching makes this easy, e.g. "[[Rust Guide]]"
                // and "[[rust guide]]"). The second UPDATE would otherwise
                // violate UNIQUE(agent_id, from_note, to_note) and — with no
                // transaction wrapping this loop — abort the whole pass,
                // leaving every remaining dangling row unprocessed while
                // earlier updates stay applied. `OR IGNORE` skips the losing
                // row instead of erroring.
                let changed = tx
                    .execute(
                        "UPDATE OR IGNORE notes_links SET to_note = ?1, confidence = ?2, \
                                resolved_by = ?3, status = 'active' WHERE id = ?4",
                        params![target, r.confidence, r.resolved_by.map(|s| s.as_str()), id],
                    )
                    .map_err(|e| AlephError::config(format!("relink update: {e}")))?;

                // If the UPDATE was ignored, this row is now a redundant
                // dangling duplicate of the edge that won — remove it. A
                // no-op when the UPDATE above succeeded, since status is
                // already 'active' by the time this runs.
                tx.execute(
                    "DELETE FROM notes_links WHERE id = ?1 AND status = 'dangling'",
                    params![id],
                )
                .map_err(|e| AlephError::config(format!("relink cleanup: {e}")))?;

                if changed > 0 {
                    updated += 1;
                }
            }
        }
        tx.commit()
            .map_err(|e| AlephError::config(format!("relink commit: {e}")))?;
        Ok(updated)
    }

    async fn backfill_inbound_links(
        &self,
        agent_id: &str,
        keys: &[String],
    ) -> Result<usize, AlephError> {
        use crate::memory::notes::links;
        if keys.is_empty() {
            return Ok(0);
        }
        let conn = lock_conn!(self)?;
        let placeholders: Vec<String> = (2..=keys.len() + 1).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "SELECT id, to_raw FROM notes_links \
             WHERE agent_id = ?1 AND status IN ('dangling','tombstone') \
               AND to_raw IN ({})",
            placeholders.join(", ")
        );
        let mut params_v: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(agent_id.to_string())];
        for k in keys {
            // rust-doctor-disable-next-line excessive-clone
            params_v.push(Box::new(k.clone()));
        }
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            params_v.iter().map(|p| p.as_ref()).collect();
        let rows: Vec<(i64, String)> = {
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| AlephError::config(format!("backfill prep: {e}")))?;
            let scanned: Vec<(i64, String)> = stmt
                .query_map(params_ref.as_slice(), |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(|e| AlephError::config(format!("backfill scan: {e}")))?
                .filter_map(|r| r.ok())
                .collect();
            scanned
        };
        if rows.is_empty() {
            return Ok(0);
        }
        let ctx = super::helpers::build_resolve_context(&conn, agent_id)
            .map_err(|e| AlephError::config(format!("backfill ctx: {e}")))?;
        let mut revived = 0usize;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AlephError::config(format!("backfill transaction: {e}")))?;
        for (id, raw) in rows {
            let r = links::resolve(&raw, &ctx);
            if let Some(target) = r.target {
                // UNIQUE(agent_id, from_note, to_note) defense, mirroring
                // relink_unresolved above: reviving this row's to_note can
                // collide with another row already occupying that (from, to)
                // key on the same from_note (e.g. the source note was
                // manually re-linked to the target through a different raw
                // wikilink text while this row was still dangling/tombstone).
                // `UPDATE OR IGNORE` skips the losing row instead of erroring
                // out mid-pass; the follow-up DELETE clears the now-redundant
                // loser so it doesn't linger as a permanent duplicate.
                let changed = tx
                    .execute(
                        "UPDATE OR IGNORE notes_links SET to_note = ?1, confidence = ?2, \
                                resolved_by = ?3, status = 'active' WHERE id = ?4",
                        params![target, r.confidence, r.resolved_by.map(|s| s.as_str()), id],
                    )
                    .map_err(|e| AlephError::config(format!("backfill update: {e}")))?;

                tx.execute(
                    "DELETE FROM notes_links WHERE id = ?1 AND status IN ('dangling', 'tombstone')",
                    params![id],
                )
                .map_err(|e| AlephError::config(format!("backfill cleanup: {e}")))?;

                if changed > 0 {
                    revived += 1;
                }
            }
        }
        tx.commit()
            .map_err(|e| AlephError::config(format!("backfill commit: {e}")))?;
        Ok(revived)
    }

    // -----------------------------------------------------------------
    // Knowledge-graph materialization (Phase 4).
    // -----------------------------------------------------------------

    async fn load_graph_snapshot(&self, agent_id: &str) -> Result<GraphSnapshot, AlephError> {
        let conn = lock_conn!(self)?;

        // Collect (path, category) first so the prepared-statement borrow is
        // released before the per-node `notes_sources` lookups reuse `conn`.
        let node_meta: Vec<(String, String)> = {
            let mut stmt = conn
                .prepare("SELECT path, category FROM notes_index WHERE agent_id = ?1")
                .map_err(|e| AlephError::config(format!("load_graph_snapshot nodes prep: {e}")))?;
            let rows = stmt
                .query_map(params![agent_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(|e| AlephError::config(format!("load_graph_snapshot nodes query: {e}")))?;
            // rust-doctor-disable-next-line unnecessary-allocation
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(|e| {
                    AlephError::config(format!("load_graph_snapshot node row: {e}"))
                })?);
            }
            out
        };

        let mut nodes = Vec::with_capacity(node_meta.len());
        // One query for ALL source refs instead of N prepared statements — the
        // previous per-node loop issued a prepare+query per node, which on a
        // large vault is thousands of round-trips on a shared connection.
        let mut source_rows: Vec<(String, String)> = Vec::new();
        {
            let mut s2 = conn
                .prepare(
                    "SELECT note_path, source_ref FROM notes_sources \
                     WHERE agent_id = ?1 ORDER BY note_path",
                )
                .map_err(|e| {
                    AlephError::config(format!("load_graph_snapshot sources prep: {e}"))
                })?;
            let srows = s2
                .query_map(params![agent_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(|e| {
                    AlephError::config(format!("load_graph_snapshot sources query: {e}"))
                })?;
            for s in srows {
                source_rows.push(s.map_err(|e| {
                    AlephError::config(format!("load_graph_snapshot source row: {e}"))
                })?);
            }
        }
        // Consume in one pass: `source_rows` is sorted by note_path, so a
        // single walk groups refs per node without extra allocation.
        let mut src_iter = source_rows.into_iter().peekable();
        for (path, category) in node_meta {
            // rust-doctor-disable-next-line unnecessary-allocation
            let mut sources = Vec::new();
            while src_iter
                .peek()
                .is_some_and(|(p, _)| p.as_str() == path.as_str())
            {
                sources.push(src_iter.next().unwrap().1);
            }
            nodes.push(GraphNode {
                path,
                category,
                sources,
            });
        }

        // Resolved edges only (skip unresolved bare-filename links).
        let mut edges = Vec::new();
        {
            let mut stmt = conn
                .prepare(
                    "SELECT from_note, to_note, relation, confidence FROM notes_links \
                     WHERE agent_id = ?1 AND to_note <> '' AND instr(to_note, '/') > 0 \
                     AND status = 'active'",
                )
                .map_err(|e| AlephError::config(format!("load_graph_snapshot edges prep: {e}")))?;
            let rows = stmt
                .query_map(params![agent_id], |r| {
                    Ok(GraphEdge {
                        from: r.get::<_, String>(0)?,
                        to: r.get::<_, String>(1)?,
                        rel_type: r.get::<_, Option<String>>(2)?,
                        confidence: r.get::<_, f32>(3)?,
                    })
                })
                .map_err(|e| AlephError::config(format!("load_graph_snapshot edges query: {e}")))?;
            for row in rows {
                edges.push(row.map_err(|e| {
                    AlephError::config(format!("load_graph_snapshot edge row: {e}"))
                })?);
            }
        }

        Ok(GraphSnapshot { nodes, edges })
    }

    async fn replace_co_recall_links(
        &self,
        agent_id: &str,
        rows: &[(String, String, f32)],
    ) -> Result<(), AlephError> {
        use crate::memory::notes::graph::CO_RECALLED_RELATION;
        let conn = lock_conn!(self)?;
        // Wrap DELETE + INSERT loop in a single transaction so a mid-loop
        // failure cannot leave the co-recall edge set half-replaced (the full
        // refresh must be all-or-nothing), and so the per-row INSERTs don't each
        // autocommit (one fsync per row) during a dream recompute.
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AlephError::config(format!("replace_co_recall_links tx begin: {e}")))?;
        // Full refresh: the co-recall edge set is re-aggregated from
        // `recall_signals` every dream cycle, so stale pairs must not linger.
        tx.execute(
            "DELETE FROM notes_links WHERE agent_id = ?1 AND relation = ?2",
            params![agent_id, CO_RECALLED_RELATION],
        )
        .map_err(|e| AlephError::config(format!("replace_co_recall_links delete: {e}")))?;
        // DO NOTHING on conflict: an existing semantic link (wikilink / typed
        // relation) for the pair always wins over the behavioral edge.
        for (from, to, confidence) in rows {
            tx.execute(
                "INSERT INTO notes_links \
                   (agent_id, from_note, to_note, to_raw, relation, confidence, status) \
                 VALUES (?1, ?2, ?3, ?3, ?4, ?5, 'active') \
                 ON CONFLICT(agent_id, from_note, to_note) DO NOTHING",
                params![
                    agent_id,
                    from,
                    to,
                    CO_RECALLED_RELATION,
                    f64::from(*confidence)
                ],
            )
            .map_err(|e| AlephError::config(format!("replace_co_recall_links insert: {e}")))?;
        }
        tx.commit()
            .map_err(|e| AlephError::config(format!("replace_co_recall_links tx commit: {e}")))?;
        Ok(())
    }

    async fn replace_mention_links(
        &self,
        agent_id: &str,
        rows: &[(String, String)],
    ) -> Result<(), AlephError> {
        use crate::memory::notes::links::mentions::{MENTION_CONFIDENCE, MENTION_RELATION};
        let conn = lock_conn!(self)?;
        // Wrap DELETE + INSERT loop in a single transaction so a mid-loop
        // failure cannot leave the mention edge set half-replaced (the full
        // refresh must be all-or-nothing).
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AlephError::config(format!("replace_mention_links tx begin: {e}")))?;
        // Full refresh: mentions are re-scanned from the whole corpus every
        // dream cycle, so stale pairs must not linger.
        tx.execute(
            "DELETE FROM notes_links WHERE agent_id = ?1 AND relation = ?2",
            params![agent_id, MENTION_RELATION],
        )
        .map_err(|e| AlephError::config(format!("replace_mention_links delete: {e}")))?;
        // DO NOTHING on conflict: an existing semantic link (wikilink / typed
        // relation) for the pair always wins over the mention soft edge.
        for (from, to) in rows {
            tx.execute(
                "INSERT INTO notes_links \
                   (agent_id, from_note, to_note, to_raw, relation, confidence, resolved_by, status) \
                 VALUES (?1, ?2, ?3, ?3, ?4, ?5, 'mention_scan', 'active') \
                 ON CONFLICT(agent_id, from_note, to_note) DO NOTHING",
                params![agent_id, from, to, MENTION_RELATION, f64::from(MENTION_CONFIDENCE)],
            )
            .map_err(|e| AlephError::config(format!("replace_mention_links insert: {e}")))?;
        }
        tx.commit()
            .map_err(|e| AlephError::config(format!("replace_mention_links tx commit: {e}")))?;
        Ok(())
    }

    async fn replace_graph_cache(
        &self,
        agent_id: &str,
        rows: &[(String, usize, f32, usize)],
    ) -> Result<(), AlephError> {
        let now = chrono::Utc::now().timestamp();
        let conn = lock_conn!(self)?;
        conn.execute(
            "DELETE FROM notes_graph_cache WHERE agent_id = ?1",
            params![agent_id],
        )
        .map_err(|e| AlephError::config(format!("replace_graph_cache delete: {e}")))?;
        for (path, comm, coh, deg) in rows {
            conn.execute(
                "INSERT OR REPLACE INTO notes_graph_cache \
                 (agent_id, node_path, community_id, cohesion, degree, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    agent_id,
                    path,
                    *comm as i64,
                    f64::from(*coh),
                    *deg as i64,
                    now
                ],
            )
            .map_err(|e| AlephError::config(format!("replace_graph_cache insert: {e}")))?;
        }
        Ok(())
    }

    async fn replace_graph_insights(
        &self,
        agent_id: &str,
        rows: &[(String, String)],
    ) -> Result<(), AlephError> {
        let now = chrono::Utc::now().timestamp();
        let conn = lock_conn!(self)?;
        conn.execute(
            "DELETE FROM notes_graph_insights WHERE agent_id = ?1",
            params![agent_id],
        )
        .map_err(|e| AlephError::config(format!("replace_graph_insights delete: {e}")))?;
        for (kind, payload) in rows {
            conn.execute(
                "INSERT INTO notes_graph_insights (agent_id, kind, payload, created_at) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![agent_id, kind, payload, now],
            )
            .map_err(|e| AlephError::config(format!("replace_graph_insights insert: {e}")))?;
        }
        Ok(())
    }

    async fn read_graph_insights(
        &self,
        agent_id: &str,
        kind: Option<&str>,
    ) -> Result<Vec<(String, String)>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut out = Vec::new();
        match kind {
            Some(k) => {
                let mut stmt = conn
                    .prepare(
                        "SELECT kind, payload FROM notes_graph_insights \
                         WHERE agent_id = ?1 AND kind = ?2",
                    )
                    .map_err(|e| AlephError::config(format!("read_graph_insights prep: {e}")))?;
                let rows = stmt
                    .query_map(params![agent_id, k], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })
                    .map_err(|e| AlephError::config(format!("read_graph_insights query: {e}")))?;
                for row in rows {
                    out.push(row.map_err(|e| {
                        AlephError::config(format!("read_graph_insights row: {e}"))
                    })?);
                }
            }
            None => {
                let mut stmt = conn
                    .prepare("SELECT kind, payload FROM notes_graph_insights WHERE agent_id = ?1")
                    .map_err(|e| AlephError::config(format!("read_graph_insights prep: {e}")))?;
                let rows = stmt
                    .query_map(params![agent_id], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })
                    .map_err(|e| AlephError::config(format!("read_graph_insights query: {e}")))?;
                for row in rows {
                    out.push(row.map_err(|e| {
                        AlephError::config(format!("read_graph_insights row: {e}"))
                    })?);
                }
            }
        }
        Ok(out)
    }

    async fn community_peers(
        &self,
        agent_id: &str,
        node_path: &str,
        limit: usize,
    ) -> Result<Vec<String>, AlephError> {
        let conn = lock_conn!(self)?;
        let cid: Option<i64> = conn
            .query_row(
                "SELECT community_id FROM notes_graph_cache \
                 WHERE agent_id = ?1 AND node_path = ?2",
                params![agent_id, node_path],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| AlephError::config(format!("community_peers lookup: {e}")))?;
        let Some(cid) = cid else {
            return Ok(vec![]);
        };
        let mut stmt = conn
            .prepare(
                "SELECT node_path FROM notes_graph_cache \
                 WHERE agent_id = ?1 AND community_id = ?2 AND node_path <> ?3 \
                 LIMIT ?4",
            )
            .map_err(|e| AlephError::config(format!("community_peers prepare: {e}")))?;
        let rows = stmt
            .query_map(params![agent_id, cid, node_path, limit as i64], |r| {
                r.get::<_, String>(0)
            })
            .map_err(|e| AlephError::config(format!("community_peers query: {e}")))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| AlephError::config(format!("community_peers row: {e}")))?);
        }
        Ok(out)
    }

    async fn relation_type_counts(&self, agent_id: &str) -> Result<Vec<(String, i64)>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut stmt = conn
            .prepare(
                "SELECT COALESCE(relation, 'link') AS rel, COUNT(*) AS n \
                 FROM notes_links \
                 WHERE agent_id = ?1 AND status = 'active' \
                 GROUP BY rel \
                 ORDER BY n DESC, rel",
            )
            .map_err(|e| AlephError::config(format!("relation_type_counts prepare: {e}")))?;
        let rows = stmt
            .query_map(params![agent_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })
            .map_err(|e| AlephError::config(format!("relation_type_counts query: {e}")))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(
                row.map_err(|e| AlephError::config(format!("relation_type_counts row: {e}")))?,
            );
        }
        Ok(out)
    }

    async fn replace_graph_related(
        &self,
        agent_id: &str,
        rows: &[(String, String, f32)],
    ) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;
        // Wrap DELETE + INSERT loop in a single transaction so a mid-loop
        // failure cannot leave the related-edge set half-replaced (the full
        // refresh must be all-or-nothing), and so the per-row INSERTs don't each
        // autocommit (one fsync per row) during a dream recompute.
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AlephError::config(format!("replace_graph_related tx begin: {e}")))?;
        tx.execute(
            "DELETE FROM notes_graph_related WHERE agent_id = ?1",
            params![agent_id],
        )
        .map_err(|e| AlephError::config(format!("replace_graph_related delete: {e}")))?;
        for (node, related, score) in rows {
            tx.execute(
                "INSERT OR REPLACE INTO notes_graph_related \
                 (agent_id, node_path, related_path, score) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![agent_id, node, related, f64::from(*score)],
            )
            .map_err(|e| AlephError::config(format!("replace_graph_related insert: {e}")))?;
        }
        tx.commit()
            .map_err(|e| AlephError::config(format!("replace_graph_related tx commit: {e}")))?;
        Ok(())
    }

    async fn related_peers(
        &self,
        agent_id: &str,
        node_path: &str,
        limit: usize,
    ) -> Result<Vec<(String, f32)>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut stmt = conn
            .prepare(
                "SELECT related_path, score FROM notes_graph_related \
                 WHERE agent_id = ?1 AND node_path = ?2 \
                 ORDER BY score DESC LIMIT ?3",
            )
            .map_err(|e| AlephError::config(format!("related_peers prepare: {e}")))?;
        let rows = stmt
            .query_map(params![agent_id, node_path, limit as i64], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)? as f32))
            })
            .map_err(|e| AlephError::config(format!("related_peers query: {e}")))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| AlephError::config(format!("related_peers row: {e}")))?);
        }
        Ok(out)
    }

    async fn related_edges_between(
        &self,
        agent_id: &str,
        visible: &std::collections::HashSet<String>,
        per_node: usize,
    ) -> Result<Vec<(String, String, f32)>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut stmt = conn
            .prepare(
                "SELECT node_path, related_path, score FROM notes_graph_related \
                 WHERE agent_id = ?1 ORDER BY node_path, score DESC",
            )
            .map_err(|e| AlephError::config(format!("related_edges prep: {e}")))?;
        let rows = stmt
            .query_map(params![agent_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, f64>(2)? as f32,
                ))
            })
            .map_err(|e| AlephError::config(format!("related_edges query: {e}")))?;
        let mut out = Vec::new();
        let mut count_for: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for row in rows {
            let (node, related, score) =
                row.map_err(|e| AlephError::config(format!("related_edges row: {e}")))?;
            if !visible.contains(&node) || !visible.contains(&related) {
                continue;
            }
            // rust-doctor-disable-next-line excessive-clone
            let c = count_for.entry(node.clone()).or_insert(0);
            if *c >= per_node {
                continue; // rows are score-DESC within node_path → top-K kept
            }
            *c += 1;
            out.push((node, related, score));
        }
        Ok(out)
    }

    // -----------------------------------------------------------------
    // Phase C2.9.2 governance: per-fact provenance + async review queue.
    // -----------------------------------------------------------------

    async fn upsert_provenance(
        &self,
        agent_id: &str,
        note_path: &str,
        provs: &[FactProvenance],
    ) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;

        conn.execute(
            "DELETE FROM notes_provenance WHERE agent_id = ?1 AND note_path = ?2",
            params![agent_id, note_path],
        )
        .map_err(|e| AlephError::config(format!("upsert_provenance delete: {e}")))?;

        let now_ts = chrono::Utc::now().timestamp();
        for (idx, p) in provs.iter().enumerate() {
            let origin_str = provenance_origin_to_str(&p.origin);
            let source_kind: Option<&str> = match p.origin {
                ProvenanceOrigin::RawSource => Some("raw"),
                ProvenanceOrigin::PriorNote => Some("note"),
                _ => None,
            };
            conn.execute(
                "INSERT INTO notes_provenance \
                 (agent_id, note_path, fact_idx, origin, source_kind, source_id, inferred, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    agent_id,
                    note_path,
                    idx as i64,
                    origin_str,
                    source_kind,
                    p.source_id,
                    i64::from(p.inferred),
                    now_ts,
                ],
            )
            .map_err(|e| AlephError::config(format!("upsert_provenance insert: {e}")))?;
        }
        Ok(())
    }

    async fn get_provenance(
        &self,
        agent_id: &str,
        note_path: &str,
    ) -> Result<Vec<FactProvenance>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT origin, source_id, inferred FROM notes_provenance \
                 WHERE agent_id = ?1 AND note_path = ?2 \
                 ORDER BY fact_idx ASC",
            )
            .map_err(|e| AlephError::config(format!("get_provenance prepare: {e}")))?;

        let rows = stmt
            .query_map(params![agent_id, note_path], |row| {
                let origin: String = row.get(0)?;
                let source_id: Option<String> = row.get(1)?;
                let inferred: i64 = row.get(2)?;
                Ok(FactProvenance {
                    origin: provenance_origin_from_str(&origin),
                    source_id,
                    inferred: inferred != 0,
                })
            })
            .map_err(|e| AlephError::config(format!("get_provenance query: {e}")))?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| AlephError::config(format!("get_provenance row: {e}")))?);
        }
        Ok(out)
    }

    async fn enqueue_review(
        &self,
        agent_id: &str,
        candidate_json: &str,
        severity: &str,
        confidence: f32,
        reason: &str,
    ) -> Result<String, AlephError> {
        let conn = lock_conn!(self)?;
        let id = uuid::Uuid::new_v4().to_string();
        let now_ts = chrono::Utc::now().timestamp();

        conn.execute(
            "INSERT INTO notes_review_queue \
             (id, agent_id, candidate_json, severity, confidence, reason, status, retry_count, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', 0, ?7)",
            params![id, agent_id, candidate_json, severity, confidence, reason, now_ts],
        )
        .map_err(|e| AlephError::config(format!("enqueue_review insert: {e}")))?;

        Ok(id)
    }

    async fn list_pending_review(
        &self,
        agent_id: &str,
        earlier_than: i64,
    ) -> Result<Vec<ReviewQueueRow>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT id, agent_id, candidate_json, severity, confidence, reason, \
                        status, retry_count, created_at \
                 FROM notes_review_queue \
                 WHERE agent_id = ?1 AND status = 'pending' AND created_at < ?2 \
                 ORDER BY created_at ASC",
            )
            .map_err(|e| AlephError::config(format!("list_pending_review prepare: {e}")))?;

        let rows = stmt
            .query_map(params![agent_id, earlier_than], |row| {
                Ok(ReviewQueueRow {
                    id: row.get(0)?,
                    agent_id: row.get(1)?,
                    candidate_json: row.get(2)?,
                    severity: row.get(3)?,
                    confidence: row.get::<_, f64>(4)? as f32,
                    reason: row.get(5)?,
                    status: row.get(6)?,
                    retry_count: row.get(7)?,
                    created_at: row.get(8)?,
                })
            })
            .map_err(|e| AlephError::config(format!("list_pending_review query: {e}")))?;

        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| AlephError::config(format!("list_pending_review row: {e}")))?);
        }
        Ok(out)
    }

    async fn mark_review_decided(
        &self,
        queue_id: &str,
        new_status: &str,
        decision_actor: &str,
    ) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;
        let now_ts = chrono::Utc::now().timestamp();
        conn.execute(
            "UPDATE notes_review_queue \
             SET status = ?1, decision_actor = ?2, decided_at = ?3 \
             WHERE id = ?4",
            params![new_status, decision_actor, now_ts, queue_id],
        )
        .map_err(|e| AlephError::config(format!("mark_review_decided update: {e}")))?;
        Ok(())
    }

    async fn increment_review_retry(&self, queue_id: &str) -> Result<i64, AlephError> {
        let conn = lock_conn!(self)?;
        conn.execute(
            "UPDATE notes_review_queue SET retry_count = retry_count + 1 WHERE id = ?1",
            params![queue_id],
        )
        .map_err(|e| AlephError::config(format!("increment_review_retry update: {e}")))?;
        let count: i64 = conn
            .query_row(
                "SELECT retry_count FROM notes_review_queue WHERE id = ?1",
                params![queue_id],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("increment_review_retry read: {e}")))?;
        Ok(count)
    }

    async fn archive_review(&self, queue_id: &str, final_status: &str) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;
        // Wrap INSERT + DELETE in a single transaction so a crash mid-archive
        // cannot leave a row in both tables (or in neither).
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AlephError::config(format!("archive_review tx begin: {e}")))?;
        let now_ts = chrono::Utc::now().timestamp();

        tx.execute(
            "INSERT INTO notes_review_archive \
             (id, agent_id, candidate_json, final_status, reason, created_at, archived_at) \
             SELECT id, agent_id, candidate_json, ?1, reason, created_at, ?2 \
             FROM notes_review_queue WHERE id = ?3",
            params![final_status, now_ts, queue_id],
        )
        .map_err(|e| AlephError::config(format!("archive_review insert: {e}")))?;

        tx.execute(
            "DELETE FROM notes_review_queue WHERE id = ?1",
            params![queue_id],
        )
        .map_err(|e| AlephError::config(format!("archive_review delete: {e}")))?;

        tx.commit()
            .map_err(|e| AlephError::config(format!("archive_review tx commit: {e}")))?;
        Ok(())
    }

    async fn list_review_archive(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ReviewArchiveRow>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut stmt = conn
            .prepare(
                "SELECT id, candidate_json, final_status, reason, created_at, archived_at \
                 FROM notes_review_archive WHERE agent_id = ?1 \
                 ORDER BY archived_at DESC LIMIT ?2",
            )
            .map_err(|e| AlephError::config(format!("list_review_archive prepare: {e}")))?;
        let rows = stmt
            .query_map(params![agent_id, limit as i64], |r| {
                Ok(ReviewArchiveRow {
                    id: r.get(0)?,
                    candidate_json: r.get(1)?,
                    final_status: r.get(2)?,
                    reason: r.get(3)?,
                    created_at: r.get(4)?,
                    archived_at: r.get(5)?,
                })
            })
            .map_err(|e| AlephError::config(format!("list_review_archive query: {e}")))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| AlephError::config(format!("list_review_archive row: {e}")))?);
        }
        Ok(out)
    }

    async fn prune_review_archive(
        &self,
        agent_id: &str,
        older_than_secs: i64,
    ) -> Result<usize, AlephError> {
        let conn = lock_conn!(self)?;
        let cutoff = chrono::Utc::now().timestamp() - older_than_secs;
        let n = conn
            .execute(
                "DELETE FROM notes_review_archive WHERE agent_id = ?1 AND archived_at < ?2",
                params![agent_id, cutoff],
            )
            .map_err(|e| AlephError::config(format!("prune_review_archive: {e}")))?;
        Ok(n)
    }

    /// Phase C2.7 — return the most recent `created_at` recall signal for
    /// `(agent_id, note_path)`, or `None` when no signals exist. Scoped to the
    /// recording agent so a sibling agent's recall of a same-named note can't
    /// resurrect this one from decay.
    async fn recall_signals_last_hit(
        &self,
        agent_id: &str,
        note_path: &str,
    ) -> Result<Option<i64>, AlephError> {
        let conn = lock_conn!(self)?;
        let v: Option<i64> = conn
            .query_row(
                "SELECT MAX(created_at) FROM recall_signals \
                 WHERE note_path = ?1 AND agent_id = ?2",
                params![note_path, agent_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| AlephError::config(format!("recall last hit: {e}")))?
            .flatten();
        Ok(v)
    }

    async fn recall_hit_counts(
        &self,
        agent_id: &str,
        note_paths: &[String],
    ) -> Result<std::collections::HashMap<String, i64>, AlephError> {
        // Reuse the existing recall-signal aggregator; `signal_count` is the
        // per-note recall frequency (deduped by query/day/channel).
        let aggregates = self.aggregate_for_facts(agent_id, note_paths)?;
        Ok(aggregates
            .into_iter()
            .map(|a| (a.note_path, a.signal_count))
            .collect())
    }

    async fn record_recall_hits(
        &self,
        query: &str,
        channel: &str,
        hits: &[(String, f32)],
        agent_id: &str,
    ) -> Result<usize, AlephError> {
        if hits.is_empty() {
            return Ok(0);
        }
        // Reuse the existing recall-signal writer; it dedups per
        // (note_path, query_hash, day_bucket, channel) via INSERT OR IGNORE.
        let recall_hits: Vec<super::super::recall_signals::RecallHit> = hits
            .iter()
            .map(
                |(note_path, score)| super::super::recall_signals::RecallHit {
                    // rust-doctor-disable-next-line excessive-clone
                    note_path: note_path.clone(),
                    score: f64::from(*score),
                },
            )
            .collect();
        // The auto-recall path carries no distinct namespace, so the vestigial
        // `namespace` column mirrors `agent_id`.
        self.record_signals(query, channel, &recall_hits, None, agent_id, agent_id)
    }

    async fn sources_of(&self, agent_id: &str, note_path: &str) -> Result<Vec<String>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut stmt = conn
            .prepare("SELECT source_ref FROM notes_sources WHERE agent_id = ?1 AND note_path = ?2")
            .map_err(|e| AlephError::config(format!("sources_of prepare: {e}")))?;
        let rows = stmt
            .query_map(params![agent_id, note_path], |r| r.get::<_, String>(0))
            .map_err(|e| AlephError::config(format!("sources_of query: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| AlephError::config(format!("sources_of row: {e}")))?);
        }
        Ok(out)
    }

    async fn notes_citing(
        &self,
        agent_id: &str,
        source_ref: &str,
    ) -> Result<Vec<String>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut stmt = conn
            .prepare("SELECT note_path FROM notes_sources WHERE agent_id = ?1 AND source_ref = ?2")
            .map_err(|e| AlephError::config(format!("notes_citing prepare: {e}")))?;
        let rows = stmt
            .query_map(params![agent_id, source_ref], |r| r.get::<_, String>(0))
            .map_err(|e| AlephError::config(format!("notes_citing query: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| AlephError::config(format!("notes_citing row: {e}")))?);
        }
        Ok(out)
    }
}

#[cfg(test)]
impl SqliteMemoryBackend {
    /// Test helper: directly update the status of a link row for testing.
    /// Used to mark a link as tombstone without going through the full delete flow.
    pub async fn set_link_status_for_test(
        &self,
        from_path: &str,
        to_raw: &str,
        new_status: &str,
        agent_id: &str,
    ) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;
        conn.execute(
            "UPDATE notes_links SET status = ?1 WHERE agent_id = ?2 AND from_note = ?3 AND to_raw = ?4",
            params![new_status, agent_id, from_path, to_raw],
        )
        .map_err(|e| AlephError::config(format!("set_link_status_for_test: {e}")))?;
        Ok(())
    }
}
