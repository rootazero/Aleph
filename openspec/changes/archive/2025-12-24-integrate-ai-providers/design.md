# Design: Integrate AI Providers

## Architecture Overview

### Component Diagram
```
┌─────────────────────────────────────────────────────────────┐
│                        AetherCore                            │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌──────────────┐      ┌──────────────┐                     │
│  │   Memory     │──────│   Router     │                     │
│  │   Module     │      │              │                     │
│  └──────────────┘      └──────┬───────┘                     │
│                               │                              │
│                               ▼                              │
│  ┌────────────────────────────────────────────┐             │
│  │          AiProvider Trait                  │             │
│  │  ┌──────────────────────────────────┐     │             │
│  │  │ async fn process(                │     │             │
│  │  │   &self,                         │     │             │
│  │  │   input: String,                 │     │             │
│  │  │   system_prompt: Option<String>  │     │             │
│  │  │ ) -> Result<String>              │     │             │
│  │  └──────────────────────────────────┘     │             │
│  └───────────┬──────────────┬─────────┬──────┘             │
│              │              │         │                     │
│        ┌─────▼─────┐  ┌────▼────┐  ┌─▼──────┐             │
│        │  OpenAI   │  │ Claude  │  │ Ollama │             │
│        │ Provider  │  │Provider │  │Provider│             │
│        └───────────┘  └─────────┘  └────────┘             │
│              │              │           │                   │
└──────────────┼──────────────┼───────────┼───────────────────┘
               │              │           │
               ▼              ▼           ▼
        ┌──────────┐   ┌──────────┐  ┌─────────┐
        │ OpenAI   │   │Anthropic │  │ Local   │
        │   API    │   │   API    │  │ Ollama  │
        └──────────┘   └──────────┘  └─────────┘
```

### Data Flow
```
1. User presses Cmd+~ (hotkey detected)
   ↓
2. AetherCore reads clipboard content
   ↓
3. Memory module retrieves relevant context
   ↓
4. Router matches content against rules
   ↓
5. Router selects appropriate AiProvider
   ↓
6. Memory augments prompt with context
   ↓
7. AiProvider.process() called with augmented prompt
   ↓
8. HTTP request to AI API (or local command execution)
   ↓
9. Response parsed and returned
   ↓
10. AetherCore writes result to clipboard
    ↓
11. Memory module stores interaction (async)
    ↓
12. Input simulator pastes result (Cmd+V)
```

## Core Abstractions

### AiProvider Trait
**Purpose**: 提供统一接口，支持多种 AI 后端

**Design Decisions**:
- **Async-first**: 所有方法返回 `Future`，使用 tokio 运行时
- **Simple interface**: 单一 `process()` 方法，避免过度抽象
- **Stateless**: Provider 不保存状态，便于并发调用
- **Testable**: 易于创建 mock implementation

```rust
#[async_trait]
pub trait AiProvider: Send + Sync {
    /// Process input text and return AI response
    async fn process(
        &self,
        input: &str,
        system_prompt: Option<&str>,
    ) -> Result<String, AetherError>;

    /// Get provider name (for logging/debugging)
    fn name(&self) -> &str;

    /// Get provider color (for Halo UI)
    fn color(&self) -> &str;
}
```

**Trade-offs**:
- ✅ 简单易用
- ✅ 易于测试
- ❌ 不支持流式响应（留给 Phase 6）
- ❌ 无法传递额外的 provider-specific 参数（未来可通过 HashMap 扩展）

### Router System
**Purpose**: 根据用户定义的规则将输入路由到合适的 provider

**Design Decisions**:
- **Regex-based matching**: 使用 `regex` crate 进行模式匹配
- **First-match wins**: 按配置顺序匹配，第一个匹配的规则生效
- **Fallback support**: 支持默认 provider（最后一条 `.*` 规则）
- **System prompt override**: 每条规则可自定义 system prompt

```rust
pub struct Router {
    rules: Vec<RoutingRule>,
    providers: HashMap<String, Arc<dyn AiProvider>>,
    default_provider: Option<String>,
}

pub struct RoutingRule {
    regex: Regex,
    provider_name: String,
    system_prompt: Option<String>,
}

impl Router {
    pub fn route(&self, input: &str) -> Option<(&dyn AiProvider, Option<&str>)> {
        // Find first matching rule
        for rule in &self.rules {
            if rule.regex.is_match(input) {
                if let Some(provider) = self.providers.get(&rule.provider_name) {
                    return Some((provider.as_ref(), rule.system_prompt.as_deref()));
                }
            }
        }
        // Fallback to default provider
        self.default_provider
            .as_ref()
            .and_then(|name| self.providers.get(name))
            .map(|p| (p.as_ref(), None))
    }
}
```

**Trade-offs**:
- ✅ 配置驱动，用户可自定义
- ✅ Regex 功能强大
- ✅ 支持 fallback 机制
- ❌ Regex 可能难以调试（考虑添加测试工具）
- ❌ 不支持动态规则（需重启，Phase 6 添加热重载）

## Provider Implementations

### OpenAI Provider
**HTTP Client**: `reqwest` with TLS
**Endpoint**: `POST https://api.openai.com/v1/chat/completions`
**Authentication**: Bearer token in header

**Request Format**:
```json
{
  "model": "gpt-4o",
  "messages": [
    {"role": "system", "content": "You are a helpful assistant."},
    {"role": "user", "content": "User input here"}
  ],
  "temperature": 0.7,
  "max_tokens": 4096
}
```

**Error Handling**:
- Network errors → `AetherError::NetworkError`
- 401/403 → `AetherError::AuthenticationError`
- 429 → `AetherError::RateLimitError`
- 500+ → `AetherError::ProviderError`

**Timeout**: 30 seconds (configurable)

### Claude Provider
**HTTP Client**: `reqwest` with TLS
**Endpoint**: `POST https://api.anthropic.com/v1/messages`
**Authentication**: `x-api-key` header + `anthropic-version` header

**Request Format**:
```json
{
  "model": "claude-3-5-sonnet-20241022",
  "messages": [
    {"role": "user", "content": "User input here"}
  ],
  "system": "You are a helpful assistant.",
  "max_tokens": 4096
}
```

**Key Differences from OpenAI**:
- `system` 是独立字段，不在 `messages` 中
- 需要 `anthropic-version: 2023-06-01` header
- 使用 `x-api-key` 而非 `Authorization: Bearer`

### Ollama Provider
**Execution Method**: `tokio::process::Command`
**Command Format**: `ollama run <model> "<prompt>"`

**Implementation**:
```rust
pub struct OllamaProvider {
    model: String,
}

impl AiProvider for OllamaProvider {
    async fn process(&self, input: &str, system_prompt: Option<&str>) -> Result<String> {
        let prompt = match system_prompt {
            Some(sys) => format!("{}\n\nUser: {}", sys, input),
            None => input.to_string(),
        };

        let output = Command::new("ollama")
            .arg("run")
            .arg(&self.model)
            .arg(&prompt)
            .output()
            .await?;

        if output.status.success() {
            Ok(String::from_utf8(output.stdout)?)
        } else {
            Err(AetherError::ProviderError(
                String::from_utf8_lossy(&output.stderr).to_string()
            ))
        }
    }
}
```

**Considerations**:
- 需要验证 `ollama` 命令可用（PATH 中）
- 本地执行无需网络，但可能耗时较长
- 无 token 限制，适合长文本

## Configuration Schema

### Extended config.toml
```toml
[general]
default_provider = "openai"  # Default provider if no rule matches

[providers.openai]
api_key = "sk-..."
model = "gpt-4o"
base_url = "https://api.openai.com/v1"  # Support proxies
color = "#10a37f"
max_tokens = 4096
temperature = 0.7
timeout_seconds = 30

[providers.claude]
api_key = "sk-ant-..."
model = "claude-3-5-sonnet-20241022"
base_url = "https://api.anthropic.com/v1"
color = "#d97757"
max_tokens = 4096
timeout_seconds = 30

[providers.ollama]
model = "llama3.2"
color = "#0000ff"

[[rules]]
regex = "^/draw"
provider = "openai"
system_prompt = "You are DALL-E. Generate images based on user descriptions."

[[rules]]
regex = "^/(code|rust|python)"
provider = "claude"
system_prompt = "You are a senior software engineer. Provide concise, production-ready code."

[[rules]]
regex = "^/local"
provider = "ollama"
system_prompt = "You are a helpful local AI assistant."

[[rules]]
regex = ".*"  # Catch-all fallback
provider = "openai"
```

### Config Validation
在加载配置时进行验证：
- API key 不为空（对于 OpenAI/Claude）
- Provider 引用存在
- Regex 语法有效
- 至少有一个 provider 配置

## Memory Integration

### Augmentation Strategy
记忆模块在 AI 处理前介入：

```rust
pub async fn process_with_memory(
    &self,
    input: &str,
    context: &CapturedContext,
) -> Result<String> {
    // 1. Retrieve relevant memories
    let memories = self.memory_store
        .retrieve(context, self.config.memory.max_context_items)
        .await?;

    // 2. Augment prompt with context
    let augmented_prompt = if !memories.is_empty() {
        let context_str = memories.iter()
            .map(|m| format!("- {}: {}", m.timestamp, m.user_input))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "Past Context (from {}):\n{}\n\nCurrent Request:\n{}",
            context.app_bundle_id,
            context_str,
            input
        )
    } else {
        input.to_string()
    };

    // 3. Route to AI provider
    let (provider, system_prompt) = self.router
        .route(&augmented_prompt)
        .ok_or(AetherError::NoProviderAvailable)?;

    // 4. Process with AI
    let response = provider.process(&augmented_prompt, system_prompt).await?;

    // 5. Store new memory (async, non-blocking)
    let memory_store = self.memory_store.clone();
    let ctx = context.clone();
    let inp = input.to_string();
    let resp = response.clone();
    tokio::spawn(async move {
        if let Err(e) = memory_store.store(&ctx, &inp, &resp).await {
            eprintln!("Failed to store memory: {}", e);
        }
    });

    Ok(response)
}
```

### Performance Considerations
- 记忆检索必须在 <50ms 内完成（见 memory-storage spec）
- 增强后的 prompt 长度不应超过 4K tokens
- 如果记忆检索失败，降级到无上下文处理（不阻塞用户）

## Error Handling Strategy

### Error Types Hierarchy
```rust
pub enum AetherError {
    // Network/API errors
    NetworkError(String),
    AuthenticationError(String),
    RateLimitError(String),
    ProviderError(String),
    Timeout,

    // Configuration errors
    NoProviderAvailable,
    InvalidConfig(String),

    // Memory errors
    MemoryError(String),

    // Other
    Unknown(String),
}
```

### Retry Strategy
- **Network errors**: 重试 3 次，指数退避（1s, 2s, 4s）
- **Rate limit**: 不重试，返回错误（避免加剧限流）
- **Authentication**: 不重试，返回错误
- **Timeout**: 不重试，返回错误
- **Provider errors (5xx)**: 重试 2 次

### Fallback Strategy
如果当前 provider 失败：
1. 记录错误日志
2. 尝试使用 `default_provider`（如果与当前不同）
3. 如果仍失败，返回友好错误信息给用户

## UniFFI Integration

### New Callback Events
扩展 `AetherEventHandler` trait：

```idl
callback interface AetherEventHandler {
    // Existing callbacks
    void on_state_changed(ProcessingState state);
    void on_hotkey_detected(string content);
    void on_error(ErrorType error_type, string message);

    // New callbacks for AI processing
    void on_ai_processing_started(string provider_name, string provider_color);
    void on_ai_response_received(string response_preview);  // First 100 chars
}
```

### Updated ProcessingState
```rust
pub enum ProcessingState {
    Idle,
    Listening,
    RetrievingMemory,     // NEW: Fetching context
    ProcessingWithAI,     // NEW: AI is processing
    Success,
    Error,
}
```

## Testing Strategy

### Unit Tests
每个 provider 单独测试：
```rust
#[tokio::test]
async fn test_openai_provider() {
    let provider = OpenAiProvider::new(config);
    let result = provider.process("Hello", None).await.unwrap();
    assert!(!result.is_empty());
}
```

### Mock Provider
用于集成测试：
```rust
pub struct MockProvider {
    response: String,
    delay: Duration,
}

#[async_trait]
impl AiProvider for MockProvider {
    async fn process(&self, _input: &str, _sys: Option<&str>) -> Result<String> {
        tokio::time::sleep(self.delay).await;
        Ok(self.response.clone())
    }
    fn name(&self) -> &str { "mock" }
    fn color(&self) -> &str { "#000000" }
}
```

### Router Tests
```rust
#[test]
fn test_router_regex_matching() {
    let router = Router::new(config);

    // Test code request routes to Claude
    let (provider, _) = router.route("/code write a function").unwrap();
    assert_eq!(provider.name(), "claude");

    // Test fallback
    let (provider, _) = router.route("random question").unwrap();
    assert_eq!(provider.name(), "openai");
}
```

### Integration Tests
端到端测试（使用 mock provider）：
```rust
#[tokio::test]
async fn test_end_to_end_ai_processing() {
    let core = AetherCore::new_with_mock_provider(handler);
    core.start_listening().unwrap();

    // Simulate clipboard content
    clipboard.write_text("Hello, AI!");

    // Trigger processing
    core.process_clipboard().await.unwrap();

    // Verify result in clipboard
    let result = clipboard.read_text().unwrap();
    assert_eq!(result, "Mock AI response");
}
```

## Performance Targets
- **Memory retrieval**: <50ms (已在 memory-storage spec 中定义)
- **Prompt augmentation**: <10ms
- **API call timeout**: 30 seconds (configurable)
- **Total E2E latency**: <5 seconds (for typical short prompts)
- **Memory storage (async)**: Non-blocking, <200ms in background

## Security Considerations

### API Key Storage
- **Phase 5**: 明文存储在 `~/.aether/config.toml`
- 文件权限：`600` (仅当前用户可读写)
- **Phase 6**: 迁移到 macOS Keychain

### PII Scrubbing
- **Phase 5**: 暂不实现（文档中警告用户）
- **Phase 6**: 添加 regex-based 过滤（电话、邮箱等）

### Network Security
- 使用 TLS 1.2+ 连接 API
- 验证 SSL 证书
- 支持自定义 `base_url` 用于企业代理

## Future Extensions (Out of Scope)

### Phase 6 Enhancements
1. **Streaming responses**: 实时显示 AI 生成进度
2. **Gemini integration**: 添加 Google Gemini provider
3. **Settings UI**: 可视化配置 providers 和 rules
4. **PII filtering**: 自动过滤敏感信息
5. **Smart context window**: 自动截断超长输入
6. **Hot reload**: 配置文件变化时自动重载

### Potential Improvements
- **Caching**: 缓存常见请求（如"翻译"）
- **Rate limiting**: 本地限流保护
- **Cost tracking**: 记录 API 使用量和费用
- **Multi-turn dialogue**: 支持多轮对话上下文
