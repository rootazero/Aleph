# Aether 媒体生成供应商支持设计

**日期**: 2026-01-17
**状态**: 已批准
**作者**: Claude (Brainstorming Session)

---

## 概述

为 Aether 添加图像/视频/音频生成供应商支持，使用户能够通过自然语言或命令直接生成多媒体内容。

## 需求总结

| 维度 | 决策 |
|------|------|
| **触发方式** | 混合模式 - AI 工具调用 + 独立命令 |
| **供应商** | OpenAI (DALL·E/TTS)、Stability AI、Replicate、Banana、OpenAI 兼容 API |
| **输出处理** | 混合策略 - 小文件粘贴，大文件保存 |
| **参数指定** | 全栈 - 自然语言 + 命令参数 + 交互式 + 配置默认值 |
| **路由策略** | 默认值 + 显式指定 + 智能路由 + 成本优先 |
| **架构** | 组合模式 - 新 `GenerationProvider` trait + 共享基础设施 |
| **异步处理** | 混合策略 - 快速任务阻塞，慢任务后台+通知 |
| **错误处理** | 智能重试 → 自动 fallback → 报错 |

---

## 第一部分：核心 Trait 设计

新建 `GenerationProvider` trait，与 `AiProvider` 并列但独立：

```rust
// core/src/generation/mod.rs

/// 生成类型枚举
pub enum GenerationType {
    Image,
    Video,
    Audio,
    Speech,  // TTS
}

/// 生成结果
pub struct GenerationOutput {
    pub media_type: GenerationType,
    pub mime_type: String,           // "image/png", "video/mp4", "audio/mp3"
    pub data: GenerationData,        // 二进制或URL
    pub metadata: GenerationMetadata,
}

pub enum GenerationData {
    Bytes(Vec<u8>),                  // 小文件直接返回
    Url(String),                     // 大文件返回URL
    LocalPath(PathBuf),              // 已保存到本地
}

/// 核心 Trait
#[async_trait]
pub trait GenerationProvider: Send + Sync {
    fn name(&self) -> &str;
    fn supported_types(&self) -> Vec<GenerationType>;

    async fn generate(
        &self,
        request: GenerationRequest,
    ) -> Result<GenerationOutput>;

    /// 预估生成时间（用于决定阻塞/后台）
    fn estimate_duration(&self, request: &GenerationRequest) -> Duration;

    /// 检查进度（如果支持）
    async fn check_progress(&self, task_id: &str) -> Result<GenerationProgress>;
}
```

---

## 第二部分：请求与参数结构

统一的生成请求结构，支持参数合并链：

```rust
// core/src/generation/request.rs

/// 生成请求
pub struct GenerationRequest {
    pub generation_type: GenerationType,
    pub prompt: String,
    pub negative_prompt: Option<String>,
    pub parameters: GenerationParams,
    pub source: RequestSource,        // 追踪参数来源
}

/// 参数结构（支持所有供应商的超集）
pub struct GenerationParams {
    // 图像参数
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub aspect_ratio: Option<String>,  // "16:9", "1:1"
    pub style: Option<String>,         // "vivid", "natural", "anime"
    pub quality: Option<String>,       // "standard", "hd"
    pub num_outputs: Option<u32>,      // 生成数量

    // 视频参数
    pub duration_seconds: Option<f32>,
    pub fps: Option<u32>,
    pub motion_strength: Option<f32>,

    // 音频参数
    pub sample_rate: Option<u32>,
    pub voice_id: Option<String>,      // TTS 声音
    pub speed: Option<f32>,

    // 通用参数
    pub seed: Option<i64>,             // 可复现性
    pub guidance_scale: Option<f32>,   // CFG scale
    pub model: Option<String>,         // 覆盖默认模型

    // 原始参数透传
    pub extra: Option<serde_json::Value>,
}

/// 参数来源追踪（用于调试和合并优先级）
pub struct RequestSource {
    pub config_defaults: GenerationParams,   // 最低优先级
    pub command_args: Option<GenerationParams>,
    pub ai_parsed: Option<GenerationParams>,
    pub user_override: Option<GenerationParams>, // 最高优先级
}

impl GenerationParams {
    /// 合并参数链：config → command → ai → user
    pub fn merge_chain(sources: &RequestSource) -> Self { ... }
}
```

---

## 第三部分：供应商实现架构

### 目录结构

```
core/src/generation/
├── mod.rs              # trait 定义、类型导出
├── request.rs          # GenerationRequest/Params
├── output.rs           # GenerationOutput/Data
├── registry.rs         # GenerationProviderRegistry
├── router.rs           # 智能路由逻辑
├── providers/
│   ├── mod.rs
│   ├── openai.rs       # DALL·E 3 + TTS
│   ├── stability.rs    # Stable Diffusion/Video/Audio
│   ├── replicate.rs    # Replicate 统一 API
│   ├── banana.rs       # Banana serverless
│   └── openai_compat.rs # OpenAI 兼容 API（通用）
```

### OpenAI 兼容供应商（支持第三方代理）

```rust
// core/src/generation/providers/openai_compat.rs

pub struct OpenAICompatGenerationProvider {
    name: String,
    config: GenerationProviderConfig,
    client: reqwest::Client,
}

impl OpenAICompatGenerationProvider {
    pub fn new(name: &str, config: GenerationProviderConfig) -> Result<Self> {
        // 支持任意 base_url，如：
        // - https://api.openai.com (官方)
        // - https://api.proxyvendor.com (第三方代理)
        // - http://localhost:8080 (本地服务)
    }
}

impl GenerationProvider for OpenAICompatGenerationProvider {
    async fn generate(&self, request: GenerationRequest) -> Result<GenerationOutput> {
        // POST {base_url}/v1/images/generations
        // 自动适配不同供应商的响应格式差异
    }
}
```

### Replicate 供应商（统一访问多模型）

```rust
// core/src/generation/providers/replicate.rs

pub struct ReplicateProvider {
    config: GenerationProviderConfig,
    model_mappings: HashMap<String, String>, // "flux" → "black-forest-labs/flux-schnell"
}

impl GenerationProvider for ReplicateProvider {
    async fn generate(&self, request: GenerationRequest) -> Result<GenerationOutput> {
        // 1. POST /predictions 创建任务
        // 2. 轮询 GET /predictions/{id} 或使用 webhook
        // 3. 返回结果
    }

    fn estimate_duration(&self, request: &GenerationRequest) -> Duration {
        // 根据模型类型估算：flux-schnell ~5s, sdxl ~15s, video ~120s
    }
}
```

---

## 第四部分：配置系统设计

### config.toml 结构

```toml
# ============ 生成供应商配置 ============

[generation]
default_image_provider = "dalle3"
default_video_provider = "stability"
default_audio_provider = "stability"
default_speech_provider = "openai-tts"

# 输出设置
output_dir = "~/Downloads/Aether/generated"
auto_paste_threshold_mb = 5        # 小于 5MB 自动粘贴
background_task_threshold_seconds = 30  # 超过 30s 转后台

# ============ 具体供应商 ============

[generation.providers.dalle3]
provider_type = "openai"
api_key = "sk-..."
base_url = "https://api.openai.com"
model = "dall-e-3"
enabled = true
color = "#10a37f"

[generation.providers.dalle3.defaults]
size = "1024x1024"
quality = "standard"
style = "vivid"

# OpenAI TTS
[generation.providers.openai-tts]
provider_type = "openai"
api_key = "sk-..."
model = "tts-1"
capabilities = ["speech"]

[generation.providers.openai-tts.defaults]
voice = "alloy"
speed = 1.0

# Stability AI
[generation.providers.stability]
provider_type = "stability"
api_key = "sk-..."
enabled = true
color = "#8b5cf6"
capabilities = ["image", "video", "audio"]

[generation.providers.stability.defaults]
image_model = "stable-diffusion-xl-1024-v1-0"
video_model = "stable-video-diffusion"

# Replicate
[generation.providers.replicate]
provider_type = "replicate"
api_key = "r8_..."
enabled = true
color = "#f59e0b"

[generation.providers.replicate.models]
flux = "black-forest-labs/flux-schnell"
sdxl = "stability-ai/sdxl"
musicgen = "meta/musicgen"

# 第三方 OpenAI 兼容代理
[generation.providers.my-proxy]
provider_type = "openai_compat"
api_key = "sk-proxy-..."
base_url = "https://api.myproxy.com"
model = "dall-e-3"
capabilities = ["image"]
color = "#06b6d4"
```

### Rust 配置类型

```rust
// core/src/config/types/generation.rs

pub struct GenerationConfig {
    pub default_image_provider: Option<String>,
    pub default_video_provider: Option<String>,
    pub default_audio_provider: Option<String>,
    pub default_speech_provider: Option<String>,
    pub output_dir: PathBuf,
    pub auto_paste_threshold_mb: u32,
    pub background_task_threshold_seconds: u32,
    pub providers: HashMap<String, GenerationProviderConfig>,
}

pub struct GenerationProviderConfig {
    pub provider_type: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub enabled: bool,
    pub color: String,
    pub capabilities: Vec<GenerationType>,
    pub defaults: GenerationParams,
    pub models: Option<HashMap<String, String>>,
}
```

---

## 第五部分：工具集成与路由

### Native Tool 注册

```rust
// core/src/tools/generation.rs

pub struct ImageGenerateTool {
    registry: Arc<GenerationProviderRegistry>,
}

impl AgentTool for ImageGenerateTool {
    fn name(&self) -> &str { "image_generate" }

    fn description(&self) -> &str {
        "Generate images from text descriptions. Supports DALL·E 3, Stable Diffusion, Flux, etc."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "description": "Image description" },
                "negative_prompt": { "type": "string" },
                "size": { "type": "string", "enum": ["1024x1024", "1792x1024", "1024x1792"] },
                "style": { "type": "string", "enum": ["vivid", "natural", "anime"] },
                "provider": { "type": "string", "description": "Override default provider" }
            },
            "required": ["prompt"]
        })
    }

    async fn execute(&self, params: Value) -> Result<ToolOutput> {
        // 1. 解析参数
        // 2. 路由到正确的 provider
        // 3. 执行生成
        // 4. 处理输出（粘贴/保存）
    }
}
```

### Builtin 命令注册

```rust
// core/src/dispatcher/registry.rs

fn register_generation_tools(&self) {
    self.register(UnifiedTool {
        id: "builtin:generate-image".into(),
        name: "generate-image".into(),
        display_name: "Generate Image".into(),
        description: "Generate image from text prompt".into(),
        source: ToolSource::Builtin,
        routing_regex: Some(r"^/(generate-image|画图|生成图片)\s+".into()),
        routing_intent_type: Some("ImageGeneration".into()),
        ..Default::default()
    });

    self.register(UnifiedTool {
        id: "builtin:generate-video".into(),
        name: "generate-video".into(),
        // ...
    });

    self.register(UnifiedTool {
        id: "builtin:generate-audio".into(),
        // ...
    });
}
```

### 智能路由器

```rust
// core/src/generation/router.rs

pub struct GenerationRouter {
    registry: Arc<GenerationProviderRegistry>,
    config: GenerationConfig,
}

impl GenerationRouter {
    pub async fn route(&self, request: &GenerationRequest) -> Result<Arc<dyn GenerationProvider>> {
        // 1. 显式指定 → 直接返回
        if let Some(provider) = &request.parameters.provider {
            return self.registry.get(provider);
        }

        // 2. 智能路由（可选）
        if self.config.smart_routing_enabled {
            if let Some(provider) = self.smart_select(request).await? {
                return Ok(provider);
            }
        }

        // 3. 默认 provider
        let default = match request.generation_type {
            GenerationType::Image => &self.config.default_image_provider,
            GenerationType::Video => &self.config.default_video_provider,
            // ...
        };

        self.registry.get(default.as_ref().ok_or(...)?)
    }

    async fn smart_select(&self, request: &GenerationRequest) -> Result<Option<...>> {
        // 动漫风格 → 优先 SD/NAI
        // 写实照片 → 优先 DALL·E/Flux
    }
}
```

---

## 第六部分：异步任务管理与输出处理

### 任务管理器

```rust
// core/src/generation/task_manager.rs

pub struct GenerationTaskManager {
    config: GenerationConfig,
    active_tasks: Arc<RwLock<HashMap<String, GenerationTask>>>,
    event_handler: Arc<dyn AetherEventHandler>,
}

pub struct GenerationTask {
    pub id: String,
    pub request: GenerationRequest,
    pub provider_name: String,
    pub status: TaskStatus,
    pub progress: Option<f32>,
    pub created_at: Instant,
    pub estimated_duration: Duration,
}

pub enum TaskStatus {
    Pending,
    Running { progress: f32 },
    Completed(GenerationOutput),
    Failed(AetherError),
}

impl GenerationTaskManager {
    pub async fn execute(&self, request: GenerationRequest) -> Result<ExecutionResult> {
        let provider = self.router.route(&request).await?;
        let estimated = provider.estimate_duration(&request);

        if estimated <= Duration::from_secs(self.config.background_task_threshold_seconds as u64) {
            self.execute_blocking(provider, request).await
        } else {
            self.execute_background(provider, request).await
        }
    }

    async fn execute_blocking(&self, ...) -> Result<ExecutionResult> {
        self.event_handler.on_generation_started(&task);
        let output = provider.generate(request).await?;
        self.event_handler.on_generation_completed(&task, &output);
        Ok(ExecutionResult::Immediate(output))
    }

    async fn execute_background(&self, ...) -> Result<ExecutionResult> {
        let task_id = uuid::Uuid::new_v4().to_string();
        self.active_tasks.write().await.insert(task_id.clone(), task);

        tokio::spawn(async move {
            let result = provider.generate(request).await;
            self.notify_completion(task_id, result).await;
        });

        Ok(ExecutionResult::Background { task_id })
    }
}

pub enum ExecutionResult {
    Immediate(GenerationOutput),
    Background { task_id: String },
}
```

### 输出处理器

```rust
// core/src/generation/output_handler.rs

pub struct OutputHandler {
    config: GenerationConfig,
    clipboard: Arc<dyn ClipboardManager>,
}

impl OutputHandler {
    pub async fn handle(&self, output: GenerationOutput) -> Result<OutputAction> {
        let size_mb = output.size_bytes() as f32 / 1_000_000.0;

        if size_mb <= self.config.auto_paste_threshold_mb as f32 {
            self.paste_to_clipboard(&output).await?;
            Ok(OutputAction::Pasted)
        } else {
            let path = self.save_to_disk(&output).await?;
            Ok(OutputAction::Saved { path })
        }
    }

    async fn save_to_disk(&self, output: &GenerationOutput) -> Result<PathBuf> {
        let filename = format!(
            "{}_{}.{}",
            output.media_type.as_str(),
            chrono::Local::now().format("%Y%m%d_%H%M%S"),
            output.extension()
        );
        let path = self.config.output_dir.join(&filename);
        tokio::fs::create_dir_all(&self.config.output_dir).await?;
        let bytes = output.to_bytes().await?;
        tokio::fs::write(&path, bytes).await?;
        Ok(path)
    }
}
```

---

## 第七部分：错误处理与 Fallback 策略

### 错误类型

```rust
// core/src/generation/error.rs

#[derive(Debug, Clone)]
pub enum GenerationError {
    // 可重试错误
    RateLimited { retry_after: Option<Duration> },
    ServiceUnavailable,
    Timeout,
    NetworkError(String),

    // 需用户干预
    ContentFiltered { reason: String, suggestion: Option<String> },
    PromptTooLong { max_length: usize },
    UnsupportedFormat { requested: String, supported: Vec<String> },

    // 配置/认证错误
    InvalidApiKey,
    QuotaExceeded,
    ProviderNotFound(String),

    // 不可恢复
    InternalError(String),
}

impl GenerationError {
    pub fn is_retryable(&self) -> bool { ... }
    pub fn needs_user_action(&self) -> bool { ... }
    pub fn should_fallback(&self) -> bool { ... }
}
```

### 重试与 Fallback 执行器

```rust
// core/src/generation/executor.rs

pub struct GenerationExecutor {
    registry: Arc<GenerationProviderRegistry>,
    router: GenerationRouter,
    config: GenerationConfig,
    event_handler: Arc<dyn AetherEventHandler>,
}

impl GenerationExecutor {
    pub async fn execute_with_fallback(
        &self,
        request: GenerationRequest,
    ) -> Result<GenerationOutput> {
        let mut last_error = None;
        let mut tried_providers = Vec::new();
        let providers = self.get_provider_chain(&request)?;

        for provider in providers {
            if tried_providers.contains(&provider.name()) {
                continue;
            }
            tried_providers.push(provider.name().to_string());

            match self.try_generate(&provider, &request).await {
                Ok(output) => return Ok(output),
                Err(e) => {
                    last_error = Some(e.clone());

                    if e.needs_user_action() {
                        return Err(self.handle_user_action_error(e, &request).await);
                    }

                    if e.should_fallback() {
                        self.event_handler.on_generation_fallback(provider.name(), &e);
                        continue;
                    }

                    return Err(e.into());
                }
            }
        }

        Err(last_error.unwrap_or(GenerationError::ProviderNotFound("No available provider".into())).into())
    }

    async fn try_generate(&self, provider: &Arc<dyn GenerationProvider>, request: &GenerationRequest) -> Result<GenerationOutput, GenerationError> {
        let max_retries = 3;
        let mut attempt = 0;

        loop {
            attempt += 1;
            match provider.generate(request.clone()).await {
                Ok(output) => return Ok(output),
                Err(e) if e.is_retryable() && attempt < max_retries => {
                    let delay = Duration::from_millis(500 * 2_u64.pow(attempt - 1));
                    let delay = match &e {
                        GenerationError::RateLimited { retry_after: Some(d) } => *d,
                        _ => delay,
                    };
                    tokio::time::sleep(delay).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }
}
```

---

## 第八部分：UniFFI 接口与 Swift 集成

### UDL 接口定义

```udl
// core/src/aether.udl (新增部分)

enum GenerationType {
    "Image",
    "Video",
    "Audio",
    "Speech",
};

dictionary GenerationParams {
    u32? width;
    u32? height;
    string? aspect_ratio;
    string? style;
    string? quality;
    u32? num_outputs;
    f32? duration_seconds;
    string? voice_id;
    f32? speed;
    i64? seed;
    string? provider;
};

dictionary GenerationRequest {
    GenerationType generation_type;
    string prompt;
    string? negative_prompt;
    GenerationParams parameters;
};

[Enum]
interface GenerationResult {
    Immediate(GenerationOutput output);
    Background(string task_id);
};

dictionary GenerationOutput {
    GenerationType media_type;
    string mime_type;
    sequence<u8>? bytes;
    string? url;
    string? local_path;
    u64 size_bytes;
};

callback interface AetherEventHandler {
    // 生成相关回调
    void on_generation_started(string task_id, GenerationType gen_type, string prompt);
    void on_generation_progress(string task_id, f32 progress);
    void on_generation_completed(string task_id, GenerationOutput output);
    void on_generation_failed(string task_id, string error);
    void on_generation_fallback(string from_provider, string to_provider, string reason);
    void on_content_filtered(string prompt, string reason, string? suggestion);
};

interface AetherCore {
    [Throws=AetherError]
    GenerationResult generate(GenerationRequest request);

    [Throws=AetherError]
    GenerationProgress get_generation_progress(string task_id);

    [Throws=AetherError]
    void cancel_generation(string task_id);

    sequence<string> list_generation_providers();
    sequence<GenerationType> get_provider_capabilities(string provider_name);
};
```

### Swift 集成

```swift
// Aether/Sources/GenerationHandler.swift

class GenerationHandler: ObservableObject {
    @Published var activeTask: GenerationTask?
    @Published var progress: Float = 0
    @Published var status: GenerationStatus = .idle

    private let core: AetherCore
    private let haloWindow: HaloWindow

    func generate(_ request: GenerationRequest) async {
        do {
            let result = try core.generate(request)

            switch result {
            case .immediate(let output):
                await handleOutput(output)
            case .background(let taskId):
                await showBackgroundNotification(taskId: taskId)
            }
        } catch {
            await showError(error)
        }
    }
}

// EventHandler 扩展
extension EventHandler: AetherEventHandler {
    func onGenerationStarted(taskId: String, genType: GenerationType, prompt: String) {
        DispatchQueue.main.async {
            self.haloWindow.showProgress(prompt: prompt)
        }
    }

    func onGenerationProgress(taskId: String, progress: Float) {
        DispatchQueue.main.async {
            self.haloWindow.updateProgress(progress)
        }
    }

    func onGenerationCompleted(taskId: String, output: GenerationOutput) {
        let notification = UNMutableNotificationContent()
        notification.title = "生成完成"
        notification.body = "您的\(output.mediaType.displayName)已生成"
        // ...
    }
}
```

---

## 第九部分：实现路线图

### 新增/修改文件清单

```
Aether/core/src/
├── generation/                     # 【新增目录】
│   ├── mod.rs                      # 模块导出、trait 定义
│   ├── request.rs                  # GenerationRequest/Params
│   ├── output.rs                   # GenerationOutput/Data
│   ├── error.rs                    # GenerationError
│   ├── registry.rs                 # GenerationProviderRegistry
│   ├── router.rs                   # GenerationRouter
│   ├── executor.rs                 # GenerationExecutor
│   ├── task_manager.rs             # GenerationTaskManager
│   ├── output_handler.rs           # OutputHandler
│   └── providers/
│       ├── mod.rs                  # provider 工厂
│       ├── openai.rs               # DALL·E 3 + TTS
│       ├── stability.rs            # Stability AI
│       ├── replicate.rs            # Replicate
│       ├── banana.rs               # Banana
│       └── openai_compat.rs        # OpenAI 兼容 API
├── config/types/
│   └── generation.rs               # 【新增】
├── tools/
│   └── generation.rs               # 【新增】
├── dispatcher/
│   └── registry.rs                 # 【修改】
├── lib.rs                          # 【修改】
└── aether.udl                      # 【修改】

Aether/Sources/
├── GenerationHandler.swift         # 【新增】
├── EventHandler.swift              # 【修改】
└── HaloWindow.swift                # 【修改】
```

### 分阶段计划

| 阶段 | 内容 | 依赖 |
|------|------|------|
| **Phase 1** | 核心框架：trait、类型、registry | - |
| **Phase 2** | OpenAI 供应商：DALL·E 3 + openai_compat | Phase 1 |
| **Phase 3** | 工具集成：Native Tools + Builtin 命令 | Phase 2 |
| **Phase 4** | Swift 集成：Handler + EventHandler + Halo UI | Phase 3 |
| **Phase 5** | 完整供应商：Stability + Replicate + Banana | Phase 4 |
| **Phase 6** | 高级特性：智能路由 + 完整 fallback + 后台任务 | Phase 5 |

---

## 关键设计原则

1. **组合模式**：`GenerationProvider` trait 独立，但共享配置基础设施
2. **参数合并链**：config → command → ai → user，优先级递增
3. **混合输出策略**：小文件粘贴，大文件保存
4. **混合异步策略**：快任务阻塞，慢任务后台+通知
5. **智能重试 + Fallback**：自动恢复，减少用户干预
6. **OpenAI 兼容 API**：复用 `base_url` 模式，支持第三方代理
