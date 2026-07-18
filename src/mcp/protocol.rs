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
    /// Sampling support. Present (an empty `{}` object) when the client can
    /// service server-initiated `sampling/createMessage` requests. Aleph wires
    /// a sampling callback for every connection (see `manager::actor`), so this
    /// is advertised unconditionally — without it, spec-compliant servers never
    /// issue sampling requests and the already-wired handler stays dormant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<SamplingCapability>,
    /// Experimental / non-standard capabilities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<Value>,
}

/// Marker for an advertised client capability. MCP signals support with an
/// empty `{}` object, so this carries no fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SamplingCapability {}

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
    /// Optional instructions describing how to use the server's tools
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
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
    /// Behavioral hints (MCP spec `ToolAnnotations`). Absent on older
    /// servers; every field is optional and untrusted — hints inform
    /// scheduling (parallelism, retry) and approval friction, never
    /// security decisions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
}

/// MCP spec tool annotations — server-declared behavioral hints.
///
/// Spec defaults (when the annotations object is present but a field is
/// absent): `readOnlyHint=false`, `destructiveHint=true`,
/// `idempotentHint=false`, `openWorldHint=true`. The helpers below encode
/// the *conservative consumption* policy instead of the raw spec defaults:
/// a hint only relaxes behavior when explicitly `true` (read-only,
/// idempotent) and only adds friction when explicitly `true` (destructive),
/// so a server that omits annotations behaves exactly like one that sends
/// none at all.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    /// Human-readable display title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Tool does not modify its environment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    /// Tool may perform destructive updates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    /// Repeated calls with the same arguments have no additional effect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    /// Tool interacts with an open world of external entities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

impl ToolAnnotations {
    /// Explicitly declared read-only. Absent → `false` (conservative:
    /// unknown tools stay whole-world exclusive under parallel dispatch).
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.read_only_hint == Some(true)
    }

    /// Explicitly declared destructive. Absent → `false` (conservative the
    /// other way: confirmation friction is only added when the server
    /// asks for it, so wiring annotations doesn't suddenly gate every
    /// legacy MCP tool behind approval prompts).
    #[must_use]
    pub fn is_destructive(&self) -> bool {
        self.destructive_hint == Some(true)
    }

    /// Safe to retry with the same arguments: explicitly idempotent, or
    /// read-only (reads are idempotent by definition).
    #[must_use]
    pub fn is_idempotent(&self) -> bool {
        self.idempotent_hint == Some(true) || self.is_read_only()
    }
}

/// Tools list response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsListResult {
    /// Available tools
    pub tools: Vec<ToolDefinition>,
    /// Opaque pagination cursor (MCP spec). When present, more tools remain
    /// and the client must re-issue `tools/list` with `params.cursor` set to
    /// this value. Absent on the final (or only) page.
    #[serde(
        default,
        rename = "nextCursor",
        skip_serializing_if = "Option::is_none"
    )]
    pub next_cursor: Option<String>,
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
///
/// Wire-format content blocks from `tools/call` responses. Field names follow
/// the MCP spec's camelCase JSON (`mimeType`), embedded resources nest their
/// payload under a `resource` key, and unrecognized `type` tags degrade to
/// [`ToolResultContent::Unknown`] instead of failing the whole result — a
/// server speaking a newer spec revision must not brick every tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolResultContent {
    /// Text content
    #[serde(rename = "text")]
    Text { text: String },
    /// Image content (base64)
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    /// Audio content (base64), spec revision 2025-03-26
    #[serde(rename = "audio")]
    Audio {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    /// Link to a resource the client may read later, spec revision 2025-06-18
    #[serde(rename = "resource_link")]
    ResourceLink {
        uri: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// Embedded resource (payload nested under `resource` per spec)
    #[serde(rename = "resource")]
    Resource { resource: EmbeddedResource },
    /// Forward-compat fallback for unrecognized content types
    #[serde(other)]
    Unknown,
}

/// Embedded resource payload inside a `resource` content block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddedResource {
    /// Resource URI
    pub uri: String,
    /// MIME type
    #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Text contents (text resources)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Base64 contents (binary resources)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
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
    /// Opaque pagination cursor (MCP spec). When present, more resources
    /// remain and the client must re-issue `resources/list` with
    /// `params.cursor` set to this value. Absent on the final (or only) page.
    #[serde(
        default,
        rename = "nextCursor",
        skip_serializing_if = "Option::is_none"
    )]
    pub next_cursor: Option<String>,
}

/// Resource *template* definition (`resources/templates/list`). A template
/// advertises a parameterized URI (RFC 6570, e.g. `file:///{path}`) that the
/// model fills in and then reads via `resources/read`. Distinct from a concrete
/// [`ResourceDefinition`], which carries a ready-to-read `uri`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTemplateDefinition {
    /// URI template (RFC 6570), e.g. `file:///{path}`.
    pub uri_template: String,
    /// Human-readable name
    pub name: String,
    /// Template description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// MIME type of resources produced by this template
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// Resource-templates list response (`resources/templates/list`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceTemplatesListResult {
    /// Available resource templates (absent field → empty, so a server that
    /// omits the key parses as "no templates").
    #[serde(default, rename = "resourceTemplates")]
    pub resource_templates: Vec<ResourceTemplateDefinition>,
    /// Opaque pagination cursor — see [`ResourcesListResult::next_cursor`].
    #[serde(
        default,
        rename = "nextCursor",
        skip_serializing_if = "Option::is_none"
    )]
    pub next_cursor: Option<String>,
}

/// Resource read request parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceReadParams {
    /// Resource URI to read
    pub uri: String,
}

/// Resource content in read response
///
/// The spec's `resources/read` contents carry NO `type` discriminator — text
/// and binary entries are distinguished by which of `text` / `blob` is
/// present, so this enum must be untagged.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResourceContentItem {
    /// Text content (has a `text` key)
    Text {
        uri: String,
        #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        text: String,
    },
    /// Binary/blob content (has a `blob` key, base64)
    Blob {
        uri: String,
        #[serde(rename = "mimeType", default, skip_serializing_if = "Option::is_none")]
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
    /// Opaque pagination cursor (MCP spec). When present, more prompts remain
    /// and the client must re-issue `prompts/list` with `params.cursor` set to
    /// this value. Absent on the final (or only) page.
    #[serde(
        default,
        rename = "nextCursor",
        skip_serializing_if = "Option::is_none"
    )]
    pub next_cursor: Option<String>,
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
///
/// Same wire rules as [`ToolResultContent`]: camelCase field names, embedded
/// resources nested under a `resource` key, unknown types degrade instead of
/// failing the whole `prompts/get` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PromptContentItem {
    /// Text content
    #[serde(rename = "text")]
    Text { text: String },
    /// Image content
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    /// Audio content (base64), spec revision 2025-03-26
    #[serde(rename = "audio")]
    Audio {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    /// Embedded resource (payload nested under `resource` per spec)
    #[serde(rename = "resource")]
    Resource { resource: EmbeddedResource },
    /// Forward-compat fallback for unrecognized content types
    #[serde(other)]
    Unknown,
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
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
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
    #[must_use]
    pub const fn final_chunk(model: Option<String>, stop_reason: StopReason) -> Self {
        Self {
            delta: String::new(),
            is_final: true,
            model,
            stop_reason: Some(stop_reason),
        }
    }
}

/// MCP protocol revision Aleph speaks. Sent in the `initialize` body and as
/// the `MCP-Protocol-Version` HTTP header on Streamable HTTP requests. Pinned
/// to `2025-03-26` — the revision that introduced the Streamable HTTP transport
/// (`transport::http`) and audio content, both of which Aleph implements.
/// Servers may negotiate this down; the transport then echoes their value (see
/// `McpTransport::set_protocol_version`).
pub const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

impl InitializeParams {
    /// Create default initialize params for Aleph
    #[must_use]
    pub fn aleph_default() -> Self {
        Self {
            protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            capabilities: ClientCapabilities {
                sampling: Some(SamplingCapability::default()),
                experimental: None,
            },
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
    #[must_use]
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    /// Set timeout
    #[must_use]
    pub const fn with_timeout(mut self, seconds: u32) -> Self {
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
    #[must_use]
    pub const fn approved() -> Self {
        Self {
            approved: true,
            reason: None,
        }
    }

    /// Create a rejected response with optional reason
    #[must_use]
    pub const fn rejected(reason: Option<String>) -> Self {
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
    fn test_list_results_parse_next_cursor() {
        // Cursor present → carried through for the next page.
        let tools: ToolsListResult =
            serde_json::from_str(r#"{"tools": [], "nextCursor": "page2"}"#).unwrap();
        assert_eq!(tools.next_cursor.as_deref(), Some("page2"));

        let resources: ResourcesListResult =
            serde_json::from_str(r#"{"resources": [], "nextCursor": "r2"}"#).unwrap();
        assert_eq!(resources.next_cursor.as_deref(), Some("r2"));

        let prompts: PromptsListResult =
            serde_json::from_str(r#"{"prompts": [], "nextCursor": "p2"}"#).unwrap();
        assert_eq!(prompts.next_cursor.as_deref(), Some("p2"));
    }

    #[test]
    fn test_list_results_absent_cursor_is_backward_compatible() {
        // Pre-pagination servers omit the field → None (final page), and we do
        // not emit a null `nextCursor` back on serialization.
        let tools: ToolsListResult = serde_json::from_str(r#"{"tools": []}"#).unwrap();
        assert!(tools.next_cursor.is_none());
        let reserialized = serde_json::to_string(&tools).unwrap();
        assert!(!reserialized.contains("nextCursor"));
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
        // Real wire shape: resources/read contents have NO "type" field.
        let json = r#"{"uri": "file:///test.txt", "text": "Hello"}"#;
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
    fn tool_result_content_parses_spec_camelcase_and_unknown() {
        let json = r#"{
            "content": [
                {"type": "text", "text": "ok"},
                {"type": "image", "data": "aGk=", "mimeType": "image/png"},
                {"type": "audio", "data": "aGk=", "mimeType": "audio/wav"},
                {"type": "resource_link", "uri": "file:///a.txt", "name": "a"},
                {"type": "resource", "resource": {"uri": "file:///b.txt", "mimeType": "text/plain", "text": "body"}},
                {"type": "hologram", "data": "future"}
            ],
            "isError": false
        }"#;
        let result: ToolCallResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.content.len(), 6);
        assert!(matches!(
            &result.content[1],
            ToolResultContent::Image { mime_type, .. } if mime_type == "image/png"
        ));
        assert!(matches!(
            &result.content[2],
            ToolResultContent::Audio { .. }
        ));
        assert!(matches!(
            &result.content[3],
            ToolResultContent::ResourceLink { .. }
        ));
        assert!(matches!(
            &result.content[4],
            ToolResultContent::Resource { resource } if resource.text.as_deref() == Some("body")
        ));
        assert!(matches!(&result.content[5], ToolResultContent::Unknown));
    }

    #[test]
    fn resource_read_contents_parse_without_type_tag() {
        let json = r#"{"contents": [{"uri": "file:///t.txt", "mimeType": "text/plain", "text": "hello"}]}"#;
        let result: ResourceReadResult = serde_json::from_str(json).unwrap();
        assert!(matches!(
            &result.contents[0],
            ResourceContentItem::Text { mime_type, text, .. }
                if text == "hello" && mime_type.as_deref() == Some("text/plain")
        ));

        let json = r#"{"contents": [{"uri": "file:///b.bin", "mimeType": "application/octet-stream", "blob": "aGk="}]}"#;
        let result: ResourceReadResult = serde_json::from_str(json).unwrap();
        assert!(matches!(
            &result.contents[0],
            ResourceContentItem::Blob { blob, .. } if blob == "aGk="
        ));
    }

    #[test]
    fn prompt_message_embedded_resource_parses_nested_shape() {
        let json = r#"{"role": "user", "content": {"type": "resource", "resource": {"uri": "db://x", "text": "row"}}}"#;
        let msg: PromptMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(
            &msg.content,
            PromptContentItem::Resource { resource } if resource.uri == "db://x"
        ));
    }

    #[test]
    fn sampling_image_content_uses_camelcase_mime_type() {
        let json = r#"{"type": "image", "data": "aGk=", "mimeType": "image/jpeg"}"#;
        let content: SamplingContent = serde_json::from_str(json).unwrap();
        assert!(matches!(
            &content,
            SamplingContent::Image { mime_type, .. } if mime_type == "image/jpeg"
        ));
        let ser = serde_json::to_string(&content).unwrap();
        assert!(ser.contains("mimeType"));
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
    fn initialize_result_deserializes_with_instructions() {
        let json = r#"{
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "serverInfo": {"name": "test-server", "version": "1.0"},
            "instructions": "Use the search tool to find documents before answering."
        }"#;
        let result: InitializeResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.protocol_version, "2024-11-05");
        assert_eq!(
            result.instructions,
            Some("Use the search tool to find documents before answering.".to_string())
        );
    }

    #[test]
    fn initialize_result_deserializes_without_instructions() {
        let json = r#"{
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "serverInfo": {"name": "test-server"}
        }"#;
        let result: InitializeResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.protocol_version, "2024-11-05");
        assert_eq!(result.instructions, None);

        // Verify instructions is omitted when serialized as None
        let serialized = serde_json::to_string(&result).unwrap();
        assert!(!serialized.contains("instructions"));
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
