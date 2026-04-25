use crate::context::DashboardState;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRuleInfo {
    pub index: usize,
    pub rule_type: String,
    pub regex: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    pub is_builtin: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRuleConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_type: Option<String>,
    pub regex: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strip_prefix: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

pub struct RoutingRulesApi;

impl RoutingRulesApi {
    /// List all routing rules
    pub async fn list(state: &DashboardState) -> Result<Vec<RoutingRuleInfo>, String> {
        let result = state.rpc_call("routing_rules.list", Value::Null).await?;

        result
            .get("rules")
            .ok_or_else(|| "Invalid response: missing rules".to_string())
            .and_then(|rules| {
                serde_json::from_value(rules.clone())
                    .map_err(|e| format!("Failed to parse rules: {}", e))
            })
    }

    /// Get a specific routing rule
    pub async fn get(state: &DashboardState, index: usize) -> Result<RoutingRuleInfo, String> {
        let params = serde_json::json!({
            "index": index,
        });

        let result = state.rpc_call("routing_rules.get", params).await?;

        result
            .get("rule")
            .ok_or_else(|| "Invalid response: missing rule".to_string())
            .and_then(|rule| {
                serde_json::from_value(rule.clone())
                    .map_err(|e| format!("Failed to parse rule: {}", e))
            })
    }

    /// Create a new routing rule
    pub async fn create(state: &DashboardState, rule: RoutingRuleConfig) -> Result<(), String> {
        let params = serde_json::json!({
            "rule": rule,
        });

        state.rpc_call("routing_rules.create", params).await?;
        Ok(())
    }

    /// Update an existing routing rule
    pub async fn update(
        state: &DashboardState,
        index: usize,
        rule: RoutingRuleConfig,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "index": index,
            "rule": rule,
        });

        state.rpc_call("routing_rules.update", params).await?;
        Ok(())
    }

    /// Delete a routing rule
    pub async fn delete(state: &DashboardState, index: usize) -> Result<(), String> {
        let params = serde_json::json!({
            "index": index,
        });

        state.rpc_call("routing_rules.delete", params).await?;
        Ok(())
    }

    /// Move a routing rule
    pub async fn move_rule(state: &DashboardState, from: usize, to: usize) -> Result<(), String> {
        let params = serde_json::json!({
            "from": from,
            "to": to,
        });

        state.rpc_call("routing_rules.move", params).await?;
        Ok(())
    }
}
