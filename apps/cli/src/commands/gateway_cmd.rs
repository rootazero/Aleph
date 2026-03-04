//! Generic Gateway RPC call command

use serde_json::Value;

use crate::client::AlephClient;
use crate::error::{CliError, CliResult};
use crate::output;

/// Call any Gateway RPC method directly
pub async fn call(
    server_url: &str,
    method: &str,
    params_json: Option<&str>,
    json_mode: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    let params: Option<Value> = match params_json {
        Some(s) => {
            let v: Value = serde_json::from_str(s).map_err(|e| {
                CliError::Other(format!("Invalid JSON params: {}", e))
            })?;
            Some(v)
        }
        None => None,
    };

    let result: Value = client.call(method, params).await?;
    // Raw gateway calls always output JSON regardless of mode
    let _ = json_mode;
    output::print_json(&result);
    client.close().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn parse_valid_json() {
        let v: serde_json::Value = serde_json::from_str(r#"{"key": "value"}"#).unwrap();
        assert!(v.is_object());
    }

    #[test]
    fn parse_invalid_json() {
        assert!(serde_json::from_str::<serde_json::Value>("not json").is_err());
    }
}
