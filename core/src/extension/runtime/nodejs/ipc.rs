//! JSON-RPC 2.0 IPC protocol for Node.js plugins

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// JSON-RPC 2.0 request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: JsonValue,
}

impl JsonRpcRequest {
    pub fn new(id: impl Into<String>, method: impl Into<String>, params: JsonValue) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC 2.0 response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn success(id: impl Into<String>, result: JsonValue) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: impl Into<String>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }

    pub fn is_success(&self) -> bool {
        self.error.is_none()
    }
}

/// JSON-RPC 2.0 error object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<JsonValue>,
}

/// JSON-RPC 2.0 notification (no id, no response expected)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: JsonValue,
}

impl JsonRpcNotification {
    pub fn new(method: impl Into<String>, params: JsonValue) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params,
        }
    }
}

/// Plugin registration message from Node.js
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRegistrationParams {
    pub plugin_id: String,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub hooks: Vec<HookDefinition>,
    #[serde(default)]
    pub channels: Vec<ChannelDefinition>,
    #[serde(default)]
    pub providers: Vec<ProviderDefinition>,
    #[serde(default)]
    pub gateway_methods: Vec<GatewayMethodDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: JsonValue,
    pub handler: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDefinition {
    pub event: String,
    #[serde(default)]
    pub priority: i32,
    pub handler: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelDefinition {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderDefinition {
    pub id: String,
    pub name: String,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayMethodDefinition {
    pub method: String,
    pub handler: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_serialization() {
        let req = JsonRpcRequest::new("1", "plugin.call", serde_json::json!({"foo": "bar"}));
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"plugin.call\""));
    }

    #[test]
    fn test_response_success() {
        let resp = JsonRpcResponse::success("1", serde_json::json!({"result": "ok"}));
        assert!(resp.is_success());
    }

    #[test]
    fn test_response_error() {
        let resp = JsonRpcResponse::error("1", -32600, "Invalid request");
        assert!(!resp.is_success());
    }

    #[test]
    fn test_notification_new() {
        let notif = JsonRpcNotification::new("event.fired", serde_json::json!({"data": 123}));
        assert_eq!(notif.jsonrpc, "2.0");
        assert_eq!(notif.method, "event.fired");
    }

    #[test]
    fn test_plugin_registration_params_defaults() {
        let json = r#"{"plugin_id": "test-plugin"}"#;
        let params: PluginRegistrationParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.plugin_id, "test-plugin");
        assert!(params.tools.is_empty());
        assert!(params.hooks.is_empty());
        assert!(params.channels.is_empty());
        assert!(params.providers.is_empty());
        assert!(params.gateway_methods.is_empty());
    }

    #[test]
    fn test_tool_definition() {
        let tool = ToolDefinition {
            name: "my_tool".to_string(),
            description: "A test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            handler: "handleMyTool".to_string(),
        };
        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("\"name\":\"my_tool\""));
    }

    #[test]
    fn test_hook_definition_default_priority() {
        let json = r#"{"event": "before_agent_start", "handler": "onStart"}"#;
        let hook: HookDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(hook.priority, 0);
    }
}
