//! Storage primitive for VESR (Verified-Experience Self-Routing) routing experiences.
//!
//! Provides `record_routing_experience` and `recall_routing_experience` methods on
//! `SqliteMemoryBackend`, backed by the `routing_experiences` relational table and
//! the `routing_exp_vec_{768,1024,1536}` sqlite-vec virtual tables.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use crate::error::AlephError;
use super::SqliteMemoryBackend;
use super::vec;

#[derive(Debug, Clone)]
pub struct RoutingExperienceRow {
    pub id: String,
    pub agent_id: String,
    pub model_id: String,
    pub provider_id: String,
    pub terminate_reason: String,
    pub iterations: i64,
    pub tool_calls: i64,
    pub tool_error_count: i64,
    pub tool_call_total: i64,
    pub tok_input: i64,
    pub tok_output: i64,
    pub tok_cache_read: i64,
    pub tok_cache_creation: i64,
    pub tok_reasoning: i64,
    pub estimated_cost: Option<f64>,
    pub duration_ms: i64,
    pub context_tokens: i64,
    pub context_window: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoutingNeighbor {
    pub id: String,
    pub agent_id: String,
    pub model_id: String,
    pub provider_id: String,
    pub terminate_reason: String,
    pub iterations: i64,
    pub tool_calls: i64,
    pub tool_error_count: i64,
    pub tool_call_total: i64,
    pub tok_input: i64,
    pub tok_output: i64,
    pub tok_cache_read: i64,
    pub tok_cache_creation: i64,
    pub tok_reasoning: i64,
    pub estimated_cost: Option<f64>,
    pub duration_ms: i64,
    pub context_tokens: i64,
    pub context_window: i64,
    pub created_at: i64,
    pub distance: f32,
}

impl SqliteMemoryBackend {
    pub fn record_routing_experience(
        &self,
        row: &RoutingExperienceRow,
        embedding: &[f32],
        dim: u32,
    ) -> Result<(), AlephError> {
        let table = vec::routing_exp_vec_table_for_dim(dim)?;
        let blob = vec::embedding_to_blob(embedding);
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        conn.execute(
            "INSERT INTO routing_experiences \
             (id, agent_id, model_id, provider_id, terminate_reason, iterations, tool_calls, \
              tool_error_count, tool_call_total, tok_input, tok_output, tok_cache_read, \
              tok_cache_creation, tok_reasoning, estimated_cost, duration_ms, context_tokens, \
              context_window, created_at) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
            params![
                row.id, row.agent_id, row.model_id, row.provider_id, row.terminate_reason,
                row.iterations, row.tool_calls, row.tool_error_count, row.tool_call_total,
                row.tok_input, row.tok_output, row.tok_cache_read, row.tok_cache_creation,
                row.tok_reasoning, row.estimated_cost, row.duration_ms, row.context_tokens,
                row.context_window, row.created_at,
            ],
        )
        .map_err(|e| AlephError::config(format!("record_routing_experience insert: {e}")))?;

        conn.execute(
            "INSERT INTO routing_exp_vec_map (routing_exp_id, agent_id, dim) VALUES (?1, ?2, ?3)",
            params![row.id, row.agent_id, dim as i64],
        )
        .map_err(|e| AlephError::config(format!("record_routing_experience map insert: {e}")))?;
        let rowid = conn.last_insert_rowid();

        conn.execute(
            &format!("INSERT INTO {table} (rowid, embedding) VALUES (?1, ?2)"),
            params![rowid, blob],
        )
        .map_err(|e| AlephError::config(format!("record_routing_experience vec insert: {e}")))?;

        Ok(())
    }

    pub fn recall_routing_experience(
        &self,
        task_emb: &[f32],
        dim: u32,
        agent_id: &str,
        k: usize,
    ) -> Result<Vec<RoutingNeighbor>, AlephError> {
        let table = vec::routing_exp_vec_table_for_dim(dim)?;
        let blob = vec::embedding_to_blob(task_emb);
        let k_over = k.saturating_mul(3).max(k) as i64;

        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        // Step 1: kNN on the routing vec0 table alone (sqlite-vec requirement).
        let knn: Vec<(i64, f64)> = {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT rowid, distance FROM {table} WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2"
                ))
                .map_err(|e| AlephError::config(format!("recall_routing_experience knn prepare: {e}")))?;
            let rows = stmt
                .query_map(params![blob, k_over], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
                })
                .map_err(|e| AlephError::config(format!("recall_routing_experience knn query: {e}")))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(|e| AlephError::config(format!("recall_routing_experience knn row: {e}")))?);
            }
            out
        };

        // Step 2: rowid -> routing_exp_id filtered by agent (post-hoc isolation), then load the row.
        let mut map_stmt = conn
            .prepare("SELECT routing_exp_id FROM routing_exp_vec_map WHERE rowid = ?1 AND agent_id = ?2")
            .map_err(|e| AlephError::config(format!("recall_routing_experience map prepare: {e}")))?;
        let mut row_stmt = conn
            .prepare(
                "SELECT id, agent_id, model_id, provider_id, terminate_reason, iterations, tool_calls, \
                 tool_error_count, tool_call_total, tok_input, tok_output, tok_cache_read, \
                 tok_cache_creation, tok_reasoning, estimated_cost, duration_ms, context_tokens, \
                 context_window, created_at FROM routing_experiences WHERE id = ?1",
            )
            .map_err(|e| AlephError::config(format!("recall_routing_experience row prepare: {e}")))?;

        let mut neighbors: Vec<RoutingNeighbor> = Vec::with_capacity(k);
        for (rowid, distance) in &knn {
            let exp_id: Option<String> = map_stmt
                .query_row(params![rowid, agent_id], |r| r.get(0))
                .optional()
                .map_err(|e| AlephError::config(format!("recall_routing_experience map row: {e}")))?;
            let Some(exp_id) = exp_id else { continue };
            let neighbor = row_stmt
                .query_row(params![exp_id], |row| {
                    Ok(RoutingNeighbor {
                        id: row.get("id")?,
                        agent_id: row.get("agent_id")?,
                        model_id: row.get("model_id")?,
                        provider_id: row.get("provider_id")?,
                        terminate_reason: row.get("terminate_reason")?,
                        iterations: row.get("iterations")?,
                        tool_calls: row.get("tool_calls")?,
                        tool_error_count: row.get("tool_error_count")?,
                        tool_call_total: row.get("tool_call_total")?,
                        tok_input: row.get("tok_input")?,
                        tok_output: row.get("tok_output")?,
                        tok_cache_read: row.get("tok_cache_read")?,
                        tok_cache_creation: row.get("tok_cache_creation")?,
                        tok_reasoning: row.get("tok_reasoning")?,
                        estimated_cost: row.get("estimated_cost")?,
                        duration_ms: row.get("duration_ms")?,
                        context_tokens: row.get("context_tokens")?,
                        context_window: row.get("context_window")?,
                        created_at: row.get("created_at")?,
                        distance: *distance as f32,
                    })
                })
                .optional()
                .map_err(|e| AlephError::config(format!("recall_routing_experience row: {e}")))?;
            if let Some(neighbor) = neighbor {
                neighbors.push(neighbor);
            }
        }

        neighbors.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal));
        neighbors.truncate(k);
        Ok(neighbors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_backend() -> SqliteMemoryBackend {
        let dir = std::env::temp_dir().join(format!("aleph-routing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        SqliteMemoryBackend::new(&dir.join("mem.db")).unwrap()
    }
    fn emb(seed: f32) -> Vec<f32> {
        let mut v = vec![0.0f32; 768];
        v[0] = seed;
        v
    }
    fn row(id: &str, agent: &str, model: &str, created_at: i64) -> RoutingExperienceRow {
        RoutingExperienceRow {
            id: id.into(), agent_id: agent.into(), model_id: model.into(), provider_id: "p".into(),
            terminate_reason: "{\"kind\":\"completed\"}".into(),
            iterations: 0, tool_calls: 0, tool_error_count: 0, tool_call_total: 0,
            tok_input: 0, tok_output: 0, tok_cache_read: 0, tok_cache_creation: 0, tok_reasoning: 0,
            estimated_cost: None, duration_ms: 0, context_tokens: 0, context_window: 0, created_at,
        }
    }

    #[test]
    fn recall_orders_by_distance_and_isolates_agents() {
        let backend = temp_backend();
        backend.record_routing_experience(&row("1", "a", "m1", 1), &emb(1.0), 768).unwrap();
        backend.record_routing_experience(&row("2", "a", "m2", 2), &emb(2.0), 768).unwrap();
        backend.record_routing_experience(&row("3", "a", "m3", 3), &emb(3.0), 768).unwrap();
        backend.record_routing_experience(&row("b1", "b", "mb", 4), &emb(1.0), 768).unwrap();

        let got = backend.recall_routing_experience(&emb(0.0), 768, "a", 3).unwrap();
        let models: Vec<String> = got.iter().map(|n| n.model_id.clone()).collect();
        assert_eq!(models, vec!["m1", "m2", "m3"]);
        assert!(got.iter().all(|n| n.agent_id == "a"));
        assert!(got.iter().all(|n| n.model_id != "mb"));
    }

    #[test]
    fn prune_keeps_newest_by_recency_not_distance() {
        let backend = temp_backend();
        backend.record_routing_experience(&row("1", "a", "m1", 1), &emb(1.0), 768).unwrap(); // nearest, oldest
        backend.record_routing_experience(&row("2", "a", "m2", 2), &emb(2.0), 768).unwrap();
        backend.record_routing_experience(&row("3", "a", "m3", 3), &emb(3.0), 768).unwrap();
        backend.prune_routing_experiences("a", 768, 2).unwrap();
        let got = backend.recall_routing_experience(&emb(0.0), 768, "a", 5).unwrap();
        let models: Vec<String> = got.iter().map(|n| n.model_id.clone()).collect();
        assert!(!models.contains(&"m1".to_string())); // oldest pruned despite being nearest
        assert!(models.contains(&"m2".to_string()));
        assert!(models.contains(&"m3".to_string()));
    }

    #[test]
    fn record_targets_routing_dim_table_and_not_notes() {
        let backend = temp_backend();
        backend.record_routing_experience(&row("1", "a", "m1", 1), &emb(1.0), 768).unwrap();
        // `conn` is private to the sqlite module but reachable from this child module.
        let conn = backend.conn.lock().unwrap();
        let routing_count: i64 = conn
            .query_row("SELECT count(*) FROM routing_exp_vec_768", [], |r| r.get(0))
            .unwrap();
        assert_eq!(routing_count, 1, "embedding must land in routing_exp_vec_768");
        let notes_count: i64 = conn
            .query_row("SELECT count(*) FROM notes_vec_768", [], |r| r.get(0))
            .unwrap();
        assert_eq!(notes_count, 0, "routing write must not touch notes_vec_768");
    }
}
