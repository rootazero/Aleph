//! Vault secret administration commands.
//!
//! Pure I/O envelope (R4): wraps `secrets.{list, set, delete, verify,
//! providers}` JSON-RPC. Values pass between TTY (rpassword) and the
//! server only — they're never echoed or written to history.

use std::io::{self, IsTerminal, Read};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::output;
use aleph_client::{AlephClient, CliError, CliResult};

#[derive(Debug, Deserialize)]
struct ListResponse {
    secrets: Vec<String>,
}

pub async fn list(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    let result: Value = client.call("secrets.list", None::<()>).await?;
    if json {
        output::print_json(&result);
    } else {
        let response: ListResponse =
            serde_json::from_value(result.clone()).map_err(|e| CliError::Other(e.to_string()))?;
        if response.secrets.is_empty() {
            println!("No secrets found");
        } else {
            let headers = &["NAME"];
            let rows: Vec<Vec<String>> = response.secrets.iter().map(|n| vec![n.clone()]).collect();
            output::print_table(headers, &rows, false, &result);
            println!();
            println!("Total: {} secrets", response.secrets.len());
        }
    }
    client.close().await?;
    Ok(())
}

pub async fn set(server_url: &str, name: &str, value: Option<&str>, json: bool) -> CliResult<()> {
    let resolved = match value {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => prompt_for_secret(json)?,
    };
    if resolved.is_empty() {
        return Err(CliError::Other("secret value cannot be empty".into()));
    }

    let (client, _events) = AlephClient::connect(server_url).await?;
    let body = json!({ "key": name, "value": resolved });
    let result: Value = client.call("secrets.set", Some(body)).await?;
    if json {
        output::print_json(&result);
    } else {
        let stored = result.get("key").and_then(|v| v.as_str()).unwrap_or(name);
        println!("Stored secret '{stored}'");
    }
    client.close().await?;
    Ok(())
}

pub async fn delete(server_url: &str, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    let body = json!({ "key": name });
    let result: Value = client.call("secrets.delete", Some(body)).await?;
    if json {
        output::print_json(&result);
    } else {
        println!("Deleted secret '{name}'");
    }
    client.close().await?;
    Ok(())
}

pub async fn verify(server_url: &str, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    let body = json!({ "key": name });
    let result: Value = client.call("secrets.verify", Some(body)).await?;
    if json {
        output::print_json(&result);
    } else {
        let bytes = result
            .get("bytes")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        println!("Secret '{name}' is available ({bytes} bytes, value redacted)");
    }
    client.close().await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ProviderEntry {
    key: String,
    #[serde(rename = "type")]
    provider_type: String,
    #[serde(default)]
    builtin: bool,
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    service_account_token_env: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProvidersResponse {
    providers: Vec<ProviderEntry>,
}

pub async fn providers(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    let result: Value = client.call("secrets.providers", None::<()>).await?;
    if json {
        output::print_json(&result);
    } else {
        let response: ProvidersResponse =
            serde_json::from_value(result.clone()).map_err(|e| CliError::Other(e.to_string()))?;
        let headers = &["KEY", "TYPE", "ACCOUNT/ENV"];
        let rows: Vec<Vec<String>> = response
            .providers
            .iter()
            .map(|p| {
                let extra = match (&p.account, &p.service_account_token_env) {
                    (Some(a), Some(e)) => format!("{a} (env: {e})"),
                    (Some(a), None) => a.clone(),
                    (None, Some(e)) => format!("env: {e}"),
                    (None, None) if p.builtin => "(built-in)".into(),
                    (None, None) => "-".into(),
                };
                vec![p.key.clone(), p.provider_type.clone(), extra]
            })
            .collect();
        output::print_table(headers, &rows, false, &result);
    }
    client.close().await?;
    Ok(())
}

/// Read a secret value from stdin (hidden TTY when interactive, raw read
/// otherwise). Matches the `aleph-server secret set` UX so muscle memory
/// carries over.
fn prompt_for_secret(json: bool) -> CliResult<String> {
    if json {
        // In machine mode, read whatever stdin hands us — no prompt, no echo
        // suppression. Callers piping `echo ...` get exactly what they sent.
        // Guard against interactive TTY (blocks waiting for EOF).
        if std::io::stdin().is_terminal() {
            return Err(CliError::Other(
                "stdin is a terminal; pipe input or use non-JSON interactive mode".into(),
            ));
        }
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| CliError::Other(format!("read stdin: {e}")))?;
        return Ok(buf.trim_end_matches(['\n', '\r']).to_string());
    }
    rpassword::prompt_password("Secret value (input hidden): ")
        .map_err(|e| CliError::Other(format!("read password: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_response_deserialises() {
        let raw = serde_json::json!({ "secrets": ["a", "b"] });
        let parsed: ListResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.secrets, vec!["a", "b"]);
    }

    #[test]
    fn providers_response_deserialises() {
        let raw = serde_json::json!({
            "providers": [
                { "key": "local", "type": "local_vault", "builtin": true },
                { "key": "op", "type": "1password", "builtin": false, "account": "acme" }
            ]
        });
        let parsed: ProvidersResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.providers.len(), 2);
        assert!(parsed.providers[0].builtin);
        assert_eq!(parsed.providers[1].account.as_deref(), Some("acme"));
    }
}
