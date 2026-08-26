//! Write surface: `create`, `update`, `append`.
//!
//! Every path here lands bytes in a note file, so every path here scans the
//! content for injection payloads first and returns a `destination` receipt
//! after — the two invariants that separate a write from a read.

use tracing::{info, warn};

use crate::error::{AlephError, Result};
use crate::memory::notes::store::NoteStore;
use crate::memory::notes::{sanitize_title, KnowledgeNote};

use super::args::{NoteListEntry, NoteManageArgs, NoteManageResult};
use super::helpers::{merge_relations, related_keywords, scan_note_for_threats};
use super::NoteManageTool;

impl NoteManageTool {
    pub(super) async fn handle_create(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id_owned = self.resolve_agent_id(args)?;
        let agent_id = agent_id_owned.as_str();

        let category_owned = Self::resolve_category(args, "create")?;
        let category = category_owned.as_str();
        let filename = args
            .filename
            .as_deref()
            .ok_or_else(|| AlephError::tool("filename is required for create"))?;
        let _title = args
            .title
            .as_deref()
            .ok_or_else(|| AlephError::tool("title is required for create"))?;

        // Hard security floor (§5.1): reject injection / exfiltration /
        // persistence payloads before they land in trusted long-term memory.
        if let Some(content) = &args.content {
            // Defence in depth (audit-2026-08-26 BTS-7): mirror the
            // read-side PER_NOTE_MAX_CHARS at the write site so a steered
            // model cannot persist a multi-MiB body that the embedding
            // step + atomic-write + reparse-index pipeline would then
            // pay for on every subsequent read. The earlier review
            // recommended this cap; landing it now closes the gap.
            const MAX_NOTE_BODY_CHARS: usize = 1_048_576; // 1 MiB chars
            if content.chars().count() > MAX_NOTE_BODY_CHARS {
                return Err(AlephError::tool(format!(
                    "note content is {} chars; the cap is {MAX_NOTE_BODY_CHARS}. \
                     Chunk long bodies via 'append' or split into multiple notes.",
                    content.chars().count()
                )));
            }
            scan_note_for_threats(content)?;
        }

        let safe_filename = sanitize_title(filename)?;
        let file_path = self
            .indexer
            .memory_dir()
            .join(agent_id)
            .join(category)
            .join(format!("{safe_filename}.md"));
        if file_path.exists() {
            return Err(AlephError::tool(format!(
                "Note '{filename}' in '{category}' already exists. Use 'update' action instead."
            )));
        }

        let tags = args.tags.clone().unwrap_or_default();
        let now = chrono::Utc::now().timestamp();
        let mut note = KnowledgeNote {
            title: safe_filename.clone(),
            category: category.to_string(),
            tags: tags.clone(),
            facts: vec![],
            links: vec![],
            created_at: now,
            updated_at: now,
            content_hash: String::new(),
            ..Default::default()
        };

        // Store the caller's markdown verbatim as the body (headings, code
        // blocks, and paragraphs survive) — facts/links become derived index
        // views. Explicit `links` args are merged via the body-sync helper.
        if let Some(content) = &args.content {
            note.set_body(content.clone());
        }
        if let Some(links) = &args.links {
            note.add_links(links);
        }
        if let Some(rels) = &args.relations {
            merge_relations(&mut note, rels);
        }

        // Single write chokepoint: atomic write + reparse-index.
        // (The pre-write existence check above leaves a narrow check-to-write
        // window; note writes are single-process, so this is acceptable.)
        self.indexer
            .write_note(agent_id, category, &note)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to write note: {e}")))?;

        let note_path = format!("{category}/{safe_filename}");
        info!(path = %note_path, "Note created");

        // Surface related existing notes (best-effort) so the model can weave
        // the new note into the wiki instead of leaving an orphan island.
        // Preferred: semantic neighbors via the embedder — this also works for
        // CJK content, which the unicode61 FTS tokenizer cannot match
        // per-word. Fallback: per-keyword FTS. Search failure must never fail
        // the create.
        let query_text = format!(
            "{} {}",
            args.title.as_deref().unwrap_or(&safe_filename),
            args.content.as_deref().unwrap_or("")
        );
        let mut rel: Vec<NoteListEntry> = Vec::new();
        if let Some(embedder) = &self.embedder {
            if let Ok(embedding) = embedder.embed(&query_text).await {
                let dim = embedding.len() as u32;
                if let Ok(hits) = self
                    .indexer
                    .store()
                    .vector_search(&embedding, dim, agent_id, 6)
                    .await
                {
                    for (path, _score) in hits {
                        if path == note_path || rel.iter().any(|r| r.path == path) {
                            continue;
                        }
                        if let Ok(Some(e)) =
                            self.indexer.store().get_note_index(&path, agent_id).await
                        {
                            rel.push(NoteListEntry {
                                path: e.path,
                                category: e.category,
                                filename: e.filename,
                                tags: e.tags,
                            });
                        }
                    }
                }
            }
        }
        if rel.is_empty() {
            for kw in related_keywords(&query_text) {
                match self
                    .indexer
                    .store()
                    .search_notes_fts(&kw, agent_id, 3)
                    .await
                {
                    Ok(hits) => {
                        for e in hits {
                            if e.path == note_path || rel.iter().any(|r| r.path == e.path) {
                                continue;
                            }
                            rel.push(NoteListEntry {
                                path: e.path,
                                category: e.category,
                                filename: e.filename,
                                tags: e.tags,
                            });
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, keyword = %kw, "note_manage create: related-note search failed");
                    }
                }
                if rel.len() >= 5 {
                    break;
                }
            }
        }
        rel.truncate(5);
        let related_notes = (!rel.is_empty()).then_some(rel);
        let message = match &related_notes {
            Some(rel) => format!(
                "Created note '{safe_filename}' in '{category}'. Found {} related note(s) — consider linking them (append with links=[...]) so this note is not an orphan: {}",
                rel.len(),
                rel.iter()
                    .map(|r| r.path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            None => format!("Created note '{safe_filename}' in '{category}'"),
        };

        Ok(NoteManageResult {
            related_notes,
            success: true,
            message,
            destination: Some(self.destination(agent_id, &note_path)),
            note_path: Some(note_path),
            content: None,
            notes: None,
            search: None,
        })
    }

    pub(super) async fn handle_update(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id_owned = self.resolve_agent_id(args)?;
        let agent_id = agent_id_owned.as_str();

        let category_owned = Self::resolve_category(args, "update")?;
        let category = category_owned.as_str();
        let filename = args
            .filename
            .as_deref()
            .ok_or_else(|| AlephError::tool("filename is required for update"))?;
        let content = args
            .content
            .as_deref()
            .ok_or_else(|| AlephError::tool("content is required for update"))?;

        // Hard security floor (§5.1): see `scan_note_for_threats`.
        scan_note_for_threats(content)?;

        let safe_filename = sanitize_title(filename)?;
        let file_path = self
            .indexer
            .memory_dir()
            .join(agent_id)
            .join(category)
            .join(format!("{safe_filename}.md"));

        if !file_path.exists() {
            return Err(AlephError::tool(format!(
                "Note '{filename}' in '{category}' does not exist. Use 'create' action first."
            )));
        }

        // Read existing note, preserve frontmatter metadata, replace the body
        // verbatim (facts/links re-derived by set_body).
        let existing = tokio::fs::read_to_string(&file_path)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to read note: {e}")))?;

        let mut note = KnowledgeNote::from_markdown(&safe_filename, &existing)
            .map_err(|e| AlephError::tool(format!("Failed to parse existing note: {e}")))?;

        note.set_body(content.to_string());

        // Apply optional field updates
        if let Some(tags) = &args.tags {
            note.tags = tags.clone();
        }
        if let Some(links) = &args.links {
            note.add_links(links);
        }
        if let Some(rels) = &args.relations {
            merge_relations(&mut note, rels);
        }
        note.updated_at = chrono::Utc::now().timestamp();

        // Single write chokepoint: atomic write + reparse-index
        // (the previous plain fs::write could leave a truncated source-of-truth
        // file on a crash).
        self.indexer
            .write_note(agent_id, category, &note)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to write note: {e}")))?;

        let note_path = format!("{category}/{safe_filename}");
        info!(path = %note_path, "Note updated");

        Ok(NoteManageResult {
            related_notes: None,
            success: true,
            message: format!("Updated note '{safe_filename}' in '{category}'"),
            destination: Some(self.destination(agent_id, &note_path)),
            note_path: Some(note_path),
            content: None,
            notes: None,
            search: None,
        })
    }

    pub(super) async fn handle_append(&self, args: &NoteManageArgs) -> Result<NoteManageResult> {
        let agent_id_owned = self.resolve_agent_id(args)?;
        let agent_id = agent_id_owned.as_str();

        let category_owned = Self::resolve_category(args, "append")?;
        let category = category_owned.as_str();
        let filename = args
            .filename
            .as_deref()
            .ok_or_else(|| AlephError::tool("filename is required for append"))?;

        let safe_filename = sanitize_title(filename)?;
        let note_path = format!("{category}/{safe_filename}");

        let new_facts = args.facts.clone().unwrap_or_default();
        let new_links = args.links.clone().unwrap_or_default();

        let has_relations = args.relations.as_ref().is_some_and(|r| !r.is_empty());
        if new_facts.is_empty() && new_links.is_empty() && !has_relations {
            return Err(AlephError::tool(
                "At least one fact, link, or relation is required for append",
            ));
        }

        // Hard security floor (§5.1): scan the appended free-text facts before
        // they are persisted. Links are wikilink references (note titles), not
        // free-form content, so only the facts carry an injection surface.
        if !new_facts.is_empty() {
            scan_note_for_threats(&new_facts.join("\n"))?;
        }

        self.indexer
            .append_to_note(agent_id, &note_path, &new_facts, &new_links)
            .await
            .map_err(|e| AlephError::tool(format!("Failed to append to note: {e}")))?;

        if let Some(rels) = &args.relations {
            let parsed: Vec<crate::memory::notes::Relation> = rels
                .iter()
                .map(|r| crate::memory::notes::Relation {
                    to: r.to.clone(),
                    rel_type: r.rel_type.clone(),
                    confidence: 1.0,
                })
                .collect();
            self.indexer
                .append_relations(agent_id, &note_path, &parsed)
                .await
                .map_err(|e| AlephError::tool(format!("Failed to append relations: {e}")))?;
        }

        info!(path = %note_path, facts = new_facts.len(), "Note appended");

        Ok(NoteManageResult {
            related_notes: None,
            success: true,
            message: format!(
                "Appended {} fact(s) to '{safe_filename}' in '{category}'",
                new_facts.len()
            ),
            destination: Some(self.destination(agent_id, &note_path)),
            note_path: Some(note_path),
            content: None,
            notes: None,
            search: None,
        })
    }
}
