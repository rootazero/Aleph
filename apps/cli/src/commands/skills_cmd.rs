//! Skill management commands
//!
//! Merges two server-side skill systems:
//! - `skills.*` — SKILL.md file-based skills
//! - `markdown_skills.*` — runtime-loaded markdown skills

use serde_json::Value;

use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

/// List all skills (file-based and runtime-loaded), merged into one table.
pub async fn list(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    // Call both RPC endpoints; silently continue if either fails.
    let file_skills: Option<Value> = client.call("skills.list", None::<()>).await.ok();
    let md_skills: Option<Value> = client.call("markdown_skills.list", None::<()>).await.ok();

    let mut rows = Vec::new();
    let mut raw_items: Vec<Value> = Vec::new();

    // Collect file-based skills
    if let Some(ref val) = file_skills {
        if let Some(items) = val.as_array() {
            for s in items {
                let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                let desc = s
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-");
                rows.push(vec![
                    name.to_string(),
                    "file".to_string(),
                    desc.to_string(),
                ]);
                let mut item = s.clone();
                if let Some(obj) = item.as_object_mut() {
                    obj.insert("type".to_string(), Value::String("file".to_string()));
                }
                raw_items.push(item);
            }
        }
    }

    // Collect markdown skills
    if let Some(ref val) = md_skills {
        if let Some(items) = val.as_array() {
            for s in items {
                let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                let desc = s
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-");
                rows.push(vec![
                    name.to_string(),
                    "markdown".to_string(),
                    desc.to_string(),
                ]);
                let mut item = s.clone();
                if let Some(obj) = item.as_object_mut() {
                    obj.insert("type".to_string(), Value::String("markdown".to_string()));
                }
                raw_items.push(item);
            }
        }
    }

    let raw = Value::Array(raw_items);
    output::print_table(&["Name", "Type", "Description"], &rows, json, &raw);

    client.close().await?;
    Ok(())
}

/// Install a skill from source.
///
/// If the source ends with `.md`, calls `markdown_skills.install`.
/// Otherwise calls `skills.install`.
pub async fn install(server_url: &str, source: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let (method, params) = if source.ends_with(".md") {
        (
            "markdown_skills.install",
            serde_json::json!({ "path": source }),
        )
    } else {
        ("skills.install", serde_json::json!({ "source": source }))
    };

    let result: Value = client.call(method, Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Skill installed from '{}'.", source);
    }

    client.close().await?;
    Ok(())
}

/// Reload a markdown skill by name.
pub async fn reload(server_url: &str, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "name": name });
    let result: Value = client.call("markdown_skills.reload", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Skill '{}' reloaded.", name);
    }

    client.close().await?;
    Ok(())
}

/// Delete/unload a skill by name.
///
/// Tries `skills.delete` first. If that fails, falls back to `markdown_skills.unload`.
pub async fn delete(server_url: &str, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "name": name });

    // Try file-based delete first, then markdown unload
    let result: Value = match client.call("skills.delete", Some(params.clone())).await {
        Ok(v) => v,
        Err(_) => client.call("markdown_skills.unload", Some(params)).await?,
    };

    if json {
        output::print_json(&result);
    } else {
        println!("Skill '{}' deleted.", name);
    }

    client.close().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn md_extension_detection() {
        let source_md = "my-skill.md";
        let source_url = "https://example.com/skill";
        let source_path = "/tmp/skills/helper";

        assert!(source_md.ends_with(".md"));
        assert!(!source_url.ends_with(".md"));
        assert!(!source_path.ends_with(".md"));
    }
}
