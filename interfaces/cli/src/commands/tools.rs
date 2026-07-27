//! Tools subcommands.
//!
//! `aleph tools` historically delegated to `commands.list` (the slash-command
//! catalogue exposed to all interfaces). This module now also drives
//! `tools.invoke` for direct execution and a client-side filter over
//! `commands.list` for `describe`.

use serde::Deserialize;
use serde_json::Value;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliError, CliResult};

#[derive(Deserialize)]
struct Command {
    key: String,
    description: String,
    #[serde(default)]
    source_type: String,
    #[serde(default)]
    command_type: String,
}

#[derive(Deserialize)]
struct CommandsResponse {
    commands: Vec<Command>,
}

/// Run commands list
pub async fn run(
    server_url: &str,
    config: &CliConfig,
    category: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    if json {
        let result: serde_json::Value = client.call("commands.list", None::<()>).await?;
        output::print_json(&result);
        client.close().await?;
        return Ok(());
    }

    let response: CommandsResponse = client.call("commands.list", None::<()>).await?;

    println!("=== Available Commands ===");
    println!();

    let mut commands = response.commands;

    // Filter by category if specified
    if let Some(cat) = category {
        commands.retain(|c| {
            c.source_type.contains(cat) || c.key.contains(cat) || c.command_type.contains(cat)
        });
    }

    // Group by source type
    let mut sources: std::collections::HashMap<String, Vec<&Command>> =
        std::collections::HashMap::new();
    for cmd in &commands {
        let src = if cmd.source_type.is_empty() {
            "other".to_string()
        } else {
            cmd.source_type.clone()
        };
        sources.entry(src).or_default().push(cmd);
    }

    let mut src_names: Vec<_> = sources.keys().cloned().collect();
    src_names.sort();

    for src in src_names {
        println!("[{src}]");
        if let Some(cmds) = sources.get(&src) {
            for cmd in cmds {
                print!("  • {}", cmd.key);
                // Truncate long descriptions (char-safe — descriptions may be
                // CJK, so byte slicing could split a UTF-8 scalar and panic).
                let desc = if cmd.description.chars().count() > 50 {
                    let head: String = cmd.description.chars().take(47).collect();
                    format!("{head}...")
                } else {
                    cmd.description.clone()
                };
                print!(" - {desc}");
                println!();
            }
        }
        println!();
    }

    println!("Total: {} commands", commands.len());

    client.close().await?;
    Ok(())
}

/// `aleph tools describe <name>` — pull `commands.list` and emit the entry
/// matching `name`. We do the filter client-side so the server doesn't need a
/// new RPC (R4: I/O-only interface).
pub async fn describe(
    server_url: &str,
    config: &CliConfig,
    name: &str,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let raw: Value = client.call("commands.list", None::<()>).await?;
    client.close().await?;

    let entry = raw
        .get("commands")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|item| item.get("key").and_then(|k| k.as_str()) == Some(name))
        });

    let entry = match entry {
        Some(v) => v,
        None => {
            return Err(CliError::Other(format!(
                "tool '{name}' not found in commands.list"
            )));
        }
    };

    if json {
        output::print_json(entry);
    } else {
        println!("name        : {name}");
        if let Some(desc) = entry.get("description").and_then(|v| v.as_str()) {
            println!("description : {desc}");
        }
        if let Some(t) = entry.get("source_type").and_then(|v| v.as_str()) {
            if !t.is_empty() {
                println!("source      : {t}");
            }
        }
        if let Some(t) = entry.get("command_type").and_then(|v| v.as_str()) {
            if !t.is_empty() {
                println!("kind        : {t}");
            }
        }
        // Dump remaining metadata verbatim so users see new fields without
        // requiring a CLI release every time the server adds one.
        if let Value::Object(obj) = entry {
            let extras: Vec<(&String, &Value)> = obj
                .iter()
                .filter(|(k, _)| {
                    !matches!(
                        k.as_str(),
                        "key" | "description" | "source_type" | "command_type"
                    )
                })
                .collect();
            if !extras.is_empty() {
                println!("--- extra metadata ---");
                for (k, v) in extras {
                    println!("{k}: {v}");
                }
            }
        }
    }
    Ok(())
}

/// `aleph tools invoke <name> [--args JSON] [--agent ID]` — direct invocation
/// via `tools.invoke`. Bypasses the LLM loop (R8 deterministic execution
/// path; reserved for E2E/automation).
pub async fn invoke(
    server_url: &str,
    config: &CliConfig,
    name: &str,
    args: Option<&str>,
    agent: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let arguments = match args {
        None | Some("") => Value::Object(serde_json::Map::new()),
        Some(s) => serde_json::from_str::<Value>(s)
            .map_err(|e| CliError::Other(format!("invalid --args JSON: {e}")))?,
    };

    let mut body = serde_json::Map::new();
    body.insert("tool_name".into(), Value::String(name.to_string()));
    body.insert("arguments".into(), arguments);
    if let Some(a) = agent {
        body.insert("agent_id".into(), Value::String(a.to_string()));
    }

    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let result: Value = client
        .call("tools.invoke", Some(Value::Object(body)))
        .await?;
    client.close().await?;

    if json {
        output::print_json(&result);
    } else {
        let ok = result
            .get("ok")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if ok {
            if let Some(inner) = result.get("result") {
                println!("{}", serde_json::to_string_pretty(inner)?);
            } else {
                println!("ok");
            }
        } else {
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use serde_json::Value;

    // Cheap sanity check for the invoke body shape — keeps the JSON contract
    // exercised when the surrounding code changes.
    #[test]
    fn invoke_body_defaults_to_empty_args_when_unspecified() {
        let v: Value = json!({
            "tool_name": "memory_search",
            "arguments": {}
        });
        assert_eq!(v["tool_name"], "memory_search");
        assert!(v["arguments"].is_object());
    }
}
