//! L1 Overview generation for VFS directories
//!
//! Generates structured Markdown summaries for VFS directory paths,
//! storing them as `FactSource::Summary` facts. Uses content hashing
//! to detect staleness and skip regeneration when the underlying
//! facts have not changed.

use crate::error::AlephError;
use crate::memory::context::{compute_parent_path, FactSpecificity, FactType, TemporalScope};
use crate::memory::namespace::NamespaceScope;
use crate::memory::EmbeddingProvider;
use crate::memory::store::{MemoryBackend, MemoryStore};
use crate::memory::vfs::compute_directory_hash;
use crate::gateway::workspace::WorkspaceFilter;
use crate::memory::{FactSource, MemoryFact, MemoryLayer, SearchFilter};
use crate::providers::AiProvider;
use std::collections::HashSet;
use crate::sync_primitives::Arc;

/// L1 Overview generator
///
/// Generates structured Markdown summaries for VFS directories,
/// storing them as `FactSource::Summary` facts. The generator
/// uses content hashing to detect when an existing L1 is stale
/// and needs regeneration.
pub struct L1Generator {
    database: MemoryBackend,
    provider: Arc<dyn AiProvider>,
    embedder: Arc<dyn EmbeddingProvider>,
}

impl L1Generator {
    /// Create a new L1 generator
    pub fn new(
        database: MemoryBackend,
        provider: Arc<dyn AiProvider>,
        embedder: Arc<dyn EmbeddingProvider>,
    ) -> Self {
        Self {
            database,
            provider,
            embedder,
        }
    }

    /// Generate or update L1 Overview for a single path.
    ///
    /// Returns `true` if the L1 was (re)generated, `false` if the
    /// existing overview is still current or no facts exist under
    /// the path.
    pub async fn generate_for_path(&self, path: &str) -> Result<bool, AlephError> {
        // 1. Get all L2 facts under this path (exclude summaries)
        let facts = self.get_l2_facts(path).await?;
        if facts.is_empty() {
            return Ok(false);
        }

        // 2. Compute content hash from current facts
        let new_hash = compute_directory_hash(&facts);

        // 3. Check existing L1 — skip if hash matches
        // Old: db.get_l1_overview(path)
        // New: db.get_by_path(path, &NamespaceScope::Owner, "default")
        if let Some(existing_l1) = self.database.get_by_path(path, &NamespaceScope::Owner, "default").await? {
            if existing_l1.fact_source == FactSource::Summary && existing_l1.content_hash == new_hash {
                tracing::debug!(path = path, "L1 Overview is current, skipping");
                return Ok(false);
            }
        }

        // 4. Generate L1 content via LLM
        let l1_content = self.generate_l1_content(path, &facts).await?;

        // 5. Embed the L1 content
        let embedding = self
            .embedder
            .embed(&l1_content)
            .await
            .map_err(|e| AlephError::config(format!("Failed to embed L1: {}", e)))?;

        // 6. Store as Summary fact
        let parent_path = compute_parent_path(path);
        let mut l1_fact = MemoryFact::new(l1_content, FactType::Other, vec![])
            .with_path(path.to_string())
            .with_fact_source(FactSource::Summary)
            .with_embedding(embedding)
            .with_specificity(FactSpecificity::Principle)
            .with_temporal_scope(TemporalScope::Permanent)
            .with_confidence(1.0);
        l1_fact.content_hash = new_hash;
        l1_fact.parent_path = parent_path;

        // Upsert: invalidate old L1 if exists, then insert new
        // Old: db.get_l1_overview(path) → New: db.get_by_path(path, &NamespaceScope::Owner, "default")
        if let Some(old_l1) = self.database.get_by_path(path, &NamespaceScope::Owner, "default").await? {
            if old_l1.fact_source == FactSource::Summary {
                self.database
                    .invalidate_fact(&old_l1.id, "Superseded by updated L1 Overview")
                    .await?;
            }
        }
        self.database.insert_fact(&l1_fact).await?;

        tracing::info!(path = path, "Generated L1 Overview");
        Ok(true)
    }

    /// Generate L1 overviews for all affected paths after compression.
    ///
    /// Returns the number of L1s that were actually updated.
    pub async fn generate_for_affected_paths(
        &self,
        affected_paths: &HashSet<String>,
    ) -> Result<usize, AlephError> {
        let mut updated = 0;
        for path in affected_paths {
            match self.generate_for_path(path).await {
                Ok(true) => updated += 1,
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(path = %path, error = %e, "Failed to generate L1, skipping");
                }
            }
        }
        Ok(updated)
    }

    /// Retrieve L2 facts under `path`, excluding summaries.
    async fn get_l2_facts(&self, path: &str) -> Result<Vec<MemoryFact>, AlephError> {
        let filter = SearchFilter::new()
            .with_valid_only()
            .with_workspace(WorkspaceFilter::Single("default".to_string()))
            .with_layer(MemoryLayer::L2Detail);

        let facts = self
            .database
            .get_facts_by_path_prefix(path, &filter, 1000)
            .await?;

        Ok(facts
            .into_iter()
            .filter(|f| f.fact_source != FactSource::Summary)
            .collect())
    }

    /// Build the LLM prompt and call the provider to generate the overview.
    async fn generate_l1_content(
        &self,
        path: &str,
        facts: &[MemoryFact],
    ) -> Result<String, AlephError> {
        let facts_list: String = facts
            .iter()
            .enumerate()
            .map(|(i, f)| format!("{}. [{}] {}", i + 1, f.fact_type, f.content))
            .collect::<Vec<_>>()
            .join("\n");

        let path_display = path
            .trim_start_matches("aleph://")
            .trim_end_matches('/');

        let prompt = format!(
            r#"You are generating an L1 Overview for the knowledge directory: {path}

Below are the facts stored under this path:
{facts_list}

Generate a structured Markdown overview that:
1. Summarizes the key themes (3-5 bullet points)
2. Lists each sub-topic with a one-line description
3. Notes any contradictions or evolution
4. Total length: 500-1000 tokens

Format:
# {path_display}

## Key Themes
- ...

## Contents
- **topic**: one-line description
...

## Notes
- any contradictions or evolution"#,
            path = path,
            facts_list = facts_list,
            path_display = path_display,
        );

        let system_prompt =
            "You are a knowledge librarian. Generate concise, structured Markdown overviews.";

        let response = self
            .provider
            .process(&prompt, Some(system_prompt))
            .await
            .map_err(|e| AlephError::config(format!("LLM failed to generate L1: {}", e)))?;

        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use crate::sync_primitives::Arc;

    use crate::memory::store::lance::LanceMemoryBackend;
    use crate::memory::MemoryFact;

    use super::*;

    #[test]
    fn test_l1_prompt_format() {
        let path = "aleph://user/preferences/";
        let path_display = path
            .trim_start_matches("aleph://")
            .trim_end_matches('/');
        assert_eq!(path_display, "user/preferences");
    }

    #[tokio::test]
    async fn test_l1_generator_uses_scoped_prefix_query() {
        let temp_dir = tempfile::tempdir().unwrap();
        let backend = LanceMemoryBackend::open_or_create(temp_dir.path())
            .await
            .unwrap();
        let db: MemoryBackend = Arc::new(backend);

        let mut target_l2 = MemoryFact::new("Target L2".into(), FactType::Preference, vec![])
            .with_path("aleph://user/preferences/coding/rust".to_string())
            .with_layer(MemoryLayer::L2Detail)
            .with_fact_source(FactSource::Extracted);
        target_l2.workspace = "default".to_string();

        let mut target_non_l2 = MemoryFact::new("Target non-L2".into(), FactType::Preference, vec![])
            .with_path("aleph://user/preferences/coding/overview".to_string())
            .with_layer(MemoryLayer::L1Overview)
            .with_fact_source(FactSource::Manual);
        target_non_l2.workspace = "default".to_string();

        let mut other_path_l2 = MemoryFact::new("Other path".into(), FactType::Preference, vec![])
            .with_path("aleph://user/preferences/ui/theme".to_string())
            .with_layer(MemoryLayer::L2Detail)
            .with_fact_source(FactSource::Extracted);
        other_path_l2.workspace = "default".to_string();

        db.batch_insert_facts(&[target_l2.clone(), target_non_l2, other_path_l2])
            .await
            .unwrap();

        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(
            crate::memory::embedding_provider::tests::MockEmbeddingProvider::new(1024, "mock-model"),
        );
        let generator = L1Generator::new(
            db,
            crate::providers::create_mock_provider(),
            embedder,
        );

        let facts = generator
            .get_l2_facts("aleph://user/preferences/coding/")
            .await
            .unwrap();

        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].id, target_l2.id);
    }
}
