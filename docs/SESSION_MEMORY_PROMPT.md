# Aether AI Provider 集成 - 会话记忆 Prompt

> **用途**: 当继续实施 `integrate-ai-providers` 的后续 Phase（5-10）时，将此 prompt 提供给 Claude，以快速恢复上下文。

---

## 项目背景

**Aether** 是一个 macOS/Windows/Linux 的系统级 AI 中间件，采用 Rust Core + Native UI 架构。核心理念是"Ghost"美学 - 无 Dock 图标、无永久窗口，只有后台进程和系统托盘。

### 技术栈
- **Rust Core**: `cdylib` + `staticlib`，通过 UniFFI 暴露接口
- **macOS UI**: Swift + SwiftUI (NSWindow 透明覆盖层)
- **通信**: UniFFI callback-driven
- **AI Providers**: OpenAI、Claude、Ollama（已实现）

### 用户交互流程
1. 用户选中文本，按 `Cmd+~`
2. Aether 模拟 `Cmd+X`（文本"消失"）
3. 光标处显示"Halo"动画（透明覆盖层）
4. 后端路由到合适的 AI Provider
5. Halo 消失，结果通过 `Cmd+V` 粘贴回去

---

## 已完成工作（Phase 1-4）

### Phase 1: Foundation ✅
- `AiProvider` trait 定义（统一接口）
- `ProviderConfig` 结构（TOML 配置）
- `ProviderRegistry`（provider 管理）
- `MockProvider`（测试用）
- 扩展 `AetherError`（新增 AuthenticationError、RateLimitError 等）

**测试**: 31 tests passed

### Phase 2: OpenAI Provider ✅
**文件**: `Aether/core/src/providers/openai.rs` (437 行)

**核心功能**:
- OpenAI Chat Completion API 客户端
- 支持 GPT-4o、GPT-4o-mini 等模型
- 自定义 base_url（兼容 OpenAI-compatible APIs）
- 完整错误处理：401 (auth)、429 (rate limit)、5xx (server error)

**关键设计**:
```rust
pub struct OpenAiProvider {
    client: reqwest::Client,  // HTTP client with timeout
    config: ProviderConfig,
    endpoint: String,  // base_url + "/chat/completions"
}
```

**API 格式**:
- Request: `{ model, messages: [{role, content}], max_tokens?, temperature? }`
- Response: `choices[0].message.content`
- Header: `Authorization: Bearer {api_key}`

**测试**: 10 tests (配置验证、请求构建、错误处理)

### Phase 3: Claude Provider ✅
**文件**: `Aether/core/src/providers/claude.rs` (442 行)

**核心功能**:
- Anthropic Messages API 客户端
- 支持 Claude 3.5 Sonnet 及其他模型
- 处理 529 (overloaded) 状态

**关键区别**（vs OpenAI）:
- System prompt 是独立字段（不在 messages 数组）
- 响应格式：`content[0].text`（不是 `choices[0]`）
- Headers: `x-api-key`（不是 `Authorization`）+ `anthropic-version: 2023-06-01`
- Endpoint: `/v1/messages`

**测试**: 11 tests

### Phase 4: Ollama Provider ✅
**文件**: `Aether/core/src/providers/ollama.rs` (318 行)

**核心功能**:
- 本地 Ollama CLI 命令执行
- 支持 Llama、Mistral 等开源模型
- 无需 API key（完全本地）

**关键设计**:
- 使用 `tokio::process::Command` 执行 `ollama run <model> <prompt>`
- ANSI 转义码清理（正则：`\x1b\[[0-9;]*m`）
- System prompt 拼接到用户输入前
- 超时控制（`tokio::time::timeout`）

**测试**: 11 tests (ANSI 清理、prompt 格式化、配置验证)

---

## 架构关键点

### 1. 统一 Provider Trait
```rust
#[async_trait]
pub trait AiProvider: Send + Sync {
    async fn process(&self, input: &str, system_prompt: Option<&str>) -> Result<String>;
    fn name(&self) -> &str;
    fn color(&self) -> &str;  // UI 主题色（#10a37f for OpenAI）
}
```

### 2. 配置结构
```rust
pub struct ProviderConfig {
    pub api_key: Option<String>,       // 云 Provider 需要
    pub model: String,                 // "gpt-4o", "claude-3-5-sonnet-20241022", "llama3.2"
    pub base_url: Option<String>,      // 自定义 API endpoint
    pub color: String,                 // Hex color for UI
    pub timeout_seconds: u64,          // 30-60 seconds
    pub max_tokens: Option<u32>,       // Response length limit
    pub temperature: Option<f32>,      // 0.0-2.0
}
```

### 3. 错误处理
```rust
pub enum AetherError {
    AuthenticationError(String),  // 401 - Invalid API key
    RateLimitError(String),       // 429 - Rate limit exceeded
    ProviderError(String),        // 4xx/5xx - API errors
    NetworkError(String),         // Connection failures
    Timeout,                      // Request timeout
    // ... 其他错误类型
}
```

---

## 当前项目状态

### 文件结构
```
Aether/core/src/
├── providers/
│   ├── mod.rs           # Exports all providers
│   ├── openai.rs        # ✅ OpenAI implementation
│   ├── claude.rs        # ✅ Claude implementation
│   ├── ollama.rs        # ✅ Ollama implementation
│   ├── mock.rs          # ✅ Mock for testing
│   └── registry.rs      # ✅ Provider registry
├── config.rs            # ✅ ProviderConfig + Config
├── error.rs             # ✅ Extended with new error types
├── core.rs              # AetherCore (需集成 Router)
└── router/              # ⚠️ 待实现（Phase 5）
```

### 依赖项（Cargo.toml）
```toml
[dependencies]
reqwest = { version = "0.11", features = ["json", "rustls-tls"] }
serde_json = "1.0"
tokio = { version = "1.35", features = ["rt-multi-thread", "sync", "time", "macros", "process"] }
regex = "1.10"
async-trait = "0.1"
# ... 其他依赖
```

### 测试覆盖
- **Providers 模块**: 55/55 tests passed
  - OpenAI: 10 tests
  - Claude: 11 tests
  - Ollama: 11 tests
  - Mock: 8 tests
  - Registry: 12 tests
  - Trait: 3 tests

---

## 待实施工作（Phase 5-10）

### Phase 5: Router Implementation ⚠️ NEXT
**目标**: 基于 regex 规则选择合适的 Provider

**关键任务**:
1. 定义 `RoutingRule` 结构（regex + provider_name + system_prompt）
2. 实现 `Router` 结构（持有 rules + providers + default_provider）
3. 实现 `route(&self, input: &str)` 方法（first-match 优先级）
4. 扩展 `Config` 支持 `rules: Vec<RoutingRule>`
5. 编写 Router 测试（prefix 匹配、catch-all、fallback）

**示例配置**（TOML）:
```toml
[general]
default_provider = "openai"

[[rules]]
regex = "^/code|rust|python"
provider = "claude"
system_prompt = "You are a senior engineer. Output code only."

[[rules]]
regex = "^/local"
provider = "ollama"

[[rules]]
regex = ".*"  # Catch-all
provider = "openai"
```

### Phase 6: Memory Integration
- 检索记忆上下文（`memory_store.retrieve()`）
- Prompt 增强（在用户输入前添加 "Past Context:"）
- 异步存储交互（`tokio::spawn()`）

### Phase 7: AetherCore Integration
- 添加 `router: Arc<Router>` 字段到 `AetherCore`
- 实现 `process_with_ai()` pipeline
- 更新 `process_clipboard()` 调用 AI
- 扩展 UniFFI 回调（`on_ai_processing_started`, `on_ai_response_received`）

### Phase 8: Configuration & Testing
- 创建 `config.example.toml`
- 实现配置加载（`~/.config/aether/config.toml`）
- 编写集成测试（`tests/integration_ai.rs`）
- 性能基准测试（`benches/ai_benchmarks.rs`）

### Phase 9: Swift UI Integration
- 生成 UniFFI bindings（`cargo run --bin uniffi-bindgen`）
- 更新 Swift EventHandler
- Halo 显示 provider color
- 端到端测试（macOS app）

### Phase 10: Polish
- 实现指数退避重试（`providers/retry.rs`）
- Fallback 策略（default provider）
- 用户友好错误消息
- Clippy 清理

---

## 重要约束和最佳实践

### 1. 模块化设计
- 所有核心组件使用 trait 抽象（便于替换实现）
- 配置驱动（避免硬编码）

### 2. 错误处理原则
- 使用 `AetherError` 统一错误类型
- 提供清晰的用户友好错误消息
- 区分可重试错误（Network）和不可重试错误（Authentication）

### 3. 测试策略
- 单元测试：Mock HTTP/Command（无需真实 API）
- 集成测试：Feature-gated（需真实 API key）
- 基准测试：性能关键路径（router <1ms, memory <50ms）

### 4. 性能考虑
- HTTP client 复用（每个 Provider 实例持有一个）
- 异步非阻塞（tokio async/await）
- 记忆检索优化（embedding <100ms, search <50ms）

### 5. 安全与隐私
- API key 明文存储（Phase 5），迁移到 Keychain（Phase 6）
- 记忆数据本地存储（不上传云端）
- PII 过滤（Phase 6+）

---

## 开发命令速查

### 编译与测试
```bash
cd Aether/core

# 编译 Rust core
cargo build
cargo build --release

# 运行所有测试
cargo test

# 运行特定模块测试
cargo test providers
cargo test providers::openai
cargo test router  # Phase 5

# 运行基准测试
cargo bench

# 代码质量检查
cargo clippy
cargo fmt
```

### UniFFI Bindings（Phase 9）
```bash
# 生成 Swift bindings
cargo run --bin uniffi-bindgen generate src/aether.udl \
  --language swift \
  --out-dir ../Sources/Generated/

# 复制 dylib 到 Frameworks
cp target/release/libaethecore.dylib ../Frameworks/
```

### macOS 构建（Phase 9）
```bash
# 生成 Xcode 项目
xcodegen generate

# 打开 Xcode
open Aether.xcodeproj

# 命令行构建
xcodebuild -project Aether.xcodeproj -scheme Aether build
```

---

## 关键文件位置

### 核心代码
- `Aether/core/src/providers/` - AI Provider 实现
- `Aether/core/src/router/` - Router 实现（待添加）
- `Aether/core/src/memory/` - Memory 模块（已实现，待集成）
- `Aether/core/src/core.rs` - AetherCore（待集成 Router）

### 配置
- `Aether/core/Cargo.toml` - Rust 依赖
- `project.yml` - XcodeGen 配置
- `config.example.toml` - 配置示例（待创建）

### 文档
- `CLAUDE.md` - 项目总体指南
- `openspec/changes/integrate-ai-providers/` - OpenSpec 变更文档
- `Aether/core/PHASE2-4_IMPLEMENTATION_SUMMARY.md` - Phase 2-4 总结

---

## 下次会话建议

1. **立即开始 Phase 5**（Router Implementation）
   - 创建 `src/router/mod.rs`
   - 定义 `RoutingRule` 和 `Router` 结构
   - 实现 first-match 路由逻辑
   - 编写全面的 Router 测试

2. **参考现有代码模式**
   - Provider trait 设计 → Router trait 设计
   - ProviderConfig → RoutingRule Config
   - OpenAI tests → Router tests

3. **使用 OpenSpec**
   - `openspec show integrate-ai-providers` 查看完整 proposal
   - `openspec/changes/integrate-ai-providers/tasks.md` 查看详细任务

4. **测试驱动开发**
   - 先写测试用例（明确预期行为）
   - 实现功能（满足测试）
   - 重构优化（保持测试通过）

---

**最后更新**: 2025-12-24
**当前 Phase**: Phase 5 (Router) - 待开始
**联系人**: Claude via Claude Code
