//! Memory graph storage and resolution.
//!
//! Provides a lightweight entity-relation graph for disambiguation and filtering.

use crate::error::AlephError;
use crate::memory::context::{FactType, MemoryEntry};
use crate::memory::database::VectorDatabase;
use chrono::Utc;
use regex::Regex;
use rusqlite::OptionalExtension;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use uuid::Uuid;

/// Graph node representation.
#[derive(Debug, Clone)]
pub struct GraphNode {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub aliases: Vec<String>,
    pub metadata: Value,
    pub created_at: i64,
    pub updated_at: i64,
    pub decay_score: f32,
}

/// Graph edge representation.
#[derive(Debug, Clone)]
pub struct GraphEdge {
    pub id: String,
    pub from_id: String,
    pub to_id: String,
    pub relation: String,
    pub weight: f32,
    pub confidence: f32,
    pub context_key: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_seen_at: i64,
    pub decay_score: f32,
}

/// Result of resolving an entity name/alias.
#[derive(Debug, Clone)]
pub struct ResolvedEntity {
    pub node_id: String,
    pub score: f32,
    pub reasons: Vec<String>,
    pub ambiguous: bool,
}

/// Graph decay summary.
#[derive(Debug, Clone, Default)]
pub struct GraphDecayReport {
    pub pruned_nodes: u64,
    pub pruned_edges: u64,
}

/// Config for graph decay.
#[derive(Debug, Clone)]
pub struct GraphDecayConfig {
    pub node_decay_per_day: f32,
    pub edge_decay_per_day: f32,
    pub min_score: f32,
}

impl Default for GraphDecayConfig {
    fn default() -> Self {
        Self {
            node_decay_per_day: 0.02,
            edge_decay_per_day: 0.03,
            min_score: 0.1,
        }
    }
}

/// Graph storage wrapper.
#[derive(Clone)]
pub struct GraphStore {
    database: Arc<VectorDatabase>,
}

impl GraphStore {
    pub fn new(database: Arc<VectorDatabase>) -> Self {
        Self { database }
    }

    /// Normalize an entity or alias name for matching.
    fn normalize_name(input: &str) -> String {
        input.trim().to_lowercase()
    }

    /// Extract entities from a fact content string.
    pub fn extract_entities_from_text(text: &str) -> Vec<String> {
        let mut candidates = Vec::new();

        let quote_re = Regex::new(r#"[\"“《]([^\"”》]{2,60})[\"”》]"#).unwrap();
        for cap in quote_re.captures_iter(text) {
            if let Some(m) = cap.get(1) {
                candidates.push(m.as_str().trim().to_string());
            }
        }

        let project_re = Regex::new(r"(?i)project\s+([A-Za-z0-9_-]{2,40})").unwrap();
        for cap in project_re.captures_iter(text) {
            if let Some(m) = cap.get(1) {
                candidates.push(format!("Project {}", m.as_str().trim()));
            }
        }

        let han_project_re = Regex::new(r"项目[:：]?([\p{Han}A-Za-z0-9_-]{2,40})").unwrap();
        for cap in han_project_re.captures_iter(text) {
            if let Some(m) = cap.get(1) {
                candidates.push(format!("项目{}", m.as_str().trim()));
            }
        }

        let proper_re = Regex::new(r"\b([A-Z][a-z0-9]+(?:\s+[A-Z][a-z0-9]+)*)\b").unwrap();
        let stopwords = ["The", "A", "An", "And", "Or", "But", "User"];
        for cap in proper_re.captures_iter(text) {
            if let Some(m) = cap.get(1) {
                let name = m.as_str().trim();
                if stopwords.contains(&name) {
                    continue;
                }
                if name.len() >= 2 {
                    candidates.push(name.to_string());
                }
            }
        }

        // Fallback: use truncated content as a concept node
        if candidates.is_empty() {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                let mut fallback = trimmed.chars().take(40).collect::<String>();
                if trimmed.chars().count() > 40 {
                    fallback.push('…');
                }
                candidates.push(fallback);
            }
        }

        // Deduplicate while preserving order
        let mut seen = HashSet::new();
        candidates
            .into_iter()
            .filter(|c| seen.insert(Self::normalize_name(c)))
            .collect()
    }

    /// Extract explicit entity hints from a query (e.g. @Entity, #Entity, entity:Entity).
    pub fn extract_query_hints(query: &str) -> Vec<String> {
        let mut hints = Vec::new();
        let tag_re = Regex::new(r"[@#]([A-Za-z0-9_\-\p{Han}]{2,40})").unwrap();
        for cap in tag_re.captures_iter(query) {
            if let Some(m) = cap.get(1) {
                hints.push(m.as_str().trim().to_string());
            }
        }

        let explicit_re = Regex::new(r"(?i)entity\s*[:：]\s*([^\s,;]+)").unwrap();
        for cap in explicit_re.captures_iter(query) {
            if let Some(m) = cap.get(1) {
                hints.push(m.as_str().trim().to_string());
            }
        }

        let mut seen = HashSet::new();
        hints
            .into_iter()
            .filter(|h| seen.insert(Self::normalize_name(h)))
            .collect()
    }

    /// Upsert a graph node by kind + normalized name.
    pub async fn upsert_node(
        &self,
        name: &str,
        kind: &str,
        aliases: &[String],
        metadata: Option<Value>,
    ) -> Result<GraphNode, AlephError> {
        let conn = self.database.conn.lock().unwrap_or_else(|e| e.into_inner());
        let normalized = Self::normalize_name(name);
        let now = Utc::now().timestamp();

        let mut stmt = conn
            .prepare(
                r#"
                SELECT id, name, kind, aliases_json, metadata_json, created_at, updated_at, decay_score
                FROM graph_nodes
                WHERE kind = ?1 AND lower(name) = ?2
                LIMIT 1
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare graph node lookup: {}", e)))?;

        let existing: Option<(String, String, String, String, String, i64, i64, f64)> = stmt
            .query_row([kind, normalized.as_str()], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            })
            .optional()
            .map_err(|e| AlephError::config(format!("Failed to query graph node: {}", e)))?;

        let mut alias_set: HashSet<String> = HashSet::new();
        let mut alias_list: Vec<String> = Vec::new();
        let metadata_is_none = metadata.is_none();
        let mut metadata_value = metadata.unwrap_or_else(|| Value::Object(Default::default()));
        let node_id = if let Some((id, existing_name, _, aliases_json, metadata_json, created_at, _, decay_score)) =
            existing
        {
            if let Ok(existing_aliases) = serde_json::from_str::<Vec<String>>(&aliases_json) {
                for alias in existing_aliases {
                    let normalized = Self::normalize_name(&alias);
                    if alias_set.insert(normalized) {
                        alias_list.push(alias);
                    }
                }
            }

            if metadata_is_none {
                if let Ok(value) = serde_json::from_str::<Value>(&metadata_json) {
                    metadata_value = value;
                }
            }

            for alias in aliases.iter().cloned() {
                let normalized = Self::normalize_name(&alias);
                if alias_set.insert(normalized) {
                    alias_list.push(alias);
                }
            }

            let aliases_json = serde_json::to_string(&alias_list)
                .map_err(|e| AlephError::config(format!("Failed to serialize aliases: {}", e)))?;
            let metadata_json = serde_json::to_string(&metadata_value).map_err(|e| {
                AlephError::config(format!("Failed to serialize metadata: {}", e))
            })?;

            conn.execute(
                r#"
                UPDATE graph_nodes
                SET aliases_json = ?1, metadata_json = ?2, updated_at = ?3
                WHERE id = ?4
                "#,
                rusqlite::params![aliases_json, metadata_json, now, id],
            )
            .map_err(|e| AlephError::config(format!("Failed to update graph node: {}", e)))?;

            // Ensure canonical name is included in aliases list for lookup
            let canonical_norm = Self::normalize_name(&existing_name);
            if alias_set.insert(canonical_norm) {
                alias_list.push(existing_name.clone());
            }

            Self::insert_aliases(&conn, &id, &alias_list)?;

            return Ok(GraphNode {
                id,
                name: existing_name,
                kind: kind.to_string(),
                aliases: alias_list,
                metadata: metadata_value,
                created_at,
                updated_at: now,
                decay_score: decay_score as f32,
            });
        } else {
            let id = format!("gn_{}", Uuid::new_v4());
            alias_list = aliases.to_vec();
            let aliases_json = serde_json::to_string(&alias_list)
                .map_err(|e| AlephError::config(format!("Failed to serialize aliases: {}", e)))?;
            let metadata_json = serde_json::to_string(&metadata_value).map_err(|e| {
                AlephError::config(format!("Failed to serialize metadata: {}", e))
            })?;

            conn.execute(
                r#"
                INSERT INTO graph_nodes (id, name, kind, aliases_json, metadata_json, created_at, updated_at, decay_score)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1.0)
                "#,
                rusqlite::params![id, name, kind, aliases_json, metadata_json, now, now],
            )
            .map_err(|e| AlephError::config(format!("Failed to insert graph node: {}", e)))?;

            let mut alias_full = alias_list.clone();
            alias_full.push(name.to_string());
            Self::insert_aliases(&conn, &id, &alias_full)?;
            id
        };

        Ok(GraphNode {
            id: node_id,
            name: name.to_string(),
            kind: kind.to_string(),
            aliases: alias_list,
            metadata: metadata_value,
            created_at: now,
            updated_at: now,
            decay_score: 1.0,
        })
    }

    fn insert_aliases(
        conn: &rusqlite::Connection,
        node_id: &str,
        aliases: &[String],
    ) -> Result<(), AlephError> {
        let mut stmt = conn
            .prepare(
                r#"
                INSERT OR IGNORE INTO graph_aliases (alias, normalized_alias, node_id)
                VALUES (?1, ?2, ?3)
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare alias insert: {}", e)))?;

        for alias in aliases {
            let normalized = Self::normalize_name(alias);
            stmt.execute(rusqlite::params![alias, normalized, node_id])
                .map_err(|e| {
                    AlephError::config(format!("Failed to insert graph alias: {}", e))
                })?;
        }
        Ok(())
    }

    /// Upsert a graph edge and return the updated edge.
    pub async fn upsert_edge(
        &self,
        from_id: &str,
        to_id: &str,
        relation: &str,
        context_key: &str,
        confidence: f32,
        weight_delta: f32,
    ) -> Result<GraphEdge, AlephError> {
        let conn = self.database.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = Utc::now().timestamp();

        let existing: Option<(String, f32, f32, i64, f32)> = conn
            .query_row(
                r#"
                SELECT id, weight, confidence, last_seen_at, decay_score
                FROM graph_edges
                WHERE from_id = ?1 AND to_id = ?2 AND relation = ?3 AND context_key = ?4
                LIMIT 1
                "#,
                rusqlite::params![from_id, to_id, relation, context_key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .optional()
            .map_err(|e| AlephError::config(format!("Failed to query graph edge: {}", e)))?;

        if let Some((id, weight, existing_conf, _, decay_score)) = existing {
            let new_weight = weight + weight_delta;
            let new_conf = existing_conf.max(confidence);
            conn.execute(
                r#"
                UPDATE graph_edges
                SET weight = ?1, confidence = ?2, updated_at = ?3, last_seen_at = ?4
                WHERE id = ?5
                "#,
                rusqlite::params![new_weight, new_conf, now, now, id],
            )
            .map_err(|e| AlephError::config(format!("Failed to update graph edge: {}", e)))?;

            return Ok(GraphEdge {
                id,
                from_id: from_id.to_string(),
                to_id: to_id.to_string(),
                relation: relation.to_string(),
                weight: new_weight,
                confidence: new_conf,
                context_key: context_key.to_string(),
                created_at: now,
                updated_at: now,
                last_seen_at: now,
                decay_score,
            });
        }

        let id = format!("ge_{}", Uuid::new_v4());
        conn.execute(
            r#"
            INSERT INTO graph_edges (
                id, from_id, to_id, relation, weight, confidence, context_key,
                created_at, updated_at, last_seen_at, decay_score
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1.0)
            "#,
            rusqlite::params![
                id,
                from_id,
                to_id,
                relation,
                weight_delta,
                confidence,
                context_key,
                now,
                now,
                now
            ],
        )
        .map_err(|e| AlephError::config(format!("Failed to insert graph edge: {}", e)))?;

        Ok(GraphEdge {
            id,
            from_id: from_id.to_string(),
            to_id: to_id.to_string(),
            relation: relation.to_string(),
            weight: weight_delta,
            confidence,
            context_key: context_key.to_string(),
            created_at: now,
            updated_at: now,
            last_seen_at: now,
            decay_score: 1.0,
        })
    }

    /// Link a memory to a graph entity.
    pub async fn link_memory_entity(
        &self,
        memory_id: &str,
        node_id: &str,
        weight: f32,
        source: &str,
    ) -> Result<(), AlephError> {
        let conn = self.database.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            r#"
            INSERT OR REPLACE INTO memory_entities (memory_id, node_id, weight, source)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            rusqlite::params![memory_id, node_id, weight, source],
        )
        .map_err(|e| AlephError::config(format!("Failed to link memory entity: {}", e)))?;
        Ok(())
    }

    /// Resolve entities by name or alias and optionally disambiguate with context_key.
    pub async fn resolve_entity(
        &self,
        name_or_alias: &str,
        context_key: Option<&str>,
    ) -> Result<Vec<ResolvedEntity>, AlephError> {
        let conn = self.database.conn.lock().unwrap_or_else(|e| e.into_inner());
        let normalized = Self::normalize_name(name_or_alias);

        let mut stmt = conn
            .prepare(
                r#"
                SELECT DISTINCT n.id, n.name, n.kind, n.aliases_json, n.metadata_json,
                    n.created_at, n.updated_at, n.decay_score,
                    CASE WHEN a.normalized_alias IS NOT NULL THEN 1 ELSE 0 END AS alias_match
                FROM graph_nodes n
                LEFT JOIN graph_aliases a
                    ON a.node_id = n.id AND a.normalized_alias = ?1
                WHERE lower(n.name) = ?1 OR a.normalized_alias = ?1
                "#,
            )
            .map_err(|e| AlephError::config(format!("Failed to prepare resolve query: {}", e)))?;

        let mut raw: Vec<(String, bool, i64, f32)> = Vec::new();
        let rows = stmt
            .query_map([normalized.as_str()], |row| {
                let id: String = row.get(0)?;
                let alias_match: i64 = row.get(8)?;
                let updated_at: i64 = row.get(6)?;
                let decay_score: f32 = row.get(7)?;
                Ok((id, alias_match > 0, updated_at, decay_score))
            })
            .map_err(|e| AlephError::config(format!("Failed to resolve entities: {}", e)))?;

        for row in rows {
            raw.push(row.map_err(|e| AlephError::config(format!("Failed to parse entity: {}", e)))?);
        }

        if raw.is_empty() {
            return Ok(Vec::new());
        }

        let now = Utc::now().timestamp();
        let mut resolved = Vec::new();

        for (node_id, alias_match, updated_at, _decay_score) in raw {
            let mut score: f32 = if alias_match { 0.6 } else { 0.4 };
            let mut reasons = Vec::new();
            if alias_match {
                reasons.push("alias_match".to_string());
            } else {
                reasons.push("name_match".to_string());
            }

            if let Some(context_key) = context_key {
                let context_matches: i64 = conn
                    .query_row(
                        r#"
                        SELECT COUNT(*)
                        FROM graph_edges
                        WHERE context_key = ?1 AND (from_id = ?2 OR to_id = ?2)
                        "#,
                        rusqlite::params![context_key, node_id],
                        |row| row.get(0),
                    )
                    .unwrap_or(0);

                if context_matches > 0 {
                    score += 0.25;
                    reasons.push("context_match".to_string());
                }
            }

            if now - updated_at <= 30 * 86400 {
                score += 0.1;
                reasons.push("recent_activity".to_string());
            }

            resolved.push(ResolvedEntity {
                node_id,
                score: score.min(1.0),
                reasons,
                ambiguous: false,
            });
        }

        resolved.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        let ambiguous = if resolved.len() > 1 {
            (resolved[0].score - resolved[1].score).abs() < 0.15
        } else {
            false
        };

        for entry in &mut resolved {
            entry.ambiguous = ambiguous;
        }

        Ok(resolved)
    }

    /// Get memory IDs linked to an entity.
    pub async fn get_memory_ids_for_entity(
        &self,
        node_id: &str,
    ) -> Result<Vec<String>, AlephError> {
        let conn = self.database.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare("SELECT memory_id FROM memory_entities WHERE node_id = ?1")
            .map_err(|e| AlephError::config(format!("Failed to prepare memory lookup: {}", e)))?;
        let ids = stmt
            .query_map([node_id], |row| row.get(0))
            .map_err(|e| AlephError::config(format!("Failed to query memory ids: {}", e)))?
            .collect::<Result<Vec<String>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to parse memory ids: {}", e)))?;
        Ok(ids)
    }

    /// Apply decay to nodes and edges and prune below threshold.
    pub async fn apply_decay(
        &self,
        config: &GraphDecayConfig,
    ) -> Result<GraphDecayReport, AlephError> {
        let conn = self.database.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = Utc::now().timestamp();
        let mut report = GraphDecayReport::default();

        // Decay edges
        let mut edge_stmt = conn
            .prepare("SELECT id, last_seen_at, decay_score FROM graph_edges")
            .map_err(|e| AlephError::config(format!("Failed to prepare edge decay: {}", e)))?;
        let edges = edge_stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, f32>(2)?)))
            .map_err(|e| AlephError::config(format!("Failed to query edges: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to parse edges: {}", e)))?;

        for (edge_id, last_seen, decay_score) in edges {
            let days = ((now - last_seen).max(0) as f32) / 86400.0;
            let mut new_score = decay_score * (1.0 - config.edge_decay_per_day).powf(days);
            if new_score < config.min_score {
                conn.execute("DELETE FROM graph_edges WHERE id = ?1", rusqlite::params![edge_id])
                    .map_err(|e| AlephError::config(format!("Failed to prune edge: {}", e)))?;
                report.pruned_edges += 1;
            } else {
                if new_score > 1.0 {
                    new_score = 1.0;
                }
                conn.execute(
                    "UPDATE graph_edges SET decay_score = ?1, updated_at = ?2 WHERE id = ?3",
                    rusqlite::params![new_score, now, edge_id],
                )
                .map_err(|e| AlephError::config(format!("Failed to update edge decay: {}", e)))?;
            }
        }

        // Decay nodes
        let mut node_stmt = conn
            .prepare("SELECT id, updated_at, decay_score FROM graph_nodes")
            .map_err(|e| AlephError::config(format!("Failed to prepare node decay: {}", e)))?;
        let nodes = node_stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, f32>(2)?)))
            .map_err(|e| AlephError::config(format!("Failed to query nodes: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to parse nodes: {}", e)))?;

        for (node_id, updated_at, decay_score) in nodes {
            let days = ((now - updated_at).max(0) as f32) / 86400.0;
            let mut new_score = decay_score * (1.0 - config.node_decay_per_day).powf(days);
            if new_score < config.min_score {
                conn.execute("DELETE FROM graph_nodes WHERE id = ?1", rusqlite::params![node_id.clone()])
                    .map_err(|e| AlephError::config(format!("Failed to prune node: {}", e)))?;
                conn.execute("DELETE FROM graph_aliases WHERE node_id = ?1", rusqlite::params![node_id.clone()])
                    .map_err(|e| AlephError::config(format!("Failed to prune node aliases: {}", e)))?;
                conn.execute("DELETE FROM memory_entities WHERE node_id = ?1", rusqlite::params![node_id.clone()])
                    .map_err(|e| AlephError::config(format!("Failed to prune node links: {}", e)))?;
                conn.execute("DELETE FROM graph_edges WHERE from_id = ?1 OR to_id = ?1", rusqlite::params![node_id])
                    .map_err(|e| AlephError::config(format!("Failed to prune node edges: {}", e)))?;
                report.pruned_nodes += 1;
            } else {
                if new_score > 1.0 {
                    new_score = 1.0;
                }
                conn.execute(
                    "UPDATE graph_nodes SET decay_score = ?1, updated_at = ?2 WHERE id = ?3",
                    rusqlite::params![new_score, now, node_id],
                )
                .map_err(|e| AlephError::config(format!("Failed to update node decay: {}", e)))?;
            }
        }

        Ok(report)
    }

    /// Update graph from a compressed fact.
    pub async fn update_from_fact(
        &self,
        fact: &crate::memory::context::MemoryFact,
        memories: &[MemoryEntry],
    ) -> Result<(), AlephError> {
        let mut context_map: HashMap<String, String> = HashMap::new();
        for memory in memories {
            let key = format!("app:{}|window:{}", memory.context.app_bundle_id, memory.context.window_title);
            context_map.insert(memory.id.clone(), key);
        }

        let entity_names = Self::extract_entities_from_text(&fact.content);
        if entity_names.is_empty() {
            return Ok(());
        }

        let mut node_ids = Vec::new();
        for name in entity_names {
            let metadata = serde_json::json!({
                "fact_type": fact.fact_type.as_str(),
            });
            let node = self
                .upsert_node(&name, fact.fact_type.as_str(), &[], Some(metadata))
                .await?;
            node_ids.push(node.id);
        }

        for memory_id in &fact.source_memory_ids {
            for node_id in &node_ids {
                self.link_memory_entity(memory_id, node_id, 1.0, "fact")
                    .await?;
            }
        }

        // Create co-occurrence edges
        let context_key = fact
            .source_memory_ids.first()
            .and_then(|id| context_map.get(id))
            .cloned()
            .unwrap_or_default();

        for i in 0..node_ids.len() {
            for j in (i + 1)..node_ids.len() {
                let _ = self
                    .upsert_edge(
                        &node_ids[i],
                        &node_ids[j],
                        "co_occurs",
                        &context_key,
                        0.6,
                        1.0,
                    )
                    .await?;
            }
        }

        Ok(())
    }
}

/// Map FactType to a graph kind (simple mapping).
pub fn graph_kind_for_fact_type(fact_type: &FactType) -> String {
    fact_type.as_str().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::ContextAnchor;
    use crate::memory::database::VectorDatabase;
    fn create_db() -> Arc<VectorDatabase> {
        let dir = std::env::temp_dir();
        let db_path = dir.join(format!("graph_test_{}.db", Uuid::new_v4()));
        Arc::new(VectorDatabase::new(db_path).unwrap())
    }

    #[tokio::test]
    async fn test_upsert_and_resolve_alias() {
        let db = create_db();
        let store = GraphStore::new(db);

        let _node = store
            .upsert_node(
                "Zhang",
                "person",
                &vec!["Lao Zhang".to_string()],
                None,
            )
            .await
            .unwrap();

        let resolved = store.resolve_entity("Lao Zhang", None).await.unwrap();
        assert_eq!(resolved.len(), 1);
        assert!(!resolved[0].ambiguous);
        assert!(resolved[0].score > 0.5);
    }

    #[tokio::test]
    async fn test_link_memory_entity() {
        let db = create_db();
        let store = GraphStore::new(Arc::clone(&db));

        let context = ContextAnchor::now("com.test.app".to_string(), "Doc.txt".to_string());
        let memory = MemoryEntry::with_embedding(
            "mem-1".to_string(),
            context,
            "input".to_string(),
            "output".to_string(),
            vec![0.1; crate::memory::EMBEDDING_DIM],
        );
        db.insert_memory(memory).await.unwrap();

        let node = store
            .upsert_node("Entity", "concept", &[], None)
            .await
            .unwrap();

        store
            .link_memory_entity("mem-1", &node.id, 1.0, "test")
            .await
            .unwrap();

        let ids = store.get_memory_ids_for_entity(&node.id).await.unwrap();
        assert_eq!(ids, vec!["mem-1".to_string()]);
    }
}
