//! `context.breakdown` — the measured layout of the LAST prompt this session
//! sent.
//!
//! # What it is and is not
//!
//! Every figure here was recorded at the moment the bytes were produced (see
//! `thinker::prompt_size_registry`): the layer sizes come from the traversal
//! that assembled the system prompt, the tool sizes from the very
//! `ToolService` the harness handed the model. Nothing is re-derived at read
//! time, because a re-derivation describes a prompt that was never sent.
//!
//! Two fields are deliberately left absent rather than filled:
//!
//! * `provider_reported` — the provider's own token count for the same turn.
//!   We do not have it here, and a locally computed total dressed as the
//!   provider's would be a confident lie. Clients reconcile against the live
//!   `ContextGauge` they already receive.
//! * `messages_tokens` — the conversation half of the window. Same rule:
//!   this method measures the PROMPT (system layers + tool schemas), and the
//!   history side has its own, differently-derived estimator
//!   (`harness_bridge::context_estimate`). Reporting one through the other's
//!   door would make two answers out of one question.
//!
//! Both are `Option`s that serialize away when `None`, so a client renders
//! "unknown" rather than a zero (判据 §8).
//!
//! # Visibility
//!
//! An addressed surface: the caller names a session, that session is
//! `KeyChecked` with [`visibility::session_visible`], and a denial reuses
//! [`visibility::not_found_response`] so a foreign key is byte-identical to a
//! missing one. Records are keyed by the same session key string the registry
//! is written under, so a foreign key never reaches a record it could read.

use aleph_protocol::{ContextBreakdown, LayerSizeView, ToolSchemaSize};

use crate::gateway::protocol::{
    JsonRpcRequest, JsonRpcResponse, INVALID_PARAMS, RESOURCE_NOT_FOUND,
};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility;
use crate::sync_primitives::Arc;
use crate::thinker::prompt_layer::LayerStability;

/// `app_config` is optional because the boot path that registers this handler
/// does not always hold one. Its ONLY effect is the operator's per-provider
/// `context_window` override; without it the static model catalogue answers,
/// which is the same value every other consumer of an unoverridden model gets.
pub async fn handle_context_breakdown(
    request: JsonRpcRequest,
    sessions: Arc<dyn SessionStore>,
    app_config: Option<Arc<tokio::sync::RwLock<crate::Config>>>,
) -> JsonRpcResponse {
    let key_str = match request
        .params
        .as_ref()
        .and_then(|p| p.get("session_key"))
        .and_then(serde_json::Value::as_str)
    {
        Some(k) if !k.is_empty() => k.to_string(),
        // A malformed/absent key is a validation error, not an existence
        // question — the same split `trace.tool_output` makes.
        _ => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    };
    let Some(session_key) = SessionKey::from_key_string(&key_str) else {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Invalid session_key format");
    };
    let meta = match sessions.get_metadata(&session_key).await {
        Ok(Some(m)) if visibility::session_visible(&m) => m,
        // Foreign owner, missing row, and store error all produce the same
        // response: no oracle.
        _ => return visibility::not_found_response(request.id),
    };

    let Some(record) = crate::thinker::prompt_size_registry::global_prompt_size_registry()
        .and_then(|r| r.latest(&key_str))
    else {
        // Unknown ≠ empty: no turn has been measured for this session (or the
        // daemon restarted since the last one). The client renders "not
        // measured yet", never a breakdown of zeros.
        return JsonRpcResponse::error(
            request.id,
            RESOURCE_NOT_FOUND,
            "no measured prompt for this session yet",
        );
    };

    let context_window = match meta.model.as_deref() {
        Some(model) => {
            let override_w = match (&app_config, meta.model_provider.as_deref()) {
                (Some(cfg), Some(provider)) => cfg
                    .read()
                    .await
                    .providers
                    .get(provider)
                    .and_then(|p| p.context_window),
                _ => None,
            };
            Some(
                crate::providers::model_catalog::resolve_context_window_with_override(
                    override_w, model,
                ),
            )
        }
        // No model on the row means nothing has run here yet under a known
        // model; the window is unknown, not a default.
        None => None,
    };

    let out = ContextBreakdown {
        session_key: key_str,
        turn: record.turn,
        layers: record
            .layers
            .iter()
            .map(|l| LayerSizeView {
                name: l.name.to_string(),
                bytes: l.bytes as u64,
                tokens: l.tokens as u64,
                zone: match l.stability {
                    LayerStability::Stable => "stable",
                    LayerStability::Dynamic => "dynamic",
                }
                .to_string(),
            })
            .collect(),
        tools: record
            .tools
            .iter()
            .map(|(name, schema_bytes, description_bytes)| ToolSchemaSize {
                name: name.clone(),
                schema_bytes: *schema_bytes,
                description_bytes: *description_bytes,
            })
            .collect(),
        messages_tokens: None,
        provider_reported: None,
        context_window,
    };
    JsonRpcResponse::success(
        request.id,
        serde_json::to_value(out).unwrap_or(serde_json::Value::Null),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::caller_identity::CALLER_USER;
    use crate::gateway::session_store::file_backend::{FileSessionStore, FileSessionStoreConfig};
    use crate::gateway::session_store::types::SessionPatch;
    use crate::thinker::prompt_pipeline::LayerSize;
    use crate::thinker::prompt_size_registry::install_test_prompt_size_registry;
    use serde_json::{json, Value};
    use tempfile::TempDir;

    const SEEDED_MODEL: &str = "claude-sonnet-4-5-20250929";

    fn req(params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "context.breakdown".into(),
            params: Some(params),
            id: Some(json!(1)),
        }
    }

    fn session_store(temp: &TempDir) -> Arc<dyn SessionStore> {
        Arc::new(
            FileSessionStore::new(FileSessionStoreConfig {
                base_dir: temp.path().to_path_buf(),
                ..Default::default()
            })
            .unwrap(),
        )
    }

    /// Create `key` owned by `owner`, with a model stamped on the row so the
    /// window resolver has something to resolve.
    async fn seed_session(sessions: &Arc<dyn SessionStore>, key: &SessionKey, owner: &str) {
        crate::scope::with_scope(
            Some(crate::scope::ScopeAttribution::personal(owner)),
            sessions.get_or_create(key),
        )
        .await
        .unwrap();
        sessions
            .patch_session(
                key,
                &SessionPatch {
                    model: Some(SEEDED_MODEL.to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }

    fn layer(name: &'static str, bytes: usize, stability: LayerStability) -> LayerSize {
        LayerSize {
            priority: 100,
            name,
            stability,
            chars: bytes,
            bytes,
            tokens: bytes / 4,
        }
    }

    async fn call(key_str: &str, sessions: Arc<dyn SessionStore>, caller: &str) -> JsonRpcResponse {
        CALLER_USER
            .scope(
                Some(caller.to_string()),
                handle_context_breakdown(req(json!({ "session_key": key_str })), sessions, None),
            )
            .await
    }

    #[tokio::test]
    async fn a_session_with_no_measured_prompt_is_not_found_not_zeros() {
        install_test_prompt_size_registry();
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("ctxbd-unmeasured");
        seed_session(&sessions, &key, "u-alice").await;

        let resp = call(&key.to_key_string(), sessions, "u-alice").await;
        let err = resp.error.expect("must refuse");
        assert_eq!(err.code, RESOURCE_NOT_FOUND);
        assert!(
            resp.result.is_none(),
            "an unmeasured session must not be served a breakdown of zeros"
        );
    }

    #[tokio::test]
    async fn a_measured_prompt_is_reported_verbatim_with_the_resolved_window() {
        let registry = install_test_prompt_size_registry();
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("ctxbd-measured");
        let key_str = key.to_key_string();
        seed_session(&sessions, &key, "u-alice").await;

        registry.record_layers(
            &key_str,
            vec![
                layer("soul", 400, LayerStability::Stable),
                layer("runtime_context", 120, LayerStability::Dynamic),
            ],
        );
        registry.record_tools(
            &key_str,
            vec![
                ("grep".to_string(), 310, 90),
                ("file_read".to_string(), 280, 75),
            ],
        );

        let resp = call(&key_str, sessions, "u-alice").await;
        let value = resp.result.expect("success");
        let out: ContextBreakdown =
            serde_json::from_value(value.clone()).expect("the response IS the contract type");

        assert_eq!(out.session_key, key_str);
        assert_eq!(out.turn, 1);
        assert_eq!(
            out.layers
                .iter()
                .map(|l| (l.name.as_str(), l.bytes, l.zone.as_str()))
                .collect::<Vec<_>>(),
            vec![("soul", 400, "stable"), ("runtime_context", 120, "dynamic")],
            "layer names, bytes and zones must survive the wire unchanged"
        );
        assert_eq!(out.tools.len(), 2);
        assert_eq!(out.tool_bytes(), 310 + 90 + 280 + 75);
        assert_eq!(
            out.provider_reported, None,
            "we do not have the provider's count, and must not invent one"
        );
        assert_eq!(out.messages_tokens, None);
        assert_eq!(
            out.context_window,
            Some(crate::providers::model_catalog::resolve_context_window(
                SEEDED_MODEL
            )),
            "the window must come from the same resolver every other consumer uses"
        );

        // The envelope over-sends nothing — the half `from_value` cannot see.
        // Expected keys derived from the type, never listed by hand.
        let keys = |v: &Value| {
            v.as_object()
                .expect("object")
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
        };
        let expected = serde_json::to_value(&out).expect("re-serialize");
        assert_eq!(
            keys(&value),
            keys(&expected),
            "envelope keys must equal ContextBreakdown's exactly"
        );
    }

    #[tokio::test]
    async fn another_users_session_is_not_found_even_when_measured() {
        let registry = install_test_prompt_size_registry();
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("ctxbd-foreign");
        let key_str = key.to_key_string();
        seed_session(&sessions, &key, "u-alice").await;
        registry.record_layers(&key_str, vec![layer("soul", 400, LayerStability::Stable)]);

        let resp = call(&key_str, sessions, "u-mallory").await;
        let err = resp.error.expect("must refuse");
        assert_eq!(
            err.code, RESOURCE_NOT_FOUND,
            "a foreign key must be indistinguishable from a missing one"
        );
        assert_eq!(err.message, "session not found");
    }

    #[tokio::test]
    async fn a_missing_or_malformed_key_is_a_validation_error() {
        install_test_prompt_size_registry();
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);

        let resp = CALLER_USER
            .scope(
                Some("u-alice".to_string()),
                handle_context_breakdown(req(json!({})), sessions.clone(), None),
            )
            .await;
        assert_eq!(resp.error.expect("must refuse").code, INVALID_PARAMS);

        let resp = call("", sessions, "u-alice").await;
        assert_eq!(resp.error.expect("must refuse").code, INVALID_PARAMS);
    }
}
