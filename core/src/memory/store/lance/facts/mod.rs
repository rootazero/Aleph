//! MemoryStore trait implementation for LanceMemoryBackend.
//!
//! Provides all Fact CRUD operations (insert, get, update, delete, batch_insert),
//! multi-modal search (vector, text, hybrid), VFS path queries, statistics,
//! and mutation helpers against the LanceDB `facts` table.

mod helpers;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use async_trait::async_trait;
use lance_index::scalar::FullTextSearchQuery;
use lancedb::query::{ExecutableQuery, QueryBase, Select};

use crate::error::AlephError;
use crate::memory::context::{FactStats, FactType, MemoryFact};
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::types::{escape_sql_string, ScoredFact, SearchFilter};
use crate::memory::store::{HybridSearchParams, MemoryStore, PathEntry};

use super::arrow_convert::facts_to_record_batch;
use super::LanceMemoryBackend;

use helpers::{
    add_batch, collect_batches, distance_to_similarity, read_distance, read_relevance_score,
    read_score, scan_facts,
};

use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

static FIRST_WRITE_LOGGED: AtomicBool = AtomicBool::new(false);
static FIRST_READ_LOGGED: AtomicBool = AtomicBool::new(false);

// ============================================================================
// MemoryStore implementation
// ============================================================================

#[async_trait]
impl MemoryStore for LanceMemoryBackend {
    // -- CRUD ---------------------------------------------------------------

    async fn insert_fact(&self, fact: &MemoryFact) -> Result<(), AlephError> {
        if !FIRST_WRITE_LOGGED.swap(true, AtomicOrdering::Relaxed) {
            tracing::info!(
                subsystem = "memory",
                event = "first_write",
                table = "facts",
                fact_id = %fact.id,
                "memory store received first fact write"
            );
        }
        let batch = facts_to_record_batch(std::slice::from_ref(fact))?;
        add_batch(&self.facts_table, batch).await
    }

    async fn get_fact(&self, id: &str) -> Result<Option<MemoryFact>, AlephError> {
        let filter = format!("id = '{}'", escape_sql_string(id));
        let facts = scan_facts(&self.facts_table, Some(&filter), Some(1)).await?;
        Ok(facts.into_iter().next())
    }

    async fn update_fact(&self, fact: &MemoryFact) -> Result<(), AlephError> {
        // LanceDB lacks native upsert. We use delete-then-insert with a safety net:
        // if the insert fails after delete, we retry once to avoid losing the fact.
        self.delete_fact(&fact.id).await?;
        match self.insert_fact(fact).await {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::warn!(id = %fact.id, error = %e, "Fact insert failed after delete, retrying");
                self.insert_fact(fact).await
            }
        }
    }

    async fn delete_fact(&self, id: &str) -> Result<(), AlephError> {
        self.facts_table
            .delete(&format!("id = '{}'", escape_sql_string(id)))
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
        if !FIRST_READ_LOGGED.swap(true, AtomicOrdering::Relaxed) {
            tracing::info!(
                subsystem = "memory",
                event = "first_read",
                table = "facts",
                dim = dim_hint,
                limit = limit,
                "memory store received first vector search"
            );
        }
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
            let facts = super::arrow_convert::record_batch_to_facts(batch)?;
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
            let facts = super::arrow_convert::record_batch_to_facts(batch)?;
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
            let facts = super::arrow_convert::record_batch_to_facts(batch)?;
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
        let pp_safe = escape_sql_string(parent_path);
        let ws_safe = escape_sql_string(workspace);
        let filter = if matches!(ns, NamespaceScope::Owner) {
            format!("parent_path = '{}' AND agent = '{}'", pp_safe, ws_safe)
        } else {
            let ns_safe = escape_sql_string(&ns_value);
            format!(
                "parent_path = '{}' AND namespace = '{}' AND agent = '{}'",
                pp_safe, ns_safe, ws_safe
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
        let path_safe = escape_sql_string(path);
        let ws_safe = escape_sql_string(workspace);
        let filter = if matches!(ns, NamespaceScope::Owner) {
            format!("path = '{}' AND agent = '{}'", path_safe, ws_safe)
        } else {
            let ns_safe = escape_sql_string(&ns_value);
            format!("path = '{}' AND namespace = '{}' AND agent = '{}'", path_safe, ns_safe, ws_safe)
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

        let prefix_clause = format!("starts_with(path, '{}')", escape_sql_string(path_prefix));
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
        let ws_safe = escape_sql_string(workspace);
        let filter = if matches!(ns, NamespaceScope::Owner) {
            format!("fact_type = '{}' AND agent = '{}'", escape_sql_string(fact_type.as_str()), ws_safe)
        } else {
            let ns_safe = escape_sql_string(&ns_value);
            format!(
                "fact_type = '{}' AND namespace = '{}' AND agent = '{}'",
                escape_sql_string(fact_type.as_str()),
                ns_safe,
                ws_safe
            )
        };

        scan_facts(&self.facts_table, Some(&filter), Some(limit)).await
    }

    async fn get_all_facts(
        &self,
        include_invalid: bool,
        workspace: Option<&str>,
    ) -> Result<Vec<MemoryFact>, AlephError> {
        let mut clauses = Vec::new();
        if !include_invalid {
            clauses.push("is_valid = true".to_string());
        }
        if let Some(ws) = workspace {
            clauses.push(format!(
                "agent = '{}'",
                escape_sql_string(ws)
            ));
        }
        let filter = if clauses.is_empty() {
            None
        } else {
            Some(clauses.join(" AND "))
        };

        scan_facts(&self.facts_table, filter.as_deref(), None).await
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
            .unwrap_or_default()
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
            .unwrap_or_default()
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
        half_life_days: f32,
        min_score: f32,
    ) -> Result<usize, AlephError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        // Collect ALL facts first, then apply mutations.
        // Mutating during offset-based pagination causes facts to be
        // skipped or double-processed because delete+insert changes row ordering.
        let all_facts = scan_facts(
            &self.facts_table,
            Some("is_valid = true"),
            None,
        )
        .await?;

        let mut affected = 0usize;

        for fact in &all_facts {
            // Compute days since last access (or creation if never accessed).
            let last_access = fact.last_accessed_at.unwrap_or(fact.updated_at);
            let days_since_access = (now - last_access).max(0) as f64 / 86400.0;

            // Ebbinghaus exponential decay: exp(-t * ln(2) / half_life)
            let decay =
                (-(days_since_access) * (2.0_f64.ln()) / half_life_days as f64).exp() as f32;
            let new_confidence = fact.confidence * decay;

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
