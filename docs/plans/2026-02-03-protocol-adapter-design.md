# Protocol Adapter 架构设计

> 将 Aleph Provider 架构从 "Vendor-Centric" 转变为 "Protocol-Centric"

**日期**: 2026-02-03
**状态**: 已批准
**优先级**: P0

---

## 1. 背景

Aleph 当前的 Provider 架构采用 "Vendor-First" 模式：每个 AI 厂商对应一个独立的 Rust Struct（DeepSeekProvider, MoonshotProvider 等）。这导致了约 600 行的代码冗余——这些 Provider 本质上都是 OpenAI 协议的换皮。

### 1.1 问题分析

| Provider | 代码行数 | 唯一差异 |
|----------|---------|---------|
| DeepSeekProvider | ~145 | `base_url = "https://api.deepseek.com"` |
| MoonshotProvider | ~152 | `base_url = "https://api.moonshot.cn/v1"` |
| DoubaoProvider | ~187 | `base_url = "https://ark.cn-beijing.volces.com/api/v3"` + UUID 验证 |
| OpenAiCompatibleProvider | ~220 | 强制要求用户提供 `base_url` |

其余 95% 的代码都是相同的 `impl AiProvider` 委托。

### 1.2 与 OpenClaw 的对比

OpenClaw 采用 "Protocol-First" 设计：
- 核心抽象是 API 协议（openai-completions, anthropic-messages）而非厂商名称
- 配置 `api: "openai-completions"` 即可指向任何兼容端点
- 新增厂商无需修改代码，只需配置

---

## 2. 目标

将架构从 "Vendor-Centric" 转变为 **"Protocol-Centric"**：

- **消除冗余**：删除 deepseek.rs, moonshot.rs, doubao.rs, openai_compatible.rs
- **配置驱动**：新增 OpenAI 兼容厂商只需修改配置，无需写代码
- **向后兼容**：现有用户配置无需修改即可工作
- **渐进演进**：第一阶段只处理 OpenAI 家族，Claude/Gemini/Ollama 暂不动

### 2.1 非目标

- 本次不迁移 ClaudeProvider、GeminiProvider、OllamaProvider
- 不改变 `AiProvider` trait 的公开接口
- 不强制用户修改现有 config.toml

---

## 3. 核心抽象

### 3.1 ProtocolAdapter Trait

协议适配器是本次重构的核心抽象，负责"如何与 API 通信"，与"哪个厂商"完全解耦。

```rust
// core/src/providers/adapter.rs

use async_trait::async_trait;
use futures::stream::BoxStream;

#[async_trait]
pub trait ProtocolAdapter: Send + Sync {
    /// 构建 HTTP 请求
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder>;

    /// 解析一次性响应
    async fn parse_response(&self, response: reqwest::Response) -> Result<String>;

    /// 解析流式响应（SSE）
    async fn parse_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<String>>>;
}
```

**设计决策**：采用双方法设计（parse_response + parse_stream）而非统一返回 Stream，因为：
- 流式和非流式的解析逻辑差异大（CPU 密集 vs IO 密集）
- 调用意图明确（请求时已决定 `stream: true/false`）
- 符合 Rust 的 Future/Stream 类型分离哲学

### 3.2 RequestPayload DTO

统一的请求载荷，包含一次 LLM 调用可能涉及的所有数据要素：

```rust
// core/src/providers/adapter.rs

use crate::core::MediaAttachment;
use crate::clipboard::ImageData;
use crate::agents::thinking::ThinkLevel;

/// 协议适配器的通用输入上下文
#[derive(Debug, Default)]
pub struct RequestPayload<'a> {
    /// 核心文本输入 (User Message)
    pub input: &'a str,

    /// 系统提示词 (System Prompt)
    pub system_prompt: Option<&'a str>,

    /// 遗留图像格式 (兼容 process_with_image)
    pub image: Option<&'a ImageData>,

    /// 多模态附件 (兼容 process_with_attachments)
    pub attachments: Option<&'a [MediaAttachment]>,

    /// 思考/推理模式配置
    pub think_level: Option<ThinkLevel>,

    // 未来扩展点：
    // pub tools: Option<&'a [ToolDefinition]>,
    // pub history: Option<&'a [Message]>,
    // pub json_mode: Option<bool>,
}

impl<'a> RequestPayload<'a> {
    pub fn new(input: &'a str) -> Self {
        Self { input, ..Default::default() }
    }

    pub fn with_system(mut self, prompt: Option<&'a str>) -> Self {
        self.system_prompt = prompt;
        self
    }

    pub fn with_image(mut self, image: Option<&'a ImageData>) -> Self {
        self.image = image;
        self
    }

    pub fn with_attachments(mut self, attachments: Option<&'a [MediaAttachment]>) -> Self {
        self.attachments = attachments;
        self
    }

    pub fn with_think_level(mut self, level: Option<ThinkLevel>) -> Self {
        self.think_level = level;
        self
    }
}
```

**设计决策**：采用 "Parameter Object" 模式（单一入口 + RequestPayload）而非多方法映射，避免方法签名的组合爆炸。

---

## 4. HttpProvider 容器

### 4.1 结构定义

通用的 Provider 容器，持有一个 `ProtocolAdapter`：

```rust
// core/src/providers/http_provider.rs

use std::sync::Arc;
use std::time::Duration;

pub struct HttpProvider {
    name: String,
    config: ProviderConfig,
    client: reqwest::Client,
    adapter: Box<dyn ProtocolAdapter>,
}

impl HttpProvider {
    pub fn new(
        name: String,
        config: ProviderConfig,
        adapter: Box<dyn ProtocolAdapter>,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()?;
        Ok(Self { name, config, client, adapter })
    }

    /// 统一执行入口（非流式）
    async fn execute(&self, payload: RequestPayload<'_>) -> Result<String> {
        let request = self.adapter.build_request(&payload, &self.config, false)?;
        let response = request.send().await?;
        self.adapter.parse_response(response).await
    }

    /// 统一执行入口（流式）
    async fn execute_stream(
        &self,
        payload: RequestPayload<'_>,
    ) -> Result<BoxStream<'static, Result<String>>> {
        let request = self.adapter.build_request(&payload, &self.config, true)?;
        let response = request.send().await?;
        self.adapter.parse_stream(response).await
    }
}
```

### 4.2 AiProvider 实现

HttpProvider 实现 AiProvider trait，将各方法映射到 RequestPayload：

```rust
#[async_trait]
impl AiProvider for HttpProvider {
    fn process(
        &self,
        input: &str,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        Box::pin(async move {
            let payload = RequestPayload::new(input).with_system(system_prompt);
            self.execute(payload).await
        })
    }

    fn process_with_image(
        &self,
        input: &str,
        image: Option<&ImageData>,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        Box::pin(async move {
            let payload = RequestPayload::new(input)
                .with_system(system_prompt)
                .with_image(image);
            self.execute(payload).await
        })
    }

    fn process_with_attachments(
        &self,
        input: &str,
        attachments: Option<&[MediaAttachment]>,
        system_prompt: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        Box::pin(async move {
            let payload = RequestPayload::new(input)
                .with_system(system_prompt)
                .with_attachments(attachments);
            self.execute(payload).await
        })
    }

    fn process_with_thinking(
        &self,
        input: &str,
        system_prompt: Option<&str>,
        think_level: ThinkLevel,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + '_>> {
        Box::pin(async move {
            let payload = RequestPayload::new(input)
                .with_system(system_prompt)
                .with_think_level(Some(think_level));
            self.execute(payload).await
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn color(&self) -> &str {
        &self.config.color
    }

    fn supports_vision(&self) -> bool {
        // 可根据 config 或 adapter 能力判断
        true
    }
}
```

---

## 5. OpenAiProtocol 实现

将现有 OpenAiProvider 的核心逻辑迁移至此：

```rust
// core/src/providers/protocols/openai.rs

use serde_json::json;

pub struct OpenAiProtocol;

#[async_trait]
impl ProtocolAdapter for OpenAiProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder> {
        let client = reqwest::Client::new();
        let mut messages = Vec::new();

        // 1. System prompt
        if let Some(sys) = payload.system_prompt {
            messages.push(json!({ "role": "system", "content": sys }));
        }

        // 2. User message (含多模态)
        let content = self.build_content(payload)?;
        messages.push(json!({ "role": "user", "content": content }));

        // 3. 构建请求体
        let mut body = json!({
            "model": &config.model,
            "messages": messages,
            "stream": is_streaming,
        });

        // 4. 可选参数
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

        // 5. 思考模式（o1/o3 系列）
        if let Some(level) = &payload.think_level {
            body["reasoning_effort"] = json!(level.to_openai_string());
        }

        // 6. 构建请求
        let base_url = config.base_url.as_deref()
            .unwrap_or("https://api.openai.com/v1");
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

        Ok(client.post(&url)
            .header("Authorization", format!("Bearer {}", config.resolve_api_key()?))
            .header("Content-Type", "application/json")
            .json(&body))
    }

    async fn parse_response(&self, response: reqwest::Response) -> Result<String> {
        let body: OpenAiResponse = response.json().await?;
        body.choices.first()
            .map(|c| c.message.content.clone())
            .ok_or_else(|| AlephError::empty_response())
    }

    async fn parse_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<String>>> {
        let stream = response.bytes_stream()
            .map_err(|e| AlephError::network(e))
            .try_filter_map(|chunk| async move {
                Self::parse_sse_chunk(&chunk)
            });
        Ok(Box::pin(stream))
    }
}

impl OpenAiProtocol {
    /// 构建多模态 content 数组
    fn build_content(&self, payload: &RequestPayload) -> Result<serde_json::Value> {
        let mut parts = Vec::new();

        // 文本部分
        parts.push(json!({ "type": "text", "text": payload.input }));

        // 图片部分
        if let Some(img) = payload.image {
            let base64 = img.to_base64()?;
            parts.push(json!({
                "type": "image_url",
                "image_url": {
                    "url": format!("data:{};base64,{}", img.mime_type(), base64)
                }
            }));
        }

        // 附件部分
        if let Some(attachments) = payload.attachments {
            for att in attachments {
                if att.is_image() {
                    let base64 = att.to_base64()?;
                    parts.push(json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:{};base64,{}", att.mime_type(), base64)
                        }
                    }));
                }
            }
        }

        // 如果只有文本，返回简单字符串；否则返回数组
        if parts.len() == 1 {
            Ok(json!(payload.input))
        } else {
            Ok(json!(parts))
        }
    }

    /// 解析 SSE chunk
    fn parse_sse_chunk(chunk: &[u8]) -> Result<Option<String>> {
        let text = std::str::from_utf8(chunk)?;
        for line in text.lines() {
            if line.starts_with("data: ") {
                let data = &line[6..];
                if data == "[DONE]" {
                    return Ok(None);
                }
                let parsed: OpenAiStreamChunk = serde_json::from_str(data)?;
                if let Some(delta) = parsed.choices.first().and_then(|c| c.delta.content.as_ref()) {
                    return Ok(Some(delta.clone()));
                }
            }
        }
        Ok(None)
    }
}
```

---

## 6. 厂商预设与工厂函数

### 6.1 预设注册表

将厂商特定的配置从代码移到数据：

```rust
// core/src/providers/presets.rs

use std::collections::HashMap;
use once_cell::sync::Lazy;

pub struct ProviderPreset {
    pub base_url: &'static str,
    pub protocol: &'static str,
    pub api_key_header: Option<&'static str>,
}

pub static PRESETS: Lazy<HashMap<&'static str, ProviderPreset>> = Lazy::new(|| {
    let mut m = HashMap::new();

    m.insert("openai", ProviderPreset {
        base_url: "https://api.openai.com/v1",
        protocol: "openai",
        api_key_header: None,
    });

    m.insert("deepseek", ProviderPreset {
        base_url: "https://api.deepseek.com",
        protocol: "openai",
        api_key_header: None,
    });

    m.insert("moonshot", ProviderPreset {
        base_url: "https://api.moonshot.cn/v1",
        protocol: "openai",
        api_key_header: None,
    });

    m.insert("kimi", ProviderPreset {
        base_url: "https://api.moonshot.cn/v1",
        protocol: "openai",
        api_key_header: None,
    });

    m.insert("doubao", ProviderPreset {
        base_url: "https://ark.cn-beijing.volces.com/api/v3",
        protocol: "openai",
        api_key_header: None,
    });

    m.insert("volcengine", ProviderPreset {
        base_url: "https://ark.cn-beijing.volces.com/api/v3",
        protocol: "openai",
        api_key_header: None,
    });

    m.insert("ark", ProviderPreset {
        base_url: "https://ark.cn-beijing.volces.com/api/v3",
        protocol: "openai",
        api_key_header: None,
    });

    m.insert("t8star", ProviderPreset {
        base_url: "https://api.t8star.cn/v1",
        protocol: "openai",
        api_key_header: None,
    });

    // 易于扩展：新增厂商只需加一行
    m
});
```

### 6.2 重构后的工厂函数

```rust
// core/src/providers/mod.rs

pub fn create_provider(name: &str, mut config: ProviderConfig) -> Result<Arc<dyn AiProvider>> {
    let name_lower = name.to_lowercase();

    // 1. 应用预设配置（如果匹配）
    if let Some(preset) = PRESETS.get(name_lower.as_str()) {
        config.base_url.get_or_insert_with(|| preset.base_url.to_string());
        if config.protocol.is_none() && config.provider_type.is_none() {
            config.protocol = Some(preset.protocol.to_string());
        }
    }

    // 2. 解析协议（带兼容性推断）
    let protocol_name = config.protocol();

    // 3. 根据协议选择适配器
    match protocol_name {
        "openai" => {
            let adapter = Box::new(OpenAiProtocol);
            Ok(Arc::new(HttpProvider::new(name.to_string(), config, adapter)?))
        }

        // Phase 2 才会添加：
        // "anthropic" => {
        //     let adapter = Box::new(AnthropicProtocol);
        //     Ok(Arc::new(HttpProvider::new(name.to_string(), config, adapter)?))
        // }

        // 4. 原生 Provider 保持现状（Phase 1 不迁移）
        "claude" | "anthropic" => {
            Ok(Arc::new(ClaudeProvider::new(name.to_string(), config)?))
        }
        "gemini" => {
            Ok(Arc::new(GeminiProvider::new(name.to_string(), config)?))
        }
        "ollama" => {
            Ok(Arc::new(OllamaProvider::new(name.to_string(), config)?))
        }
        "mock" => {
            Ok(Arc::new(MockProvider::new(name.to_string())))
        }

        _ => Err(AlephError::unknown_protocol(protocol_name)),
    }
}
```

---

## 7. 配置兼容性

### 7.1 ProviderConfig 演进

新增 `protocol` 字段，同时保持向后兼容：

```rust
// core/src/config/types/provider.rs

#[derive(Debug, Deserialize, Clone)]
pub struct ProviderConfig {
    /// 新字段：显式指定协议
    #[serde(default)]
    pub protocol: Option<String>,

    /// 旧字段：保留用于兼容
    #[serde(default)]
    pub provider_type: Option<String>,

    // ... 其他字段保持不变
    pub api_key: Option<String>,
    pub model: String,
    pub base_url: Option<String>,
    pub color: String,
    pub enabled: bool,
    pub timeout_seconds: u64,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    // ...
}

impl ProviderConfig {
    /// 获取有效的协议名称
    /// 优先级：protocol > provider_type > 默认 "openai"
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
}
```

### 7.2 配置示例

**旧配置（继续工作，无需修改）：**
```toml
[providers.my_deepseek]
provider_type = "openai"
api_key = "sk-..."
model = "deepseek-chat"
base_url = "https://api.deepseek.com"
```

**新配置（推荐写法）：**
```toml
[providers.my_deepseek]
protocol = "openai"
api_key = "sk-..."
model = "deepseek-chat"
# base_url 可省略，自动从预设获取
```

**高级场景（跨协议）：**
```toml
[providers.minimax_anthropic]
protocol = "anthropic"  # Minimax 使用 Anthropic 协议
api_key = "..."
model = "abab6.5-chat"
base_url = "https://api.minimax.chat/v1"
```

---

## 8. 文件结构变更

### 8.1 新增文件

```
core/src/providers/
├── adapter.rs              # ProtocolAdapter trait + RequestPayload
├── http_provider.rs        # HttpProvider 容器
├── presets.rs              # 厂商预设注册表
└── protocols/
    ├── mod.rs
    └── openai.rs           # OpenAiProtocol 实现
```

### 8.2 删除文件（~600 行）

```
core/src/providers/
├── deepseek.rs             # 删除
├── moonshot.rs             # 删除
├── doubao.rs               # 删除
├── t8star.rs               # 删除
└── openai_compatible.rs    # 删除
```

### 8.3 保留不变

```
core/src/providers/
├── openai/                 # 保留，逻辑迁移后可考虑删除
├── claude.rs               # 保留（Phase 2）
├── gemini.rs               # 保留（Phase 2）
└── ollama.rs               # 保留（Phase 2）
```

---

## 9. 迁移步骤

| 步骤 | 描述 | 风险等级 |
|------|------|---------|
| **Step 1** | 创建 `adapter.rs`：定义 ProtocolAdapter trait 和 RequestPayload | 低 |
| **Step 2** | 创建 `http_provider.rs`：实现 HttpProvider 容器 | 低 |
| **Step 3** | 创建 `protocols/openai.rs`：从现有 OpenAiProvider 迁移核心逻辑 | 中 |
| **Step 4** | 创建 `presets.rs`：定义厂商预设 | 低 |
| **Step 5** | 重构 `mod.rs`：更新 create_provider 工厂函数 | 中 |
| **Step 6** | 更新 `ProviderConfig`：添加 protocol 字段和推断逻辑 | 低 |
| **Step 7** | 删除冗余文件：deepseek.rs, moonshot.rs 等 | 低 |
| **Step 8** | 测试验证：确保所有现有配置继续工作 | 关键 |

---

## 10. 测试策略

### 10.1 单元测试

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // 1. RequestPayload 构建测试
    #[test]
    fn test_payload_builder() {
        let payload = RequestPayload::new("Hello")
            .with_system(Some("You are helpful"))
            .with_think_level(Some(ThinkLevel::Medium));
        assert_eq!(payload.input, "Hello");
        assert!(payload.think_level.is_some());
    }

    // 2. OpenAiProtocol 请求构建测试
    #[test]
    fn test_openai_protocol_builds_correct_json() {
        let protocol = OpenAiProtocol;
        let payload = RequestPayload::new("Test");
        let config = ProviderConfig::test_config("gpt-4o");
        let request = protocol.build_request(&payload, &config, false).unwrap();
        // 验证 JSON 结构
    }

    // 3. 预设注册表测试
    #[test]
    fn test_presets_contain_known_vendors() {
        assert!(PRESETS.contains_key("deepseek"));
        assert!(PRESETS.contains_key("moonshot"));
        assert_eq!(PRESETS["deepseek"].protocol, "openai");
    }

    // 4. 配置兼容性测试
    #[test]
    fn test_protocol_inference_from_provider_type() {
        let config = ProviderConfig {
            provider_type: Some("openai".into()),
            ..Default::default()
        };
        assert_eq!(config.protocol(), "openai");
    }

    #[test]
    fn test_protocol_takes_precedence() {
        let config = ProviderConfig {
            protocol: Some("anthropic".into()),
            provider_type: Some("openai".into()),
            ..Default::default()
        };
        assert_eq!(config.protocol(), "anthropic");
    }
}
```

### 10.2 集成测试

- 验证 DeepSeek 通过 HttpProvider + OpenAiProtocol 正常工作
- 验证流式响应正确解析
- 验证多模态请求正确构建
- 验证现有用户配置无需修改即可工作

---

## 11. 未来演进

### Phase 2：原生 Provider 迁移

当 Phase 1 稳定后，可考虑：

| 阶段 | 内容 |
|------|------|
| **Phase 2a** | 将 ClaudeProvider 迁移为 AnthropicProtocol |
| **Phase 2b** | 将 GeminiProvider 迁移为 GeminiProtocol |
| **Phase 2c** | 将 OllamaProvider 迁移为 OllamaProtocol |

### Phase 3：动态模型发现

实现 P2 建议的动态模型目录：
- 内置 models.json 包含主流模型元数据
- 启动时自动探测可用模型（Live Probe）
- 用户只需配置 provider，系统自动选择最佳模型

### Phase 4：Profile CLI

实现 P3 建议的管理工具：
- `aleph profile add` - 添加新配置
- `aleph profile list` - 列出所有配置
- `aleph profile switch` - 切换默认配置

---

## 12. 成功指标

- [ ] 删除 ~600 行冗余代码
- [ ] 所有现有单元测试通过
- [ ] 所有现有用户配置无需修改即可工作
- [ ] 新增 OpenAI 兼容厂商只需修改 presets.rs（一行代码）
- [ ] 流式响应功能正常
- [ ] 多模态（图片）功能正常
- [ ] 思考模式（ThinkLevel）功能正常

---

## 附录：设计决策记录

| 决策点 | 选项 | 选择 | 理由 |
|--------|------|------|------|
| 多模态处理 | A. 单一入口 / B. 多方法 / C. 能力协商 | **A** | 避免方法爆炸，协议层只需关心序列化 |
| 流式响应 | A. 双方法 / B. 统一 Stream / C. 枚举返回 | **A** | 逻辑差异大，符合 Rust Future/Stream 分离哲学 |
| 迁移策略 | A. 全部迁移 / B. 渐进迁移 / C. 混合架构 | **B** | 聚焦核心矛盾，验证模式后再扩展 |
| 配置兼容 | A. 自动推断 / B. 显式要求 / C. 双字段 | **A** | 零迁移成本，Convention over Configuration |
