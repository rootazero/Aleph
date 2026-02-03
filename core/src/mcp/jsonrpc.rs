//! JSON-RPC 2.0 Protocol Implementation
//!
//! Provides types and utilities for MCP's JSON-RPC based communication.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};

/// JSON-RPC 2.0 Request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// JSON-RPC version (always "2.0")
    pub jsonrpc: String,
    /// Request ID for matching responses
    pub id: u64,
    /// Method name to invoke
    pub method: String,
    /// Optional parameters
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// Create a new JSON-RPC request
    pub fn new(id: u64, method: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.into(),
            params: None,
        }
    }

    /// Create a new request with parameters
    pub fn with_params(id: u64, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.into(),
            params: Some(params),
        }
    }

    /// Serialize to JSON string with newline delimiter
    pub fn to_json_line(&self) -> Result<String, serde_json::Error> {
        let json = serde_json::to_string(self)?;
        Ok(format!("{}\n", json))
    }
}

/// JSON-RPC 2.0 Notification (no id field per spec)
///
/// Notifications are requests without an id field. The server MUST NOT
/// reply to a notification, and the client should not expect a response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    /// JSON-RPC version (always "2.0")
    pub jsonrpc: String,
    /// Method name to invoke
    pub method: String,
    /// Optional parameters
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcNotification {
    /// Create a new JSON-RPC notification
    pub fn new(method: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params: None,
        }
    }

    /// Create a new notification with parameters
    pub fn with_params(method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params: Some(params),
        }
    }

    /// Serialize to JSON string with newline delimiter
    pub fn to_json_line(&self) -> Result<String, serde_json::Error> {
        let json = serde_json::to_string(self)?;
        Ok(format!("{}\n", json))
    }
}

/// JSON-RPC 2.0 Response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// JSON-RPC version (always "2.0")
    pub jsonrpc: String,
    /// Request ID this response corresponds to
    pub id: Option<u64>,
    /// Successful result (mutually exclusive with error)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error object (mutually exclusive with result)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// Check if this is a successful response
    pub fn is_success(&self) -> bool {
        self.error.is_none() && self.result.is_some()
    }

    /// Check if this is an error response
    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }

    /// Get the result, consuming the response
    pub fn into_result(self) -> Result<Value, JsonRpcError> {
        if let Some(error) = self.error {
            Err(error)
        } else {
            Ok(self.result.unwrap_or(Value::Null))
        }
    }

    /// Parse from JSON string
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

/// JSON-RPC 2.0 Error Object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Error code
    pub code: i32,
    /// Human-readable error message
    pub message: String,
    /// Optional additional data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl std::fmt::Display for JsonRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSON-RPC Error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for JsonRpcError {}

/// Standard JSON-RPC error codes
pub mod error_codes {
    /// Parse error - Invalid JSON
    pub const PARSE_ERROR: i32 = -32700;
    /// Invalid Request - Not a valid JSON-RPC request
    pub const INVALID_REQUEST: i32 = -32600;
    /// Method not found
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// Invalid params
    pub const INVALID_PARAMS: i32 = -32602;
    /// Internal error
    pub const INTERNAL_ERROR: i32 = -32603;
}

/// Thread-safe request ID generator
pub struct IdGenerator {
    next_id: AtomicU64,
}

impl IdGenerator {
    /// Create a new ID generator starting from 1
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
        }
    }

    /// Generate the next unique ID
    pub fn next(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }
}

impl Default for IdGenerator {
    fn default() -> Self {
        Self::new()
    }
}

/// MCP-specific message types
pub mod mcp {
    use super::*;

    /// MCP Initialize request parameters
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct InitializeParams {
        /// Protocol version
        pub protocol_version: String,
        /// Client capabilities
        pub capabilities: ClientCapabilities,
        /// Client info
        pub client_info: ClientInfo,
    }

    /// Client capabilities
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct ClientCapabilities {
        /// Supported features
        #[serde(skip_serializing_if = "Option::is_none")]
        pub experimental: Option<Value>,
    }

    /// Client info
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ClientInfo {
        /// Client name
        pub name: String,
        /// Client version
        pub version: String,
    }

    /// MCP Initialize response result
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct InitializeResult {
        /// Protocol version
        pub protocol_version: String,
        /// Server capabilities
        pub capabilities: ServerCapabilities,
        /// Server info
        #[serde(skip_serializing_if = "Option::is_none")]
        pub server_info: Option<ServerInfo>,
    }

    /// Server capabilities
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    pub struct ServerCapabilities {
        /// Tool support
        #[serde(skip_serializing_if = "Option::is_none")]
        pub tools: Option<ToolCapability>,
        /// Resource support
        #[serde(skip_serializing_if = "Option::is_none")]
        pub resources: Option<ResourceCapability>,
        /// Prompt support
        #[serde(skip_serializing_if = "Option::is_none")]
        pub prompts: Option<PromptCapability>,
    }

    /// Tool capability config
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ToolCapability {
        /// List changed notifications
        #[serde(skip_serializing_if = "Option::is_none")]
        pub list_changed: Option<bool>,
    }

    /// Resource capability config
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ResourceCapability {
        /// Subscribe support
        #[serde(skip_serializing_if = "Option::is_none")]
        pub subscribe: Option<bool>,
        /// List changed notifications
        #[serde(skip_serializing_if = "Option::is_none")]
        pub list_changed: Option<bool>,
    }

    /// Prompt capability config
    #[derive(Debug, Clone, Default, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct PromptCapability {
        /// List changed notifications
        #[serde(skip_serializing_if = "Option::is_none")]
        pub list_changed: Option<bool>,
    }

    /// Server info
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ServerInfo {
        /// Server name
        pub name: String,
        /// Server version
        #[serde(skip_serializing_if = "Option::is_none")]
        pub version: Option<String>,
    }

    /// MCP Tool definition from server
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ToolDefinition {
        /// Tool name
        pub name: String,
        /// Tool description
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        /// Input schema (JSON Schema)
        #[serde(skip_serializing_if = "Option::is_none")]
        pub input_schema: Option<Value>,
    }

    /// Tools list response
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ToolsListResult {
        /// Available tools
        pub tools: Vec<ToolDefinition>,
    }

    /// Tool call request parameters
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ToolCallParams {
        /// Tool name
        pub name: String,
        /// Tool arguments
        #[serde(skip_serializing_if = "Option::is_none")]
        pub arguments: Option<Value>,
    }

    /// Tool call result content
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "type")]
    pub enum ToolResultContent {
        /// Text content
        #[serde(rename = "text")]
        Text { text: String },
        /// Image content (base64)
        #[serde(rename = "image")]
        Image { data: String, mime_type: String },
        /// Resource reference
        #[serde(rename = "resource")]
        Resource { uri: String, text: Option<String> },
    }

    /// Tool call result
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ToolCallResult {
        /// Result content
        pub content: Vec<ToolResultContent>,
        /// Whether the tool execution failed
        #[serde(skip_serializing_if = "Option::is_none")]
        pub is_error: Option<bool>,
    }

    // ===== Resources RPC Types =====

    /// Resource definition from server
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ResourceDefinition {
        /// Resource URI
        pub uri: String,
        /// Human-readable name
        pub name: String,
        /// Resource description
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        /// MIME type
        #[serde(skip_serializing_if = "Option::is_none")]
        pub mime_type: Option<String>,
    }

    /// Resources list response
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ResourcesListResult {
        /// Available resources
        pub resources: Vec<ResourceDefinition>,
    }

    /// Resource read request parameters
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ResourceReadParams {
        /// Resource URI to read
        pub uri: String,
    }

    /// Resource content in read response
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "type")]
    pub enum ResourceContentItem {
        /// Text content
        #[serde(rename = "text")]
        Text {
            uri: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            mime_type: Option<String>,
            text: String,
        },
        /// Binary/blob content (base64)
        #[serde(rename = "blob")]
        Blob {
            uri: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            mime_type: Option<String>,
            blob: String,
        },
    }

    /// Resource read response
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ResourceReadResult {
        /// Resource contents
        pub contents: Vec<ResourceContentItem>,
    }

    // ===== Prompts RPC Types =====

    /// Prompt argument definition
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PromptArgument {
        /// Argument name
        pub name: String,
        /// Argument description
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        /// Whether required
        #[serde(default)]
        pub required: bool,
    }

    /// Prompt definition from server
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PromptDefinition {
        /// Prompt name
        pub name: String,
        /// Prompt description
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        /// Prompt arguments
        #[serde(default)]
        pub arguments: Vec<PromptArgument>,
    }

    /// Prompts list response
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PromptsListResult {
        /// Available prompts
        pub prompts: Vec<PromptDefinition>,
    }

    /// Prompt get request parameters
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PromptGetParams {
        /// Prompt name
        pub name: String,
        /// Prompt arguments
        #[serde(skip_serializing_if = "Option::is_none")]
        pub arguments: Option<std::collections::HashMap<String, serde_json::Value>>,
    }

    /// Message role in prompt response
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum PromptRole {
        User,
        Assistant,
        System,
    }

    /// Content in a prompt message
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "type")]
    pub enum PromptContentItem {
        /// Text content
        #[serde(rename = "text")]
        Text { text: String },
        /// Image content
        #[serde(rename = "image")]
        Image { data: String, mime_type: String },
        /// Resource reference
        #[serde(rename = "resource")]
        Resource {
            uri: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            text: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            mime_type: Option<String>,
        },
    }

    /// Message in prompt response
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PromptMessage {
        /// Message role
        pub role: PromptRole,
        /// Message content
        pub content: PromptContentItem,
    }

    /// Prompt get response
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PromptGetResult {
        /// Optional description
        #[serde(skip_serializing_if = "Option::is_none")]
        pub description: Option<String>,
        /// Prompt messages
        pub messages: Vec<PromptMessage>,
    }

    // ===== Sampling RPC Types (P2) =====

    /// Content types for sampling messages
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "type")]
    pub enum SamplingContent {
        /// Text content
        #[serde(rename = "text")]
        Text { text: String },
        /// Image content (base64)
        #[serde(rename = "image")]
        Image { data: String, mime_type: String },
    }

    /// Message in a sampling request
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SamplingMessage {
        /// Message role
        pub role: PromptRole,
        /// Message content
        pub content: SamplingContent,
    }

    /// Context inclusion mode for sampling requests
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "camelCase")]
    pub enum IncludeContext {
        /// Include context from the requesting server only
        ThisServer,
        /// Include context from all connected MCP servers
        AllServers,
    }

    /// Sampling/createMessage request from server
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct SamplingRequest {
        /// Messages to send to client LLM
        pub messages: Vec<SamplingMessage>,
        /// Optional model hint (client may ignore)
        #[serde(skip_serializing_if = "Option::is_none")]
        pub model_preferences: Option<ModelPreferences>,
        /// System prompt override
        #[serde(skip_serializing_if = "Option::is_none")]
        pub system_prompt: Option<String>,
        /// Include context from MCP servers
        /// - "thisServer": Include context from the requesting server only
        /// - "allServers": Include context from all connected servers
        #[serde(skip_serializing_if = "Option::is_none")]
        pub include_context: Option<IncludeContext>,
        /// Max tokens for response
        #[serde(skip_serializing_if = "Option::is_none")]
        pub max_tokens: Option<u32>,
    }

    /// Model preferences for sampling
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ModelPreferences {
        /// Hints for model selection
        #[serde(default)]
        pub hints: Vec<ModelHint>,
        /// Cost priority (0-1)
        #[serde(skip_serializing_if = "Option::is_none")]
        pub cost_priority: Option<f32>,
        /// Speed priority (0-1)
        #[serde(skip_serializing_if = "Option::is_none")]
        pub speed_priority: Option<f32>,
        /// Intelligence priority (0-1)
        #[serde(skip_serializing_if = "Option::is_none")]
        pub intelligence_priority: Option<f32>,
    }

    /// Model hint
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct ModelHint {
        /// Model name hint
        #[serde(skip_serializing_if = "Option::is_none")]
        pub name: Option<String>,
    }

    /// Stop reason for sampling response
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum StopReason {
        EndTurn,
        StopSequence,
        MaxTokens,
    }

    /// Sampling/createMessage response
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct SamplingResponse {
        /// Response role (usually "assistant")
        pub role: PromptRole,
        /// Response content
        pub content: SamplingContent,
        /// Model that generated the response
        #[serde(skip_serializing_if = "Option::is_none")]
        pub model: Option<String>,
        /// Stop reason
        #[serde(skip_serializing_if = "Option::is_none")]
        pub stop_reason: Option<StopReason>,
    }

    impl InitializeParams {
        /// Create default initialize params for Aether
        pub fn aether_default() -> Self {
            Self {
                protocol_version: "2024-11-05".to_string(),
                capabilities: ClientCapabilities::default(),
                client_info: ClientInfo {
                    name: "Aether".to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_request_serialization() {
        let req = JsonRpcRequest::new(1, "test_method");
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"method\":\"test_method\""));
    }

    #[test]
    fn test_request_with_params() {
        let req = JsonRpcRequest::with_params(2, "tools/call", json!({"name": "test_tool"}));
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"params\""));
        assert!(json.contains("\"name\":\"test_tool\""));
    }

    #[test]
    fn test_request_to_json_line() {
        let req = JsonRpcRequest::new(1, "test");
        let line = req.to_json_line().unwrap();
        assert!(line.ends_with('\n'));
    }

    #[test]
    fn test_response_success() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"data":"test"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(resp.is_success());
        assert!(!resp.is_error());
        assert_eq!(resp.result.unwrap()["data"], "test");
    }

    #[test]
    fn test_response_error() {
        let json =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(resp.is_error());
        assert!(!resp.is_success());
        let err = resp.error.unwrap();
        assert_eq!(err.code, error_codes::METHOD_NOT_FOUND);
    }

    #[test]
    fn test_into_result_success() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(1),
            result: Some(json!({"status": "ok"})),
            error: None,
        };
        let result = resp.into_result().unwrap();
        assert_eq!(result["status"], "ok");
    }

    #[test]
    fn test_into_result_error() {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(1),
            result: None,
            error: Some(JsonRpcError {
                code: -32600,
                message: "Invalid request".to_string(),
                data: None,
            }),
        };
        let result = resp.into_result();
        assert!(result.is_err());
    }

    #[test]
    fn test_id_generator() {
        let gen = IdGenerator::new();
        assert_eq!(gen.next(), 1);
        assert_eq!(gen.next(), 2);
        assert_eq!(gen.next(), 3);
    }

    #[test]
    fn test_mcp_initialize_params() {
        let params = mcp::InitializeParams::aether_default();
        assert_eq!(params.client_info.name, "Aether");
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("protocolVersion"));
    }

    #[test]
    fn test_tool_definition_deserialization() {
        let json = r#"{
            "name": "file_read",
            "description": "Read a file",
            "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}}
        }"#;
        let tool: mcp::ToolDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "file_read");
        assert!(tool.description.is_some());
    }

    #[test]
    fn test_notification_serialization() {
        let notif = JsonRpcNotification::new("notifications/initialized");
        let json = serde_json::to_string(&notif).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"notifications/initialized\""));
        // Notifications should NOT have an id field
        assert!(!json.contains("\"id\""));
    }

    #[test]
    fn test_notification_with_params() {
        let notif =
            JsonRpcNotification::with_params("tools/listChanged", json!({"reason": "refresh"}));
        let json = serde_json::to_string(&notif).unwrap();
        assert!(json.contains("\"params\""));
        assert!(json.contains("\"reason\":\"refresh\""));
        assert!(!json.contains("\"id\""));
    }

    #[test]
    fn test_notification_to_json_line() {
        let notif = JsonRpcNotification::new("test/notify");
        let line = notif.to_json_line().unwrap();
        assert!(line.ends_with('\n'));
        assert!(!line.contains("\"id\""));
    }

    #[test]
    fn test_resource_definition_deserialization() {
        let json = r#"{
            "uri": "file:///test.txt",
            "name": "test.txt",
            "description": "A test file",
            "mimeType": "text/plain"
        }"#;
        let resource: mcp::ResourceDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(resource.uri, "file:///test.txt");
        assert_eq!(resource.mime_type, Some("text/plain".to_string()));
    }

    #[test]
    fn test_prompt_definition_deserialization() {
        let json = r#"{
            "name": "code_review",
            "description": "Review code changes",
            "arguments": [
                {"name": "code", "description": "Code to review", "required": true}
            ]
        }"#;
        let prompt: mcp::PromptDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(prompt.name, "code_review");
        assert_eq!(prompt.arguments.len(), 1);
        assert!(prompt.arguments[0].required);
    }

    #[test]
    fn test_resource_content_text() {
        let json = r#"{"type": "text", "uri": "file:///test.txt", "text": "Hello"}"#;
        let content: mcp::ResourceContentItem = serde_json::from_str(json).unwrap();
        assert!(matches!(content, mcp::ResourceContentItem::Text { .. }));
    }

    #[test]
    fn test_prompt_message_deserialization() {
        let json = r#"{"role": "user", "content": {"type": "text", "text": "Hello"}}"#;
        let msg: mcp::PromptMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg.role, mcp::PromptRole::User));
    }

    #[test]
    fn test_sampling_request_deserialization() {
        let json = r#"{
            "messages": [
                {"role": "user", "content": {"type": "text", "text": "Hello"}}
            ],
            "maxTokens": 1000
        }"#;
        let req: mcp::SamplingRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.max_tokens, Some(1000));
    }

    #[test]
    fn test_sampling_response_serialization() {
        let resp = mcp::SamplingResponse {
            role: mcp::PromptRole::Assistant,
            content: mcp::SamplingContent::Text { text: "Hello back!".to_string() },
            model: Some("claude-3".to_string()),
            stop_reason: Some(mcp::StopReason::EndTurn),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("assistant"));
        assert!(json.contains("Hello back!"));
    }

    #[test]
    fn test_sampling_content_variants() {
        let text = mcp::SamplingContent::Text { text: "Hello".to_string() };
        assert!(matches!(text, mcp::SamplingContent::Text { .. }));

        let image = mcp::SamplingContent::Image {
            data: "base64data".to_string(),
            mime_type: "image/png".to_string()
        };
        assert!(matches!(image, mcp::SamplingContent::Image { .. }));
    }

    #[test]
    fn test_include_context_serialization() {
        // Test serialization to camelCase as per MCP spec
        let this_server = mcp::IncludeContext::ThisServer;
        let json = serde_json::to_string(&this_server).unwrap();
        assert_eq!(json, "\"thisServer\"");

        let all_servers = mcp::IncludeContext::AllServers;
        let json = serde_json::to_string(&all_servers).unwrap();
        assert_eq!(json, "\"allServers\"");

        // Test deserialization
        let parsed: mcp::IncludeContext = serde_json::from_str("\"thisServer\"").unwrap();
        assert_eq!(parsed, mcp::IncludeContext::ThisServer);

        let parsed: mcp::IncludeContext = serde_json::from_str("\"allServers\"").unwrap();
        assert_eq!(parsed, mcp::IncludeContext::AllServers);
    }

    #[test]
    fn test_sampling_request_with_include_context() {
        let json = r#"{
            "messages": [
                {"role": "user", "content": {"type": "text", "text": "Hello"}}
            ],
            "includeContext": "allServers",
            "maxTokens": 1000
        }"#;
        let req: mcp::SamplingRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.include_context, Some(mcp::IncludeContext::AllServers));
    }
}
