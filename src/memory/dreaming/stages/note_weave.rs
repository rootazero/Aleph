//! NoteWeave stage — weave orphan notes into the wiki link graph.
//!
//! An orphan (zero outgoing AND zero incoming links) is invisible to graph
//! expansion in `gather_related`, earns no `link_weight` in `NoteDecay`
//! scoring, and is therefore archived early — a vicious cycle this stage
//! breaks. Detection is store-query-only (cheap); per orphan, one LLM call
//! picks 0-3 targets from hybrid-search candidates (R7 — the model decides
//! who to link, the harness only validates `[C<n>]` tokens against the
//! candidate set, mirroring the ingest `RefTable` anti-hallucination line).
//! Links are written through `NoteIndexer::append_to_note` in both
//! directions, the same primitive `CompoundApplyTx::add_link` uses.

use async_trait::async_trait;
use tracing::{info, warn};

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::store::NoteStore;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::utils::json_extract::extract_json_robust;

use super::DreamStage;

/// Max orphan notes processed (one LLM call each) per dream cycle.
const MAX_WEAVE_PER_CYCLE: usize = 10;
/// Max link candidates shown to the LLM per orphan.
const MAX_CANDIDATES: usize = 8;
/// Max links accepted per orphan.
const MAX_LINKS_PER_NOTE: usize = 3;

const PROMPT_WEAVE: &str = r#"You maintain an Aleph personal-memory wiki.
The note below is an ORPHAN — no other note links to it and it links to
none. From the candidate pages, pick 0-3 that genuinely relate to it.

Rules:
- Only use `[C<n>]` tokens that appear in the "Candidates" section.
- Pick a candidate only when a reader of one note would benefit from the
  other; do NOT force a link.
- When nothing truly relates, return an empty list.
- Output valid JSON only, no prose: {"links": ["[C1]", "[C4]"]}
"#;

/// NoteWeave stage. `max_per_cycle` caps LLM spend per dream cycle.
pub struct NoteWeaveStage {
    pub max_per_cycle: usize,
}

impl Default for NoteWeaveStage {
    fn default() -> Self {
        Self {
            max_per_cycle: MAX_WEAVE_PER_CYCLE,
        }
    }
}

#[async_trait]
impl DreamStage for NoteWeaveStage {
    fn name(&self) -> &'static str {
        "note_weave"
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        // --- Phase 1: orphan detection (store queries only, no LLM) ---
        let mut orphans: Vec<String> = Vec::new();
        for note in &ctx.notes {
            if orphans.len() >= self.max_per_cycle {
                break;
            }
            let Some((_, filename)) = note.path.split_once('/') else {
                continue;
            };
            let outgoing = ctx
                .indexer
                .store()
                .get_outgoing_links(&note.path, &ctx.agent_id)
                .await
                .unwrap_or_default();
            if !outgoing.is_empty() {
                continue;
            }
            // notes_links stores raw wikilink targets by filename — query by
            // filename, mirroring NoteDecay's incoming-link count.
            let incoming = ctx
                .indexer
                .store()
                .get_incoming_links(filename, &ctx.agent_id)
                .await
                .unwrap_or_default();
            if !incoming.is_empty() {
                continue;
            }
            orphans.push(note.path.clone());
        }
        if orphans.is_empty() {
            info!("NoteWeave: no orphan notes");
            return Ok(ctx);
        }
        info!(count = orphans.len(), "NoteWeave: found orphan notes");

        // --- Phase 2: weave each orphan (one LLM call each; a per-note
        // failure skips that note only). ---
        let mut woven = 0u32;
        for path in orphans {
            match weave_one(&mut ctx, &path).await {
                Ok(n) => woven += n,
                Err(e) => warn!(path = %path, error = %e, "NoteWeave: skipped orphan"),
            }
        }
        ctx.report.notes_woven = woven;
        info!(woven, "NoteWeave completed");
        Ok(ctx)
    }
}

/// Weave a single orphan: gather candidates via hybrid search over the
/// orphan's own content, ask the LLM to pick targets, write bidirectional
/// links. Returns the number of links written.
async fn weave_one(ctx: &mut DreamContext, path: &str) -> Result<u32, AlephError> {
    let Some(content) = ctx.load_content(path).await else {
        return Ok(0);
    };
    let embedding = ctx.embedder.embed(&content).await?;
    let dim = embedding.len() as u32;
    let hits = ctx
        .indexer
        .store()
        .hybrid_search_notes(&embedding, &content, &ctx.agent_id, dim, MAX_CANDIDATES + 1)
        .await?;
    let candidates: Vec<_> = hits
        .into_iter()
        .filter(|h| h.path != path)
        .take(MAX_CANDIDATES)
        .collect();
    if candidates.is_empty() {
        return Ok(0); // genuinely alone — nothing to weave
    }

    let mut user = format!("## Orphan note: {path}\n\n");
    for line in content.lines().take(40) {
        user.push_str(line);
        user.push('\n');
    }
    user.push_str("\n## Candidates\n\n");
    for (i, c) in candidates.iter().enumerate() {
        let preview: String = c.content.chars().take(200).collect();
        user.push_str(&format!(
            "[C{i}] {} — {}\n",
            c.path,
            preview.replace('\n', " ")
        ));
    }

    let msgs = [UnifiedMessage::user(&user)];
    let resp = ctx
        .provider
        .process(RequestPayload::new(&msgs).with_system(Some(PROMPT_WEAVE)))
        .await
        .map_err(|e| AlephError::other(format!("weave LLM: {e}")))?;
    let Some(json) = extract_json_robust(&resp.text_content()) else {
        return Ok(0);
    };
    let targets = parse_weave_targets(&json, candidates.len());

    let mut written = 0u32;
    for idx in targets.into_iter().take(MAX_LINKS_PER_NOTE) {
        let target = candidates[idx].path.clone();
        // Bidirectional, mirroring CompoundApplyTx::add_link.
        ctx.indexer
            .append_to_note(
                &ctx.agent_id,
                path,
                &Vec::<String>::new(),
                std::slice::from_ref(&target),
            )
            .await?;
        ctx.indexer
            .append_to_note(
                &ctx.agent_id,
                &target,
                &Vec::<String>::new(),
                &[path.to_string()],
            )
            .await?;
        // Evict cached bodies so downstream stages re-read updated markdown.
        ctx.note_contents.remove(path);
        ctx.note_contents.remove(target.as_str());
        written += 1;
    }
    Ok(written)
}

/// Parse `{"links": ["[C1]", ...]}` into in-range candidate indices.
/// Out-of-range, malformed, or duplicate tokens are dropped
/// (anti-hallucination, mirroring the ingest `RefTable` line).
fn parse_weave_targets(json: &serde_json::Value, n_candidates: usize) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    let Some(arr) = json.get("links").and_then(|v| v.as_array()) else {
        return out;
    };
    for v in arr {
        let Some(s) = v.as_str() else { continue };
        let Some(inner) = s
            .trim()
            .strip_prefix("[C")
            .and_then(|r| r.strip_suffix(']'))
        else {
            continue;
        };
        if inner.is_empty() || !inner.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(idx) = inner.parse::<usize>() else {
            continue;
        };
        if idx < n_candidates && !out.contains(&idx) {
            out.push(idx);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_name_is_note_weave() {
        assert_eq!(NoteWeaveStage::default().name(), "note_weave");
    }

    #[test]
    fn default_cap_is_ten() {
        assert_eq!(NoteWeaveStage::default().max_per_cycle, 10);
    }

    #[test]
    fn parse_targets_accepts_in_range_tokens() {
        let j: serde_json::Value = serde_json::json!({"links": ["[C0]", "[C2]"]});
        assert_eq!(parse_weave_targets(&j, 3), vec![0, 2]);
    }

    #[test]
    fn parse_targets_drops_out_of_range_and_malformed() {
        let j: serde_json::Value =
            serde_json::json!({"links": ["[C9]", "[P1]", "C1", "[C]", 42, "[C1]"]});
        assert_eq!(parse_weave_targets(&j, 3), vec![1]);
    }

    #[test]
    fn parse_targets_dedups() {
        let j: serde_json::Value = serde_json::json!({"links": ["[C1]", "[C1]"]});
        assert_eq!(parse_weave_targets(&j, 3), vec![1]);
    }

    #[test]
    fn parse_targets_empty_or_missing() {
        assert!(parse_weave_targets(&serde_json::json!({"links": []}), 3).is_empty());
        assert!(parse_weave_targets(&serde_json::json!({}), 3).is_empty());
    }

    // -----------------------------------------------------------------
    // Stage-level tests: orphan detection against a real SQLite store.
    // Fixture mirrors note_lint.rs::build_test_dream_ctx.
    // -----------------------------------------------------------------

    use crate::memory::dreaming::{DreamContext, NoteEntry};
    use crate::memory::embedding_provider::EmbeddingProvider;
    use crate::memory::notes::{KnowledgeNote, NoteIndexer};
    use crate::memory::store::SqliteMemoryBackend;
    use crate::providers::mock::MockProvider;
    use crate::sync_primitives::Arc;

    struct StubEmbedder;

    #[async_trait::async_trait]
    impl EmbeddingProvider for StubEmbedder {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
            Ok(Vec::new())
        }
        async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Ok(Vec::new())
        }
        fn dimensions(&self) -> usize {
            0
        }
        fn model_name(&self) -> &str {
            "stub"
        }
        fn provider_id(&self) -> &str {
            "stub"
        }
    }

    async fn build_ctx(llm_response: &str) -> (DreamContext, Arc<SqliteMemoryBackend>) {
        let temp = std::env::temp_dir().join(format!("aleph_weave_{}", uuid::Uuid::new_v4()));
        let store = Arc::new(SqliteMemoryBackend::new(&temp).unwrap());
        let indexer = NoteIndexer::new(temp.clone(), store.clone());
        let provider: std::sync::Arc<dyn crate::providers::AiProvider> =
            std::sync::Arc::new(MockProvider::new(llm_response));
        let embedder: std::sync::Arc<dyn EmbeddingProvider> = std::sync::Arc::new(StubEmbedder);
        let ctx = DreamContext {
            notes: Vec::new(),
            note_contents: std::collections::HashMap::new(),
            agent_id: "default".into(),
            database: store.clone(),
            indexer,
            provider,
            embedder,
            report: crate::memory::dreaming::DreamReport::default(),
            pipeline_type: "consolidate".into(),
            activity_checker: std::sync::Arc::new(|| false),
            strategy: crate::memory::dreaming::DreamStrategy::Consolidate,
            orientation: None,
        };
        (ctx, store)
    }

    fn entry(path: &str) -> NoteEntry {
        let (category, _) = path.split_once('/').unwrap();
        NoteEntry {
            path: path.into(),
            category: category.into(),
            tags: vec![],
            created_at: 0,
            updated_at: 0,
            content_hash: "h".into(),
        }
    }

    #[tokio::test]
    async fn linked_note_is_not_treated_as_orphan() {
        let (mut ctx, store) = build_ctx("{\"links\": []}").await;
        // a links to b → neither is an orphan.
        store
            .index_note(
                &KnowledgeNote {
                    title: "a".into(),
                    category: "learning".into(),
                    facts: vec!["see [[b]]".into()],
                    links: vec!["learning/b".into()],
                    content_hash: "h1".into(),
                    ..Default::default()
                },
                "default",
                "learning",
            )
            .await
            .unwrap();
        store
            .index_note(
                &KnowledgeNote {
                    title: "b".into(),
                    category: "learning".into(),
                    facts: vec!["fact".into()],
                    content_hash: "h2".into(),
                    ..Default::default()
                },
                "default",
                "learning",
            )
            .await
            .unwrap();
        ctx.notes = vec![entry("learning/a"), entry("learning/b")];

        let out = NoteWeaveStage::default().execute(ctx).await.unwrap();
        // No orphans → no weave writes recorded.
        assert_eq!(out.report.notes_woven, 0);
    }

    #[tokio::test]
    async fn orphan_with_no_disk_file_is_skipped_gracefully() {
        // Orphan detected in the index but its markdown file does not exist →
        // load_content returns None → weave_one returns 0, stage completes.
        let (mut ctx, store) = build_ctx("{\"links\": [\"[C0]\"]}").await;
        store
            .index_note(
                &KnowledgeNote {
                    title: "lonely".into(),
                    category: "learning".into(),
                    facts: vec!["isolated fact".into()],
                    content_hash: "h1".into(),
                    ..Default::default()
                },
                "default",
                "learning",
            )
            .await
            .unwrap();
        ctx.notes = vec![entry("learning/lonely")];

        let out = NoteWeaveStage::default().execute(ctx).await.unwrap();
        assert_eq!(out.report.notes_woven, 0);
    }
}
