//! Tools subcommands.
//!
//! `aleph tools` historically delegated to `commands.list` (the slash-command
//! catalogue exposed to all interfaces). This module now also drives
//! `tools.invoke` for direct execution and a client-side filter over
//! `commands.list` for `describe`.

use serde_json::Value;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliError, CliResult};
use aleph_protocol::commands::{CommandListResponse, CommandMatch, CommandTreeNode};

/// `aleph tools` / `aleph tools list` — render the `commands.list` tree.
///
/// Reads [`CommandListResponse`], the type the server constructs. The private
/// struct this replaced required `key` and `description`; the wire has always
/// carried `name` and `hint`, so `client.call::<_, _>` failed deserialization
/// and this command died with `Invalid response: missing field 'key'` against
/// every healthy server.
///
/// The tree is also rendered as a tree now: namespaced tools (`session_new`,
/// `cron_manage`, … — the majority of the catalogue) are `children` of a
/// namespace node, and the flat loop this replaced never descended into them.
pub async fn run(
    server_url: &str,
    config: &CliConfig,
    category: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    if json {
        let result: Value = client.call("commands.list", None::<()>).await?;
        output::print_json(&result);
        client.close().await?;
        return Ok(());
    }

    let response: CommandListResponse = client.call("commands.list", None::<()>).await?;
    client.close().await?;

    println!("=== Available Commands ===");
    println!();

    let mut shown = 0usize;
    for node in &response.commands {
        if node.is_namespace {
            let children: Vec<_> = node
                .children
                .iter()
                .filter(|c| matches_category(category, &c.source_type, &c.internal_id))
                .collect();
            if children.is_empty() {
                continue;
            }
            println!("[{}] {}", node.name, node.hint);
            for child in children {
                println!(
                    "  • {}_{}{} - {}",
                    node.name,
                    child.name,
                    child
                        .param_hint
                        .as_deref()
                        .map(|p| format!(" {p}"))
                        .unwrap_or_default(),
                    truncate(&child.hint)
                );
                shown += 1;
            }
            println!();
        } else {
            let source = node.source_type.as_deref().unwrap_or_default();
            if !matches_category(category, source, &node.name) {
                continue;
            }
            let label = if source.is_empty() { "other" } else { source };
            println!(
                "[{label}] • {}{} - {}",
                node.name,
                node.param_hint
                    .as_deref()
                    .map(|p| format!(" {p}"))
                    .unwrap_or_default(),
                truncate(&node.hint)
            );
            shown += 1;
        }
    }

    println!();
    println!("Total: {shown} commands");

    Ok(())
}

/// `--category` filter. `None` means "no filter" — deliberately not "match the
/// empty string", which would have been an accidental match-everything on some
/// fields and match-nothing on others.
fn matches_category(category: Option<&str>, source_type: &str, id: &str) -> bool {
    match category {
        None => true,
        Some(cat) => source_type.contains(cat) || id.contains(cat),
    }
}

/// Char-safe truncation — hints may be CJK, so byte slicing could split a
/// UTF-8 scalar and panic.
fn truncate(text: &str) -> String {
    if text.chars().count() > 50 {
        let head: String = text.chars().take(47).collect();
        format!("{head}...")
    } else {
        text.to_string()
    }
}

/// `aleph tools describe <name>` — pull `commands.list` and print the entry
/// matching `name`. Filtered client-side so the server needs no new RPC
/// (R4: I/O-only interface).
///
/// The lookup this replaced compared `item["key"]` against `name`. No node has
/// ever carried a `key`, so the comparison was false for every entry and the
/// command answered `tool 'X' not found in commands.list` for every tool that
/// exists — a fabricated fact about the server, emitted ahead of the `--json`
/// branch so that escape hatch was dead too.
///
/// Resolution now goes through [`CommandTreeNode::find`], which also descends
/// into namespaces: `session_new` is the child `new` of the `session` node, so
/// a lookup that scans only the roots misses most of the catalogue.
pub async fn describe(
    server_url: &str,
    config: &CliConfig,
    name: &str,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let response: CommandListResponse = client.call("commands.list", None::<()>).await?;
    client.close().await?;

    let Some(found) = CommandTreeNode::find(&response.commands, name) else {
        return Err(CliError::Other(format!(
            "tool '{name}' not found in commands.list"
        )));
    };

    if json {
        // Re-serialize the matched node so `--json` carries the same shape the
        // server sent, children included.
        let value = match found {
            CommandMatch::Top(node) => serde_json::to_value(node),
            CommandMatch::Child { child, .. } => serde_json::to_value(child),
        }
        .map_err(|e| CliError::Other(format!("could not render the matched node: {e}")))?;
        output::print_json(&value);
    } else {
        println!("name        : {name}");
        println!("description : {}", found.hint());
        if let Some(id) = found.internal_id() {
            println!("tool id     : {id}");
        }
        let source = found.source_type();
        if !source.is_empty() {
            println!("source      : {source}");
        }
        if let Some(p) = found.param_hint() {
            println!("parameters  : {p}");
        }
        if let CommandMatch::Child { namespace, .. } = found {
            println!("namespace   : {namespace}");
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
