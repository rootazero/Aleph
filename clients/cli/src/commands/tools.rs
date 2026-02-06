//! Tools listing command

use serde::Deserialize;

use crate::client::AlephClient;
use crate::error::CliResult;

#[derive(Deserialize)]
struct Tool {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    category: Option<String>,
}

#[derive(Deserialize)]
struct ToolsResponse {
    tools: Vec<Tool>,
}

/// Run tools list command
pub async fn run(server_url: &str, category: Option<&str>) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let response: ToolsResponse = client.call("tools.list", None::<()>).await?;

    println!("=== Available Tools ===");
    println!();

    let mut tools = response.tools;

    // Filter by category if specified
    if let Some(cat) = category {
        tools.retain(|t| {
            t.category.as_ref().map(|c| c.contains(cat)).unwrap_or(false)
                || t.name.contains(cat)
        });
    }

    // Group by category
    let mut categories: std::collections::HashMap<String, Vec<&Tool>> = std::collections::HashMap::new();
    for tool in &tools {
        let cat = tool.category.clone().unwrap_or_else(|| "other".to_string());
        categories.entry(cat).or_default().push(tool);
    }

    let mut cat_names: Vec<_> = categories.keys().cloned().collect();
    cat_names.sort();

    for cat in cat_names {
        println!("[{}]", cat);
        if let Some(tools) = categories.get(&cat) {
            for tool in tools {
                print!("  • {}", tool.name);
                if let Some(desc) = &tool.description {
                    // Truncate long descriptions
                    let desc = if desc.len() > 50 {
                        format!("{}...", &desc[..47])
                    } else {
                        desc.clone()
                    };
                    print!(" - {}", desc);
                }
                println!();
            }
        }
        println!();
    }

    println!("Total: {} tools", tools.len());

    client.close().await?;
    Ok(())
}
