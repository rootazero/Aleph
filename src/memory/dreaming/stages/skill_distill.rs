//! `SkillDistill` stage — extracts reusable skill-notes from synthesis output.
//!
//! Per Phase 2 Decision 2, this stage uses **code-injected candidates**:
//! before each LLM call, [`find_similar_notes`] retrieves the top-N most
//! similar existing skill-notes by cosine similarity and injects their IDs
//! into the prompt. The LLM then emits a [`DistillAction`] referencing
//! those IDs verbatim — no hallucination, no string-matching dedup.
//!
//! When no embedding is available for the synthesis note, the candidate
//! list is empty and the LLM gracefully degrades to emitting `New` actions.

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::dreaming::distill_action::referenced_path;
use crate::memory::dreaming::{DistillAction, DistillActionRecord, DistillOutcome, DreamContext};
use crate::memory::notes::find_similar_notes;
use crate::memory::notes::store::NoteStore;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;

use super::DreamStage;

/// Number of existing skill candidates to inject into the LLM prompt per
/// synthesis note (Phase 2 Decision 2).
const CANDIDATES_TOP_N: usize = 5;

pub struct SkillDistillStage {
    pub max_per_cycle: usize,
}

impl Default for SkillDistillStage {
    fn default() -> Self {
        Self {
            max_per_cycle: crate::config::types::memory::default_skill_distill_max_per_cycle(),
        }
    }
}

#[async_trait]
impl DreamStage for SkillDistillStage {
    fn name(&self) -> &'static str {
        "skill_distill"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        ctx.notes.iter().any(|n| n.category == "synthesis")
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let synthesis_paths: Vec<String> = ctx
            .notes
            .iter()
            .filter(|n| n.category == "synthesis")
            .map(|n| n.path.clone())
            .collect();

        let mut applied = 0usize;
        let dim_hint = ctx.embedder.dimensions() as u32;

        // Supersedes the recall-evidence gate rejected on prior cycles. The
        // fingerprints drive the O(1) drop in `gate_action_evidence`; the full
        // records feed the prompt as negative feedback so the LLM stops
        // re-proposing losing edits (SkillOpt's rejected-edit buffer).
        let reject_records = ctx
            .database
            .distill_reject_records(&ctx.agent_id)
            .unwrap_or_default();
        let rejected_fingerprints: Vec<String> = reject_records
            .iter()
            .map(|r| r.fingerprint.clone())
            .collect();
        let rejected_feedback: Vec<(String, String, String)> = reject_records
            .into_iter()
            .map(|r| (r.target, r.summary, r.reason))
            .collect();

        for path in &synthesis_paths {
            let content = match ctx.load_content(path).await {
                Some(c) => c,
                None => continue,
            };

            // Decision 2: code fetches top-N existing skill candidates BEFORE
            // the LLM call. Empty embedding → empty candidates → LLM defaults
            // to `New` (graceful degradation).
            let synth_embedding = ctx
                .indexer
                .store()
                .get_embedding(path, &ctx.agent_id, dim_hint)
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            let candidates = find_similar_notes(
                ctx.indexer.store().as_ref(),
                "skill",
                &ctx.agent_id,
                &synth_embedding,
                CANDIDATES_TOP_N,
            )
            .await
            .unwrap_or_default();

            let prompt = build_distill_prompt_with_candidates(
                &content,
                "skill",
                self.max_per_cycle,
                &candidates,
                &rejected_feedback,
            );
            let system = "You are a skill distillation engine. Choose the right \
                          DistillAction variant per the schema. Reference candidate \
                          IDs verbatim when strengthening or superseding.";

            let msgs = vec![UnifiedMessage::user(&prompt)];
            let response = match ctx
                .provider
                .process(RequestPayload::new(&msgs).with_system(Some(system)))
                .await
            {
                Ok(r) => r,
                Err(e) if super::is_provider_exhausted(&e) => {
                    tracing::warn!(error = %e, "SkillDistill: provider exhausted — aborting dream cycle");
                    return Err(e);
                }
                Err(e) => {
                    tracing::warn!(path, error = %e, "SkillDistill LLM call failed");
                    continue;
                }
            };

            let actions = parse_distill_response(&response.text_content());
            let candidate_set: std::collections::HashSet<&str> =
                candidates.iter().map(|(p, _)| p.as_str()).collect();
            for raw_action in actions
                .into_iter()
                .take(self.max_per_cycle)
                .map(clamp_action)
            {
                // Spec 5 validation gate: format / semantic / safety checks.
                // Rejections never reach the indexer; downgrades (severity
                // softening on low confidence) pass through silently.
                use crate::memory::dreaming::skill_gate::{
                    validate_skill_action, SkillGateDecision,
                };
                let action = match validate_skill_action(raw_action.clone()) {
                    SkillGateDecision::Allow(a) => a,
                    SkillGateDecision::Reject(reason) => {
                        tracing::warn!(
                            reason = %reason,
                            "SkillDistill: skill_gate rejected action; dropping"
                        );
                        ctx.report
                            .distill_actions
                            .push(DistillActionRecord::from_action(
                                "skill_distill",
                                &raw_action,
                                DistillOutcome::FilteredInvalid,
                                Some(reason),
                            ));
                        continue;
                    }
                };
                // Anti-hallucination guard: actions referencing a path that
                // was NOT in the candidate set the LLM was shown are dropped
                // BEFORE apply. Record them with FilteredNonCandidate so the
                // provenance trail still shows the attempted mutation.
                if let Some(p) = referenced_path(&action) {
                    if !candidate_set.contains(p) {
                        tracing::warn!(
                            path = p,
                            "SkillDistill: action references non-candidate path; \
                             dropping to prevent cross-category mutation"
                        );
                        ctx.report
                            .distill_actions
                            .push(DistillActionRecord::from_action(
                                "skill_distill",
                                &action,
                                DistillOutcome::FilteredNonCandidate,
                                None,
                            ));
                        continue;
                    }
                }
                // Recall-evidence gate (SkillOpt F-gate analog): a destructive
                // Supersede must strictly out-score the target note's recall
                // support, and a previously rejected edit never re-applies.
                if let Some(record) = super::gate_action_evidence(
                    &ctx,
                    &action,
                    &rejected_fingerprints,
                    "skill_distill",
                )
                .await
                {
                    ctx.report.distill_actions.push(record);
                    continue;
                }
                // Destructive-edit budget: a Supersede replaces an existing
                // skill note and spends the shared per-cycle budget; additive
                // New/Strengthen are free.
                if let Some(record) = super::charge_distill_budget(
                    &mut ctx.evolution_budget,
                    &action,
                    "skill_distill",
                ) {
                    ctx.report.distill_actions.push(record);
                    continue;
                }
                match ctx
                    .indexer
                    .apply_distill_action(&ctx.agent_id, "skill", &action)
                    .await
                {
                    Ok(_) => {
                        // `Skip` is a pure no-op inside `apply_distill_action`,
                        // so `Ok` is not proof a note was written. Mirrors
                        // `FeedbackDistill`: the record still lands in the audit
                        // trail, but the write counter only counts writes.
                        if !matches!(action, DistillAction::Skip { .. }) {
                            applied += 1;
                        }
                        ctx.report
                            .distill_actions
                            .push(DistillActionRecord::from_action(
                                "skill_distill",
                                &action,
                                DistillOutcome::Applied,
                                None,
                            ));
                    }
                    Err(e) => {
                        tracing::warn!(path, error = %e, "apply_distill_action failed");
                        ctx.report
                            .distill_actions
                            .push(DistillActionRecord::from_action(
                                "skill_distill",
                                &action,
                                DistillOutcome::Error,
                                Some(e.to_string()),
                            ));
                    }
                }
            }
        }

        ctx.report
            .extra
            .insert("skill_distill_count".into(), applied.to_string());
        // This cycle's flow count lives in `extra["skill_distill_count"]`.
        // MutationGate's `distill_produced`/`distill_recalled` are set later
        // from the mature skill-note cohort (see compute_raw_metrics), not from
        // fresh produce — a just-written note can't have been recalled yet.
        tracing::info!(applied, "SkillDistill completed");
        Ok(ctx)
    }
}

/// Build the LLM prompt for skill distillation with code-injected candidates.
///
/// `candidates` is the output of `find_similar_notes` (path + cosine similarity).
/// The prompt instructs the LLM to choose one of four `DistillAction` variants
/// (`new`/`strengthen`/`supersede`/`skip`) per insight and to reference
/// candidate IDs verbatim when strengthening or superseding.
#[must_use]
pub fn build_distill_prompt_with_candidates(
    synthesis_text: &str,
    source_category: &str,
    max_per_cycle: usize,
    candidates: &[(String, f32)],
    rejected: &[(String, String, String)],
) -> String {
    let candidates_block = if candidates.is_empty() {
        "[]".to_string()
    } else {
        let entries: Vec<String> = candidates
            .iter()
            .map(|(path, sim)| format!("  {{\"id\": \"{path}\", \"similarity\": {sim:.2}}}"))
            .collect();
        format!("[\n{}\n]", entries.join(",\n"))
    };
    let rejected_block = super::render_rejected_block(rejected);
    format!(
        "Analyze this synthesis note from the '{source_category}' category and \
         decide whether each insight is:\n\
         - a NEW skill (no existing candidate covers it)\n\
         - a STRENGTHEN of an existing candidate (same rule, more evidence)\n\
         - a SUPERSEDE of an existing candidate (better wording / corrects it)\n\
         - a SKIP (transient noise, not actionable)\n\n\
         Synthesis:\n{synthesis_text}\n\n\
         Existing skill-note candidates (you MUST reference these IDs verbatim \
         if you choose strengthen or supersede):\n\
         existing_candidates: {candidates_block}\n\n\
         {rejected_block}\
         Quality bar: each NEW or SUPERSEDE rule must be a transferable procedure or \
         invariant (not a one-off fact); use a kebab-case title; prefer a \
         symptom→cause→fix shape; calibrate confidence to evidence strength and set \
         severity to real impact (low..critical); STRENGTHEN (don't reword) when the rule \
         already exists and you only have more evidence.\n\n\
         Emit at most {max_per_cycle} actions in this JSON shape:\n\
         ```json\n\
         {{\"actions\": [\n\
           {{\"type\": \"new\", \"title\": \"kebab-case-name\", \"rule\": \"...\", \"confidence\": 0.0-1.0, \"severity\": \"low|med|high|critical\", \"source_facts\": [\"...\"]}},\n\
           {{\"type\": \"strengthen\", \"existing_note_path\": \"<id from candidates>\", \"source_facts\": [\"...\"]}},\n\
           {{\"type\": \"supersede\", \"old_note_path\": \"<id from candidates>\", \"title\": \"...\", \"rule\": \"...\", \"confidence\": 0.0-1.0, \"severity\": \"low|med|high|critical\", \"source_facts\": [\"...\"]}},\n\
           {{\"type\": \"skip\", \"source_fact\": \"...\", \"reason\": \"...\"}}\n\
         ]}}\n\
         ```\n\
         Return `{{\"actions\": []}}` if nothing actionable."
    )
}

#[derive(serde::Deserialize)]
struct DistillResponse {
    actions: Vec<DistillAction>,
}

/// Tolerant parser: extracts the outermost `{...}` JSON object from the LLM
/// response text and deserializes it as `DistillResponse`. Returns empty on
/// any parse failure.
#[must_use]
pub fn parse_distill_response(text: &str) -> Vec<DistillAction> {
    let start = match text.find('{') {
        Some(s) => s,
        None => return Vec::new(),
    };
    let end = match text.rfind('}') {
        Some(e) => e,
        None => return Vec::new(),
    };
    // Guard against a `}` that precedes the first `{` (e.g. prose like
    // "no action}\n...{reconsider"): `&text[start..=end]` becomes `start..end+1`
    // and panics when start > end. Degrade to empty instead of crashing the
    // dream daemon. Mirrors the `end <= start` guard in note_review.rs.
    if end <= start {
        return Vec::new();
    }
    let json_str = &text[start..=end];
    serde_json::from_str::<DistillResponse>(json_str)
        .map(|r| r.actions)
        .unwrap_or_default()
}

/// Clamp `confidence` into `[0.0, 1.0]` for `New` and `Supersede` variants
/// to defend against out-of-range values from the LLM.
const fn clamp_action(mut a: DistillAction) -> DistillAction {
    use DistillAction::*;
    match &mut a {
        New { confidence, .. } | Supersede { confidence, .. } => {
            *confidence = confidence.clamp(0.0, 1.0);
        }
        _ => {}
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_name() {
        assert_eq!(SkillDistillStage::default().name(), "skill_distill");
    }

    #[test]
    fn stage_default_uses_config_default_cap() {
        assert_eq!(SkillDistillStage::default().max_per_cycle, 3);
    }

    #[test]
    fn build_distill_prompt_includes_quality_bar() {
        let prompt = build_distill_prompt_with_candidates(
            "synthesis text",
            "skill",
            5,
            &[("skill/async-error-handling".to_string(), 0.71)],
            &[],
        );
        assert!(
            prompt.contains("Quality bar:"),
            "prompt must teach the skill-note quality bar:\n{prompt}"
        );
        // existing contract preserved
        assert!(prompt.contains("existing_candidates"));
        assert!(prompt.contains("strengthen") && prompt.contains("supersede"));
        // No rejected feedback → no rejected block (byte-compatible baseline).
        assert!(!prompt.contains("Previously REJECTED"));
    }

    #[test]
    fn build_distill_prompt_includes_rejected_feedback() {
        let prompt = build_distill_prompt_with_candidates(
            "synthesis text",
            "skill",
            5,
            &[("skill/async-error-handling".to_string(), 0.71)],
            &[(
                "skill/retry-policy".to_string(),
                "retry-with-jitter".to_string(),
                "recall-evidence gate: confidence 0.40 does not beat support 0.60".to_string(),
            )],
        );
        assert!(
            prompt.contains("Previously REJECTED"),
            "prompt must replay rejected edits as negative feedback:\n{prompt}"
        );
        assert!(
            prompt.contains("skill/retry-policy"),
            "must name the rejected target"
        );
        assert!(
            prompt.contains("retry-with-jitter"),
            "must include the proposed title (summary)"
        );
    }

    #[test]
    fn build_distill_prompt_with_candidates_includes_existing_block() {
        let candidates = vec![
            ("skill/async-error-handling".to_string(), 0.92_f32),
            ("skill/borrow-fights".to_string(), 0.88),
        ];
        let prompt = build_distill_prompt_with_candidates(
            "Synthesis: borrow checker fights are common",
            "skill",
            3,
            &candidates,
            &[],
        );
        assert!(
            prompt.contains("existing_candidates"),
            "prompt must include candidates block:\n{prompt}"
        );
        assert!(
            prompt.contains("skill/async-error-handling"),
            "must list candidate IDs:\n{prompt}"
        );
        assert!(
            prompt.contains("strengthen"),
            "prompt must teach LLM about strengthen action"
        );
        assert!(
            prompt.contains("supersede"),
            "prompt must teach LLM about supersede action"
        );
        assert!(
            prompt.contains("\"new\"") || prompt.contains("\"type\": \"new\""),
            "prompt must teach about new"
        );
        assert!(
            prompt.contains("\"skip\"") || prompt.contains("\"type\": \"skip\""),
            "prompt must teach about skip"
        );
    }

    #[test]
    fn build_distill_prompt_with_no_candidates_still_works() {
        let prompt = build_distill_prompt_with_candidates("text", "skill", 3, &[], &[]);
        assert!(prompt.contains("existing_candidates"));
        assert!(prompt.contains("[]") || prompt.contains("(none)"));
    }

    #[test]
    fn parse_distill_response_extracts_actions() {
        let raw = r#"{"actions":[{"type":"new","title":"x","rule":"y","confidence":0.7,"severity":"med","source_facts":["S1"]}]}"#;
        let actions = parse_distill_response(raw);
        assert_eq!(actions.len(), 1);
    }

    #[test]
    fn parse_distill_response_invalid_returns_empty() {
        assert!(parse_distill_response("not json").is_empty());
    }

    #[test]
    fn parse_distill_response_handles_markdown_fence() {
        // LLMs often wrap JSON in ```json fences — the tolerant `{ ... }` extractor handles it.
        let raw = "Sure, here is the result:\n\
                   ```json\n\
                   {\"actions\":[{\"type\":\"skip\",\"source_fact\":\"F\",\"reason\":\"noise\"}]}\n\
                   ```";
        let actions = parse_distill_response(raw);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0], DistillAction::Skip { .. }));
    }

    // -----------------------------------------------------------------------
    // Stage-level: a Skip is a decision, not a write
    // -----------------------------------------------------------------------

    struct StubEmbedder;

    #[async_trait::async_trait]
    impl crate::memory::embedding_provider::EmbeddingProvider for StubEmbedder {
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

    /// `DreamContext` holding one synthesis note on disk (so `SkillDistill` has
    /// something to distill) plus a canned planner response.
    async fn ctx_with_one_synthesis(
        llm_response: &str,
    ) -> (crate::memory::dreaming::DreamContext, tempfile::TempDir) {
        use crate::memory::notes::NoteIndexer;
        use crate::memory::store::SqliteMemoryBackend;
        use crate::sync_primitives::Arc;

        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteMemoryBackend::new(&dir.path().join("mem.db")).unwrap());
        let indexer = NoteIndexer::new(dir.path().join("note"), store.clone());

        let synth_dir = dir.path().join("note").join("default").join("synthesis");
        tokio::fs::create_dir_all(&synth_dir).await.unwrap();
        tokio::fs::write(synth_dir.join("s1.md"), "- retries need jitter\n")
            .await
            .unwrap();

        let ctx = crate::memory::dreaming::DreamContext {
            notes: vec![crate::memory::dreaming::NoteEntry {
                path: "synthesis/s1".into(),
                category: "synthesis".into(),
                tags: vec![],
                created_at: 0,
                updated_at: 0,
                content_hash: "h".into(),
            }],
            note_contents: std::collections::HashMap::new(),
            agent_id: "default".into(),
            database: store.clone(),
            indexer,
            provider: Arc::new(crate::providers::mock::MockProvider::new(llm_response)),
            embedder: Arc::new(StubEmbedder),
            report: crate::memory::dreaming::DreamReport::default(),
            pipeline_type: "synthesize".into(),
            activity_checker: Arc::new(|| false),
            strategy: crate::memory::dreaming::DreamStrategy::Synthesize,
            orientation: None,
            evolution_budget: crate::memory::dreaming::EditBudget::default(),
        };
        (ctx, dir)
    }

    /// `apply_distill_action` is a pure no-op for `Skip`, so treating its `Ok`
    /// as a write reported skill notes that were never written. Mirrors the
    /// same fix in `FeedbackDistill`.
    #[tokio::test]
    async fn a_batch_of_only_skips_records_zero_applied() {
        let response = r#"{"actions":[
            {"type":"skip","source_fact":"S1","reason":"transient"},
            {"type":"skip","source_fact":"S2","reason":"transient"}
        ]}"#;
        let (ctx, _dir) = ctx_with_one_synthesis(response).await;

        let ctx = SkillDistillStage { max_per_cycle: 3 }
            .execute(ctx)
            .await
            .unwrap();

        assert_eq!(
            ctx.report
                .extra
                .get("skill_distill_count")
                .map(String::as_str),
            Some("0"),
            "a skip writes no note, so it must not be counted as a distilled skill"
        );
        // The audit trail still records the decision.
        assert_eq!(ctx.report.distill_actions.len(), 2);
        assert!(ctx
            .report
            .distill_actions
            .iter()
            .all(|r| r.action_kind == "skip"));
    }

    /// Positive control: a real `New` still counts, so the fix narrowed the
    /// counter rather than disabling it.
    #[tokio::test]
    async fn a_new_skill_still_counts_as_applied() {
        let response = r#"{"actions":[
            {"type":"new","title":"retry-with-jitter","rule":"add jitter to retries",
             "confidence":0.9,"severity":"med","source_facts":["S1"]},
            {"type":"skip","source_fact":"S2","reason":"transient"}
        ]}"#;
        let (ctx, _dir) = ctx_with_one_synthesis(response).await;

        let ctx = SkillDistillStage { max_per_cycle: 3 }
            .execute(ctx)
            .await
            .unwrap();

        assert_eq!(
            ctx.report
                .extra
                .get("skill_distill_count")
                .map(String::as_str),
            Some("1"),
            "the new skill is the only write in the batch"
        );
    }

    #[test]
    fn clamp_action_clamps_new_confidence() {
        let a = DistillAction::New {
            title: "t".into(),
            rule: "r".into(),
            confidence: 2.5,
            severity: crate::memory::notes::Severity::Low,
            source_facts: vec![],
        };
        let clamped = clamp_action(a);
        match clamped {
            DistillAction::New { confidence, .. } => {
                assert!((confidence - 1.0).abs() < 1e-6);
            }
            _ => panic!("variant changed"),
        }
    }
}
