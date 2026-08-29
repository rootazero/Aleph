//! Runtime RPC handlers: list + install + refresh.

use crate::sync_primitives::{Arc, AsyncRwLock as RwLock};

use serde::Serialize;

use crate::gateway::event_bus::{
    GatewayEventBus, RuntimeInstallProgressEvent, TopicEvent, RUNTIME_INSTALL_PROGRESS_TOPIC,
};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::runtimes::ledger::{CapabilityLedger, CapabilityStatus};
use crate::runtimes::{ensure_capability, find_spec, supported_on_current_os, SPECS};

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
            let status = entry.map_or(CapabilityStatus::Missing, |e| e.status);
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
                .map_or(0, |d| d.as_secs());
            guard.update(crate::runtimes::ledger::CapabilityEntry {
                name: spec.name.to_string(),
                bin_path: probe_result.bin_path.unwrap_or_default(),
                version: probe_result.version.unwrap_or_default(),
                status: CapabilityStatus::Ready,
                source: probe_result.source,
                last_probed: now,
            });
        } else {
            // mark_missing (not update_status) so a stale path/version left from
            // a previous probe / a ledger copied off another machine is cleared.
            guard.mark_missing(spec.name);
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
        publish_progress(
            &bus,
            RuntimeInstallProgressEvent {
                step: cap_for_event.clone(),
                status: "started".into(),
                error: None,
                stderr: None,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        );
        let result = ensure_capability(&cap, &ledger).await;
        let event = match result {
            Ok(_) => RuntimeInstallProgressEvent {
                step: cap_for_event,
                status: "done".into(),
                error: None,
                stderr: None,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
            Err(e) => {
                let err_str = e.to_string();
                // `ensure.rs` emits "Stderr tail: <tail>" as part of the error
                // string, followed by the canonical "Fix options:" block.
                // Extract just the tail (stop before the fix-options suffix);
                // otherwise the Panel's stderr pane would show the boilerplate
                // help text instead of the actual diagnostic.
                let stderr = err_str
                    .split_once("Stderr tail: ")
                    .map(|(_, tail)| {
                        tail.split_once("Fix options:")
                            .map(|(t, _)| t.trim().to_string())
                            .unwrap_or_else(|| tail.to_string())
                    })
                    .or_else(|| Some(err_str.clone()));
                RuntimeInstallProgressEvent {
                    step: cap_for_event,
                    status: "failed".into(),
                    error: Some(err_str),
                    stderr,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                }
            }
        };
        publish_progress(&bus, event);
    });

    JsonRpcResponse::success(request.id, serde_json::json!({ "accepted": true }))
}

/// Publish one [`RuntimeInstallProgressEvent`] wrapped in a [`TopicEvent`] on
/// [`RUNTIME_INSTALL_PROGRESS_TOPIC`] so the Panel receives it through the
/// standard `events.subscribe` pipeline (the raw `GatewayEvent` envelope is not
/// dispatched by the Panel's event parser).
fn publish_progress(bus: &Arc<GatewayEventBus>, event: RuntimeInstallProgressEvent) {
    let data = serde_json::to_value(&event).unwrap_or_default();
    let _ = bus.publish_json(&TopicEvent::new(RUNTIME_INSTALL_PROGRESS_TOPIC, data));
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

        // cargo installs via Shell (Unix) / PowerShell (Windows) per
        // `src/runtimes/specs.rs`. We use it as the test target because it is
        // almost always already installed (probe → done) — both the success
        // and failure paths must carry the `stderr` field (null when unused).
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
                // Progress events are published as a `TopicEvent` envelope; the
                // payload fields live under `data`.
                assert_eq!(
                    evt_json["topic"].as_str(),
                    Some(RUNTIME_INSTALL_PROGRESS_TOPIC),
                    "event must be published on the install-progress topic, got: {evt_json}",
                );
                let data = &evt_json["data"];
                let status = data["status"].as_str().unwrap_or("");
                assert!(
                    data.get("stderr").is_some(),
                    "event must have a `stderr` key (null allowed), got: {evt_json}",
                );
                if status == "failed" {
                    // On the failure path stderr must be a non-null string.
                    assert!(
                        data["stderr"].is_string(),
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
