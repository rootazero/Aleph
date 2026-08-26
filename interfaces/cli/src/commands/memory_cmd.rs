//! Memory management commands

use serde_json::Value;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliResult};
use aleph_protocol::dreaming::{DreamGateBlock, DreamSchedulingStatus};

/// Truncate a string to a maximum character length, appending "..." if truncated.
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{truncated}...")
    }
}

/// Parse a `memory.search` response into table rows.
///
/// The response is an object: `{"memories": [...], "total": N}`. Calling
/// `as_array()` on it returned `None`, so this table was unconditionally
/// empty.
fn search_rows(result: &Value) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    if let Some(memories) = result.get("memories").and_then(Value::as_array) {
        for item in memories {
            let ts = item
                .get("timestamp")
                .and_then(Value::as_i64)
                .map_or_else(|| "-".to_string(), |t| t.to_string());
            let agent = item.get("agent_id").and_then(|v| v.as_str()).unwrap_or("-");
            let content = item.get("content").and_then(|v| v.as_str()).unwrap_or("-");
            rows.push(vec![ts, agent.to_string(), truncate(content, 80)]);
        }
    }
    rows
}

/// Search memory
pub async fn search(
    server_url: &str,
    config: &CliConfig,
    query: &str,
    limit: usize,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = serde_json::json!({ "query": query, "limit": limit });
    let result: Value = client.call("memory.search", Some(params)).await?;

    let rows = search_rows(&result);
    output::print_table(&["Timestamp", "Agent", "Content"], &rows, json, &result);

    client.close().await?;
    Ok(())
}

/// Parse a `memory.stats` response into display pairs.
///
/// Keys are camelCase on the wire (see gateway `handle_stats`). The previous
/// snake_case reads plus two keys the server never emits made every row
/// print "-".
fn stats_pairs(result: &Value) -> Vec<(&'static str, String)> {
    let num = |key: &str| -> String {
        result
            .get(key)
            .and_then(Value::as_i64)
            .map_or_else(|| "-".to_string(), |n| n.to_string())
    };

    vec![
        (
            "Scope",
            result
                .get("scope")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_string(),
        ),
        ("Note Memory", num("totalFacts")),
        ("Raw Memory", num("totalMemories")),
        // null when unscoped: the note graph is per-agent, so a store-wide
        // request has no honest single answer.
        ("Graph Nodes", num("totalGraphNodes")),
        ("Graph Edges", num("totalGraphEdges")),
    ]
}

/// Show memory statistics
pub async fn stats(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client.call("memory.stats", None::<()>).await?;

    let pairs = stats_pairs(&result);
    output::print_detail(&pairs, json, &result);

    client.close().await?;
    Ok(())
}

/// Explain why bulk clearing does not exist, and what to use instead.
///
/// This used to dispatch `memory.clear` / `memory.clearFacts`. Both server
/// handlers were unconditional `INTERNAL_ERROR` tombstones — bulk clearing is
/// not a thing in the notes-based memory model — so every invocation since
/// they were stubbed has been a round-trip that could only fail. The two
/// handlers are gone (zero consumers once this stopped calling them); the
/// explanation they carried is the part worth keeping, so it is stated here
/// with no server involved.
///
/// Still an `Err`: the user asked for a wipe and no wipe happened, so a
/// zero exit code would be its own small lie.
pub fn clear(facts_only: bool) -> CliResult<()> {
    let what = if facts_only { "notes" } else { "memory" };
    Err(aleph_client::CliError::Other(format!(
        "Bulk clearing of {what} is not supported.\n\
         \n\
         Knowledge notes are curated individually (the `note_manage` tool) and \n\
         retired by the dream daemon's decay pass; raw conversation rows are \n\
         deleted one at a time from the Panel's memory tab, or with the \n\
         `memory.delete` RPC."
    )))
}

/// Compress and optimize memory
pub async fn compress(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client.call("memory.compress", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let message = result
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Memory compressed successfully.");
        println!("{message}");
    }

    client.close().await?;
    Ok(())
}

/// Render the dream daemon's scheduling gates as display pairs.
///
/// Reads `aleph_protocol::dreaming::DreamSchedulingStatus` — the same type the
/// server builds the response from and the Panel deserialises — so the keys
/// cannot drift the way `providers list`'s did. Three "I don't know" cases
/// must each stay distinguishable from a real answer, because collapsing any
/// of them invents a reply the server never gave:
///
/// * no daemon in the server process (memory disabled, or a build with no
///   dreaming) is **not** `enabled = false`;
/// * a store that has never completed a cycle is **not** a failed one;
/// * a `duration_ms` the row does not carry is **not** zero.
fn dreaming_pairs(status: &DreamSchedulingStatus) -> Vec<(&'static str, String)> {
    let Some(d) = status.daemon.as_ref() else {
        return vec![(
            "State",
            "no dream daemon in this server process (memory disabled)".to_string(),
        )];
    };

    // Wording lives here; *which* gate is shut is `blocking_gate`, shared with
    // the Panel. A second ladder written from the raw fields would be a second
    // answer to one question, and the two would part company on the first
    // change to either.
    let state = match d.blocking_gate() {
        Some(DreamGateBlock::Disabled) => "disabled".to_string(),
        Some(DreamGateBlock::OutsideWindow) => format!(
            "waiting — outside the window ({}–{} local)",
            d.window_start_local, d.window_end_local
        ),
        Some(DreamGateBlock::UserActive) => format!(
            "waiting — user active ({}s idle, needs {}s)",
            d.idle_seconds, d.idle_threshold_seconds
        ),
        None if d.is_running => "running now".to_string(),
        None => "ready — every gate open".to_string(),
    };

    vec![
        ("State", state),
        (
            "Window",
            format!("{}–{} local", d.window_start_local, d.window_end_local),
        ),
        (
            "Idle",
            format!(
                "{}s (threshold {}s)",
                d.idle_seconds, d.idle_threshold_seconds
            ),
        ),
        ("Cycle cap", format!("{}s", d.max_duration_seconds)),
        (
            "Last run",
            status.last_run.as_ref().map_or_else(
                || "never".to_string(),
                |r| {
                    let when = chrono::DateTime::from_timestamp(r.run_at, 0).map_or_else(
                        || r.run_at.to_string(),
                        |t| {
                            t.with_timezone(&chrono::Local)
                                .format("%Y-%m-%d %H:%M:%S")
                                .to_string()
                        },
                    );
                    // `stale_running` is the server's crash tombstone, spelled
                    // out because "stale_running" tells an operator nothing
                    // and it is the one status that means a night was lost.
                    let what = if r.status == "stale_running" {
                        "crashed mid-cycle".to_string()
                    } else {
                        r.status.clone()
                    };
                    match r.duration_ms {
                        Some(ms) => format!("{when} · {what} ({ms} ms)"),
                        None => format!("{when} · {what}"),
                    }
                },
            ),
        ),
    ]
}

/// Explain why the dream daemon did or did not run.
///
/// The run history structurally cannot answer that — a cycle that never
/// started leaves no row — so this reads the daemon's live entry gates
/// instead. It exists because the Panel's memory pane was the only face that
/// could answer it, and the box most likely to be asked is the headless one
/// with no browser pointed at it.
pub async fn dreaming(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    // `dreaming.list_insights` carries the scheduling section in both its
    // branches (a refused partition answers with the same key set as an empty
    // success), so this needs no admin gate and no second RPC.
    let result: Value = client
        .call("dreaming.list_insights", Some(serde_json::json!({})))
        .await?;

    let status: DreamSchedulingStatus = serde_json::from_value(result.clone()).unwrap_or_default();
    let pairs = dreaming_pairs(&status);
    output::print_detail(&pairs, json, &result);

    client.close().await?;
    Ok(())
}

/// Delete a specific memory entry
pub async fn delete(server_url: &str, config: &CliConfig, id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = serde_json::json!({ "id": id });
    let result: Value = client.call("memory.delete", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Memory entry '{id}' deleted.");
    }

    client.close().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::dreaming::{DaemonStatus, DreamLastRun};

    /// Shaped exactly like `handle_search` in `src/gateway/handlers/memory.rs`:
    /// `{"memories": [MemoryEntry], "total": i64}`, not a bare array.
    fn sample_search_response() -> Value {
        serde_json::json!({
            "memories": [
                {
                    "id": "m1",
                    "agent_id": "main",
                    "content": "what's the weather",
                    "session_id": "s1",
                    "timestamp": 1_700_000_000_i64,
                },
                {
                    "id": "m2",
                    "agent_id": "main",
                    "content": "it's sunny",
                    "session_id": "s1",
                    "timestamp": 1_700_000_001_i64,
                },
            ],
            "total": 2,
        })
    }

    #[test]
    fn search_rows_reads_the_memories_field_not_the_top_level_array() {
        let rows = search_rows(&sample_search_response());
        assert_eq!(
            rows,
            vec![
                vec![
                    "1700000000".to_string(),
                    "main".to_string(),
                    "what's the weather".to_string(),
                ],
                vec![
                    "1700000001".to_string(),
                    "main".to_string(),
                    "it's sunny".to_string(),
                ],
            ]
        );
    }

    /// A row with no body renders a dash, not an empty cell. There is no
    /// second half to fall back to: `raw_memories` has one `content` column,
    /// and the `user_input`/`ai_output` pair this used to read was a shape the
    /// server filled with `("the whole row", "")` on every response.
    #[test]
    fn search_rows_renders_a_dash_for_a_row_with_no_body() {
        let response = serde_json::json!({
            "memories": [{
                "id": "m1",
                "agent_id": "main",
                "session_id": "s1",
                "timestamp": 1_700_000_000_i64,
            }],
            "total": 1,
        });
        let rows = search_rows(&response);
        assert_eq!(rows[0][2], "-");
    }

    #[test]
    fn search_rows_empty_memories_yields_no_rows() {
        let response = serde_json::json!({ "memories": [], "total": 0 });
        assert!(search_rows(&response).is_empty());
    }

    /// Shaped exactly like `handle_stats` in `src/gateway/handlers/memory.rs`:
    /// camelCase keys, plus `totalGraphNodes`/`totalGraphEdges` as real numbers
    /// for a scoped (per-agent) request.
    #[test]
    fn stats_pairs_reads_camel_case_keys() {
        let response = serde_json::json!({
            "totalMemories": 42,
            "totalFacts": 10,
            "totalGraphNodes": 7,
            "totalGraphEdges": 3,
            "scope": "agent",
        });
        assert_eq!(
            stats_pairs(&response),
            vec![
                ("Scope", "agent".to_string()),
                ("Note Memory", "10".to_string()),
                ("Raw Memory", "42".to_string()),
                ("Graph Nodes", "7".to_string()),
                ("Graph Edges", "3".to_string()),
            ]
        );
    }

    // An unscoped (whole-store) request has no honest per-agent graph count,
    // so the handler sends JSON `null` for both graph fields. That must
    // render as an explicit "-", never as "0" — a `0` would misreport an
    // unknown count as a known empty one.
    // -- dreaming ----------------------------------------------------------

    /// Round-trip a status through the wire the way the server sends it.
    ///
    /// Deliberately not a hand-written JSON literal: an assertion that reads
    /// back keys the test itself just typed is testing `serde_json`, not this
    /// command. The server builds its response by serialising this same type,
    /// so serialising and parsing here exercises the contract rather than a
    /// remembered copy of it.
    fn over_the_wire(status: &DreamSchedulingStatus) -> DreamSchedulingStatus {
        let wire = serde_json::to_value(status).expect("serialise");
        serde_json::from_value(wire).expect("parse")
    }

    fn daemon(enabled: bool, within_window: bool, user_active: bool) -> DaemonStatus {
        DaemonStatus {
            enabled,
            within_window,
            user_active,
            idle_seconds: 42,
            idle_threshold_seconds: 900,
            window_start_local: "02:00".to_string(),
            window_end_local: "06:00".to_string(),
            is_running: false,
            max_duration_seconds: 1800,
        }
    }

    fn state_of(status: &DreamSchedulingStatus) -> String {
        dreaming_pairs(status)
            .into_iter()
            .find(|(k, _)| *k == "State")
            .expect("State row")
            .1
    }

    /// "No daemon in this process" and "the daemon is switched off" are
    /// different answers, and only one of them means the operator has
    /// something to change. Collapsing them would put words in the server's
    /// mouth — it said nothing about `enabled`, because there was nobody to
    /// ask.
    #[test]
    fn an_absent_daemon_does_not_render_as_a_disabled_one() {
        let pairs = dreaming_pairs(&over_the_wire(&DreamSchedulingStatus::default()));
        assert_eq!(pairs.len(), 1, "no gates to report: {pairs:?}");
        assert!(
            pairs[0].1.contains("no dream daemon"),
            "must say the daemon is absent, not that it is disabled: {pairs:?}"
        );
        assert_ne!(state_of(&DreamSchedulingStatus::default()), "disabled");
    }

    /// Each gate gets its own sentence, and the ladder stops at the first shut
    /// one — naming a later gate would send the operator to change a setting
    /// that would not have mattered.
    #[test]
    fn each_shut_gate_renders_its_own_reason() {
        let off = DreamSchedulingStatus {
            daemon: Some(daemon(false, true, false)),
            last_run: None,
        };
        assert_eq!(state_of(&over_the_wire(&off)), "disabled");

        let outside = DreamSchedulingStatus {
            daemon: Some(daemon(true, false, false)),
            last_run: None,
        };
        let s = state_of(&over_the_wire(&outside));
        assert!(
            s.contains("outside the window") && s.contains("02:00–06:00"),
            "{s}"
        );

        let busy = DreamSchedulingStatus {
            daemon: Some(daemon(true, true, true)),
            last_run: None,
        };
        let s = state_of(&over_the_wire(&busy));
        assert!(
            s.contains("user active") && s.contains("42s") && s.contains("900s"),
            "{s}"
        );

        let ready = DreamSchedulingStatus {
            daemon: Some(daemon(true, true, false)),
            last_run: None,
        };
        assert!(state_of(&over_the_wire(&ready)).contains("ready"));

        let mut running_daemon = daemon(true, true, true);
        running_daemon.is_running = true;
        let running = DreamSchedulingStatus {
            daemon: Some(running_daemon),
            last_run: None,
        };
        // A running cycle is not blocked by the user being active — it yields
        // at the next stage boundary instead, which is not the same news.
        assert!(state_of(&over_the_wire(&running)).contains("running"));
    }

    /// `max_duration_seconds` is on the wire and was missing from the Panel's
    /// hand-written DTO, so it parsed clean and vanished for four rounds. It
    /// is the bound that decides when a stranded `running` row becomes a crash
    /// tombstone, which makes it part of the answer, not decoration.
    #[test]
    fn the_cycle_cap_reaches_the_output() {
        let status = DreamSchedulingStatus {
            daemon: Some(daemon(true, true, false)),
            last_run: None,
        };
        let pairs = dreaming_pairs(&over_the_wire(&status));
        let cap = pairs
            .iter()
            .find(|(k, _)| *k == "Cycle cap")
            .expect("Cycle cap row");
        assert_eq!(cap.1, "1800s");
    }

    /// Three distinct things a run row can mean, none of which may be read as
    /// another: never ran, ran and failed, died mid-cycle.
    #[test]
    fn last_run_distinguishes_never_from_failed_from_crashed() {
        let never = DreamSchedulingStatus {
            daemon: Some(daemon(true, true, false)),
            last_run: None,
        };
        let last = |s: &DreamSchedulingStatus| {
            dreaming_pairs(s)
                .into_iter()
                .find(|(k, _)| *k == "Last run")
                .expect("Last run row")
                .1
        };
        assert_eq!(last(&over_the_wire(&never)), "never");

        let failed = DreamSchedulingStatus {
            daemon: Some(daemon(true, true, false)),
            last_run: Some(DreamLastRun {
                run_at: 1_700_000_000,
                status: "error".to_string(),
                duration_ms: Some(1234),
            }),
        };
        let line = last(&over_the_wire(&failed));
        assert!(line.contains("error") && line.contains("1234 ms"), "{line}");

        let crashed = DreamSchedulingStatus {
            daemon: Some(daemon(true, true, false)),
            last_run: Some(DreamLastRun {
                run_at: 1_700_000_000,
                status: "stale_running".to_string(),
                duration_ms: None,
            }),
        };
        let line = last(&over_the_wire(&crashed));
        assert!(
            line.contains("crashed mid-cycle"),
            "the server's tombstone must be spelled out, not echoed as \
             `stale_running`: {line}"
        );
        // No duration recorded is not a zero-length run.
        assert!(!line.contains("0 ms"), "{line}");
    }

    #[test]
    fn stats_pairs_renders_null_graph_counts_as_dash_not_zero() {
        let response = serde_json::json!({
            "totalMemories": 5,
            "totalFacts": 0,
            "totalGraphNodes": null,
            "totalGraphEdges": null,
            "scope": "global",
        });
        let pairs = stats_pairs(&response);
        let graph_nodes = pairs.iter().find(|(k, _)| *k == "Graph Nodes").unwrap();
        let graph_edges = pairs.iter().find(|(k, _)| *k == "Graph Edges").unwrap();
        assert_eq!(graph_nodes.1, "-");
        assert_eq!(graph_edges.1, "-");
    }
}
