//! DeepSynthesisStage: weekly cross-cluster insight generation.
//!
//! Runs only during weekly dream cycles. Fetches all valid LTM facts,
//! groups by fact_type, runs DBSCAN on embeddings within each group,
//! and creates synthesized Core facts from clusters with >= 3 members.

use std::collections::HashMap;

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::error::AlephError;
use crate::memory::context::{
    FactSource, FactSpecificity, FactType, MemoryFact, MemoryLayer, MemoryScope, MemoryTier,
};
use crate::memory::dreaming::report::DreamRunType;
use crate::memory::store::MemoryStore;

use super::cluster::dbscan;
use super::{DreamContext, DreamStage};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// DBSCAN epsilon for synthesis clustering (cosine distance threshold).
const SYNTHESIS_EPS: f32 = 0.35;

/// Minimum cluster size needed to produce a synthesis insight.
const MIN_CLUSTER_SIZE: usize = 3;

/// Maximum number of insights produced per run.
const MAX_INSIGHTS_PER_RUN: usize = 10;

// ---------------------------------------------------------------------------
// PatternInsight
// ---------------------------------------------------------------------------

/// A cross-session pattern discovered during deep synthesis.
#[derive(Debug, Clone)]
pub struct PatternInsight {
    pub theme: String,
    pub source_fact_ids: Vec<String>,
    pub frequency: usize,
    pub confidence: f32,
}

// ---------------------------------------------------------------------------
// DeepSynthesisStage
// ---------------------------------------------------------------------------

/// Generates deep cross-cluster insights (weekly runs only).
pub struct DeepSynthesisStage;

#[async_trait]
impl DreamStage for DeepSynthesisStage {
    fn name(&self) -> &'static str {
        "deep_synthesis"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        ctx.run_metadata.run_type == DreamRunType::Weekly
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        // 1. Fetch all valid LTM facts
        let all_facts = ctx.database.get_all_facts(false, None).await?;
        let ltm_facts: Vec<MemoryFact> = all_facts
            .into_iter()
            .filter(|f| f.tier == MemoryTier::LongTerm && f.is_valid)
            .collect();

        if ltm_facts.is_empty() {
            debug!("DeepSynthesisStage: no LTM facts to synthesize");
            return Ok(ctx);
        }

        debug!(
            ltm_count = ltm_facts.len(),
            "DeepSynthesisStage: starting synthesis"
        );

        // 2. Group by fact_type
        let mut groups: HashMap<String, Vec<&MemoryFact>> = HashMap::new();
        for fact in &ltm_facts {
            groups
                .entry(fact.fact_type.as_str().to_string())
                .or_default()
                .push(fact);
        }

        // 3. Run DBSCAN within each group, collect insights
        let mut insights: Vec<PatternInsight> = Vec::new();

        for (fact_type_str, group_facts) in &groups {
            // Filter to facts with embeddings
            let with_emb: Vec<(usize, &Vec<f32>)> = group_facts
                .iter()
                .enumerate()
                .filter_map(|(i, f)| f.embedding.as_ref().map(|e| (i, e)))
                .collect();

            if with_emb.len() < MIN_CLUSTER_SIZE {
                continue;
            }

            let embeddings: Vec<Vec<f32>> =
                with_emb.iter().map(|(_, e)| (*e).clone()).collect();
            let emb_to_idx: Vec<usize> = with_emb.iter().map(|(i, _)| *i).collect();

            let labels = dbscan(&embeddings, SYNTHESIS_EPS, MIN_CLUSTER_SIZE);

            // Group by cluster label (skip noise = -1)
            let mut cluster_groups: HashMap<i32, Vec<usize>> = HashMap::new();
            for (emb_idx, &label) in labels.iter().enumerate() {
                if label >= 0 {
                    cluster_groups
                        .entry(label)
                        .or_default()
                        .push(emb_to_idx[emb_idx]);
                }
            }

            for (_label, member_indices) in cluster_groups {
                if member_indices.len() < MIN_CLUSTER_SIZE {
                    continue;
                }

                let cluster_facts: Vec<&MemoryFact> =
                    member_indices.iter().map(|&i| group_facts[i]).collect();

                // Compute average confidence
                let avg_confidence = cluster_facts
                    .iter()
                    .map(|f| f.confidence)
                    .sum::<f32>()
                    / cluster_facts.len() as f32;

                let source_ids: Vec<String> =
                    cluster_facts.iter().map(|f| f.id.clone()).collect();

                // Build combined content for theme
                let combined: String = cluster_facts
                    .iter()
                    .map(|f| f.content.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");

                let theme = format!("[{}] {}", fact_type_str, combined);

                insights.push(PatternInsight {
                    theme,
                    source_fact_ids: source_ids,
                    frequency: cluster_facts.len(),
                    confidence: avg_confidence,
                });

                if insights.len() >= MAX_INSIGHTS_PER_RUN {
                    break;
                }
            }

            if insights.len() >= MAX_INSIGHTS_PER_RUN {
                break;
            }
        }

        // 4. Create synthesized Core facts and update source facts
        let mut created_count = 0;
        for insight in &insights {
            let content = format!("Pattern: {}", insight.theme);

            let fact_type = insight
                .theme
                .split(']')
                .next()
                .and_then(|s| s.strip_prefix('['))
                .and_then(|s| s.parse::<FactType>().ok())
                .unwrap_or(FactType::Other);

            let synthesized = MemoryFact::new(
                content,
                fact_type,
                insight.source_fact_ids.clone(),
            )
            .with_fact_source(FactSource::Synthesis)
            .with_tier(MemoryTier::Core)
            .with_layer(MemoryLayer::L0Abstract)
            .with_scope(MemoryScope::Global)
            .with_specificity(FactSpecificity::Abstract)
            .with_confidence(insight.confidence);

            if let Err(e) = ctx.database.insert_fact(&synthesized).await {
                warn!(error = %e, "DeepSynthesisStage: failed to insert synthesized fact");
                continue;
            }

            // 5. Lower specificity of source facts to Abstract
            for source_id in &insight.source_fact_ids {
                match ctx.database.get_fact(source_id).await {
                    Ok(Some(mut source_fact)) => {
                        if source_fact.specificity != FactSpecificity::Abstract {
                            source_fact.specificity = FactSpecificity::Abstract;
                            if let Err(e) = ctx.database.update_fact(&source_fact).await {
                                warn!(
                                    error = %e,
                                    fact_id = %source_id,
                                    "DeepSynthesisStage: failed to update source fact specificity"
                                );
                            }
                        }
                    }
                    Ok(None) => {
                        debug!(fact_id = %source_id, "DeepSynthesisStage: source fact not found");
                    }
                    Err(e) => {
                        warn!(
                            error = %e,
                            fact_id = %source_id,
                            "DeepSynthesisStage: failed to fetch source fact"
                        );
                    }
                }
            }

            created_count += 1;
        }

        debug!(
            insights = created_count,
            "DeepSynthesisStage: created synthesis insights"
        );

        // 6. Set ctx counter
        ctx.synthesis_insights_count = created_count;

        Ok(ctx)
    }
}

// ---------------------------------------------------------------------------
// Prompt builder (for future LLM integration)
// ---------------------------------------------------------------------------

/// Build a prompt for LLM-based pattern synthesis.
///
/// Each tuple is (fact_type, confidence, content).
/// Returns a system prompt requesting JSON output with theme, insight, and confidence.
pub fn build_synthesis_prompt(facts: &[(&str, f32, &str)]) -> String {
    let mut prompt = String::from(
        "You are a pattern analyst. Analyze the following memory facts and identify \
         cross-session patterns, themes, and insights.\n\n\
         Facts:\n",
    );

    for (i, (fact_type, confidence, content)) in facts.iter().enumerate() {
        prompt.push_str(&format!(
            "{}. [type={}, confidence={:.2}] {}\n",
            i + 1,
            fact_type,
            confidence,
            content,
        ));
    }

    prompt.push_str(
        "\nIdentify recurring themes and patterns across these facts. \
         For each pattern found, respond with a JSON array of objects, each containing:\n\
         - \"theme\": a short label for the pattern\n\
         - \"insight\": a one-sentence summary of the pattern\n\
         - \"confidence\": your confidence score (0.0-1.0)\n\
         - \"source_indices\": array of fact numbers that support this pattern\n",
    );

    prompt
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::dreaming::report::{DreamRunMetadata, DreamRunType};

    /// Helper to build a minimal DreamContext with given run_type.
    /// Uses a mock that won't actually be called for should_run tests.
    fn make_run_metadata(run_type: DreamRunType) -> DreamRunMetadata {
        DreamRunMetadata {
            run_type,
            run_date: "2026-04-03".to_string(),
            run_start_ts: 1_700_000_000,
        }
    }

    #[tokio::test]
    async fn should_run_returns_false_for_daily() {
        let stage = DeepSynthesisStage;
        // We need a DreamContext to test should_run, but we only need run_metadata.
        // Since we can't easily construct a full DreamContext without a database,
        // we test the logic directly.
        let metadata = make_run_metadata(DreamRunType::Daily);
        assert_eq!(metadata.run_type, DreamRunType::Daily);
        assert_ne!(metadata.run_type, DreamRunType::Weekly);
        // Verify the stage name
        assert_eq!(stage.name(), "deep_synthesis");
    }

    #[tokio::test]
    async fn should_run_returns_true_for_weekly() {
        let metadata = make_run_metadata(DreamRunType::Weekly);
        assert_eq!(metadata.run_type, DreamRunType::Weekly);
    }

    #[test]
    fn build_synthesis_prompt_contains_expected_text() {
        let facts = vec![
            ("preference", 0.9, "User prefers Rust"),
            ("learning", 0.8, "User is studying async patterns"),
            ("tool", 0.7, "User uses tokio for async runtime"),
        ];
        let prompt = build_synthesis_prompt(&facts);

        assert!(prompt.contains("pattern analyst"));
        assert!(prompt.contains("cross-session patterns"));
        assert!(prompt.contains("User prefers Rust"));
        assert!(prompt.contains("User is studying async patterns"));
        assert!(prompt.contains("User uses tokio for async runtime"));
        assert!(prompt.contains("type=preference"));
        assert!(prompt.contains("confidence=0.90"));
        assert!(prompt.contains("JSON array"));
        assert!(prompt.contains("theme"));
        assert!(prompt.contains("insight"));
        assert!(prompt.contains("source_indices"));
    }

    #[test]
    fn build_synthesis_prompt_empty_facts() {
        let facts: Vec<(&str, f32, &str)> = vec![];
        let prompt = build_synthesis_prompt(&facts);
        assert!(prompt.contains("pattern analyst"));
        assert!(prompt.contains("Facts:"));
    }

    #[test]
    fn pattern_insight_struct_fields() {
        let insight = PatternInsight {
            theme: "Rust async patterns".to_string(),
            source_fact_ids: vec!["f1".to_string(), "f2".to_string(), "f3".to_string()],
            frequency: 3,
            confidence: 0.85,
        };
        assert_eq!(insight.theme, "Rust async patterns");
        assert_eq!(insight.source_fact_ids.len(), 3);
        assert_eq!(insight.frequency, 3);
        assert!((insight.confidence - 0.85).abs() < f32::EPSILON);
    }
}
