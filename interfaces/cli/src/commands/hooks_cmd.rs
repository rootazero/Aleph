//! User-level hooks administration commands.
//!
//! Pure I/O envelope (R4): wraps `hooks.{list, add, remove, reload,
//! events}` JSON-RPC. Operates on `~/.aleph/hooks.json` — the global
//! user hooks file. Project hooks (under `<repo>/.aleph/hooks.json`)
//! are not exposed here because the server runs in a different cwd
//! than the operator's shell.

use serde_json::{json, Value};

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliError, CliResult};

pub async fn list(server_url: &str, config: &CliConfig, json_out: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let result: Value = client.call("hooks.list", None::<()>).await?;
    if json_out {
        output::print_json(&result);
        client.close().await?;
        return Ok(());
    }

    let path = result.get("path").and_then(|v| v.as_str()).unwrap_or("?");
    let exists = result
        .get("exists")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let events = result
        .get("events")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    println!(
        "hooks file : {path} ({})",
        if exists { "present" } else { "missing" }
    );
    if events.is_empty() {
        println!();
        println!("No user hooks defined. Use `aleph hooks add` to create one.");
        client.close().await?;
        return Ok(());
    }

    let mut event_names: Vec<&String> = events.keys().collect();
    event_names.sort();
    for event in event_names {
        let groups = events.get(event).and_then(|v| v.as_array());
        let count = groups.map_or(0, std::vec::Vec::len);
        println!();
        println!("[{event}] {count} group(s)");
        if let Some(groups) = groups {
            for (idx, g) in groups.iter().enumerate() {
                let matcher = g.get("matcher").and_then(|v| v.as_str()).unwrap_or("*");
                let actions = g
                    .get("hooks")
                    .or_else(|| g.get("actions"))
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                println!("  #{idx} matcher='{matcher}'");
                for a in &actions {
                    let kind = a.get("type").and_then(|v| v.as_str()).unwrap_or("?");
                    let label = a
                        .get("command")
                        .or_else(|| a.get("url"))
                        .or_else(|| a.get("prompt"))
                        .or_else(|| a.get("agent"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let timeout = a.get("timeout_secs").and_then(serde_json::Value::as_u64);
                    if let Some(t) = timeout {
                        println!("      - {kind}: {label}  (timeout {t}s)");
                    } else {
                        println!("      - {kind}: {label}");
                    }
                }
            }
        }
    }

    client.close().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn add(
    server_url: &str,
    config: &CliConfig,
    event: &str,
    command: Option<&str>,
    prompt: Option<&str>,
    agent: Option<&str>,
    url: Option<&str>,
    matcher: Option<&str>,
    timeout_secs: Option<u64>,
    json_out: bool,
) -> CliResult<()> {
    // Validate exactly-one client-side too so misuse is caught before the round-trip.
    let count = [command, prompt, agent, url]
        .iter()
        .filter(|o| o.is_some())
        .count();
    if count != 1 {
        return Err(CliError::Other(
            "must set exactly one of --command / --prompt / --agent / --url".into(),
        ));
    }

    let mut body = serde_json::Map::new();
    body.insert("event".into(), json!(event));
    if let Some(c) = command {
        body.insert("command".into(), json!(c));
    }
    if let Some(p) = prompt {
        body.insert("prompt".into(), json!(p));
    }
    if let Some(a) = agent {
        body.insert("agent".into(), json!(a));
    }
    if let Some(u) = url {
        body.insert("url".into(), json!(u));
    }
    if let Some(m) = matcher {
        if !m.is_empty() {
            body.insert("matcher".into(), json!(m));
        }
    }
    if let Some(t) = timeout_secs {
        body.insert("timeout_secs".into(), json!(t));
    }

    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let result: Value = client.call("hooks.add", Some(Value::Object(body))).await?;
    if json_out {
        output::print_json(&result);
    } else {
        println!("✓ Hook added for event '{event}'");
    }
    client.close().await?;
    Ok(())
}

pub async fn remove(
    server_url: &str,
    config: &CliConfig,
    event: &str,
    command: Option<&str>,
    index: Option<usize>,
    json_out: bool,
) -> CliResult<()> {
    if command.is_none() && index.is_none() {
        return Err(CliError::Other(
            "must set either --command <substring> or --index <n>".into(),
        ));
    }
    let mut body = serde_json::Map::new();
    body.insert("event".into(), json!(event));
    if let Some(c) = command {
        body.insert("command".into(), json!(c));
    }
    if let Some(i) = index {
        body.insert("index".into(), json!(i));
    }

    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let result: Value = client
        .call("hooks.remove", Some(Value::Object(body)))
        .await?;
    if json_out {
        output::print_json(&result);
    } else {
        let removed = result
            .get("removed")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        println!(
            "✓ Removed {removed} hook entr{}",
            if removed == 1 { "y" } else { "ies" }
        );
    }
    client.close().await?;
    Ok(())
}

pub async fn reload(server_url: &str, config: &CliConfig, json_out: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let result: Value = client.call("hooks.reload", None::<()>).await?;
    if json_out {
        output::print_json(&result);
    } else {
        let count = result
            .get("loaded_plugins")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        println!("✓ Extensions reloaded ({count} plugins loaded)");
    }
    client.close().await?;
    Ok(())
}

pub async fn events(server_url: &str, config: &CliConfig, json_out: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let result: Value = client.call("hooks.events", None::<()>).await?;
    if json_out {
        output::print_json(&result);
    } else {
        let arr = result.get("events").and_then(|v| v.as_array());
        match arr {
            Some(events) => {
                println!("Valid hook events:");
                for e in events {
                    if let Some(s) = e.as_str() {
                        println!("  - {s}");
                    }
                }
            }
            None => println!("(no events returned)"),
        }
    }
    client.close().await?;
    Ok(())
}
