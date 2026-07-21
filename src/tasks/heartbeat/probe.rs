//! L1 Probe Executor
//!
//! Executes tool probes and evaluates trigger conditions to determine
//! whether L2 agent analysis should be triggered.

use async_trait::async_trait;

use crate::sync_primitives::Arc;
use serde_json::Value;

use crate::executor::{BuiltinToolRegistry, ToolRegistry};
use crate::tasks::heartbeat::config::{ProbeConfig, TriggerCondition};

// ── ProbeResult ──────────────────────────────────────────────────────────────

/// Result of a single L1 probe execution.
#[derive(Debug)]
pub struct ProbeResult {
    /// Raw value returned by the probe tool
    pub raw_value: Value,
    /// Whether the trigger condition evaluated to true
    pub triggered: bool,
    /// How long the probe took (milliseconds)
    pub duration_ms: i64,
}

// ── ProbeExecutor ────────────────────────────────────────────────────────────

/// Abstraction over tool execution for L1 probes.
///
/// Implementors call into the actual tool registry or a mock in tests.
#[async_trait]
pub trait ProbeExecutor: Send + Sync {
    async fn execute(&self, tool_name: &str, params: Option<&Value>) -> Result<Value, String>;
}

// ── DefaultProbeExecutor ─────────────────────────────────────────────────────

/// Production probe executor that calls tools via `BuiltinToolRegistry`.
pub struct DefaultProbeExecutor {
    registry: Arc<BuiltinToolRegistry>,
}

impl DefaultProbeExecutor {
    #[must_use]
    pub const fn new(registry: Arc<BuiltinToolRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl ProbeExecutor for DefaultProbeExecutor {
    async fn execute(&self, tool_name: &str, params: Option<&Value>) -> Result<Value, String> {
        // Heartbeat probes run on a timer without an LLM in the loop, so they
        // must never reach RCE / host-mutation / control-plane tools or any
        // tool that requires user confirmation — there is no approval
        // transport here. Mirrors the same gate the `tools.invoke` RPC uses
        // (`is_denied_on_gateway_surface`).
        if crate::security::dangerous_tools::is_denied_on_gateway_surface(tool_name) {
            return Err(format!(
                "Probe tool '{tool_name}' is denied on the heartbeat surface \
                 (dangerous or confirmation-gated); configure a read-only probe \
                 or grant it via {ALLOW_ENV} (DANGEROUS only).",
                ALLOW_ENV = crate::security::dangerous_tools::GATEWAY_TOOLS_ALLOW_ENV
            ));
        }
        let arguments = params.cloned().unwrap_or(serde_json::json!({}));
        let result: crate::error::Result<Value> =
            self.registry.execute_tool(tool_name, arguments).await;
        result.map_err(|e| format!("Probe tool '{tool_name}' failed: {e}"))
    }
}

// ── evaluate_trigger ─────────────────────────────────────────────────────────

/// Evaluate a trigger condition against a probe result value.
///
/// `last_result` is the JSON string of the previous probe output, used
/// only by `TriggerCondition::Changed`.
#[must_use]
pub fn evaluate_trigger(
    condition: &TriggerCondition,
    value: &Value,
    last_result: Option<&str>,
) -> bool {
    match condition {
        TriggerCondition::NonEmpty => !is_empty_value(value),
        TriggerCondition::GreaterThan(threshold) => value.as_f64().is_some_and(|v| v > *threshold),
        TriggerCondition::Contains(s) => {
            let text = match value {
                Value::String(str_val) => str_val.clone(),
                other => other.to_string(),
            };
            text.contains(s.as_str())
        }
        TriggerCondition::Changed => {
            let current = value.to_string();
            last_result.is_none_or(|prev| prev != current)
        }
        TriggerCondition::Always => true,
    }
}

// ── execute_probe ─────────────────────────────────────────────────────────────

/// Run a probe and evaluate its trigger condition.
///
/// The probe tool call is bounded by `timeout_secs` so a hung tool cannot
/// stall a heartbeat worker (and, via the shared semaphore, every other task).
pub async fn execute_probe(
    probe: &ProbeConfig,
    executor: &dyn ProbeExecutor,
    last_probe_result: Option<&str>,
    timeout_secs: u64,
) -> Result<ProbeResult, String> {
    let start = std::time::Instant::now();
    let raw_value = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs.max(1)),
        executor.execute(&probe.tool_name, probe.tool_params.as_ref()),
    )
    .await
    .map_err(|_| {
        format!(
            "probe tool '{}' timed out after {timeout_secs}s",
            probe.tool_name
        )
    })??;
    let duration_ms = start.elapsed().as_millis() as i64;
    let triggered = evaluate_trigger(&probe.trigger_condition, &raw_value, last_probe_result);
    Ok(ProbeResult {
        raw_value,
        triggered,
        duration_ms,
    })
}

// ── is_empty_value ────────────────────────────────────────────────────────────

fn is_empty_value(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Number(n) => n.as_f64().is_some_and(|f| f == 0.0),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        Value::Bool(b) => !b,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── evaluate_trigger ────────────────────────────────────────────────────

    #[test]
    fn test_non_empty_triggers_on_non_empty() {
        assert!(evaluate_trigger(
            &TriggerCondition::NonEmpty,
            &json!("hello"),
            None
        ));
        assert!(evaluate_trigger(
            &TriggerCondition::NonEmpty,
            &json!(42),
            None
        ));
        assert!(evaluate_trigger(
            &TriggerCondition::NonEmpty,
            &json!([1, 2]),
            None
        ));
        assert!(evaluate_trigger(
            &TriggerCondition::NonEmpty,
            &json!(true),
            None
        ));
    }

    #[test]
    fn test_non_empty_does_not_trigger_on_empty() {
        assert!(!evaluate_trigger(
            &TriggerCondition::NonEmpty,
            &json!(""),
            None
        ));
        assert!(!evaluate_trigger(
            &TriggerCondition::NonEmpty,
            &json!(0),
            None
        ));
        assert!(!evaluate_trigger(
            &TriggerCondition::NonEmpty,
            &json!([]),
            None
        ));
        assert!(!evaluate_trigger(
            &TriggerCondition::NonEmpty,
            &json!({}),
            None
        ));
        assert!(!evaluate_trigger(
            &TriggerCondition::NonEmpty,
            &Value::Null,
            None
        ));
        assert!(!evaluate_trigger(
            &TriggerCondition::NonEmpty,
            &json!(false),
            None
        ));
    }

    #[test]
    fn test_greater_than_triggers_when_above_threshold() {
        assert!(evaluate_trigger(
            &TriggerCondition::GreaterThan(5.0),
            &json!(6),
            None
        ));
        assert!(evaluate_trigger(
            &TriggerCondition::GreaterThan(0.0),
            &json!(0.1),
            None
        ));
    }

    #[test]
    fn test_greater_than_does_not_trigger_when_at_or_below_threshold() {
        assert!(!evaluate_trigger(
            &TriggerCondition::GreaterThan(5.0),
            &json!(5),
            None
        ));
        assert!(!evaluate_trigger(
            &TriggerCondition::GreaterThan(5.0),
            &json!(4.9),
            None
        ));
        assert!(!evaluate_trigger(
            &TriggerCondition::GreaterThan(5.0),
            &json!("text"),
            None
        ));
    }

    #[test]
    fn test_contains_triggers_on_match() {
        assert!(evaluate_trigger(
            &TriggerCondition::Contains("error".to_string()),
            &json!("some error occurred"),
            None
        ));
    }

    #[test]
    fn test_contains_does_not_trigger_without_match() {
        assert!(!evaluate_trigger(
            &TriggerCondition::Contains("error".to_string()),
            &json!("everything is fine"),
            None
        ));
    }

    #[test]
    fn test_contains_works_on_non_string_values() {
        // JSON serialization of array should be searchable
        assert!(evaluate_trigger(
            &TriggerCondition::Contains("1".to_string()),
            &json!([1, 2, 3]),
            None
        ));
    }

    #[test]
    fn test_changed_triggers_when_value_differs() {
        assert!(evaluate_trigger(
            &TriggerCondition::Changed,
            &json!(42),
            Some("99")
        ));
    }

    #[test]
    fn test_changed_does_not_trigger_when_same() {
        // Value::Number(42) serializes to "42"
        assert!(!evaluate_trigger(
            &TriggerCondition::Changed,
            &json!(42),
            Some("42")
        ));
    }

    #[test]
    fn test_changed_triggers_when_no_previous_result() {
        assert!(evaluate_trigger(
            &TriggerCondition::Changed,
            &json!(1),
            None
        ));
    }

    #[test]
    fn test_always_triggers_unconditionally() {
        assert!(evaluate_trigger(
            &TriggerCondition::Always,
            &Value::Null,
            None
        ));
        assert!(evaluate_trigger(&TriggerCondition::Always, &json!(0), None));
        assert!(evaluate_trigger(
            &TriggerCondition::Always,
            &json!(""),
            None
        ));
        assert!(evaluate_trigger(
            &TriggerCondition::Always,
            &json!(false),
            None
        ));
    }

    // ── execute_probe ───────────────────────────────────────────────────────

    struct MockExecutor {
        result: Value,
    }

    #[async_trait]
    impl ProbeExecutor for MockExecutor {
        async fn execute(
            &self,
            _tool_name: &str,
            _params: Option<&Value>,
        ) -> Result<Value, String> {
            Ok(self.result.clone())
        }
    }

    struct FailingExecutor;

    #[async_trait]
    impl ProbeExecutor for FailingExecutor {
        async fn execute(
            &self,
            _tool_name: &str,
            _params: Option<&Value>,
        ) -> Result<Value, String> {
            Err("tool execution failed".to_string())
        }
    }

    #[tokio::test]
    async fn test_execute_probe_always_triggers() {
        let executor = MockExecutor { result: json!(42) };
        let probe = ProbeConfig {
            tool_name: "test.tool".to_string(),
            tool_params: None,
            trigger_condition: TriggerCondition::Always,
        };
        let result = execute_probe(&probe, &executor, None, 30).await.unwrap();
        assert!(result.triggered);
        assert_eq!(result.raw_value, json!(42));
        assert!(result.duration_ms >= 0);
    }

    #[tokio::test]
    async fn test_execute_probe_propagates_error() {
        let executor = FailingExecutor;
        let probe = ProbeConfig {
            tool_name: "test.tool".to_string(),
            tool_params: None,
            trigger_condition: TriggerCondition::Always,
        };
        let result = execute_probe(&probe, &executor, None, 30).await;
        assert!(result.is_err());
    }

    struct HangingExecutor;

    #[async_trait]
    impl ProbeExecutor for HangingExecutor {
        async fn execute(
            &self,
            _tool_name: &str,
            _params: Option<&Value>,
        ) -> Result<Value, String> {
            // Far longer than any probe timeout.
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            Ok(Value::Null)
        }
    }

    /// H7: a hung probe tool is bounded by the timeout, not left to stall.
    #[tokio::test(start_paused = true)]
    async fn test_execute_probe_times_out() {
        let executor = HangingExecutor;
        let probe = ProbeConfig {
            tool_name: "slow.tool".to_string(),
            tool_params: None,
            trigger_condition: TriggerCondition::Always,
        };
        let result = execute_probe(&probe, &executor, None, 1).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("timed out"));
    }
}
