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
//!   `ContextGauge` they already receive. ⚠️ `shared_ui_logic`'s `reconcile`
//!   returns `total: None` and `percent: None` whenever this is `None`, and
//!   this method ALWAYS sends `None` — so a client that renders the response
//!   verbatim gets rows with no total and no percent bar. It must fill this
//!   field from the live gauge first. That requirement is now also stated on
//!   the wire type and on `reconcile` itself, because those are what a client
//!   author reads (判据 §7).
//! * `messages_tokens` — the conversation half of the window. Same rule:
//!   this method measures the PROMPT (system layers + tool schemas), and the
//!   history side has its own, differently-derived estimator
//!   (`harness_bridge::context_estimate`). Reporting one through the other's
//!   door would make two answers out of one question.
//!
//! Both are `Option`s that serialize away when `None`, so a client renders
//! "unknown" rather than a zero (判据 §8).
//!
//! # The layer rows are the assembly BEFORE the budget trim
//!
//! `layers` attributes bytes to the layer that emitted them, which only exists
//! before `prompt_budget::fit_dynamic_suffix_with_content` head/tail-trims the
//! dynamic suffix — after a trim no layer owns the cut bytes. So for a session
//! over the system-prompt budget the rows OVERSTATE what the model received,
//! which is the one case this method exists for. `dynamic_bytes_sent` carries
//! the post-trim size of the dynamic half so the overstatement is visible on
//! the wire rather than only in a doc; the stable prefix is a protected floor
//! the trim never touches.
//!
//! # What is NOT recorded at all
//!
//! Only the main-loop prompt is. A sub-agent's prompt (the `Basic` assembly
//! path, `subagent_spawner`) is measured and traced but never written to the
//! registry, so a sub-agent session key answers `RESOURCE_NOT_FOUND`
//! permanently rather than "not measured yet, ask again". That is honest but
//! it is not temporary — wiring it needs a writer at the spawner.
//!
//! # Visibility
//!
//! An addressed surface: the caller names a session, that session is
//! `KeyChecked` with [`visibility::session_visible`], and a denial reuses
//! [`visibility::not_found_response`] so a foreign key is byte-identical to a
//! missing one. The registry is keyed by the CANONICAL key string
//! (`SessionKey::to_key_string`), so the lookup uses the parsed key rather
//! than the caller's raw text — a caller whose spelling differs from the
//! canonical form (case, whitespace) still resolves to the same session, and
//! must not then be told its measured prompt does not exist.

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

    // Look up under the CANONICAL key — the string the writer used
    // (`session_id.to_key_string()` in `harness_bridge::runner_impl`), not the
    // caller's raw text. `SessionKey::parse` trims, matches `agent`
    // case-insensitively and normalises the agent id, so a non-canonical
    // spelling passes the visibility check above and would then miss its own
    // record, producing a permanent "not measured yet" for a session measured
    // every turn — a fail-closed answer that is indistinguishable from the
    // honest one (判据 §8). The `trace.tool_output` twin already parses once
    // and uses the parsed value (判据 §16).
    let canonical_key = session_key.to_key_string();
    let Some(record) = crate::thinker::prompt_size_registry::global_prompt_size_registry()
        .and_then(|r| r.latest(&canonical_key))
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

    // `layout: None` = this turn built no system prompt at all, so there are no
    // rows and no sent-size to report. Deliberately NOT an empty layout: the
    // absent `dynamic_bytes_sent` says "nothing was measured", where a `0`
    // would say "the whole dynamic half was cut".
    let (layers, dynamic_bytes_sent) = match &record.layout {
        Some(layout) => (layout.layers.as_slice(), Some(layout.dynamic_bytes_sent)),
        None => (&[][..], None),
    };
    let out = ContextBreakdown {
        // Echo the canonical form, not the caller's spelling: the response
        // names the session the numbers came from.
        session_key: canonical_key,
        turn: record.turn,
        layers: layers
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
        dynamic_bytes_sent,
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
    use crate::thinker::prompt_builder::PromptLayout;
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

    /// A layout whose `dynamic_bytes_sent` equals its Dynamic rows — i.e. the
    /// budget did not bite. `trimmed_layout` below is the other case.
    fn untrimmed_layout(layers: Vec<LayerSize>) -> PromptLayout {
        let dynamic_bytes_sent = layers
            .iter()
            .filter(|l| l.stability == LayerStability::Dynamic)
            .map(|l| l.bytes as u64)
            .sum();
        PromptLayout {
            layers,
            dynamic_bytes_sent,
        }
    }

    async fn call(key_str: &str, sessions: Arc<dyn SessionStore>, caller: &str) -> JsonRpcResponse {
        call_with_config(key_str, sessions, caller, None).await
    }

    async fn call_with_config(
        key_str: &str,
        sessions: Arc<dyn SessionStore>,
        caller: &str,
        app_config: Option<Arc<tokio::sync::RwLock<crate::Config>>>,
    ) -> JsonRpcResponse {
        CALLER_USER
            .scope(
                Some(caller.to_string()),
                handle_context_breakdown(
                    req(json!({ "session_key": key_str })),
                    sessions,
                    app_config,
                ),
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

        registry.record_turn(
            &key_str,
            Some(untrimmed_layout(vec![
                layer("soul", 400, LayerStability::Stable),
                layer("runtime_context", 120, LayerStability::Dynamic),
            ])),
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
            out.dynamic_bytes_sent,
            Some(120),
            "the sent size of the dynamic half must cross the wire, so a client \
             can tell an over-budget prompt's rows from what was sent"
        );
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
        registry.record_turn(
            &key_str,
            Some(untrimmed_layout(vec![layer(
                "soul",
                400,
                LayerStability::Stable,
            )])),
            vec![],
        );

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

    /// The registry is written under `SessionKey::to_key_string()`. A caller
    /// whose spelling differs — a typed key, a URL round-trip that upcased —
    /// still resolves to the SAME session through `SessionKey::parse`, and
    /// must therefore reach the same record. Looking up the caller's raw text
    /// made every such request a permanent "no measured prompt yet" for a
    /// session measured every turn: fail-closed, and indistinguishable from
    /// the honest answer.
    #[tokio::test]
    async fn a_non_canonical_spelling_of_the_key_reaches_the_same_record() {
        let registry = install_test_prompt_size_registry();
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("ctxbd-noncanonical");
        let canonical = key.to_key_string();
        seed_session(&sessions, &key, "u-alice").await;
        registry.record_turn(
            &canonical,
            Some(untrimmed_layout(vec![layer(
                "soul",
                400,
                LayerStability::Stable,
            )])),
            vec![],
        );

        // `parse` matches the `agent` prefix case-insensitively, so this is a
        // different STRING naming the same session.
        let shouted = format!(
            "AGENT:{}",
            canonical.strip_prefix("agent:").expect("canonical prefix")
        );
        assert_ne!(shouted, canonical, "self-guard: the spellings must differ");
        assert_eq!(
            SessionKey::from_key_string(&shouted).map(|k| k.to_key_string()),
            Some(canonical.clone()),
            "self-guard: both spellings must name the same session"
        );

        let resp = call(&shouted, sessions, "u-alice").await;
        let out: ContextBreakdown =
            serde_json::from_value(resp.result.expect("success")).expect("ContextBreakdown");
        assert_eq!(out.layers.len(), 1, "the record must be reachable");
        assert_eq!(
            out.session_key, canonical,
            "the response names the session the numbers came from, not the \
             caller's spelling"
        );
    }

    /// The `app_config` parameter exists solely so an operator's per-provider
    /// `context_window` override reaches the resolver. Every other test passes
    /// `None`, which never executes that arm — this one does.
    #[tokio::test]
    async fn a_provider_context_window_override_beats_the_model_catalogue() {
        let registry = install_test_prompt_size_registry();
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("ctxbd-override");
        let key_str = key.to_key_string();
        seed_session(&sessions, &key, "u-alice").await;
        sessions
            .patch_session(
                &key,
                &SessionPatch {
                    model_provider: Some("moonshot".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        registry.record_turn(&key_str, Some(untrimmed_layout(vec![])), vec![]);

        // A window the static catalogue would never produce for this model.
        const OPERATOR_WINDOW: u32 = 12_345;
        assert_ne!(
            crate::providers::model_catalog::resolve_context_window(SEEDED_MODEL),
            OPERATOR_WINDOW,
            "self-guard: the override must differ from the catalogue answer, \
             or this test passes without the override arm running"
        );
        // Built by deserializing what an operator would actually write in
        // `config.toml`, rather than a struct literal: `ProviderConfig` has no
        // `Default`, and this exercises the same path that puts the override
        // there in production.
        let provider_cfg: crate::config::types::provider::ProviderConfig =
            serde_json::from_value(json!({
                "models": ["kimi-k2"],
                "context_window": OPERATOR_WINDOW,
            }))
            .expect("provider config");
        let mut config = crate::Config::default();
        config
            .providers
            .insert("moonshot".to_string(), provider_cfg);
        let app_config = Arc::new(tokio::sync::RwLock::new(config));

        let resp = call_with_config(&key_str, sessions, "u-alice", Some(app_config)).await;
        let out: ContextBreakdown =
            serde_json::from_value(resp.result.expect("success")).expect("ContextBreakdown");
        assert_eq!(
            out.context_window,
            Some(OPERATOR_WINDOW),
            "the operator's per-provider override must reach the resolver"
        );
    }

    /// The case the feature exists for: a prompt over the system-prompt budget.
    /// The rows describe the assembly before the trim (that is where
    /// attribution lives), so the wire must also carry what was actually sent,
    /// or the client renders a total larger than the model ever received.
    #[tokio::test]
    async fn a_trimmed_prompt_reports_the_sent_size_beside_the_overstating_rows() {
        let registry = install_test_prompt_size_registry();
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("ctxbd-trimmed");
        let key_str = key.to_key_string();
        seed_session(&sessions, &key, "u-alice").await;
        registry.record_turn(
            &key_str,
            Some(PromptLayout {
                layers: vec![
                    layer("soul", 400, LayerStability::Stable),
                    layer("memory_window", 5_000, LayerStability::Dynamic),
                ],
                // The budget bit: the dynamic half was cut to a fifth.
                dynamic_bytes_sent: 1_000,
            }),
            vec![],
        );

        let resp = call(&key_str, sessions, "u-alice").await;
        let out: ContextBreakdown =
            serde_json::from_value(resp.result.expect("success")).expect("ContextBreakdown");
        let dynamic_rows: u64 = out
            .layers
            .iter()
            .filter(|l| l.zone == "dynamic")
            .map(|l| l.bytes)
            .sum();
        assert_eq!(dynamic_rows, 5_000, "rows keep the pre-trim attribution");
        assert_eq!(
            out.dynamic_bytes_sent,
            Some(1_000),
            "and the sent size is on the wire beside them, so the difference is \
             visible instead of only being true in a doc comment"
        );
    }

    /// A turn that built no system prompt at all reports no rows and no sent
    /// size — an absent measurement, never a `0` that would read as "the whole
    /// dynamic half was cut".
    #[tokio::test]
    async fn a_turn_with_no_system_prompt_reports_absence_not_zero() {
        let registry = install_test_prompt_size_registry();
        let temp = TempDir::new().unwrap();
        let sessions = session_store(&temp);
        let key = SessionKey::main("ctxbd-noprompt");
        let key_str = key.to_key_string();
        seed_session(&sessions, &key, "u-alice").await;
        registry.record_turn(&key_str, None, vec![("grep".to_string(), 310, 90)]);

        let resp = call(&key_str, sessions, "u-alice").await;
        let value = resp.result.expect("success");
        let out: ContextBreakdown =
            serde_json::from_value(value.clone()).expect("ContextBreakdown");
        assert_eq!(out.turn, 1, "a measured turn, just one without a prompt");
        assert!(out.layers.is_empty());
        assert_eq!(out.dynamic_bytes_sent, None);
        assert!(
            value.get("dynamic_bytes_sent").is_none(),
            "absence is an absent key, not a zero"
        );
        assert_eq!(out.tools.len(), 1, "the tools of THIS turn are still real");
    }
}
