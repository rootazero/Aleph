//! Memory management commands

use serde_json::Value;

use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

/// Truncate a string to a maximum character length, appending "..." if truncated.
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{truncated}...")
    }
}

/// Search memory
pub async fn search(server_url: &str, query: &str, limit: usize, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "query": query, "limit": limit });
    let result: Value = client.call("memory.search", Some(params)).await?;

    let mut rows = Vec::new();
    if let Some(results) = result.as_array() {
        for item in results {
            let score = item
                .get("score")
                .and_then(|v| v.as_f64())
                .map(|s| format!("{:.3}", s))
                .unwrap_or_else(|| "-".to_string());
            let content = item
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            let source = item
                .get("source")
                .and_then(|v| v.as_str())
                .unwrap_or("-");
            rows.push(vec![
                score,
                truncate(content, 80),
                source.to_string(),
            ]);
        }
    }

    output::print_table(&["Score", "Content", "Source"], &rows, json, &result);

    client.close().await?;
    Ok(())
}

/// Show memory statistics
pub async fn stats(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("memory.stats", None::<()>).await?;

    let pairs = vec![
        (
            "Total Facts",
            result
                .get("total_facts")
                .and_then(|v| v.as_u64())
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "Total Sessions",
            result
                .get("total_sessions")
                .and_then(|v| v.as_u64())
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "Total Graphs",
            result
                .get("total_graphs")
                .and_then(|v| v.as_u64())
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "Storage Size",
            result
                .get("storage_size")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_string(),
        ),
        (
            "Last Compressed",
            result
                .get("last_compressed")
                .and_then(|v| v.as_str())
                .unwrap_or("-")
                .to_string(),
        ),
    ];

    output::print_detail(&pairs, json, &result);

    client.close().await?;
    Ok(())
}

/// Clear memory
pub async fn clear(server_url: &str, facts_only: bool, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let method = if facts_only {
        "memory.clearFacts"
    } else {
        "memory.clear"
    };

    let result: Value = client.call(method, None::<()>).await?;

    if json {
        output::print_json(&result);
    } else if facts_only {
        println!("Memory facts cleared.");
    } else {
        println!("All memory cleared.");
    }

    client.close().await?;
    Ok(())
}

/// Compress and optimize memory
pub async fn compress(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

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
pub async fn delete(server_url: &str, id: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "id": id });
    let result: Value = client.call("memory.delete", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Memory entry '{}' deleted.", id);
    }

    client.close().await?;
    Ok(())
}
