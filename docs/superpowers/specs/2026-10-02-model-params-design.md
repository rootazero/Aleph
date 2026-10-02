# Per-Model 参数管理重构设计

**日期**: 2026-10-02  
**状态**: 待审阅  
**范围**: Core config schema · ModelRecord 连线 · 计价 tier · Panel UI · RPC wire 类型

---

## 一、背景与目标

Aleph 当前的 `ProviderConfig`（`src/config/types/provider.rs:133`）只在 provider 级存储连接参数和生成参数，`models` 字段仅是 `Vec<String>` id 列表。per-model 的 contextWindow、maxTokens、cost+tiers、input 模态、reasoning 标志、thinking 档位映射和 compat 兼容开关全部缺失，导致：

1. **自定义模型能力未知**：命中不了静态前缀表的模型，`capabilities = None`，落到 128K 保守值（`capabilities.rs:1648`）。
2. **计费永远 Unknown**：自定义模型和新上线模型的 cost 只能等发版刷新。
3. **同一 provider 多模型参数冲突**：`context_window`、`max_tokens`、`thinking_level` 是 provider 级单值，对下面所有模型一刀切。
4. **Panel 无法编辑**：`ProviderConfigJson`（`shared/protocol/src/providers/wire.rs:255`）是有损子集，`thinking_level`、`effort`、`context_window` 等 Panel 均无入口（`detail_panel.rs:235` 写死 `context_window: None`）。

**目标**：在 `config.toml` 内嵌 per-model 参数（对齐 pi 的 `models.json` 数据模型），通过 `ModelRecord::resolve` 唯一 join 点接入，让压缩预算、计费、picker、failover 所有下游自动生效。compat 按实际 adapter 消费者裁剪，不引入死配置。

---

## 二、不做什么（本轮范围外）

- 独立的 `~/.aleph/models.json` 文件：存储走 config.toml 内嵌，避免双配置源冲突。
- 全量对齐 pi 的 ~30 个 compat 键：只做有 Aleph adapter 生产消费者的键（约 8–12 个），其余留给后续轮次。
- 破坏性重写：四表 join 结构不变，`ModelRecord::resolve` 的下游消费方不需要改签名。
- vault key 结构变更：chat/gen/rerank 三套前缀保留原状（属于独立重构）。

---

## 三、Schema 设计

### 3.1 新类型 `ModelDef`（`src/config/types/provider.rs`）

```toml
# 示例：config.toml 中的用法
[[providers.my-openai.model_defs]]
id = "gpt-6.1-sol"
name = "GPT-6.1 Sol"
context_window = 1050000
max_output_tokens = 128000
input = ["text", "image"]
reasoning = true

[providers.my-openai.model_defs.cost]
input = 2.0        # $/M tokens
output = 10.0
cache_read = 0.1
cache_write = 2.5

[[providers.my-openai.model_defs.cost.tiers]]
input_tokens_above = 272000
input = 4.0
output = 15.0
cache_read = 0.2
cache_write = 5.0

[providers.my-openai.model_defs.thinking_level_map]
off = ""       # 空字符串表示该档不支持，null 等价
low = "low"
medium = "medium"
high = "high"

[providers.my-openai.model_defs.compat]
max_tokens_field = "max_completion_tokens"
supports_temperature = false
```

**Rust 结构**（新增，`src/config/types/provider.rs`）：

```rust
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ModelDef {
    pub id: String,
    pub name: Option<String>,
    pub base_url: Option<String>,           // 模型级 baseUrl，覆盖 provider 级
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub input: Option<Vec<InputModality>>,  // "text" | "image"
    pub reasoning: Option<bool>,
    pub thinking_level_map: Option<ThinkingLevelMap>,
    pub cost: Option<ModelCost>,
    pub compat: Option<ModelCompat>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum InputModality { Text, Image }

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ModelCost {
    pub input: Option<f64>,           // $/M tokens
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
    pub reasoning: Option<f64>,
    pub tiers: Option<Vec<CostTier>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CostTier {
    pub input_tokens_above: u64,
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

// ThinkingLevelMap: off/minimal/low/medium/high/xhigh/max → Option<String>
// None 表示未声明（使用 provider 级 thinking_level）
// Some("") 或 Some(特定值) 表示映射；映射到空字符串表示该档不支持
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ThinkingLevelMap {
    pub off: Option<String>,
    pub minimal: Option<String>,
    pub low: Option<String>,
    pub medium: Option<String>,
    pub high: Option<String>,
    pub xhigh: Option<String>,
    pub max: Option<String>,
}
```

**`ModelCompat`**（键名单在实施第一步核查 adapter 后确定，预计包含）：

```rust
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ModelCompat {
    /// "max_tokens" | "max_completion_tokens"
    pub max_tokens_field: Option<String>,
    pub supports_temperature: Option<bool>,
    /// "openai" | "deepseek" | "qwen" | "string-thinking" | ...
    pub thinking_format: Option<String>,
    pub supports_developer_role: Option<bool>,
    pub supports_reasoning_effort: Option<bool>,
    pub supports_vision: Option<bool>,
    pub cache_control_format: Option<String>,  // "anthropic"
    pub supports_tools: Option<bool>,
    pub supports_mid_convo_system_messages: Option<bool>,
}
```

**`ProviderConfig` 改动**：

```rust
// 原有字段不变；新增：
pub model_defs: Vec<ModelDef>,   // 默认空，[[providers.x.model_defs]] TOML 语法
```

旧的 `models: Vec<String>` 保持不变，表示 failover 梯。`model_defs` 只补充元数据，按 id 匹配覆盖。老配置零迁移。

**缺省值**（与 pi 对齐）：`context_window` 未设时 = 128000，`max_output_tokens` 未设时 = 16384，`cost` 未设时 = 全 0，`input` 未设时 = `["text"]`，`reasoning` 未设时 = false。

**校验**（`src/config/validate.rs`）：
- `id` 不能为空
- `context_window` 和 `max_output_tokens` 若设置必须 > 0
- `cost.tiers` 中 `input_tokens_above` 必须严格递增
- 报错格式参照 pi：`Provider X, model Y: context_window must be > 0`

### 3.2 兼容旧字段

`ProviderConfig` 上的 provider 级 `context_window`、`max_tokens`、`thinking_level` 保留，但优先级低于 `model_defs` 中对应字段。优先级链：

```
model_defs[id] > capabilities.rs CAPABILITY_TABLE > provider 级 config > 128K 保守默认
```

---

## 四、连线设计

### 4.1 `ModelRecord::resolve`（`src/providers/model_catalog/record.rs:73`）

在 `resolve` 的唯一 join 点插入 `model_defs` 作为最高优先级层：

```
resolve(provider_id, model_id, base_url, source, model_defs: Option<&ModelDef>)
```

- `model_def.context_window` → 覆盖 `ModelCapabilities::context_window`
- `model_def.max_output_tokens` → 覆盖 `max_output_tokens`
- `model_def.input` → 覆盖 `supports_vision`（含 image 即为 true）
- `model_def.reasoning` → 覆盖 `supports_reasoning`
- `model_def.cost` → 构造运行时 `RateCard`，覆盖静态 `PRICE_TABLE` 查找结果
- `model_def.cost.tiers` → 构造运行时 `Vec<PriceTier>`，覆盖静态 `TIER_TABLE`

调用 `resolve` 的所有上层（`create_provider`、`start/helpers.rs`）穿 `model_defs`，无 override 时传 `None`，与旧行为等价。

### 4.2 计价 tier 运行时化（`src/pricing.rs`）

`PriceTier` 当前是 `&'static`，改为支持运行时 `Vec<PriceTier>` 传入：

- `Rates` 新增 `tiers: Option<Vec<PriceTier>>`
- `price_for_tokens` 的 tier 查找先看 `Rates.tiers`，为空时 fallback 到静态 `TIER_TABLE`

这是局部改动，不影响没有 model_defs 的现有 provider。

### 4.3 压缩预算（`src/context/budget/`）

`derive_chain_min_budget` 和 `ContextBudgetRefiner` 已经通过 `resolve_context_window_with_override`（`capabilities.rs:1666`）读窗口，不需要额外改动——`model_defs` 进 `resolve` 后自动生效。

### 4.4 preset 回填收敛

`start/helpers.rs:663 build_http_provider` 与 `providers::create_provider` 各有一份 preset 回填逻辑（已在 Aleph subagent 调研中确认为重复）。本次收敛成 `apply_preset_defaults(config: &mut ProviderConfig, preset: &ProviderPreset)`，两处共用，删掉其中一份。这是范围内清理（熵减，符合 R10）。

---

## 五、线上协议与 RPC

### 5.1 `ProviderConfigJson` 补全（`shared/protocol/src/providers/wire.rs:255`）

新增字段（当前缺失）：

```rust
pub model_defs: Option<Vec<ModelDefJson>>,
pub thinking_level: Option<String>,
pub effort: Option<String>,
pub cache_retention: Option<String>,
pub context_window: Option<u64>,   // 去掉 detail_panel.rs:235 的写死 None
pub top_p: Option<f64>,
pub top_k: Option<u32>,
```

`ModelDefJson` 对齐 `ModelDef`，可直接序列化。

### 5.2 handlers（`src/gateway/handlers/providers/handlers.rs`）

`providers.update` 在合并时不再丢失 `model_defs`、`thinking_level`、`effort` 等字段（当前注释说明靠合并保住 wire 未覆盖字段，本次直接补全 wire）。

### 5.3 R8 工具面

`providers.update` RPC 对应的 provider 管理工具在补全 wire 类型后，自然支持 LLM 用自然语言配置 per-model 参数（R8 合规）。后续实施时确认现有工具描述覆盖新字段，若有遗漏补全 `DESCRIPTION`（R9：工具有了，prompt 就不教这个）。

---

## 六、Panel UI（`interfaces/webchat`）

### 6.1 模型参数编辑器

在 `detail_panel.rs` 的模型梯（`model_ladder.rs`）下方新增"模型参数"折叠区，每个模型独立展开编辑：

- 窗口（context_window）、输出上限（max_output_tokens）
- 输入模态勾选：文本 / 图片
- reasoning 开关
- thinking 档位映射（off/low/medium/high/xhigh/max → 映射值或"不支持"）
- 价格：input/output/cache_read/cache_write（$/M token），可添加 tier 行
- compat：下拉/开关，只显示本 provider 协议（openai/anthropic/openai-responses）支持的键

UI 显示"值来源"标签（用户设置 / 预设 / 默认），让用户知道哪些参数是自己覆盖的。这是 pi 没有的能力。

### 6.2 修复 `detail_panel.rs:235`

去掉写死的 `context_window: None`，改为从 `ProviderConfigJson` 读取。

---

## 七、执行编排

### 双线并行（worktree 隔离，main 分支只读）

**线 1（Core + Catalog）**，分支 `feat/per-model-params-core`：
1. 核查 openai_chat/openai_responses/anthropic 各 adapter 里的 compat 硬编码分支，确定 `ModelCompat` 最终键名单
2. 新增 `ModelDef`、`ModelCost`、`CostTier`、`ThinkingLevelMap`、`ModelCompat` 类型 + validate
3. `ModelRecord::resolve` 插入 model_defs 层
4. `pricing.rs` PriceTier 运行时化
5. preset 回填收敛（删重复）
6. 单元测试 + `cargo clippy -D warnings`

**线 2（Protocol + Panel）**，分支 `feat/per-model-params-panel`，等线 1 schema 合并后启动：
1. `ProviderConfigJson` 补全字段
2. handlers 补全字段传递
3. Panel 模型参数编辑器
4. 修复 `detail_panel.rs:235`
5. E2E 可用性验证

**内存守卫**：每次 cargo 前检查 MemAvailable（`(free + inactive + speculative) × page_size`，与现有 guard 口径一致），低于 4 GiB 等待；2–3 个子任务完成后合并编译，减少编译次数。

**模型分工**：实现类工作（读大量代码、写代码、批量改）派 minimax 子代理；复杂推理和架构决策用 opus/sonnet；分类和路由判断用 Jev（实施阶段 adapter 键名单扫描步骤会用到 Jev 分类）。

---

## 八、清理（熵减）

本轮同步删除：
- `start/helpers.rs` 中重复的 preset 回填逻辑（保留 `create_provider` 版本，删另一份）
- `detail_panel.rs:235` 的写死 `context_window: None`

不在本轮删除：
- `PRICE_TABLE`/`TIER_TABLE` 的 `const` 条目（继续存在，用户 model_defs 优先级更高，非死代码）
- 三套 vault key 前缀（独立重构，范围外）

---

## 九、未解决的问题（实施前需确认）

1. **compat 键名单**：实施第一步核查 adapter 后确定，预计 8–12 个。
2. **`PriceTier` 改动波及面**：`Rates` 有多少调用点需要接 `Option<Vec<PriceTier>>`，需小实验确认，若波及超过预期则先只做 model_defs 层的其他字段，tier 单独一轮。
3. **R8 工具面**：确认现有 provider 管理工具的描述是否需要补全 model_defs 相关说明。

---

## 十、文档更新

完成后补充：
- `docs/reference/FEATURE_LOCATOR.md` §158 "Provider & Model Catalog" 新增 per-model override 层
- `docs/reference/MODEL_CATALOG.md` 更新四表 join 点说明，标注 model_defs 优先级

---

*spec 写于 2026-10-02，待用户审阅后进入 writing-plans 阶段*
