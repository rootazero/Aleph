//! MCP-specific message types
//!
//! Protocol types for the Model Context Protocol (MCP) built on JSON-RPC 2.0.
//! Includes initialization, tools, resources, prompts, sampling, and approval types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

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

/// Streaming sampling response chunk
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SamplingChunk {
    /// Partial text content
    pub delta: String,
    /// Whether this is the final chunk
    #[serde(default)]
    pub is_final: bool,
    /// Model that generated the response (only in final chunk)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Stop reason (only in final chunk)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
}

impl SamplingChunk {
    /// Create a content chunk
    pub fn content(delta: impl Into<String>) -> Self {
        Self {
            delta: delta.into(),
            is_final: false,
            model: None,
            stop_reason: None,
        }
    }

    /// Create a final chunk
    pub fn final_chunk(model: Option<String>, stop_reason: StopReason) -> Self {
        Self {
            delta: String::new(),
            is_final: true,
            model,
            stop_reason: Some(stop_reason),
        }
    }
}

impl InitializeParams {
    /// Create default initialize params for Aleph
    pub fn aleph_default() -> Self {
        Self {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ClientCapabilities::default(),
            client_info: ClientInfo {
                name: "Aleph".to_string(),
                version: env!("ALEPH_VERSION").to_string(),
            },
        }
    }
}

// ===== Approval Types (Human-in-the-Loop) =====

/// Request for human approval of an action
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    /// Unique request ID
    pub request_id: String,
    /// Description of the action requiring approval
    pub action: String,
    /// Server requesting approval
    pub server_name: String,
    /// Details for the user to review
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    /// Timeout for response (seconds)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u32>,
}

impl ApprovalRequest {
    /// Create a new approval request
    pub fn new(
        request_id: impl Into<String>,
        action: impl Into<String>,
        server_name: impl Into<String>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            action: action.into(),
            server_name: server_name.into(),
            details: None,
            timeout_seconds: None,
        }
    }

    /// Set details for the request
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, seconds: u32) -> Self {
        self.timeout_seconds = Some(seconds);
        self
    }
}

/// Response to an approval request
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalResponse {
    /// Whether the action was approved
    pub approved: bool,
    /// Optional reason for rejection
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl ApprovalResponse {
    /// Create an approved response
    pub fn approved() -> Self {
        Self {
            approved: true,
            reason: None,
        }
    }

    /// Create a rejected response with optional reason
    pub fn rejected(reason: Option<String>) -> Self {
        Self {
            approved: false,
            reason,
        }
    }
}

/// Approval decision
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    /// User approved the action
    Approved,
    /// User rejected the action
    Rejected,
    /// Request timed out
    Timeout,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_initialize_params() {
        let params = InitializeParams::aleph_default();
        assert_eq!(params.client_info.name, "Aleph");
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
        let tool: ToolDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "file_read");
        assert!(tool.description.is_some());
    }

    #[test]
    fn test_resource_definition_deserialization() {
        let json = r#"{
            "uri": "file:///test.txt",
            "name": "test.txt",
            "description": "A test file",
            "mimeType": "text/plain"
        }"#;
        let resource: ResourceDefinition = serde_json::from_str(json).unwrap();
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
        let prompt: PromptDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(prompt.name, "code_review");
        assert_eq!(prompt.arguments.len(), 1);
        assert!(prompt.arguments[0].required);
    }

    #[test]
    fn test_resource_content_text() {
        let json = r#"{"type": "text", "uri": "file:///test.txt", "text": "Hello"}"#;
        let content: ResourceContentItem = serde_json::from_str(json).unwrap();
        assert!(matches!(content, ResourceContentItem::Text { .. }));
    }

    #[test]
    fn test_prompt_message_deserialization() {
        let json = r#"{"role": "user", "content": {"type": "text", "text": "Hello"}}"#;
        let msg: PromptMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg.role, PromptRole::User));
    }

    #[test]
    fn test_sampling_request_deserialization() {
        let json = r#"{
            "messages": [
                {"role": "user", "content": {"type": "text", "text": "Hello"}}
            ],
            "maxTokens": 1000
        }"#;
        let req: SamplingRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.max_tokens, Some(1000));
    }

    #[test]
    fn test_sampling_response_serialization() {
        let resp = SamplingResponse {
            role: PromptRole::Assistant,
            content: SamplingContent::Text {
                text: "Hello back!".to_string(),
            },
            model: Some("claude-3".to_string()),
            stop_reason: Some(StopReason::EndTurn),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("assistant"));
        assert!(json.contains("Hello back!"));
    }

    #[test]
    fn test_sampling_content_variants() {
        let text = SamplingContent::Text {
            text: "Hello".to_string(),
        };
        assert!(matches!(text, SamplingContent::Text { .. }));

        let image = SamplingContent::Image {
            data: "base64data".to_string(),
            mime_type: "image/png".to_string(),
        };
        assert!(matches!(image, SamplingContent::Image { .. }));
    }

    #[test]
    fn test_include_context_serialization() {
        let this_server = IncludeContext::ThisServer;
        let json = serde_json::to_string(&this_server).unwrap();
        assert_eq!(json, "\"thisServer\"");

        let all_servers = IncludeContext::AllServers;
        let json = serde_json::to_string(&all_servers).unwrap();
        assert_eq!(json, "\"allServers\"");

        let parsed: IncludeContext = serde_json::from_str("\"thisServer\"").unwrap();
        assert_eq!(parsed, IncludeContext::ThisServer);

        let parsed: IncludeContext = serde_json::from_str("\"allServers\"").unwrap();
        assert_eq!(parsed, IncludeContext::AllServers);
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
        let req: SamplingRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.include_context, Some(IncludeContext::AllServers));
    }
}
