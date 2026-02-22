//! Memory graph storage and resolution.
//!
//! Provides a lightweight entity-relation graph for disambiguation and filtering.
//! This module wraps MemoryBackend and delegates to the GraphStore trait.

use crate::error::AlephError;
use crate::memory::context::{FactType, MemoryEntry};
use crate::memory::store::{self, MemoryBackend};
use crate::memory::store::GraphStore as StoreGraphStore;
use chrono::Utc;
use regex::Regex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// Graph node representation (local type, bridges to store::GraphNode).
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

/// Graph edge representation (local type, bridges to store::GraphEdge).
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

// -- Conversion helpers between local types and store types --

impl From<store::GraphNode> for GraphNode {
    fn from(n: store::GraphNode) -> Self {
        let metadata = serde_json::from_str(&n.metadata_json).unwrap_or(Value::Object(Default::default()));
        Self {
            id: n.id,
            name: n.name,
            kind: n.kind,
            aliases: n.aliases,
            metadata,
            created_at: n.created_at,
            updated_at: n.updated_at,
            decay_score: n.decay_score,
        }
    }
}

impl From<&GraphNode> for store::GraphNode {
    fn from(n: &GraphNode) -> Self {
        let metadata_json = serde_json::to_string(&n.metadata).unwrap_or_else(|_| "{}".to_string());
        Self {
            id: n.id.clone(),
            name: n.name.clone(),
            kind: n.kind.clone(),
            aliases: n.aliases.clone(),
            metadata_json,
            decay_score: n.decay_score,
            created_at: n.created_at,
            updated_at: n.updated_at,
            workspace: "default".to_string(),
        }
    }
}

impl From<store::GraphEdge> for GraphEdge {
    fn from(e: store::GraphEdge) -> Self {
        Self {
            id: e.id,
            from_id: e.from_id,
            to_id: e.to_id,
            relation: e.relation,
            weight: e.weight,
            confidence: e.confidence,
            context_key: e.context_key,
            created_at: e.created_at,
            updated_at: e.updated_at,
            last_seen_at: e.last_seen_at,
            decay_score: e.decay_score,
        }
    }
}

impl From<&GraphEdge> for store::GraphEdge {
    fn from(e: &GraphEdge) -> Self {
        Self {
            id: e.id.clone(),
            from_id: e.from_id.clone(),
            to_id: e.to_id.clone(),
            relation: e.relation.clone(),
            weight: e.weight,
            confidence: e.confidence,
            context_key: e.context_key.clone(),
            created_at: e.created_at,
            updated_at: e.updated_at,
            last_seen_at: e.last_seen_at,
            decay_score: e.decay_score,
            workspace: "default".to_string(),
        }
    }
}

/// Graph storage wrapper that delegates to MemoryBackend via GraphStore trait.
#[derive(Clone)]
pub struct GraphStore {
    database: MemoryBackend,
}

impl GraphStore {
    pub fn new(database: MemoryBackend) -> Self {
        Self { database }
    }

    /// Normalize an entity or alias name for matching.
    fn normalize_name(input: &str) -> String {
        input.trim().to_lowercase()
    }

    /// Extract entities from a fact content string.
    pub fn extract_entities_from_text(text: &str) -> Vec<String> {
        let mut candidates = Vec::new();

        let quote_re = Regex::new(r#"[\"\u{201C}《]([^\"\u{201D}》]{2,60})[\"\u{201D}》]"#).unwrap();
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
        let now = Utc::now().timestamp();
        let metadata_value = metadata.unwrap_or_else(|| Value::Object(Default::default()));
        let metadata_json = serde_json::to_string(&metadata_value)
            .map_err(|e| AlephError::config(format!("Failed to serialize metadata: {}", e)))?;

        // Try to find existing node by resolving entity
        let existing = StoreGraphStore::resolve_entity(
            self.database.as_ref(),
            name,
            None,
            "default",
        ).await?;

        let node_id = if let Some(resolved) = existing.first() {
            // Update existing node
            let mut existing_node = StoreGraphStore::get_node(
                self.database.as_ref(),
                &resolved.node_id,
                "default",
            ).await?.unwrap_or_else(|| store::GraphNode {
                id: resolved.node_id.clone(),
                name: name.to_string(),
                kind: kind.to_string(),
                aliases: aliases.to_vec(),
                metadata_json: metadata_json.clone(),
                decay_score: 1.0,
                created_at: now,
                updated_at: now,
            workspace: "default".to_string(),
            });

            // Merge aliases
            let mut alias_set: HashSet<String> = existing_node.aliases.iter()
                .map(|a| Self::normalize_name(a))
                .collect();
            for alias in aliases {
                let norm = Self::normalize_name(alias);
                if alias_set.insert(norm) {
                    existing_node.aliases.push(alias.clone());
                }
            }

            existing_node.metadata_json = metadata_json.clone();
            existing_node.updated_at = now;

            StoreGraphStore::upsert_node(self.database.as_ref(), &existing_node, "default").await?;

            GraphNode::from(existing_node)
        } else {
            // Create new node
            let id = format!("gn_{}", Uuid::new_v4());
            let store_node = store::GraphNode {
                id: id.clone(),
                name: name.to_string(),
                kind: kind.to_string(),
                aliases: aliases.to_vec(),
                metadata_json,
                decay_score: 1.0,
                created_at: now,
                updated_at: now,
            workspace: "default".to_string(),
            };

            StoreGraphStore::upsert_node(self.database.as_ref(), &store_node, "default").await?;

            GraphNode {
                id,
                name: name.to_string(),
                kind: kind.to_string(),
                aliases: aliases.to_vec(),
                metadata: metadata_value,
                created_at: now,
                updated_at: now,
                decay_score: 1.0,
            }
        };

        Ok(node_id)
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
        let now = Utc::now().timestamp();
        let id = format!("ge_{}", Uuid::new_v4());

        let store_edge = store::GraphEdge {
            id: id.clone(),
            from_id: from_id.to_string(),
            to_id: to_id.to_string(),
            relation: relation.to_string(),
            weight: weight_delta,
            confidence,
            context_key: context_key.to_string(),
            decay_score: 1.0,
            created_at: now,
            updated_at: now,
            last_seen_at: now,
            workspace: "default".to_string(),
        };

        StoreGraphStore::upsert_edge(self.database.as_ref(), &store_edge, "default").await?;

        Ok(GraphEdge::from(store_edge))
    }

    /// Link a memory to a graph entity.
    ///
    /// TODO: Implement via MemoryBackend when memory_entities support is added.
    pub async fn link_memory_entity(
        &self,
        _memory_id: &str,
        _node_id: &str,
        _weight: f32,
        _source: &str,
    ) -> Result<(), AlephError> {
        // TODO: Delegate to store trait when memory_entities table is available in Lance
        Ok(())
    }

    /// Resolve entities by name or alias.
    pub async fn resolve_entity(
        &self,
        name_or_alias: &str,
        context_key: Option<&str>,
    ) -> Result<Vec<ResolvedEntity>, AlephError> {
        let resolved = StoreGraphStore::resolve_entity(
            self.database.as_ref(),
            name_or_alias,
            context_key,
            "default",
        ).await?;

        Ok(resolved.into_iter().map(|r| {
            ResolvedEntity {
                node_id: r.node_id,
                score: r.context_score,
                reasons: vec![],
                ambiguous: r.ambiguous,
            }
        }).collect())
    }

    /// Get memory IDs linked to an entity.
    ///
    /// TODO: Implement via MemoryBackend when memory_entities support is added.
    pub async fn get_memory_ids_for_entity(
        &self,
        _node_id: &str,
    ) -> Result<Vec<String>, AlephError> {
        // TODO: Delegate to store trait when memory_entities table is available in Lance
        Ok(Vec::new())
    }

    /// Apply decay to nodes and edges and prune below threshold.
    pub async fn apply_decay(
        &self,
        config: &GraphDecayConfig,
    ) -> Result<GraphDecayReport, AlephError> {
        let policy = crate::config::types::memory::GraphDecayPolicy {
            node_decay_per_day: config.node_decay_per_day,
            edge_decay_per_day: config.edge_decay_per_day,
            min_score: config.min_score,
        };

        let stats = StoreGraphStore::apply_decay(self.database.as_ref(), &policy, "default").await?;

        Ok(GraphDecayReport {
            pruned_nodes: stats.nodes_pruned as u64,
            pruned_edges: stats.edges_pruned as u64,
        })
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

    // TODO: Tests need to be rewritten to use LanceMemoryBackend
    // The old tests used StateDatabase with SQLite
}
