//! Execution engine configuration RPC handlers
//!
//! Provides RPC methods for managing agent execution settings (timeout, iterations).

use crate::config::types::ExecutionConfig;
use crate::config::Config;
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::sync_primitives::Arc;
use serde_json::Value;
use tokio::sync::RwLock;

/// Get execution configuration
pub async fn handle_get(request: JsonRpcRequest, config: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let cfg = config.read().await;
    match serde_json::to_value(&cfg.execution) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize config: {e}"),
        ),
    }
}

/// Update execution configuration.
///
/// Two things this handler is easy to get wrong, and did:
///
/// 1. **The body is merged, not assigned.** All twelve `ExecutionConfig` fields
///    carry `#[serde(default…)]` and the Panel's DTO carries two of them
///    (`default_timeout_secs`, `max_iterations`), so assigning the deserialized
///    body over the section rebuilt the other ten from their defaults and
///    `save_incremental` wrote that to disk: raising the run timeout silently
///    reset `max_runs_global`, `max_concurrent_subagents` and
///    `mid_turn_steering`. See [`super::general_config::json_merge`]. Widening
///    the Panel DTO is NOT the fix — that re-arms the trap for field #13.
/// 2. **`execution` is a live section, so persisting it is only half the
///    write.** `reload_impact::LIVE_SECTIONS` declares it live and
///    `live_apply::apply_live_sections` has a real arm for it; this handler is
///    outside `ConfigPatcher`, the chokepoint that used to be the only caller,
///    so a cap raised here landed on disk and the running `ConcurrencyLimiter`
///    kept admitting at the old value for the rest of the process — under a
///    `{"success": true}`. The verdict is reported *verified*
///    ([`crate::config::classify_verified`]), so a process where the handles
///    were never installed downgrades to `restart` instead of claiming a poke
///    that did not happen.
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let incoming = match request.params {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let (applied_value, impact) = {
        let mut cfg = config.write().await;

        let mut base = match serde_json::to_value(&cfg.execution) {
            Ok(v) => v,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to serialize existing config: {e}"),
                );
            }
        };
        super::general_config::json_merge(&mut base, &incoming);

        let update: ExecutionConfig = match serde_json::from_value(base) {
            Ok(u) => u,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {e}"),
                );
            }
        };

        // Validate ranges on the MERGED value, not on the raw body: a narrow
        // body would otherwise smuggle an out-of-range default past every check
        // below simply by omitting the key.
        if update.default_timeout_secs < 60 {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "default_timeout_secs must be at least 60 (1 minute)",
            );
        }
        if update.default_timeout_secs > 604_800 {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "default_timeout_secs must be at most 604800 (7 days)",
            );
        }
        if update.max_iterations < 5 {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "max_iterations must be at least 5",
            );
        }
        if update.max_iterations > 10_000 {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "max_iterations must be at most 10000",
            );
        }

        cfg.execution = update;

        if let Err(e) = cfg.save_incremental(&["execution"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }

        // Push the new caps onto the running runtime. Executing the declaration
        // table rather than hand-inlining `reconfigure_global` here is the rule
        // `reload_impact::LIVE_SECTIONS`'s own doc states: one table, one
        // executor, however many write surfaces.
        let landed = crate::config::live_apply::apply_live_sections(&cfg, &["execution"]);
        let impact = crate::config::classify_verified("execution", &landed);

        (
            serde_json::to_value(&cfg.execution).unwrap_or(Value::Null),
            impact,
        )
    };

    // Broadcast change event — carrying what landed, not what was asked for.
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("execution".to_string()),
        value: applied_value,
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_gateway_event(&event);

    JsonRpcResponse::success(
        request.id,
        serde_json::json!({ "success": true, "reload_impact": impact }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::paths::AlephHomeEnvGuard;
    use serde_json::json;

    fn make_event_bus() -> Arc<GatewayEventBus> {
        Arc::new(GatewayEventBus::new())
    }

    /// A narrow body must leave every key it did not mention byte-identical.
    ///
    /// Derived from `ExecutionConfig`'s own serialization: snapshot before,
    /// snapshot after, require the diff to be exactly the keys the request
    /// carried — so a thirteenth field is covered without editing this test.
    // Serialised with the rest of the `subagent_concurrency_cap` group, and NOT
    // because this test reads that global: it does not. It gets here through
    // `handle_update` -> `apply_live_sections(.., ["execution"])`, whose
    // `execution` arm WRITES `set_max_concurrent_subagents`. A test that drives
    // a handler inherits every global that handler touches, which is why the
    // group's membership cannot be read off the test bodies.
    #[tokio::test]
    #[serial_test::serial(subagent_concurrency_cap)]
    async fn a_narrow_body_leaves_every_unmentioned_key_byte_identical() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());

        let mut cfg = Config::default();
        cfg.execution.max_runs_global = 2;
        cfg.execution.max_concurrent_subagents = 1;
        cfg.execution.mid_turn_steering = false;
        cfg.execution.busy_queue_max_per_session = 3;
        let before = serde_json::to_value(&cfg.execution).expect("serialize");
        let config = Arc::new(RwLock::new(cfg));

        // Exactly what the Panel sends when the operator raises the timeout.
        let body = json!({ "default_timeout_secs": 3600, "max_iterations": 40 });
        let request = JsonRpcRequest::with_id("execution_config.update", Some(body), json!(1));
        let response = handle_update(request, Arc::clone(&config), make_event_bus()).await;
        assert!(
            response.is_success(),
            "execution_config.update failed: {response:?}"
        );

        let after = serde_json::to_value(&config.read().await.execution).expect("serialize");

        let mut expected = before.clone();
        expected["default_timeout_secs"] = json!(3600);
        expected["max_iterations"] = json!(40);
        assert_eq!(
            after, expected,
            "execution_config.update reset a concurrency/steering key the caller \
             never mentioned — an omitted key must keep its value, not take its \
             serde default"
        );
    }

    /// Persisting a live section is only half the write: the running runtime
    /// must be poked too.
    ///
    /// Asserted by reading the sub-agent fan-out cap back out of
    /// `agents::subagent_tool` — the value `SubagentTool::new` consults — rather
    /// than by checking that some function was called. That knob is the one arm
    /// of `apply_live_sections`'s `execution` case with no boot-installed
    /// handle, so it lands in a test process; the run-admission caps do not,
    /// which is exactly why the response's `reload_impact` is `restart` here.
    #[tokio::test]
    #[serial_test::serial(subagent_concurrency_cap)]
    async fn updating_execution_pushes_the_new_caps_onto_the_running_runtime() {
        use crate::agents::subagent_tool::{max_concurrent_subagents, set_max_concurrent_subagents};

        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());

        let restore = max_concurrent_subagents();
        let config = Arc::new(RwLock::new(Config::default()));

        let body = json!({ "max_concurrent_subagents": 9 });
        let request = JsonRpcRequest::with_id("execution_config.update", Some(body), json!(1));
        let response = handle_update(request, Arc::clone(&config), make_event_bus()).await;
        let observed = max_concurrent_subagents();
        set_max_concurrent_subagents(restore);

        assert!(
            response.is_success(),
            "execution_config.update failed: {response:?}"
        );
        assert_eq!(
            observed, 9,
            "execution_config.update persisted the section but never hot-applied it — \
             the running runtime kept its boot-time caps under a success response"
        );
    }
}
