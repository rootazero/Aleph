//! Vault secret administration commands.
//!
//! Pure I/O envelope (R4): wraps `secrets.{list, set, delete, verify,
//! providers}` JSON-RPC. Values pass between TTY (rpassword) and the
//! server only — they're never echoed or written to history.

use std::io::{self, IsTerminal, Read};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliError, CliResult};

#[derive(Debug, Deserialize)]
struct ListResponse {
    secrets: Vec<String>,
}

pub async fn list(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;
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

pub async fn set(
    server_url: &str,
    config: &CliConfig,
    name: &str,
    value: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let resolved = match value {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => prompt_for_secret(json)?,
    };
    if resolved.is_empty() {
        return Err(CliError::Other("secret value cannot be empty".into()));
    }

    let (client, _events) = AlephClient::connect(server_url, config).await?;
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

pub async fn delete(server_url: &str, config: &CliConfig, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;
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

pub async fn verify(server_url: &str, config: &CliConfig, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;
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

/// One row of `secrets.providers`. The external-provider plugin point was
/// removed server-side in the 2026-09-05 audit pass, so `account` and
/// `service_account_token_env` — which only an external provider ever carried
/// — were dropped here too: no producer can emit them, and their display arms
/// were branches the operator could never reach.
#[derive(Debug, Deserialize)]
struct ProviderEntry {
    key: String,
    #[serde(rename = "type")]
    provider_type: String,
    #[serde(default)]
    builtin: bool,
}

#[derive(Debug, Deserialize)]
struct ProvidersResponse {
    providers: Vec<ProviderEntry>,
}

pub async fn providers(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let result: Value = client.call("secrets.providers", None::<()>).await?;
    if json {
        output::print_json(&result);
    } else {
        let response: ProvidersResponse =
            serde_json::from_value(result.clone()).map_err(|e| CliError::Other(e.to_string()))?;
        let rows = provider_rows(&response);
        output::print_table(PROVIDER_HEADERS, &rows, false, &result);
    }
    client.close().await?;
    Ok(())
}

const PROVIDER_HEADERS: &[&str; 3] = &["KEY", "TYPE", "STATUS"];

/// Render the provider listing as table rows.
///
/// Split out of [`providers`] so the tests exercise the rows the operator
/// actually sees; asserting only that the response deserialises would test
/// serde, not this rendering.
fn provider_rows(response: &ProvidersResponse) -> Vec<Vec<String>> {
    response
        .providers
        .iter()
        .map(|p| {
            // `builtin` is the only qualifier the server sends. A row without
            // it renders "-" rather than guessing: absent means unknown, not
            // "external".
            let status = if p.builtin { "(built-in)" } else { "-" };
            vec![p.key.clone(), p.provider_type.clone(), status.to_string()]
        })
        .collect()
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

    /// The literal below is the **whole** `secrets.providers` response the
    /// server can produce today — `handle_secrets_providers` (alephcore) and
    /// `aleph-server secret providers` both emit exactly this one built-in row.
    /// The previous version of this test fabricated a second `1password` row
    /// that no producer has emitted since the 2026-09-05 audit pass, so it
    /// asserted a literal it had written itself.
    ///
    /// Caveat this test cannot lift: `ProvidersResponse` is a hand-written
    /// mirror of a shape owned by a crate `aleph-cli` deliberately does not
    /// depend on, so parsing proves only that the CLI accepts a superset. A
    /// real equality check needs a key set both sides construct from — there is
    /// no such shared constant today.
    #[test]
    fn the_builtin_local_row_renders_the_way_the_server_sends_it() {
        let raw = serde_json::json!({
            "providers": [
                { "key": "local", "type": "local_vault", "builtin": true }
            ]
        });
        let parsed: ProvidersResponse = serde_json::from_value(raw).unwrap();

        assert_eq!(
            provider_rows(&parsed),
            vec![vec![
                "local".to_string(),
                "local_vault".to_string(),
                "(built-in)".to_string()
            ]]
        );
    }

    /// A row that omits `builtin` must read as unknown, not as a claim. The
    /// status column is the only place this listing says anything beyond the
    /// two strings the server sent, so it must not invent a third.
    #[test]
    fn a_row_without_builtin_renders_a_dash_rather_than_guessing() {
        let raw = serde_json::json!({ "providers": [{ "key": "x", "type": "y" }] });
        let parsed: ProvidersResponse = serde_json::from_value(raw).unwrap();

        assert_eq!(provider_rows(&parsed)[0][2], "-");
    }
}
