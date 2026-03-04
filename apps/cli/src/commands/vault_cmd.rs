//! Vault (key management) commands

use serde_json::Value;

use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

/// Show vault status
pub async fn status(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("vault.status", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("=== Vault Status ===");
        println!();
        if let Some(obj) = result.as_object() {
            for (k, v) in obj {
                let display = match v {
                    Value::String(s) => s.clone(),
                    Value::Bool(b) => b.to_string(),
                    Value::Number(n) => n.to_string(),
                    Value::Null => "-".to_string(),
                    other => other.to_string(),
                };
                println!("  {}: {}", k, display);
            }
        }
    }

    client.close().await?;
    Ok(())
}

/// Store a master key (reads interactively, never from CLI args)
pub async fn store(server_url: &str, json: bool) -> CliResult<()> {
    let master_key = if json {
        // In JSON mode, read from stdin
        let mut key = String::new();
        std::io::stdin().read_line(&mut key).map_err(|e| {
            crate::error::CliError::Other(format!("Failed to read from stdin: {}", e))
        })?;
        key.trim().to_string()
    } else {
        // Interactive: prompt with hidden input
        rpassword::prompt_password("Enter master key: ").map_err(|e| {
            crate::error::CliError::Other(format!("Failed to read password: {}", e))
        })?
    };

    if master_key.is_empty() {
        if json {
            output::print_json(&serde_json::json!({"error": "Empty key provided"}));
        } else {
            eprintln!("Error: Empty key provided.");
        }
        return Ok(());
    }

    let (client, _events) = AlephClient::connect(server_url).await?;

    let params = serde_json::json!({ "master_key": master_key });
    let result: Value = client.call("vault.storeKey", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let success = result
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if success {
            println!("Master key stored in vault.");
        } else {
            println!("Failed to store key.");
        }
    }

    client.close().await?;
    Ok(())
}

/// Delete master key from vault
pub async fn delete(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("vault.deleteKey", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let was_present = result
            .get("was_present")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if was_present {
            println!("Master key deleted from vault.");
        } else {
            println!("No key was present in vault.");
        }
    }

    client.close().await?;
    Ok(())
}

/// Verify vault integrity
pub async fn verify(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let result: Value = client.call("vault.verify", None::<()>).await?;

    if json {
        output::print_json(&result);
    } else {
        let verified = result
            .get("verified")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let message = result
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if verified {
            println!("Vault verified: {}", message);
        } else {
            println!("Vault verification failed: {}", message);
        }
    }

    client.close().await?;
    Ok(())
}
