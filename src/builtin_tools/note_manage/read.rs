//! Read surface: `query` (hybrid semantic + full-text) and `list`.
//!
//! The advisory this module builds is the point: it reports what the query
//! *actually ran*, not what the deployment was configured to run — "semantic
//! found nothing" and "semantic never executed" are opposite instructions
//! about whether to trust an empty result.

use tracing::warn;

use crate::error::{AlephError, Result};
use crate::memory::notes::canonicalize_category;
use crate::memory::notes::sanitize_title;
use crate::memory::notes::store::NoteStore;

use super::args::{NoteListEntry, NoteManageArgs, NoteManageResult, SearchAdvisory};
use super::helpers::{bound_chars, validate_category};
use super::NoteManageTool;

/// Per-note content cap in `query` results. A single sprawling note must not
/// crowd out the other hits (or the context window).
const PER_NOTE_MAX_CHARS: usize = 4_000;

/// Total content budget for one `query` response, in chars. Mirrors the
/// output-bounding discipline used by the browser tools' `bound_content`.
const TOTAL_CONTENT_MAX_CHARS: usize = 24_000;

/// Content ceiling for one `get`, in chars — 16x the per-hit `query` cap.
///
/// `query` budgets because it returns *many* notes and one sprawling hit must
/// not crowd out the rest; `get` returns exactly the note that was asked for,
/// so the only thing left to bound is the context window itself. Sixteen times
/// the survey cap clears every note the ingest and tool paths actually produce
/// while still refusing to hand a pathological file to the model whole.
const GET_MAX_CHARS: usize = 64_000;

/// recall_signals channel for explicit `note_manage(query)` look-ups. Distinct
/// from the auto-recall channel so the per-day dedup of the two paths is
/// independent (mirrors `note_retrieval::AUTO_RECALL_CHANNEL`).
const NOTE_MANAGE_RECALL_CHANNEL: &str = "note_manage";

/// `(path, category, filename, tags, content, score)` rows from `search_notes`.
type SearchRows = Vec<(String, String, String, Vec<String>, String, f32)>;

/// Why a `query` ran without its semantic leg.
#[derive(Debug, Clone, Copy)]
enum DegradedReason {
    /// FTS-only deployment — a steady state, not a fault.
    NoEmbedder,
    /// The embedding endpoint was unreachable.
    EmbedFailed,
    /// The embedding succeeded but the vector index could not serve it —
    /// most often a provider dimension with no matching vec0 table.
    VectorLegUnavailable,
}

impl DegradedReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::NoEmbedder => "no embedding provider configured",
            Self::EmbedFailed => "embedding provider unreachable",
            Self::VectorLegUnavailable => "vector index unavailable for this embedding dimension",
        }
    }
}

impl SearchAdvisory {
    fn fused(vector_candidates: usize, fts_candidates: usize) -> Self {
        let mode = match (vector_candidates, fts_candidates) {
            (0, _) => "full-text",
            (_, 0) => "semantic",
            _ => "hybrid",
        };
        Self {
            mode: mode.to_string(),
            vector_candidates,
            fts_candidates,
            degraded: None,
            bodies_omitted: None,
            bodies_unreadable: None,
        }
    }

    // The single previous call site was replaced with an inlined struct
    // literal at the `text_only` construction point so the new
    // `bodies_unreadable` field (BT-D-R4-22) cannot be forgotten — the
    // helper would have shadowed the new field by default-initialising it
    // to `None`, hiding the very signal we just added.
}

impl NoteManageTool {
    /// Hybrid (vector + FTS) search when an embedder is wired; a failed embed
    /// degrades to FTS rather than failing the query (P7). Returns
    /// `(path, category, filename, tags, content, score)` tuples plus the
    /// mode label used in the result message.
    async fn search_notes(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
    ) -> Result<(SearchRows, SearchAdvisory)> {
        // Three ways the vector leg can be absent, and the reason to degrade is
        // the same for all of them: the notes and the full-text index are both
        // local and intact. Only the first two were covered — a store-side
        // failure (typically an embedding dimension with no vec0 table) failed
        // the whole query, in a tool documented to fall back to full text.
        let degraded = match &self.embedder {
            None => Some(DegradedReason::NoEmbedder),
            Some(embedder) => match embedder.embed(query).await {
                Err(e) => {
                    warn!(error = %e, "note_manage query: embed failed — falling back to FTS");
                    Some(DegradedReason::EmbedFailed)
                }
                Ok(embedding) => {
                    let dim = embedding.len() as u32;
                    match self
                        .indexer
                        .store()
                        .hybrid_search_notes(&embedding, query, agent_id, dim, limit)
                        .await
                    {
                        Ok(outcome) => {
                            let rows = outcome
                                .results
                                .into_iter()
                                .map(|h| {
                                    (h.path, h.category, h.filename, h.tags, h.content, h.score)
                                })
                                .collect();
                            return Ok((
                                rows,
                                SearchAdvisory::fused(
                                    outcome.vector_candidates,
                                    outcome.fts_candidates,
                                ),
                            ));
                        }
                        Err(e) => {
                            warn!(
                                error = %e,
                                dim,
                                "note_manage query: vector leg unavailable — falling back to FTS"
                            );
                            Some(DegradedReason::VectorLegUnavailable)
                        }
                    }
                }
            },
        };

        let entries = self
            .indexer
            .store()
            .search_notes_fts(query, agent_id, limit)
            .await
            .map_err(|e| AlephError::tool(format!("Note search failed: {e}")))?;
        let fts_hits = entries.len();
        // Bodies are read against *this tool's* note root, through the shared
        // `note_content_path` derivation. The store's own loader is not reused
        // here on purpose: it resolves the root from the process-global
        // `utils::paths::get_note_memory_dir()` rather than from the indexer it
        // was called through, so borrowing it would trade one duplicated
        // derivation for a reader that can look in a different directory than
        // the writer used.
        let memory_dir = self.indexer.memory_dir().to_path_buf();
        // Per-body read result, so unreadable files count toward the advisory
        // instead of silently flattening to an empty body (the previous shape
        // inflated `fts_candidates` with rows the model could not see).
        let bodies = futures::future::join_all(entries.iter().map(|entry| {
            let path = crate::memory::notes::store::note_content_path(
                &memory_dir,
                agent_id,
                &entry.category,
                &entry.filename,
            );
            async move {
                match tokio::fs::read_to_string(&path).await {
                    Ok(content) => Ok(content),
                    Err(e) => {
                        warn!(
                            path = %path.display(),
                            error = %e,
                            "note_manage query: body read failed"
                        );
                        Err(e)
                    }
                }
            }
        }))
        .await;
        let bodies_unreadable = bodies.iter().filter(|r| r.is_err()).count();
        let rows: SearchRows = entries
            .into_iter()
            .zip(bodies)
            .enumerate()
            .map(|(rank, (entry, content))| {
                // Rank-derived pseudo score — FTS entries carry no fused score.
                let score = 1.0 / (1.0 + rank as f32);
                // `bodies` is `Vec<Result<String, io::Error>>` after BT-D-R4-22:
                // a per-body read failure is counted in
                // `bodies_unreadable` above. The row itself still surfaces
                // (the hit exists in the index) but its body is empty —
                // downstream consumers should check `bodies_unreadable` to
                // distinguish "found a thing but couldn't show it" from
                // "truncated to fit the response".
                let content = content.unwrap_or_default();
                (
                    entry.path,
                    entry.category,
                    entry.filename,
                    entry.tags,
                    content,
                    score,
                )
            })
            .collect();
        Ok((
            rows,
            SearchAdvisory {
                mode: "full-text".to_string(),
                vector_candidates: 0,
                fts_candidates: fts_hits,
                degraded: degraded.map(|r| r.as_str().to_string()),
                bodies_omitted: None,
                bodies_unreadable: (bodies_unreadable > 0).then_some(bodies_unreadable),
            },
        ))
    }

    pub(super) async fn handle_query(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id_owned = self.resolve_agent_id(args)?;
        let agent_id = agent_id_owned.as_str();

        let query = args
            .query
            .as_deref()
            .ok_or_else(|| AlephError::tool("query is required for query action"))?;

        let limit = args.limit.unwrap_or(20);

        let (results, mut advisory) = self.search_notes(query, agent_id, limit).await?;

        if results.is_empty() {
            return Ok(NoteManageResult {
                related_notes: None,
                success: true,
                message: format!("No notes found matching '{query}'"),
                destination: None,
                note_path: None,
                content: None,
                notes: Some(vec![]),
                // An empty result under a degraded mode reads very differently
                // from an empty result under a working one, so the advisory
                // rides along here too.
                search: Some(advisory),
            });
        }

        // Recall bookkeeping: notes the LLM explicitly looks up must accrue
        // recall signals, or the decay stage ages them as never-used.
        // Best-effort — a signal write failure never breaks the query.
        let hits: Vec<(String, f32)> = results
            .iter()
            .map(|(path, .., score)| (path.clone(), *score))
            .collect();
        if let Err(e) = self
            .indexer
            .store()
            .record_recall_hits(query, NOTE_MANAGE_RECALL_CHANNEL, &hits, agent_id)
            .await
        {
            tracing::debug!(error = %e, "note_manage query: recall signal write failed");
        }

        let mut notes = Vec::new();
        let mut combined_content = String::new();
        let mut bodies_omitted = 0usize;

        for (path, category, filename, tags, file_content, _score) in &results {
            // Budget the response: full metadata for every hit, but bodies stop
            // once the total content budget is spent — an unbounded query over
            // 20 full notes can flood the context window.
            if combined_content.len() < TOTAL_CONTENT_MAX_CHARS {
                let body = bound_chars(file_content, PER_NOTE_MAX_CHARS);
                combined_content.push_str(&format!("## {filename} ({path})\n\n{body}\n\n---\n\n"));
            } else {
                bodies_omitted += 1;
            }

            notes.push(NoteListEntry {
                path: path.clone(),
                category: category.clone(),
                filename: filename.clone(),
                tags: tags.clone(),
            });
        }
        if bodies_omitted > 0 {
            combined_content.push_str(&format!(
                "[{bodies_omitted} more matching note(s) listed above without bodies — \
                 read any of them whole with action `get`]\n"
            ));
            advisory.bodies_omitted = Some(bodies_omitted);
        }

        let mode = advisory.mode.clone();
        let suffix = advisory
            .degraded
            .as_deref()
            .map(|why| format!(" — semantic leg skipped: {why}"))
            .unwrap_or_default();
        Ok(NoteManageResult {
            related_notes: None,
            success: true,
            message: format!(
                "Found {} note(s) matching '{query}' ({mode} search){suffix}",
                notes.len()
            ),
            destination: None,
            note_path: None,
            content: Some(combined_content),
            notes: Some(notes),
            search: Some(advisory),
        })
    }

    /// `get` — read one note by address.
    ///
    /// The write surface replaces a note's body wholesale (`update`), and until
    /// this action existed the only body-returning read was `query`: a *ranked
    /// search* whose every hit is cut at [`PER_NOTE_MAX_CHARS`]. So the model
    /// could overwrite a note it had only ever seen 4,000 chars of, and the
    /// truncation marker sat in the very text it would copy forward. The Panel
    /// never had this problem — `graph.node_detail` reads the same file from
    /// disk, whole. This is that capability on the model's face.
    ///
    /// Two rules it does not share with `query`:
    ///
    /// * **The address must be unambiguous.** With no `category`, the filename
    ///   is resolved through the index; two notes of that name in different
    ///   categories is a refusal that names both, not a first-hit guess. Same
    ///   discipline as the wikilink resolver's tiers — a wrong note handed to a
    ///   wholesale rewrite is worse than no note.
    /// * **Disk is the answer.** The index row supplies tags and category; the
    ///   body comes from the markdown file, which is the source of truth. A row
    ///   whose file is gone reports missing rather than serving stale metadata.
    pub(super) async fn handle_get(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id_owned = self.resolve_agent_id(args)?;
        let agent_id = agent_id_owned.as_str();

        let filename = args
            .filename
            .as_deref()
            .ok_or_else(|| AlephError::tool("filename is required for get"))?;
        let safe_filename = sanitize_title(filename)?;

        let note_path = match args.category.as_deref() {
            Some(raw) => {
                let category = canonicalize_category(raw);
                validate_category(&category)?;
                format!("{category}/{safe_filename}")
            }
            None => {
                let mut hits = self
                    .indexer
                    .store()
                    .find_by_filename(&safe_filename, agent_id)
                    .await
                    .map_err(|e| AlephError::tool(format!("Failed to locate note: {e}")))?;
                hits.sort();
                match hits.len() {
                    0 => {
                        return Err(AlephError::tool(format!(
                            "Note '{safe_filename}' not found. Pass `category` if you know it, \
                             or use the `query` action to search."
                        )))
                    }
                    1 => hits.remove(0),
                    _ => {
                        return Err(AlephError::tool(format!(
                            "Note '{safe_filename}' exists in {} categories ({}). \
                             Pass `category` to say which one.",
                            hits.len(),
                            hits.join(", ")
                        )))
                    }
                }
            }
        };

        let (category, file_stem) = note_path
            .split_once('/')
            .map(|(c, f)| (c.to_string(), f.to_string()))
            .ok_or_else(|| AlephError::tool(format!("malformed note path '{note_path}'")))?;

        let file_path = crate::memory::notes::store::note_content_path(
            self.indexer.memory_dir(),
            agent_id,
            &category,
            &file_stem,
        );
        let raw = match tokio::fs::read_to_string(&file_path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(AlephError::tool(format!(
                    "Note '{note_path}' does not exist."
                )))
            }
            Err(e) => return Err(AlephError::tool(format!("Failed to read note: {e}"))),
        };

        // Recall bookkeeping, same channel as `query`: an addressed read is the
        // strongest form of explicit look-up there is, and a note the model
        // keeps opening must not age as never-used. The query text is the path,
        // so `get` and `query` occupy different dedup keys on the same channel.
        // Best-effort — a signal write failure never fails the read.
        if let Err(e) = self
            .indexer
            .store()
            .record_recall_hits(
                &note_path,
                NOTE_MANAGE_RECALL_CHANNEL,
                &[(note_path.clone(), 1.0)],
                agent_id,
            )
            .await
        {
            tracing::debug!(error = %e, "note_manage get: recall signal write failed");
        }

        let tags = self
            .indexer
            .store()
            .get_note_index(&note_path, agent_id)
            .await
            .ok()
            .flatten()
            .map(|e| e.tags)
            .unwrap_or_default();

        let total_chars = raw.chars().count();
        let content = bound_chars(&raw, GET_MAX_CHARS);
        let message = if total_chars > GET_MAX_CHARS {
            // Naming the overflow is the difference between "this is the note"
            // and "this is the front of the note": a wholesale `update` written
            // from a truncated read is exactly the loss this action exists to
            // stop, so the ceiling has to be stated where the caller cannot
            // miss it, not only inside the body's own marker.
            format!(
                "Read note '{note_path}' — TRUNCATED at {GET_MAX_CHARS} of {total_chars} chars. \
                 Do NOT `update` this note from this response; the tail is not shown."
            )
        } else {
            format!("Read note '{note_path}' ({total_chars} chars)")
        };

        Ok(NoteManageResult {
            related_notes: None,
            success: true,
            message,
            // Deliberately no `destination`: that field is a *write* receipt
            // (see `NoteManageResult::destination`). Stamping one on a read is
            // how a model ends up telling the user something was filed away
            // when nothing was written.
            destination: None,
            note_path: Some(note_path.clone()),
            content: Some(content),
            notes: Some(vec![NoteListEntry {
                path: note_path,
                category,
                filename: file_stem,
                tags,
            }]),
            search: None,
        })
    }

    pub(super) async fn handle_list(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id_owned = self.resolve_agent_id(args)?;
        let agent_id = agent_id_owned.as_str();
        let limit = args.limit.unwrap_or(100);

        // Category filter dispatches to the paginated store query instead of
        // scanning every note for the agent and filtering in memory. The filter
        // is canonicalized like every write path, so `projects` lists the notes
        // that a `projects` create actually filed under `project`.
        let category_filter = args.category.as_deref().map(canonicalize_category);
        let all_entries = match category_filter.as_deref() {
            Some(cat) => self
                .indexer
                .store()
                .get_notes_by_category(agent_id, cat, limit)
                .await
                .map_err(|e| AlephError::tool(format!("Failed to list notes: {e}")))?,
            None => self
                .indexer
                .store()
                .list_notes(agent_id)
                .await
                .map_err(|e| AlephError::tool(format!("Failed to list notes: {e}")))?,
        };

        let entries: Vec<NoteListEntry> = all_entries
            .into_iter()
            .take(limit)
            .map(|e| NoteListEntry {
                path: e.path.clone(),
                category: e.category.clone(),
                filename: e.filename.clone(),
                tags: e.tags,
            })
            .collect();

        let category_label = args
            .category
            .as_deref()
            .map(|c| format!(" in '{c}'"))
            .unwrap_or_default();

        Ok(NoteManageResult {
            related_notes: None,
            success: true,
            message: format!("{} note(s){category_label}", entries.len()),
            destination: None,
            note_path: None,
            content: None,
            notes: Some(entries),
            search: None,
        })
    }
}
