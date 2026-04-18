//! Runtime RPC handlers: list + install + refresh.

use crate::sync_primitives::Arc;

use serde::Serialize;

use crate::gateway::event_bus::{GatewayEvent, GatewayEventBus, RuntimeInstallProgressEvent};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::runtimes::ledger::{CapabilityLedger, CapabilityStatus};
use crate::runtimes::{ensure_capability, find_spec, supported_on_current_os, SPECS};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeInfo {
    pub name: String,
    pub status: CapabilityStatus,
    pub bin_path: Option<String>,
    pub version: Option<String>,
    pub llm_hint: Option<String>,
    pub deps: Vec<String>,
    pub supported_on_current_os: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimesListResponse {
    pub runtimes: Vec<RuntimeInfo>,
}

fn build_list(ledger: &CapabilityLedger) -> RuntimesListResponse {
    let runtimes = SPECS
        .iter()
        .map(|spec| {
            let entry = ledger.entries.get(spec.name);
            let status = entry.map(|e| e.status).unwrap_or(CapabilityStatus::Missing);
            let bin_path = entry
                .filter(|e| !e.bin_path.as_os_str().is_empty())
                .map(|e| e.bin_path.to_string_lossy().to_string());
            let version = entry
                .filter(|e| !e.version.is_empty())
                .map(|e| e.version.clone());
            RuntimeInfo {
                name: spec.name.to_string(),
                status,
                bin_path,
                version,
                llm_hint: spec.llm_hint.map(str::to_string),
                deps: spec.deps.iter().map(|d| d.to_string()).collect(),
                supported_on_current_os: supported_on_current_os(spec.name),
            }
        })
        .collect();
    RuntimesListResponse { runtimes }
}

pub async fn handle_list(
    request: JsonRpcRequest,
    ledger: Arc<RwLock<CapabilityLedger>>,
) -> JsonRpcResponse {
    let guard = ledger.read().await;
    let response = build_list(&guard);
    match serde_json::to_value(&response) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("serialize: {e}")),
    }
}

pub async fn handle_refresh(
    request: JsonRpcRequest,
    ledger: Arc<RwLock<CapabilityLedger>>,
) -> JsonRpcResponse {
    for spec in SPECS {
        let probe_result = crate::runtimes::probe::probe(spec.name);
        let mut guard = ledger.write().await;
        if probe_result.found {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            guard.update(crate::runtimes::ledger::CapabilityEntry {
                name: spec.name.to_string(),
                bin_path: probe_result.bin_path.unwrap_or_default(),
                version: probe_result.version.unwrap_or_default(),
                status: CapabilityStatus::Ready,
                source: probe_result.source,
                last_probed: now,
            });
        } else {
            guard.update_status(spec.name, CapabilityStatus::Missing);
        }
    }
    let guard = ledger.read().await;
    let response = build_list(&guard);
    match serde_json::to_value(&response) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("serialize: {e}")),
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct InstallParams {
    pub capability: String,
}

pub async fn handle_install(
    request: JsonRpcRequest,
    ledger: Arc<RwLock<CapabilityLedger>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: InstallParams = match request.params.clone() {
        Some(p) => match serde_json::from_value(p) {
            Ok(v) => v,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("invalid params: {e}"),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                "missing 'capability' param",
            );
        }
    };

    if find_spec(&params.capability).is_none() {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("unknown capability: {}", params.capability),
        );
    }

    let cap = params.capability.clone();
    let cap_for_event = params.capability.clone();
    let bus = event_bus.clone();

    tokio::spawn(async move {
        let _ = bus.publish_json(&GatewayEvent::RuntimeInstallProgress(
            RuntimeInstallProgressEvent {
                step: cap_for_event.clone(),
                status: "started".into(),
                log_line: None,
                error: None,
                stderr: None,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        ));
        let result = ensure_capability(&cap, &ledger).await;
        let bus2 = bus.clone();
        let event = match result {
            Ok(_) => RuntimeInstallProgressEvent {
                step: cap_for_event,
                status: "done".into(),
                log_line: None,
                error: None,
                stderr: None,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
            Err(e) => {
                let err_str = e.to_string();
                // Task 4 will emit "Stderr tail: <tail>" as part of the error string.
                // Extract that tail if present; otherwise fall back to the whole error.
                let stderr = err_str
                    .split_once("Stderr tail: ")
                    .map(|(_, tail)| tail.to_string())
                    .or_else(|| Some(err_str.clone()));
                RuntimeInstallProgressEvent {
                    step: cap_for_event,
                    status: "failed".into(),
                    log_line: None,
                    error: Some(err_str),
                    stderr,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                }
            }
        };
        let _ = bus2.publish_json(&GatewayEvent::RuntimeInstallProgress(event));
    });

    JsonRpcResponse::success(request.id, serde_json::json!({ "accepted": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_handle_list_returns_all_specs() {
        let dir = tempfile::TempDir::new().unwrap();
        let ledger_path = dir.path().join("ledger.json");
        let ledger = Arc::new(RwLock::new(CapabilityLedger::load_or_create(ledger_path)));
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "runtimes.list".into(),
            params: None,
            id: Some(serde_json::json!(1)),
        };
        let resp = handle_list(req, ledger).await;
        assert!(resp.result.is_some());
        let v = resp.result.unwrap();
        let runtimes = v.get("runtimes").unwrap().as_array().unwrap();
        assert!(runtimes.len() >= 5);
        let names: Vec<String> = runtimes
            .iter()
            .map(|r| r["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"fnm".to_string()));
        assert!(names.contains(&"node".to_string()));
        assert!(names.contains(&"uv".to_string()));
        assert!(names.contains(&"playwright-cli".to_string()));
        assert!(names.contains(&"cargo".to_string()));
    }

    #[tokio::test]
    async fn test_install_failed_event_carries_stderr_field() {
        use crate::sync_primitives::Arc;
        use tokio::sync::RwLock;

        let dir = tempfile::TempDir::new().unwrap();
        let ledger = Arc::new(RwLock::new(CapabilityLedger::load_or_create(
            dir.path().join("ledger.json"),
        )));
        let bus = Arc::new(GatewayEventBus::new());
        let mut rx = bus.subscribe();

        // cargo has `install: &[]` → Unsupported → ensure_capability Err path
        // (on machines where cargo is already installed, probe succeeds and we
        // get a "done" event instead — both paths must carry the `stderr` field).
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "runtimes.install".into(),
            params: Some(serde_json::json!({ "capability": "cargo" })),
            id: Some(serde_json::json!(1)),
        };
        let _ = handle_install(req, ledger, bus.clone()).await;

        // Collect events until we see the terminal event (done or failed).
        // Every event must carry the `stderr` key (null is fine for non-failed).
        let mut saw_terminal = false;
        for _ in 0..20 {
            if let Ok(Ok(json_str)) =
                tokio::time::timeout(std::time::Duration::from_millis(250), rx.recv()).await
            {
                let evt_json: serde_json::Value =
                    serde_json::from_str(&json_str).expect("event must be valid JSON");
                let status = evt_json["status"].as_str().unwrap_or("");
                assert!(
                    evt_json.get("stderr").is_some(),
                    "event must have a `stderr` key (null allowed), got: {evt_json}",
                );
                if status == "failed" {
                    // On the failure path stderr must be a non-null string.
                    assert!(
                        evt_json["stderr"].is_string(),
                        "failed event `stderr` must be a string, got: {evt_json}",
                    );
                    saw_terminal = true;
                    break;
                }
                if status == "done" {
                    saw_terminal = true;
                    break;
                }
            } else {
                break;
            }
        }
        assert!(
            saw_terminal,
            "expected at least one terminal (done or failed) event"
        );
    }
}
