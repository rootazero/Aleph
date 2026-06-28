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
