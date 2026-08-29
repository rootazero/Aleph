//! Replay DTOs for querying persisted traces.
//!
//! Used by the gateway trace replay handler and consumed by
//! CLI / TUI / webchat frontends.

use serde::{Deserialize, Serialize};

use crate::events::AgentTraceEvent;

/// A single step in a trace replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTraceReplayEntry {
    pub step: u64,
    pub event: AgentTraceEvent,
}

/// Task metadata summary for replay display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTraceTaskSummary {
    pub task_id: String,
    pub session_id: String,
    pub agent_id: String,
    pub status: String,
    pub prompt_preview: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub started_at: Option<u64>,
    pub completed_at: Option<u64>,
    pub trace_count: usize,
    pub last_event_kind: Option<String>,
}

/// Full trace replay for a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTraceReplay {
    pub task: AgentTraceTaskSummary,
    pub traces: Vec<AgentTraceReplayEntry>,
}

/// One row of `trace.list`.
///
/// ⚠️ This type replaced `AgentTraceReplayListItem` on 2026-08-29, and the
/// difference is the point. That type declared `started_at: Option<String>` and
/// `status: String` — two REQUIRED-shaped fields the server had never emitted —
/// while the handler hand-wrote a `json!` object of `{task_id, event_count,
/// last_timestamp}` wrapped in a `{traces, next_cursor}` envelope. Three
/// clients each guessed a different shape and all three were wrong:
/// `aleph trace list` parsed the whole result as a sequence (`invalid type:
/// map, expected a sequence`), the TUI's `/replay list` parsed it as
/// `Vec<AgentTraceTaskSummary>`, and the Panel carried the CLI's bug with zero
/// call sites. Neither command had ever printed a row.
///
/// The fix is not a bigger DTO, it is a DIRECTION: `handle_list` now
/// **constructs** its response from [`AgentTraceListPage`] instead of parsing
/// against it, so over-sending a field with no reader is not expressible, and
/// the missing facts were made real (`task_traces.task_id` is a FK to
/// `agent_tasks(id)` with `ON DELETE RESTRICT`, so one LEFT JOIN always has a
/// parent row to read `status` / `started_at` / prompt from).
///
/// No field carries `#[serde(default)]`, deliberately: a server-side rename
/// must fail loudly at the client rather than render a column of dashes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTraceListRow {
    /// Also the `run_id`.
    pub task_id: String,
    /// `agent_tasks.status`, lowercased — `"unknown"` when the parent row has
    /// gone (the same word [`AgentTraceTaskSummary`]'s defensive arm uses).
    pub status: String,
    /// Epoch **seconds**, from `agent_tasks.started_at`. `None` = never started.
    pub started_at: Option<i64>,
    /// Epoch **seconds**: `MAX(task_traces.timestamp)` for this task.
    pub last_timestamp: i64,
    /// Persisted trace rows for this task.
    pub event_count: usize,
    /// First 200 characters of the task prompt; empty when there is no parent.
    pub prompt_preview: String,
}

/// The opaque page cursor `trace.list` hands back and accepts.
///
/// Compound on purpose: a single-timestamp cursor drops rows whose
/// `last_timestamp` collides with the previous page's last entry. Clients pass
/// it back verbatim and never construct one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTraceListCursor {
    pub last_timestamp: i64,
    pub task_id: String,
}

/// The `trace.list` response envelope.
///
/// A window must say what it was cut from — `next_cursor: None` is the only
/// honest way for a client to learn the listing is exhausted, and guessing it
/// from `rows.len() < limit` is the "is this page all of it?" question a client
/// structurally cannot answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTraceListPage {
    pub traces: Vec<AgentTraceListRow>,
    pub next_cursor: Option<AgentTraceListCursor>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `trace.list` envelope must round-trip, and no row field may gain a
    /// `#[serde(default)]` by accident.
    ///
    /// The second half is the one that matters: a defaulted field turns a
    /// renamed or dropped server key into a column of dashes forever, which
    /// reads as "no value yet" rather than as a broken contract. The type this
    /// replaced had two such fields the server never sent.
    ///
    /// ⚠️ `Option<T>` fields are exempt, and NOT by choice. serde gives every
    /// `Option` field an implicit default — a missing key deserializes to
    /// `None` with no attribute present — so absence cannot be made loud for
    /// them at all. This guard originally asserted "every one of them" and was
    /// red from the moment it was written, on `started_at`. The nullable set is
    /// DERIVED below (serialize a row whose optionals are `None`; the keys that
    /// come out `null` are exactly the exempt ones) rather than listed, so it
    /// cannot rot into an exemption for a field that stopped being optional.
    #[test]
    fn the_trace_list_page_round_trips_and_every_row_field_is_required() {
        let page = AgentTraceListPage {
            traces: vec![AgentTraceListRow {
                task_id: "run-1".to_string(),
                status: "completed".to_string(),
                started_at: Some(1_700_000_000),
                last_timestamp: 1_700_000_050,
                event_count: 7,
                prompt_preview: "summarise the audit".to_string(),
            }],
            next_cursor: Some(AgentTraceListCursor {
                last_timestamp: 1_700_000_050,
                task_id: "run-1".to_string(),
            }),
        };
        let json = serde_json::to_value(&page).expect("serialize");
        let back: AgentTraceListPage = serde_json::from_value(json.clone()).expect("deserialize");
        assert_eq!(back.traces.len(), 1);
        assert_eq!(back.traces[0].status, "completed");
        assert_eq!(back.next_cursor.expect("cursor").task_id, "run-1");

        // Which keys serde can be made to demand: derived, by serializing a row
        // whose every optional is `None` and reading off the nulls.
        let all_none = serde_json::to_value(AgentTraceListRow {
            task_id: String::new(),
            status: String::new(),
            started_at: None,
            last_timestamp: 0,
            event_count: 0,
            prompt_preview: String::new(),
        })
        .expect("serialize");
        let nullable: Vec<String> = all_none
            .as_object()
            .expect("row is an object")
            .iter()
            .filter(|(_, v)| v.is_null())
            .map(|(k, _)| k.clone())
            .collect();

        // Drop each non-nullable row key in turn: every one must refuse to parse.
        let row = json["traces"][0]
            .as_object()
            .expect("row is an object")
            .clone();
        assert_eq!(row.len(), 6, "row shape changed — update this guard");
        let mut checked = 0usize;
        for key in row.keys() {
            if nullable.contains(key) {
                continue;
            }
            let mut broken = row.clone();
            broken.remove(key);
            assert!(
                serde_json::from_value::<AgentTraceListRow>(serde_json::Value::Object(broken))
                    .is_err(),
                "`{key}` parsed while absent — a `#[serde(default)]` here turns a \
                 broken server contract into a silently empty column"
            );
            checked += 1;
        }
        // Self-check: the exemption must not have swallowed the whole row.
        assert_eq!(
            checked,
            row.len() - nullable.len(),
            "the nullable exemption is covering more keys than it derived"
        );
        assert!(
            checked >= 5,
            "only {checked} row keys were actually checked"
        );
    }

    #[test]
    fn roundtrip_agent_trace_replay() {
        let replay = AgentTraceReplay {
            task: AgentTraceTaskSummary {
                task_id: "task-001".to_string(),
                session_id: "session-1".to_string(),
                agent_id: "agent-1".to_string(),
                status: "completed".to_string(),
                prompt_preview: "hello".to_string(),
                created_at: 100,
                updated_at: 200,
                started_at: Some(110),
                completed_at: Some(190),
                trace_count: 1,
                last_event_kind: Some("session_completed".to_string()),
            },
            traces: vec![AgentTraceReplayEntry {
                step: 0,
                event: AgentTraceEvent::TurnStarted { iteration: 1 },
            }],
        };

        let json = serde_json::to_string(&replay).expect("serialize");
        let deserialized: AgentTraceReplay = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deserialized.task.task_id, "task-001");
        assert_eq!(deserialized.task.status, "completed");
        assert_eq!(deserialized.traces.len(), 1);
        assert_eq!(deserialized.traces[0].step, 0);
        assert_eq!(
            deserialized.traces[0].event,
            AgentTraceEvent::TurnStarted { iteration: 1 }
        );
    }
}
