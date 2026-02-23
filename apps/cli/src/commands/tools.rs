//! Commands listing command

use serde::Deserialize;

use crate::client::AlephClient;
use crate::error::CliResult;

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
pub async fn run(server_url: &str, category: Option<&str>) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let response: CommandsResponse = client.call("commands.list", None::<()>).await?;

    println!("=== Available Commands ===");
    println!();

    let mut commands = response.commands;

    // Filter by category if specified
    if let Some(cat) = category {
        commands.retain(|c| {
            c.source_type.contains(cat)
                || c.key.contains(cat)
                || c.command_type.contains(cat)
        });
    }

    // Group by source type
    let mut sources: std::collections::HashMap<String, Vec<&Command>> = std::collections::HashMap::new();
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
        println!("[{}]", src);
        if let Some(cmds) = sources.get(&src) {
            for cmd in cmds {
                print!("  • {}", cmd.key);
                // Truncate long descriptions
                let desc = if cmd.description.len() > 50 {
                    format!("{}...", &cmd.description[..47])
                } else {
                    cmd.description.clone()
                };
                print!(" - {}", desc);
                println!();
            }
        }
        println!();
    }

    println!("Total: {} commands", commands.len());

    client.close().await?;
    Ok(())
}
