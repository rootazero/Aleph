# Protocol Adapter Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Refactor Aleph's provider architecture from "Vendor-Centric" to "Protocol-Centric" by introducing ProtocolAdapter trait and HttpProvider container.

**Architecture:** Introduce a `ProtocolAdapter` trait that handles protocol-specific logic (request building, response parsing), and a generic `HttpProvider` that uses adapters via composition. This eliminates ~600 lines of redundant wrapper code.

**Tech Stack:** Rust, async-trait, reqwest, serde_json, futures

**Worktree:** `/Volumes/TBU4/Workspace/Aleph/.worktrees/protocol-adapter`

**Design Doc:** `docs/plans/2026-02-03-protocol-adapter-design.md`

---

## Task 1: Define RequestPayload DTO

**Files:**
- Create: `core/src/providers/adapter.rs`
- Modify: `core/src/providers/mod.rs` (add module declaration)

**Step 1: Create adapter.rs with RequestPayload struct**

```rust
// core/src/providers/adapter.rs

//! Protocol adapter abstraction for AI providers
//!
//! This module defines the `ProtocolAdapter` trait and `RequestPayload` DTO
//! that enable protocol-centric provider architecture.

use crate::agents::thinking::ThinkLevel;
use crate::clipboard::ImageData;
use crate::core::MediaAttachment;

/// Unified request payload for protocol adapters
///
/// This DTO (Data Transfer Object) contains all possible inputs for an LLM request.
/// Protocol adapters translate this into provider-specific request formats.
#[derive(Debug, Default)]
pub struct RequestPayload<'a> {
    /// Core text input (user message)
    pub input: &'a str,

    /// System prompt (optional)
    pub system_prompt: Option<&'a str>,

    /// Legacy image format (for process_with_image compatibility)
    pub image: Option<&'a ImageData>,

    /// Multimodal attachments (for process_with_attachments compatibility)
    pub attachments: Option<&'a [MediaAttachment]>,

    /// Thinking/reasoning level configuration
    pub think_level: Option<ThinkLevel>,

    /// Force standard mode for system prompt handling
    pub force_standard_mode: bool,
}

impl<'a> RequestPayload<'a> {
    /// Create a new payload with input text
    pub fn new(input: &'a str) -> Self {
        Self {
            input,
            ..Default::default()
        }
    }

    /// Add system prompt
    pub fn with_system(mut self, prompt: Option<&'a str>) -> Self {
        self.system_prompt = prompt;
        self
    }

    /// Add legacy image
    pub fn with_image(mut self, image: Option<&'a ImageData>) -> Self {
        self.image = image;
        self
    }

    /// Add multimodal attachments
    pub fn with_attachments(mut self, attachments: Option<&'a [MediaAttachment]>) -> Self {
        self.attachments = attachments;
        self
    }

    /// Add thinking level
    pub fn with_think_level(mut self, level: Option<ThinkLevel>) -> Self {
        self.think_level = level;
        self
    }

    /// Set force standard mode
    pub fn with_force_standard_mode(mut self, force: bool) -> Self {
        self.force_standard_mode = force;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_payload_builder() {
        let payload = RequestPayload::new("Hello")
            .with_system(Some("You are helpful"))
            .with_think_level(Some(ThinkLevel::Medium));

        assert_eq!(payload.input, "Hello");
        assert_eq!(payload.system_prompt, Some("You are helpful"));
        assert!(payload.think_level.is_some());
        assert!(!payload.force_standard_mode);
    }

    #[test]
    fn test_payload_default() {
        let payload = RequestPayload::new("Test");
        assert_eq!(payload.input, "Test");
        assert!(payload.system_prompt.is_none());
        assert!(payload.image.is_none());
        assert!(payload.attachments.is_none());
        assert!(payload.think_level.is_none());
    }
}
```

**Step 2: Add module declaration to mod.rs**

In `core/src/providers/mod.rs`, add after line 50 (after `pub mod t8star;`):

```rust
pub mod adapter;
```

And add re-export after line 81:

```rust
pub use adapter::RequestPayload;
```

**Step 3: Run tests to verify**

```bash
cd /Volumes/TBU4/Workspace/Aleph/.worktrees/protocol-adapter
cargo test -p alephcore adapter::tests --no-fail-fast
```

Expected: 2 tests pass

**Step 4: Commit**

```bash
git add core/src/providers/adapter.rs core/src/providers/mod.rs
git commit -m "feat(providers): add RequestPayload DTO for protocol adapters"
```

---

## Task 2: Define ProtocolAdapter Trait

**Files:**
- Modify: `core/src/providers/adapter.rs`

**Step 1: Add ProtocolAdapter trait definition**

Add to `core/src/providers/adapter.rs` after RequestPayload:

```rust
use crate::config::ProviderConfig;
use crate::error::Result;
use async_trait::async_trait;
use futures::stream::BoxStream;

/// Protocol adapter trait for building requests and parsing responses
///
/// Each protocol (OpenAI, Anthropic, Gemini, etc.) implements this trait
/// to handle protocol-specific serialization and deserialization.
#[async_trait]
pub trait ProtocolAdapter: Send + Sync {
    /// Build an HTTP request from the payload
    ///
    /// # Arguments
    /// * `payload` - The unified request payload
    /// * `config` - Provider configuration (API key, model, etc.)
    /// * `is_streaming` - Whether to enable streaming response
    ///
    /// # Returns
    /// A configured reqwest::RequestBuilder ready to send
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder>;

    /// Parse a non-streaming response
    ///
    /// # Arguments
    /// * `response` - The HTTP response from the API
    ///
    /// # Returns
    /// The extracted text content from the response
    async fn parse_response(&self, response: reqwest::Response) -> Result<String>;

    /// Parse a streaming response (SSE)
    ///
    /// # Arguments
    /// * `response` - The HTTP response with chunked body
    ///
    /// # Returns
    /// A stream of text chunks
    async fn parse_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<String>>>;

    /// Get the protocol name for logging
    fn name(&self) -> &'static str;
}
```

**Step 2: Add necessary imports at top of file**

Update imports at top of `adapter.rs`:

```rust
use crate::agents::thinking::ThinkLevel;
use crate::clipboard::ImageData;
use crate::config::ProviderConfig;
use crate::core::MediaAttachment;
use crate::error::Result;
use async_trait::async_trait;
use futures::stream::BoxStream;
```

**Step 3: Run build to verify**

```bash
cargo build -p alephcore 2>&1 | head -20
```

Expected: Build succeeds (warnings OK)

**Step 4: Commit**

```bash
git add core/src/providers/adapter.rs
git commit -m "feat(providers): add ProtocolAdapter trait with streaming support"
```

---

## Task 3: Implement OpenAiProtocol Adapter

**Files:**
- Create: `core/src/providers/protocols/mod.rs`
- Create: `core/src/providers/protocols/openai.rs`
- Modify: `core/src/providers/mod.rs` (add module)

**Step 1: Create protocols module**

```rust
// core/src/providers/protocols/mod.rs

//! Protocol implementations for different AI APIs
//!
//! Each protocol handles the specific request/response format for an API family.

pub mod openai;

pub use openai::OpenAiProtocol;
```

**Step 2: Create OpenAiProtocol implementation**

```rust
// core/src/providers/protocols/openai.rs

//! OpenAI protocol adapter
//!
//! Handles OpenAI-compatible chat completion API format.
//! Used by: OpenAI, DeepSeek, Moonshot, Doubao, vLLM, etc.

use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
use crate::providers::openai::types::{
    ChatCompletionRequest, ChatCompletionResponse, ContentBlock, ImageUrl, Message, MessageContent,
};
use crate::providers::shared::{
    build_document_context, combine_with_document_context, separate_attachments,
    should_use_prepend_mode,
};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use reqwest::Client;
use serde_json::json;
use tracing::{debug, error};

/// OpenAI protocol adapter
///
/// Implements the OpenAI chat completion API format.
pub struct OpenAiProtocol {
    client: Client,
}

impl OpenAiProtocol {
    /// Create a new OpenAI protocol adapter
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Build the API endpoint URL
    fn build_endpoint(config: &ProviderConfig) -> String {
        let raw_base_url = config
            .base_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());

        // Detect API version (v1 or v3)
        let is_v3_api = raw_base_url.contains("/v3") || raw_base_url.contains("/api/v3");

        // Normalize URL
        let base_url = raw_base_url
            .trim_end_matches('/')
            .trim_end_matches("/v3")
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string();

        if is_v3_api {
            format!("{}/v3/chat/completions", base_url)
        } else {
            format!("{}/v1/chat/completions", base_url)
        }
    }

    /// Build messages array from payload
    fn build_messages(payload: &RequestPayload, config: &ProviderConfig) -> Vec<Message> {
        let mut messages = Vec::new();
        let use_prepend_mode = !payload.force_standard_mode && should_use_prepend_mode(config);

        // Check if we have multimodal content
        let has_image = payload.image.is_some();
        let has_image_attachments = payload
            .attachments
            .map(|a| a.iter().any(|att| att.media_type == "image"))
            .unwrap_or(false);

        if has_image || has_image_attachments {
            // Multimodal request
            Self::build_multimodal_messages(payload, config, use_prepend_mode, &mut messages);
        } else {
            // Text-only request
            Self::build_text_messages(payload, use_prepend_mode, &mut messages);
        }

        messages
    }

    /// Build text-only messages
    fn build_text_messages(
        payload: &RequestPayload,
        use_prepend_mode: bool,
        messages: &mut Vec<Message>,
    ) {
        // Handle document attachments by injecting into text
        let input = if let Some(attachments) = payload.attachments {
            let (_, documents) = separate_attachments(attachments);
            if !documents.is_empty() {
                let doc_context = build_document_context(&documents);
                combine_with_document_context(&doc_context, payload.input)
            } else {
                payload.input.to_string()
            }
        } else {
            payload.input.to_string()
        };

        if use_prepend_mode {
            let user_content = if let Some(prompt) = payload.system_prompt {
                format!(
                    "<<< SYSTEM INSTRUCTIONS - YOU MUST FOLLOW EXACTLY >>>\n\n{}\n\n<<< END INSTRUCTIONS >>>\n\n<<< USER INPUT >>>\n{}",
                    prompt, input
                )
            } else {
                input
            };

            messages.push(Message {
                role: "user".to_string(),
                content: MessageContent::Text {
                    content: user_content,
                },
            });
        } else {
            if let Some(prompt) = payload.system_prompt {
                messages.push(Message {
                    role: "system".to_string(),
                    content: MessageContent::Text {
                        content: prompt.to_string(),
                    },
                });
            }

            messages.push(Message {
                role: "user".to_string(),
                content: MessageContent::Text { content: input },
            });
        }
    }

    /// Build multimodal messages with images
    fn build_multimodal_messages(
        payload: &RequestPayload,
        config: &ProviderConfig,
        use_prepend_mode: bool,
        messages: &mut Vec<Message>,
    ) {
        // Add system prompt if not prepending
        if !use_prepend_mode {
            if let Some(prompt) = payload.system_prompt {
                messages.push(Message {
                    role: "system".to_string(),
                    content: MessageContent::Text {
                        content: prompt.to_string(),
                    },
                });
            }
        }

        let mut content_blocks = Vec::new();

        // Build text content
        let mut text_input = payload.input.to_string();

        // Handle document attachments
        if let Some(attachments) = payload.attachments {
            let (_, documents) = separate_attachments(attachments);
            if !documents.is_empty() {
                let doc_context = build_document_context(&documents);
                text_input = combine_with_document_context(&doc_context, &text_input);
            }
        }

        // Prepend system prompt if in prepend mode
        let text_content = if use_prepend_mode {
            if let Some(prompt) = payload.system_prompt {
                format!("{}\n\n{}", prompt, text_input)
            } else {
                text_input
            }
        } else {
            text_input
        };

        // Use default description if empty
        let final_text = if text_content.is_empty() {
            "Describe this image in detail.".to_string()
        } else {
            text_content
        };

        content_blocks.push(ContentBlock::Text { text: final_text });

        // Add legacy image
        if let Some(image) = payload.image {
            content_blocks.push(ContentBlock::ImageUrl {
                image_url: ImageUrl {
                    url: image.to_base64(),
                    detail: Some("auto".to_string()),
                },
            });
        }

        // Add image attachments
        if let Some(attachments) = payload.attachments {
            let (images, _) = separate_attachments(attachments);
            for attachment in images {
                let data_uri = format!("data:{};base64,{}", attachment.mime_type, attachment.data);
                content_blocks.push(ContentBlock::ImageUrl {
                    image_url: ImageUrl {
                        url: data_uri,
                        detail: Some("auto".to_string()),
                    },
                });
            }
        }

        messages.push(Message {
            role: "user".to_string(),
            content: MessageContent::Multimodal {
                content: content_blocks,
            },
        });
    }

    /// Map ThinkLevel to OpenAI reasoning_effort
    fn map_think_level(level: &crate::agents::thinking::ThinkLevel) -> Option<String> {
        use crate::agents::thinking::ThinkLevel;
        match level {
            ThinkLevel::Off | ThinkLevel::Minimal => None,
            ThinkLevel::Low => Some("low".to_string()),
            ThinkLevel::Medium => Some("medium".to_string()),
            ThinkLevel::High | ThinkLevel::XHigh => Some("high".to_string()),
        }
    }

    /// Parse SSE chunk
    fn parse_sse_line(line: &str) -> Option<String> {
        if !line.starts_with("data: ") {
            return None;
        }

        let data = &line[6..];
        if data == "[DONE]" {
            return None;
        }

        // Parse JSON
        let parsed: serde_json::Value = serde_json::from_str(data).ok()?;
        parsed["choices"][0]["delta"]["content"]
            .as_str()
            .map(|s| s.to_string())
    }
}

#[async_trait]
impl ProtocolAdapter for OpenAiProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder> {
        let endpoint = Self::build_endpoint(config);
        let messages = Self::build_messages(payload, config);

        // Build request body
        let mut body = json!({
            "model": &config.model,
            "messages": messages,
            "stream": is_streaming,
        });

        // Add optional parameters
        if let Some(max_tokens) = config.max_tokens {
            body["max_tokens"] = json!(max_tokens);
        }
        if let Some(temp) = config.temperature {
            body["temperature"] = json!(temp);
        }
        if let Some(top_p) = config.top_p {
            body["top_p"] = json!(top_p);
        }
        if let Some(freq) = config.frequency_penalty {
            body["frequency_penalty"] = json!(freq);
        }
        if let Some(pres) = config.presence_penalty {
            body["presence_penalty"] = json!(pres);
        }

        // Add reasoning_effort for thinking models
        if let Some(ref level) = payload.think_level {
            if let Some(effort) = Self::map_think_level(level) {
                body["reasoning_effort"] = json!(effort);
            }
        }

        // Get API key
        let api_key = config
            .api_key
            .as_ref()
            .ok_or_else(|| AlephError::invalid_config("API key is required"))?;

        debug!(
            endpoint = %endpoint,
            model = %config.model,
            streaming = is_streaming,
            "Building OpenAI request"
        );

        Ok(self
            .client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .json(&body))
    }

    async fn parse_response(&self, response: reqwest::Response) -> Result<String> {
        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            error!(status = %status, error = %error_text, "OpenAI API error");
            return Err(AlephError::provider(format!(
                "API error ({}): {}",
                status, error_text
            )));
        }

        let completion: ChatCompletionResponse = response.json().await.map_err(|e| {
            error!(error = %e, "Failed to parse OpenAI response");
            AlephError::provider(format!("Failed to parse response: {}", e))
        })?;

        completion
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .ok_or_else(|| AlephError::provider("No response choices"))
    }

    async fn parse_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<String>>> {
        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!(
                "API error ({}): {}",
                status, error_text
            )));
        }

        let stream = response
            .bytes_stream()
            .map_err(|e| AlephError::network(format!("Stream error: {}", e)))
            .try_filter_map(|chunk| async move {
                let text = std::str::from_utf8(&chunk)
                    .map_err(|e| AlephError::provider(format!("UTF-8 error: {}", e)))?;

                let mut result = String::new();
                for line in text.lines() {
                    if let Some(content) = Self::parse_sse_line(line) {
                        result.push_str(&content);
                    }
                }

                if result.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(result))
                }
            });

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &'static str {
        "openai"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;

    #[test]
    fn test_build_endpoint_default() {
        let config = ProviderConfig::test_config("gpt-4o");
        let endpoint = OpenAiProtocol::build_endpoint(&config);
        assert_eq!(endpoint, "https://api.openai.com/v1/chat/completions");
    }

    #[test]
    fn test_build_endpoint_custom() {
        let mut config = ProviderConfig::test_config("deepseek-chat");
        config.base_url = Some("https://api.deepseek.com".to_string());
        let endpoint = OpenAiProtocol::build_endpoint(&config);
        assert_eq!(endpoint, "https://api.deepseek.com/v1/chat/completions");
    }

    #[test]
    fn test_build_endpoint_v3() {
        let mut config = ProviderConfig::test_config("doubao-pro");
        config.base_url = Some("https://ark.cn-beijing.volces.com/api/v3".to_string());
        let endpoint = OpenAiProtocol::build_endpoint(&config);
        assert_eq!(
            endpoint,
            "https://ark.cn-beijing.volces.com/api/v3/chat/completions"
        );
    }

    #[test]
    fn test_map_think_level() {
        use crate::agents::thinking::ThinkLevel;

        assert!(OpenAiProtocol::map_think_level(&ThinkLevel::Off).is_none());
        assert_eq!(
            OpenAiProtocol::map_think_level(&ThinkLevel::Low),
            Some("low".to_string())
        );
        assert_eq!(
            OpenAiProtocol::map_think_level(&ThinkLevel::Medium),
            Some("medium".to_string())
        );
        assert_eq!(
            OpenAiProtocol::map_think_level(&ThinkLevel::High),
            Some("high".to_string())
        );
    }
}
```

**Step 3: Add module to mod.rs**

In `core/src/providers/mod.rs`, add after `pub mod adapter;`:

```rust
pub mod protocols;
```

And add re-export:

```rust
pub use protocols::OpenAiProtocol;
```

**Step 4: Run tests**

```bash
cargo test -p alephcore protocols::openai::tests --no-fail-fast
```

Expected: 4 tests pass

**Step 5: Commit**

```bash
git add core/src/providers/protocols/ core/src/providers/mod.rs
git commit -m "feat(providers): implement OpenAiProtocol adapter"
```

---

## Task 4: Create HttpProvider Container

**Files:**
- Create: `core/src/providers/http_provider.rs`
- Modify: `core/src/providers/mod.rs`

**Step 1: Create HttpProvider**

```rust
// core/src/providers/http_provider.rs

//! Generic HTTP-based AI provider
//!
//! Uses a ProtocolAdapter for protocol-specific logic.

use crate::agents::thinking::ThinkLevel;
use crate::clipboard::ImageData;
use crate::config::ProviderConfig;
use crate::core::MediaAttachment;
use crate::error::Result;
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
use crate::providers::AiProvider;
use futures::stream::BoxStream;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tracing::debug;

/// Generic HTTP-based AI provider
///
/// This provider uses a ProtocolAdapter for protocol-specific request/response handling.
/// It implements the AiProvider trait by delegating to the adapter.
pub struct HttpProvider {
    name: String,
    config: ProviderConfig,
    adapter: Arc<dyn ProtocolAdapter>,
}

impl std::fmt::Debug for HttpProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpProvider")
            .field("name", &self.name)
            .field("protocol", &self.adapter.name())
            .finish_non_exhaustive()
    }
}

impl HttpProvider {
    /// Create a new HttpProvider with the given adapter
    pub fn new(
        name: String,
        config: ProviderConfig,
        adapter: Arc<dyn ProtocolAdapter>,
    ) -> Result<Self> {
        debug!(
            name = %name,
            protocol = adapter.name(),
            model = %config.model,
            "Creating HttpProvider"
        );

        Ok(Self {
            name,
            config,
            adapter,
        })
    }

    /// Execute a request (non-streaming)
    async fn execute(&self, payload: RequestPayload<'_>) -> Result<String> {
        let request = self.adapter.build_request(&payload, &self.config, false)?;
        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                crate::error::AlephError::Timeout {
                    suggestion: Some("Request timed out. Try again or switch providers.".into()),
                }
            } else {
                crate::error::AlephError::network(format!("Network error: {}", e))
            }
        })?;
        self.adapter.parse_response(response).await
    }

    /// Execute a streaming request
    #[allow(dead_code)]
    async fn execute_stream(
        &self,
        payload: RequestPayload<'_>,
    ) -> Result<BoxStream<'static, Result<String>>> {
        let request = self.adapter.build_request(&payload, &self.config, true)?;
        let response = request.send().await.map_err(|e| {
            crate::error::AlephError::network(format!("Network error: {}", e))
        })?;
        self.adapter.parse_stream(response).await
    }
}

impl AiProvider for HttpProvider {
    fn process(
        &self,
        input: &str,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let input = input.to_string();
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            let payload = RequestPayload::new(&input).with_system(system_prompt.as_deref());
            self.execute(payload).await
        })
    }

    fn process_with_image(
        &self,
        input: &str,
        image: Option<&ImageData>,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let input = input.to_string();
        let image = image.cloned();
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            let payload = RequestPayload::new(&input)
                .with_system(system_prompt.as_deref())
                .with_image(image.as_ref());
            self.execute(payload).await
        })
    }

    fn process_with_attachments(
        &self,
        input: &str,
        attachments: Option<&[MediaAttachment]>,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let input = input.to_string();
        let attachments = attachments.map(|a| a.to_vec());
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            let payload = RequestPayload::new(&input)
                .with_system(system_prompt.as_deref())
                .with_attachments(attachments.as_deref());
            self.execute(payload).await
        })
    }

    fn process_with_mode(
        &self,
        input: &str,
        system_prompt: Option<&str>,
        force_standard_mode: bool,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let input = input.to_string();
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            let payload = RequestPayload::new(&input)
                .with_system(system_prompt.as_deref())
                .with_force_standard_mode(force_standard_mode);
            self.execute(payload).await
        })
    }

    fn process_with_thinking(
        &self,
        input: &str,
        system_prompt: Option<&str>,
        think_level: ThinkLevel,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        let input = input.to_string();
        let system_prompt = system_prompt.map(|s| s.to_string());

        Box::pin(async move {
            let payload = RequestPayload::new(&input)
                .with_system(system_prompt.as_deref())
                .with_think_level(Some(think_level));
            self.execute(payload).await
        })
    }

    fn supports_vision(&self) -> bool {
        true // OpenAI protocol supports vision
    }

    fn supports_thinking(&self) -> bool {
        // Check if model supports thinking
        let model_lower = self.config.model.to_lowercase();
        model_lower.contains("o1") || model_lower.contains("o3") || model_lower.contains("gpt-5")
    }

    fn max_think_level(&self) -> ThinkLevel {
        if self.supports_thinking() {
            ThinkLevel::High
        } else {
            ThinkLevel::Off
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn color(&self) -> &str {
        &self.config.color
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_provider_creation() {
        // This test just verifies the type compiles correctly
        // Actual functionality tested via integration tests
    }
}
```

**Step 2: Add module to mod.rs**

In `core/src/providers/mod.rs`, add:

```rust
pub mod http_provider;
```

And re-export:

```rust
pub use http_provider::HttpProvider;
```

**Step 3: Build and verify**

```bash
cargo build -p alephcore 2>&1 | tail -10
```

Expected: Build succeeds

**Step 4: Commit**

```bash
git add core/src/providers/http_provider.rs core/src/providers/mod.rs
git commit -m "feat(providers): add HttpProvider container with ProtocolAdapter"
```

---

## Task 5: Create Provider Presets Registry

**Files:**
- Create: `core/src/providers/presets.rs`
- Modify: `core/src/providers/mod.rs`

**Step 1: Create presets.rs**

```rust
// core/src/providers/presets.rs

//! Provider presets registry
//!
//! Contains default configurations for known AI providers.

use once_cell::sync::Lazy;
use std::collections::HashMap;

/// Provider preset configuration
#[derive(Debug, Clone)]
pub struct ProviderPreset {
    /// Default base URL for the provider
    pub base_url: &'static str,
    /// Protocol to use (e.g., "openai", "anthropic")
    pub protocol: &'static str,
    /// Default color for UI
    pub color: &'static str,
}

/// Registry of known provider presets
pub static PRESETS: Lazy<HashMap<&'static str, ProviderPreset>> = Lazy::new(|| {
    let mut m = HashMap::new();

    // OpenAI official
    m.insert(
        "openai",
        ProviderPreset {
            base_url: "https://api.openai.com/v1",
            protocol: "openai",
            color: "#10a37f",
        },
    );

    // DeepSeek
    m.insert(
        "deepseek",
        ProviderPreset {
            base_url: "https://api.deepseek.com",
            protocol: "openai",
            color: "#0066cc",
        },
    );

    // Moonshot / Kimi
    m.insert(
        "moonshot",
        ProviderPreset {
            base_url: "https://api.moonshot.cn/v1",
            protocol: "openai",
            color: "#6366f1",
        },
    );
    m.insert(
        "kimi",
        ProviderPreset {
            base_url: "https://api.moonshot.cn/v1",
            protocol: "openai",
            color: "#6366f1",
        },
    );

    // Volcengine Doubao
    m.insert(
        "doubao",
        ProviderPreset {
            base_url: "https://ark.cn-beijing.volces.com/api/v3",
            protocol: "openai",
            color: "#ff6b35",
        },
    );
    m.insert(
        "volcengine",
        ProviderPreset {
            base_url: "https://ark.cn-beijing.volces.com/api/v3",
            protocol: "openai",
            color: "#ff6b35",
        },
    );
    m.insert(
        "ark",
        ProviderPreset {
            base_url: "https://ark.cn-beijing.volces.com/api/v3",
            protocol: "openai",
            color: "#ff6b35",
        },
    );

    // T8Star
    m.insert(
        "t8star",
        ProviderPreset {
            base_url: "https://api.t8star.cn/v1",
            protocol: "openai",
            color: "#f59e0b",
        },
    );

    m
});

/// Get a preset by name (case-insensitive)
pub fn get_preset(name: &str) -> Option<&'static ProviderPreset> {
    PRESETS.get(name.to_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_presets_contain_known_vendors() {
        assert!(PRESETS.contains_key("deepseek"));
        assert!(PRESETS.contains_key("moonshot"));
        assert!(PRESETS.contains_key("doubao"));
        assert!(PRESETS.contains_key("openai"));
    }

    #[test]
    fn test_all_presets_use_openai_protocol() {
        for (name, preset) in PRESETS.iter() {
            assert_eq!(
                preset.protocol, "openai",
                "Preset '{}' should use openai protocol",
                name
            );
        }
    }

    #[test]
    fn test_get_preset_case_insensitive() {
        assert!(get_preset("DeepSeek").is_some());
        assert!(get_preset("MOONSHOT").is_some());
        assert!(get_preset("doubao").is_some());
    }

    #[test]
    fn test_kimi_alias() {
        let moonshot = get_preset("moonshot").unwrap();
        let kimi = get_preset("kimi").unwrap();
        assert_eq!(moonshot.base_url, kimi.base_url);
    }
}
```

**Step 2: Add module to mod.rs**

```rust
pub mod presets;
pub use presets::{get_preset, ProviderPreset, PRESETS};
```

**Step 3: Run tests**

```bash
cargo test -p alephcore presets::tests --no-fail-fast
```

Expected: 4 tests pass

**Step 4: Commit**

```bash
git add core/src/providers/presets.rs core/src/providers/mod.rs
git commit -m "feat(providers): add provider presets registry"
```

---

## Task 6: Add protocol Field to ProviderConfig

**Files:**
- Modify: `core/src/config/types/provider.rs`

**Step 1: Add protocol field**

In `core/src/config/types/provider.rs`, add after `provider_type` field (around line 45):

```rust
    /// Protocol to use: "openai", "anthropic", "gemini", "ollama"
    /// If not specified, inferred from provider_type or provider name
    #[serde(default)]
    pub protocol: Option<String>,
```

**Step 2: Add protocol() method**

Add this method to `impl ProviderConfig` (after `infer_provider_type`):

```rust
    /// Get the effective protocol name
    ///
    /// Priority: protocol > provider_type > default "openai"
    pub fn protocol(&self) -> &str {
        if let Some(ref p) = self.protocol {
            return p.as_str();
        }

        if let Some(ref t) = self.provider_type {
            return match t.to_lowercase().as_str() {
                "claude" => "anthropic",
                other => other,
            };
        }

        "openai"
    }
```

**Step 3: Update test_config to include protocol**

In the `test_config` method, add:

```rust
            protocol: None,
```

**Step 4: Add tests**

Add to the test module (or create one):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_default() {
        let config = ProviderConfig::test_config("gpt-4o");
        assert_eq!(config.protocol(), "openai");
    }

    #[test]
    fn test_protocol_explicit() {
        let mut config = ProviderConfig::test_config("model");
        config.protocol = Some("anthropic".to_string());
        assert_eq!(config.protocol(), "anthropic");
    }

    #[test]
    fn test_protocol_from_provider_type() {
        let mut config = ProviderConfig::test_config("model");
        config.provider_type = Some("claude".to_string());
        assert_eq!(config.protocol(), "anthropic");
    }

    #[test]
    fn test_protocol_precedence() {
        let mut config = ProviderConfig::test_config("model");
        config.protocol = Some("gemini".to_string());
        config.provider_type = Some("openai".to_string());
        // protocol takes precedence
        assert_eq!(config.protocol(), "gemini");
    }
}
```

**Step 5: Run tests**

```bash
cargo test -p alephcore config::types::provider --no-fail-fast
```

Expected: 4 tests pass

**Step 6: Commit**

```bash
git add core/src/config/types/provider.rs
git commit -m "feat(config): add protocol field to ProviderConfig"
```

---

## Task 7: Refactor create_provider Factory

**Files:**
- Modify: `core/src/providers/mod.rs`

**Step 1: Update create_provider function**

Replace the `create_provider` function (lines 152-214) with:

```rust
/// Create a provider instance from configuration
///
/// This factory function instantiates the appropriate provider based on
/// the protocol and preset configuration.
///
/// # Provider Resolution Order
///
/// 1. Check for preset providers by name (deepseek, moonshot, etc.)
/// 2. Apply preset defaults (base_url, protocol)
/// 3. Route to appropriate provider based on protocol
///
/// # Supported Protocols
///
/// - `"openai"` - OpenAI and OpenAI-compatible APIs (via HttpProvider)
/// - `"claude"` / `"anthropic"` - Anthropic Claude API (native)
/// - `"gemini"` - Google Gemini API (native)
/// - `"ollama"` - Local Ollama models (native)
pub fn create_provider(name: &str, mut config: ProviderConfig) -> Result<Arc<dyn AiProvider>> {
    let name_lower = name.to_lowercase();

    // 1. Apply preset configuration if available
    if let Some(preset) = presets::get_preset(&name_lower) {
        // Set base_url if not provided
        if config.base_url.is_none() || config.base_url.as_ref().map(|s| s.is_empty()).unwrap_or(false) {
            config.base_url = Some(preset.base_url.to_string());
        }
        // Set protocol if not provided
        if config.protocol.is_none() && config.provider_type.is_none() {
            config.protocol = Some(preset.protocol.to_string());
        }
        // Set color if default
        if config.color == "#808080" {
            config.color = preset.color.to_string();
        }
    }

    // 2. Determine protocol
    let protocol = config.protocol();

    // 3. Route based on protocol
    match protocol {
        "openai" => {
            // Use new HttpProvider + OpenAiProtocol
            use std::time::Duration;

            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(config.timeout_seconds))
                .build()
                .map_err(|e| AlephError::invalid_config(format!("Failed to build HTTP client: {}", e)))?;

            let adapter = Arc::new(protocols::OpenAiProtocol::new(client));
            let provider = HttpProvider::new(name.to_string(), config, adapter)?;
            Ok(Arc::new(provider))
        }

        // Native providers (Phase 1: keep as-is)
        "claude" | "anthropic" => {
            let provider = ClaudeProvider::new(name.to_string(), config)?;
            Ok(Arc::new(provider))
        }
        "gemini" => {
            let provider = GeminiProvider::new(name.to_string(), config)?;
            Ok(Arc::new(provider))
        }
        "ollama" => {
            let provider = OllamaProvider::new(name.to_string(), config)?;
            Ok(Arc::new(provider))
        }
        "mock" => {
            let provider = MockProvider::new("Mock response".to_string());
            Ok(Arc::new(provider))
        }

        unknown => Err(AlephError::invalid_config(format!(
            "Unknown protocol: '{}'. Supported: openai, claude, anthropic, gemini, ollama, mock.",
            unknown
        ))),
    }
}
```

**Step 2: Update imports at top of mod.rs**

Ensure these are imported:

```rust
use std::sync::Arc;
```

**Step 3: Run existing tests**

```bash
cargo test -p alephcore providers::tests --no-fail-fast
```

Expected: All existing factory tests pass

**Step 4: Commit**

```bash
git add core/src/providers/mod.rs
git commit -m "refactor(providers): use HttpProvider for OpenAI protocol in factory"
```

---

## Task 8: Delete Redundant Provider Files

**Files:**
- Delete: `core/src/providers/deepseek.rs`
- Delete: `core/src/providers/moonshot.rs`
- Delete: `core/src/providers/doubao.rs`
- Delete: `core/src/providers/t8star.rs`
- Delete: `core/src/providers/openai_compatible.rs`
- Modify: `core/src/providers/mod.rs` (remove module declarations)

**Step 1: Remove module declarations from mod.rs**

Remove these lines from `core/src/providers/mod.rs`:

```rust
// Remove these module declarations:
pub mod deepseek;
pub mod doubao;
pub mod moonshot;
pub mod openai_compatible;
pub mod t8star;

// Remove these re-exports:
pub use deepseek::DeepSeekProvider;
pub use doubao::DoubaoProvider;
pub use moonshot::MoonshotProvider;
pub use openai_compatible::OpenAiCompatibleProvider;
pub use t8star::T8StarProvider;
```

**Step 2: Delete the files**

```bash
cd /Volumes/TBU4/Workspace/Aleph/.worktrees/protocol-adapter
rm core/src/providers/deepseek.rs
rm core/src/providers/moonshot.rs
rm core/src/providers/doubao.rs
rm core/src/providers/t8star.rs
rm core/src/providers/openai_compatible.rs
```

**Step 3: Build and verify**

```bash
cargo build -p alephcore 2>&1 | tail -20
```

Expected: Build succeeds

**Step 4: Run all tests**

```bash
cargo test -p alephcore --no-fail-fast 2>&1 | tail -30
```

Expected: Tests pass (same baseline failures as before)

**Step 5: Commit**

```bash
git add -A
git commit -m "refactor(providers): remove redundant vendor wrappers (~600 lines)

Deleted:
- deepseek.rs
- moonshot.rs
- doubao.rs
- t8star.rs
- openai_compatible.rs

These are now handled by HttpProvider + OpenAiProtocol + presets."
```

---

## Task 9: Final Verification and Documentation

**Step 1: Run full test suite**

```bash
cargo test -p alephcore 2>&1 | grep -E "(passed|failed|FAILED)"
```

Expected: Same number of passes as baseline, no new failures

**Step 2: Verify code reduction**

```bash
# Count lines removed
git diff --stat main
```

Expected: Net reduction of ~500+ lines

**Step 3: Update mod.rs documentation**

Update the module doc comment at top of `core/src/providers/mod.rs` to reflect new architecture.

**Step 4: Final commit**

```bash
git add -A
git commit -m "docs(providers): update module documentation for Protocol Adapter architecture"
```

---

## Summary

After completing all tasks:

- **New files created:** 4 (adapter.rs, http_provider.rs, presets.rs, protocols/openai.rs)
- **Files deleted:** 5 (~600 lines removed)
- **Net change:** Significant code reduction with improved architecture

**Success Criteria:**
- [ ] All existing tests pass
- [ ] DeepSeek/Moonshot/Doubao/T8Star work via presets
- [ ] New providers can be added by editing presets.rs
- [ ] Configuration backward compatible
