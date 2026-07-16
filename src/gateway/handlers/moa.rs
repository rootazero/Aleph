//! MoA preset configuration RPC handlers. Thin I/O over MoaPresetStore — the
//! Panel's visual config talks to these; the `moa` tool shares the same core.

use crate::config::patcher::ConfigPatcher;
use crate::config::{default_advisor_timeout_secs, Config, MoaFanout, MoaSlot};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::providers::moa::{MoaPresetStore, MoaStoreError};
use crate::sync_primitives::Arc;
use serde::Deserialize;
use tokio::sync::RwLock;

#[derive(Debug, Deserialize)]
struct SavePresetParams {
    name: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    advisors: Vec<MoaSlot>,
    aggregator: MoaSlot,
    #[serde(default)]
    fanout: MoaFanout,
    #[serde(default = "default_advisor_timeout_secs")]
    advisor_timeout_secs: u64,
    #[serde(default)]
    advisor_max_tokens: Option<u32>,
    #[serde(default)]
    advisor_temperature: Option<f32>,
    #[serde(default)]
    aggregator_temperature: Option<f32>,
    #[serde(default)]
    make_default: bool,
}

const fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct NameParam {
    name: String,
}

#[derive(Debug, Deserialize)]
struct SaveTracesParam {
    on: bool,
}

// `JsonRpcResponse` is the wire response value type this module returns by
// value everywhere; the `Err` here is really a ready-to-send response, not an
// error to bubble. Boxing it to shave stack bytes would add a heap alloc on
// every invalid-params path plus a deref at all four call sites — not worth it.
#[allow(clippy::result_large_err)]
fn parse<T: for<'de> Deserialize<'de>>(req: &JsonRpcRequest) -> Result<T, JsonRpcResponse> {
    let params = req
        .params
        .clone()
        .ok_or_else(|| JsonRpcResponse::error(req.id.clone(), INVALID_PARAMS, "Missing params"))?;
    serde_json::from_value(params).map_err(|e| {
        JsonRpcResponse::error(
            req.id.clone(),
            INVALID_PARAMS,
            format!("Invalid params: {e}"),
        )
    })
}

/// Map a store error to the right JSON-RPC error code.
fn store_err_response(id: Option<serde_json::Value>, e: MoaStoreError) -> JsonRpcResponse {
    let code = match e {
        MoaStoreError::Validation(_) | MoaStoreError::Absent(_) | MoaStoreError::OnlyPreset(_) => {
            INVALID_PARAMS
        }
        MoaStoreError::Patch(_) => INTERNAL_ERROR,
    };
    JsonRpcResponse::error(id, code, e.to_string())
}

pub async fn handle_list_presets(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
) -> JsonRpcResponse {
    // Fresh install has `Config.moa == None`; default to an empty `MoaToml` so
    // the wire shape is a consistent `{}` (matching `MoaPresetStore::list`)
    // rather than JSON `null`, which every consumer would otherwise special-case.
    let moa = config.read().await.moa.clone().unwrap_or_default();
    match serde_json::to_value(&moa) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    }
}

pub async fn handle_save_preset(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    config_patcher: Arc<ConfigPatcher>,
) -> JsonRpcResponse {
    let p: SavePresetParams = match parse(&request) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let preset = crate::config::MoaPreset {
        enabled: p.enabled,
        advisors: p.advisors,
        aggregator: p.aggregator,
        fanout: p.fanout,
        advisor_timeout_secs: p.advisor_timeout_secs,
        advisor_max_tokens: p.advisor_max_tokens,
        advisor_temperature: p.advisor_temperature,
        aggregator_temperature: p.aggregator_temperature,
    };
    let store = MoaPresetStore::new(config, config_patcher);
    match store.save_preset(&p.name, preset, p.make_default).await {
        Ok(result) => JsonRpcResponse::success(
            request.id,
            serde_json::to_value(&result).unwrap_or_default(),
        ),
        Err(e) => store_err_response(request.id, e),
    }
}

pub async fn handle_delete_preset(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    config_patcher: Arc<ConfigPatcher>,
) -> JsonRpcResponse {
    let p: NameParam = match parse(&request) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let store = MoaPresetStore::new(config, config_patcher);
    match store.delete_preset(&p.name).await {
        Ok(result) => JsonRpcResponse::success(
            request.id,
            serde_json::to_value(&result).unwrap_or_default(),
        ),
        Err(e) => store_err_response(request.id, e),
    }
}

pub async fn handle_set_default(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    config_patcher: Arc<ConfigPatcher>,
) -> JsonRpcResponse {
    let p: NameParam = match parse(&request) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let store = MoaPresetStore::new(config, config_patcher);
    match store.set_default(&p.name).await {
        Ok(result) => JsonRpcResponse::success(
            request.id,
            serde_json::to_value(&result).unwrap_or_default(),
        ),
        Err(e) => store_err_response(request.id, e),
    }
}

pub async fn handle_set_save_traces(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    config_patcher: Arc<ConfigPatcher>,
) -> JsonRpcResponse {
    let p: SaveTracesParam = match parse(&request) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let store = MoaPresetStore::new(config, config_patcher);
    match store.set_save_traces(p.on).await {
        Ok(result) => JsonRpcResponse::success(
            request.id,
            serde_json::to_value(&result).unwrap_or_default(),
        ),
        Err(e) => store_err_response(request.id, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::backup::ConfigBackup;
    use crate::providers::moa::config_handle::moa_config_test_lock;

    // Build a handler-ready (config, patcher) over a temp config.toml.
    // Mirrors the temp-store helper shape used by preset_store.rs tests.
    async fn ctx() -> (Arc<RwLock<Config>>, Arc<ConfigPatcher>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        tokio::fs::write(&path, "").await.unwrap();
        let config = Arc::new(RwLock::new(Config::default()));
        let backup = ConfigBackup::new(dir.path().join("backups"), 10);
        let patcher = Arc::new(ConfigPatcher::new(Arc::clone(&config), path, backup));
        (config, patcher, dir)
    }

    #[tokio::test]
    async fn save_preset_persists_and_list_returns_it() {
        let _g = moa_config_test_lock();
        let (config, patcher, _dir) = ctx().await;
        let params = serde_json::json!({
            "name": "default",
            "enabled": true,
            "advisors": [{"provider": "openai", "model": "gpt-5.5"}],
            "aggregator": {"provider": "anthropic", "model": "claude-opus-4-8"},
            "make_default": true
        });
        let req = JsonRpcRequest::with_id("moa.savePreset", Some(params), serde_json::json!(1));
        let resp = handle_save_preset(req, Arc::clone(&config), Arc::clone(&patcher)).await;
        assert!(
            resp.error.is_none(),
            "save should succeed: {:?}",
            resp.error
        );

        let list_req = JsonRpcRequest::with_id("moa.listPresets", None, serde_json::json!(2));
        let list_resp = handle_list_presets(list_req, Arc::clone(&config)).await;
        let v = list_resp.result.unwrap();
        assert!(v["presets"]["default"].is_object());
        assert_eq!(v["default_preset"], "default");
    }

    #[tokio::test]
    async fn save_preset_rejects_duplicate_slot() {
        let _g = moa_config_test_lock();
        let (config, patcher, _dir) = ctx().await;
        let params = serde_json::json!({
            "name": "p",
            "enabled": true,
            "advisors": [{"provider": "openai", "model": "gpt-5.5"}],
            "aggregator": {"provider": "openai", "model": "gpt-5.5"}
        });
        let req = JsonRpcRequest::with_id("moa.savePreset", Some(params), serde_json::json!(1));
        let resp = handle_save_preset(req, config, patcher).await;
        let err = resp.error.expect("must reject duplicate slot");
        assert_eq!(err.code, INVALID_PARAMS);
    }
}
