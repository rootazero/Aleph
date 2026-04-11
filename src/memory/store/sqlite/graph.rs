//! GraphStore trait implementation for SqliteMemoryBackend.
//!
//! Provides knowledge-graph node/edge CRUD, entity resolution,
//! context-scoped edge queries, and temporal decay operations
//! against the SQLite `graph_nodes` and `graph_edges` tables.

use async_trait::async_trait;
use rusqlite::params;
use rusqlite::OptionalExtension;
use std::time::SystemTime;

use crate::config::types::memory::GraphDecayPolicy;
use crate::error::AlephError;
use crate::memory::store::{DecayStats, GraphEdge, GraphNode, GraphStore, ResolvedEntity};

use super::SqliteMemoryBackend;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map a SQLite row to a GraphNode.
fn row_to_node(row: &rusqlite::Row) -> rusqlite::Result<GraphNode> {
    let aliases_json: Option<String> = row.get("aliases")?;
    let aliases: Vec<String> = aliases_json
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    Ok(GraphNode {
        id: row.get("id")?,
        name: row.get("name")?,
        kind: row.get("kind")?,
        aliases,
        metadata_json: row.get::<_, Option<String>>("metadata")?.unwrap_or_default(),
        decay_score: row.get::<_, Option<f64>>("decay_score")?.unwrap_or(1.0) as f32,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        agent: row.get::<_, Option<String>>("agent")?.unwrap_or_default(),
    })
}

/// Map a SQLite row to a GraphEdge.
fn row_to_edge(row: &rusqlite::Row) -> rusqlite::Result<GraphEdge> {
    Ok(GraphEdge {
        id: row.get("id")?,
        from_id: row.get("from_id")?,
        to_id: row.get("to_id")?,
        relation: row.get("relation")?,
        weight: row.get::<_, Option<f64>>("weight")?.unwrap_or(1.0) as f32,
        confidence: row.get::<_, Option<f64>>("confidence")?.unwrap_or(1.0) as f32,
        context_key: row.get::<_, Option<String>>("context_key")?.unwrap_or_default(),
        decay_score: row.get::<_, Option<f64>>("decay_score")?.unwrap_or(1.0) as f32,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        last_seen_at: row.get::<_, Option<i64>>("last_seen_at")?.unwrap_or(0),
        agent: row.get::<_, Option<String>>("agent")?.unwrap_or_default(),
    })
}

/// Acquire the connection lock, returning an AlephError on poison.
macro_rules! lock_conn {
    ($self:expr) => {
        $self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Lock: {}", e)))
    };
}

// ============================================================================
// Batch queries (inherent methods, not part of GraphStore trait)
// ============================================================================

impl SqliteMemoryBackend {
    /// Count edges for multiple entities in a single query.
    ///
    /// Returns a map from entity name to its total edge count (both incoming
    /// and outgoing). Entities with zero edges are included with count 0.
    /// Returns an empty map if `entity_names` is empty.
    pub fn edge_count_for_entities(
        &self,
        entity_names: &[String],
        workspace: &str,
    ) -> Result<std::collections::HashMap<String, u32>, AlephError> {
        if entity_names.is_empty() {
            return Ok(std::collections::HashMap::new());
        }

        let conn = lock_conn!(self)?;

        // Build dynamic IN clause with positional parameters.
        // Parameter ?1 is workspace (agent column), ?2.. are entity names.
        let placeholders: Vec<String> = (0..entity_names.len())
            .map(|i| format!("?{}", i + 2))
            .collect();
        let in_clause = placeholders.join(", ");

        let sql = format!(
            "SELECT gn.name, COUNT(DISTINCT ge.id) as edge_count \
             FROM graph_nodes gn \
             LEFT JOIN graph_edges ge ON (ge.from_id = gn.id OR ge.to_id = gn.id) \
                 AND ge.agent = ?1 \
             WHERE gn.name IN ({in_clause}) \
                 AND gn.agent = ?1 \
             GROUP BY gn.name"
        );

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| AlephError::config(format!("edge_count_for_entities prepare: {e}")))?;

        // Bind workspace as ?1, then each entity name.
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        param_values.push(Box::new(workspace.to_string()));
        for name in entity_names {
            param_values.push(Box::new(name.clone()));
        }
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            param_values.iter().map(|b| b.as_ref()).collect();

        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                let name: String = row.get(0)?;
                let count: i64 = row.get(1)?;
                Ok((name, count as u32))
            })
            .map_err(|e| AlephError::config(format!("edge_count_for_entities query: {e}")))?;

        let mut result = std::collections::HashMap::new();
        for row in rows {
            let (name, count) =
                row.map_err(|e| AlephError::config(format!("edge_count_for_entities row: {e}")))?;
            result.insert(name, count);
        }

        Ok(result)
    }
}

// ============================================================================
// GraphStore implementation
// ============================================================================

#[async_trait]
impl GraphStore for SqliteMemoryBackend {
    async fn upsert_node(&self, node: &GraphNode, workspace: &str) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;
        let aliases_json =
            serde_json::to_string(&node.aliases).unwrap_or_else(|_| "[]".to_string());

        conn.execute(
            "INSERT OR REPLACE INTO graph_nodes \
             (id, name, kind, aliases, metadata, decay_score, created_at, updated_at, agent) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                node.id,
                node.name,
                node.kind,
                aliases_json,
                node.metadata_json,
                node.decay_score as f64,
                node.created_at,
                node.updated_at,
                workspace,
            ],
        )
        .map_err(|e| AlephError::config(format!("upsert_node: {e}")))?;

        Ok(())
    }

    async fn get_node(&self, id: &str, workspace: &str) -> Result<Option<GraphNode>, AlephError> {
        let conn = lock_conn!(self)?;

        let result = conn
            .query_row(
                "SELECT * FROM graph_nodes WHERE id = ?1 AND agent = ?2",
                params![id, workspace],
                row_to_node,
            )
            .optional()
            .map_err(|e| AlephError::config(format!("get_node: {e}")))?;

        Ok(result)
    }

    async fn upsert_edge(&self, edge: &GraphEdge, workspace: &str) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;

        conn.execute(
            "INSERT OR REPLACE INTO graph_edges \
             (id, from_id, to_id, relation, weight, confidence, context_key, \
              decay_score, created_at, updated_at, last_seen_at, agent) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                edge.id,
                edge.from_id,
                edge.to_id,
                edge.relation,
                edge.weight as f64,
                edge.confidence as f64,
                edge.context_key,
                edge.decay_score as f64,
                edge.created_at,
                edge.updated_at,
                edge.last_seen_at,
                workspace,
            ],
        )
        .map_err(|e| AlephError::config(format!("upsert_edge: {e}")))?;

        Ok(())
    }

    async fn resolve_entity(
        &self,
        query: &str,
        context_key: Option<&str>,
        workspace: &str,
    ) -> Result<Vec<ResolvedEntity>, AlephError> {
        let conn = lock_conn!(self)?;

        // Escape LIKE wildcards in the query to prevent unintended pattern matching
        let escaped_query = query.replace('%', "\\%").replace('_', "\\_");

        // Find nodes where name matches case-insensitively OR aliases JSON contains the query.
        let mut stmt = conn
            .prepare(
                "SELECT * FROM graph_nodes \
                 WHERE agent = ?1 \
                   AND (LOWER(name) = LOWER(?2) OR aliases LIKE '%' || ?3 || '%' ESCAPE '\\')",
            )
            .map_err(|e| AlephError::config(format!("resolve_entity prepare: {e}")))?;

        let candidates: Vec<GraphNode> = stmt
            .query_map(params![workspace, query, escaped_query], row_to_node)
            .map_err(|e| AlephError::config(format!("resolve_entity query: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("resolve_entity collect: {e}")))?;

        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let ambiguous = candidates.len() > 1;

        let mut results: Vec<ResolvedEntity> = Vec::with_capacity(candidates.len());
        for node in &candidates {
            let context_score = if let Some(ctx) = context_key {
                // Count edges in the given context to compute a relevance score.
                // We do this inline to avoid releasing & re-acquiring the lock.
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM graph_edges \
                         WHERE (from_id = ?1 OR to_id = ?2) \
                           AND context_key = ?3 AND agent = ?4",
                        params![node.id, node.id, ctx, workspace],
                        |row| row.get(0),
                    )
                    .map_err(|e| {
                        AlephError::config(format!("resolve_entity edge count: {e}"))
                    })?;
                count as f32
            } else {
                0.0
            };

            results.push(ResolvedEntity {
                node_id: node.id.clone(),
                name: node.name.clone(),
                kind: node.kind.clone(),
                aliases: node.aliases.clone(),
                context_score,
                ambiguous,
            });
        }

        // Sort by context_score descending so the most relevant entity is first.
        results.sort_by(|a, b| {
            b.context_score
                .partial_cmp(&a.context_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(results)
    }

    async fn get_edges_for_node(
        &self,
        node_id: &str,
        context_key: Option<&str>,
        workspace: &str,
    ) -> Result<Vec<GraphEdge>, AlephError> {
        let conn = lock_conn!(self)?;

        if let Some(ctx) = context_key {
            let mut stmt = conn
                .prepare(
                    "SELECT * FROM graph_edges \
                     WHERE (from_id = ?1 OR to_id = ?2) AND agent = ?3 AND context_key = ?4",
                )
                .map_err(|e| AlephError::config(format!("get_edges_for_node prepare: {e}")))?;

            let edges = stmt
                .query_map(params![node_id, node_id, workspace, ctx], row_to_edge)
                .map_err(|e| AlephError::config(format!("get_edges_for_node query: {e}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| AlephError::config(format!("get_edges_for_node collect: {e}")))?;

            Ok(edges)
        } else {
            let mut stmt = conn
                .prepare(
                    "SELECT * FROM graph_edges \
                     WHERE (from_id = ?1 OR to_id = ?2) AND agent = ?3",
                )
                .map_err(|e| AlephError::config(format!("get_edges_for_node prepare: {e}")))?;

            let edges = stmt
                .query_map(params![node_id, node_id, workspace], row_to_edge)
                .map_err(|e| AlephError::config(format!("get_edges_for_node query: {e}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| AlephError::config(format!("get_edges_for_node collect: {e}")))?;

            Ok(edges)
        }
    }

    async fn count_edges_in_context(
        &self,
        node_id: &str,
        context_key: &str,
        workspace: &str,
    ) -> Result<usize, AlephError> {
        let conn = lock_conn!(self)?;

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_edges \
                 WHERE (from_id = ?1 OR to_id = ?2) AND context_key = ?3 AND agent = ?4",
                params![node_id, node_id, context_key, workspace],
                |row| row.get(0),
            )
            .map_err(|e| AlephError::config(format!("count_edges_in_context: {e}")))?;

        Ok(count as usize)
    }

    async fn apply_decay(
        &self,
        policy: &GraphDecayPolicy,
        workspace: &str,
    ) -> Result<DecayStats, AlephError> {
        let conn = lock_conn!(self)?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let seconds_per_day: f64 = 86400.0;
        let mut stats = DecayStats::default();

        conn.execute_batch("BEGIN TRANSACTION")
            .map_err(|e| AlephError::config(format!("apply_decay begin: {e}")))?;

        // --- Decay nodes ---
        {
            let mut stmt = conn
                .prepare("SELECT * FROM graph_nodes WHERE agent = ?1")
                .map_err(|e| AlephError::config(format!("apply_decay nodes prepare: {e}")))?;

            let nodes: Vec<GraphNode> = stmt
                .query_map(params![workspace], row_to_node)
                .map_err(|e| AlephError::config(format!("apply_decay nodes query: {e}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| AlephError::config(format!("apply_decay nodes collect: {e}")))?;
            drop(stmt);

            for node in &nodes {
                let days_since_update =
                    (now - node.updated_at).max(0) as f64 / seconds_per_day;
                let new_score =
                    node.decay_score - (days_since_update as f32 * policy.node_decay_per_day);

                if new_score < policy.min_score {
                    // Prune: delete orphaned edges first, then the node
                    let cascade_count = conn.execute(
                        "DELETE FROM graph_edges WHERE (from_id = ?1 OR to_id = ?2) AND agent = ?3",
                        params![node.id, node.id, workspace],
                    )
                    .map_err(|e| {
                        let _ = conn.execute_batch("ROLLBACK");
                        AlephError::config(format!("apply_decay cascade edges: {e}"))
                    })?;
                    stats.edges_pruned += cascade_count;
                    // Cascade: delete memory_entities for this pruned node
                    conn.execute(
                        "DELETE FROM memory_entities WHERE node_id = ?1 AND agent = ?2",
                        params![node.id, workspace],
                    )
                    .map_err(|e| {
                        let _ = conn.execute_batch("ROLLBACK");
                        AlephError::config(format!("apply_decay cascade memory_entities: {e}"))
                    })?;
                    conn.execute(
                        "DELETE FROM graph_nodes WHERE id = ?1 AND agent = ?2",
                        params![node.id, workspace],
                    )
                    .map_err(|e| {
                        let _ = conn.execute_batch("ROLLBACK");
                        AlephError::config(format!("apply_decay prune node: {e}"))
                    })?;
                    stats.nodes_pruned += 1;
                } else if (new_score - node.decay_score).abs() > f32::EPSILON {
                    conn.execute(
                        "UPDATE graph_nodes SET decay_score = ?1 WHERE id = ?2 AND agent = ?3",
                        params![new_score as f64, node.id, workspace],
                    )
                    .map_err(|e| {
                        let _ = conn.execute_batch("ROLLBACK");
                        AlephError::config(format!("apply_decay update node: {e}"))
                    })?;
                    stats.nodes_decayed += 1;
                }
            }
        }

        // --- Decay edges ---
        {
            let mut stmt = conn
                .prepare("SELECT * FROM graph_edges WHERE agent = ?1")
                .map_err(|e| AlephError::config(format!("apply_decay edges prepare: {e}")))?;

            let edges: Vec<GraphEdge> = stmt
                .query_map(params![workspace], row_to_edge)
                .map_err(|e| AlephError::config(format!("apply_decay edges query: {e}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| AlephError::config(format!("apply_decay edges collect: {e}")))?;
            drop(stmt);

            for edge in &edges {
                let days_since_update =
                    (now - edge.updated_at).max(0) as f64 / seconds_per_day;
                let new_score =
                    edge.decay_score - (days_since_update as f32 * policy.edge_decay_per_day);

                if new_score < policy.min_score {
                    conn.execute(
                        "DELETE FROM graph_edges WHERE id = ?1 AND agent = ?2",
                        params![edge.id, workspace],
                    )
                    .map_err(|e| {
                        let _ = conn.execute_batch("ROLLBACK");
                        AlephError::config(format!("apply_decay prune edge: {e}"))
                    })?;
                    stats.edges_pruned += 1;
                } else if (new_score - edge.decay_score).abs() > f32::EPSILON {
                    conn.execute(
                        "UPDATE graph_edges SET decay_score = ?1 WHERE id = ?2 AND agent = ?3",
                        params![new_score as f64, edge.id, workspace],
                    )
                    .map_err(|e| {
                        let _ = conn.execute_batch("ROLLBACK");
                        AlephError::config(format!("apply_decay update edge: {e}"))
                    })?;
                    stats.edges_decayed += 1;
                }
            }
        }

        conn.execute_batch("COMMIT")
            .map_err(|e| AlephError::config(format!("apply_decay commit: {e}")))?;

        Ok(stats)
    }

    async fn link_memory_entity(
        &self,
        fact_id: &str,
        node_id: &str,
        weight: f32,
        source: &str,
        workspace: &str,
    ) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;
        let id = format!("me_{}", uuid::Uuid::new_v4());
        let now = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        conn.execute(
            "INSERT INTO memory_entities (id, fact_id, node_id, weight, source, created_at, agent) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(fact_id, node_id) DO UPDATE SET weight = ?4, source = ?5",
            params![id, fact_id, node_id, weight as f64, source, now, workspace],
        )
        .map_err(|e| AlephError::config(format!("link_memory_entity: {e}")))?;

        Ok(())
    }

    async fn get_nodes_for_fact(
        &self,
        fact_id: &str,
        workspace: &str,
    ) -> Result<Vec<(GraphNode, f32)>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT gn.id, gn.name, gn.kind, gn.aliases, gn.metadata, \
                        gn.decay_score, gn.created_at, gn.updated_at, gn.agent, \
                        me.weight \
                 FROM memory_entities me \
                 JOIN graph_nodes gn ON gn.id = me.node_id \
                 WHERE me.fact_id = ?1 AND me.agent = ?2",
            )
            .map_err(|e| AlephError::config(format!("get_nodes_for_fact prepare: {e}")))?;

        let rows = stmt
            .query_map(params![fact_id, workspace], |row| {
                let aliases_json: Option<String> = row.get("aliases")?;
                let aliases: Vec<String> = aliases_json
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default();
                let node = GraphNode {
                    id: row.get("id")?,
                    name: row.get("name")?,
                    kind: row.get("kind")?,
                    aliases,
                    metadata_json: row.get::<_, Option<String>>("metadata")?.unwrap_or_default(),
                    decay_score: row.get::<_, Option<f64>>("decay_score")?.unwrap_or(1.0) as f32,
                    created_at: row.get("created_at")?,
                    updated_at: row.get("updated_at")?,
                    agent: row.get::<_, Option<String>>("agent")?.unwrap_or_default(),
                };
                let weight: f64 = row.get("weight")?;
                Ok((node, weight as f32))
            })
            .map_err(|e| AlephError::config(format!("get_nodes_for_fact query: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("get_nodes_for_fact collect: {e}")))?;

        Ok(rows)
    }

    async fn get_facts_for_node(
        &self,
        node_id: &str,
        workspace: &str,
    ) -> Result<Vec<(String, f32)>, AlephError> {
        let conn = lock_conn!(self)?;

        let mut stmt = conn
            .prepare(
                "SELECT fact_id, weight FROM memory_entities \
                 WHERE node_id = ?1 AND agent = ?2",
            )
            .map_err(|e| AlephError::config(format!("get_facts_for_node prepare: {e}")))?;

        let rows = stmt
            .query_map(params![node_id, workspace], |row| {
                let fact_id: String = row.get(0)?;
                let weight: f64 = row.get(1)?;
                Ok((fact_id, weight as f32))
            })
            .map_err(|e| AlephError::config(format!("get_facts_for_node query: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("get_facts_for_node collect: {e}")))?;

        Ok(rows)
    }

    async fn unlink_memory_entity(
        &self,
        fact_id: &str,
        node_id: &str,
        workspace: &str,
    ) -> Result<(), AlephError> {
        let conn = lock_conn!(self)?;

        conn.execute(
            "DELETE FROM memory_entities WHERE fact_id = ?1 AND node_id = ?2 AND agent = ?3",
            params![fact_id, node_id, workspace],
        )
        .map_err(|e| AlephError::config(format!("unlink_memory_entity: {e}")))?;

        Ok(())
    }

    async fn delete_memory_entities_for_fact(
        &self,
        fact_id: &str,
        workspace: &str,
    ) -> Result<usize, AlephError> {
        let conn = lock_conn!(self)?;

        let count = conn
            .execute(
                "DELETE FROM memory_entities WHERE fact_id = ?1 AND agent = ?2",
                params![fact_id, workspace],
            )
            .map_err(|e| AlephError::config(format!("delete_memory_entities_for_fact: {e}")))?;

        Ok(count)
    }

    async fn delete_memory_entities_for_node(
        &self,
        node_id: &str,
        workspace: &str,
    ) -> Result<usize, AlephError> {
        let conn = lock_conn!(self)?;

        let count = conn
            .execute(
                "DELETE FROM memory_entities WHERE node_id = ?1 AND agent = ?2",
                params![node_id, workspace],
            )
            .map_err(|e| AlephError::config(format!("delete_memory_entities_for_node: {e}")))?;

        Ok(count)
    }

    async fn delete_memory_entities_by_source(
        &self,
        fact_id: &str,
        source: &str,
        workspace: &str,
    ) -> Result<usize, AlephError> {
        let conn = lock_conn!(self)?;

        let count = conn
            .execute(
                "DELETE FROM memory_entities WHERE fact_id = ?1 AND source = ?2 AND agent = ?3",
                params![fact_id, source, workspace],
            )
            .map_err(|e| AlephError::config(format!("delete_memory_entities_by_source: {e}")))?;

        Ok(count)
    }

    async fn get_high_score_nodes(
        &self,
        min_score: f32,
        limit: usize,
        workspace: &str,
    ) -> Result<Vec<GraphNode>, AlephError> {
        let conn = lock_conn!(self)?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, kind, aliases, metadata, decay_score, created_at, updated_at, agent \
                 FROM graph_nodes \
                 WHERE decay_score > ?1 AND agent = ?2 \
                 ORDER BY decay_score DESC \
                 LIMIT ?3",
            )
            .map_err(|e| AlephError::config(format!("get_high_score_nodes prepare: {e}")))?;

        let rows = stmt
            .query_map(
                params![min_score as f64, workspace, limit as i64],
                row_to_node,
            )
            .map_err(|e| AlephError::config(format!("get_high_score_nodes query: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("get_high_score_nodes collect: {e}")))?;

        Ok(rows)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a test SqliteMemoryBackend in a temp directory.
    fn create_test_backend() -> (tempfile::TempDir, SqliteMemoryBackend) {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test_graph.db");
        let backend = SqliteMemoryBackend::new(&db_path).unwrap();
        (tmp, backend)
    }

    /// Helper: create a test GraphNode.
    fn make_test_node(id: &str, name: &str, kind: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            aliases: vec![],
            metadata_json: String::new(),
            decay_score: 1.0,
            created_at: 1700000000,
            updated_at: 1700000000,
            agent: "default".to_string(),
        }
    }

    /// Helper: create a test GraphEdge.
    fn make_test_edge(
        id: &str,
        from_id: &str,
        to_id: &str,
        relation: &str,
        context_key: &str,
    ) -> GraphEdge {
        GraphEdge {
            id: id.to_string(),
            from_id: from_id.to_string(),
            to_id: to_id.to_string(),
            relation: relation.to_string(),
            weight: 1.0,
            confidence: 0.9,
            context_key: context_key.to_string(),
            decay_score: 1.0,
            created_at: 1700000000,
            updated_at: 1700000000,
            last_seen_at: 1700000000,
            agent: "default".to_string(),
        }
    }

    #[tokio::test]
    async fn test_upsert_and_get_node() {
        let (_tmp, backend) = create_test_backend();
        let node = make_test_node("gn-001", "Rust", "language");

        backend.upsert_node(&node, "default").await.unwrap();

        let retrieved = backend.get_node("gn-001", "default").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.name, "Rust");
        assert_eq!(retrieved.kind, "language");
        assert_eq!(retrieved.agent, "default");
    }

    #[tokio::test]
    async fn test_upsert_node_updates_existing() {
        let (_tmp, backend) = create_test_backend();
        let node = make_test_node("gn-001", "Rust", "language");
        backend.upsert_node(&node, "default").await.unwrap();

        // Update name and add aliases
        let mut updated_node = node.clone();
        updated_node.name = "Rust Lang".to_string();
        updated_node.aliases = vec!["rust-lang".to_string(), "rustlang".to_string()];
        backend.upsert_node(&updated_node, "default").await.unwrap();

        let retrieved = backend
            .get_node("gn-001", "default")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.name, "Rust Lang");
        assert_eq!(
            retrieved.aliases,
            vec!["rust-lang".to_string(), "rustlang".to_string()]
        );
    }

    #[tokio::test]
    async fn test_upsert_and_get_edges() {
        let (_tmp, backend) = create_test_backend();
        let edge = make_test_edge("ge-001", "gn-001", "gn-002", "uses", "app:test");

        backend.upsert_edge(&edge, "default").await.unwrap();

        let edges = backend
            .get_edges_for_node("gn-001", None, "default")
            .await
            .unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].id, "ge-001");
        assert_eq!(edges[0].relation, "uses");
        assert_eq!(edges[0].from_id, "gn-001");
        assert_eq!(edges[0].to_id, "gn-002");

        // Also retrievable via to_id
        let edges_to = backend
            .get_edges_for_node("gn-002", None, "default")
            .await
            .unwrap();
        assert_eq!(edges_to.len(), 1);
        assert_eq!(edges_to[0].id, "ge-001");
    }

    #[tokio::test]
    async fn test_resolve_entity() {
        let (_tmp, backend) = create_test_backend();

        // Insert nodes with aliases
        let mut node1 = make_test_node("gn-001", "Rust", "language");
        node1.aliases = vec!["rust-lang".to_string(), "Rust Language".to_string()];

        let mut node2 = make_test_node("gn-002", "Python", "language");
        node2.aliases = vec!["py".to_string()];

        backend.upsert_node(&node1, "default").await.unwrap();
        backend.upsert_node(&node2, "default").await.unwrap();

        // Resolve by exact name (case-insensitive)
        let results = backend
            .resolve_entity("rust", None, "default")
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].node_id, "gn-001");
        assert!(!results[0].ambiguous);

        // Resolve by alias
        let results = backend
            .resolve_entity("py", None, "default")
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].node_id, "gn-002");

        // Resolve non-existent
        let results = backend
            .resolve_entity("Java", None, "default")
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_resolve_entity_with_context() {
        let (_tmp, backend) = create_test_backend();

        let node1 = make_test_node("gn-001", "Rust", "language");
        let node2 = make_test_node("gn-002", "Rust", "game");

        backend.upsert_node(&node1, "default").await.unwrap();
        backend.upsert_node(&node2, "default").await.unwrap();

        // Add edges in context for node2 to make it rank higher
        let edge1 = make_test_edge("ge-001", "gn-002", "gn-003", "played_by", "gaming");
        let edge2 = make_test_edge("ge-002", "gn-002", "gn-004", "sequel_of", "gaming");
        backend.upsert_edge(&edge1, "default").await.unwrap();
        backend.upsert_edge(&edge2, "default").await.unwrap();

        let results = backend
            .resolve_entity("Rust", Some("gaming"), "default")
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert!(results[0].ambiguous);
        // node2 (game) should rank first due to context edges
        assert_eq!(results[0].node_id, "gn-002");
        assert_eq!(results[0].context_score, 2.0);
    }

    #[tokio::test]
    async fn test_apply_decay() {
        let (_tmp, backend) = create_test_backend();

        // Create a node that was last updated a long time ago (should be pruned)
        let mut old_node = make_test_node("gn-old", "OldEntity", "concept");
        old_node.decay_score = 0.15;
        old_node.updated_at = 1600000000; // far in the past

        // Create a recent node (should survive)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let mut recent_node = make_test_node("gn-recent", "RecentEntity", "concept");
        recent_node.decay_score = 1.0;
        recent_node.updated_at = now;

        backend.upsert_node(&old_node, "default").await.unwrap();
        backend.upsert_node(&recent_node, "default").await.unwrap();

        // Create an old edge (should be pruned)
        let mut old_edge = make_test_edge("ge-old", "gn-old", "gn-recent", "mentions", "ctx");
        old_edge.decay_score = 0.15;
        old_edge.updated_at = 1600000000;

        backend.upsert_edge(&old_edge, "default").await.unwrap();

        let policy = GraphDecayPolicy {
            node_decay_per_day: 0.02,
            edge_decay_per_day: 0.03,
            min_score: 0.1,
        };

        let stats = backend.apply_decay(&policy, "default").await.unwrap();

        // Old node and edge should have been pruned
        assert!(
            stats.nodes_pruned >= 1,
            "expected at least 1 node pruned, got {}",
            stats.nodes_pruned
        );
        assert!(
            stats.edges_pruned >= 1,
            "expected at least 1 edge pruned, got {}",
            stats.edges_pruned
        );

        // Old node should be gone
        assert!(backend
            .get_node("gn-old", "default")
            .await
            .unwrap()
            .is_none());

        // Recent node should still exist
        assert!(backend
            .get_node("gn-recent", "default")
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn test_link_and_get_nodes_for_fact() {
        let (_tmp, backend) = create_test_backend();
        let node = make_test_node("gn-001", "Rust", "language");
        backend.upsert_node(&node, "default").await.unwrap();

        backend
            .link_memory_entity("fact-001", "gn-001", 0.8, "extracted", "default")
            .await
            .unwrap();

        let nodes = backend.get_nodes_for_fact("fact-001", "default").await.unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].0.id, "gn-001");
        assert!((nodes[0].1 - 0.8).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_get_facts_for_node() {
        let (_tmp, backend) = create_test_backend();
        let node = make_test_node("gn-001", "Rust", "language");
        backend.upsert_node(&node, "default").await.unwrap();

        backend
            .link_memory_entity("fact-001", "gn-001", 0.9, "extracted", "default")
            .await
            .unwrap();
        backend
            .link_memory_entity("fact-002", "gn-001", 0.7, "wikilink", "default")
            .await
            .unwrap();

        let facts = backend.get_facts_for_node("gn-001", "default").await.unwrap();
        assert_eq!(facts.len(), 2);
    }

    #[tokio::test]
    async fn test_link_upserts_on_duplicate() {
        let (_tmp, backend) = create_test_backend();
        let node = make_test_node("gn-001", "Rust", "language");
        backend.upsert_node(&node, "default").await.unwrap();

        backend
            .link_memory_entity("fact-001", "gn-001", 0.5, "extracted", "default")
            .await
            .unwrap();
        backend
            .link_memory_entity("fact-001", "gn-001", 0.9, "extracted", "default")
            .await
            .unwrap();

        let nodes = backend.get_nodes_for_fact("fact-001", "default").await.unwrap();
        assert_eq!(nodes.len(), 1);
        assert!((nodes[0].1 - 0.9).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_unlink_memory_entity() {
        let (_tmp, backend) = create_test_backend();
        let node = make_test_node("gn-001", "Rust", "language");
        backend.upsert_node(&node, "default").await.unwrap();

        backend
            .link_memory_entity("fact-001", "gn-001", 0.8, "extracted", "default")
            .await
            .unwrap();
        backend
            .unlink_memory_entity("fact-001", "gn-001", "default")
            .await
            .unwrap();

        let nodes = backend.get_nodes_for_fact("fact-001", "default").await.unwrap();
        assert!(nodes.is_empty());
    }

    #[tokio::test]
    async fn test_delete_memory_entities_for_fact() {
        let (_tmp, backend) = create_test_backend();
        let node_a = make_test_node("gn-a", "Rust", "language");
        let node_b = make_test_node("gn-b", "Python", "language");
        backend.upsert_node(&node_a, "default").await.unwrap();
        backend.upsert_node(&node_b, "default").await.unwrap();

        backend.link_memory_entity("fact-001", "gn-a", 0.8, "extracted", "default").await.unwrap();
        backend.link_memory_entity("fact-001", "gn-b", 0.7, "wikilink", "default").await.unwrap();

        let deleted = backend.delete_memory_entities_for_fact("fact-001", "default").await.unwrap();
        assert_eq!(deleted, 2);

        let nodes = backend.get_nodes_for_fact("fact-001", "default").await.unwrap();
        assert!(nodes.is_empty());
    }

    #[tokio::test]
    async fn test_delete_memory_entities_for_node() {
        let (_tmp, backend) = create_test_backend();
        let node = make_test_node("gn-001", "Rust", "language");
        backend.upsert_node(&node, "default").await.unwrap();

        backend.link_memory_entity("fact-001", "gn-001", 0.8, "extracted", "default").await.unwrap();
        backend.link_memory_entity("fact-002", "gn-001", 0.7, "extracted", "default").await.unwrap();

        let deleted = backend.delete_memory_entities_for_node("gn-001", "default").await.unwrap();
        assert_eq!(deleted, 2);

        let facts = backend.get_facts_for_node("gn-001", "default").await.unwrap();
        assert!(facts.is_empty());
    }

    #[test]
    fn edge_count_for_entities_empty_returns_empty() {
        let (_tmp, backend) = create_test_backend();
        let result = backend.edge_count_for_entities(&[], "default").unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn edge_count_for_entities_returns_counts() {
        let (_tmp, backend) = create_test_backend();

        // Insert three nodes
        let node_a = make_test_node("gn-a", "Alice", "person");
        let node_b = make_test_node("gn-b", "Bob", "person");
        let node_c = make_test_node("gn-c", "Carol", "person");
        backend.upsert_node(&node_a, "default").await.unwrap();
        backend.upsert_node(&node_b, "default").await.unwrap();
        backend.upsert_node(&node_c, "default").await.unwrap();

        // Alice --knows--> Bob  (edge 1)
        // Alice --knows--> Carol (edge 2)
        // Bob   --knows--> Carol (edge 3)
        let e1 = make_test_edge("ge-1", "gn-a", "gn-b", "knows", "ctx");
        let e2 = make_test_edge("ge-2", "gn-a", "gn-c", "knows", "ctx");
        let e3 = make_test_edge("ge-3", "gn-b", "gn-c", "knows", "ctx");
        backend.upsert_edge(&e1, "default").await.unwrap();
        backend.upsert_edge(&e2, "default").await.unwrap();
        backend.upsert_edge(&e3, "default").await.unwrap();

        let names = vec![
            "Alice".to_string(),
            "Bob".to_string(),
            "Carol".to_string(),
        ];
        let counts = backend.edge_count_for_entities(&names, "default").unwrap();

        // Alice: e1 (from), e2 (from) = 2
        assert_eq!(counts.get("Alice").copied().unwrap_or(0), 2);
        // Bob: e1 (to), e3 (from) = 2
        assert_eq!(counts.get("Bob").copied().unwrap_or(0), 2);
        // Carol: e2 (to), e3 (to) = 2
        assert_eq!(counts.get("Carol").copied().unwrap_or(0), 2);

        // Query a subset — only Alice
        let subset = vec!["Alice".to_string()];
        let counts2 = backend.edge_count_for_entities(&subset, "default").unwrap();
        assert_eq!(counts2.len(), 1);
        assert_eq!(counts2["Alice"], 2);

        // Query with a name that has no node — should not appear
        let unknown = vec!["Unknown".to_string()];
        let counts3 = backend
            .edge_count_for_entities(&unknown, "default")
            .unwrap();
        assert!(counts3.is_empty());
    }

    #[tokio::test]
    async fn test_get_high_score_nodes() {
        let (_tmp, backend) = create_test_backend();

        // Insert nodes with different decay scores
        let mut node1 = make_test_node("gn-high", "Rust", "language");
        node1.decay_score = 0.9;
        backend.upsert_node(&node1, "default").await.unwrap();

        let mut node2 = make_test_node("gn-low", "Python", "language");
        node2.decay_score = 0.2;
        backend.upsert_node(&node2, "default").await.unwrap();

        let mut node3 = make_test_node("gn-mid", "Go", "language");
        node3.decay_score = 0.6;
        backend.upsert_node(&node3, "default").await.unwrap();

        // Query with threshold 0.5 — should return Rust (0.9) and Go (0.6), sorted desc
        let nodes = backend.get_high_score_nodes(0.5, 10, "default").await.unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].name, "Rust");
        assert_eq!(nodes[1].name, "Go");

        // Query with limit 1 — should return only Rust
        let nodes = backend.get_high_score_nodes(0.5, 1, "default").await.unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "Rust");
    }
}
