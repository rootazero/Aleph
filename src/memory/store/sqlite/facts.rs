//! MemoryStore trait implementation for SqliteMemoryBackend.
//!
//! Provides fact CRUD, vector/text/hybrid search, VFS path operations,
//! statistics, and decay against the SQLite `facts` table with sqlite-vec
//! for vector search and FTS5 for full-text search.

use std::collections::HashMap;

use async_trait::async_trait;
use rusqlite::OptionalExtension;

use crate::error::AlephError;
use crate::memory::context::{
    FactSource, FactSpecificity, FactStats, FactType, MemoryCategory, MemoryFact, MemoryLayer,
    MemoryScope, MemoryTier, TemporalScope,
};
use crate::memory::namespace::NamespaceScope;
use crate::memory::store::types::{ScoredFact, SearchFilter};
use crate::memory::store::{HybridSearchParams, MemoryStore, PathEntry};

use super::vec;
use super::SqliteMemoryBackend;

// ---------------------------------------------------------------------------
// Helper: map a rusqlite::Row to MemoryFact
// ---------------------------------------------------------------------------

/// Map a SQLite row to a MemoryFact.
///
/// Expects columns in the order produced by `SELECT * FROM facts` (by schema order).
/// Embeddings are stored separately in vec tables and are NOT returned here.
fn row_to_fact(row: &rusqlite::Row) -> rusqlite::Result<MemoryFact> {
    row_to_fact_pub(row)
}

/// Public variant of `row_to_fact` for use by `SqliteMemoryBackend` methods in `mod.rs`.
pub(super) fn row_to_fact_pub(row: &rusqlite::Row) -> rusqlite::Result<MemoryFact> {
    let source_ids_json: Option<String> = row.get("source_memory_ids")?;

    let source_memory_ids: Vec<String> = source_ids_json
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let fact_type_str: String = row.get("fact_type")?;
    let fact_source_str: String = row.get("fact_source")?;
    let specificity_str: Option<String> = row.get("specificity")?;
    let temporal_scope_str: Option<String> = row.get("temporal_scope")?;
    let layer_str: Option<String> = row.get("layer")?;
    let category_str: Option<String> = row.get("category")?;
    let tier_str: Option<String> = row.get("tier")?;
    let scope_str: Option<String> = row.get("scope")?;
    let is_valid_int: i32 = row.get("is_valid")?;

    Ok(MemoryFact {
        id: row.get("id")?,
        content: row.get("content")?,
        fact_type: FactType::from_str_or_other(&fact_type_str),
        embedding: None, // stored in vec tables, not returned here
        source_memory_ids,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        confidence: row.get::<_, Option<f64>>("confidence")?.unwrap_or(1.0) as f32,
        is_valid: is_valid_int != 0,
        invalidation_reason: row.get("invalidation_reason")?,
        decay_invalidated_at: row.get("decay_invalidated_at")?,
        specificity: specificity_str
            .map(|s| FactSpecificity::from_str_or_default(&s))
            .unwrap_or_default(),
        temporal_scope: temporal_scope_str
            .map(|s| TemporalScope::from_str_or_default(&s))
            .unwrap_or_default(),
        namespace: row.get::<_, Option<String>>("namespace")?.unwrap_or_else(|| "owner".into()),
        agent: row.get::<_, Option<String>>("agent")?.unwrap_or_else(|| "default".into()),
        similarity_score: None,
        path: row.get::<_, Option<String>>("path")?.unwrap_or_default(),
        layer: layer_str
            .map(|s| MemoryLayer::from_str_or_default(&s))
            .unwrap_or_default(),
        category: category_str
            .map(|s| MemoryCategory::from_str_or_default(&s))
            .unwrap_or_default(),
        fact_source: FactSource::from_str_or_default(&fact_source_str),
        content_hash: row.get::<_, Option<String>>("content_hash")?.unwrap_or_default(),
        parent_path: row.get::<_, Option<String>>("parent_path")?.unwrap_or_default(),
        embedding_model: row.get::<_, Option<String>>("embedding_model")?.unwrap_or_default(),
        tier: tier_str
            .map(|s| MemoryTier::from_str_or_default(&s))
            .unwrap_or_default(),
        scope: scope_str
            .map(|s| MemoryScope::from_str_or_default(&s))
            .unwrap_or_default(),
        persona_id: row.get("persona_id")?,
        strength: row.get::<_, Option<f64>>("strength")?.unwrap_or(1.0) as f32,
        access_count: row.get::<_, Option<i32>>("access_count")?.unwrap_or(0) as u32,
        last_accessed_at: row.get("last_accessed_at")?,
        valid_from: row.get("valid_from")?,
        valid_to: row.get("valid_to")?,
    })
}

// ---------------------------------------------------------------------------
// Helper: build WHERE clause from SearchFilter for SQLite
// ---------------------------------------------------------------------------

/// Build a WHERE clause and parameter values from a SearchFilter.
///
/// Returns (clause, params) where clause starts with " WHERE ..." or is empty
/// if no filters are set. Parameters use positional `?N` placeholders.
///
/// `param_offset` specifies how many parameters have already been bound before
/// this clause (e.g. 1 if `?1` is already used for a rowid or MATCH param).
fn build_filter_clause(
    filter: &SearchFilter,
    param_offset: usize,
) -> (String, Vec<Box<dyn rusqlite::types::ToSql>>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(ref ns) = filter.namespace {
        let val = ns.to_namespace_value();
        if matches!(ns, NamespaceScope::Owner) {
            // Owner = no namespace filter (sees everything)
        } else {
            params.push(Box::new(val));
            clauses.push(format!("namespace = ?{}", param_offset + params.len()));
        }
    }

    if let Some(ref agent_filter) = filter.agent_filter {
        use crate::gateway::agent_env::AgentEnvFilter;
        match agent_filter {
            AgentEnvFilter::All => {} // no filter
            AgentEnvFilter::Single(id) => {
                params.push(Box::new(id.clone()));
                clauses.push(format!("agent = ?{}", param_offset + params.len()));
            }
            AgentEnvFilter::Multiple(ids) => {
                if ids.is_empty() {
                    clauses.push("1=0".to_string());
                } else {
                    let placeholders: Vec<String> = ids
                        .iter()
                        .map(|id| {
                            params.push(Box::new(id.clone()));
                            format!("?{}", param_offset + params.len())
                        })
                        .collect();
                    clauses.push(format!("agent IN ({})", placeholders.join(", ")));
                }
            }
        }
    }

    if let Some(ref ft) = filter.fact_type {
        params.push(Box::new(ft.as_str().to_string()));
        clauses.push(format!("fact_type = ?{}", param_offset + params.len()));
    }

    if let Some(layer) = filter.layer {
        params.push(Box::new(layer.as_str().to_string()));
        clauses.push(format!("layer = ?{}", param_offset + params.len()));
    }

    if let Some(category) = filter.category {
        params.push(Box::new(category.as_str().to_string()));
        clauses.push(format!("category = ?{}", param_offset + params.len()));
    }

    if let Some(valid) = filter.is_valid {
        params.push(Box::new(if valid { 1i32 } else { 0i32 }));
        clauses.push(format!("is_valid = ?{}", param_offset + params.len()));
    }

    if let Some(ref prefix) = filter.path_prefix {
        params.push(Box::new(format!("{}%", prefix)));
        clauses.push(format!("path LIKE ?{}", param_offset + params.len()));
    }

    if let Some(min_conf) = filter.min_confidence {
        params.push(Box::new(min_conf as f64));
        clauses.push(format!("confidence >= ?{}", param_offset + params.len()));
    }

    if let Some(ts) = filter.created_after {
        params.push(Box::new(ts));
        clauses.push(format!("created_at >= ?{}", param_offset + params.len()));
    }

    if let Some(ts) = filter.created_before {
        params.push(Box::new(ts));
        clauses.push(format!("created_at <= ?{}", param_offset + params.len()));
    }

    if let Some(ref tier) = filter.tier {
        params.push(Box::new(tier.as_str().to_string()));
        clauses.push(format!("tier = ?{}", param_offset + params.len()));
    }

    if let Some(ref scope) = filter.scope {
        params.push(Box::new(scope.as_str().to_string()));
        clauses.push(format!("scope = ?{}", param_offset + params.len()));
    }

    if let Some(ref persona_id) = filter.persona_id {
        params.push(Box::new(persona_id.clone()));
        clauses.push(format!("persona_id = ?{}", param_offset + params.len()));
    }

    if clauses.is_empty() {
        (String::new(), params)
    } else {
        (format!(" WHERE {}", clauses.join(" AND ")), params)
    }
}

// ---------------------------------------------------------------------------
// Helper: insert a single fact into the facts table + vec + FTS5
// ---------------------------------------------------------------------------

fn insert_fact_inner(
    conn: &rusqlite::Connection,
    fact: &MemoryFact,
) -> Result<(), AlephError> {
    // Content scanner check
    if let crate::memory::content_scanner::ScanVerdict::Rejected { reason, pattern } =
        crate::memory::content_scanner::scan_content(&fact.content)
    {
        tracing::warn!(fact_id = %fact.id, pattern = pattern, "Memory content rejected by scanner: {reason}");
        return Err(AlephError::Validation(format!(
            "Memory content rejected: {reason}"
        )));
    }

    let tags_json = serde_json::to_string(&Vec::<String>::new()).unwrap_or_else(|_| "[]".into());
    let source_ids_json =
        serde_json::to_string(&fact.source_memory_ids).unwrap_or_else(|_| "[]".into());

    conn.execute(
        "INSERT INTO facts (
            id, content, fact_type, fact_source, specificity, temporal_scope,
            layer, category, path, parent_path, namespace, agent,
            tags, source_memory_ids, content_hash, confidence, decay_score,
            is_valid, invalidation_reason, embedding_model,
            created_at, updated_at, decay_invalidated_at, version,
            tier, scope, persona_id, strength, access_count, last_accessed_at,
            valid_from, valid_to
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6,
            ?7, ?8, ?9, ?10, ?11, ?12,
            ?13, ?14, ?15, ?16, ?17,
            ?18, ?19, ?20,
            ?21, ?22, ?23, ?24,
            ?25, ?26, ?27, ?28, ?29, ?30,
            ?31, ?32
        )",
        rusqlite::params![
            fact.id,
            fact.content,
            fact.fact_type.as_str(),
            fact.fact_source.as_str(),
            fact.specificity.as_str(),
            fact.temporal_scope.as_str(),
            fact.layer.as_str(),
            fact.category.as_str(),
            fact.path,
            fact.parent_path,
            fact.namespace,
            fact.agent,
            tags_json,
            source_ids_json,
            fact.content_hash,
            fact.confidence as f64,
            fact.strength as f64, // decay_score maps to strength
            fact.is_valid as i32,
            fact.invalidation_reason,
            fact.embedding_model,
            fact.created_at,
            fact.updated_at,
            fact.decay_invalidated_at,
            1i32, // version
            fact.tier.as_str(),
            fact.scope.as_str(),
            fact.persona_id,
            fact.strength as f64,
            fact.access_count as i32,
            fact.last_accessed_at,
            fact.valid_from,
            fact.valid_to,
        ],
    )
    .map_err(|e| AlephError::config(format!("Failed to insert fact {}: {e}", fact.id)))?;

    let rowid = conn.last_insert_rowid();

    // Insert into FTS5 index
    conn.execute(
        "INSERT INTO facts_fts(rowid, content) VALUES (?1, ?2)",
        rusqlite::params![rowid, fact.content],
    )
    .map_err(|e| AlephError::config(format!("Failed to insert FTS5 entry: {e}")))?;

    // Insert embedding into vec table if present
    if let Some(ref emb) = fact.embedding {
        let dim = emb.len() as u32;
        // Only insert for supported dimensions
        if matches!(dim, 768 | 1024 | 1536) {
            vec::insert_vec(conn, rowid, emb, dim)?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helper: delete a fact by id (internal, handles vec + FTS cleanup)
// ---------------------------------------------------------------------------

fn delete_fact_inner(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<(), AlephError> {
    // Get rowid and content for FTS cleanup
    let mut stmt = conn
        .prepare("SELECT rowid, content FROM facts WHERE id = ?1")
        .map_err(|e| AlephError::config(format!("Failed to prepare delete query: {e}")))?;

    let row_data: Option<(i64, String)> = stmt
        .query_row(rusqlite::params![id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .optional()
        .map_err(|e| AlephError::config(format!("Failed to get fact for delete: {e}")))?;

    let Some((rowid, content)) = row_data else {
        return Ok(()); // fact doesn't exist, nothing to delete
    };

    // Delete from all vec tables
    vec::delete_vec_all_dims(conn, rowid)?;

    // Delete from FTS5 (content-sync deletion)
    conn.execute(
        "INSERT INTO facts_fts(facts_fts, rowid, content) VALUES ('delete', ?1, ?2)",
        rusqlite::params![rowid, content],
    )
    .map_err(|e| AlephError::config(format!("Failed to delete FTS5 entry: {e}")))?;

    // Delete from facts table
    conn.execute("DELETE FROM facts WHERE id = ?1", rusqlite::params![id])
        .map_err(|e| AlephError::config(format!("Failed to delete fact {id}: {e}")))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Helper: current Unix timestamp
// ---------------------------------------------------------------------------

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// ============================================================================
// MemoryStore implementation
// ============================================================================

#[async_trait]
impl MemoryStore for SqliteMemoryBackend {
    // -- CRUD ---------------------------------------------------------------

    async fn insert_fact(&self, fact: &MemoryFact) -> Result<(), AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;
        insert_fact_inner(&conn, fact)
    }

    async fn get_fact(&self, id: &str) -> Result<Option<MemoryFact>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let mut stmt = conn
            .prepare("SELECT * FROM facts WHERE id = ?1")
            .map_err(|e| AlephError::config(format!("Failed to prepare get_fact: {e}")))?;

        let result = stmt
            .query_row(rusqlite::params![id], row_to_fact)
            .optional()
            .map_err(|e| AlephError::config(format!("Failed to get fact {id}: {e}")))?;

        Ok(result)
    }

    async fn update_fact(&self, fact: &MemoryFact) -> Result<(), AlephError> {
        // Validate content before acquiring lock to avoid holding it unnecessarily
        if let crate::memory::content_scanner::ScanVerdict::Rejected { reason, pattern } =
            crate::memory::content_scanner::scan_content(&fact.content)
        {
            tracing::warn!(fact_id = %fact.id, pattern = pattern, "Memory content rejected by scanner on update: {reason}");
            return Err(AlephError::Validation(format!(
                "Memory content rejected: {reason}"
            )));
        }

        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        // Wrap delete+insert in a transaction to prevent data loss if insert fails
        conn.execute_batch("BEGIN TRANSACTION")
            .map_err(|e| AlephError::config(format!("Failed to begin update transaction: {e}")))?;

        if let Err(e) = delete_fact_inner(&conn, &fact.id)
            .and_then(|_| insert_fact_inner(&conn, fact))
        {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(e);
        }

        conn.execute_batch("COMMIT")
            .map_err(|e| AlephError::config(format!("Failed to commit update transaction: {e}")))?;

        Ok(())
    }

    async fn delete_fact(&self, id: &str) -> Result<(), AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;
        delete_fact_inner(&conn, id)
    }

    async fn batch_insert_facts(&self, facts: &[MemoryFact]) -> Result<(), AlephError> {
        if facts.is_empty() {
            return Ok(());
        }

        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        conn.execute_batch("BEGIN TRANSACTION")
            .map_err(|e| AlephError::config(format!("Failed to begin transaction: {e}")))?;

        for fact in facts {
            if let Err(e) = insert_fact_inner(&conn, fact) {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }

        conn.execute_batch("COMMIT")
            .map_err(|e| AlephError::config(format!("Failed to commit transaction: {e}")))?;

        Ok(())
    }

    // -- Search -------------------------------------------------------------

    async fn vector_search(
        &self,
        embedding: &[f32],
        dim_hint: u32,
        filter: &SearchFilter,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        // Overshoot k to account for post-filtering
        let k = limit.saturating_mul(3).max(limit);
        let knn_results = vec::knn_search(&conn, embedding, dim_hint, k)?;

        if knn_results.is_empty() {
            return Ok(Vec::new());
        }

        // Build a distance lookup from KNN results
        let distance_map: HashMap<i64, f64> = knn_results.iter().copied().collect();
        let rowids: Vec<i64> = knn_results.iter().map(|(r, _)| *r).collect();

        // Batch query: fetch all matching facts in one SQL statement
        let rowid_placeholders: Vec<String> =
            (1..=rowids.len()).map(|i| format!("?{i}")).collect();

        let (filter_clause, filter_params) = build_filter_clause(filter, rowids.len());

        let sql = format!(
            "SELECT rowid, * FROM facts WHERE rowid IN ({}){}",
            rowid_placeholders.join(", "),
            if filter_clause.is_empty() {
                String::new()
            } else {
                format!(" AND {}", &filter_clause[7..]) // skip " WHERE "
            }
        );

        let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = rowids
            .iter()
            .map(|r| Box::new(*r) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        for p in &filter_params {
            all_params.push(clone_tosql_param(p));
        }

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            all_params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AlephError::config(format!("vec search query failed: {e}")))?;

        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                let rowid: i64 = row.get::<_, i64>(0)?; // first column is explicit rowid
                let fact = row_to_fact(row)?;
                Ok((fact, rowid))
            })
            .map_err(|e| AlephError::config(format!("vec search batch query failed: {e}")))?;

        let mut results = Vec::with_capacity(limit);
        for row in rows {
            let (fact, rowid) = row.map_err(|e| {
                AlephError::config(format!("vec search row read failed: {e}"))
            })?;
            let distance = distance_map.get(&rowid).copied().unwrap_or(1.0);
            let score = 1.0 / (1.0 + distance);
            results.push(ScoredFact {
                fact,
                score: score as f32,
            });
        }

        // Sort by score descending (KNN order may not be preserved after IN query)
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit);

        Ok(results)
    }

    async fn text_search(
        &self,
        query: &str,
        filter: &SearchFilter,
        limit: usize,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let (filter_clause, filter_params) = build_filter_clause(filter, 1);

        // FTS5 query: join facts_fts with facts to get full records
        let sql = format!(
            "SELECT f.*, fts.rank \
             FROM facts_fts fts \
             JOIN facts f ON f.rowid = fts.rowid \
             WHERE facts_fts MATCH ?1{} \
             ORDER BY fts.rank \
             LIMIT ?{}",
            if filter_clause.is_empty() {
                String::new()
            } else {
                format!(" AND {}", &filter_clause[7..]) // skip " WHERE "
            },
            filter_params.len() + 2, // +1 for MATCH, +1 for LIMIT
        );

        let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(query.to_string())];
        for p in &filter_params {
            all_params.push(clone_tosql_param(p));
        }
        all_params.push(Box::new(limit as i64));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            all_params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AlephError::config(format!("text search query failed: {e}")))?;

        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                let fact = row_to_fact(row)?;
                let rank: f64 = row.get("rank")?;
                Ok((fact, rank))
            })
            .map_err(|e| AlephError::config(format!("text search failed: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            let (fact, rank) = row.map_err(|e| {
                AlephError::config(format!("text search row read failed: {e}"))
            })?;
            // FTS5 rank is negative (more negative = better match), normalize to [0, 1]
            let score = 1.0 / (1.0 + rank.abs());
            results.push(ScoredFact {
                fact,
                score: score as f32,
            });
        }

        Ok(results)
    }

    async fn hybrid_search(
        &self,
        params: &HybridSearchParams<'_>,
    ) -> Result<Vec<ScoredFact>, AlephError> {
        // Run vector and text search independently, then merge scores
        let vec_results = self
            .vector_search(params.embedding, params.dim_hint, params.filter, params.limit)
            .await
            .unwrap_or_default();

        let text_results = self
            .text_search(params.query_text, params.filter, params.limit)
            .await
            .unwrap_or_default();

        // Merge by fact ID with weighted scores
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

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(params.limit);

        Ok(results)
    }

    // -- VFS path operations ------------------------------------------------

    async fn list_by_path(
        &self,
        parent_path: &str,
        ns: &NamespaceScope,
        workspace: &str,
    ) -> Result<Vec<PathEntry>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
            if matches!(ns, NamespaceScope::Owner) {
                (
                    "SELECT path FROM facts WHERE parent_path = ?1 AND agent = ?2".into(),
                    vec![
                        Box::new(parent_path.to_string()),
                        Box::new(workspace.to_string()),
                    ],
                )
            } else {
                let ns_value = ns.to_namespace_value();
                (
                    "SELECT path FROM facts WHERE parent_path = ?1 AND namespace = ?2 AND agent = ?3"
                        .into(),
                    vec![
                        Box::new(parent_path.to_string()),
                        Box::new(ns_value),
                        Box::new(workspace.to_string()),
                    ],
                )
            };

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AlephError::config(format!("list_by_path query failed: {e}")))?;

        let rows = stmt
            .query_map(param_refs.as_slice(), |row| row.get::<_, String>(0))
            .map_err(|e| AlephError::config(format!("list_by_path failed: {e}")))?;

        let mut path_counts: HashMap<String, usize> = HashMap::new();
        for row in rows {
            let path =
                row.map_err(|e| AlephError::config(format!("list_by_path row failed: {e}")))?;
            *path_counts.entry(path).or_insert(0) += 1;
        }

        let entries = path_counts
            .into_iter()
            .map(|(path, count)| PathEntry {
                path,
                is_leaf: true,
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
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
            if matches!(ns, NamespaceScope::Owner) {
                (
                    "SELECT * FROM facts WHERE path = ?1 AND agent = ?2 LIMIT 1".into(),
                    vec![
                        Box::new(path.to_string()),
                        Box::new(workspace.to_string()),
                    ],
                )
            } else {
                let ns_value = ns.to_namespace_value();
                (
                    "SELECT * FROM facts WHERE path = ?1 AND namespace = ?2 AND agent = ?3 LIMIT 1"
                        .into(),
                    vec![
                        Box::new(path.to_string()),
                        Box::new(ns_value),
                        Box::new(workspace.to_string()),
                    ],
                )
            };

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AlephError::config(format!("get_by_path query failed: {e}")))?;

        let result = stmt
            .query_row(param_refs.as_slice(), row_to_fact)
            .optional()
            .map_err(|e| AlephError::config(format!("get_by_path failed: {e}")))?;

        Ok(result)
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

        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let (filter_clause, filter_params) = build_filter_clause(filter, 1);

        let sql = format!(
            "SELECT * FROM facts WHERE path LIKE ?1{}  LIMIT ?{}",
            if filter_clause.is_empty() {
                String::new()
            } else {
                format!(" AND {}", &filter_clause[7..])
            },
            filter_params.len() + 2,
        );

        let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(format!("{}%", path_prefix))];
        for p in &filter_params {
            all_params.push(clone_tosql_param(p));
        }
        all_params.push(Box::new(limit as i64));

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            all_params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AlephError::config(format!("path_prefix query failed: {e}")))?;

        let rows = stmt
            .query_map(param_refs.as_slice(), row_to_fact)
            .map_err(|e| AlephError::config(format!("path_prefix query failed: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(
                row.map_err(|e| AlephError::config(format!("path_prefix row failed: {e}")))?,
            );
        }

        Ok(results)
    }

    // -- Statistics & bulk --------------------------------------------------

    async fn count_facts(&self, filter: &SearchFilter) -> Result<usize, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let (filter_clause, filter_params) = build_filter_clause(filter, 0);
        let sql = format!("SELECT COUNT(*) FROM facts{}", filter_clause);

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            filter_params.iter().map(|p| p.as_ref()).collect();

        let count: i64 = conn
            .query_row(&sql, param_refs.as_slice(), |row| row.get(0))
            .map_err(|e| AlephError::config(format!("count_facts failed: {e}")))?;

        Ok(count as usize)
    }

    async fn get_facts_by_type(
        &self,
        fact_type: FactType,
        ns: &NamespaceScope,
        workspace: &str,
        limit: usize,
    ) -> Result<Vec<MemoryFact>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
            if matches!(ns, NamespaceScope::Owner) {
                (
                    "SELECT * FROM facts WHERE fact_type = ?1 AND agent = ?2 LIMIT ?3".into(),
                    vec![
                        Box::new(fact_type.as_str().to_string()),
                        Box::new(workspace.to_string()),
                        Box::new(limit as i64),
                    ],
                )
            } else {
                let ns_value = ns.to_namespace_value();
                (
                    "SELECT * FROM facts WHERE fact_type = ?1 AND namespace = ?2 AND agent = ?3 LIMIT ?4"
                        .into(),
                    vec![
                        Box::new(fact_type.as_str().to_string()),
                        Box::new(ns_value),
                        Box::new(workspace.to_string()),
                        Box::new(limit as i64),
                    ],
                )
            };

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AlephError::config(format!("get_facts_by_type query failed: {e}")))?;

        let rows = stmt
            .query_map(param_refs.as_slice(), row_to_fact)
            .map_err(|e| AlephError::config(format!("get_facts_by_type failed: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(
                row.map_err(|e| {
                    AlephError::config(format!("get_facts_by_type row failed: {e}"))
                })?,
            );
        }

        Ok(results)
    }

    async fn get_all_facts(
        &self,
        include_invalid: bool,
        workspace: Option<&str>,
    ) -> Result<Vec<MemoryFact>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let mut clauses = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if !include_invalid {
            params.push(Box::new(1i32));
            clauses.push(format!("is_valid = ?{}", params.len()));
        }
        if let Some(ws) = workspace {
            params.push(Box::new(ws.to_string()));
            clauses.push(format!("agent = ?{}", params.len()));
        }

        let sql = if clauses.is_empty() {
            "SELECT * FROM facts".to_string()
        } else {
            format!("SELECT * FROM facts WHERE {}", clauses.join(" AND "))
        };

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AlephError::config(format!("get_all_facts query failed: {e}")))?;

        let rows = stmt
            .query_map(param_refs.as_slice(), row_to_fact)
            .map_err(|e| AlephError::config(format!("get_all_facts failed: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(
                row.map_err(|e| {
                    AlephError::config(format!("get_all_facts row failed: {e}"))
                })?,
            );
        }

        Ok(results)
    }

    async fn load_embedding_for_fact(
        &self,
        fact: &mut MemoryFact,
    ) -> Result<(), AlephError> {
        if fact.embedding.is_some() {
            return Ok(());
        }
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        // Look up the rowid for this fact
        let rowid: Option<i64> = conn
            .query_row(
                "SELECT rowid FROM facts WHERE id = ?1",
                rusqlite::params![fact.id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| AlephError::config(format!("load_embedding rowid lookup: {e}")))?;

        if let Some(rowid) = rowid {
            fact.embedding = vec::get_embedding(&conn, rowid)?;
        }

        Ok(())
    }

    // -- Mutation helpers ---------------------------------------------------

    async fn invalidate_fact(&self, id: &str, reason: &str) -> Result<(), AlephError> {
        let existing = self.get_fact(id).await?;
        let mut fact =
            existing.ok_or_else(|| AlephError::NotFound(format!("Fact '{}'", id)))?;

        fact.is_valid = false;
        fact.invalidation_reason = Some(reason.to_string());
        fact.decay_invalidated_at = Some(now_unix());
        fact.updated_at = now_unix();

        self.update_fact(&fact).await
    }

    async fn close_fact_validity(&self, id: &str, valid_to: i64) -> Result<(), AlephError> {
        let existing = self.get_fact(id).await?;
        let mut fact =
            existing.ok_or_else(|| AlephError::NotFound(format!("Fact '{}'", id)))?;

        fact.valid_to = Some(valid_to);
        fact.updated_at = now_unix();

        self.update_fact(&fact).await
    }

    async fn set_fact_valid_from(&self, id: &str, valid_from: i64) -> Result<(), AlephError> {
        let existing = self.get_fact(id).await?;
        let mut fact =
            existing.ok_or_else(|| AlephError::NotFound(format!("Fact '{}'", id)))?;

        fact.valid_from = Some(valid_from);
        fact.updated_at = now_unix();

        self.update_fact(&fact).await
    }

    async fn update_fact_content(&self, id: &str, new_content: &str) -> Result<(), AlephError> {
        if let crate::memory::content_scanner::ScanVerdict::Rejected { reason, pattern } =
            crate::memory::content_scanner::scan_content(new_content)
        {
            tracing::warn!(fact_id = %id, pattern = pattern, "Memory content rejected by scanner: {reason}");
            return Err(AlephError::Validation(format!(
                "Memory content rejected: {reason}"
            )));
        }

        let existing = self.get_fact(id).await?;
        let mut fact =
            existing.ok_or_else(|| AlephError::NotFound(format!("Fact '{}'", id)))?;

        fact.content = new_content.to_string();
        fact.updated_at = now_unix();

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
        let now = now_unix();

        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        // Read all valid facts under the same lock to prevent TOCTOU races
        let mut stmt = conn
            .prepare("SELECT * FROM facts WHERE is_valid = 1")
            .map_err(|e| AlephError::config(format!("apply_fact_decay prepare: {e}")))?;

        let all_facts: Vec<MemoryFact> = stmt
            .query_map([], row_to_fact)
            .map_err(|e| AlephError::config(format!("apply_fact_decay query: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("apply_fact_decay collect: {e}")))?;
        drop(stmt);

        conn.execute_batch("BEGIN TRANSACTION")
            .map_err(|e| AlephError::config(format!("apply_fact_decay begin: {e}")))?;

        let mut affected = 0usize;

        for fact in &all_facts {
            let last_access = fact.last_accessed_at.unwrap_or(fact.updated_at);
            let days_since_access = (now - last_access).max(0) as f64 / 86400.0;

            // Ebbinghaus exponential decay
            let decay =
                (-(days_since_access) * (2.0_f64.ln()) / decay_factor as f64).exp() as f32;
            let new_confidence = fact.confidence * decay;

            if new_confidence < min_score {
                // Invalidate: soft-delete via UPDATE (cheaper than delete+insert)
                conn.execute(
                    "UPDATE facts SET is_valid = 0, invalidation_reason = 'decay_prune', \
                     decay_invalidated_at = ?1, confidence = ?2, updated_at = ?3 \
                     WHERE id = ?4",
                    rusqlite::params![now, new_confidence as f64, now, fact.id],
                )
                .map_err(|e| {
                    let _ = conn.execute_batch("ROLLBACK");
                    AlephError::config(format!("apply_fact_decay invalidate: {e}"))
                })?;
                affected += 1;
            } else if (new_confidence - fact.confidence).abs() > f32::EPSILON {
                conn.execute(
                    "UPDATE facts SET confidence = ?1, updated_at = ?2 WHERE id = ?3",
                    rusqlite::params![new_confidence as f64, now, fact.id],
                )
                .map_err(|e| {
                    let _ = conn.execute_batch("ROLLBACK");
                    AlephError::config(format!("apply_fact_decay update: {e}"))
                })?;
                affected += 1;
            }
        }

        conn.execute_batch("COMMIT")
            .map_err(|e| AlephError::config(format!("apply_fact_decay commit: {e}")))?;

        Ok(affected)
    }

    async fn get_fact_stats(&self) -> Result<FactStats, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let total_facts: i64 = conn
            .query_row("SELECT COUNT(*) FROM facts", [], |row| row.get(0))
            .map_err(|e| AlephError::config(format!("count total facts failed: {e}")))?;

        let valid_facts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM facts WHERE is_valid = 1",
                [],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("count valid facts failed: {e}")))?;

        // Breakdown by type
        let mut facts_by_type: HashMap<String, u64> = HashMap::new();
        {
            let mut stmt = conn
                .prepare("SELECT fact_type, COUNT(*) FROM facts GROUP BY fact_type")
                .map_err(|e| AlephError::config(format!("facts_by_type query failed: {e}")))?;

            let rows = stmt
                .query_map([], |row| {
                    let ft: String = row.get(0)?;
                    let count: i64 = row.get(1)?;
                    Ok((ft, count as u64))
                })
                .map_err(|e| AlephError::config(format!("facts_by_type failed: {e}")))?;

            for row in rows {
                let (ft, count) = row.map_err(|e| {
                    AlephError::config(format!("facts_by_type row failed: {e}"))
                })?;
                facts_by_type.insert(ft, count);
            }
        }

        // Timestamps
        let oldest: Option<i64> = conn
            .query_row(
                "SELECT MIN(created_at) FROM facts",
                [],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("oldest fact query failed: {e}")))?;

        let newest: Option<i64> = conn
            .query_row(
                "SELECT MAX(created_at) FROM facts",
                [],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("newest fact query failed: {e}")))?;

        Ok(FactStats {
            total_facts: total_facts as u64,
            valid_facts: valid_facts as u64,
            facts_by_type,
            oldest_fact_timestamp: oldest,
            newest_fact_timestamp: newest,
        })
    }

    async fn soft_delete_fact(&self, id: &str, reason: &str) -> Result<(), AlephError> {
        self.invalidate_fact(id, reason).await
    }
}

// ---------------------------------------------------------------------------
// Helper: clone a Box<dyn ToSql> (workaround for filter param reuse)
// ---------------------------------------------------------------------------

/// Clone a boxed ToSql parameter by downcasting to known types.
///
/// This is needed because `dyn ToSql` is not `Clone`. We support the types
/// that `build_filter_clause` actually produces: String, i32, i64, f64.
fn clone_tosql_param(param: &dyn rusqlite::types::ToSql) -> Box<dyn rusqlite::types::ToSql> {
    use rusqlite::types::ToSqlOutput;

    // Use to_sql() to get the value and reconstruct
    match param.to_sql().unwrap() {
        ToSqlOutput::Borrowed(rusqlite::types::ValueRef::Text(s)) => {
            Box::new(String::from_utf8_lossy(s).into_owned())
        }
        ToSqlOutput::Owned(rusqlite::types::Value::Text(s)) => Box::new(s),
        ToSqlOutput::Owned(rusqlite::types::Value::Integer(i)) => Box::new(i),
        ToSqlOutput::Owned(rusqlite::types::Value::Real(f)) => Box::new(f),
        ToSqlOutput::Borrowed(rusqlite::types::ValueRef::Integer(i)) => Box::new(i),
        ToSqlOutput::Borrowed(rusqlite::types::ValueRef::Real(f)) => Box::new(f),
        other => {
            // Fallback: convert to string representation
            Box::new(format!("{:?}", other))
        }
    }
}

// ============================================================================
// Inherent helpers on SqliteMemoryBackend (not part of the MemoryStore trait)
// ============================================================================

impl SqliteMemoryBackend {
    /// Return the most-recently-updated synthesis facts visible to `namespace`.
    ///
    /// Facts with `fact_source = 'synthesis'` and `is_valid = 1` are returned,
    /// scoped to the given namespace **or** the global `'owner'` namespace.
    pub fn top_synthesized(
        &self,
        namespace: &str,
        limit: usize,
    ) -> Result<Vec<MemoryFact>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT * FROM facts \
                 WHERE fact_source = 'synthesis' \
                   AND is_valid = 1 \
                   AND (namespace = ?1 OR namespace = 'owner') \
                 ORDER BY updated_at DESC \
                 LIMIT ?2",
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare top_synthesized: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![namespace, limit as i64], row_to_fact)
            .map_err(|e| AlephError::config(format!("Failed to query top_synthesized: {e}")))?;

        let mut facts = Vec::new();
        for row in rows {
            facts.push(
                row.map_err(|e| {
                    AlephError::config(format!("Failed to read top_synthesized row: {e}"))
                })?,
            );
        }
        Ok(facts)
    }

    /// Bump the `updated_at` timestamp of a fact to the current time.
    pub fn touch_fact_updated_at(&self, fact_id: &str) -> Result<(), AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "UPDATE facts SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now, fact_id],
        )
        .map_err(|e| {
            AlephError::config(format!("Failed to touch_fact_updated_at {fact_id}: {e}"))
        })?;

        Ok(())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Create a test backend with an in-memory-like temp database.
    fn test_backend() -> (SqliteMemoryBackend, TempDir) {
        let dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = dir.path().join("test_memory.db");
        let backend =
            SqliteMemoryBackend::new(&db_path).expect("Failed to create test backend");
        (backend, dir)
    }

    /// Create a test MemoryFact with minimal fields.
    fn test_fact(id: &str, content: &str) -> MemoryFact {
        MemoryFact::with_id(id.to_string(), content.to_string(), FactType::Other)
    }

    #[tokio::test]
    async fn test_insert_and_get_fact() {
        let (backend, _dir) = test_backend();
        let fact = test_fact("fact-1", "The user likes Rust programming");

        backend.insert_fact(&fact).await.unwrap();

        let retrieved = backend.get_fact("fact-1").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.id, "fact-1");
        assert_eq!(retrieved.content, "The user likes Rust programming");
        assert_eq!(retrieved.fact_type.as_str(), "other");
        assert!(retrieved.is_valid);
    }

    #[tokio::test]
    async fn test_delete_fact() {
        let (backend, _dir) = test_backend();
        let fact = test_fact("fact-del", "Temporary fact");

        backend.insert_fact(&fact).await.unwrap();
        assert!(backend.get_fact("fact-del").await.unwrap().is_some());

        backend.delete_fact("fact-del").await.unwrap();
        assert!(backend.get_fact("fact-del").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_update_fact() {
        let (backend, _dir) = test_backend();
        let fact = test_fact("fact-upd", "Original content");
        backend.insert_fact(&fact).await.unwrap();

        let mut updated = fact.clone();
        updated.content = "Updated content".to_string();
        backend.update_fact(&updated).await.unwrap();

        let retrieved = backend.get_fact("fact-upd").await.unwrap().unwrap();
        assert_eq!(retrieved.content, "Updated content");
    }

    #[tokio::test]
    async fn test_batch_insert() {
        let (backend, _dir) = test_backend();
        let facts = vec![
            test_fact("batch-1", "First fact"),
            test_fact("batch-2", "Second fact"),
            test_fact("batch-3", "Third fact"),
        ];

        backend.batch_insert_facts(&facts).await.unwrap();

        assert!(backend.get_fact("batch-1").await.unwrap().is_some());
        assert!(backend.get_fact("batch-2").await.unwrap().is_some());
        assert!(backend.get_fact("batch-3").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_text_search() {
        let (backend, _dir) = test_backend();

        let facts = vec![
            test_fact("ts-1", "The user enjoys writing Rust code"),
            test_fact("ts-2", "The user prefers Python for scripting"),
            test_fact("ts-3", "The user likes cooking Italian food"),
        ];
        backend.batch_insert_facts(&facts).await.unwrap();

        let results = backend
            .text_search("Rust code", &SearchFilter::new(), 10)
            .await
            .unwrap();

        assert!(!results.is_empty());
        assert_eq!(results[0].fact.id, "ts-1");
    }

    #[tokio::test]
    async fn test_invalidate_fact() {
        let (backend, _dir) = test_backend();
        let fact = test_fact("inv-1", "Fact to invalidate");
        backend.insert_fact(&fact).await.unwrap();

        backend
            .invalidate_fact("inv-1", "outdated information")
            .await
            .unwrap();

        let retrieved = backend.get_fact("inv-1").await.unwrap().unwrap();
        assert!(!retrieved.is_valid);
        assert_eq!(
            retrieved.invalidation_reason.as_deref(),
            Some("outdated information")
        );
    }

    #[tokio::test]
    async fn test_count_facts() {
        let (backend, _dir) = test_backend();
        let facts = vec![
            test_fact("cnt-1", "Fact one"),
            test_fact("cnt-2", "Fact two"),
        ];
        backend.batch_insert_facts(&facts).await.unwrap();

        let count = backend.count_facts(&SearchFilter::new()).await.unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_get_fact_stats() {
        let (backend, _dir) = test_backend();
        let facts = vec![
            test_fact("st-1", "Stat fact one"),
            test_fact("st-2", "Stat fact two"),
        ];
        backend.batch_insert_facts(&facts).await.unwrap();

        let stats = backend.get_fact_stats().await.unwrap();
        assert_eq!(stats.total_facts, 2);
        assert_eq!(stats.valid_facts, 2);
        assert!(stats.oldest_fact_timestamp.is_some());
        assert!(stats.newest_fact_timestamp.is_some());
    }

    #[tokio::test]
    async fn test_get_all_facts_excludes_invalid() {
        let (backend, _dir) = test_backend();
        let facts = vec![
            test_fact("all-1", "Valid fact"),
            test_fact("all-2", "Will be invalidated"),
        ];
        backend.batch_insert_facts(&facts).await.unwrap();
        backend
            .invalidate_fact("all-2", "test")
            .await
            .unwrap();

        let valid_only = backend.get_all_facts(false, None).await.unwrap();
        assert_eq!(valid_only.len(), 1);
        assert_eq!(valid_only[0].id, "all-1");

        let all = backend.get_all_facts(true, None).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_update_fact_content() {
        let (backend, _dir) = test_backend();
        let fact = test_fact("uc-1", "Original");
        backend.insert_fact(&fact).await.unwrap();

        backend
            .update_fact_content("uc-1", "New content")
            .await
            .unwrap();

        let retrieved = backend.get_fact("uc-1").await.unwrap().unwrap();
        assert_eq!(retrieved.content, "New content");
    }

    #[tokio::test]
    async fn test_vector_search() {
        let (backend, _dir) = test_backend();

        // Create facts with simple 768-dim embeddings
        let mut fact1 = test_fact("vs-1", "Rust programming");
        let mut emb1 = vec![0.0f32; 768];
        emb1[0] = 1.0;
        fact1.embedding = Some(emb1);

        let mut fact2 = test_fact("vs-2", "Python scripting");
        let mut emb2 = vec![0.0f32; 768];
        emb2[1] = 1.0;
        fact2.embedding = Some(emb2);

        backend.insert_fact(&fact1).await.unwrap();
        backend.insert_fact(&fact2).await.unwrap();

        // Search with embedding close to fact1
        let mut query_emb = vec![0.0f32; 768];
        query_emb[0] = 0.9;
        query_emb[1] = 0.1;

        let results = backend
            .vector_search(&query_emb, 768, &SearchFilter::new(), 2)
            .await
            .unwrap();

        assert!(!results.is_empty());
        // fact1 should be closer
        assert_eq!(results[0].fact.id, "vs-1");
        assert!(results[0].score > 0.0);
    }

    #[tokio::test]
    async fn test_soft_delete_fact() {
        let (backend, _dir) = test_backend();
        let fact = test_fact("sd-1", "Soft deletable fact");
        backend.insert_fact(&fact).await.unwrap();

        backend.soft_delete_fact("sd-1", "no longer relevant").await.unwrap();

        let retrieved = backend.get_fact("sd-1").await.unwrap().unwrap();
        assert!(!retrieved.is_valid);
    }

    #[tokio::test]
    async fn test_invalidate_consumed_chunks() {
        let (backend, _dir) = test_backend();

        // Insert two raw chunk facts
        let mut f1 = MemoryFact::new("raw chunk 1".into(), FactType::Other, vec![]);
        f1.path = "aleph://session/s1/raw/0".into();
        f1.fact_source = FactSource::SessionCompressed;
        backend.insert_fact(&f1).await.unwrap();

        let mut f2 = MemoryFact::new("raw chunk 2".into(), FactType::Other, vec![]);
        f2.path = "aleph://session/s1/raw/1".into();
        f2.fact_source = FactSource::SessionCompressed;
        backend.insert_fact(&f2).await.unwrap();

        // Insert one non-raw fact (should NOT be invalidated)
        let f3 = MemoryFact::new("extracted fact".into(), FactType::Preference, vec![]);
        backend.insert_fact(&f3).await.unwrap();

        // Invalidate consumed chunks
        let count = backend
            .invalidate_consumed_chunks(&[f1.id.clone(), f2.id.clone()])
            .unwrap();
        assert_eq!(count, 2);

        // Verify raw chunks are invalid
        let f1_after = backend.get_fact(&f1.id).await.unwrap().unwrap();
        assert!(!f1_after.is_valid);
        assert_eq!(
            f1_after.invalidation_reason.as_deref(),
            Some("consumed_by_compression")
        );

        // Verify extracted fact is still valid
        let f3_after = backend.get_fact(&f3.id).await.unwrap().unwrap();
        assert!(f3_after.is_valid);
    }

    #[tokio::test]
    async fn test_cleanup_expired_sessions() {
        let (backend, _dir) = test_backend();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        // Insert an old session fact (25 hours ago)
        let mut old = MemoryFact::new("old session summary".into(), FactType::Other, vec![]);
        old.path = "aleph://session/old-sess/d2/0".into();
        old.fact_source = FactSource::SessionCompressed;
        old.scope = MemoryScope::SessionLocal;
        old.created_at = now - 25 * 3600;
        old.updated_at = old.created_at;
        backend.insert_fact(&old).await.unwrap();

        // Insert a recent session fact (1 hour ago)
        let mut recent = MemoryFact::new("recent session".into(), FactType::Other, vec![]);
        recent.path = "aleph://session/new-sess/d0/0".into();
        recent.fact_source = FactSource::SessionCompressed;
        recent.scope = MemoryScope::SessionLocal;
        recent.created_at = now - 3600;
        recent.updated_at = recent.created_at;
        backend.insert_fact(&recent).await.unwrap();

        // Insert a global fact (should NOT be touched)
        let global = MemoryFact::new("global fact".into(), FactType::Preference, vec![]);
        backend.insert_fact(&global).await.unwrap();

        // Cleanup with 24h retention
        let count = backend.cleanup_expired_sessions(24).unwrap();
        assert_eq!(count, 1);

        // Old session fact invalidated
        let old_after = backend.get_fact(&old.id).await.unwrap().unwrap();
        assert!(!old_after.is_valid);

        // Recent session fact still valid
        let recent_after = backend.get_fact(&recent.id).await.unwrap().unwrap();
        assert!(recent_after.is_valid);

        // Global fact untouched
        let global_after = backend.get_fact(&global.id).await.unwrap().unwrap();
        assert!(global_after.is_valid);
    }

    #[tokio::test]
    async fn top_synthesized_empty() {
        let (backend, _dir) = test_backend();
        // No facts at all — should return empty vec
        let results = backend.top_synthesized("owner", 10).unwrap();
        assert!(results.is_empty());

        // Insert a non-synthesis fact — still empty
        let fact = test_fact("non-synth-1", "Regular fact");
        backend.insert_fact(&fact).await.unwrap();
        let results = backend.top_synthesized("owner", 10).unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn touch_fact_updated_at_updates_timestamp() {
        let (backend, _dir) = test_backend();
        let fact = test_fact("touch-1", "Touchable fact");
        backend.insert_fact(&fact).await.unwrap();

        let before = backend.get_fact("touch-1").await.unwrap().unwrap();
        let original_updated = before.updated_at;

        // Sleep briefly so the timestamp differs
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        backend.touch_fact_updated_at("touch-1").unwrap();

        let after = backend.get_fact("touch-1").await.unwrap().unwrap();
        // updated_at should be >= original (timestamps are in seconds, so >= is correct)
        assert!(
            after.updated_at >= original_updated,
            "updated_at should have been bumped: before={}, after={}",
            original_updated,
            after.updated_at
        );
    }
}
