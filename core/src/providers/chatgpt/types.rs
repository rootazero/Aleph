//! Codex Responses API request/response types
//!
//! Types for the OpenAI Responses API used by the Codex backend
//! at `chatgpt.com/backend-api/codex/responses`.

use serde::{Deserialize, Serialize};

// ─── Request Types ───────────────────────────────────────────────

/// Codex Responses API request body
#[derive(Debug, Serialize)]
pub struct ResponsesRequest {
    pub model: String,
    pub input: Vec<InputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    pub stream: bool,
    pub store: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<FunctionToolDef>>,
}

/// Function tool definition for the Responses API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionToolDef {
    /// Always "function"
    #[serde(rename = "type")]
    pub tool_type: String,
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Input item in the conversation (tagged union)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum InputItem {
    /// A text message from user, assistant, or developer
    #[serde(rename = "message")]
    Message { role: String, content: String },
}

/// Reasoning effort configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReasoningConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

// ─── Response Types ──────────────────────────────────────────────

/// Top-level response resource from the Responses API
#[derive(Debug, Deserialize)]
pub struct ResponseResource {
    pub id: String,
    pub status: String,
    pub model: String,
    #[serde(default)]
    pub output: Vec<OutputItem>,
    #[serde(default)]
    pub usage: Option<UsageInfo>,
    #[serde(default)]
    pub error: Option<ResponseError>,
}

/// Output item in the response (tagged union)
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum OutputItem {
    /// Assistant text message
    #[serde(rename = "message")]
    Message {
        id: String,
        #[serde(default)]
        role: String,
        #[serde(default)]
        content: Vec<ContentPart>,
    },
    /// Reasoning trace
    #[serde(rename = "reasoning")]
    Reasoning {
        id: String,
        #[serde(default)]
        content: Option<String>,
        #[serde(default)]
        summary: Option<String>,
    },
    /// Function/tool call
    #[serde(rename = "function_call")]
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
        arguments: String,
    },
}

/// Text content part within a message output
#[derive(Debug, Deserialize)]
pub struct ContentPart {
    /// Usually "output_text"
    #[serde(rename = "type")]
    pub part_type: String,
    pub text: String,
}

/// Token usage information
#[derive(Debug, Deserialize)]
pub struct UsageInfo {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

/// Error detail in a failed response
#[derive(Debug, Deserialize)]
pub struct ResponseError {
    pub code: String,
    pub message: String,
}

// ─── Streaming Event Types ───────────────────────────────────────

/// SSE streaming events from the Responses API
///
/// Events arrive as `event: <type>\ndata: <json>\n\n`.
/// We only need to act on TextDelta (for streaming text),
/// Completed (final state), and Failed (error).
/// Other events are accepted but ignored.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum StreamEvent {
    #[serde(rename = "response.created")]
    Created { response: ResponseResource },

    #[serde(rename = "response.in_progress")]
    InProgress { response: ResponseResource },

    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        output_index: usize,
        item: OutputItem,
    },

    #[serde(rename = "response.content_part.added")]
    ContentPartAdded {
        output_index: usize,
        content_index: usize,
    },

    #[serde(rename = "response.output_text.delta")]
    TextDelta {
        delta: String,
        output_index: usize,
        content_index: usize,
    },

    #[serde(rename = "response.output_text.done")]
    TextDone {
        text: String,
        output_index: usize,
        content_index: usize,
    },

    #[serde(rename = "response.output_item.done")]
    OutputItemDone {
        output_index: usize,
        item: OutputItem,
    },

    #[serde(rename = "response.content_part.done")]
    ContentPartDone {
        output_index: usize,
        content_index: usize,
    },

    #[serde(rename = "response.completed")]
    Completed { response: ResponseResource },

    #[serde(rename = "response.failed")]
    Failed { response: ResponseResource },
}

// ─── Security Types (unchanged, used by security.rs) ─────────────

/// Chat requirements response (security tokens)
#[derive(Debug, Deserialize)]
pub struct ChatRequirements {
    pub token: String,
    #[serde(default)]
    pub proofofwork: Option<ProofOfWork>,
}

/// Proof-of-work challenge
#[derive(Debug, Deserialize)]
pub struct ProofOfWork {
    pub required: bool,
    pub seed: Option<String>,
    pub difficulty: Option<String>,
}
