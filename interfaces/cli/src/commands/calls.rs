//! In-flight tool-call inspection + cancellation (Gap B follow-up).
//!
//! `aleph calls list`            → `tools.in_flight`
//! `aleph calls cancel <call_id>` → `tools.cancel_call`
//!
//! Production target: a tool is stuck or running away (long bash, runaway
//! `web_fetch`) and the user wants to abort just THAT call without losing the
//! rest of the session. Pair with `aleph calls list` to recover the
//! `tool_call_id` if you don't already have it.

use serde::Deserialize;
use serde_json::json;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliResult};

#[derive(Debug, Deserialize)]
struct InFlightCall {
    call_id: String,
    tool_name: String,
    started_at_ms: u64,
}

#[derive(Debug, Deserialize)]
struct InFlightResponse {
    calls: Vec<InFlightCall>,
    #[allow(dead_code)]
    count: usize,
}

#[derive(Debug, Deserialize)]
struct CancelResponse {
    #[allow(dead_code)]
    ok: bool,
    cancelled: bool,
    call_id: String,
}

/// `aleph calls list` — show every in-flight tool call.
pub async fn list(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    if json {
        let result: serde_json::Value = client.call("tools.in_flight", None::<()>).await?;
        output::print_json(&result);
        client.close().await?;
        return Ok(());
    }

    let response: InFlightResponse = client.call("tools.in_flight", None::<()>).await?;

    if response.calls.is_empty() {
        println!("(no in-flight tool calls)");
        client.close().await?;
        return Ok(());
    }

    println!("=== In-flight tool calls ===");
    println!();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    for call in &response.calls {
        let elapsed_ms = now_ms.saturating_sub(call.started_at_ms);
        println!(
            "  • {}  tool={}  elapsed={}.{:03}s",
            call.call_id,
            call.tool_name,
            elapsed_ms / 1000,
            elapsed_ms % 1000,
        );
    }
    println!();
    println!("Cancel one with: aleph calls cancel <call_id>");
    client.close().await?;
    Ok(())
}

/// `aleph calls cancel <call_id>` — fire the per-call cancellation token.
pub async fn cancel(
    server_url: &str,
    config: &CliConfig,
    call_id: &str,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = json!({ "call_id": call_id });
    if json {
        let result: serde_json::Value = client.call("tools.cancel_call", Some(params)).await?;
        output::print_json(&result);
        client.close().await?;
        return Ok(());
    }

    let response: CancelResponse = client.call("tools.cancel_call", Some(params)).await?;
    if response.cancelled {
        println!("cancelled: {}", response.call_id);
    } else {
        println!(
            "not in flight (already completed or unknown): {}",
            response.call_id
        );
    }
    client.close().await?;
    Ok(())
}
