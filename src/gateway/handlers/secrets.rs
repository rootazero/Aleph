//! Vault secret CRUD over JSON-RPC.
//!
//! Mirrors the HTTP `/v1/admin/secrets` surface so thin clients can
//! manage vault entries without going through the policy/HTTP layer.
//! All operations route through [`SharedTokenManager`] just like the
//! HTTP path — the auth token still acts as the AES-GCM master key.
//!
//! RPCs:
//! - `secrets.list` → `{ secrets: [String] }` (names only; never values)
//! - `secrets.set` (params: `{ key, value }`) → `{ key }`
//! - `secrets.delete` (params: `{ key }`) → `{ deleted: true }` or `-32004`
//! - `secrets.verify` (params: `{ key }`) → `{ key, bytes, present }`
//! - `secrets.providers` → `{ providers: [{ key, type, account? }] }`
//!
//! Values are intentionally never returned over the wire — `verify`
//! reports presence + byte length only. Use the local `aleph-server
//! secret` CLI (`LockOnly` path) when the raw value is needed.

use super::auth::AuthContext;
use crate::config::Config;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::secrets::validate_secret_name;
use crate::sync_primitives::Arc;
use serde::Deserialize;
use serde_json::json;
use tracing::warn;

const INVALID_PARAMS: i32 = -32602;
const INTERNAL_ERROR: i32 = -32603;
const NOT_FOUND: i32 = -32004;

pub async fn handle_secrets_list(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    match ctx.shared_token_mgr.list_secret_names() {
        Ok(mut names) => {
            names.sort();
            JsonRpcResponse::success(request.id, json!({ "secrets": names }))
        }
        Err(e) => {
            warn!(error = %e, "secrets.list failed");
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("list failed: {e}"))
        }
    }
}

#[derive(Debug, Deserialize)]
struct SetParams {
    key: String,
    value: String,
}

pub async fn handle_secrets_set(request: JsonRpcRequest, ctx: Arc<AuthContext>) -> JsonRpcResponse {
    let params: SetParams = match request
        .params
        .as_ref()
        .map(|p| serde_json::from_value::<SetParams>(p.clone()))
    {
        Some(Ok(p)) => p,
        Some(Err(e)) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("invalid params: {e}"),
            );
        }
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "missing params: key, value",
            );
        }
    };

    let key = match validate_secret_name(&params.key) {
        Ok(k) => k,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e),
    };
    if params.value.is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "value cannot be empty");
    }

    match ctx.shared_token_mgr.store_secret(&key, &params.value) {
        Ok(()) => {
            // Vault writes are the highest-privilege mutation on the admin
            // surface; they used to leave no forensic row. The audit entry
            // records the key NAME only — never the value.
            if let Some(log) = crate::security::audit::global() {
                log.log(crate::security::audit::AuditEntry::authority_change(
                    crate::gateway::caller_identity::current_caller_user(),
                    format!("secrets.set: stored key '{key}'"),
                )).await;
            }
            JsonRpcResponse::success(request.id, json!({ "key": key }))
        }
        Err(e) => {
            warn!(error = %e, key = %key, "secrets.set failed");
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("store failed: {e}"))
        }
    }
}

#[derive(Debug, Deserialize)]
struct KeyOnlyParams {
    key: String,
}

pub async fn handle_secrets_delete(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let params: KeyOnlyParams = match request
        .params
        .as_ref()
        .map(|p| serde_json::from_value::<KeyOnlyParams>(p.clone()))
    {
        Some(Ok(p)) => p,
        Some(Err(e)) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("invalid params: {e}"),
            );
        }
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "missing params: key"),
    };

    let key = match validate_secret_name(&params.key) {
        Ok(k) => k,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e),
    };

    match ctx.shared_token_mgr.delete_secret(&key) {
        Ok(true) => {
            if let Some(log) = crate::security::audit::global() {
                log.log(crate::security::audit::AuditEntry::authority_change(
                    crate::gateway::caller_identity::current_caller_user(),
                    format!("secrets.delete: deleted key '{key}'"),
                )).await;
            }
            JsonRpcResponse::success(request.id, json!({ "deleted": true, "key": key }))
        }
        Ok(false) => JsonRpcResponse::error(request.id, NOT_FOUND, format!("no secret: {key}")),
        Err(e) => {
            warn!(error = %e, key = %key, "secrets.delete failed");
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("delete failed: {e}"))
        }
    }
}

pub async fn handle_secrets_verify(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let params: KeyOnlyParams = match request
        .params
        .as_ref()
        .map(|p| serde_json::from_value::<KeyOnlyParams>(p.clone()))
    {
        Some(Ok(p)) => p,
        Some(Err(e)) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("invalid params: {e}"),
            );
        }
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "missing params: key"),
    };

    let key = match validate_secret_name(&params.key) {
        Ok(k) => k,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e),
    };

    match ctx.shared_token_mgr.get_secret(&key) {
        Ok(Some(secret)) => JsonRpcResponse::success(
            request.id,
            json!({
                "key": key,
                "bytes": secret.expose().len(),
                "present": true,
            }),
        ),
        Ok(None) => JsonRpcResponse::error(request.id, NOT_FOUND, format!("no secret: {key}")),
        Err(e) => {
            warn!(error = %e, key = %key, "secrets.verify failed");
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("verify failed: {e}"))
        }
    }
}

/// Surface the configured provider catalogue (config-only; does NOT
/// perform health checks against external providers). The thin client
/// can correlate this with `secrets.list` for an at-a-glance view of
/// where each secret can be sourced from.
pub async fn handle_secrets_providers(
    request: JsonRpcRequest,
    _ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    let config = Config::load().unwrap_or_default();
    let mut providers: Vec<serde_json::Value> = vec![
        // The built-in local vault is implicit — always listed first so
        // the response is never empty even on a fresh install.
        json!({ "key": "local", "type": "local_vault", "builtin": true }),
    ];
    for (key, p) in &config.secret_providers {
        if key == "local" {
            // Skip user override of the built-in slot to avoid duplicates.
            continue;
        }
        let mut item = json!({
            "key": key,
            "type": p.provider_type,
            "builtin": false,
        });
        if let Some(obj) = item.as_object_mut() {
            if let Some(account) = &p.account {
                obj.insert("account".into(), json!(account));
            }
            if let Some(env) = &p.service_account_token_env {
                obj.insert("service_account_token_env".into(), json!(env));
            }
        }
        providers.push(item);
    }
    JsonRpcResponse::success(request.id, json!({ "providers": providers }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_empty_and_invalid() {
        assert!(validate_secret_name("").is_err());
        assert!(validate_secret_name("   ").is_err());
        assert!(validate_secret_name("bad name").is_err());
        assert!(validate_secret_name("bad/name").is_err());
    }

    #[test]
    fn validate_accepts_allowed_chars() {
        assert_eq!(
            validate_secret_name("openai.api-key_1").unwrap(),
            "openai.api-key_1"
        );
        assert_eq!(
            validate_secret_name("  trimmed:value  ").unwrap(),
            "trimmed:value"
        );
    }

    #[test]
    fn validate_rejects_overlength() {
        let oversized = "a".repeat(129);
        assert!(validate_secret_name(&oversized).is_err());
    }
}
