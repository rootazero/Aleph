//! CRUD operations for `task_traces` table
//!
//! Provides database operations for execution trace management,
//! enabling Shadow Replay for deterministic task recovery.

use super::StateDatabase;
use crate::error::AlephError;
use crate::resilience::{TaskTrace, TaskTraceInfo};
use aleph_protocol::AgentTraceEvent;
use rusqlite::params;
use rusqlite::types::Type;
use rusqlite::OptionalExtension;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Per-agent rollup of `ProviderUsage` trace events.
///
/// Sums are saturating-cast from `SQLite` `INTEGER` (i64) to u64; in practice
/// token counts fit comfortably below `i64::MAX`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentUsageTotal {
    pub agent_id: String,
    pub call_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub reasoning_tokens: u64,
}

impl AgentUsageTotal {
    /// Cumulative cache-hit ratio across every `ProviderUsage` event that
    /// contributed to this rollup. `None` when no cached input was ever
    /// observed (cumulative `cache_read` == 0 AND input == 0); `Some(0.0)`
    /// when input was non-zero but no cache reads occurred. See
    /// [`crate::providers::adapter::TokenUsage::cache_hit_ratio`] for the
    /// per-call counterpart, which enforces the same disjoint-counter
    /// invariant this function relies on.
    ///
    /// The denominator is unconditionally `input + cache_read`. Every adapter
    /// normalises its provider's usage into *disjoint* counters before it is
    /// persisted (Anthropic reports them that way natively;
    /// `openai_chat/sse.rs`, `openai_responses/mod.rs` and `gemini/sse.rs` each
    /// subtract the cached portion out of the inclusive prompt total), so
    /// `input_tokens` never contains `cache_read_tokens` in any stored row.
    ///
    /// This used to guess the protocol from the magnitudes
    /// (`cache_read > input ⇒ Anthropic, else assume inclusive`) — the same
    /// heuristic the per-call twin deleted as a bug. The guess was wrong for
    /// every provider, and wrong in the direction that hides trouble: it took
    /// the else-branch exactly when `cache_read <= input`, i.e. when less than
    /// half the prompt came from cache, and reported `cache_read / input`
    /// instead of `cache_read / (input + cache_read)`. A true 50% hit rate
    /// read as 100%; a true 20% read as 25%. The rollup therefore flattered
    /// the cache monotonically harder as the prefix broke — the one number a
    /// user checks after a suspicious bill.
    #[must_use]
    pub fn cache_hit_ratio(&self) -> Option<f64> {
        if self.cache_read_tokens == 0 && self.input_tokens == 0 {
            return None;
        }
        if self.cache_read_tokens == 0 {
            return Some(0.0);
        }
        let total_prompt = self.input_tokens.saturating_add(self.cache_read_tokens);
        if total_prompt == 0 {
            return None;
        }
        Some(self.cache_read_tokens as f64 / total_prompt as f64)
    }
}

/// Construct `TaskTrace` from a rusqlite row.
/// Expected column order: id, `task_id`, `step_index`, `event_kind`, `event_json`, timestamp
fn task_trace_from_row(row: &rusqlite::Row) -> rusqlite::Result<TaskTrace> {
    let event_kind: String = row.get(3)?;
    let event_json: String = row.get(4)?;
    let event = serde_json::from_str::<AgentTraceEvent>(&event_json)
        .map_err(|err| rusqlite::Error::FromSqlConversionFailure(4, Type::Text, Box::new(err)))?;

    if event.kind() != event_kind {
        tracing::warn!(
            stored_kind = %event_kind,
            parsed_kind = event.kind(),
            "task_traces row has mismatched event_kind and event_json"
        );
    }

    Ok(TaskTrace {
        id: row.get(0)?,
        task_id: row.get(1)?,
        step_index: row.get(2)?,
        event,
        timestamp: row.get(5)?,
    })
}

/// The `trace.list` projection, shared verbatim by both branches of
/// [`StateDatabase::list_trace_tasks_paged`].
///
/// Positional decoding is a contract without a compiler: the two SELECTs and
/// one `row_map` are three hand-written statements of one column order, and
/// adding a column to two of them is a RUNTIME error in the third. One
/// constant, interpolated, makes that impossible. (This repo has taken that
/// hit before — see the criteria list's "一个 `from_row` 配 N 个 `SELECT`".)
///
/// `substr(..., 1, 200)` counts CHARACTERS in SQLite, so the preview cannot
/// split a multi-byte codepoint the way a byte slice would.
const TRACE_LIST_COLUMNS: &str = "tr.task_id, \
     COUNT(*) AS event_count, \
     MAX(tr.timestamp) AS last_timestamp, \
     t.status, \
     t.started_at, \
     substr(t.task_prompt, 1, 200)";

impl StateDatabase {
    // =========================================================================
    // Task Traces CRUD
    // =========================================================================

    /// Insert a single trace entry
    pub async fn insert_trace(&self, trace: &TaskTrace) -> Result<i64, AlephError> {
        let trace = trace.clone();
        self.with_conn(move |conn| {
            let event_json = serde_json::to_string(&trace.event)
                .map_err(|e| AlephError::config(format!("Failed to serialize trace event: {e}")))?;
            conn.execute(
                r#"
                INSERT INTO task_traces (task_id, step_index, event_kind, event_json, timestamp)
                VALUES (?1, ?2, ?3, ?4, ?5)
                "#,
                params![
                    trace.task_id,
                    trace.step_index,
                    trace.event_kind(),
                    event_json,
                    trace.timestamp,
                ],
            )
            .map_err(|e| AlephError::config(format!("Failed to insert trace: {e}")))?;

            Ok(conn.last_insert_rowid())
        })
        .await
    }

    /// Bulk insert traces (for efficient batch writes)
    pub async fn bulk_insert_traces(&self, traces: &[TaskTrace]) -> Result<(), AlephError> {
        if traces.is_empty() {
            return Ok(());
        }

        let traces: Vec<TaskTrace> = traces.to_vec();
        self.with_conn(move |conn| {
            let tx = conn
                .transaction()
                .map_err(|e| AlephError::config(format!("Failed to begin transaction: {e}")))?;

            {
                let mut stmt = tx
                    .prepare(
                        r#"
                        INSERT INTO task_traces (task_id, step_index, event_kind, event_json, timestamp)
                        VALUES (?1, ?2, ?3, ?4, ?5)
                        "#,
                    )
                    .map_err(|e| AlephError::config(format!("Failed to prepare statement: {e}")))?;

                for trace in &traces {
                    let event_json = serde_json::to_string(&trace.event).map_err(|e| {
                        AlephError::config(format!("Failed to serialize trace event: {e}"))
                    })?;
                    stmt.execute(params![
                        trace.task_id,
                        trace.step_index,
                        trace.event_kind(),
                        event_json,
                        trace.timestamp,
                    ])
                    .map_err(|e| AlephError::config(format!("Failed to insert trace: {e}")))?;
                }
            }

            tx.commit()
                .map_err(|e| AlephError::config(format!("Failed to commit transaction: {e}")))?;

            Ok(())
        })
        .await
    }

    /// Get all traces for a task (ordered by `step_index`)
    pub async fn get_traces_by_task(&self, task_id: &str) -> Result<Vec<TaskTrace>, AlephError> {
        let task_id = task_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT id, task_id, step_index, event_kind, event_json, timestamp
                    FROM task_traces
                    WHERE task_id = ?1
                    ORDER BY step_index ASC
                    "#,
                )
                .map_err(|e| AlephError::config(format!("Failed to prepare query: {e}")))?;

            let traces = stmt
                .query_map(params![task_id], task_trace_from_row)
                .map_err(|e| AlephError::config(format!("Failed to query traces: {e}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| AlephError::config(format!("Failed to collect traces: {e}")))?;

            Ok(traces)
        })
        .await
    }

    /// Get the last trace entry for a task (for recovery checkpoint)
    pub async fn get_last_trace(&self, task_id: &str) -> Result<Option<TaskTrace>, AlephError> {
        let task_id = task_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT id, task_id, step_index, event_kind, event_json, timestamp
                    FROM task_traces
                    WHERE task_id = ?1
                    ORDER BY step_index DESC
                    LIMIT 1
                    "#,
                )
                .map_err(|e| AlephError::config(format!("Failed to prepare query: {e}")))?;

            let result = stmt
                .query_row(params![task_id], task_trace_from_row)
                .optional()
                .map_err(|e| AlephError::config(format!("Failed to get last trace: {e}")))?;

            Ok(result)
        })
        .await
    }

    /// Get traces from a specific step index (for resuming from checkpoint)
    pub async fn get_traces_from_step(
        &self,
        task_id: &str,
        from_step: u32,
    ) -> Result<Vec<TaskTrace>, AlephError> {
        let task_id = task_id.to_string();
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT id, task_id, step_index, event_kind, event_json, timestamp
                    FROM task_traces
                    WHERE task_id = ?1 AND step_index >= ?2
                    ORDER BY step_index ASC
                    "#,
                )
                .map_err(|e| AlephError::config(format!("Failed to prepare query: {e}")))?;

            let traces = stmt
                .query_map(params![task_id, from_step], task_trace_from_row)
                .map_err(|e| AlephError::config(format!("Failed to query traces: {e}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| AlephError::config(format!("Failed to collect traces: {e}")))?;

            Ok(traces)
        })
        .await
    }

    /// Delete all traces for a task (cleanup)
    pub async fn delete_traces_for_task(&self, task_id: &str) -> Result<u64, AlephError> {
        let task_id = task_id.to_string();
        self.with_conn(move |conn| {
            let count = conn
                .execute(
                    "DELETE FROM task_traces WHERE task_id = ?1",
                    params![task_id],
                )
                .map_err(|e| AlephError::config(format!("Failed to delete traces: {e}")))?;
            Ok(count as u64)
        })
        .await
    }

    /// Get trace count for a task
    pub async fn get_trace_count(&self, task_id: &str) -> Result<u64, AlephError> {
        let task_id = task_id.to_string();
        self.with_conn(move |conn| {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM task_traces WHERE task_id = ?1",
                    params![task_id],
                    |row| row.get(0),
                )
                .map_err(|e| AlephError::config(format!("Failed to count traces: {e}")))?;
            Ok(count as u64)
        })
        .await
    }
    /// Returns at most `limit`
    /// (clamped to 1..200) trace-task summaries ordered by
    /// `(last_timestamp DESC, task_id DESC)` so each page is a strict prefix
    /// of the deterministic ordering.
    ///
    /// `before` is the optional cursor: `(last_timestamp, task_id)` of the
    /// last entry of the previous page (or `None` for the first page).
    /// Tie-break on `task_id` is required because `TaskTrace::new` stamps
    /// timestamp as epoch SECONDS — rapid inserts collide, and a strict
    /// `HAVING MAX(timestamp) < ?` cursor would silently drop every task
    /// whose `last_timestamp` equals the previous page's last entry.
    ///
    /// Keeps each page O(limit) regardless of total trace volume, so
    /// callers can paginate without scanning the whole table on every
    /// request.
    ///
    /// The unpaginated sibling `list_trace_tasks` was removed on 2026-08-29:
    /// zero callers repo-wide, so R10 says CUT rather than reconnect. Its doc
    /// claimed it was "preserved for callers that want everything in one shot"
    /// — a promise to a caller that never arrived, and the second SELECT that
    /// would have had to learn about the `agent_tasks` join below.
    ///
    /// The rows carry `status` / `started_at` / `prompt_preview` from the
    /// `agent_tasks` parent since the same date. They are not new facts: the FK
    /// `task_traces.task_id -> agent_tasks(id) ON DELETE RESTRICT` guarantees
    /// the parent exists, and `trace.list`'s three clients had all been written
    /// as though those fields were already on the wire.
    pub async fn list_trace_tasks_paged(
        &self,
        limit: usize,
        before: Option<(i64, String)>,
    ) -> Result<Vec<TaskTraceInfo>, AlephError> {
        let clamped_limit = limit.clamp(1, 200) as i64;
        self.with_conn(move |conn| {
            // Column order is the ONE contract shared by both branches below;
            // they interpolate `TRACE_LIST_COLUMNS` rather than each spelling
            // out a SELECT list, so adding a column cannot leave one branch
            // decoding by a stale index.
            let row_map = |row: &rusqlite::Row<'_>| {
                Ok(TaskTraceInfo {
                    task_id: row.get(0)?,
                    event_count: row.get(1)?,
                    last_timestamp: row.get(2)?,
                    status: row.get(3)?,
                    started_at: row.get(4)?,
                    prompt_preview: row.get(5)?,
                })
            };

            let collect_err =
                |e: rusqlite::Error| AlephError::config(format!("Failed to collect paged traces: {e}"));

            // Cursor rows are those ordered STRICTLY before `(ts, task_id)`
            // in the `(last_timestamp DESC, task_id DESC)` ordering. With that
            // ordering, "strictly before" means:
            //   last_timestamp < cursor_ts
            //     OR (last_timestamp = cursor_ts AND task_id < cursor_task_id)
            // The cursor's own row is excluded (it was on the previous page).
            match before {
                Some((ts, task_id)) => {
                    let mut stmt = conn
                        .prepare(&format!(
                            r#"
                            SELECT {TRACE_LIST_COLUMNS}
                            FROM task_traces tr
                            LEFT JOIN agent_tasks t ON t.id = tr.task_id
                            GROUP BY tr.task_id
                            HAVING MAX(tr.timestamp) < ?1
                                OR (MAX(tr.timestamp) = ?1 AND tr.task_id < ?2)
                            ORDER BY last_timestamp DESC, tr.task_id DESC
                            LIMIT ?3
                            "#
                        ))
                        .map_err(|e| {
                            AlephError::config(format!("Failed to prepare paged query: {e}"))
                        })?;
                    let rows = stmt
                        .query_map(params![ts, task_id, clamped_limit], row_map)
                        .map_err(|e| {
                            AlephError::config(format!("Failed to query paged traces: {e}"))
                        })?;
                    let collected: Result<Vec<_>, _> = rows.collect();
                    collected.map_err(collect_err)
                }
                None => {
                    let mut stmt = conn
                        .prepare(&format!(
                            r#"
                            SELECT {TRACE_LIST_COLUMNS}
                            FROM task_traces tr
                            LEFT JOIN agent_tasks t ON t.id = tr.task_id
                            GROUP BY tr.task_id
                            ORDER BY last_timestamp DESC, tr.task_id DESC
                            LIMIT ?1
                            "#
                        ))
                        .map_err(|e| {
                            AlephError::config(format!("Failed to prepare paged query: {e}"))
                        })?;
                    let rows = stmt
                        .query_map(params![clamped_limit], row_map)
                        .map_err(|e| {
                            AlephError::config(format!("Failed to query paged traces: {e}"))
                        })?;
                    let collected: Result<Vec<_>, _> = rows.collect();
                    collected.map_err(collect_err)
                }
            }
        })
        .await
    }

    /// Get a trace by its ID
    pub async fn get_trace_by_id(&self, trace_id: i64) -> Result<Option<TaskTrace>, AlephError> {
        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    r#"
                    SELECT id, task_id, step_index, event_kind, event_json, timestamp
                    FROM task_traces
                    WHERE id = ?1
                    "#,
                )
                .map_err(|e| AlephError::config(format!("Failed to prepare query: {e}")))?;

            let result = stmt
                .query_row(params![trace_id], task_trace_from_row)
                .optional()
                .map_err(|e| AlephError::config(format!("Failed to get trace: {e}")))?;

            Ok(result)
        })
        .await
    }

    // =========================================================================
    // ProviderUsage aggregation (per-team / per-agent cost rollup)
    // =========================================================================

    /// Aggregate `ProviderUsage` events grouped by `agent_id`.
    ///
    /// Scans `task_traces` for rows where `event_kind = 'provider_usage'` and
    /// `event_json -> agent_id` is in `agent_ids`. Optional `since` / `until`
    /// bounds restrict to a timestamp window (epoch seconds, same units written
    /// by `TaskTrace::new`).
    ///
    /// Returns one row per agent that actually had usage in the window;
    /// agents with zero usage are omitted (callers fill zeros at the
    /// presentation layer).
    pub async fn aggregate_usage_by_agents(
        &self,
        agent_ids: &[String],
        since: Option<i64>,
        until: Option<i64>,
    ) -> Result<Vec<AgentUsageTotal>, AlephError> {
        if agent_ids.is_empty() {
            return Ok(Vec::new());
        }
        let agent_ids: Vec<String> = agent_ids.to_vec();
        self.with_conn(move |conn| {
            // Build positional placeholders ?1..?N for the IN clause, then ?N+1
            // and ?N+2 for the optional time bounds (omitted when absent).
            let placeholders: Vec<String> = (1..=agent_ids.len()).map(|i| format!("?{i}")).collect();
            let in_list = placeholders.join(", ");
            let mut next_pos = agent_ids.len() + 1;
            let mut where_extras: Vec<String> = Vec::new();
            let mut bind_extras: Vec<i64> = Vec::new();
            if let Some(ts) = since {
                where_extras.push(format!("timestamp >= ?{next_pos}"));
                bind_extras.push(ts);
                next_pos += 1;
            }
            if let Some(ts) = until {
                where_extras.push(format!("timestamp <= ?{next_pos}"));
                bind_extras.push(ts);
            }
            let extra_sql = if where_extras.is_empty() {
                String::new()
            } else {
                format!(" AND {}", where_extras.join(" AND "))
            };

            let sql = format!(
                r#"
                SELECT
                    json_extract(event_json, '$.agent_id') AS agent_id,
                    COUNT(*) AS call_count,
                    SUM(COALESCE(CAST(json_extract(event_json, '$.input_tokens') AS INTEGER), 0)) AS input,
                    SUM(COALESCE(CAST(json_extract(event_json, '$.output_tokens') AS INTEGER), 0)) AS output,
                    SUM(COALESCE(CAST(json_extract(event_json, '$.cache_read_tokens') AS INTEGER), 0)) AS cache_read,
                    SUM(COALESCE(CAST(json_extract(event_json, '$.cache_creation_tokens') AS INTEGER), 0)) AS cache_creation,
                    SUM(COALESCE(CAST(json_extract(event_json, '$.thinking_tokens') AS INTEGER), 0)) AS reasoning
                FROM task_traces
                WHERE event_kind = 'provider_usage'
                  AND json_extract(event_json, '$.agent_id') IN ({in_list}){extra_sql}
                GROUP BY agent_id
                ORDER BY agent_id ASC
                "#
            );

            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| AlephError::config(format!("Failed to prepare usage query: {e}")))?;

            // Combine agent_id text params + optional timestamp ints into a single
            // params_from_iter sequence in positional order.
            let id_values: Vec<rusqlite::types::Value> = agent_ids
                .iter()
                .map(|s| rusqlite::types::Value::Text(s.clone()))
                .collect();
            let ts_values: Vec<rusqlite::types::Value> = bind_extras
                .into_iter()
                .map(rusqlite::types::Value::Integer)
                .collect();
            let all_values: Vec<rusqlite::types::Value> =
                id_values.into_iter().chain(ts_values).collect();

            let rows = stmt
                .query_map(rusqlite::params_from_iter(all_values), |row| {
                    let to_u64 = |v: i64, col: &str| -> Result<u64, rusqlite::Error> {
                        if v < 0 {
                            Err(rusqlite::Error::FromSqlConversionFailure(
                                0,
                                rusqlite::types::Type::Integer,
                                Box::new(std::io::Error::new(
                                    std::io::ErrorKind::InvalidData,
                                    format!("{col} must be non-negative, got {v}"),
                                )),
                            ))
                        } else {
                            Ok(v as u64)
                        }
                    };
                    Ok(AgentUsageTotal {
                        agent_id: row.get::<_, String>(0)?,
                        call_count: to_u64(row.get::<_, i64>(1)?, "call_count")?,
                        input_tokens: to_u64(row.get::<_, i64>(2)?, "input_tokens")?,
                    output_tokens: to_u64(row.get::<_, i64>(3)?, "output_tokens")?,
                    cache_read_tokens: to_u64(row.get::<_, i64>(4)?, "cache_read_tokens")?,
                    cache_creation_tokens: to_u64(row.get::<_, i64>(5)?, "cache_creation_tokens")?,
                    reasoning_tokens: to_u64(row.get::<_, i64>(6)?, "reasoning_tokens")?,
                })
            })
            .map_err(|e| AlephError::config(format!("Failed to query usage: {e}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to collect usage: {e}")))?;

            Ok(rows)
        })
        .await
    }

    /// Roll every per-advisor MoA `ProviderUsage` row (synthetic agent_id
    /// `moa:<idx>:<provider>:<model>`, written by the per-advisor
    /// MeteringProvider) into ONE `"moa-advisors"` bucket. Keeps advisor
    /// spend visible in usage rollups without materializing phantom agents
    /// (round-2 B6). `None` when no advisor usage exists in the window.
    pub async fn aggregate_moa_advisor_usage(
        &self,
        since: Option<i64>,
        until: Option<i64>,
    ) -> Result<Option<AgentUsageTotal>, AlephError> {
        self.with_conn(move |conn| {
            let mut where_extras = String::new();
            let mut binds: Vec<rusqlite::types::Value> = Vec::new();
            if let Some(ts) = since {
                where_extras.push_str(" AND timestamp >= ?1");
                binds.push(rusqlite::types::Value::Integer(ts));
            }
            if let Some(ts) = until {
                where_extras.push_str(&format!(" AND timestamp <= ?{}", binds.len() + 1));
                binds.push(rusqlite::types::Value::Integer(ts));
            }
            // Every SUM is double-COALESCE'd: the inner one substitutes 0 for a
            // row whose field is absent from event_json, the outer one substitutes
            // 0 for the whole aggregate when zero rows match the WHERE clause (a
            // no-GROUP-BY SUM over an empty set is SQL NULL, not 0).
            let sql = format!(
                r#"
                SELECT
                    COUNT(*),
                    COALESCE(SUM(COALESCE(CAST(json_extract(event_json, '$.input_tokens') AS INTEGER), 0)), 0),
                    COALESCE(SUM(COALESCE(CAST(json_extract(event_json, '$.output_tokens') AS INTEGER), 0)), 0),
                    COALESCE(SUM(COALESCE(CAST(json_extract(event_json, '$.cache_read_tokens') AS INTEGER), 0)), 0),
                    COALESCE(SUM(COALESCE(CAST(json_extract(event_json, '$.cache_creation_tokens') AS INTEGER), 0)), 0),
                    COALESCE(SUM(COALESCE(CAST(json_extract(event_json, '$.thinking_tokens') AS INTEGER), 0)), 0)
                FROM task_traces
                WHERE event_kind = 'provider_usage'
                  AND json_extract(event_json, '$.agent_id') LIKE 'moa:%'{where_extras}
                "#
            );
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| AlephError::config(format!("Failed to prepare moa usage query: {e}")))?;
            let row = stmt
                .query_row(rusqlite::params_from_iter(binds), |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                })
                .map_err(|e| AlephError::config(format!("Failed to query moa usage: {e}")))?;
            if row.0 == 0 {
                return Ok(None);
            }
            let to_u64 = |v: i64, col: &str| -> Result<u64, AlephError> {
                if v < 0 {
                    Err(AlephError::config(format!(
                        "{col} must be non-negative, got {v}"
                    )))
                } else {
                    Ok(v as u64)
                }
            };
            Ok(Some(AgentUsageTotal {
                agent_id: "moa-advisors".to_string(),
                call_count: to_u64(row.0, "call_count")?,
                input_tokens: to_u64(row.1, "input_tokens")?,
                output_tokens: to_u64(row.2, "output_tokens")?,
                cache_read_tokens: to_u64(row.3, "cache_read_tokens")?,
                cache_creation_tokens: to_u64(row.4, "cache_creation_tokens")?,
                reasoning_tokens: to_u64(row.5, "reasoning_tokens")?,
            }))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resilience::{AgentTask, RiskLevel};
    use aleph_protocol::{AgentTraceEvent, AgentTraceTextKind};

    #[tokio::test]
    async fn test_insert_and_get_structured_trace() {
        let db = StateDatabase::in_memory().unwrap();
        db.insert_agent_task(&AgentTask::new(
            "task-1",
            "session-1",
            "coder",
            "replay trace",
            RiskLevel::Low,
        ))
        .await
        .unwrap();

        let trace = TaskTrace::new(
            "task-1",
            0,
            AgentTraceEvent::TextEmitted {
                iteration: 0,
                stream: AgentTraceTextKind::Final,
                text: "hello".to_string(),
            },
        );

        db.insert_trace(&trace).await.unwrap();

        let traces = db.get_traces_by_task("task-1").await.unwrap();
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].event.kind(), "text_emitted");
        assert_eq!(
            traces[0].event,
            AgentTraceEvent::TextEmitted {
                iteration: 0,
                stream: AgentTraceTextKind::Final,
                text: "hello".to_string(),
            }
        );
    }

    // -------------------------------------------------------------------------
    // P1 — paginated trace task listing
    // -------------------------------------------------------------------------

    async fn seed_n_tasks_each_one_trace(db: &StateDatabase, n: usize) {
        for i in 0..n {
            let tid = format!("task-{i}");
            db.insert_agent_task(&AgentTask::new(
                &tid,
                "session",
                "coder",
                "seeded",
                RiskLevel::Low,
            ))
            .await
            .unwrap();
            db.insert_trace(&TaskTrace::new(
                &tid,
                0,
                AgentTraceEvent::TextEmitted {
                    iteration: 0,
                    stream: AgentTraceTextKind::Final,
                    text: format!("payload-{i}"),
                },
            ))
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn list_paged_returns_at_most_limit() {
        let db = StateDatabase::in_memory().unwrap();
        seed_n_tasks_each_one_trace(&db, 5).await;

        let page = db.list_trace_tasks_paged(3, None).await.unwrap();
        assert_eq!(page.len(), 3);
    }

    #[tokio::test]
    async fn list_paged_clamps_oversize_limit() {
        let db = StateDatabase::in_memory().unwrap();
        seed_n_tasks_each_one_trace(&db, 5).await;

        // 9999 must clamp to <=200 (and we only have 5 tasks, so we get 5).
        let page = db.list_trace_tasks_paged(9999, None).await.unwrap();
        assert_eq!(page.len(), 5);
    }

    #[tokio::test]
    async fn list_paged_cursor_advances_without_overlap() {
        let db = StateDatabase::in_memory().unwrap();
        // Build TaskTrace by hand with explicit increasing timestamps so the
        // (timestamp DESC, task_id DESC) ordering is deterministic.
        let base_ts = chrono::Utc::now().timestamp();
        for i in 0..4i64 {
            let tid = format!("task-{i}");
            db.insert_agent_task(&AgentTask::new(&tid, "s", "coder", "x", RiskLevel::Low))
                .await
                .unwrap();
            let trace = TaskTrace {
                id: 0,
                task_id: tid.clone(),
                step_index: 0,
                event: AgentTraceEvent::TextEmitted {
                    iteration: 0,
                    stream: AgentTraceTextKind::Final,
                    text: "x".into(),
                },
                timestamp: base_ts + i,
            };
            db.insert_trace(&trace).await.unwrap();
        }

        let page_a = db.list_trace_tasks_paged(2, None).await.unwrap();
        assert_eq!(page_a.len(), 2);
        let cursor_ts = page_a.last().unwrap().last_timestamp;
        let cursor_tid = page_a.last().unwrap().task_id.clone();

        let page_b = db
            .list_trace_tasks_paged(2, Some((cursor_ts, cursor_tid)))
            .await
            .unwrap();
        assert!(!page_b.is_empty());
        for r in &page_b {
            assert!(
                page_a.iter().all(|p| p.task_id != r.task_id),
                "page B leaked page A row: {}",
                r.task_id
            );
        }
    }

    /// The cursor must NOT drop rows whose `last_timestamp` collides with
    /// the previous page's last entry. Compound `(timestamp, task_id)` cursor
    /// is the fix.
    #[tokio::test]
    async fn list_paged_does_not_drop_rows_on_timestamp_collision() {
        let db = StateDatabase::in_memory().unwrap();
        let pinned_ts = chrono::Utc::now().timestamp();
        let ids: Vec<String> = (0..5).map(|i| format!("task-{i}")).collect();
        for tid in &ids {
            db.insert_agent_task(&AgentTask::new(tid, "s", "coder", "x", RiskLevel::Low))
                .await
                .unwrap();
            let trace = TaskTrace {
                id: 0,
                task_id: tid.clone(),
                step_index: 0,
                event: AgentTraceEvent::TextEmitted {
                    iteration: 0,
                    stream: AgentTraceTextKind::Final,
                    text: "x".into(),
                },
                timestamp: pinned_ts,
            };
            db.insert_trace(&trace).await.unwrap();
        }

        // Paginate with limit=2 across all 5 colliding tasks. Visit them all.
        let mut seen: Vec<String> = Vec::new();
        let mut cursor: Option<(i64, String)> = None;
        loop {
            let page = db.list_trace_tasks_paged(2, cursor).await.unwrap();
            if page.is_empty() {
                break;
            }
            let last = page.last().unwrap();
            cursor = Some((last.last_timestamp, last.task_id.clone()));
            for info in &page {
                assert!(
                    !seen.contains(&info.task_id),
                    "duplicate task_id {} across pages",
                    info.task_id
                );
                seen.push(info.task_id.clone());
            }
            if page.len() < 2 {
                break;
            }
        }
        assert_eq!(
            seen.len(),
            ids.len(),
            "all 5 colliding tasks must be visited; got {seen:?}"
        );
    }

    // -------------------------------------------------------------------------
    // ProviderUsage aggregation (per-team cost feature)
    // -------------------------------------------------------------------------

    /// Insert a ProviderUsage trace event directly with a chosen timestamp so
    /// tests can build deterministic windows.
    async fn seed_usage(
        db: &StateDatabase,
        task_id: &str,
        step: u32,
        agent_id: &str,
        input: u32,
        output: u32,
        cache_read: Option<u32>,
        cache_creation: Option<u32>,
        thinking: Option<u32>,
        ts: i64,
    ) {
        // Ensure the parent task row exists so foreign keys don't trip.
        let _ = db
            .insert_agent_task(&AgentTask::new(
                task_id,
                "session",
                "coder",
                "seed",
                RiskLevel::Low,
            ))
            .await;
        let trace = TaskTrace {
            id: 0,
            task_id: task_id.to_string(),
            step_index: step,
            event: AgentTraceEvent::ProviderUsage {
                agent_id: agent_id.to_string(),
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: cache_read,
                cache_creation_tokens: cache_creation,
                thinking_tokens: thinking,
            },
            timestamp: ts,
        };
        db.insert_trace(&trace).await.unwrap();
    }

    #[tokio::test]
    async fn aggregate_usage_returns_empty_when_no_agents_supplied() {
        let db = StateDatabase::in_memory().unwrap();
        seed_usage(&db, "t1", 0, "alice", 100, 50, None, None, None, 1000).await;
        let rows = db.aggregate_usage_by_agents(&[], None, None).await.unwrap();
        assert!(rows.is_empty(), "empty agent_ids must short-circuit");
    }

    #[tokio::test]
    async fn aggregate_usage_sums_per_agent() {
        let db = StateDatabase::in_memory().unwrap();
        seed_usage(
            &db,
            "t1",
            0,
            "alice",
            100,
            50,
            Some(20),
            Some(10),
            Some(5),
            1000,
        )
        .await;
        seed_usage(&db, "t1", 1, "alice", 200, 80, Some(30), None, None, 1100).await;
        seed_usage(&db, "t2", 0, "bob", 50, 20, None, None, None, 1200).await;
        // Carol is in agent_ids but produced no usage — must be omitted from
        // the result rows (caller is responsible for zero-filling).
        let rows = db
            .aggregate_usage_by_agents(&["alice".into(), "bob".into(), "carol".into()], None, None)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2, "carol must not appear with zero usage");
        let alice = rows.iter().find(|r| r.agent_id == "alice").unwrap();
        assert_eq!(alice.call_count, 2);
        assert_eq!(alice.input_tokens, 300);
        assert_eq!(alice.output_tokens, 130);
        assert_eq!(alice.cache_read_tokens, 50);
        assert_eq!(alice.cache_creation_tokens, 10);
        assert_eq!(alice.reasoning_tokens, 5);
        let bob = rows.iter().find(|r| r.agent_id == "bob").unwrap();
        assert_eq!(bob.call_count, 1);
        assert_eq!(bob.input_tokens, 50);
    }

    #[tokio::test]
    async fn aggregate_usage_honours_time_window() {
        let db = StateDatabase::in_memory().unwrap();
        seed_usage(&db, "t1", 0, "alice", 100, 50, None, None, None, 1000).await;
        seed_usage(&db, "t1", 1, "alice", 200, 80, None, None, None, 2000).await;
        seed_usage(&db, "t1", 2, "alice", 400, 100, None, None, None, 3000).await;
        let mid_only = db
            .aggregate_usage_by_agents(&["alice".into()], Some(1500), Some(2500))
            .await
            .unwrap();
        assert_eq!(mid_only.len(), 1);
        assert_eq!(mid_only[0].call_count, 1);
        assert_eq!(mid_only[0].input_tokens, 200);
        let lower_bound_only = db
            .aggregate_usage_by_agents(&["alice".into()], Some(2000), None)
            .await
            .unwrap();
        assert_eq!(lower_bound_only[0].call_count, 2);
        assert_eq!(lower_bound_only[0].input_tokens, 600);
    }

    #[tokio::test]
    async fn aggregate_usage_ignores_non_provider_usage_events() {
        let db = StateDatabase::in_memory().unwrap();
        // One legitimate ProviderUsage row.
        seed_usage(&db, "t1", 0, "alice", 100, 50, None, None, None, 1000).await;
        // A TextEmitted event with the same agent must NOT contribute.
        let textual = TaskTrace::new(
            "t1",
            1,
            AgentTraceEvent::TextEmitted {
                iteration: 0,
                stream: AgentTraceTextKind::Final,
                text: "alice said hi".into(),
            },
        );
        db.insert_trace(&textual).await.unwrap();
        let rows = db
            .aggregate_usage_by_agents(&["alice".into()], None, None)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].call_count, 1);
        assert_eq!(rows[0].input_tokens, 100);
    }

    // -------------------------------------------------------------------------
    // MoA advisor usage bucket (round-2 B6)
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn moa_advisor_usage_rolls_into_single_bucket() {
        let db = StateDatabase::in_memory().unwrap();
        // Two advisor usage rows + one real-agent row, all in the same window.
        seed_usage(
            &db,
            "t1",
            0,
            "moa:0:openai:gpt-5",
            100,
            10,
            None,
            None,
            None,
            1000,
        )
        .await;
        seed_usage(
            &db,
            "t1",
            1,
            "moa:1:deepseek:v4",
            200,
            20,
            None,
            None,
            None,
            1000,
        )
        .await;
        seed_usage(&db, "t1", 2, "main", 999, 99, None, None, None, 1000).await;

        let bucket = db
            .aggregate_moa_advisor_usage(None, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(bucket.agent_id, "moa-advisors");
        assert_eq!(bucket.call_count, 2);
        assert_eq!(bucket.input_tokens, 300);
        assert_eq!(bucket.output_tokens, 30);

        // Real agents are untouched by the bucket query, and an out-of-range
        // window with zero matching rows must yield None, not a NULL-coercion
        // error from the bare SUM aggregate.
        let empty = db
            .aggregate_moa_advisor_usage(Some(i64::MAX - 1), None)
            .await
            .unwrap();
        assert!(empty.is_none());
    }

    /// The denominator is always `input + cache_read` — there is no
    /// per-protocol shape, because every adapter stores disjoint counters.
    ///
    /// This case is the regression guard: it used to assert `800/1000 = 0.8`
    /// on the theory that an OpenAI-family row carries `cache_read` *inside*
    /// `input`. No adapter can produce that row (see
    /// `TokenUsage::cache_hit_ratio`), and asserting it locked in a rollup
    /// that over-reported the hit rate by up to 2x in exactly the degraded
    /// regime. The disjoint answer is 800/1800.
    #[test]
    fn agent_usage_total_cache_hit_ratio_uses_disjoint_prompt_total() {
        let total = AgentUsageTotal {
            agent_id: "alice".into(),
            call_count: 5,
            input_tokens: 1_000,
            output_tokens: 200,
            cache_read_tokens: 800,
            cache_creation_tokens: 50,
            reasoning_tokens: 0,
        };
        let ratio = total.cache_hit_ratio().expect("ratio present");
        assert!(
            (ratio - 800.0 / 1800.0).abs() < 1e-9,
            "expected 0.444…, got {ratio}"
        );
    }

    /// A genuine 50% hit rate must read as 50%, not 100%. `cache_read ==
    /// input` was the exact boundary the old `>` comparison put on the wrong
    /// side of the branch.
    #[test]
    fn agent_usage_total_cache_hit_ratio_half_cached_reads_as_half() {
        let total = AgentUsageTotal {
            agent_id: "alice".into(),
            call_count: 3,
            input_tokens: 50_000,
            cache_read_tokens: 50_000,
            ..Default::default()
        };
        let ratio = total.cache_hit_ratio().expect("ratio present");
        assert!((ratio - 0.5).abs() < 1e-9, "expected 0.5, got {ratio}");
    }

    #[test]
    fn agent_usage_total_cache_hit_ratio_anthropic_shape() {
        let total = AgentUsageTotal {
            agent_id: "alice".into(),
            call_count: 5,
            input_tokens: 100,
            output_tokens: 200,
            cache_read_tokens: 400,
            ..Default::default()
        };
        let ratio = total.cache_hit_ratio().expect("ratio present");
        assert!((ratio - 0.8).abs() < 1e-9, "expected 0.8, got {ratio}");
    }

    #[test]
    fn agent_usage_total_cache_hit_ratio_zero_when_no_hits_but_input() {
        let total = AgentUsageTotal {
            agent_id: "alice".into(),
            call_count: 1,
            input_tokens: 100,
            ..Default::default()
        };
        assert_eq!(total.cache_hit_ratio(), Some(0.0));
    }

    #[test]
    fn agent_usage_total_cache_hit_ratio_none_when_empty() {
        let total = AgentUsageTotal {
            agent_id: "alice".into(),
            ..Default::default()
        };
        assert_eq!(total.cache_hit_ratio(), None);
    }
}
