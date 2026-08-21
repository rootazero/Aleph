//! Memory management commands

use serde_json::Value;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliResult};

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
            let content = item
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("-");
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

    /// An unscoped (whole-store) request has no honest per-agent graph count,
    /// so the handler sends JSON `null` for both graph fields. That must
    /// render as an explicit "-", never as "0" — a `0` would misreport an
    /// unknown count as a known empty one.
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
