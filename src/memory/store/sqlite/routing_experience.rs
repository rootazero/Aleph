//! Storage primitive for VESR (Verified-Experience Self-Routing) routing experiences.
//!
//! Provides `record_routing_experience` and `recall_routing_experience` methods on
//! `SqliteMemoryBackend`, backed by the `routing_experiences` relational table and
//! the `routing_exp_vec_{768,1024,1536}` sqlite-vec virtual tables.

use super::vec;
use super::SqliteMemoryBackend;
use crate::error::AlephError;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

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

/// Per-(model, provider) lifetime aggregate for ONE agent. R7: raw facts only —
/// no `success_rate`, no score, no ranking, no `best_for`. The LLM interprets
/// these; the code never derives a verdict. `terminate_reason_counts` is the raw
/// distribution of the verbatim `terminate_reason` discriminant ("kind").
#[derive(Debug, Clone, PartialEq)]
pub struct ModelAggregate {
    pub model_id: String,
    pub provider_id: String,
    pub n_runs: u32,
    /// (`terminate_reason` kind, count), ordered count-desc then name for
    /// determinism. NOT a success metric — just how runs ended.
    pub terminate_reason_counts: Vec<(String, u32)>,
    pub avg_iterations: f64,
    pub avg_tool_errors: f64,
    pub avg_total_tokens: f64,
    /// Mean over runs with a known cost (NULL costs ignored); `None` if none.
    pub avg_cost: Option<f64>,
    pub last_used_unix: i64,
}

/// Extract the verbatim variant tag ("kind") from a serialized `TerminateReason`
/// JSON string. Pure string handling — no JSON1 SQL dependency.
fn terminate_kind(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(String::from))
        .unwrap_or_else(|| "unknown".to_string())
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
                row.id,
                row.agent_id,
                row.model_id,
                row.provider_id,
                row.terminate_reason,
                row.iterations,
                row.tool_calls,
                row.tool_error_count,
                row.tool_call_total,
                row.tok_input,
                row.tok_output,
                row.tok_cache_read,
                row.tok_cache_creation,
                row.tok_reasoning,
                row.estimated_cost,
                row.duration_ms,
                row.context_tokens,
                row.context_window,
                row.created_at,
            ],
        )
        .map_err(|e| AlephError::config(format!("record_routing_experience insert: {e}")))?;

        conn.execute(
            "INSERT INTO routing_exp_vec_map (routing_exp_id, agent_id, dim) VALUES (?1, ?2, ?3)",
            params![row.id, row.agent_id, dim as i64],
        )
        .map_err(|e| AlephError::config(format!("record_routing_experience map insert: {e}")))?;
        let rowid = conn.last_insert_rowid();

        // Table name is validated by `vec::routing_exp_vec_table_for_dim` against a static allowlist.
        // rust-doctor-disable-next-line sql-injection-risk
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
                .map_err(|e| {
                    AlephError::config(format!("recall_routing_experience knn query: {e}"))
                })?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(|e| {
                    AlephError::config(format!("recall_routing_experience knn row: {e}"))
                })?);
            }
            out
        };

        // Step 2: rowid -> routing_exp_id filtered by agent (post-hoc isolation), then load the row.
        let mut map_stmt = conn
            .prepare(
                "SELECT routing_exp_id FROM routing_exp_vec_map WHERE rowid = ?1 AND agent_id = ?2",
            )
            .map_err(|e| {
                AlephError::config(format!("recall_routing_experience map prepare: {e}"))
            })?;
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
                .map_err(|e| {
                    AlephError::config(format!("recall_routing_experience map row: {e}"))
                })?;
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

        neighbors.sort_by(|a, b| {
            a.distance
                .partial_cmp(&b.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        neighbors.truncate(k);
        Ok(neighbors)
    }

    pub fn prune_routing_experiences(
        &self,
        agent_id: &str,
        dim: u32,
        cap: usize,
    ) -> Result<(), AlephError> {
        let table = vec::routing_exp_vec_table_for_dim(dim)?;
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;

        let drop_ids: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM routing_experiences WHERE agent_id = ?1 \
                     ORDER BY created_at DESC LIMIT -1 OFFSET ?2",
                )
                .map_err(|e| {
                    AlephError::config(format!("prune_routing_experiences select: {e}"))
                })?;
            let rows = stmt
                .query_map(params![agent_id, cap as i64], |r| r.get::<_, String>(0))
                .map_err(|e| AlephError::config(format!("prune_routing_experiences query: {e}")))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(|e| {
                    AlephError::config(format!("prune_routing_experiences row: {e}"))
                })?);
            }
            out
        };

        let tx = conn.unchecked_transaction().map_err(|e| {
            AlephError::config(format!("prune_routing_experiences transaction: {e}"))
        })?;
        for id in drop_ids {
            let rowid: Option<i64> = tx
                .query_row(
                    "SELECT rowid FROM routing_exp_vec_map WHERE agent_id = ?1 AND routing_exp_id = ?2",
                    params![agent_id, id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| AlephError::config(format!("prune_routing_experiences map: {e}")))?;
            if let Some(rowid) = rowid {
                // Table name is validated by `vec::routing_exp_vec_table_for_dim` against a static allowlist.
                // rust-doctor-disable-next-line sql-injection-risk
                tx.execute(
                    &format!("DELETE FROM {table} WHERE rowid = ?1"),
                    params![rowid],
                )
                .map_err(|e| {
                    AlephError::config(format!("prune_routing_experiences vec del: {e}"))
                })?;
                tx.execute(
                    "DELETE FROM routing_exp_vec_map WHERE rowid = ?1",
                    params![rowid],
                )
                .map_err(|e| {
                    AlephError::config(format!("prune_routing_experiences map del: {e}"))
                })?;
            }
            tx.execute("DELETE FROM routing_experiences WHERE id = ?1", params![id])
                .map_err(|e| {
                    AlephError::config(format!("prune_routing_experiences exp del: {e}"))
                })?;
        }
        tx.commit()
            .map_err(|e| AlephError::config(format!("prune_routing_experiences commit: {e}")))?;
        Ok(())
    }

    /// Per-(model, provider) lifetime aggregate for one agent. Read-side fold
    /// over the relational table (no vec table, no DDL). Bounded by the
    /// retention cap (<=5000 rows/agent). R7: raw aggregates only.
    pub fn aggregate_routing_experiences_by_model(
        &self,
        agent_id: &str,
    ) -> Result<Vec<ModelAggregate>, AlephError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| AlephError::config(format!("Mutex poisoned: {e}")))?;
        let mut stmt = conn
            .prepare(
                "SELECT model_id, provider_id, terminate_reason, iterations, tool_error_count, \
                 tok_input + tok_output + tok_cache_read + tok_cache_creation + tok_reasoning, \
                 estimated_cost, created_at \
                 FROM routing_experiences WHERE agent_id = ?1",
            )
            .map_err(|e| AlephError::config(format!("aggregate prepare: {e}")))?;
        let rows = stmt
            .query_map(params![agent_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, Option<f64>>(6)?,
                    r.get::<_, i64>(7)?,
                ))
            })
            .map_err(|e| AlephError::config(format!("aggregate query: {e}")))?;

        struct Acc {
            model_id: String,
            provider_id: String,
            n: u64,
            sum_iter: u64,
            sum_tool_err: u64,
            sum_tok: u64,
            cost_sum: f64,
            cost_n: u64,
            last_used: i64,
            kinds: std::collections::HashMap<String, u32>,
        }
        let mut accs: std::collections::HashMap<(String, String), Acc> =
            std::collections::HashMap::new();
        for r in rows {
            let (model_id, provider_id, tr, iters, tool_err, tok, cost, created) =
                r.map_err(|e| AlephError::config(format!("aggregate row: {e}")))?;
            let kind = terminate_kind(&tr);
            let acc = accs
                .entry((model_id.clone(), provider_id.clone()))
                .or_insert_with(|| Acc {
                    model_id,
                    provider_id,
                    n: 0,
                    sum_iter: 0,
                    sum_tool_err: 0,
                    sum_tok: 0,
                    cost_sum: 0.0,
                    cost_n: 0,
                    last_used: 0,
                    kinds: std::collections::HashMap::new(),
                });
            acc.n += 1;
            acc.sum_iter += iters.max(0) as u64;
            acc.sum_tool_err += tool_err.max(0) as u64;
            acc.sum_tok += tok.max(0) as u64;
            if let Some(c) = cost {
                acc.cost_sum += c;
                acc.cost_n += 1;
            }
            if created > acc.last_used {
                acc.last_used = created;
            }
            *acc.kinds.entry(kind).or_insert(0) += 1;
        }

        let mut out: Vec<ModelAggregate> = accs
            .into_values()
            .map(|a| {
                let n = a.n.max(1) as f64;
                let mut kinds: Vec<(String, u32)> = a.kinds.into_iter().collect();
                kinds.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
                ModelAggregate {
                    model_id: a.model_id,
                    provider_id: a.provider_id,
                    n_runs: a.n.min(u32::MAX as u64) as u32,
                    terminate_reason_counts: kinds,
                    avg_iterations: a.sum_iter as f64 / n,
                    avg_tool_errors: a.sum_tool_err as f64 / n,
                    avg_total_tokens: a.sum_tok as f64 / n,
                    avg_cost: if a.cost_n > 0 {
                        Some(a.cost_sum / a.cost_n as f64)
                    } else {
                        None
                    },
                    last_used_unix: a.last_used,
                }
            })
            .collect();
        // D5: neutral order — most recently used first, NEVER by an outcome
        // metric (sorting by quality would be an implicit ranking = R7 breach).
        out.sort_by(|x, y| {
            y.last_used_unix
                .cmp(&x.last_used_unix)
                .then_with(|| x.model_id.cmp(&y.model_id))
        });
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_backend() -> (tempfile::TempDir, SqliteMemoryBackend) {
        let (scratch, dir) = crate::utils::scratch::scratch_root();
        std::fs::create_dir_all(&dir).unwrap();
        (
            scratch,
            SqliteMemoryBackend::new(&dir.join("mem.db")).unwrap(),
        )
    }
    fn emb(seed: f32) -> Vec<f32> {
        let mut v = vec![0.0f32; 768];
        v[0] = seed;
        v
    }
    fn row(id: &str, agent: &str, model: &str, created_at: i64) -> RoutingExperienceRow {
        RoutingExperienceRow {
            id: id.into(),
            agent_id: agent.into(),
            model_id: model.into(),
            provider_id: "p".into(),
            terminate_reason: "{\"kind\":\"completed\"}".into(),
            iterations: 0,
            tool_calls: 0,
            tool_error_count: 0,
            tool_call_total: 0,
            tok_input: 0,
            tok_output: 0,
            tok_cache_read: 0,
            tok_cache_creation: 0,
            tok_reasoning: 0,
            estimated_cost: None,
            duration_ms: 0,
            context_tokens: 0,
            context_window: 0,
            created_at,
        }
    }

    #[test]
    fn recall_orders_by_distance_and_isolates_agents() {
        let (_scratch, backend) = temp_backend();
        backend
            .record_routing_experience(&row("1", "a", "m1", 1), &emb(1.0), 768)
            .unwrap();
        backend
            .record_routing_experience(&row("2", "a", "m2", 2), &emb(2.0), 768)
            .unwrap();
        backend
            .record_routing_experience(&row("3", "a", "m3", 3), &emb(3.0), 768)
            .unwrap();
        backend
            .record_routing_experience(&row("b1", "b", "mb", 4), &emb(1.0), 768)
            .unwrap();

        let got = backend
            .recall_routing_experience(&emb(0.0), 768, "a", 3)
            .unwrap();
        let models: Vec<String> = got.iter().map(|n| n.model_id.clone()).collect();
        assert_eq!(models, vec!["m1", "m2", "m3"]);
        assert!(got.iter().all(|n| n.agent_id == "a"));
        assert!(got.iter().all(|n| n.model_id != "mb"));
    }

    #[test]
    fn prune_keeps_newest_by_recency_not_distance() {
        let (_scratch, backend) = temp_backend();
        backend
            .record_routing_experience(&row("1", "a", "m1", 1), &emb(1.0), 768)
            .unwrap(); // nearest, oldest
        backend
            .record_routing_experience(&row("2", "a", "m2", 2), &emb(2.0), 768)
            .unwrap();
        backend
            .record_routing_experience(&row("3", "a", "m3", 3), &emb(3.0), 768)
            .unwrap();
        backend.prune_routing_experiences("a", 768, 2).unwrap();
        let got = backend
            .recall_routing_experience(&emb(0.0), 768, "a", 5)
            .unwrap();
        let models: Vec<String> = got.iter().map(|n| n.model_id.clone()).collect();
        assert!(!models.contains(&"m1".to_string())); // oldest pruned despite being nearest
        assert!(models.contains(&"m2".to_string()));
        assert!(models.contains(&"m3".to_string()));
    }

    #[test]
    fn record_targets_routing_dim_table_and_not_notes() {
        let (_scratch, backend) = temp_backend();
        backend
            .record_routing_experience(&row("1", "a", "m1", 1), &emb(1.0), 768)
            .unwrap();
        // `conn` is private to the sqlite module but reachable from this child module.
        let conn = backend.conn.lock().unwrap();
        let routing_count: i64 = conn
            .query_row("SELECT count(*) FROM routing_exp_vec_768", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            routing_count, 1,
            "embedding must land in routing_exp_vec_768"
        );
        let notes_count: i64 = conn
            .query_row("SELECT count(*) FROM notes_vec_768", [], |r| r.get(0))
            .unwrap();
        assert_eq!(notes_count, 0, "routing write must not touch notes_vec_768");
    }

    #[test]
    fn ddl_has_no_judgment_columns() {
        let ddl = crate::memory::store::sqlite::schema::ddl::ROUTING_EXPERIENCE_DDL.to_lowercase();
        assert!(!ddl.contains("success"));
        assert!(!ddl.contains("score"));
        assert!(!ddl.contains("rank"));
        assert!(!ddl.contains("best_for"));
    }

    #[test]
    fn aggregate_groups_by_model_with_raw_facts() {
        let (_scratch, backend) = temp_backend();
        let mut r1 = row("1", "a", "m1", 10);
        r1.iterations = 2;
        r1.tok_input = 100;
        r1.estimated_cost = Some(0.01);
        let mut r2 = row("2", "a", "m1", 20);
        r2.iterations = 4;
        r2.tok_output = 50;
        r2.estimated_cost = Some(0.03);
        let mut r3 = row("3", "a", "m2", 30);
        r3.terminate_reason = "{\"kind\":\"max_iterations\"}".into();
        r3.tool_error_count = 2;
        backend
            .record_routing_experience(&r1, &emb(1.0), 768)
            .unwrap();
        backend
            .record_routing_experience(&r2, &emb(2.0), 768)
            .unwrap();
        backend
            .record_routing_experience(&r3, &emb(3.0), 768)
            .unwrap();

        let aggs = backend.aggregate_routing_experiences_by_model("a").unwrap();
        assert_eq!(aggs.len(), 2);
        // Neutral order = most recently used first → m2 (30) before m1 (max 20).
        assert_eq!(aggs[0].model_id, "m2");
        assert_eq!(aggs[1].model_id, "m1");

        let m1 = &aggs[1];
        assert_eq!(m1.n_runs, 2);
        assert_eq!(m1.avg_iterations, 3.0); // (2+4)/2
        assert_eq!(m1.avg_total_tokens, 75.0); // (100 + 50)/2
        assert_eq!(m1.avg_cost, Some(0.02)); // (0.01 + 0.03)/2
        assert_eq!(
            m1.terminate_reason_counts,
            vec![("completed".to_string(), 2)]
        );

        let m2 = &aggs[0];
        assert_eq!(m2.n_runs, 1);
        assert_eq!(m2.avg_tool_errors, 2.0);
        assert_eq!(
            m2.terminate_reason_counts,
            vec![("max_iterations".to_string(), 1)]
        );
    }

    #[test]
    fn aggregate_isolates_agents_and_handles_missing_cost() {
        let (_scratch, backend) = temp_backend();
        let mut ra = row("1", "a", "m1", 1);
        ra.estimated_cost = None;
        let rb = row("2", "b", "mb", 2);
        backend
            .record_routing_experience(&ra, &emb(1.0), 768)
            .unwrap();
        backend
            .record_routing_experience(&rb, &emb(2.0), 768)
            .unwrap();

        let aggs = backend.aggregate_routing_experiences_by_model("a").unwrap();
        assert_eq!(aggs.len(), 1); // agent "b" excluded
        assert_eq!(aggs[0].model_id, "m1");
        assert_eq!(aggs[0].avg_cost, None); // no run had a known cost
    }
}
