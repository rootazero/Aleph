/// Ollama local LLM client implementation
///
/// Implements the `AiProvider` trait for locally-hosted Ollama models using
/// the native chat endpoint (`/api/chat`).
///
/// # Why `/api/chat` (not `/api/generate`)
///
/// The legacy `/api/generate` endpoint accepts only a single `prompt` string,
/// which forced this provider to discard all but the last user message — every
/// turn arrived at the model amnesiac, with no assistant history, tool results,
/// or earlier user turns. `/api/chat` takes a structured `messages` array, so it
/// preserves the full multi-turn conversation, carries native `tools` for
/// function calling, and attaches per-message `images` for vision models — the
/// three capabilities the agent loop actually depends on. This mirrors the
/// native-API path openclaw documents as the only reliable surface for tool
/// calling.
///
/// # Configuration
///
/// Required fields:
/// - `model`: Model name (e.g., "llama3.2", "qwen2.5", "llava")
///
/// Optional fields:
/// - `base_url`: Ollama server URL (defaults to "<http://localhost:11434>")
/// - `timeout_seconds`: Request timeout (defaults to 60)
///
/// # Prerequisites
///
/// Ollama must be installed and running, and models pulled before use:
/// ```bash
/// ollama pull llama3.2
/// ollama pull llava       # vision-capable
/// ```
///
/// # Vision Support
///
/// Vision-capable models (llava, bakllava, etc.) receive any
/// `ContentBlock::Image` blocks as the message's base64 `images` array.
use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{
    NativeToolCall, ProviderResponse, RequestPayload, StopReason, TokenUsage,
};
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::AiProvider;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Default Ollama server URL
const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";

/// Ollama local provider
pub struct OllamaProvider {
    /// Provider name (e.g., "ollama", "ollama-llama3")
    name: String,
    /// Model name (e.g., "llama3.2", "llava")
    model: String,
    /// HTTP client
    client: Client,
    /// API endpoint (`{base_url}/api/chat`)
    endpoint: String,
    /// Provider brand color
    color: String,
    /// Provider config for advanced options
    config: ProviderConfig,
}

/// Optional generation parameters (Ollama `options` object).
#[derive(Debug, Serialize)]
struct GenerateOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repeat_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<u32>,
}

/// Response from Ollama `/api/chat` (non-streaming, `stream: false`).
#[derive(Debug, Deserialize)]
struct ChatResponse {
    message: ChatMessage,
    /// Why generation stopped (`"stop"`, `"length"`, …). Absent on older servers.
    #[serde(default)]
    done_reason: Option<String>,
    /// Prompt tokens evaluated (input).
    #[serde(default)]
    prompt_eval_count: Option<u32>,
    /// Tokens generated (output).
    #[serde(default)]
    eval_count: Option<u32>,
}

/// Assistant message inside a chat response.
#[derive(Debug, Deserialize)]
struct ChatMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Option<Vec<ChatToolCall>>,
}

/// A single tool call emitted by the model. Ollama assigns no id and returns
/// `arguments` as a JSON object (not a stringified blob like `OpenAI`).
#[derive(Debug, Deserialize)]
struct ChatToolCall {
    function: ChatFunctionCall,
}

#[derive(Debug, Deserialize)]
struct ChatFunctionCall {
    name: String,
    #[serde(default)]
    arguments: Value,
}

/// Error response from Ollama API
#[derive(Debug, Deserialize)]
struct OllamaError {
    error: String,
}

impl OllamaProvider {
    /// Create new Ollama provider
    ///
    /// # Errors
    ///
    /// Returns `InvalidConfig` if the model name is empty or the timeout is zero.
    pub fn new(name: String, config: ProviderConfig) -> Result<Self> {
        if config.default_model().is_empty() {
            return Err(AlephError::invalid_config("Model name cannot be empty"));
        }

        if config.timeout_seconds == 0 {
            return Err(AlephError::invalid_config(
                "Timeout must be greater than zero",
            ));
        }

        // Build HTTP client with timeout
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|e| AlephError::invalid_config(format!("Failed to build HTTP client: {e}")))?;

        // Build API endpoint
        let base_url = config.base_url.as_ref().map_or_else(
            || DEFAULT_OLLAMA_URL.to_string(),
            |s| s.trim_end_matches('/').to_string(),
        );
        let endpoint = format!("{base_url}/api/chat");

        info!(
            model = %config.default_model(),
            endpoint = %endpoint,
            timeout_seconds = config.timeout_seconds,
            "Ollama provider initialized successfully (chat API)"
        );

        Ok(Self {
            name,
            model: config.default_model().to_string(),
            client,
            endpoint,
            color: config.color.clone(),
            config,
        })
    }

    /// Build generate options from config.
    ///
    /// Forwards `top_p`/`top_k` (previously declared in `ProviderConfig` but
    /// silently dropped here) into the Ollama `options` object.
    ///
    /// Greedy normalization: under greedy decoding (`temperature == 0`) Ollama
    /// emits empty/malformed output when `top_p` is any non-unit value, so we
    /// force `top_p = 1.0` in that case regardless of the configured value
    /// (mirrors openclaw #87049 `normalizeOllamaGreedySamplingOptions`).
    fn build_options(&self) -> Option<GenerateOptions> {
        let c = &self.config;
        if c.temperature.is_none()
            && c.max_tokens.is_none()
            && c.repeat_penalty.is_none()
            && c.top_p.is_none()
            && c.top_k.is_none()
        {
            return None;
        }

        // temperature is non-negative by contract; `<= 0.0` == greedy decoding.
        let greedy = matches!(c.temperature, Some(t) if t <= 0.0);
        let top_p = if greedy { Some(1.0) } else { c.top_p };

        Some(GenerateOptions {
            temperature: c.temperature,
            num_predict: c.max_tokens,
            repeat_penalty: c.repeat_penalty,
            top_p,
            top_k: c.top_k,
        })
    }

    /// Translate unified conversation history into Ollama `/api/chat` messages.
    ///
    /// Each `UnifiedMessage` maps to a role-tagged object so the full multi-turn
    /// context survives the wire:
    /// - `User` → `{role:"user", content, images?}` (base64 image blocks attached)
    /// - `Assistant` → `{role:"assistant", content, tool_calls?}` (arguments stay a
    ///   JSON object — Ollama's native shape, not a stringified blob)
    /// - `ToolResult` → `{role:"tool", tool_name, content}`
    fn convert_messages(messages: &[UnifiedMessage], system_prompt: Option<&str>) -> Vec<Value> {
        let mut out = Vec::with_capacity(messages.len() + 1);

        if let Some(sys) = system_prompt {
            if !sys.is_empty() {
                out.push(json!({ "role": "system", "content": sys }));
            }
        }

        for msg in messages {
            match msg {
                UnifiedMessage::User { content } => {
                    let text = content
                        .iter()
                        .filter_map(|b| b.as_text())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let images: Vec<&str> = content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Image { data, .. } => Some(data.as_str()),
                            _ => None,
                        })
                        .collect();
                    let mut m = json!({ "role": "user", "content": text });
                    if !images.is_empty() {
                        m["images"] = json!(images);
                    }
                    out.push(m);
                }
                UnifiedMessage::Assistant { content } => {
                    let text = content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let tool_calls: Vec<Value> = content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::ToolCall {
                                name, arguments, ..
                            } => Some(json!({
                                "function": { "name": name, "arguments": arguments }
                            })),
                            _ => None,
                        })
                        .collect();
                    let mut m = json!({ "role": "assistant", "content": text });
                    if !tool_calls.is_empty() {
                        m["tool_calls"] = json!(tool_calls);
                    }
                    out.push(m);
                }
                UnifiedMessage::ToolResult {
                    tool_name, content, ..
                } => {
                    let output = content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text, .. } => {
                                Some(std::borrow::Cow::Borrowed(text.as_str()))
                            }
                            ContentBlock::Json { value } => Some(std::borrow::Cow::Owned(
                                serde_json::to_string(value).unwrap_or_default(),
                            )),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    out.push(json!({
                        "role": "tool",
                        "tool_name": tool_name,
                        "content": output,
                    }));
                }
            }
        }

        out
    }

    /// Build the `/api/chat` request body from a payload.
    ///
    /// Returns an owned `Value` so the caller can move it into the async send
    /// block without borrowing the (non-`'static`) payload.
    fn build_chat_body(&self, payload: &RequestPayload) -> Value {
        let model = payload.model.as_deref().unwrap_or(&self.model);
        let messages = Self::convert_messages(payload.messages, payload.system_prompt);

        let mut body = json!({
            "model": model,
            "messages": messages,
            "stream": false,
        });

        if let Some(opts) = self.build_options() {
            if let Ok(v) = serde_json::to_value(opts) {
                body["options"] = v;
            }
        }

        if let Some(tools) = payload.tools {
            if !tools.is_empty() {
                let tool_specs: Vec<Value> = tools
                    .iter()
                    .map(|t| {
                        json!({
                            "type": "function",
                            "function": {
                                "name": t.name,
                                "description": t.description,
                                "parameters": t.parameters,
                            }
                        })
                    })
                    .collect();
                body["tools"] = json!(tool_specs);
            }
        }

        body
    }

    /// Map an Ollama chat response into the unified `ProviderResponse`.
    ///
    /// Ollama omits tool-call ids, so synthesize them (`ollama_call_<n>_<nonce>`)
    /// — the agent loop needs them to pair the eventual `ToolResult` back to its
    /// call.
    ///
    /// The nonce is not decoration. A bare `ollama_call_0` repeats on every
    /// response (the index restarts at 0 each time), and `build_prompt` matches
    /// tool results by call_id *anywhere later in the log*: one user Stop leaves
    /// a tool_use with no result, and the next turn's `ollama_call_0` result
    /// then marks that orphan resolved — replaying an unpaired tool_use the
    /// provider rejects with a 400. Same failure and same fix as
    /// `promoted_{i}_{nonce}` in `harness/agent/think.rs`.
    fn build_provider_response(&self, chat: ChatResponse) -> ProviderResponse {
        let text = chat.message.content.trim().to_string();

        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let tool_calls: Vec<NativeToolCall> = chat
            .message
            .tool_calls
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(i, tc)| NativeToolCall {
                id: format!("ollama_call_{i}_{nonce}"),
                name: tc.function.name,
                arguments: tc.function.arguments,
                thought_signature: None,
            })
            .collect();

        let stop_reason = if !tool_calls.is_empty() {
            StopReason::ToolUse
        } else {
            match chat.done_reason.as_deref() {
                Some("length") => StopReason::MaxTokens,
                _ => StopReason::EndTurn,
            }
        };

        let usage = match (chat.prompt_eval_count, chat.eval_count) {
            (None, None) => None,
            (p, e) => Some(TokenUsage {
                input_tokens: p.unwrap_or(0),
                output_tokens: e.unwrap_or(0),
                ..Default::default()
            }),
        };

        if text.is_empty() && tool_calls.is_empty() {
            warn!(model = %self.model, "Ollama returned an empty response with no tool calls");
        }

        ProviderResponse {
            text: if text.is_empty() { None } else { Some(text) },
            tool_calls,
            stop_reason,
            usage,
            ..Default::default()
        }
    }

    /// Handle error response from Ollama
    async fn handle_error(&self, response: reqwest::Response) -> AlephError {
        let status = response.status();

        // Try to parse error response
        if let Ok(error_response) = response.json::<OllamaError>().await {
            let error_msg = error_response.error;

            // Check for specific error patterns
            if error_msg.contains("model") && error_msg.contains("not found") {
                error!(model = %self.model, "Ollama model not found");
                return AlephError::provider(format!(
                    "Ollama model '{}' not found. Run: ollama pull {}",
                    self.model, self.model
                ));
            }

            error!(status = %status, error = %error_msg, "Ollama API error");
            return AlephError::provider(format!("Ollama error: {error_msg}"));
        }

        // Fallback error
        error!(status = %status, "Ollama request failed");
        AlephError::provider(format!("Ollama request failed: {status}"))
    }
}

impl AiProvider for OllamaProvider {
    fn process(
        &self,
        payload: RequestPayload<'_>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + '_>> {
        // Build the owned request body up front so the async block doesn't borrow
        // the (non-'static) payload.
        let body = self.build_chat_body(&payload);

        Box::pin(async move {
            debug!(
                model = %self.model,
                endpoint = %self.endpoint,
                "Sending chat request to Ollama"
            );

            let response = self
                .client
                .post(&self.endpoint)
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        error!("Ollama request timed out");
                        AlephError::Timeout {
                            suggestion: Some(
                                "The Ollama model is taking too long. Try a smaller model or increase the timeout.".to_string(),
                            ),
                        }
                    } else if e.is_connect() {
                        error!(error = %e, "Failed to connect to Ollama");
                        AlephError::network(format!(
                            "Failed to connect to Ollama at {}. Is Ollama running?",
                            self.endpoint
                        ))
                    } else {
                        error!(error = %e, "Ollama network error");
                        AlephError::network(e.to_string())
                    }
                })?;

            if !response.status().is_success() {
                return Err(self.handle_error(response).await);
            }

            let chat_response: ChatResponse = response.json().await.map_err(|e| {
                error!(error = %e, "Failed to parse Ollama response");
                AlephError::provider(format!("Failed to parse Ollama response: {e}"))
            })?;

            let provider_response = self.build_provider_response(chat_response);

            info!(
                model = %self.model,
                has_tool_calls = provider_response.has_tool_calls(),
                "Ollama request completed successfully"
            );

            Ok(provider_response)
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn color(&self) -> &str {
        &self.color
    }

    fn supports_native_tools(&self) -> bool {
        // `/api/chat` carries `tools` and returns `message.tool_calls`, so the
        // loop should treat Ollama as a native tool-use provider rather than
        // falling back to text-protocol parsing.
        true
    }

    fn protocol(&self) -> Cow<'_, str> {
        Cow::Borrowed("ollama")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> ProviderConfig {
        let mut config = ProviderConfig::test_config("llama3.2");
        config.api_key = None; // Not needed for Ollama
        config.color = "#0000ff".to_string(); // Ollama brand color
        config.timeout_seconds = 60;
        config
    }

    #[test]
    fn test_new_provider_success() {
        let config = create_test_config();
        let provider = OllamaProvider::new("ollama".to_string(), config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_new_provider_empty_model() {
        let mut config = create_test_config();
        config.models = vec!["".to_string()];
        let result = OllamaProvider::new("ollama".to_string(), config);
        assert!(matches!(result, Err(AlephError::InvalidConfig { .. })));
    }

    #[test]
    fn test_new_provider_zero_timeout() {
        let mut config = create_test_config();
        config.timeout_seconds = 0;
        let result = OllamaProvider::new("ollama".to_string(), config);
        assert!(matches!(result, Err(AlephError::InvalidConfig { .. })));
    }

    #[test]
    fn test_default_endpoint() {
        let config = create_test_config();
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();
        assert_eq!(provider.endpoint, "http://localhost:11434/api/chat");
    }

    #[test]
    fn test_custom_endpoint() {
        let mut config = create_test_config();
        config.base_url = Some("http://192.168.1.100:11434".to_string());
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();
        assert_eq!(provider.endpoint, "http://192.168.1.100:11434/api/chat");
    }

    #[test]
    fn test_provider_metadata() {
        let config = create_test_config();
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();

        assert_eq!(provider.name(), "ollama");
        assert_eq!(provider.color(), "#0000ff");
        assert!(provider.supports_native_tools());
        assert_eq!(provider.protocol().as_ref(), "ollama");
    }

    #[test]
    fn build_chat_body_includes_full_history_and_system() {
        let config = create_test_config();
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();
        let msgs = vec![
            UnifiedMessage::user("Hi"),
            UnifiedMessage::assistant("Hello there"),
            UnifiedMessage::user("How are you?"),
        ];
        let payload = RequestPayload::new(&msgs).with_system(Some("Be terse"));
        let body = provider.build_chat_body(&payload);

        assert_eq!(body["model"], "llama3.2");
        assert_eq!(body["stream"], false);
        let arr = body["messages"].as_array().unwrap();
        // system + 3 conversation turns — the full history, not just the last user msg.
        assert_eq!(arr.len(), 4);
        assert_eq!(arr[0]["role"], "system");
        assert_eq!(arr[0]["content"], "Be terse");
        assert_eq!(arr[1]["role"], "user");
        assert_eq!(arr[2]["role"], "assistant");
        assert_eq!(arr[2]["content"], "Hello there");
        assert_eq!(arr[3]["content"], "How are you?");
    }

    #[test]
    fn build_chat_body_honors_model_override() {
        let config = create_test_config();
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();
        let msgs = vec![UnifiedMessage::user("hi")];
        let payload = RequestPayload::new(&msgs).with_model(Some("qwen2.5:32b".to_string()));
        let body = provider.build_chat_body(&payload);
        assert_eq!(body["model"], "qwen2.5:32b");
    }

    #[test]
    fn build_chat_body_serializes_tools() {
        use crate::tool_metadata::{ToolCategory, ToolDefinition};
        let config = create_test_config();
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();
        let tools = vec![ToolDefinition::new(
            "search",
            "Search the web",
            json!({"type": "object", "properties": {"q": {"type": "string"}}}),
            ToolCategory::Builtin,
        )];
        let msgs = vec![UnifiedMessage::user("find rust docs")];
        let payload = RequestPayload::new(&msgs).with_tools(Some(&tools));
        let body = provider.build_chat_body(&payload);

        let t = body["tools"].as_array().unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0]["type"], "function");
        assert_eq!(t[0]["function"]["name"], "search");
        assert_eq!(t[0]["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn build_chat_body_omits_tools_when_absent() {
        let config = create_test_config();
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();
        let msgs = vec![UnifiedMessage::user("hi")];
        let payload = RequestPayload::new(&msgs);
        let body = provider.build_chat_body(&payload);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn convert_messages_attaches_images() {
        let msgs = vec![UnifiedMessage::user_with_content(vec![
            ContentBlock::Text {
                text: "describe this".into(),
                cache_control: None,
            },
            ContentBlock::Image {
                data: "BASE64DATA".into(),
                mime_type: "image/png".into(),
            },
        ])];
        let out = OllamaProvider::convert_messages(&msgs, None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["role"], "user");
        assert_eq!(out[0]["content"], "describe this");
        assert_eq!(out[0]["images"][0], "BASE64DATA");
    }

    #[test]
    fn convert_messages_maps_tool_result_and_assistant_tool_calls() {
        let msgs = vec![
            UnifiedMessage::Assistant {
                content: vec![ContentBlock::ToolCall {
                    id: "ollama_call_0".into(),
                    name: "search".into(),
                    arguments: json!({"q": "rust"}),
                    thought_signature: None,
                }],
            },
            UnifiedMessage::tool_result("ollama_call_0", "search", "found 3 hits", false),
        ];
        let out = OllamaProvider::convert_messages(&msgs, None);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["role"], "assistant");
        assert_eq!(out[0]["tool_calls"][0]["function"]["name"], "search");
        assert_eq!(
            out[0]["tool_calls"][0]["function"]["arguments"]["q"],
            "rust"
        );
        assert_eq!(out[1]["role"], "tool");
        assert_eq!(out[1]["tool_name"], "search");
        assert_eq!(out[1]["content"], "found 3 hits");
    }

    #[test]
    fn test_build_options() {
        let mut config = create_test_config();
        config.temperature = Some(0.8);
        config.max_tokens = Some(1000);
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();

        let options = provider.build_options();
        assert!(options.is_some());
        let opts = options.unwrap();
        assert_eq!(opts.temperature, Some(0.8));
        assert_eq!(opts.num_predict, Some(1000));
    }

    #[test]
    fn build_options_forwards_top_p_and_top_k() {
        let mut config = create_test_config();
        config.temperature = Some(0.7); // non-greedy
        config.top_p = Some(0.9);
        config.top_k = Some(40);
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();

        let opts = provider.build_options().unwrap();
        assert_eq!(
            opts.top_p,
            Some(0.9),
            "configured top_p preserved when non-greedy"
        );
        assert_eq!(opts.top_k, Some(40), "configured top_k forwarded");
    }

    #[test]
    fn build_options_normalizes_top_p_under_greedy_decoding() {
        // temperature == 0 → Ollama needs top_p == 1; a configured 0.9 is overridden.
        let mut config = create_test_config();
        config.temperature = Some(0.0);
        config.top_p = Some(0.9);
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();
        assert_eq!(provider.build_options().unwrap().top_p, Some(1.0));

        // Greedy with no configured top_p → still normalized to 1.
        let mut config2 = create_test_config();
        config2.temperature = Some(0.0);
        let provider2 = OllamaProvider::new("ollama".to_string(), config2).unwrap();
        assert_eq!(provider2.build_options().unwrap().top_p, Some(1.0));
    }

    #[test]
    fn build_options_none_when_no_sampling_params() {
        let config = create_test_config(); // test_config leaves sampling params unset
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();
        assert!(provider.build_options().is_none());
    }

    #[test]
    fn build_options_top_k_alone_triggers_options() {
        let mut config = create_test_config();
        config.top_k = Some(20);
        let provider = OllamaProvider::new("ollama".to_string(), config).unwrap();
        let opts = provider.build_options().unwrap();
        assert_eq!(opts.top_k, Some(20));
        assert!(opts.top_p.is_none());
    }

    #[test]
    fn generate_options_serializes_top_p_top_k_and_omits_none() {
        let opts = GenerateOptions {
            temperature: Some(0.0),
            num_predict: None,
            repeat_penalty: None,
            top_p: Some(1.0),
            top_k: Some(40),
        };
        let json = serde_json::to_value(&opts).unwrap();
        assert_eq!(json["top_p"], 1.0);
        assert_eq!(json["top_k"], 40);
        assert_eq!(json["temperature"], 0.0);
        assert!(json.get("num_predict").is_none(), "None fields omitted");
        assert!(json.get("repeat_penalty").is_none());
    }

    #[test]
    fn parse_chat_response_with_tool_calls() {
        let json = r#"{
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {"function": {"name": "search", "arguments": {"q": "rust"}}}
                ]
            },
            "done_reason": "stop",
            "prompt_eval_count": 10,
            "eval_count": 5
        }"#;
        let chat: ChatResponse = serde_json::from_str(json).unwrap();
        let provider = OllamaProvider::new("ollama".to_string(), create_test_config()).unwrap();
        let resp = provider.build_provider_response(chat);

        assert!(resp.has_tool_calls());
        assert_eq!(resp.tool_calls[0].name, "search");
        assert!(
            resp.tool_calls[0].id.starts_with("ollama_call_0_"),
            "synthetic id must keep its ordered prefix, got {}",
            resp.tool_calls[0].id
        );
        assert_eq!(resp.tool_calls[0].arguments["q"], "rust");
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        let usage = resp.usage.unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
    }

    /// Regression: the tool-call index restarts at 0 on every response, so a
    /// bare `ollama_call_0` collides across turns. One user Stop leaves an
    /// unpaired tool_use whose id the next turn reuses; `build_prompt` then
    /// resolves the orphan with the new turn's result and replays a tool_use
    /// with no matching tool_result — a provider 400.
    #[test]
    fn synthetic_tool_ids_never_collide_across_responses() {
        let json = r#"{
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {"function": {"name": "search", "arguments": {"q": "rust"}}}
                ]
            },
            "done_reason": "stop"
        }"#;
        let provider = OllamaProvider::new("ollama".to_string(), create_test_config()).unwrap();

        let first = provider.build_provider_response(serde_json::from_str(json).unwrap());
        let second = provider.build_provider_response(serde_json::from_str(json).unwrap());

        assert_ne!(
            first.tool_calls[0].id, second.tool_calls[0].id,
            "the same call index in two responses must not reuse an id"
        );
    }

    #[test]
    fn parse_chat_response_text_only() {
        let json =
            r#"{"message": {"role": "assistant", "content": "Hello!"}, "done_reason": "stop"}"#;
        let chat: ChatResponse = serde_json::from_str(json).unwrap();
        let provider = OllamaProvider::new("ollama".to_string(), create_test_config()).unwrap();
        let resp = provider.build_provider_response(chat);

        assert_eq!(resp.text.as_deref(), Some("Hello!"));
        assert!(!resp.has_tool_calls());
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn parse_chat_response_length_maps_to_max_tokens() {
        let json = r#"{"message": {"role": "assistant", "content": "truncated"}, "done_reason": "length"}"#;
        let chat: ChatResponse = serde_json::from_str(json).unwrap();
        let provider = OllamaProvider::new("ollama".to_string(), create_test_config()).unwrap();
        let resp = provider.build_provider_response(chat);
        assert_eq!(resp.stop_reason, StopReason::MaxTokens);
    }
}
