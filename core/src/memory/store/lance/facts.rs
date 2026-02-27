//! MemoryStore trait implementation for LanceMemoryBackend.
//!
//! Provides all Fact CRUD operations (insert, get, update, delete, batch_insert),
//! multi-modal search (vector, text, hybrid), VFS path queries, statistics,
//! and mutation helpers against the LanceDB `facts` table.

use std::collections::HashMap;

use arrow_array::RecordBatch;
use arrow_array::{Float32Array, RecordBatchIterator};
use async_trait::async_trait;
use futures::TryStreamExt;
use lance_index::scalar::FullTextSearchQuery;
use lancedb::query::{ExecutableQuery, QueryBase, Select};

use crate::error::AlephError;
use crate::memory::audit::AuditEntry;
use crate::memory::context::{FactStats, FactType, MemoryFact};
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::types::{ScoredFact, SearchFilter};
#[allow(deprecated)]
use crate::memory::store::{AuditStore, HybridSearchParams, MemoryStore, PathEntry};

use super::arrow_convert::{facts_to_record_batch, record_batch_to_facts};
use super::LanceMemoryBackend;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Collect a LanceDB query stream into a vector of RecordBatches.
async fn collect_batches(
    stream: lancedb::arrow::SendableRecordBatchStream,
) -> Result<Vec<RecordBatch>, AlephError> {
    stream.try_collect().await.map_err(super::lance_err)
}

/// Execute a filtered scan and return all matching facts.
async fn scan_facts(
    table: &lancedb::Table,
    filter: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<MemoryFact>, AlephError> {
    let mut query = table.query();

    if let Some(f) = filter {
        query = query.only_if(f);
    }
    if let Some(lim) = limit {
        query = query.limit(lim);
    }

    query = query.select(Select::All);

    let stream = query.execute().await.map_err(super::lance_err)?;
    let batches = collect_batches(stream).await?;

    let mut facts = Vec::new();
    for batch in &batches {
        let mut batch_facts = record_batch_to_facts(batch)?;
        facts.append(&mut batch_facts);
    }
    Ok(facts)
}

/// Insert a RecordBatch into the facts table.
async fn add_batch(
    table: &lancedb::Table,
    batch: RecordBatch,
) -> Result<(), AlephError> {
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
    table
        .add(batches)
        .execute()
        .await
        .map_err(super::lance_err)?;
    Ok(())
}

/// Extract `_distance` score from a RecordBatch at a given row.
fn read_distance(batch: &RecordBatch, row: usize) -> f32 {
    batch
        .column_by_name("_distance")
        .and_then(|col| col.as_any().downcast_ref::<Float32Array>())
        .map(|arr| arr.value(row))
        .unwrap_or(0.0)
}

/// Extract `_score` from a RecordBatch at a given row (FTS relevance).
fn read_score(batch: &RecordBatch, row: usize) -> f32 {
    batch
        .column_by_name("_score")
        .and_then(|col| col.as_any().downcast_ref::<Float32Array>())
        .map(|arr| arr.value(row))
        .unwrap_or(0.0)
}

/// Extract `_relevance_score` from a RecordBatch at a given row (hybrid search).
fn read_relevance_score(batch: &RecordBatch, row: usize) -> f32 {
    batch
        .column_by_name("_relevance_score")
        .and_then(|col| col.as_any().downcast_ref::<Float32Array>())
        .map(|arr| arr.value(row))
        .unwrap_or(0.0)
}

/// Convert distance to a similarity score in [0, 1].
///
/// LanceDB uses L2 distance by default, so lower is better.
/// We convert: similarity = 1 / (1 + distance).
fn distance_to_similarity(distance: f32) -> f32 {
    1.0 / (1.0 + distance)
}

// ============================================================================
// MemoryStore implementation
// ============================================================================

#[async_trait]
impl MemoryStore for LanceMemoryBackend {
    // -- CRUD ---------------------------------------------------------------

    async fn insert_fact(&self, fact: &MemoryFact) -> Result<(), AlephError> {
        let batch = facts_to_record_batch(std::slice::from_ref(fact))?;
        add_batch(&self.facts_table, batch).await
    }

    async fn get_fact(&self, id: &str) -> Result<Option<MemoryFact>, AlephError> {
        let filter = format!("id = '{}'", id);
        let facts = scan_facts(&self.facts_table, Some(&filter), Some(1)).await?;
        Ok(facts.into_iter().next())
    }

    async fn update_fact(&self, fact: &MemoryFact) -> Result<(), AlephError> {
        // LanceDB's update API is limited; delete-then-insert is simpler.
        self.delete_fact(&fact.id).await?;
        self.insert_fact(fact).await
    }

    async fn delete_fact(&self, id: &str) -> Result<(), AlephError> {
        self.facts_table
            .delete(&format!("id = '{}'", id))
            .await
            .map_err(super::lance_err)?;
        Ok(())
    }

    async fn batch_insert_facts(&self, facts: &[MemoryFact]) -> Result<(), AlephError> {
        if facts.is_empty() {
            return Ok(());
        }
        let batch = facts_to_record_batch(facts)?;
        add_batch(&self.facts_table, batch).await
    }

    // -- Search -------------------------------------------------------------

    async fn vector_search(
        &self,
        embedding: &[f32],
        dim_hint: u32,
        filter: &SearchFilter,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let column_name = format!("vec_{}", dim_hint);

        let mut query = self
            .facts_table
            .query()
            .nearest_to(embedding)
            .map_err(super::lance_err)?
            .column(&column_name)
            .limit(limit);

        if let Some(f) = filter.to_lance_filter() {
            query = query.only_if(f);
        }

        let stream = query.execute().await.map_err(super::lance_err)?;
        let batches = collect_batches(stream).await?;

        let mut results = Vec::new();
        for batch in &batches {
            let facts = record_batch_to_facts(batch)?;
            for (i, fact) in facts.into_iter().enumerate() {
                let distance = read_distance(batch, i);
                let score = distance_to_similarity(distance);
                results.push(ScoredFact { fact, score });
            }
        }

        Ok(results)
    }

    async fn text_search(
        &self,
        query_text: &str,
        filter: &SearchFilter,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let fts_query = FullTextSearchQuery::new(query_text.to_owned());

        let mut query = self
            .facts_table
            .query()
            .full_text_search(fts_query)
            .select(Select::All)
            .limit(limit);

        if let Some(f) = filter.to_lance_filter() {
            query = query.only_if(f);
        }

        let stream = query.execute().await.map_err(super::lance_err)?;
        let batches = collect_batches(stream).await?;

        let mut results = Vec::new();
        for batch in &batches {
            let facts = record_batch_to_facts(batch)?;
            for (i, fact) in facts.into_iter().enumerate() {
                let score = read_score(batch, i);
                results.push(ScoredFact { fact, score });
            }
        }

        Ok(results)
    }

    async fn hybrid_search(
        &self,
        params: &HybridSearchParams<'_>,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        // LanceDB supports hybrid search when both nearest_to and full_text_search
        // are combined on a VectorQuery. It uses RRFReranker by default.
        let column_name = format!("vec_{}", params.dim_hint);
        let fts_query = FullTextSearchQuery::new(params.query_text.to_owned());

        let mut query = self
            .facts_table
            .query()
            .full_text_search(fts_query)
            .nearest_to(params.embedding)
            .map_err(super::lance_err)?
            .column(&column_name)
            .limit(params.limit);

        if let Some(f) = params.filter.to_lance_filter() {
            query = query.only_if(f);
        }

        let stream = match query.execute().await {
            Ok(s) => s,
            Err(_) => {
                // If hybrid search fails (e.g. no FTS index), fall back to
                // manual score fusion.
                return self.manual_hybrid_search(params).await;
            }
        };

        let batches = collect_batches(stream).await?;

        let mut results = Vec::new();
        for batch in &batches {
            let facts = record_batch_to_facts(batch)?;
            for (i, fact) in facts.into_iter().enumerate() {
                let score = read_relevance_score(batch, i);
                results.push(ScoredFact { fact, score });
            }
        }

        Ok(results)
    }

    // -- VFS path operations ------------------------------------------------

    async fn list_by_path(
        &self,
        parent_path: &str,
        ns: &NamespaceScope,
        workspace: &str,
    ) -> Result<Vec<PathEntry>, AlephError> {
        let ns_value = ns.to_namespace_value();
        let filter = if matches!(ns, NamespaceScope::Owner) {
            format!("parent_path = '{}' AND workspace = '{}'", parent_path, workspace)
        } else {
            format!(
                "parent_path = '{}' AND namespace = '{}' AND workspace = '{}'",
                parent_path, ns_value, workspace
            )
        };

        let facts = scan_facts(&self.facts_table, Some(&filter), None).await?;

        // Group by unique child paths. A child path is the fact's own `path`.
        // If multiple facts share the same path, they form one "directory entry"
        // (leaf with count > 1).
        let mut path_counts: HashMap<String, usize> = HashMap::new();
        for fact in &facts {
            *path_counts.entry(fact.path.clone()).or_insert(0) += 1;
        }

        let entries = path_counts
            .into_iter()
            .map(|(path, count)| PathEntry {
                path,
                is_leaf: true, // facts are always leaves
                child_count: count,
            })
            .collect();

        Ok(entries)
    }

    async fn get_by_path(
        &self,
        path: &str,
        ns: &NamespaceScope,
        workspace: &str,
    ) -> Result<Option<MemoryFact>, AlephError> {
        let ns_value = ns.to_namespace_value();
        let filter = if matches!(ns, NamespaceScope::Owner) {
            format!("path = '{}' AND workspace = '{}'", path, workspace)
        } else {
            format!("path = '{}' AND namespace = '{}' AND workspace = '{}'", path, ns_value, workspace)
        };

        let facts = scan_facts(&self.facts_table, Some(&filter), Some(1)).await?;
        Ok(facts.into_iter().next())
    }

    async fn get_facts_by_path_prefix(
        &self,
        path_prefix: &str,
        filter: &SearchFilter,
        limit: usize,
    ) -> Result<Vec<MemoryFact>, AlephError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let prefix_clause = format!("starts_with(path, '{}')", path_prefix);
        let scoped_filter = match filter.to_lance_filter() {
            Some(existing) => format!("{} AND {}", existing, prefix_clause),
            None => prefix_clause,
        };

        scan_facts(&self.facts_table, Some(scoped_filter.as_str()), Some(limit)).await
    }

    // -- Statistics & bulk --------------------------------------------------

    async fn count_facts(&self, filter: &SearchFilter) -> Result<usize, AlephError> {
        let facts = scan_facts(
            &self.facts_table,
            filter.to_lance_filter().as_deref(),
            None,
        )
        .await?;
        Ok(facts.len())
    }

    async fn get_facts_by_type(
        &self,
        fact_type: FactType,
        ns: &NamespaceScope,
        workspace: &str,
        limit: usize,
    ) -> Result<Vec<MemoryFact>, AlephError> {
        let ns_value = ns.to_namespace_value();
        let filter = if matches!(ns, NamespaceScope::Owner) {
            format!("fact_type = '{}' AND workspace = '{}'", fact_type.as_str(), workspace)
        } else {
            format!(
                "fact_type = '{}' AND namespace = '{}' AND workspace = '{}'",
                fact_type.as_str(),
                ns_value,
                workspace
            )
        };

        scan_facts(&self.facts_table, Some(&filter), Some(limit)).await
    }

    async fn get_all_facts(
        &self,
        include_invalid: bool,
    ) -> Result<Vec<MemoryFact>, AlephError> {
        let filter = if include_invalid {
            None
        } else {
            Some("is_valid = true")
        };

        scan_facts(&self.facts_table, filter, None).await
    }

    // -- Mutation helpers ---------------------------------------------------

    async fn invalidate_fact(&self, id: &str, reason: &str) -> Result<(), AlephError> {
        // Read-modify-write: fetch the fact, update fields, delete+insert.
        let existing = self.get_fact(id).await?;
        let mut fact = existing.ok_or_else(|| AlephError::NotFound(format!("Fact '{}'", id)))?;

        fact.is_valid = false;
        fact.invalidation_reason = Some(reason.to_string());
        fact.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        self.update_fact(&fact).await
    }

    async fn update_fact_content(
        &self,
        id: &str,
        new_content: &str,
    ) -> Result<(), AlephError> {
        let existing = self.get_fact(id).await?;
        let mut fact = existing.ok_or_else(|| AlephError::NotFound(format!("Fact '{}'", id)))?;

        fact.content = new_content.to_string();
        fact.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        self.update_fact(&fact).await
    }

    async fn find_similar_facts(
        &self,
        embedding: &[f32],
        dim_hint: u32,
        filter: &SearchFilter,
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        // Perform vector search with a generous limit, then filter by threshold.
        let all = self
            .vector_search(embedding, dim_hint, filter, limit * 2)
            .await?;

        let filtered: Vec<ScoredFact> = all
            .into_iter()
            .filter(|sf| sf.score >= threshold)
            .take(limit)
            .collect();

        Ok(filtered)
    }

    async fn apply_fact_decay(
        &self,
        decay_factor: f32,
        min_score: f32,
    ) -> Result<usize, AlephError> {
        // Fetch all valid facts.
        let facts = scan_facts(&self.facts_table, Some("is_valid = true"), None).await?;
        let mut affected = 0usize;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        for fact in &facts {
            // Read the current decay_score from the stored fact.
            // Since MemoryFact doesn't track decay_score directly, we
            // compute a new effective score: confidence * decay_factor.
            let new_confidence = fact.confidence * decay_factor;

            if new_confidence < min_score {
                // Invalidate the fact due to decay.
                let mut invalidated = fact.clone();
                invalidated.is_valid = false;
                invalidated.invalidation_reason = Some("decay_prune".to_string());
                invalidated.decay_invalidated_at = Some(now);
                invalidated.confidence = new_confidence;
                invalidated.updated_at = now;
                self.update_fact(&invalidated).await?;
                affected += 1;
            } else if (new_confidence - fact.confidence).abs() > f32::EPSILON {
                // Update confidence with decayed value.
                let mut updated = fact.clone();
                updated.confidence = new_confidence;
                updated.updated_at = now;
                self.update_fact(&updated).await?;
                affected += 1;
            }
        }

        Ok(affected)
    }

    async fn get_fact_stats(&self) -> Result<FactStats, AlephError> {
        let all_facts = scan_facts(&self.facts_table, None, None).await?;

        let total_facts = all_facts.len() as u64;
        let mut valid_facts = 0u64;
        let mut facts_by_type: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        let mut oldest: Option<i64> = None;
        let mut newest: Option<i64> = None;

        for fact in &all_facts {
            if fact.is_valid {
                valid_facts += 1;
            }

            *facts_by_type
                .entry(fact.fact_type.as_str().to_string())
                .or_insert(0) += 1;

            match oldest {
                Some(ts) if fact.created_at < ts => oldest = Some(fact.created_at),
                None => oldest = Some(fact.created_at),
                _ => {}
            }
            match newest {
                Some(ts) if fact.created_at > ts => newest = Some(fact.created_at),
                None => newest = Some(fact.created_at),
                _ => {}
            }
        }

        Ok(FactStats {
            total_facts,
            valid_facts,
            facts_by_type,
            oldest_fact_timestamp: oldest,
            newest_fact_timestamp: newest,
        })
    }

    async fn soft_delete_fact(&self, id: &str, reason: &str) -> Result<(), AlephError> {
        // Delegate to invalidate_fact — they are semantically identical.
        self.invalidate_fact(id, reason).await
    }
}

// ---------------------------------------------------------------------------
// Manual hybrid search fallback
// ---------------------------------------------------------------------------

impl LanceMemoryBackend {
    /// Fallback hybrid search via manual score fusion when native LanceDB
    /// hybrid search is unavailable (e.g. no FTS index).
    async fn manual_hybrid_search(
        &self,
        params: &HybridSearchParams<'_>,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        // Run vector search and text search independently.
        let vec_results = self
            .vector_search(params.embedding, params.dim_hint, params.filter, params.limit)
            .await
            .unwrap_or_default();

        let text_results = self
            .text_search(params.query_text, params.filter, params.limit)
            .await
            .unwrap_or_default();

        // Merge by fact ID, combining weighted scores.
        let mut merged: HashMap<String, (MemoryFact, f32)> = HashMap::new();

        for sf in vec_results {
            let entry = merged
                .entry(sf.fact.id.clone())
                .or_insert_with(|| (sf.fact.clone(), 0.0));
            entry.1 += sf.score * params.vector_weight;
        }

        for sf in text_results {
            let entry = merged
                .entry(sf.fact.id.clone())
                .or_insert_with(|| (sf.fact.clone(), 0.0));
            entry.1 += sf.score * params.text_weight;
        }

        let mut results: Vec<ScoredFact> = merged
            .into_values()
            .map(|(fact, score)| ScoredFact { fact, score })
            .collect();

        // Sort by score descending.
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(params.limit);

        Ok(results)
    }
}

// ============================================================================
// AuditStore implementation
// ============================================================================

#[allow(deprecated)]
#[async_trait]
impl AuditStore for LanceMemoryBackend {
    async fn insert_audit_entry(&self, _entry: &AuditEntry) -> Result<(), AlephError> {
        // TODO: Store audit entries in a dedicated LanceDB table.
        // The audit schema needs to be designed to serialize AuditAction/AuditActor
        // as strings plus a JSON details column. For now, this is a no-op.
        Ok(())
    }

    async fn get_audit_entries_for_fact(
        &self,
        _fact_id: &str,
    ) -> Result<Vec<AuditEntry>, AlephError> {
        // TODO: Query audit entries from a dedicated table filtered by fact_id.
        // For now, return an empty list.
        Ok(Vec::new())
    }

    async fn get_recent_audit_entries(
        &self,
        _limit: usize,
    ) -> Result<Vec<AuditEntry>, AlephError> {
        // TODO: Query the most recent audit entries ordered by created_at DESC.
        // For now, return an empty list.
        Ok(Vec::new())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    /// Helper: create a test LanceMemoryBackend in a temp directory.
    async fn create_test_backend() -> (tempfile::TempDir, LanceMemoryBackend) {
        let tmp = tempfile::tempdir().unwrap();
        let backend = LanceMemoryBackend::open_or_create(tmp.path())
            .await
            .unwrap();
        (tmp, backend)
    }

    /// Helper: create a test fact with optional embedding.
    fn make_test_fact(content: &str, fact_type: FactType, with_embedding: bool) -> MemoryFact {
        let mut fact = MemoryFact::new(
            content.to_string(),
            fact_type,
            vec!["mem-001".to_string()],
        );
        fact.confidence = 0.9;
        fact.content_hash = "hash123".to_string();
        fact.embedding_model = "test-model".to_string();
        if with_embedding {
            fact.embedding = Some(vec![0.1_f32; 1024]);
        }
        fact
    }

    #[tokio::test]
    async fn test_insert_and_get_fact() {
        let (_tmp, backend) = create_test_backend().await;
        let fact = make_test_fact("Test content", FactType::Learning, false);
        let fact_id = fact.id.clone();

        backend.insert_fact(&fact).await.unwrap();

        let retrieved = backend.get_fact(&fact_id).await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.content, "Test content");
        assert_eq!(retrieved.fact_type, FactType::Learning);
        assert_eq!(retrieved.id, fact_id);
    }

    #[tokio::test]
    async fn test_get_nonexistent_fact() {
        let (_tmp, backend) = create_test_backend().await;
        let result = backend.get_fact("nonexistent-id").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_delete_fact() {
        let (_tmp, backend) = create_test_backend().await;
        let fact = make_test_fact("To be deleted", FactType::Other, false);
        let fact_id = fact.id.clone();

        backend.insert_fact(&fact).await.unwrap();
        assert!(backend.get_fact(&fact_id).await.unwrap().is_some());

        backend.delete_fact(&fact_id).await.unwrap();
        assert!(backend.get_fact(&fact_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_update_fact() {
        let (_tmp, backend) = create_test_backend().await;
        let mut fact = make_test_fact("Original content", FactType::Preference, false);
        let fact_id = fact.id.clone();

        backend.insert_fact(&fact).await.unwrap();

        fact.content = "Updated content".to_string();
        backend.update_fact(&fact).await.unwrap();

        let retrieved = backend.get_fact(&fact_id).await.unwrap().unwrap();
        assert_eq!(retrieved.content, "Updated content");
    }

    #[tokio::test]
    async fn test_batch_insert() {
        let (_tmp, backend) = create_test_backend().await;
        let facts = vec![
            make_test_fact("Fact A", FactType::Learning, false),
            make_test_fact("Fact B", FactType::Preference, false),
            make_test_fact("Fact C", FactType::Project, false),
        ];

        backend.batch_insert_facts(&facts).await.unwrap();

        for fact in &facts {
            let retrieved = backend.get_fact(&fact.id).await.unwrap();
            assert!(retrieved.is_some());
        }
    }

    #[tokio::test]
    async fn test_batch_insert_empty() {
        let (_tmp, backend) = create_test_backend().await;
        backend.batch_insert_facts(&[]).await.unwrap();
    }

    #[tokio::test]
    async fn test_invalidate_fact() {
        let (_tmp, backend) = create_test_backend().await;
        let fact = make_test_fact("Valid fact", FactType::Learning, false);
        let fact_id = fact.id.clone();

        backend.insert_fact(&fact).await.unwrap();
        backend
            .invalidate_fact(&fact_id, "superseded")
            .await
            .unwrap();

        let retrieved = backend.get_fact(&fact_id).await.unwrap().unwrap();
        assert!(!retrieved.is_valid);
        assert_eq!(
            retrieved.invalidation_reason,
            Some("superseded".to_string())
        );
    }

    #[tokio::test]
    async fn test_update_fact_content() {
        let (_tmp, backend) = create_test_backend().await;
        let fact = make_test_fact("Old content", FactType::Personal, false);
        let fact_id = fact.id.clone();

        backend.insert_fact(&fact).await.unwrap();
        backend
            .update_fact_content(&fact_id, "New content")
            .await
            .unwrap();

        let retrieved = backend.get_fact(&fact_id).await.unwrap().unwrap();
        assert_eq!(retrieved.content, "New content");
    }

    #[tokio::test]
    async fn test_get_all_facts() {
        let (_tmp, backend) = create_test_backend().await;
        let fact_valid = make_test_fact("Valid", FactType::Learning, false);
        let mut fact_invalid = make_test_fact("Invalid", FactType::Other, false);
        fact_invalid.is_valid = false;
        fact_invalid.invalidation_reason = Some("old".to_string());

        backend.insert_fact(&fact_valid).await.unwrap();
        backend.insert_fact(&fact_invalid).await.unwrap();

        // Without invalid
        let valid_only = backend.get_all_facts(false).await.unwrap();
        assert_eq!(valid_only.len(), 1);
        assert_eq!(valid_only[0].content, "Valid");

        // With invalid
        let all = backend.get_all_facts(true).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_count_facts() {
        let (_tmp, backend) = create_test_backend().await;
        let fact1 = make_test_fact("Fact 1", FactType::Learning, false);
        let fact2 = make_test_fact("Fact 2", FactType::Preference, false);
        let fact3 = make_test_fact("Fact 3", FactType::Learning, false);

        backend
            .batch_insert_facts(&[fact1, fact2, fact3])
            .await
            .unwrap();

        let total = backend.count_facts(&SearchFilter::new()).await.unwrap();
        assert_eq!(total, 3);

        let learning_only = backend
            .count_facts(&SearchFilter::new().with_fact_type(FactType::Learning))
            .await
            .unwrap();
        assert_eq!(learning_only, 2);
    }

    #[tokio::test]
    async fn test_get_facts_by_type() {
        let (_tmp, backend) = create_test_backend().await;
        let fact1 = make_test_fact("Learning 1", FactType::Learning, false);
        let fact2 = make_test_fact("Preference 1", FactType::Preference, false);
        let fact3 = make_test_fact("Learning 2", FactType::Learning, false);

        backend
            .batch_insert_facts(&[fact1, fact2, fact3])
            .await
            .unwrap();

        let learning = backend
            .get_facts_by_type(FactType::Learning, &NamespaceScope::Owner, "default", 10)
            .await
            .unwrap();
        assert_eq!(learning.len(), 2);

        let prefs = backend
            .get_facts_by_type(FactType::Preference, &NamespaceScope::Owner, "default", 10)
            .await
            .unwrap();
        assert_eq!(prefs.len(), 1);
    }

    #[tokio::test]
    async fn test_vector_search() {
        let (_tmp, backend) = create_test_backend().await;

        // Insert facts WITH embeddings
        let fact1 = make_test_fact("Rust programming", FactType::Learning, true);
        // fact1 has embedding = [0.1; 1024]

        let mut fact2 = make_test_fact("Python scripting", FactType::Learning, true);
        fact2.embedding = Some(vec![0.9_f32; 1024]);

        backend
            .batch_insert_facts(&[fact1.clone(), fact2.clone()])
            .await
            .unwrap();

        // Search with a vector close to fact1's embedding
        let query_vec = vec![0.1_f32; 1024];
        let results = backend
            .vector_search(&query_vec, 1024, &SearchFilter::new(), 10)
            .await
            .unwrap();

        assert!(!results.is_empty());
        // The result closest to [0.1; 1024] should be fact1
        assert_eq!(results[0].fact.content, "Rust programming");
        assert!(results[0].score > 0.0);
    }

    #[tokio::test]
    async fn test_find_similar_facts() {
        let (_tmp, backend) = create_test_backend().await;

        let fact1 = make_test_fact("Similar fact", FactType::Learning, true);
        let mut fact2 = make_test_fact("Different fact", FactType::Learning, true);
        fact2.embedding = Some(vec![0.9_f32; 1024]);

        backend
            .batch_insert_facts(&[fact1.clone(), fact2.clone()])
            .await
            .unwrap();

        let query_vec = vec![0.1_f32; 1024];
        let results = backend
            .find_similar_facts(&query_vec, 1024, &SearchFilter::new(), 0.5, 10)
            .await
            .unwrap();

        // At least the very similar fact should be returned
        assert!(!results.is_empty());
        // All returned facts should meet the threshold
        for sf in &results {
            assert!(sf.score >= 0.5);
        }
    }

    #[tokio::test]
    async fn test_list_by_path() {
        let (_tmp, backend) = create_test_backend().await;

        let mut fact1 = make_test_fact("Pref 1", FactType::Preference, false);
        fact1.path = "aleph://user/preferences/coding/".to_string();
        fact1.parent_path = "aleph://user/preferences/".to_string();

        let mut fact2 = make_test_fact("Pref 2", FactType::Preference, false);
        fact2.path = "aleph://user/preferences/ui/".to_string();
        fact2.parent_path = "aleph://user/preferences/".to_string();

        let mut fact3 = make_test_fact("Plan", FactType::Plan, false);
        fact3.path = "aleph://user/plans/trip/".to_string();
        fact3.parent_path = "aleph://user/plans/".to_string();

        backend
            .batch_insert_facts(&[fact1, fact2, fact3])
            .await
            .unwrap();

        let entries = backend
            .list_by_path("aleph://user/preferences/", &NamespaceScope::Owner, "default")
            .await
            .unwrap();

        assert_eq!(entries.len(), 2);
        let paths: HashSet<String> = entries.iter().map(|e| e.path.clone()).collect();
        assert!(paths.contains("aleph://user/preferences/coding/"));
        assert!(paths.contains("aleph://user/preferences/ui/"));
    }

    #[tokio::test]
    async fn test_get_by_path() {
        let (_tmp, backend) = create_test_backend().await;

        let mut fact = make_test_fact("Coding preference", FactType::Preference, false);
        fact.path = "aleph://user/preferences/coding/rust".to_string();
        fact.parent_path = "aleph://user/preferences/coding/".to_string();

        backend.insert_fact(&fact).await.unwrap();

        let result = backend
            .get_by_path(
                "aleph://user/preferences/coding/rust",
                &NamespaceScope::Owner,
                "default",
            )
            .await
            .unwrap();

        assert!(result.is_some());
        assert_eq!(result.unwrap().content, "Coding preference");

        // Non-existent path
        let missing = backend
            .get_by_path("aleph://nonexistent/path", &NamespaceScope::Owner, "default")
            .await
            .unwrap();
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn test_get_facts_by_path_prefix() {
        let (_tmp, backend) = create_test_backend().await;

        let mut fact_a = make_test_fact("A", FactType::Preference, false);
        fact_a.path = "aleph://user/preferences/coding/rust".to_string();
        fact_a.parent_path = "aleph://user/preferences/coding/".to_string();

        let mut fact_b = make_test_fact("B", FactType::Preference, false);
        fact_b.path = "aleph://user/preferences/coding/vim".to_string();
        fact_b.parent_path = "aleph://user/preferences/coding/".to_string();

        let mut fact_c = make_test_fact("C", FactType::Preference, false);
        fact_c.path = "aleph://user/preferences/ui/theme".to_string();
        fact_c.parent_path = "aleph://user/preferences/ui/".to_string();

        backend
            .batch_insert_facts(&[fact_a, fact_b, fact_c])
            .await
            .unwrap();

        let results = backend
            .get_facts_by_path_prefix(
                "aleph://user/preferences/coding/",
                &SearchFilter::new().with_fact_type(FactType::Preference),
                10,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|f| f.path.starts_with("aleph://user/preferences/coding/")));
    }

    #[tokio::test]
    async fn test_invalidate_nonexistent_fact() {
        let (_tmp, backend) = create_test_backend().await;
        let result = backend
            .invalidate_fact("nonexistent", "reason")
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_update_content_nonexistent_fact() {
        let (_tmp, backend) = create_test_backend().await;
        let result = backend
            .update_fact_content("nonexistent", "new content")
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_search_with_filter() {
        let (_tmp, backend) = create_test_backend().await;

        let fact1 = make_test_fact("Learning Rust", FactType::Learning, true);
        let fact2 = make_test_fact("Preference coding", FactType::Preference, true);

        backend
            .batch_insert_facts(&[fact1, fact2])
            .await
            .unwrap();

        // Search with filter for Learning only
        let query_vec = vec![0.1_f32; 1024];
        let results = backend
            .vector_search(
                &query_vec,
                1024,
                &SearchFilter::new().with_fact_type(FactType::Learning),
                10,
            )
            .await
            .unwrap();

        // Only the Learning fact should be returned
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fact.fact_type, FactType::Learning);
    }
}
