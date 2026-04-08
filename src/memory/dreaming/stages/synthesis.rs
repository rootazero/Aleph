//! DeepSynthesisStage: cross-cluster insight generation.
//!
//! Runs during both daily and weekly dream cycles. Fetches all valid LTM facts,
//! groups by fact_type, runs DBSCAN on embeddings within each group,
//! and creates synthesized Core facts from clusters with >= min_cluster_size members.
//! Uses LLM-based synthesis when a provider is available, with dedup against
//! existing synthesis facts.

use std::collections::HashMap;

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::error::AlephError;
use crate::memory::context::{
    FactSource, FactSpecificity, FactType, MemoryFact, MemoryLayer, MemoryScope, MemoryTier,
};
use crate::memory::dreaming::report::DreamRunType;
use crate::memory::store::MemoryStore;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;

use super::cluster::dbscan;
use super::{DreamContext, DreamStage};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// DBSCAN epsilon for synthesis clustering (cosine distance threshold).
const SYNTHESIS_EPS: f32 = 0.35;

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

/// Generates deep cross-cluster insights (daily and weekly runs).
pub struct DeepSynthesisStage;

#[async_trait]
impl DreamStage for DeepSynthesisStage {
    fn name(&self) -> &'static str {
        "deep_synthesis"
    }

    async fn should_run(&self, _ctx: &DreamContext) -> bool {
        true
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let min_cluster_size = ctx.config.synthesis_min_cluster_size();
        let max_insights = ctx.config.synthesis_max_insights();

        // For daily runs, skip if no clusters were produced by earlier stages
        if ctx.run_metadata.run_type == DreamRunType::Daily && ctx.clusters.is_empty() {
            debug!("DeepSynthesisStage: daily run with no clusters, skipping");
            return Ok(ctx);
        }

        // 1. Fetch all valid facts once, partition into LTM and existing synthesis
        let all_facts = ctx.database.get_all_facts(false, None).await?;
        let mut ltm_facts: Vec<MemoryFact> = Vec::new();
        let mut existing_synthesis: Vec<MemoryFact> = Vec::new();
        for f in all_facts {
            if !f.is_valid {
                continue;
            }
            if f.fact_source == FactSource::Synthesis {
                existing_synthesis.push(f);
            } else if f.tier == MemoryTier::LongTerm {
                ltm_facts.push(f);
            }
        }

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

            if with_emb.len() < min_cluster_size {
                continue;
            }

            let embeddings: Vec<Vec<f32>> = with_emb.iter().map(|(_, e)| (*e).clone()).collect();
            let emb_to_idx: Vec<usize> = with_emb.iter().map(|(i, _)| *i).collect();

            let labels = dbscan(&embeddings, SYNTHESIS_EPS, min_cluster_size);

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
                if member_indices.len() < min_cluster_size {
                    continue;
                }

                let cluster_facts: Vec<&MemoryFact> =
                    member_indices.iter().map(|&i| group_facts[i]).collect();

                // Compute average confidence
                let avg_confidence = cluster_facts.iter().map(|f| f.confidence).sum::<f32>()
                    / cluster_facts.len() as f32;

                let source_ids: Vec<String> = cluster_facts.iter().map(|f| f.id.clone()).collect();

                // Build combined content for theme
                let combined: String = cluster_facts
                    .iter()
                    .map(|f| f.content.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");

                // Use LLM synthesis when provider is available
                let theme = if let Some(ref provider) = ctx.provider {
                    let facts_tuples: Vec<(&str, f32, &str)> = cluster_facts
                        .iter()
                        .map(|f| (f.fact_type.as_str(), f.confidence, f.content.as_str()))
                        .collect();
                    let prompt = build_synthesis_prompt(&facts_tuples);
                    let msgs = [UnifiedMessage::user(&prompt)];
                    let system = "You are a pattern analyst. Respond with a short theme label summarizing the pattern. No JSON, just the label.";
                    let payload = RequestPayload::new(&msgs).with_system(Some(system));
                    match provider.process(payload).await {
                        Ok(response) => {
                            let text = response.text_content();
                            parse_synthesis_content(&text)
                                .unwrap_or_else(|| format!("[{}] {}", fact_type_str, combined))
                        }
                        Err(e) => {
                            warn!(error = %e, "LLM synthesis theme failed, using fallback");
                            format!("[{}] {}", fact_type_str, combined)
                        }
                    }
                } else {
                    format!("[{}] {}", fact_type_str, combined)
                };

                insights.push(PatternInsight {
                    theme,
                    source_fact_ids: source_ids,
                    frequency: cluster_facts.len(),
                    confidence: avg_confidence,
                });

                if insights.len() >= max_insights {
                    break;
                }
            }

            if insights.len() >= max_insights {
                break;
            }
        }

        // 4. Create synthesized Core facts with dedup (existing_synthesis from step 1)
        let mut created_count = 0;
        for insight in &insights {
            // Use LLM to generate synthesis content when provider is available
            let content = if let Some(ref provider) = ctx.provider {
                let facts_tuples: Vec<(&str, f32, &str)> = insight
                    .source_fact_ids
                    .iter()
                    .filter_map(|id| ltm_facts.iter().find(|f| f.id == *id))
                    .map(|f| (f.fact_type.as_str(), f.confidence, f.content.as_str()))
                    .collect();
                if facts_tuples.is_empty() {
                    format!("Pattern: {}", insight.theme)
                } else {
                    let prompt = build_synthesis_prompt(&facts_tuples);
                    let msgs = [UnifiedMessage::user(&prompt)];
                    let system = "You are a pattern analyst. Synthesize a concise insight from the given facts. Respond with JSON: {\"content\": \"...\"}";
                    let payload = RequestPayload::new(&msgs).with_system(Some(system));
                    match provider.process(payload).await {
                        Ok(response) => {
                            let text = response.text_content();
                            parse_synthesis_content(&text)
                                .unwrap_or_else(|| format!("Pattern: {}", insight.theme))
                        }
                        Err(e) => {
                            warn!(error = %e, "LLM synthesis content failed, using fallback");
                            format!("Pattern: {}", insight.theme)
                        }
                    }
                }
            } else {
                format!("Pattern: {}", insight.theme)
            };

            let fact_type = insight
                .theme
                .split(']')
                .next()
                .and_then(|s| s.strip_prefix('['))
                .and_then(|s| s.parse::<FactType>().ok())
                .unwrap_or(FactType::Other);

            let synthesized = MemoryFact::new(content, fact_type, insight.source_fact_ids.clone())
                .with_fact_source(FactSource::Synthesis)
                .with_tier(MemoryTier::Core)
                .with_layer(MemoryLayer::L0Abstract)
                .with_scope(MemoryScope::Global)
                .with_specificity(FactSpecificity::Abstract)
                .with_confidence(insight.confidence);

            // Dedup: check content similarity against existing synthesis facts.
            // New facts don't have embeddings yet, so we compare by content text.
            // Use embedding cosine similarity when both have embeddings, otherwise
            // fall back to normalized content comparison.
            let near_dup = existing_synthesis.iter().find(|e| {
                // Try embedding-based similarity first (if both have embeddings)
                if let (Some(ref new_emb), Some(ref old_emb)) =
                    (&synthesized.embedding, &e.embedding)
                {
                    cosine_similarity(new_emb, old_emb) > 0.85
                } else {
                    // Fallback: normalized content string comparison
                    let new_norm = synthesized.content.trim().to_lowercase();
                    let old_norm = e.content.trim().to_lowercase();
                    new_norm == old_norm
                }
            });
            if let Some(dup) = near_dup {
                // Refresh the existing fact's timestamp instead of inserting a duplicate
                if let Err(e) = ctx.database.touch_fact_updated_at(&dup.id) {
                    warn!(error = %e, fact_id = %dup.id, "DeepSynthesisStage: failed to touch duplicate fact");
                }
                continue;
            }

            if let Err(e) = ctx.database.insert_fact(&synthesized).await {
                warn!(error = %e, "DeepSynthesisStage: failed to insert synthesized fact");
                continue;
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
// Prompt builder
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
// Helpers
// ---------------------------------------------------------------------------

/// Parse LLM synthesis response, trying JSON fields then plain text.
fn parse_synthesis_content(response: &str) -> Option<String> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(response) {
        if let Some(content) = v.get("content").and_then(|c| c.as_str()) {
            return Some(content.to_string());
        }
        if let Some(insight) = v.get("insight").and_then(|c| c.as_str()) {
            return Some(insight.to_string());
        }
    }
    let trimmed = response.trim();
    if !trimmed.is_empty() && trimmed.len() < 500 {
        Some(trimmed.to_string())
    } else {
        None
    }
}

/// Cosine similarity between two embedding vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        dot / denom
    }
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
    async fn should_run_returns_true_for_daily() {
        let stage = DeepSynthesisStage;
        // Deep synthesis now runs for both daily and weekly cycles.
        let metadata = make_run_metadata(DreamRunType::Daily);
        assert_eq!(metadata.run_type, DreamRunType::Daily);
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

    // -----------------------------------------------------------------------
    // should_run via DreamStage trait with real DreamContext
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_run_true_for_daily_via_trait() {
        use super::DreamContext;
        use crate::memory::decay::DecayConfig;
        use crate::memory::dreaming::stages::drift::DriftAction;
        use crate::memory::graph::{GraphDecayConfig, GraphStore};
        use crate::sync_primitives::Arc;

        let tmp = tempfile::tempdir().expect("tempdir");
        let database = crate::memory::store::SqliteMemoryBackend::new(tmp.path())
                .expect("sqlite backend");
        let database = Arc::new(database);

        let ctx = DreamContext {
            memories: Vec::new(),
            clusters: Vec::new(),
            new_facts: Vec::new(),
            drift_resolutions: Vec::<DriftAction>::new(),
            config: crate::config::DreamingConfig::default(),
            run_metadata: make_run_metadata(DreamRunType::Daily),
            activity_checker: Arc::new(|| false),
            synthesis_insights_count: 0,
            database: database.clone(),
            graph_store: GraphStore::new(database),
            graph_decay_config: GraphDecayConfig::default(),
            memory_decay_config: DecayConfig::default(),
            command_handler: None,
            provider: None,
            graph_decay_report: None,
            memory_decay_report: None,
        };

        let stage = DeepSynthesisStage;
        assert!(
            stage.should_run(&ctx).await,
            "Daily runs should now execute deep synthesis"
        );
    }

    #[tokio::test]
    async fn should_run_true_for_weekly_via_trait() {
        use super::DreamContext;
        use crate::memory::decay::DecayConfig;
        use crate::memory::dreaming::stages::drift::DriftAction;
        use crate::memory::graph::{GraphDecayConfig, GraphStore};
        use crate::sync_primitives::Arc;

        let tmp = tempfile::tempdir().expect("tempdir");
        let database = crate::memory::store::SqliteMemoryBackend::new(tmp.path())
                .expect("sqlite backend");
        let database = Arc::new(database);

        let ctx = DreamContext {
            memories: Vec::new(),
            clusters: Vec::new(),
            new_facts: Vec::new(),
            drift_resolutions: Vec::<DriftAction>::new(),
            config: crate::config::DreamingConfig::default(),
            run_metadata: make_run_metadata(DreamRunType::Weekly),
            activity_checker: Arc::new(|| false),
            synthesis_insights_count: 0,
            database: database.clone(),
            graph_store: GraphStore::new(database),
            graph_decay_config: GraphDecayConfig::default(),
            memory_decay_config: DecayConfig::default(),
            command_handler: None,
            provider: None,
            graph_decay_report: None,
            memory_decay_report: None,
        };

        let stage = DeepSynthesisStage;
        assert!(
            stage.should_run(&ctx).await,
            "Weekly runs should execute deep synthesis"
        );
    }

    // -----------------------------------------------------------------------
    // execute skips when too few LTM facts
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn execute_skips_when_too_few_ltm_facts() {
        use super::DreamContext;
        use crate::memory::decay::DecayConfig;
        use crate::memory::dreaming::stages::drift::DriftAction;
        use crate::memory::graph::{GraphDecayConfig, GraphStore};
        use crate::sync_primitives::Arc;

        let tmp = tempfile::tempdir().expect("tempdir");
        let database = crate::memory::store::SqliteMemoryBackend::new(tmp.path())
                .expect("sqlite backend");
        let database = Arc::new(database);

        // The empty database has zero LTM facts — fewer than min_cluster_size (3).
        let ctx = DreamContext {
            memories: Vec::new(),
            clusters: Vec::new(),
            new_facts: Vec::new(),
            drift_resolutions: Vec::<DriftAction>::new(),
            config: crate::config::DreamingConfig::default(),
            run_metadata: make_run_metadata(DreamRunType::Weekly),
            activity_checker: Arc::new(|| false),
            synthesis_insights_count: 0,
            database: database.clone(),
            graph_store: GraphStore::new(database),
            graph_decay_config: GraphDecayConfig::default(),
            memory_decay_config: DecayConfig::default(),
            command_handler: None,
            provider: None,
            graph_decay_report: None,
            memory_decay_report: None,
        };

        let stage = DeepSynthesisStage;
        let result = stage.execute(ctx).await.unwrap();
        assert_eq!(
            result.synthesis_insights_count, 0,
            "Should produce zero insights when database has no LTM facts"
        );
    }
}
