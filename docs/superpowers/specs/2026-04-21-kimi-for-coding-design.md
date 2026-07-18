# Kimi for Coding Provider 集成设计文档

**日期**: 2026-04-21  
**作者**: AI Assistant  
**状态**: 待实现

---

## 1. 背景与目标

### 1.1 背景

Aleph 已经预设了 `moonshot` 和 `kimi` provider（使用 OpenAI 协议），以及 `kimi-for-coding` 和 `kimi-coding` preset（使用 Anthropic 协议）。然而：

1. Panel UI (`preset_data.rs`) 中没有 `kimi-for-coding` 的预设，用户无法通过可视化界面配置
2. 缺少针对 Kimi for Coding 的模型行为优化（temperature、topP、thinking 等）
3. 默认模型需要更新为 `Kimi-K2.6`

参考 opencode 的实现，Kimi for Coding 有以下特点：
- 使用 Anthropic SDK 协议（`@ai-sdk/anthropic`）
- 端点：`https://api.kimi.com/coding/v1`
- 模型ID：`kimi-k2-thinking`, `kimi-k2.5`, `kimi-k2p5`, `Kimi-K2.6`
- 特殊参数：启用 thinking（`thinking: { type: "enabled" }`）
- 温度设置：kimi-k2.5 系列默认 1.0，其他 0.6
- topP 设置：kimi-k2.5 系列为 0.95

### 1.2 目标

1. 在 Panel UI 中添加 `kimi-for-coding` 预设
2. 更新 `presets.rs` 中的默认模型为 `Kimi-K2.6`
3. 在 AnthropicProtocol 中添加 Kimi for Coding 的模型行为优化
4. 确保与 Aleph 现有架构完全兼容

---

## 2. 架构分析

### 2.1 Aleph Provider 架构

```
Provider 层
├── presets.rs          - 预设配置 (base_url, protocol, color)
├── registry.rs         - Provider 注册表
├── mod.rs              - Provider 工厂函数
├── protocols/          - 协议适配器层
│   ├── anthropic.rs   - Anthropic 协议实现
│   ├── openai_chat.rs - OpenAI 协议实现
│   └── ...
├── model_behaviors/    - 模型行为指令
└── model_discovery.rs  - 动态模型发现
```

### 2.2 关键设计原则

1. **协议驱动**：Providers 按协议（OpenAI/Anthropic/Gemini）而非厂商组织
2. **Preset 系统**：通过 `ProviderPreset` 快速配置已知 provider
3. **协议注册表**：`ProtocolRegistry` 动态管理协议适配器
4. **模型行为**：通过 `model_behaviors` 模块加载 per-LLM-family 的行为指令

### 2.3 Panel UI 架构

```
interfaces/webchat/src/
├── preset_data.rs     - UI 预设数据 (PRESETS 数组)
└── views/settings/
    └── providers.rs   - Provider 配置页面
```

---

## 3. 设计方案

### 3.1 方案概述

采用 **"Preset 扩展 + 协议层优化"** 的组合方案：

1. **Preset 层**：更新 `presets.rs` 和 `preset_data.rs`
2. **协议层**：在 `AnthropicProtocol` 中检测 Kimi 模型并应用优化参数
3. **零侵入**：不创建新的 protocol adapter，充分利用现有 Anthropic 协议支持

### 3.2 详细设计

#### 3.2.1 更新 `src/providers/presets.rs`

当前 `kimi-for-coding` preset 已存在且默认模型已是 `Kimi-K2.6`，无需修改。

验证现有配置：
```rust
m.insert(
    "kimi-for-coding",
    ProviderPreset {
        base_url: "https://api.kimi.com/coding/v1",
        protocol: "anthropic",
        color: "#6366f1",
        default_model: "Kimi-K2.6",
    },
);
```

#### 3.2.2 添加 Panel UI 预设 `interfaces/webchat/src/preset_data.rs`

在 `PRESETS` 数组的 `moonshot` 之后添加：

```rust
ProviderPreset {
    name: "kimi-for-coding",
    protocol: "anthropic",
    model: "Kimi-K2.6",
    base_url: "https://api.kimi.com/coding/v1",
    description: "Kimi for Coding - Optimized for IDE/agent tool use",
    api_key_placeholder: "sk-...",
    icon_color: "#6366F1",
    needs_api_key: true,
    auth_type: "api_key",
},
```

#### 3.2.3 AnthropicProtocol 模型优化

在 `src/providers/protocols/anthropic.rs` 的 `AnthropicProtocol` impl 中添加辅助方法：

```rust
/// Detect if model is a Kimi for Coding model
fn is_kimi_model(model: &str) -> bool {
    let m = model.to_lowercase();
    m.contains("kimi-k2") || m.contains("kimi-k2.5") || m.contains("kimi-k2p5")
}

/// Get default temperature for Kimi models
fn kimi_default_temperature(model: &str) -> Option<f32> {
    let m = model.to_lowercase();
    if m.contains("k2.5") || m.contains("k2p5") || m.contains("k2-5") {
        Some(1.0)
    } else if m.contains("kimi-k2") {
        Some(0.6)
    } else {
        None
    }
}

/// Get default topP for Kimi models  
fn kimi_default_top_p(model: &str) -> Option<f32> {
    let m = model.to_lowercase();
    if m.contains("k2.5") || m.contains("k2p5") || m.contains("k2-5") {
        Some(0.95)
    } else {
        None
    }
}
```

在 `build_request` 中应用优化：

```rust
fn build_request(...) -> Result<reqwest::RequestBuilder> {
    // ... existing code ...
    
    // Apply Kimi-specific defaults if not explicitly set
    let temperature = payload.temperature
        .or_else(|| Self::kimi_default_temperature(actual_model))
        .or(config.temperature);
        
    // Enable thinking for Kimi models by default when no explicit think_level
    let thinking = if Self::is_kimi_model(actual_model) && payload.think_level.is_none() {
        Some(ThinkingBlock {
            thinking_type: "enabled".to_string(),
            budget_tokens: Some(16_000),
            display: None,
        })
    } else {
        payload.think_level
            .as_ref()
            .and_then(Self::map_think_level)
            .map(|budget| ThinkingBlock {
                thinking_type: "enabled".to_string(),
                budget_tokens: Some(budget),
                display: None,
            })
    };
    
    // ... rest of the code ...
}
```

#### 3.2.4 错误处理增强

在 AnthropicProtocol 的错误处理中添加 Kimi 特定错误模式：

```rust
// Kimi for Coding specific error patterns
const KIMI_TOKEN_LIMIT_PATTERN: &str = "exceeded model token limit";
```

---

## 4. 实施计划

### 4.1 文件变更清单

| 文件 | 变更类型 | 描述 |
|------|----------|------|
| `src/providers/presets.rs` | 验证 | 确认默认模型为 Kimi-K2.6 |
| `interfaces/webchat/src/preset_data.rs` | 修改 | 添加 kimi-for-coding 到 PRESETS 数组 |
| `src/providers/protocols/anthropic.rs` | 修改 | 添加 Kimi 模型检测和参数优化 |
| `src/providers/protocols/anthropic.rs` | 修改 | 添加 Kimi 错误模式识别 |

### 4.2 测试计划

1. **Preset 测试**：验证 `get_preset("kimi-for-coding")` 返回正确配置
2. **UI 测试**：验证 Panel 中显示 kimi-for-coding 预设
3. **协议测试**：验证 AnthropicProtocol 对 Kimi 模型的参数注入
4. **集成测试**：端到端验证 kimi-for-coding provider 创建和请求构建

### 4.3 回滚计划

所有变更为增量添加，不影响现有功能：
- Preset 变更：移除 PRESETS 数组中的新增项即可
- 协议变更：移除 `is_kimi_model` 相关逻辑即可
- 零数据库迁移，零配置变更

---

## 5. 风险评估

| 风险 | 可能性 | 影响 | 缓解措施 |
|------|--------|------|----------|
| AnthropicProtocol 耦合度增加 | 中 | 低 | 辅助方法独立，不影响其他协议 |
| Kimi 模型ID变更 | 低 | 中 | 使用前缀匹配，兼容多种命名方式 |
| 默认参数与用户需求冲突 | 低 | 低 | payload 显式设置优先于默认值 |

---

## 6. 附录

### 6.1 opencode 参考代码

opencode 中 Kimi 相关处理：
- 文件：`packages/opencode/src/provider/transform.ts`
- 温度设置：`kimi-k2` 系列 0.6，`kimi-k2.5` 系列 1.0
- topP 设置：`kimi-k2.5` 系列 0.95
- Thinking 启用：`thinking: { type: "enabled", budgetTokens }`

### 6.2 Aleph 现有预设

```rust
// 当前 moonshot preset
m.insert("moonshot", ProviderPreset {
    base_url: "https://api.moonshot.ai/v1",
    protocol: "openai",
    color: "#6366f1",
    default_model: "kimi-k2-0905-preview",
});

// 当前 kimi-for-coding preset（已存在）
m.insert("kimi-for-coding", ProviderPreset {
    base_url: "https://api.kimi.com/coding/v1",
    protocol: "anthropic",
    color: "#6366f1",
    default_model: "Kimi-K2.6",
});
```
