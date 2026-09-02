//! Diagnostics Handlers
//!
//! RPC surface for the unified self-diagnosis engine (`crate::diagnostics`).
//! Lets connected clients (the `aleph doctor` CLI, the Panel) run the same
//! check battery the daemon-side `doctor` tool uses, instead of duplicating
//! the checks client-side.

use crate::sync_primitives::Arc;
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::debug;

use crate::config::Config;
use crate::diagnostics::{DiagnosticEngine, Posture};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::security::SharedTokenManager;

/// Parameters for `diagnostics.run`. All optional — a bare call runs the
/// full battery in `Inspect` posture.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct DiagnosticsRunParams {
    /// Run in `Fix` posture. Note: Fix performs mechanical repairs only
    /// (mkdir of missing directories, stale-lock cleanup) — never config
    /// edits, credential changes, or LLM reasoning.
    pub fix: bool,
    /// Whitelist of check ids (e.g. `core/data-dir`); wins over `skip`.
    pub only: Option<Vec<String>>,
    /// Blacklist of check ids.
    pub skip: Vec<String>,
}

/// Handle diagnostics.run RPC request
///
/// Builds the engine (`default_registry` + runtime checks against the live
/// config/vault handles) and returns the stable report envelope (same shape
/// as the `doctor` tool's `--json` output: `ok`, `checksRun`, `errors`,
/// `warnings`, `repaired`, `findings`).
///
/// # Request
///
/// ```json
/// {
///   "jsonrpc": "2.0",
///   "method": "diagnostics.run",
///   "id": 1,
///   "params": { "fix": false, "only": ["core/data-dir"], "skip": [] }
/// }
/// ```
/// `mcp` is the live MCP manager handle. It is threaded in — rather than left
/// out and defaulted — so this RPC face and the `doctor` tool face register the
/// SAME battery: without it `ext/idle-extensions` would report the MCP category
/// as unknown here while answering it in the tool, and the two doctors would
/// disagree about the same machine.
pub async fn handle_run(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
    mcp: Option<crate::mcp::manager::McpManagerHandle>,
) -> JsonRpcResponse {
    debug!("Handling diagnostics.run");

    let params: DiagnosticsRunParams = match &request.params {
        Some(p) => match serde_json::from_value(p.clone()) {
            Ok(v) => v,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {e}"),
                )
            }
        },
        None => DiagnosticsRunParams::default(),
    };

    let engine = match DiagnosticEngine::default_registry() {
        Ok(e) => e
            .with_runtime_checks(config, vault)
            .with_extension_usage_check(mcp)
            // In-daemon: this process ran `aleph-server start`, so the
            // capability roster is a real fact here. Deliberately absent from
            // `default_registry()` — see that builder's doc.
            .with_capability_wiring_check()
            // Daemon-side: the projector and the event log are live here.
            .with_projection_holes_check(),
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to build diagnostic registry: {e}"),
            )
        }
    };

    let posture = if params.fix {
        Posture::Fix
    } else {
        Posture::Inspect
    };
    let report = engine
        .run_with_filter(posture, params.only.as_deref(), &params.skip)
        .await;

    // Serialize through the stable JSON envelope (to_json) so the RPC result
    // stays byte-identical to the other diagnostic surfaces.
    match serde_json::from_str::<serde_json::Value>(&report.to_json()) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize diagnostic report: {e}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn params_default_to_inspect_full_battery() {
        let params: DiagnosticsRunParams = serde_json::from_value(json!({})).unwrap();
        assert!(!params.fix);
        assert!(params.only.is_none());
        assert!(params.skip.is_empty());
    }

    #[test]
    fn params_parse_fix_only_skip() {
        let params: DiagnosticsRunParams = serde_json::from_value(json!({
            "fix": true,
            "only": ["core/data-dir"],
            "skip": ["core/stale-lock"]
        }))
        .unwrap();
        assert!(params.fix);
        assert_eq!(
            params.only.as_deref(),
            Some(&["core/data-dir".to_string()][..])
        );
        assert_eq!(params.skip, vec!["core/stale-lock".to_string()]);
    }
}
