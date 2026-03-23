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
    /// Tool selection strategy ("auto", "required", "none")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
    /// Enable parallel tool calls
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    /// Text output verbosity config (Codex mode)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<TextConfig>,
    /// Maximum number of output tokens (prevents silent truncation)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Additional fields to include in response (e.g. reasoning.encrypted_content)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
}

/// Text output verbosity configuration (Codex mode)
#[derive(Debug, Serialize)]
pub struct TextConfig {
    pub verbosity: String,
}

/// Function tool definition for the Responses API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionToolDef {
    /// Always "function"
    #[serde(rename = "type")]
    pub tool_type: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: serde_json::Value,
    /// Enable strict mode for reliable argument generation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

/// Input item in the conversation (tagged union)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum InputItem {
    /// A text message from user, assistant, or developer
    #[serde(rename = "message")]
    Message {
        role: String,
        #[serde(flatten)]
        content: MessageContent,
    },
    /// A function call from the assistant
    #[serde(rename = "function_call")]
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    /// Output from a function call
    #[serde(rename = "function_call_output")]
    FunctionCallOutput { call_id: String, output: String },
}

/// Message content — either plain text or multimodal (text + images)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum MessageContent {
    /// Simple text content
    Text { content: String },
    /// Multimodal content array (text + images)
    Multimodal { content: Vec<InputContentPart> },
}

impl MessageContent {
    /// Get the text content (for Text variant) or concatenated text parts (for Multimodal)
    pub fn as_text(&self) -> String {
        match self {
            Self::Text { content } => content.clone(),
            Self::Multimodal { content } => content
                .iter()
                .filter_map(|p| match p {
                    InputContentPart::InputText { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

/// A part of multimodal input content
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum InputContentPart {
    /// Text part
    #[serde(rename = "input_text")]
    InputText { text: String },
    /// Image part (base64 data URI)
    #[serde(rename = "input_image")]
    InputImage { image_url: String },
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

    /// Function call arguments delta (streaming tool call arguments)
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta {
        item_id: String,
        #[serde(default)]
        output_index: Option<usize>,
        delta: String,
    },

    /// Function call arguments complete
    #[serde(rename = "response.function_call_arguments.done")]
    FunctionCallArgumentsDone {
        item_id: String,
        #[serde(default)]
        output_index: Option<usize>,
        arguments: String,
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
