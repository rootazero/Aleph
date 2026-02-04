# Protocol Adapter Phase 2 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Migrate ClaudeProvider and GeminiProvider to ProtocolAdapter architecture, achieving unified cloud API design.

**Architecture:** Create AnthropicProtocol and GeminiProtocol adapters implementing the existing ProtocolAdapter trait, update factory routing, then delete legacy provider files.

**Tech Stack:** Rust, tokio, reqwest, serde, async-trait, futures, base64

---

## Task 1: Add Anthropic Types Module

**Files:**
- Create: `core/src/providers/anthropic/mod.rs`
- Create: `core/src/providers/anthropic/types.rs`
- Modify: `core/src/providers/mod.rs`

**Step 1: Create anthropic module directory structure**

```bash
mkdir -p core/src/providers/anthropic
```

**Step 2: Create types.rs with request/response structures**

```rust
// core/src/providers/anthropic/types.rs

//! Anthropic API types
//!
//! Request and response structures for Claude Messages API.

use serde::{Deserialize, Serialize};

/// Request body for Claude Messages API
#[derive(Debug, Serialize)]
pub struct MessagesRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingBlock>,
}

/// Extended thinking configuration
#[derive(Debug, Serialize)]
pub struct ThinkingBlock {
    #[serde(rename = "type")]
    pub thinking_type: String,
    pub budget_tokens: u32,
}

/// Message structure
#[derive(Debug, Serialize)]
pub struct Message {
    pub role: String,
    #[serde(flatten)]
    pub content: MessageContent,
}

/// Message content variants
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Simple text message
    Text { content: String },
    /// Multimodal message with content blocks
    Multimodal { content: Vec<ContentBlock> },
}

/// Content block for multimodal messages
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum ContentBlock {
    /// Text content
    Text { text: String },
    /// Image content (base64)
    Image { source: ImageSource },
}

/// Image source for base64 encoded images
#[derive(Debug, Serialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    pub data: String,
}

/// Response from Messages API
#[derive(Debug, Deserialize)]
pub struct MessagesResponse {
    pub content: Vec<ResponseContent>,
    #[serde(default)]
    pub stop_reason: Option<String>,
}

/// Response content block
#[derive(Debug, Deserialize)]
pub struct ResponseContent {
    #[serde(rename = "type")]
    pub content_type: String,
    #[serde(default)]
    pub text: String,
}

/// Error response
#[derive(Debug, Deserialize)]
pub struct ErrorResponse {
    pub error: ErrorDetails,
}

#[derive(Debug, Deserialize)]
pub struct ErrorDetails {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
}
```

**Step 3: Create mod.rs for anthropic module**

```rust
// core/src/providers/anthropic/mod.rs

//! Anthropic Claude API types and protocol adapter

pub mod types;

pub use types::*;
```

**Step 4: Add module to providers/mod.rs**

Add after `pub mod openai;`:

```rust
pub mod anthropic;
```

**Step 5: Run cargo check**

```bash
cd /Volumes/TBU4/Workspace/Aether/.worktrees/protocol-adapter-phase2
cargo check -p aethecore 2>&1 | tail -10
```

**Step 6: Commit**

```bash
git add core/src/providers/anthropic/
git add core/src/providers/mod.rs
git commit -m "feat(providers): add Anthropic API types module

Add request/response types for Claude Messages API.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 2: Implement AnthropicProtocol Adapter

**Files:**
- Create: `core/src/providers/protocols/anthropic.rs`
- Modify: `core/src/providers/protocols/mod.rs`

**Step 1: Create anthropic.rs protocol implementation**

```rust
// core/src/providers/protocols/anthropic.rs

//! Anthropic protocol adapter
//!
//! Handles Claude Messages API format.

use crate::agents::thinking::ThinkLevel;
use crate::config::ProviderConfig;
use crate::dispatcher::DEFAULT_MAX_TOKENS;
use crate::error::{AetherError, Result};
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
use crate::providers::anthropic::{
    ContentBlock, ErrorResponse, ImageSource, Message, MessageContent, MessagesRequest,
    MessagesResponse, ThinkingBlock,
};
use crate::providers::shared::{
    build_document_context, combine_with_document_context, separate_attachments,
};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use reqwest::Client;
use tracing::{debug, error};

/// Anthropic API version header value
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Anthropic protocol adapter
pub struct AnthropicProtocol {
    client: Client,
}

impl AnthropicProtocol {
    /// Create a new Anthropic protocol adapter
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Build the endpoint URL
    fn build_endpoint(config: &ProviderConfig) -> String {
        let raw_base_url = config
            .base_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "https://api.anthropic.com".to_string());

        // Normalize URL
        let base_url = raw_base_url
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .trim_end_matches('/')
            .to_string();

        format!("{}/v1/messages", base_url)
    }

    /// Build messages from payload
    fn build_messages(payload: &RequestPayload, config: &ProviderConfig) -> Vec<Message> {
        // Check for image content
        let has_image = payload.image.is_some();
        let has_image_attachments = payload
            .attachments
            .map(|a| a.iter().any(|att| att.media_type == "image"))
            .unwrap_or(false);

        if has_image || has_image_attachments {
            Self::build_multimodal_messages(payload)
        } else {
            Self::build_text_messages(payload)
        }
    }

    /// Build text-only messages
    fn build_text_messages(payload: &RequestPayload) -> Vec<Message> {
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

        vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text { content: input },
        }]
    }

    /// Build multimodal messages with images
    fn build_multimodal_messages(payload: &RequestPayload) -> Vec<Message> {
        let mut content_blocks = Vec::new();

        // Handle document attachments
        let text_input = if let Some(attachments) = payload.attachments {
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

        // Add text content
        let text = if text_input.is_empty() {
            "Describe this image in detail.".to_string()
        } else {
            text_input
        };
        content_blocks.push(ContentBlock::Text { text });

        // Add legacy image
        if let Some(image) = payload.image {
            let media_type = match image.format {
                crate::clipboard::ImageFormat::Png => "image/png",
                crate::clipboard::ImageFormat::Jpeg => "image/jpeg",
                crate::clipboard::ImageFormat::Gif => "image/gif",
            };
            let base64_data = {
                use base64::{engine::general_purpose, Engine as _};
                general_purpose::STANDARD.encode(&image.data)
            };
            content_blocks.push(ContentBlock::Image {
                source: ImageSource {
                    source_type: "base64".to_string(),
                    media_type: media_type.to_string(),
                    data: base64_data,
                },
            });
        }

        // Add image attachments
        if let Some(attachments) = payload.attachments {
            let (images, _) = separate_attachments(attachments);
            for attachment in images {
                content_blocks.push(ContentBlock::Image {
                    source: ImageSource {
                        source_type: "base64".to_string(),
                        media_type: attachment.mime_type.clone(),
                        data: attachment.data.clone(),
                    },
                });
            }
        }

        vec![Message {
            role: "user".to_string(),
            content: MessageContent::Multimodal {
                content: content_blocks,
            },
        }]
    }

    /// Map ThinkLevel to budget_tokens
    fn map_think_level(level: &ThinkLevel) -> Option<u32> {
        match level {
            ThinkLevel::Off => None,
            ThinkLevel::Minimal => Some(1024),
            ThinkLevel::Low => Some(4096),
            ThinkLevel::Medium => Some(10000),
            ThinkLevel::High => Some(20000),
            ThinkLevel::XHigh => Some(50000),
        }
    }

    /// Parse SSE line for streaming
    fn parse_sse_line(line: &str) -> Option<String> {
        if !line.starts_with("data: ") {
            return None;
        }

        let data = &line[6..];
        if data == "[DONE]" {
            return None;
        }

        let parsed: serde_json::Value = serde_json::from_str(data).ok()?;

        // Handle content_block_delta events
        if parsed.get("type").and_then(|t| t.as_str()) == Some("content_block_delta") {
            return parsed["delta"]["text"].as_str().map(|s| s.to_string());
        }

        None
    }
}

#[async_trait]
impl ProtocolAdapter for AnthropicProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder> {
        let endpoint = Self::build_endpoint(config);
        let messages = Self::build_messages(payload, config);

        let max_tokens = config.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);

        // Build thinking config if enabled
        let thinking = payload
            .think_level
            .as_ref()
            .and_then(Self::map_think_level)
            .map(|budget| ThinkingBlock {
                thinking_type: "enabled".to_string(),
                budget_tokens: budget,
            });

        let request_body = MessagesRequest {
            model: config.model.clone(),
            messages,
            max_tokens,
            system: payload.system_prompt.map(|s| s.to_string()),
            temperature: config.temperature,
            stream: if is_streaming { Some(true) } else { None },
            thinking,
        };

        let api_key = config
            .api_key
            .as_ref()
            .ok_or_else(|| AetherError::invalid_config("API key is required"))?;

        debug!(
            endpoint = %endpoint,
            model = %config.model,
            streaming = is_streaming,
            "Building Anthropic request"
        );

        Ok(self
            .client
            .post(&endpoint)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("Content-Type", "application/json")
            .json(&request_body))
    }

    async fn parse_response(&self, response: reqwest::Response) -> Result<String> {
        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();

            if let Ok(error_response) = serde_json::from_str::<ErrorResponse>(&error_text) {
                let msg = error_response.error.message;
                return match status.as_u16() {
                    401 => Err(AetherError::authentication("Anthropic", &msg)),
                    429 => Err(AetherError::rate_limit(format!("Anthropic: {}", msg))),
                    _ => Err(AetherError::provider(format!("Anthropic error: {}", msg))),
                };
            }

            return Err(AetherError::provider(format!(
                "Anthropic error ({}): {}",
                status, error_text
            )));
        }

        let response_body: MessagesResponse = response.json().await.map_err(|e| {
            error!(error = %e, "Failed to parse Anthropic response");
            AetherError::provider(format!("Failed to parse response: {}", e))
        })?;

        // Extract text from content blocks
        let text = response_body
            .content
            .iter()
            .filter(|c| c.content_type == "text")
            .map(|c| c.text.clone())
            .collect::<Vec<_>>()
            .join("");

        Ok(text)
    }

    async fn parse_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<String>>> {
        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AetherError::provider(format!(
                "Anthropic streaming error ({}): {}",
                status, error_text
            )));
        }

        let stream = response
            .bytes_stream()
            .map(move |chunk| {
                let bytes = chunk.map_err(|e| AetherError::network(e.to_string()))?;
                let text = String::from_utf8_lossy(&bytes);

                let mut result = String::new();
                for line in text.lines() {
                    if let Some(content) = Self::parse_sse_line(line) {
                        result.push_str(&content);
                    }
                }

                Ok(result)
            })
            .filter(|r| {
                let keep = match r {
                    Ok(s) => !s.is_empty(),
                    Err(_) => true,
                };
                std::future::ready(keep)
            })
            .boxed();

        Ok(stream)
    }

    fn name(&self) -> &'static str {
        "anthropic"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_endpoint_default() {
        let config = ProviderConfig::test_config("claude-3-5-sonnet");
        let endpoint = AnthropicProtocol::build_endpoint(&config);
        assert_eq!(endpoint, "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn test_build_endpoint_custom() {
        let mut config = ProviderConfig::test_config("claude-3-5-sonnet");
        config.base_url = Some("https://custom.api.com/v1".to_string());
        let endpoint = AnthropicProtocol::build_endpoint(&config);
        assert_eq!(endpoint, "https://custom.api.com/v1/messages");
    }

    #[test]
    fn test_map_think_level() {
        assert_eq!(AnthropicProtocol::map_think_level(&ThinkLevel::Off), None);
        assert_eq!(
            AnthropicProtocol::map_think_level(&ThinkLevel::Medium),
            Some(10000)
        );
        assert_eq!(
            AnthropicProtocol::map_think_level(&ThinkLevel::High),
            Some(20000)
        );
    }

    #[test]
    fn test_parse_sse_content_block_delta() {
        let line = r#"data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello"}}"#;
        let result = AnthropicProtocol::parse_sse_line(line);
        assert_eq!(result, Some("Hello".to_string()));
    }

    #[test]
    fn test_parse_sse_done() {
        let line = "data: [DONE]";
        let result = AnthropicProtocol::parse_sse_line(line);
        assert_eq!(result, None);
    }
}
```

**Step 2: Update protocols/mod.rs**

```rust
// core/src/providers/protocols/mod.rs

//! Protocol implementations for different AI APIs
//!
//! Each protocol handles the specific request/response format for an API family.

pub mod openai;
pub mod anthropic;

pub use openai::OpenAiProtocol;
pub use anthropic::AnthropicProtocol;
```

**Step 3: Run tests**

```bash
cd /Volumes/TBU4/Workspace/Aether/.worktrees/protocol-adapter-phase2
cargo test -p aethecore protocols::anthropic --no-fail-fast
```

**Step 4: Commit**

```bash
git add core/src/providers/protocols/
git commit -m "feat(providers): implement AnthropicProtocol adapter

Implements ProtocolAdapter trait for Claude Messages API:
- Request building with x-api-key header
- Extended thinking support (budget_tokens)
- Multimodal content handling
- SSE streaming response parsing

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 3: Add Claude/Anthropic Presets

**Files:**
- Modify: `core/src/providers/presets.rs`

**Step 1: Add Claude and Anthropic presets**

Add after the t8star entry:

```rust
    // Anthropic Claude
    m.insert(
        "claude",
        ProviderPreset {
            base_url: "https://api.anthropic.com",
            protocol: "anthropic",
            color: "#d97757",
        },
    );
    m.insert(
        "anthropic",
        ProviderPreset {
            base_url: "https://api.anthropic.com",
            protocol: "anthropic",
            color: "#d97757",
        },
    );
```

**Step 2: Update test to not require all presets use openai protocol**

Replace `test_all_presets_use_openai_protocol` with:

```rust
    #[test]
    fn test_presets_have_valid_protocol() {
        let valid_protocols = ["openai", "anthropic", "gemini"];
        for (name, preset) in PRESETS.iter() {
            assert!(
                valid_protocols.contains(&preset.protocol),
                "Preset '{}' uses invalid protocol '{}'",
                name,
                preset.protocol
            );
        }
    }
```

**Step 3: Add test for claude preset**

```rust
    #[test]
    fn test_claude_preset() {
        let claude = get_preset("claude").unwrap();
        assert_eq!(claude.protocol, "anthropic");
        assert_eq!(claude.base_url, "https://api.anthropic.com");
    }
```

**Step 4: Run tests**

```bash
cargo test -p aethecore presets --no-fail-fast
```

**Step 5: Commit**

```bash
git add core/src/providers/presets.rs
git commit -m "feat(providers): add Claude/Anthropic presets

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 4: Update Factory for Anthropic Protocol

**Files:**
- Modify: `core/src/providers/mod.rs`

**Step 1: Update factory to use AnthropicProtocol**

Replace the Claude/Anthropic case (lines 159-163):

```rust
        "claude" | "anthropic" => {
            // Use HttpProvider + AnthropicProtocol
            use std::time::Duration;

            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(config.timeout_seconds))
                .build()
                .map_err(|e| AetherError::invalid_config(format!("Failed to build HTTP client: {}", e)))?;

            let adapter = Arc::new(protocols::AnthropicProtocol::new(client));
            let provider = HttpProvider::new(name.to_string(), config, adapter)?;
            Ok(Arc::new(provider))
        }
```

**Step 2: Run existing tests**

```bash
cargo test -p aethecore providers::tests --no-fail-fast
```

**Step 3: Commit**

```bash
git add core/src/providers/mod.rs
git commit -m "refactor(providers): use HttpProvider for Anthropic protocol

Factory now routes claude/anthropic to HttpProvider + AnthropicProtocol.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 5: Delete ClaudeProvider

**Files:**
- Delete: `core/src/providers/claude.rs`
- Modify: `core/src/providers/mod.rs`

**Step 1: Remove module declaration and re-export**

In `mod.rs`, remove:
```rust
pub mod claude;
```

And remove:
```rust
pub use claude::ClaudeProvider;
```

**Step 2: Delete the file**

```bash
rm core/src/providers/claude.rs
```

**Step 3: Build and verify**

```bash
cargo build -p aethecore 2>&1 | tail -10
```

**Step 4: Run tests**

```bash
cargo test -p aethecore --no-fail-fast 2>&1 | grep -E "(passed|failed)"
```

**Step 5: Commit**

```bash
git add -A
git commit -m "refactor(providers): remove legacy ClaudeProvider (~1040 lines)

ClaudeProvider is now replaced by HttpProvider + AnthropicProtocol.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 6: Add Gemini Types Module

**Files:**
- Create: `core/src/providers/gemini/mod.rs`
- Create: `core/src/providers/gemini/types.rs`
- Modify: `core/src/providers/mod.rs`

**Step 1: Create gemini module directory**

```bash
mkdir -p core/src/providers/gemini
```

**Step 2: Create types.rs**

```rust
// core/src/providers/gemini/types.rs

//! Gemini API types
//!
//! Request and response structures for Google Gemini generateContent API.

use serde::{Deserialize, Serialize};

/// Request body for generateContent API
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContentRequest {
    pub contents: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GenerationConfig>,
}

/// Content structure
#[derive(Debug, Serialize)]
pub struct Content {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub parts: Vec<Part>,
}

/// Part can be text or inline image data
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Part {
    /// Text content
    Text { text: String },
    /// Inline image data
    InlineData { inline_data: InlineData },
}

/// Inline data for images
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineData {
    pub mime_type: String,
    pub data: String,
}

/// Generation configuration
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
}

/// Response from generateContent API
#[derive(Debug, Deserialize)]
pub struct GenerateContentResponse {
    pub candidates: Option<Vec<Candidate>>,
    pub error: Option<GeminiError>,
}

#[derive(Debug, Deserialize)]
pub struct Candidate {
    pub content: CandidateContent,
}

#[derive(Debug, Deserialize)]
pub struct CandidateContent {
    pub parts: Vec<ResponsePart>,
}

#[derive(Debug, Deserialize)]
pub struct ResponsePart {
    pub text: String,
}

/// Error response
#[derive(Debug, Deserialize)]
pub struct GeminiError {
    pub code: i32,
    pub message: String,
    pub status: String,
}
```

**Step 3: Create mod.rs**

```rust
// core/src/providers/gemini/mod.rs

//! Google Gemini API types and protocol adapter

pub mod types;

pub use types::*;
```

**Step 4: Rename existing gemini.rs temporarily**

The current `gemini.rs` will be deleted after we confirm the new implementation works.

```bash
mv core/src/providers/gemini.rs core/src/providers/gemini_legacy.rs
```

**Step 5: Update mod.rs**

Replace `pub mod gemini;` with:
```rust
pub mod gemini;
mod gemini_legacy;
pub use gemini_legacy::GeminiProvider;
```

**Step 6: Run cargo check**

```bash
cargo check -p aethecore 2>&1 | tail -10
```

**Step 7: Commit**

```bash
git add core/src/providers/gemini/
git add core/src/providers/gemini_legacy.rs
git add core/src/providers/mod.rs
git commit -m "feat(providers): add Gemini API types module

Add request/response types for Gemini generateContent API.
Temporarily rename legacy gemini.rs for migration.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 7: Implement GeminiProtocol Adapter

**Files:**
- Create: `core/src/providers/protocols/gemini.rs`
- Modify: `core/src/providers/protocols/mod.rs`

**Step 1: Create gemini.rs protocol implementation**

```rust
// core/src/providers/protocols/gemini.rs

//! Gemini protocol adapter
//!
//! Handles Google Gemini generateContent API format.

use crate::config::ProviderConfig;
use crate::error::{AetherError, Result};
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
use crate::providers::gemini::{
    Content, GenerateContentRequest, GenerateContentResponse, GenerationConfig, InlineData, Part,
};
use crate::providers::shared::{
    build_document_context, combine_with_document_context, separate_attachments,
};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use reqwest::Client;
use tracing::{debug, error};

/// Gemini protocol adapter
pub struct GeminiProtocol {
    client: Client,
}

impl GeminiProtocol {
    /// Create a new Gemini protocol adapter
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Build the endpoint URL
    /// Gemini uses: {base_url}/v1beta/models/{model}:generateContent?key={api_key}
    /// For streaming: :streamGenerateContent?alt=sse&key={api_key}
    fn build_endpoint(config: &ProviderConfig, is_streaming: bool) -> Result<String> {
        let base_url = config
            .base_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| "https://generativelanguage.googleapis.com".to_string());

        let api_key = config
            .api_key
            .as_ref()
            .ok_or_else(|| AetherError::invalid_config("Gemini API key is required"))?;

        let action = if is_streaming {
            "streamGenerateContent"
        } else {
            "generateContent"
        };

        let query = if is_streaming {
            format!("alt=sse&key={}", api_key)
        } else {
            format!("key={}", api_key)
        };

        Ok(format!(
            "{}/v1beta/models/{}:{}?{}",
            base_url, config.model, action, query
        ))
    }

    /// Build contents from payload
    fn build_contents(payload: &RequestPayload) -> Vec<Content> {
        // Check for image content
        let has_image = payload.image.is_some();
        let has_image_attachments = payload
            .attachments
            .map(|a| a.iter().any(|att| att.media_type == "image"))
            .unwrap_or(false);

        if has_image || has_image_attachments {
            Self::build_multimodal_contents(payload)
        } else {
            Self::build_text_contents(payload)
        }
    }

    /// Build text-only contents
    fn build_text_contents(payload: &RequestPayload) -> Vec<Content> {
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

        vec![Content {
            role: Some("user".to_string()),
            parts: vec![Part::Text { text: input }],
        }]
    }

    /// Build multimodal contents with images
    fn build_multimodal_contents(payload: &RequestPayload) -> Vec<Content> {
        let mut parts = Vec::new();

        // Handle document attachments
        let text_input = if let Some(attachments) = payload.attachments {
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

        // Add text content
        let text = if text_input.is_empty() {
            "Describe this image in detail.".to_string()
        } else {
            text_input
        };
        parts.push(Part::Text { text });

        // Add legacy image
        if let Some(image) = payload.image {
            let mime_type = match image.format {
                crate::clipboard::ImageFormat::Png => "image/png",
                crate::clipboard::ImageFormat::Jpeg => "image/jpeg",
                crate::clipboard::ImageFormat::Gif => "image/gif",
            };
            let data = {
                use base64::{engine::general_purpose, Engine as _};
                general_purpose::STANDARD.encode(&image.data)
            };
            parts.push(Part::InlineData {
                inline_data: InlineData {
                    mime_type: mime_type.to_string(),
                    data,
                },
            });
        }

        // Add image attachments
        if let Some(attachments) = payload.attachments {
            let (images, _) = separate_attachments(attachments);
            for attachment in images {
                parts.push(Part::InlineData {
                    inline_data: InlineData {
                        mime_type: attachment.mime_type.clone(),
                        data: attachment.data.clone(),
                    },
                });
            }
        }

        vec![Content {
            role: Some("user".to_string()),
            parts,
        }]
    }

    /// Parse SSE line for streaming
    fn parse_sse_line(line: &str) -> Option<String> {
        if !line.starts_with("data: ") {
            return None;
        }

        let data = &line[6..];
        let parsed: serde_json::Value = serde_json::from_str(data).ok()?;

        // Extract text from candidates[0].content.parts[0].text
        parsed["candidates"]
            .get(0)?
            .get("content")?
            .get("parts")?
            .get(0)?
            .get("text")?
            .as_str()
            .map(|s| s.to_string())
    }
}

#[async_trait]
impl ProtocolAdapter for GeminiProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder> {
        let endpoint = Self::build_endpoint(config, is_streaming)?;
        let contents = Self::build_contents(payload);

        // Build system instruction if provided
        let system_instruction = payload.system_prompt.map(|s| Content {
            role: None,
            parts: vec![Part::Text {
                text: s.to_string(),
            }],
        });

        // Build generation config
        let generation_config = Some(GenerationConfig {
            max_output_tokens: config.max_tokens,
            temperature: config.temperature,
            top_p: config.top_p,
            top_k: None,
        });

        let request_body = GenerateContentRequest {
            contents,
            system_instruction,
            generation_config,
        };

        debug!(
            endpoint = %endpoint,
            model = %config.model,
            streaming = is_streaming,
            "Building Gemini request"
        );

        Ok(self
            .client
            .post(&endpoint)
            .header("Content-Type", "application/json")
            .json(&request_body))
    }

    async fn parse_response(&self, response: reqwest::Response) -> Result<String> {
        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();

            // Try to parse structured error
            if let Ok(parsed) = serde_json::from_str::<GenerateContentResponse>(&error_text) {
                if let Some(err) = parsed.error {
                    return Err(AetherError::provider(format!(
                        "Gemini error ({}): {}",
                        err.code, err.message
                    )));
                }
            }

            return Err(AetherError::provider(format!(
                "Gemini error ({}): {}",
                status, error_text
            )));
        }

        let response_body: GenerateContentResponse = response.json().await.map_err(|e| {
            error!(error = %e, "Failed to parse Gemini response");
            AetherError::provider(format!("Failed to parse response: {}", e))
        })?;

        // Check for error in response
        if let Some(err) = response_body.error {
            return Err(AetherError::provider(format!(
                "Gemini error ({}): {}",
                err.code, err.message
            )));
        }

        // Extract text from candidates
        let text = response_body
            .candidates
            .unwrap_or_default()
            .into_iter()
            .flat_map(|c| c.content.parts)
            .map(|p| p.text)
            .collect::<Vec<_>>()
            .join("");

        Ok(text)
    }

    async fn parse_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<String>>> {
        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AetherError::provider(format!(
                "Gemini streaming error ({}): {}",
                status, error_text
            )));
        }

        let stream = response
            .bytes_stream()
            .map(move |chunk| {
                let bytes = chunk.map_err(|e| AetherError::network(e.to_string()))?;
                let text = String::from_utf8_lossy(&bytes);

                let mut result = String::new();
                for line in text.lines() {
                    if let Some(content) = Self::parse_sse_line(line) {
                        result.push_str(&content);
                    }
                }

                Ok(result)
            })
            .filter(|r| {
                let keep = match r {
                    Ok(s) => !s.is_empty(),
                    Err(_) => true,
                };
                std::future::ready(keep)
            })
            .boxed();

        Ok(stream)
    }

    fn name(&self) -> &'static str {
        "gemini"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_endpoint() {
        let mut config = ProviderConfig::test_config("gemini-1.5-flash");
        config.api_key = Some("test-key".to_string());

        let endpoint = GeminiProtocol::build_endpoint(&config, false).unwrap();
        assert!(endpoint.contains("v1beta/models/gemini-1.5-flash:generateContent"));
        assert!(endpoint.contains("key=test-key"));
    }

    #[test]
    fn test_build_endpoint_streaming() {
        let mut config = ProviderConfig::test_config("gemini-1.5-pro");
        config.api_key = Some("test-key".to_string());

        let endpoint = GeminiProtocol::build_endpoint(&config, true).unwrap();
        assert!(endpoint.contains(":streamGenerateContent"));
        assert!(endpoint.contains("alt=sse"));
    }

    #[test]
    fn test_parse_sse_line() {
        let line = r#"data: {"candidates":[{"content":{"parts":[{"text":"Hello"}]}}]}"#;
        let result = GeminiProtocol::parse_sse_line(line);
        assert_eq!(result, Some("Hello".to_string()));
    }
}
```

**Step 2: Update protocols/mod.rs**

```rust
// core/src/providers/protocols/mod.rs

//! Protocol implementations for different AI APIs

pub mod openai;
pub mod anthropic;
pub mod gemini;

pub use openai::OpenAiProtocol;
pub use anthropic::AnthropicProtocol;
pub use gemini::GeminiProtocol;
```

**Step 3: Run tests**

```bash
cargo test -p aethecore protocols::gemini --no-fail-fast
```

**Step 4: Commit**

```bash
git add core/src/providers/protocols/
git commit -m "feat(providers): implement GeminiProtocol adapter

Implements ProtocolAdapter trait for Gemini generateContent API:
- API key in query parameter
- Model-specific endpoint construction
- systemInstruction support
- SSE streaming response parsing

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 8: Add Gemini Presets and Update Factory

**Files:**
- Modify: `core/src/providers/presets.rs`
- Modify: `core/src/providers/mod.rs`

**Step 1: Add Gemini presets**

Add after the anthropic entries in presets.rs:

```rust
    // Google Gemini
    m.insert(
        "gemini",
        ProviderPreset {
            base_url: "https://generativelanguage.googleapis.com",
            protocol: "gemini",
            color: "#4285f4",
        },
    );
    m.insert(
        "google",
        ProviderPreset {
            base_url: "https://generativelanguage.googleapis.com",
            protocol: "gemini",
            color: "#4285f4",
        },
    );
```

**Step 2: Add gemini preset test**

```rust
    #[test]
    fn test_gemini_preset() {
        let gemini = get_preset("gemini").unwrap();
        assert_eq!(gemini.protocol, "gemini");
        assert_eq!(gemini.base_url, "https://generativelanguage.googleapis.com");
    }
```

**Step 3: Update factory for Gemini**

Replace the gemini case (lines 164-167) in mod.rs:

```rust
        "gemini" => {
            // Use HttpProvider + GeminiProtocol
            use std::time::Duration;

            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(config.timeout_seconds))
                .build()
                .map_err(|e| AetherError::invalid_config(format!("Failed to build HTTP client: {}", e)))?;

            let adapter = Arc::new(protocols::GeminiProtocol::new(client));
            let provider = HttpProvider::new(name.to_string(), config, adapter)?;
            Ok(Arc::new(provider))
        }
```

**Step 4: Run tests**

```bash
cargo test -p aethecore providers::tests --no-fail-fast
cargo test -p aethecore presets --no-fail-fast
```

**Step 5: Commit**

```bash
git add core/src/providers/presets.rs core/src/providers/mod.rs
git commit -m "feat(providers): add Gemini presets and update factory

Factory now routes gemini/google to HttpProvider + GeminiProtocol.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 9: Delete Legacy Gemini Provider

**Files:**
- Delete: `core/src/providers/gemini_legacy.rs`
- Modify: `core/src/providers/mod.rs`

**Step 1: Remove legacy module and re-export**

In mod.rs, remove:
```rust
mod gemini_legacy;
pub use gemini_legacy::GeminiProvider;
```

**Step 2: Delete the file**

```bash
rm core/src/providers/gemini_legacy.rs
```

**Step 3: Build and verify**

```bash
cargo build -p aethecore 2>&1 | tail -10
```

**Step 4: Run tests**

```bash
cargo test -p aethecore --no-fail-fast 2>&1 | grep -E "(passed|failed)"
```

**Step 5: Commit**

```bash
git add -A
git commit -m "refactor(providers): remove legacy GeminiProvider (~663 lines)

GeminiProvider is now replaced by HttpProvider + GeminiProtocol.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 10: Delete Legacy OpenAI Provider

**Files:**
- Delete: `core/src/providers/openai/provider.rs`
- Modify: `core/src/providers/openai/mod.rs`

**Step 1: Check if provider.rs is used**

```bash
grep -r "openai::provider" core/src/ --include="*.rs"
grep -r "OpenAiProvider" core/src/ --include="*.rs"
```

If only in mod.rs re-exports, proceed.

**Step 2: Remove from openai/mod.rs**

Remove the provider module export if present. Keep types.rs and request.rs.

**Step 3: Delete the file**

```bash
rm core/src/providers/openai/provider.rs
```

**Step 4: Update mod.rs re-exports if needed**

Remove `pub use openai::OpenAiProvider;` from providers/mod.rs if present.

**Step 5: Build and verify**

```bash
cargo build -p aethecore 2>&1 | tail -10
```

**Step 6: Run tests**

```bash
cargo test -p aethecore --no-fail-fast 2>&1 | grep -E "(passed|failed)"
```

**Step 7: Commit**

```bash
git add -A
git commit -m "refactor(providers): remove legacy OpenAiProvider (~792 lines)

OpenAI is now handled by HttpProvider + OpenAiProtocol.
Types and request modules preserved for protocol adapter use.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 11: Final Verification and Documentation

**Step 1: Run full test suite**

```bash
cargo test -p aethecore 2>&1 | grep -E "(passed|failed|FAILED)"
```

**Step 2: Verify code reduction**

```bash
git diff --stat main
```

Expected: Net reduction of ~1,500+ lines

**Step 3: Update providers module documentation**

Update the doc comment at top of `core/src/providers/mod.rs` to list:
- OpenAI protocol (HttpProvider + OpenAiProtocol)
- Anthropic protocol (HttpProvider + AnthropicProtocol)
- Gemini protocol (HttpProvider + GeminiProtocol)
- Ollama (native OllamaProvider)

**Step 4: Final commit**

```bash
git add -A
git commit -m "docs(providers): update module documentation for Phase 2

All cloud API providers now use ProtocolAdapter architecture:
- OpenAI family: HttpProvider + OpenAiProtocol
- Claude: HttpProvider + AnthropicProtocol
- Gemini: HttpProvider + GeminiProtocol
- Ollama: Native implementation (local models)

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Summary

After completing all tasks:

- **Files created:** 6 (anthropic/types.rs, anthropic/mod.rs, gemini/types.rs, gemini/mod.rs, protocols/anthropic.rs, protocols/gemini.rs)
- **Files deleted:** 3 (claude.rs ~1040 lines, gemini.rs ~663 lines, openai/provider.rs ~792 lines)
- **Net change:** ~1,500+ lines removed

**Success Criteria:**
- [ ] All existing tests pass
- [ ] Claude works via HttpProvider + AnthropicProtocol
- [ ] Gemini works via HttpProvider + GeminiProtocol
- [ ] Configuration backward compatible
- [ ] Extended thinking support preserved
- [ ] Vision/multimodal support preserved
