# Phase 4: 更多生成供应商实现计划

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 添加 Stability AI、Replicate 和 ElevenLabs 生成供应商支持

**Architecture:** 每个供应商实现 GenerationProvider trait，使用统一的错误处理和响应转换模式

**Tech Stack:** Rust async/await, reqwest HTTP client, serde JSON, base64 encoding

---

## Task 1: 实现 Stability AI 图像生成供应商

**Files:**
- Create: `src/generation/providers/stability.rs`
- Modify: `src/generation/providers/mod.rs`

### Step 1: 创建 stability.rs 基础结构

```rust
//! Stability AI Image Generation Provider
//!
//! Supports Stable Diffusion XL and other Stability AI models via their REST API.
//!
//! # API Reference
//!
//! - Endpoint: POST `https://api.stability.ai/v1/generation/{engine_id}/text-to-image`
//! - Auth: Bearer token (API key)
//! - Request body: `{ text_prompts: [{text, weight}], cfg_scale, height, width, samples, steps }`
//! - Response: `{ artifacts: [{ base64, seed, finishReason }] }`

use crate::generation::{
    GenerationData, GenerationError, GenerationMetadata, GenerationOutput, GenerationProvider,
    GenerationRequest, GenerationResult, GenerationType,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tracing::{debug, error, info};

const DEFAULT_ENDPOINT: &str = "https://api.stability.ai";
const DEFAULT_MODEL: &str = "stable-diffusion-xl-1024-v1-0";
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Stability AI Image Generation Provider
#[derive(Debug, Clone)]
pub struct StabilityImageProvider {
    client: Client,
    api_key: String,
    endpoint: String,
    model: String,
}
```

### Step 2: 实现 new() 构造函数

```rust
impl StabilityImageProvider {
    pub fn new<S: Into<String>>(
        api_key: S,
        base_url: Option<String>,
        model: Option<String>,
    ) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            api_key: api_key.into(),
            endpoint: base_url.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string()),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        }
    }

    fn generation_url(&self) -> String {
        format!("{}/v1/generation/{}/text-to-image", self.endpoint, self.model)
    }
}
```

### Step 3: 实现 API 请求/响应结构

```rust
#[derive(Debug, Serialize)]
struct TextPrompt {
    text: String,
    weight: f32,
}

#[derive(Debug, Serialize)]
struct StabilityRequest {
    text_prompts: Vec<TextPrompt>,
    cfg_scale: f32,
    height: u32,
    width: u32,
    samples: u32,
    steps: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    style_preset: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StabilityResponse {
    artifacts: Vec<Artifact>,
}

#[derive(Debug, Deserialize)]
struct Artifact {
    base64: String,
    seed: i64,
    #[serde(rename = "finishReason")]
    finish_reason: String,
}
```

### Step 4: 实现 GenerationProvider trait

```rust
impl GenerationProvider for StabilityImageProvider {
    fn name(&self) -> &str { "stability-image" }
    fn color(&self) -> &str { "#8b5cf6" }
    fn default_model(&self) -> Option<&str> { Some(&self.model) }
    fn supports(&self, gen_type: GenerationType) -> bool {
        matches!(gen_type, GenerationType::Image)
    }
    fn supported_types(&self) -> Vec<GenerationType> {
        vec![GenerationType::Image]
    }

    fn generate<'a>(
        &'a self,
        request: GenerationRequest,
    ) -> Pin<Box<dyn Future<Output = GenerationResult<GenerationOutput>> + Send + 'a>> {
        Box::pin(async move {
            // Implementation...
        })
    }
}
```

### Step 5: 添加测试

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation() {
        let provider = StabilityImageProvider::new("sk-test", None, None);
        assert_eq!(provider.name(), "stability-image");
        assert!(provider.supports(GenerationType::Image));
    }

    // More tests...
}
```

### Step 6: 在 mod.rs 中导出和注册

在 `providers/mod.rs` 添加:
```rust
pub mod stability;
pub use stability::StabilityImageProvider;

// In create_provider():
"stability" | "stability_image" | "sdxl" => {
    Arc::new(StabilityImageProvider::new(
        api_key,
        config.base_url.clone(),
        config.model.clone(),
    ))
}
```

---

## Task 2: 实现 Replicate 统一 API 供应商

**Files:**
- Create: `src/generation/providers/replicate.rs`
- Modify: `src/generation/providers/mod.rs`

### Step 1: 创建 replicate.rs 基础结构

```rust
//! Replicate API Provider
//!
//! Provides unified access to multiple AI models via Replicate's API.
//! Supports various models for image, video, and audio generation.
//!
//! # API Reference
//!
//! - Create prediction: POST `https://api.replicate.com/v1/predictions`
//! - Get prediction: GET `https://api.replicate.com/v1/predictions/{id}`
//! - Auth: Bearer token

const DEFAULT_ENDPOINT: &str = "https://api.replicate.com";
const DEFAULT_TIMEOUT_SECS: u64 = 300; // Replicate can be slow
const POLL_INTERVAL_MS: u64 = 1000;

/// Built-in model mappings
const MODEL_FLUX_SCHNELL: &str = "black-forest-labs/flux-schnell";
const MODEL_SDXL: &str = "stability-ai/sdxl";
const MODEL_MUSICGEN: &str = "meta/musicgen";

#[derive(Debug, Clone)]
pub struct ReplicateProvider {
    client: Client,
    api_key: String,
    endpoint: String,
    model_mappings: HashMap<String, String>,
    supported_types: Vec<GenerationType>,
}
```

### Step 2: 实现 Builder 模式

```rust
pub struct ReplicateProviderBuilder {
    api_key: String,
    endpoint: String,
    model_mappings: HashMap<String, String>,
    supported_types: Vec<GenerationType>,
}

impl ReplicateProviderBuilder {
    pub fn new<S: Into<String>>(api_key: S) -> Self {
        let mut mappings = HashMap::new();
        mappings.insert("flux".to_string(), MODEL_FLUX_SCHNELL.to_string());
        mappings.insert("sdxl".to_string(), MODEL_SDXL.to_string());
        mappings.insert("musicgen".to_string(), MODEL_MUSICGEN.to_string());

        Self {
            api_key: api_key.into(),
            endpoint: DEFAULT_ENDPOINT.to_string(),
            model_mappings: mappings,
            supported_types: vec![GenerationType::Image, GenerationType::Audio],
        }
    }

    pub fn endpoint<S: Into<String>>(mut self, endpoint: S) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    pub fn add_model<S: Into<String>>(mut self, alias: S, model_version: S) -> Self {
        self.model_mappings.insert(alias.into(), model_version.into());
        self
    }

    pub fn build(self) -> ReplicateProvider {
        ReplicateProvider {
            client: Client::builder()
                .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
                .build()
                .expect("Failed to build HTTP client"),
            api_key: self.api_key,
            endpoint: self.endpoint,
            model_mappings: self.model_mappings,
            supported_types: self.supported_types,
        }
    }
}
```

### Step 3: 实现预测轮询逻辑

```rust
impl ReplicateProvider {
    async fn create_prediction(&self, model: &str, input: serde_json::Value) -> GenerationResult<String> {
        // POST /v1/predictions
    }

    async fn poll_prediction(&self, id: &str) -> GenerationResult<PredictionResponse> {
        // GET /v1/predictions/{id} with polling
    }
}
```

### Step 4: 实现 GenerationProvider trait

```rust
impl GenerationProvider for ReplicateProvider {
    fn name(&self) -> &str { "replicate" }
    fn color(&self) -> &str { "#f59e0b" }
    // ...

    fn generate<'a>(&'a self, request: GenerationRequest) -> Pin<Box<...>> {
        Box::pin(async move {
            let model = self.resolve_model(&request)?;
            let input = self.build_input(&request)?;
            let prediction_id = self.create_prediction(&model, input).await?;
            let result = self.poll_prediction(&prediction_id).await?;
            self.convert_output(result)
        })
    }
}
```

---

## Task 3: 实现 ElevenLabs TTS 供应商

**Files:**
- Create: `src/generation/providers/elevenlabs.rs`
- Modify: `src/generation/providers/mod.rs`

### Step 1: 创建 elevenlabs.rs 基础结构

```rust
//! ElevenLabs Text-to-Speech Provider
//!
//! High-quality voice synthesis with multiple voices and languages.
//!
//! # API Reference
//!
//! - Endpoint: POST `https://api.elevenlabs.io/v1/text-to-speech/{voice_id}`
//! - Auth: xi-api-key header
//! - Request body: `{ text, model_id, voice_settings: { stability, similarity_boost } }`
//! - Response: Raw audio bytes (mp3)

const DEFAULT_ENDPOINT: &str = "https://api.elevenlabs.io";
const DEFAULT_MODEL: &str = "eleven_monolingual_v1";
const DEFAULT_VOICE: &str = "21m00Tcm4TlvDq8ikWAM"; // Rachel

/// Available ElevenLabs voices (subset)
pub const VOICES: &[(&str, &str)] = &[
    ("rachel", "21m00Tcm4TlvDq8ikWAM"),
    ("domi", "AZnzlk1XvdvUeBnXmlld"),
    ("bella", "EXAVITQu4vr4xnSDxMaL"),
    ("antoni", "ErXwobaYiN019PkySvjV"),
    ("elli", "MF3mGyEYCl7XYWbV9V6O"),
    ("josh", "TxGEqnHWrfWFTfGW9XjX"),
    ("arnold", "VR6AewLTigWG4xSOukaG"),
    ("adam", "pNInz6obpgDQGcFmaJgB"),
    ("sam", "yoZ06aMxZJJ28mfd3POQ"),
];

#[derive(Debug, Clone)]
pub struct ElevenLabsProvider {
    client: Client,
    api_key: String,
    endpoint: String,
    model: String,
    default_voice_id: String,
}
```

### Step 2: 实现构造函数和辅助方法

```rust
impl ElevenLabsProvider {
    pub fn new<S: Into<String>>(
        api_key: S,
        base_url: Option<String>,
        model: Option<String>,
        default_voice: Option<String>,
    ) -> GenerationResult<Self> {
        let voice_id = default_voice
            .map(|v| Self::resolve_voice_id(&v))
            .transpose()?
            .unwrap_or_else(|| DEFAULT_VOICE.to_string());

        Ok(Self {
            client: Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .expect("Failed to build HTTP client"),
            api_key: api_key.into(),
            endpoint: base_url.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string()),
            model: model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            default_voice_id: voice_id,
        })
    }

    fn resolve_voice_id(voice: &str) -> GenerationResult<String> {
        // Check if it's already a voice ID
        if voice.len() > 15 {
            return Ok(voice.to_string());
        }
        // Look up by name
        VOICES.iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(voice))
            .map(|(_, id)| id.to_string())
            .ok_or_else(|| GenerationError::invalid_parameters(
                format!("Unknown voice: '{}'. Available: {:?}", voice, VOICES.iter().map(|(n,_)| n).collect::<Vec<_>>()),
                Some("voice".to_string()),
            ))
    }
}
```

### Step 3: 实现 GenerationProvider trait

```rust
impl GenerationProvider for ElevenLabsProvider {
    fn name(&self) -> &str { "elevenlabs" }
    fn color(&self) -> &str { "#00c7b7" }
    fn supports(&self, gen_type: GenerationType) -> bool {
        matches!(gen_type, GenerationType::Speech)
    }

    fn generate<'a>(&'a self, request: GenerationRequest) -> Pin<Box<...>> {
        Box::pin(async move {
            let voice_id = request.params.voice
                .as_ref()
                .map(|v| Self::resolve_voice_id(v))
                .transpose()?
                .unwrap_or_else(|| self.default_voice_id.clone());

            let url = format!("{}/v1/text-to-speech/{}", self.endpoint, voice_id);

            let body = serde_json::json!({
                "text": request.prompt,
                "model_id": self.model,
                "voice_settings": {
                    "stability": 0.5,
                    "similarity_boost": 0.75
                }
            });

            let response = self.client
                .post(&url)
                .header("xi-api-key", &self.api_key)
                .json(&body)
                .send()
                .await?;

            // Handle response...
        })
    }
}
```

---

## Task 4: 更新工厂函数和模块导出

**Files:**
- Modify: `src/generation/providers/mod.rs`

### Step 1: 添加新模块导出

```rust
pub mod elevenlabs;
pub mod replicate;
pub mod stability;

pub use elevenlabs::ElevenLabsProvider;
pub use replicate::{ReplicateProvider, ReplicateProviderBuilder};
pub use stability::StabilityImageProvider;
```

### Step 2: 更新 create_provider() 工厂函数

```rust
pub fn create_provider(
    name: &str,
    config: &GenerationProviderConfig,
) -> GenerationResult<Arc<dyn GenerationProvider>> {
    let api_key = config.api_key.clone().ok_or_else(|| {
        GenerationError::authentication(
            format!("API key is required for provider '{}'", name),
            name,
        )
    })?;

    let provider: Arc<dyn GenerationProvider> = match config.provider_type.as_str() {
        // Existing providers...
        "openai" | "openai_image" | "dalle" => { /* ... */ }
        "openai_tts" | "tts" => { /* ... */ }
        "openai_compat" => { /* ... */ }

        // New providers
        "stability" | "stability_image" | "sdxl" => {
            Arc::new(StabilityImageProvider::new(
                api_key,
                config.base_url.clone(),
                config.model.clone(),
            ))
        }
        "replicate" => {
            let mut builder = ReplicateProviderBuilder::new(api_key);
            if let Some(url) = &config.base_url {
                builder = builder.endpoint(url);
            }
            // Add custom model mappings from config.models if present
            if let Some(models) = &config.models {
                for (alias, version) in models {
                    builder = builder.add_model(alias, version);
                }
            }
            Arc::new(builder.build())
        }
        "elevenlabs" => {
            Arc::new(ElevenLabsProvider::new(
                api_key,
                config.base_url.clone(),
                config.model.clone(),
                config.defaults.voice.clone(),
            )?)
        }

        other => {
            return Err(GenerationError::invalid_parameters(
                format!(
                    "Unknown provider type: '{}'. Supported: openai, dalle, openai_tts, tts, openai_compat, stability, sdxl, replicate, elevenlabs",
                    other
                ),
                Some("provider_type".to_string()),
            ));
        }
    };

    Ok(provider)
}
```

---

## Task 5: 添加配置类型支持

**Files:**
- Modify: `src/config/types/generation.rs`

### Step 1: 添加 models 字段到 GenerationProviderConfig

```rust
pub struct GenerationProviderConfig {
    // Existing fields...
    pub provider_type: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub enabled: bool,
    pub color: String,
    pub capabilities: Vec<GenerationType>,
    pub defaults: GenerationDefaults,

    // New field for Replicate model mappings
    #[serde(default)]
    pub models: Option<HashMap<String, String>>,
}
```

---

## Task 6: 完整构建验证

**Commands:**

```bash
cd /Users/zouguojun/Workspace/Aether/Aether/core

# Check compilation
cargo check

# Run all tests
cargo test

# Run specific provider tests
cargo test generation::providers::stability
cargo test generation::providers::replicate
cargo test generation::providers::elevenlabs
```

**Expected:**
- All tests pass
- No compilation errors
- All 3 new providers work correctly

---

## 提交

```bash
git add src/generation/providers/
git commit -m "feat(generation): add Stability AI, Replicate, and ElevenLabs providers (Phase 4)"
```
