//! WikiIngestStage: compiles discrete facts into structured wiki pages.
//!
//! Scans the knowledge graph for high-value entity nodes, collects associated
//! facts, calls LLM to synthesize structured markdown wiki pages, stores them
//! as `FactType::Wiki` facts, and syncs wikilinks back to the graph.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::{DreamContext, DreamStage};
use crate::error::AlephError;
use crate::memory::context::{
    FactSource, FactSpecificity, FactType, MemoryFact, MemoryLayer, MemoryScope, MemoryTier,
};
use crate::memory::store::{GraphStore as StoreGraphStore, MemoryStore};
use crate::memory::wiki_sync;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;

/// Configuration for wiki ingestion during dreams.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiIngestConfig {
    pub enabled: bool,
    pub max_pages_per_run: usize,
    pub cooldown_days: u32,
    /// Minimum entity node decay_score to consider for compilation.
    pub min_node_score: f32,
    /// Minimum number of valid facts needed to trigger wiki compilation.
    pub min_facts_for_compile: usize,
    /// Source-fact validity ratio below which a wiki page is recompiled.
    pub stale_threshold: f32,
}

impl Default for WikiIngestConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_pages_per_run: 10,
            cooldown_days: 1,
            min_node_score: 0.5,
            min_facts_for_compile: 3,
            stale_threshold: 0.5,
        }
    }
}

/// Build a prompt for LLM wiki page compilation.
fn build_wiki_compile_prompt(
    entity_name: &str,
    facts: &[&MemoryFact],
    existing_content: Option<&str>,
) -> String {
    let mut prompt = String::new();

    if let Some(existing) = existing_content {
        prompt.push_str(&format!(
            "Update the following wiki page for \"{}\" with new information.\n\n\
             Existing page:\n```markdown\n{}\n```\n\n\
             New facts to integrate:\n",
            entity_name, existing
        ));
    } else {
        prompt.push_str(&format!(
            "Create a wiki page for \"{}\" by synthesizing these facts:\n\n",
            entity_name
        ));
    }

    for (i, fact) in facts.iter().enumerate() {
        prompt.push_str(&format!(
            "{}. [type={}, confidence={:.2}] {}\n",
            i + 1,
            fact.fact_type.as_str(),
            fact.confidence,
            fact.content,
        ));
    }

    prompt.push_str(
        "\nWrite a concise structured markdown page (under 500 words):\n\
         - Start with `# Entity Name`\n\
         - Include a `## Summary` section synthesizing key themes\n\
         - Include a `## Key Facts` section with bullet points\n\
         - Include a `## Related` section with [[wikilinks]] to related entities\n\
         - Note contradictions between facts explicitly\n\
         - Do NOT wrap the output in a code fence\n\
         - Respond ONLY with the markdown content, no preamble\n",
    );

    prompt
}

/// System prompt for wiki compilation.
const WIKI_COMPILE_SYSTEM: &str =
    "You are a knowledge base compiler. Synthesize facts into structured wiki pages. \
     Use [[wikilinks]] to reference related entities. Be concise and factual.";

/// Compiles discrete facts into structured wiki pages via LLM synthesis.
pub struct WikiIngestStage;

#[async_trait]
impl DreamStage for WikiIngestStage {
    fn name(&self) -> &'static str {
        "wiki_ingest"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        ctx.provider.is_some()
    }

    async fn execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let config = WikiIngestConfig::default();
        if !config.enabled {
            return Ok(ctx);
        }

        let provider = match ctx.provider.as_ref() {
            Some(p) => p,
            None => return Ok(ctx),
        };

        // 1. Get high-score entity nodes
        let candidate_nodes = StoreGraphStore::get_high_score_nodes(
            ctx.graph_store.database_ref(),
            config.min_node_score,
            config.max_pages_per_run * 2, // fetch extra to allow filtering
            "default",
        )
        .await?;

        if candidate_nodes.is_empty() {
            debug!("WikiIngestStage: no high-score entity nodes found");
            return Ok(ctx);
        }

        // 2. Collect existing wiki facts (all sources — manual pages take priority)
        let all_facts = ctx.database.get_all_facts(false, None).await?;
        let wiki_facts: Vec<&MemoryFact> = all_facts
            .iter()
            .filter(|f| f.fact_type == FactType::Wiki)
            .collect();

        let mut compiled_count: usize = 0;
        let mut recompiled_count: usize = 0;

        for node in &candidate_nodes {
            if compiled_count + recompiled_count >= config.max_pages_per_run {
                break;
            }

            // 3. Get fact IDs linked to this node
            let fact_links = StoreGraphStore::get_facts_for_node(
                ctx.graph_store.database_ref(),
                &node.id,
                "default",
            )
            .await?;

            // 4. Load and filter valid facts
            let mut source_facts: Vec<&MemoryFact> = Vec::new();
            for (fact_id, _weight) in &fact_links {
                if let Some(fact) = all_facts.iter().find(|f| f.id == *fact_id) {
                    if fact.is_valid && fact.fact_type != FactType::Wiki {
                        source_facts.push(fact);
                    }
                }
            }

            if source_facts.len() < config.min_facts_for_compile {
                continue;
            }

            // 5. Check if wiki page already exists for this node
            let existing_wiki = wiki_facts.iter().find(|wf| {
                let slug = wf
                    .path
                    .split('/')
                    .next_back()
                    .unwrap_or("")
                    .trim_end_matches(".md");
                slug.eq_ignore_ascii_case(&slugify(&node.name))
            });

            if let Some(existing) = existing_wiki {
                // Check staleness: what ratio of source_memory_ids are still valid?
                let total_sources = existing.source_memory_ids.len();
                if total_sources > 0 {
                    let valid_sources = existing
                        .source_memory_ids
                        .iter()
                        .filter(|sid| all_facts.iter().any(|f| f.id == **sid && f.is_valid))
                        .count();
                    let valid_ratio = valid_sources as f32 / total_sources as f32;
                    if valid_ratio >= config.stale_threshold {
                        // Page is fresh enough, skip
                        continue;
                    }
                    debug!(
                        entity = %node.name,
                        valid_ratio = valid_ratio,
                        "WikiIngestStage: recompiling stale wiki page"
                    );
                } else {
                    continue; // No source tracking, skip
                }
            }

            // 6. Call LLM to compile wiki page
            let existing_content = existing_wiki.map(|wf| wf.content.as_str());
            let prompt = build_wiki_compile_prompt(&node.name, &source_facts, existing_content);
            let msgs = [UnifiedMessage::user(&prompt)];
            let payload = RequestPayload::new(&msgs).with_system(Some(WIKI_COMPILE_SYSTEM));

            let content = match provider.process(payload).await {
                Ok(response) => {
                    let text = response.text_content();
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        warn!(entity = %node.name, "WikiIngestStage: LLM returned empty content");
                        continue;
                    }
                    trimmed.to_string()
                }
                Err(e) => {
                    warn!(entity = %node.name, error = %e, "WikiIngestStage: LLM compilation failed");
                    continue;
                }
            };

            let slug = slugify(&node.name);
            let path = format!("aleph://wiki/{}.md", slug);
            let source_ids: Vec<String> = source_facts.iter().map(|f| f.id.clone()).collect();

            // 7. Create or update the wiki fact
            if let Some(existing) = existing_wiki {
                // Update existing fact
                let mut updated = (*existing).clone();
                updated.content = content;
                updated.source_memory_ids = source_ids;
                updated.updated_at = crate::memory::dreaming::now_timestamp();
                ctx.database.update_fact(&updated).await?;

                // Re-sync wikilinks
                wiki_sync::sync_wikilinks_to_graph(&updated, &ctx.graph_store).await?;

                recompiled_count += 1;
            } else {
                // Create new wiki fact
                let wiki_fact = MemoryFact::new(content, FactType::Wiki, source_ids)
                    .with_fact_source(FactSource::Synthesis)
                    .with_tier(MemoryTier::Core)
                    .with_layer(MemoryLayer::L0Abstract)
                    .with_scope(MemoryScope::Global)
                    .with_specificity(FactSpecificity::Abstract)
                    .with_path(path)
                    .with_confidence(0.8);

                ctx.database.insert_fact(&wiki_fact).await?;

                // Sync wikilinks to graph
                wiki_sync::sync_wikilinks_to_graph(&wiki_fact, &ctx.graph_store).await?;

                // Link wiki fact to entity node via memory_entities
                ctx.graph_store
                    .link_memory_entity(&wiki_fact.id, &node.id, 1.0, "wiki_compile")
                    .await?;

                compiled_count += 1;
            }
        }

        info!(
            compiled = compiled_count,
            recompiled = recompiled_count,
            "WikiIngestStage: wiki compilation complete"
        );

        Ok(ctx)
    }
}

/// Convert an entity name to a URL-safe slug.
fn slugify(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiki_ingest_stage_name() {
        assert_eq!(WikiIngestStage.name(), "wiki_ingest");
    }

    #[test]
    fn wiki_ingest_config_defaults() {
        let config = WikiIngestConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_pages_per_run, 10);
        assert_eq!(config.cooldown_days, 1);
    }

    #[test]
    fn wiki_ingest_config_compile_defaults() {
        let config = WikiIngestConfig::default();
        assert!((config.min_node_score - 0.5).abs() < f32::EPSILON);
        assert_eq!(config.min_facts_for_compile, 3);
        assert!((config.stale_threshold - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Rust Ownership"), "rust-ownership");
        assert_eq!(slugify("LLM/GPT-4"), "llm-gpt-4");
        assert_eq!(slugify("hello"), "hello");
        assert_eq!(slugify("  spaces  "), "spaces");
    }

    #[test]
    fn build_prompt_new_page() {
        let fact = MemoryFact::new(
            "Rust uses ownership for memory safety".into(),
            FactType::Learning,
            vec![],
        );
        let facts = vec![&fact];
        let prompt = build_wiki_compile_prompt("Rust", &facts, None);
        assert!(prompt.contains("Create a wiki page"));
        assert!(prompt.contains("Rust"));
        assert!(prompt.contains("ownership for memory safety"));
        assert!(prompt.contains("[[wikilinks]]"));
    }

    #[test]
    fn build_prompt_update_page() {
        let fact = MemoryFact::new("New fact".into(), FactType::Learning, vec![]);
        let facts = vec![&fact];
        let prompt = build_wiki_compile_prompt("Rust", &facts, Some("# Rust\nOld content"));
        assert!(prompt.contains("Update the following wiki page"));
        assert!(prompt.contains("Old content"));
        assert!(prompt.contains("New fact"));
    }
}
