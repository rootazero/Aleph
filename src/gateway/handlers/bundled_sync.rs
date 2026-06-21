//! `bundled.sync` — explicitly refresh official skills/plugins from the
//! external repos (clone latest `main` → re-extract). Reserved for explicit
//! triggers (CLI / LLM tool / Hub button); the startup path never auto-pulls.

use crate::bundled::SyncKind;
use crate::gateway::handlers::parse_params;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
pub struct SyncParams {
    #[serde(default = "default_kind")]
    pub kind: String,
}

fn default_kind() -> String {
    "all".to_string()
}

pub(crate) fn parse_kind(s: &str) -> Option<SyncKind> {
    match s {
        "skills" => Some(SyncKind::Skills),
        "plugins" => Some(SyncKind::Plugins),
        "all" => Some(SyncKind::All),
        _ => None,
    }
}

pub async fn handle_sync(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: SyncParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(kind) = parse_kind(&params.kind) else {
        return JsonRpcResponse::error(request.id, INTERNAL_ERROR, "invalid kind".to_string());
    };
    let aleph_home = match crate::utils::paths::get_config_dir() {
        Ok(p) => p,
        Err(e) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e.to_string()),
    };
    match tokio::task::spawn_blocking(move || crate::bundled::sync_official_now(&aleph_home, kind))
        .await
    {
        Ok(Ok(r)) => JsonRpcResponse::success(
            request.id,
            json!({ "ok": true, "skills": r.skills, "plugins": r.plugins }),
        ),
        Ok(Err(e)) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
        Err(e) => {
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("sync task failed: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_kind_maps_known_values() {
        assert!(matches!(parse_kind("skills"), Some(SyncKind::Skills)));
        assert!(matches!(parse_kind("plugins"), Some(SyncKind::Plugins)));
        assert!(matches!(parse_kind("all"), Some(SyncKind::All)));
        assert!(parse_kind("bogus").is_none());
    }
}
