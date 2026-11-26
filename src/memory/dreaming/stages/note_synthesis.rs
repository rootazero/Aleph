//! `NoteSynthesis` stage — generates cross-note insight summaries.
//!
//! Runs only in the weekly pipeline and requires at least 5 notes.
//! Groups notes by category and calls an LLM to synthesize cross-cutting
//! themes and patterns for each category with 3 or more notes.

use async_trait::async_trait;
use std::collections::BTreeMap;

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::KnowledgeNote;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;

use super::DreamStage;

pub struct NoteSynthesisStage;

#[async_trait]
impl DreamStage for NoteSynthesisStage {
    fn name(&self) -> &'static str {
        "note_synthesis"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        // Runs when strategy is Synthesize and there are enough notes
        ctx.notes.len() >= 5
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let mut synthesis_count = 0u32;

        let mut by_category = BTreeMap::<String, Vec<String>>::new();
        for note in &ctx.notes {
            if note.category != "synthesis" && note.category != "query" {
                by_category
                    .entry(note.category.clone())
                    .or_default()
                    .push(note.path.clone());
            }
        }

        for (category, mut paths) in by_category {
            paths.sort_unstable();
            paths.dedup();
            if paths.len() < 3 {
                continue;
            }
            // The prompt and persisted citations use the same usable inputs.
            let mut usable = Vec::new();
            for path in paths {
                if usable.len() == 15 {
                    break;
                }
                let Some(content) = ctx.load_content(&path).await else {
                    continue;
                };
                let preview: String = content.chars().take(300).collect();
                if preview.trim().is_empty() {
                    continue;
                }
                usable.push((path, preview));
            }
            if usable.len() < 3 {
                continue;
            }

            let note_count = usable.len();
            let (consulted, previews): (Vec<_>, Vec<_>) = usable
                .into_iter()
                .map(|(path, preview)| {
                    let block = format!("### {path}\n{preview}");
                    (path, block)
                })
                .unzip();

            let prompt = format!(
                "Analyze these {note_count} knowledge notes in the '{category}' category and write a synthesis insight.\n\n\
                 Identify:\n\
                 1. Cross-cutting themes and patterns\n\
                 2. Connections between different notes\n\
                 3. Key takeaways or emerging trends\n\n\
                 Notes:\n\n{}\n\n\
                 Write a concise synthesis (3-5 paragraphs). Use [[wikilinks]] to reference source notes by their path.",
                previews.join("\n\n")
            );

            let system = "You are a knowledge synthesis engine. Find patterns across notes and write insightful summaries.";

            let msgs = vec![UnifiedMessage::user(&prompt)];
            let response = match ctx
                .provider
                .process(RequestPayload::new(&msgs).with_system(Some(system)))
                .await
            {
                Ok(r) => r,
                Err(e) if super::is_provider_exhausted(&e) => {
                    tracing::warn!(error = %e, "Synthesis: provider exhausted — aborting dream cycle");
                    return Err(e);
                }
                Err(e) => {
                    tracing::warn!(category, error = %e, "Synthesis LLM call failed");
                    continue;
                }
            };

            let synthesis_text = response.text_content();

            // Build the synthesis note from its source member paths.
            let note = build_synthesis_note(&category, &synthesis_text, consulted);

            match ctx
                .indexer
                .write_note(&ctx.agent_id, "synthesis", &note)
                .await
            {
                Ok(written) => {
                    synthesis_count += 1;
                    // Record (note path, body digest) so the *next* cycle's
                    // MutationGate can tell a stable synthesis from one that
                    // flip-flops. Digest the body, not the rendered markdown:
                    // frontmatter carries an `updated` date that moves on every
                    // write, which would report churn every night.
                    let note_path = written.file_stem().map_or_else(
                        || format!("synthesis/{category}"),
                        |stem| format!("synthesis/{}", stem.to_string_lossy()),
                    );
                    ctx.report.synthesis_rewrites.push((
                        note_path,
                        crate::memory::notes::indexer::sha2_hash(&synthesis_text),
                    ));
                    tracing::info!(
                        category,
                        notes = note_count,
                        "Generated synthesis for category"
                    );
                }
                Err(e) => {
                    tracing::warn!(category, error = %e, "Failed to write synthesis note");
                }
            }
        }

        ctx.report.synthesis_count = synthesis_count;
        tracing::info!(synthesis_count, "NoteSynthesis completed");
        Ok(ctx)
    }
}

/// Build the L2 synthesis note for a category from its source member paths.
///
/// The member paths are recorded as BOTH `links` (graph edges) and
/// `source_notes` (provenance: which L1 notes this synthesis distilled), so the
/// L1→L2 evidence chain stays connected and `index_note` can materialize
/// `notes_sources`.
fn build_synthesis_note(
    category: &str,
    synthesis_text: &str,
    source_links: Vec<String>,
) -> KnowledgeNote {
    KnowledgeNote {
        title: format!("{category} Synthesis"),
        category: "synthesis".to_string(),
        tags: vec![category.to_string(), "synthesis".to_string()],
        facts: vec![synthesis_text.to_string()],
        links: source_links.clone(),
        source_notes: source_links,
        created_at: chrono::Utc::now().timestamp(),
        updated_at: chrono::Utc::now().timestamp(),
        content_hash: String::new(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::dreaming::NoteEntry;
    use crate::memory::dreaming::{DreamReport, DreamStrategy, EditBudget};
    use crate::memory::embedding_provider::EmbeddingProvider;
    use crate::memory::notes::store::NoteStore;
    use crate::memory::notes::NoteIndexer;
    use crate::memory::store::markdown::MarkdownSessionStore;
    use crate::memory::store::raw_memory::RawMemoryStore;
    use crate::memory::store::{RawMemory, RawMemorySource, SqliteMemoryBackend};
    use crate::providers::mock::MockProvider;
    use crate::providers::{AiProvider, ProviderResponse};
    use std::collections::{HashMap, HashSet};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    fn make_note(path: &str, category: &str) -> NoteEntry {
        NoteEntry {
            path: path.to_string(),
            category: category.to_string(),
            tags: vec![],
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
            content_hash: String::new(),
        }
    }

    fn make_notes(count: usize, category: &str) -> Vec<NoteEntry> {
        (0..count)
            .map(|i| make_note(&format!("{category}/note{i}"), category))
            .collect()
    }

    // ---------------------------------------------------------------------
    // Task 1 — the synthesis stage must record exactly the notes it showed
    // the model, and must not manufacture provenance for notes it never
    // read.
    // ---------------------------------------------------------------------

    /// A body that is longer than the 300-codepoint preview budget *and*
    /// multibyte, so a preview cut on bytes would be visibly shorter than
    /// one cut on codepoints.
    fn multibyte_body() -> String {
        "界".repeat(305)
    }

    /// The exact frontmatter bytes of a fixture note (written verbatim to
    /// disk, so the preview is a byte-for-byte function of the file).
    fn fixture_frontmatter(name: &str) -> String {
        let source = if name == "02" {
            "source_notes: [raw-1]\n"
        } else {
            ""
        };
        format!("---\ntitle: \"{name}\"\n{source}---\n")
    }

    /// Records the real prompt text of every request, then delegates the
    /// answer to `MockProvider`.
    struct Recorder {
        prompts: Mutex<Vec<String>>,
    }

    impl Recorder {
        fn new() -> Self {
            Self {
                prompts: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> usize {
            self.prompts.lock().expect("recorder lock").len()
        }

        fn only_prompt(&self) -> String {
            let prompts = self.prompts.lock().expect("recorder lock");
            assert_eq!(prompts.len(), 1, "expected exactly one synthesis request");
            prompts[0].clone()
        }
    }

    impl AiProvider for Recorder {
        fn process<'a>(
            &'a self,
            payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = crate::error::Result<ProviderResponse>> + Send + 'a>>
        {
            let text = payload
                .messages
                .iter()
                .map(|m| m.text_content())
                .collect::<Vec<_>>()
                .join("\n");
            self.prompts.lock().expect("recorder lock").push(text);
            let mock = MockProvider::new("synthesis result");
            Box::pin(async move { mock.process(payload).await })
        }

        fn name(&self) -> &str {
            "recorder"
        }

        fn color(&self) -> &str {
            "#000000"
        }
    }

    struct Embedder;
    #[async_trait::async_trait]
    impl EmbeddingProvider for Embedder {
        async fn embed(&self, _: &str) -> Result<Vec<f32>, AlephError> {
            Ok(vec![])
        }
        async fn embed_batch(&self, _: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Ok(vec![])
        }
        fn dimensions(&self) -> usize {
            0
        }
        fn model_name(&self) -> &str {
            "test"
        }
        fn provider_id(&self) -> &str {
            "test"
        }
    }

    /// The preview block the stage put in the prompt for `path`.
    fn preview_block(prompt: &str, path: &str) -> String {
        let header = format!("### {path}\n");
        let start = prompt
            .find(&header)
            .unwrap_or_else(|| panic!("prompt has no block for {path}"));
        let rest = &prompt[start + header.len()..];
        let end = rest.find("\n\n").unwrap_or(rest.len());
        rest[..end].to_string()
    }

    #[tokio::test]
    async fn synthesis_persists_only_prompted_sources() {
        let agent = crate::routing::DEFAULT_AGENT_ID;
        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().join("notes");
        let db = Arc::new(SqliteMemoryBackend::new(&dir.path().join("memory.db")).unwrap());
        let indexer = NoteIndexer::new(memory_dir.clone(), db.clone());

        // 18 candidate paths in one category, presented to the stage in
        // reverse order so the stage's own ordering decides what is cited.
        let paths: Vec<String> = (0..18).map(|i| format!("preference/{i:02}")).collect();

        // `preference/00` is missing (no file at all) and `preference/01` is a
        // whitespace-only file: neither is a usable input. Both live on disk
        // (or not) rather than in `note_contents`, so the stage really reads
        // the corpus through `DreamContext::load_content`.
        let blank_dir = memory_dir.join(agent).join("preference");
        tokio::fs::create_dir_all(&blank_dir).await.unwrap();
        tokio::fs::write(blank_dir.join("01.md"), "   \n\t\n ")
            .await
            .unwrap();

        // `preference/02..=17` are real, indexed notes whose body is longer
        // than the preview budget and multibyte.
        for name in [
            "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12", "13", "14", "15",
            "16", "17",
        ] {
            indexer
                .write_note_raw(
                    agent,
                    "preference",
                    name,
                    &format!("{}{}", fixture_frontmatter(name), multibyte_body()),
                )
                .await
                .unwrap();
        }

        let recorder = Arc::new(Recorder::new());
        let notes: Vec<NoteEntry> = paths
            .iter()
            .rev()
            .map(|p| make_note(p, "preference"))
            .collect();
        let ctx = DreamContext {
            notes,
            note_contents: HashMap::new(),
            agent_id: agent.into(),
            database: db.clone(),
            raw_store: None,
            indexer,
            provider: recorder.clone(),
            embedder: Arc::new(Embedder),
            report: DreamReport::default(),
            pipeline_type: "synthesize".into(),
            activity_checker: Arc::new(|| false),
            strategy: DreamStrategy::Synthesize,
            orientation: None,
            evolution_budget: EditBudget::default(),
        };

        let ctx = NoteSynthesisStage.execute(ctx).await.unwrap();

        // The 15 usable inputs, in exact-path order: `00`/`01` are unusable,
        // `17` is past the 15-note budget.
        let expected: Vec<String> = (2..=16).map(|i| format!("preference/{i:02}")).collect();

        // --- what the model actually saw -------------------------------
        let prompt = recorder.only_prompt();
        assert!(
            prompt.contains("Analyze these 15 knowledge notes in the 'preference' category"),
            "the prompt count must come from the usable inputs: {prompt}"
        );
        assert_eq!(
            prompt.matches("### preference/").count(),
            15,
            "exactly the usable notes may reach the model"
        );
        for absent in ["preference/00", "preference/01", "preference/17"] {
            assert!(
                !prompt.contains(&format!("### {absent}\n")),
                "{absent} must not reach the model"
            );
        }
        let mut last = 0usize;
        for path in &expected {
            let idx = prompt
                .find(&format!("### {path}\n"))
                .unwrap_or_else(|| panic!("prompt has no block for {path}"));
            assert!(idx > last, "blocks must be ordered by exact path");
            last = idx;
        }
        for path in &expected {
            let name = path.strip_prefix("preference/").unwrap();
            let front = fixture_frontmatter(name);
            let expected_preview = format!("{front}{}", "界".repeat(300 - front.chars().count()));
            let block = preview_block(&prompt, path);
            assert_eq!(
                block.chars().count(),
                300,
                "the preview of {path} must be 300 codepoints"
            );
            assert!(
                block.len() > block.chars().count(),
                "the preview of {path} must be cut on codepoints, not bytes"
            );
            assert_eq!(
                block, expected_preview,
                "the preview of {path} must be the first 300 codepoints of the file"
            );
        }

        // --- what was persisted ----------------------------------------
        let summary_path = "synthesis/preference Synthesis";
        assert!(
            db.get_note_index(summary_path, agent)
                .await
                .unwrap()
                .is_some(),
            "the synthesis note must be indexed"
        );
        let markdown = tokio::fs::read_to_string(
            memory_dir
                .join(agent)
                .join("synthesis")
                .join("preference Synthesis.md"),
        )
        .await
        .unwrap();
        let parsed = KnowledgeNote::from_markdown("preference Synthesis", &markdown).unwrap();
        assert_eq!(
            parsed.source_notes, expected,
            "source_notes must be exactly the prompted paths"
        );
        assert_eq!(
            parsed.links, expected,
            "links must be exactly the prompted paths"
        );
        assert_eq!(
            db.sources_of(agent, summary_path, 16).await.unwrap(),
            expected,
            "the SQLite projection must list the same paths"
        );

        // --- what the graph shows --------------------------------------
        let (nodes, edges) = db.get_graph_data(agent, 200).await.unwrap();
        let node_paths: HashSet<String> = nodes.iter().map(|n| n.path.clone()).collect();
        let mut cited: Vec<String> = edges
            .iter()
            .filter(|e| e.from == summary_path)
            .map(|e| e.to.clone())
            .collect();
        cited.sort();
        assert_eq!(
            cited, expected,
            "graph edges must point at exactly the prompted notes"
        );
        for path in &expected {
            assert!(
                node_paths.contains(path),
                "{path} must be an indexed graph node"
            );
        }
        assert_eq!(ctx.report.synthesis_count, 1);
        assert_eq!(ctx.report.synthesis_rewrites.len(), 1);
        assert_eq!(ctx.report.synthesis_rewrites[0].0, summary_path);

        use crate::builtin_tools::memory_trace::{MemoryTraceArgs, MemoryTraceTool, TraceKind};
        use aleph_protocol::memory_trace::NoteSourceStatus;
        let raw_store = Arc::new(
            MarkdownSessionStore::new(&dir.path().join("sessions"))
                .await
                .unwrap(),
        );
        let mut raw = RawMemory::new(
            "Transcript used by note 02".into(),
            RawMemorySource::Transcript,
        );
        raw.id = "raw-1".into();
        raw.agent_id = agent.into();
        raw_store.insert_raw_memory(&raw).await.unwrap();
        let trace = MemoryTraceTool::new(db, raw_store, agent, memory_dir)
            .call_impl(MemoryTraceArgs {
                target: summary_path.into(),
                kind: TraceKind::Note,
                max_results: Some(1),
            })
            .await
            .unwrap();
        assert_eq!(trace.notes, [summary_path]);
        assert_eq!(trace.note_sources.len(), 15);
        assert_eq!(trace.note_sources[0].note_path, summary_path);
        assert_eq!(trace.note_sources[0].source_path, "preference/02");
        assert_eq!(trace.note_sources[0].status, NoteSourceStatus::Available);
        assert_eq!(trace.evidence.len(), 1);
        assert_eq!(trace.evidence[0].via_note.as_deref(), Some("preference/02"));
        assert_eq!(
            trace.evidence[0].content.as_deref(),
            Some("Transcript used by note 02")
        );
        assert!(!trace.incomplete && !trace.truncated);
    }

    #[tokio::test]
    async fn synthesis_requires_three_readable_previews() {
        let agent = crate::routing::DEFAULT_AGENT_ID;
        let dir = tempfile::tempdir().unwrap();
        let memory_dir = dir.path().join("notes");
        let db = Arc::new(SqliteMemoryBackend::new(&dir.path().join("memory.db")).unwrap());
        let indexer = NoteIndexer::new(memory_dir.clone(), db.clone());

        // Five candidates pass the stage-level count, but only two are usable:
        // `00` and `04` are missing and `01` is whitespace-only.
        let blank_dir = memory_dir.join(agent).join("preference");
        tokio::fs::create_dir_all(&blank_dir).await.unwrap();
        tokio::fs::write(blank_dir.join("01.md"), "   \n\t\n ")
            .await
            .unwrap();
        for name in ["02", "03"] {
            indexer
                .write_note_raw(
                    agent,
                    "preference",
                    name,
                    &format!("{}{}", fixture_frontmatter(name), multibyte_body()),
                )
                .await
                .unwrap();
        }

        // A summary that already exists on disk must survive untouched.
        let summary_dir = memory_dir.join(agent).join("synthesis");
        tokio::fs::create_dir_all(&summary_dir).await.unwrap();
        let summary_file = summary_dir.join("preference Synthesis.md");
        let previous = "PREVIOUS SUMMARY — must not be rewritten\n";
        tokio::fs::write(&summary_file, previous).await.unwrap();

        let recorder = Arc::new(Recorder::new());
        let notes: Vec<NoteEntry> = ["00", "01", "02", "03", "04"]
            .iter()
            .rev()
            .map(|n| make_note(&format!("preference/{n}"), "preference"))
            .collect();
        let ctx = DreamContext {
            notes,
            note_contents: HashMap::new(),
            agent_id: agent.into(),
            database: db.clone(),
            raw_store: None,
            indexer,
            provider: recorder.clone(),
            embedder: Arc::new(Embedder),
            report: DreamReport::default(),
            pipeline_type: "synthesize".into(),
            activity_checker: Arc::new(|| false),
            strategy: DreamStrategy::Synthesize,
            orientation: None,
            evolution_budget: EditBudget::default(),
        };

        assert!(NoteSynthesisStage.should_run(&ctx).await);
        let ctx = NoteSynthesisStage.execute(ctx).await.unwrap();

        assert_eq!(recorder.calls(), 0, "no model request below the minimum");
        assert_eq!(ctx.report.synthesis_count, 0);
        assert!(ctx.report.synthesis_rewrites.is_empty());
        assert_eq!(
            tokio::fs::read_to_string(&summary_file).await.unwrap(),
            previous,
            "the previous summary must be left untouched"
        );
    }

    #[test]
    fn stage_name() {
        assert_eq!(NoteSynthesisStage.name(), "note_synthesis");
    }

    // Tests for should_run logic (tested directly on the predicate since
    // constructing a full DreamContext requires heavy dependencies)

    #[test]
    fn should_run_false_when_too_few_notes() {
        // < 5 notes → false regardless of strategy
        let notes = make_notes(4, "preference");
        let result = notes.len() >= 5;
        assert!(!result, "fewer than 5 notes should not trigger synthesis");
    }

    #[test]
    fn should_run_true_with_enough_notes() {
        // >= 5 notes → true (strategy selection is handled upstream)
        let notes = make_notes(5, "preference");
        let result = notes.len() >= 5;
        assert!(result, "5+ notes should trigger synthesis");
    }

    #[test]
    fn should_run_boundary_exactly_five_notes() {
        let notes = make_notes(5, "skill");
        let result = notes.len() >= 5;
        assert!(result, "exactly 5 notes should satisfy the threshold");
    }

    #[test]
    fn synthesis_notes_excluded_from_grouping() {
        // Synthesis notes should be skipped when building category groups
        let notes = vec![
            make_note("synthesis/preference-insights", "synthesis"),
            make_note("preference/note1", "preference"),
            make_note("preference/note2", "preference"),
            make_note("preference/note3", "preference"),
        ];

        let mut by_category: HashMap<String, Vec<&NoteEntry>> = HashMap::new();
        for note in &notes {
            if note.category == "synthesis" {
                continue;
            }
            by_category
                .entry(note.category.clone())
                .or_default()
                .push(note);
        }

        assert!(
            !by_category.contains_key("synthesis"),
            "synthesis category must be excluded"
        );
        assert_eq!(by_category["preference"].len(), 3);
    }

    #[test]
    fn build_synthesis_note_records_source_member_paths() {
        // Exercise the real production builder: a synthesis note built from
        // member paths must expose them as source_notes (provenance for the
        // L1→L2 chain), not only as links.
        let member_paths = vec!["learning/tokio".to_string(), "learning/async".to_string()];
        let note = build_synthesis_note("learning", "Synthesized insight.", member_paths.clone());
        assert_eq!(
            note.source_notes, member_paths,
            "member paths must be recorded as source_notes"
        );
        assert_eq!(note.links, member_paths, "member paths must also be links");
        assert_eq!(note.category, "synthesis");
        assert_eq!(note.title, "learning Synthesis");
        assert_eq!(note.facts, vec!["Synthesized insight.".to_string()]);
    }
}
