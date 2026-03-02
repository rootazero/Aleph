# ~/.aleph 配置外置化设计

> **日期**: 2026-03-02
> **状态**: Approved
> **范围**: 将硬编码的 Provider Presets、System Prompts、默认值外置为 `~/.aleph/` 下的 TOML 文件

---

## 1. 动机

当前 Aleph 有大量配置硬编码在 Rust 源码中，导致：

- **Provider Presets** (15+): 新增提供商需要修改代码并重新编译
- **System Prompts**: Planning、Bootstrap、Scratchpad 模板无法运行时调整
- **默认值** (30+): 分散在 20+ 文件的 `fn default_*()` 中，调参需编译

目标：让编译后的 Aleph 二进制依然有大量参数可修改的空间，无需重新编译。

---

## 2. 设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 文件格式 | 统一 TOML | 与现有 config.toml 一致 |
| 加载策略 | 内置默认 + 文件覆盖 | 文件不存在时不影响启动 |
| 预设合并 | 字段级合并 | 用户可以只覆盖想改的字段 |
| 热重载 | 重启生效 | 实现简单，配置变更不频繁 |
| 架构方案 | 统一扩展文件 (方案 B) | 仅新增 3 个文件，职责清晰 |

---

## 3. 文件结构

```
~/.aleph/
├── config.toml          # 主配置 (现有，不变)
├── presets.toml          # NEW: Provider & Generation 预设
├── prompts.toml          # NEW: 系统提示与模板
└── defaults.toml         # NEW: 默认值覆盖
```

---

## 4. presets.toml — Provider & Generation 预设

### 4.1 文件格式

```toml
# Provider presets (merged with built-in presets)
[providers.my-custom-provider]
base_url = "https://api.example.com/v1"
protocol = "openai"           # "openai" | "anthropic" | "gemini" | "ollama"
color = "#ff6600"
default_model = "gpt-4o"
aliases = ["my-provider", "custom"]

[providers.openai]             # Override built-in: change default model
default_model = "gpt-4-turbo"

# Generation presets
[generation.image.my-image-gen]
base_url = "https://api.example.com/v1/images"
protocol = "openai"
color = "#0088ff"
default_model = "dall-e-3"
timeout_seconds = 120

[generation.video.my-video-gen]
base_url = "https://api.example.com/v1/video"
protocol = "custom"
default_model = "video-v1"

[generation.audio.my-tts]
base_url = "https://api.example.com/v1/audio"
protocol = "openai"
default_model = "tts-1-hd"
```

### 4.2 合并逻辑

```
Built-in presets (Rust code)
    ↓ field-level merge
~/.aleph/presets.toml
    ↓ result
Final presets (runtime)
```

- **同名 key**: 用户值覆盖内置值（字段级合并，非整体替换）
- **新 key**: 直接新增为新提供商
- **禁用内置**: 设置 `enabled = false`

---

## 5. prompts.toml — 系统提示与模板

### 5.1 文件格式

```toml
[planner]
system_prompt = """
You are a planning assistant...
"""

[bootstrap]
identity_phase = """
Welcome! I'm Aleph...
"""
user_phase = """
Now let me learn about you...
"""
calibration_phase = """
Great! Let me calibrate...
"""

[scratchpad]
template = """
# Current Task
## Objective
## Plan
## Working State
## Notes
"""

[memory]
compression_prompt = """
Summarize the following conversation...
"""
extraction_prompt = """
Extract structured facts...
"""

[agent]
system_prefix = """
You are Aleph, a personal AI assistant...
"""
observation_prompt = """
Analyze the following observation...
"""
```

### 5.2 设计原则

- 每个 prompt 是独立字段，按用途拆分
- TOML 多行字符串 (`"""..."""`) 适合长文本
- 所有字段 `Option<String>`，缺失使用内置默认
- 不做变量插值，动态内容由 Rust 代码运行时拼接

---

## 6. defaults.toml — 默认值覆盖

### 6.1 三层优先级链

```
编译默认 (Rust code fn default_*())
    ↓ overridden by
~/.aleph/defaults.toml           ← 用户的"工厂设置"
    ↓ overridden by
~/.aleph/config.toml             ← 用户的运行时配置
```

### 6.2 文件格式

```toml
[memory]
similarity_threshold = 0.75
retention_days = 90
compression_threshold = 0.6
max_facts_per_query = 10
embedding_batch_size = 32

[memory.graph_decay]
half_life_days = 30
min_weight = 0.1

[memory.noise_filter]
min_confidence = 0.3
dedup_similarity = 0.95

[agent]
max_retries = 3
timeout_seconds = 120
max_thinking_depth = 5

[agent.model_routing]
complexity_threshold = 0.7
code_weight = 1.2
reasoning_weight = 1.5

[provider]
timeout_seconds = 60
max_tokens = 4096
temperature = 0.7

[generation]
timeout_seconds = 120

[dispatcher]
max_concurrent_tasks = 5
task_timeout_seconds = 300
dag_max_depth = 10
```

### 6.3 defaults.toml vs config.toml

- `defaults.toml`: 调整"出厂设置"，影响所有未显式配置的参数
- `config.toml`: 运行时具体配置（API key、行为偏好等）

---

## 7. 加载流程

```
Aleph 启动
    │
    ├─ 1a. 加载编译默认值 (Rust fn default_*())
    │
    ├─ 1b. ~/.aleph/defaults.toml 存在?
    │       ├─ YES → 解析并覆盖默认值
    │       └─ NO  → 跳过
    │
    ├─ 1c. ~/.aleph/config.toml 存在?
    │       ├─ YES → 解析并覆盖
    │       └─ NO  → 创建默认 config.toml
    │
    ├─ 1d. ~/.aleph/presets.toml 存在?
    │       ├─ YES → 与内置 presets 合并
    │       └─ NO  → 跳过 (用内置 presets)
    │
    ├─ 1e. ~/.aleph/prompts.toml 存在?
    │       ├─ YES → 覆盖对应 prompt
    │       └─ NO  → 跳过 (用内置 prompts)
    │
    └─ 2. 继续正常启动流程
```

### 错误处理

| 情况 | 行为 |
|------|------|
| 文件不存在 | 静默跳过，使用内置默认 |
| 解析错误 | `warn!()` 日志 + 使用内置默认（不阻塞启动） |
| 字段类型错误 | serde 跳过该字段，使用默认值 |
| 权限错误 | `warn!()` 日志 + 使用内置默认 |

---

## 8. 核心类型

```rust
/// ~/.aleph/presets.toml
#[derive(Debug, Default, Deserialize)]
pub struct PresetsOverride {
    #[serde(default)]
    pub providers: HashMap<String, PartialProviderPreset>,
    #[serde(default)]
    pub generation: GenerationPresetsOverride,
}

#[derive(Debug, Default, Deserialize)]
pub struct GenerationPresetsOverride {
    #[serde(default)]
    pub image: HashMap<String, PartialGenerationPreset>,
    #[serde(default)]
    pub video: HashMap<String, PartialGenerationPreset>,
    #[serde(default)]
    pub audio: HashMap<String, PartialGenerationPreset>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PartialProviderPreset {
    pub base_url: Option<String>,
    pub protocol: Option<String>,
    pub color: Option<String>,
    pub default_model: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub enabled: Option<bool>,
}

/// ~/.aleph/prompts.toml
#[derive(Debug, Default, Deserialize)]
pub struct PromptsOverride {
    #[serde(default)]
    pub planner: Option<PlannerPrompts>,
    #[serde(default)]
    pub bootstrap: Option<BootstrapPrompts>,
    #[serde(default)]
    pub scratchpad: Option<ScratchpadPrompts>,
    #[serde(default)]
    pub memory: Option<MemoryPrompts>,
    #[serde(default)]
    pub agent: Option<AgentPrompts>,
}

/// ~/.aleph/defaults.toml
#[derive(Debug, Default, Deserialize)]
pub struct DefaultsOverride {
    #[serde(default)]
    pub memory: Option<MemoryDefaults>,
    #[serde(default)]
    pub agent: Option<AgentDefaults>,
    #[serde(default)]
    pub provider: Option<ProviderDefaults>,
    #[serde(default)]
    pub generation: Option<GenerationDefaults>,
    #[serde(default)]
    pub dispatcher: Option<DispatcherDefaults>,
}
```

---

## 9. 文件变动清单

| 变动类型 | 文件 | 描述 |
|----------|------|------|
| **新增** | `core/src/config/presets_override.rs` | PresetsOverride 类型 + 加载/合并逻辑 |
| **新增** | `core/src/config/prompts_override.rs` | PromptsOverride 类型 + 加载逻辑 |
| **新增** | `core/src/config/defaults_override.rs` | DefaultsOverride 类型 + 加载逻辑 |
| **修改** | `core/src/config/mod.rs` | 导出新模块 |
| **修改** | `core/src/config/load.rs` | 加载流程增加三个新文件 |
| **修改** | `core/src/config/structs.rs` | Config struct 增加 override 字段 |
| **修改** | `core/src/providers/presets.rs` | `get_preset()` 先查 override 再回退内置 |
| **修改** | `core/src/config/types/generation/presets.rs` | Generation presets 合并逻辑 |
| **修改** | `core/src/dispatcher/planner/prompt.rs` | 从 PromptsOverride 获取 prompt |
| **修改** | `core/src/agent_loop/bootstrap.rs` | 从 PromptsOverride 获取 prompt |
| **修改** | `core/src/memory/scratchpad/template.rs` | 从 PromptsOverride 获取 template |
| **修改** | 各 `fn default_*()` 函数 | 查询 DefaultsOverride 再回退编译默认 |

---

## 10. 实施阶段

| 阶段 | 范围 | 优先级 |
|------|------|--------|
| P1 | presets.toml (Provider + Generation) | 最高 — 最近频繁的痛点 |
| P2 | prompts.toml (System Prompts + Templates) | 高 |
| P3 | defaults.toml (默认值覆盖) | 中 |
