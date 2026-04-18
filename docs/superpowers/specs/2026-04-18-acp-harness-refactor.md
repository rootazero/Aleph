# ACP Harness 架构重构设计文档

> 日期: 2026-04-18
> 目标: 消除预设 harness 的代码重复，支持配置驱动的大规模 agent 注册
> 范围: 后端 `src/acp/` + 前端 `interfaces/webchat/src/views/settings/acp_harnesses.rs`

---

## 1. 现状分析

### 1.1 当前预设 Harness

Aleph 当前支持 3 个预设 ACP harness：

| ID | 显示名 | 模式 | 文件 | 行数 |
|---|---|---|---|---|
| `claude-code` | Claude Code | Oneshot/NativeAcp | `src/acp/harnesses/claude_code.rs` | 129 |
| `codex` | Codex | Oneshot/NativeAcp | `src/acp/harnesses/codex.rs` | 105 |
| `gemini` | Gemini | NativeAcp/Oneshot | `src/acp/harnesses/gemini.rs` | 87 |

### 1.2 重复代码分析

3 个文件的重复度约 **85%**：

- 相同的 `struct` 定义模式（`executable: String`, `default_mode: HarnessMode`）
- 相同的 `AcpHarness` trait 实现骨架（`id()`, `display_name()`, `mode()`, `supported_modes()`, `build_config()`）
- 相同的 `execute_oneshot()` 实现（Command 构造 → 超时等待 → 状态检查 → stderr 截取 → stdout 返回）
- 相同的 `spawn_session()` 实现（仅 args 不同）
- 相同的错误处理模式

唯一差异：
- **executable 名称**（`claude`/`codex`/`gemini`）
- **args 参数**（`--print`/`exec`/`--acp`）
- **输出解析**（Claude Code 需要 JSON 字段提取，其余为 PlainText）

### 1.3 acpx 中的 Agent 注册表

acpx 项目支持 15+ 个 agents：

```
pi, openclaw, codex, claude, gemini, cursor, copilot, droid,
iflow, kilocode, kimi, kiro, opencode, qoder, qwen, trae
```

Aleph 已覆盖：codex, claude, gemini（3/16）
Aleph 缺失：13 个 agents

### 1.4 前端现状

Panel 配置页面（`interfaces/webchat/src/views/settings/acp_harnesses.rs`）：

- 硬编码了 3 个 preset 的元数据（ID、名称、图标颜色）：
  ```rust
  const HARNESS_PRESETS: &[HarnessPreset] = &[
      HarnessPreset { id: "claude-code", name: "Claude Code", icon_color: "#F97316" },
      HarnessPreset { id: "codex", name: "Codex", icon_color: "#3B82F6" },
      HarnessPreset { id: "gemini", name: "Gemini CLI", icon_color: "#10B981" },
  ];
  ```
- 通过 `AcpApi::list()` 获取后端 harness 列表
- 将列表分为 Preset CLI 和 Custom CLI 两个区域展示
- 支持测试、保存、启用/禁用、删除操作

---

## 2. 问题诊断

### 2.1 核心问题

1. **重复代码**：每新增一个 preset 需要新增 ~100 行几乎相同的代码
2. **扩展性差**：添加新 agent 成本高，容易引入不一致
3. **维护负担**：修改通用逻辑需要改 N 个文件（如超时处理、错误格式）
4. **前端硬编码**：preset 列表在前端写死，新增 preset 需要同步修改前端

### 2.2 设计目标

| 目标 | 指标 |
|---|---|
| 消除重复 | 预设 harness 实现从 3 个文件 → 1 个通用实现 |
| 降低扩展成本 | 新增 preset 从 ~100 行 → ~5 行配置 |
| 前后端一致 | preset 元数据统一从后端获取，前端不再硬编码 |
| 保持兼容 | 现有 RPC API 契约不变，配置格式不变 |
| 可回滚 | 保留 CustomHarness 作为用户自定义入口 |

---

## 3. 后端设计方案

### 3.1 新增 `GenericAcpHarness`

新建文件：`src/acp/harnesses/generic.rs`

```rust
/// 通用 ACP harness — 通过配置驱动，覆盖 90% 的预设 agent
pub struct GenericAcpHarness {
    id: String,
    display_name: String,
    executable: String,
    default_mode: HarnessMode,
    supported_modes: Vec<HarnessMode>,
    oneshot_args: Vec<String>,
    native_acp_args: Vec<String>,
    output_format: OutputFormat,
}

enum OutputFormat {
    PlainText,
    JsonField { field: String },
}
```

实现 `AcpHarness` trait：
- `id()` / `display_name()` / `mode()` / `supported_modes()` — 直接返回字段值
- `build_config()` — 根据当前 mode 选择对应的 args
- `execute_oneshot()` — 通用实现，根据 `output_format` 决定解析策略
- `spawn_session()` — 通用实现，使用 `native_acp_args`

### 3.2 Preset 规范化为常量数组

修改文件：`src/config/types/acp.rs`

新增 `PresetSpec` 结构：

```rust
/// Preset harness 规范定义
pub struct PresetSpec {
    pub id: &'static str,
    pub display_name: &'static str,
    pub executable: &'static str,
    pub oneshot_args: &'static [&'static str],
    pub native_acp_args: &'static [&'static str],
    pub default_mode: HarnessModeSerde,
    pub output_format: OutputFormatSerde,
    pub trust_level: TrustLevel,
}
```

定义常量数组（16 个 presets）：

```rust
pub const HARNESS_PRESETS: &[PresetSpec] = &[
    // 现有
    PresetSpec {
        id: "claude-code",
        display_name: "Claude Code",
        executable: "claude",
        oneshot_args: &["--print", "--output-format", "json", "-p"],
        native_acp_args: &["--acp"],
        default_mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::Json { field: "result".into() },
        trust_level: TrustLevel::Full,
    },
    PresetSpec {
        id: "codex",
        display_name: "Codex",
        executable: "codex",
        oneshot_args: &["exec"],
        native_acp_args: &["--acp"],
        default_mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        trust_level: TrustLevel::Full,
    },
    PresetSpec {
        id: "gemini",
        display_name: "Gemini",
        executable: "gemini",
        oneshot_args: &["-p"],
        native_acp_args: &["--acp"],
        default_mode: HarnessModeSerde::NativeAcp,
        output_format: OutputFormatSerde::PlainText,
        trust_level: TrustLevel::Full,
    },
    // 新增（来自 acpx）
    PresetSpec {
        id: "opencode",
        display_name: "OpenCode",
        executable: "opencode",
        oneshot_args: &["acp"],
        native_acp_args: &["--acp"],
        default_mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        trust_level: TrustLevel::Full,
    },
    PresetSpec {
        id: "kimi",
        display_name: "Kimi",
        executable: "kimi",
        oneshot_args: &["acp"],
        native_acp_args: &["--acp"],
        default_mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        trust_level: TrustLevel::Full,
    },
    PresetSpec {
        id: "cursor",
        display_name: "Cursor",
        executable: "cursor-agent",
        oneshot_args: &["acp"],
        native_acp_args: &["--acp"],
        default_mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        trust_level: TrustLevel::Full,
    },
    PresetSpec {
        id: "copilot",
        display_name: "Copilot",
        executable: "copilot",
        oneshot_args: &["--acp", "--stdio"],
        native_acp_args: &["--acp"],
        default_mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        trust_level: TrustLevel::Full,
    },
    PresetSpec {
        id: "droid",
        display_name: "Droid",
        executable: "droid",
        oneshot_args: &["exec", "--output-format", "acp"],
        native_acp_args: &["--acp"],
        default_mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        trust_level: TrustLevel::Full,
    },
    PresetSpec {
        id: "pi",
        display_name: "Pi",
        executable: "pi-acp",
        oneshot_args: &[],
        native_acp_args: &["--acp"],
        default_mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        trust_level: TrustLevel::Full,
    },
    PresetSpec {
        id: "iflow",
        display_name: "iFlow",
        executable: "iflow",
        oneshot_args: &["--experimental-acp"],
        native_acp_args: &["--acp"],
        default_mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        trust_level: TrustLevel::Full,
    },
    PresetSpec {
        id: "kilocode",
        display_name: "KiloCode",
        executable: "kilocode",
        oneshot_args: &["acp"],
        native_acp_args: &["--acp"],
        default_mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        trust_level: TrustLevel::Full,
    },
    PresetSpec {
        id: "kiro",
        display_name: "Kiro",
        executable: "kiro-cli-chat",
        oneshot_args: &["acp"],
        native_acp_args: &["--acp"],
        default_mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        trust_level: TrustLevel::Full,
    },
    PresetSpec {
        id: "qoder",
        display_name: "Qoder",
        executable: "qodercli",
        oneshot_args: &["--acp"],
        native_acp_args: &["--acp"],
        default_mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        trust_level: TrustLevel::Full,
    },
    PresetSpec {
        id: "qwen",
        display_name: "Qwen",
        executable: "qwen",
        oneshot_args: &["--acp"],
        native_acp_args: &["--acp"],
        default_mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        trust_level: TrustLevel::Full,
    },
    PresetSpec {
        id: "trae",
        display_name: "Trae",
        executable: "traecli",
        oneshot_args: &["acp", "serve"],
        native_acp_args: &["--acp"],
        default_mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        trust_level: TrustLevel::Full,
    },
    PresetSpec {
        id: "openclaw",
        display_name: "OpenClaw",
        executable: "openclaw",
        oneshot_args: &["acp"],
        native_acp_args: &["--acp"],
        default_mode: HarnessModeSerde::Oneshot,
        output_format: OutputFormatSerde::PlainText,
        trust_level: TrustLevel::Full,
    },
];
```

重构 `AcpHarnessEntry` 的 preset 工厂方法：

```rust
impl AcpHarnessEntry {
    pub fn preset_by_id(id: &str) -> Option<Self> {
        HARNESS_PRESETS.iter().find(|p| p.id == id).map(|p| p.into())
    }

    pub fn all_presets() -> Vec<(String, Self)> {
        HARNESS_PRESETS.iter()
            .map(|p| (p.id.to_string(), p.into()))
            .collect()
    }

    pub fn preset_ids() -> Vec<&'static str> {
        HARNESS_PRESETS.iter().map(|p| p.id).collect()
    }

    pub fn is_preset_id(id: &str) -> bool {
        HARNESS_PRESETS.iter().any(|p| p.id == id)
    }
}
```

### 3.3 重构 `build_harness`

修改文件：`src/acp/manager.rs`

重构前（每个 preset 一个 match 分支）：

```rust
fn build_harness(id: &str, entry: &AcpHarnessEntry) -> Arc<dyn AcpHarness> {
    let preset = entry.preset.as_deref().unwrap_or("");
    match preset {
        "claude-code" => Arc::new(ClaudeCodeHarness::new(...)),
        "codex" => Arc::new(CodexHarness::new(...)),
        "gemini" => Arc::new(GeminiHarness::new(...)),
        _ => Arc::new(CustomHarness::new(...)),
    }
}
```

重构后（统一走 GenericAcpHarness）：

```rust
fn build_harness(id: &str, entry: &AcpHarnessEntry) -> Arc<dyn AcpHarness> {
    if entry.preset.is_some() {
        // 所有 preset 统一使用 GenericAcpHarness
        Arc::new(GenericAcpHarness::from_entry(entry))
    } else {
        // 用户自定义 harness
        Arc::new(CustomHarness::new(id.to_string(), entry.clone()))
    }
}
```

### 3.4 新增 RPC: `acp.presets_meta`

修改文件：`src/gateway/handlers/acp_config.rs`

新增 handler 返回 preset 元数据（供前端动态渲染预设列表）：

```rust
#[derive(Debug, Serialize)]
struct PresetMeta {
    id: String,
    display_name: String,
    icon_color: String,  // 后端分配颜色，前端不再硬编码
}

pub async fn handle_presets_meta(request: JsonRpcRequest) -> JsonRpcResponse {
    let presets: Vec<PresetMeta> = HARNESS_PRESETS.iter().map(|p| {
        PresetMeta {
            id: p.id.to_string(),
            display_name: p.display_name.to_string(),
            icon_color: generate_preset_color(p.id),  // 基于 ID 哈希生成稳定颜色
        }
    }).collect();
    
    JsonRpcResponse::success(request.id, serde_json::to_value(&presets).unwrap())
}
```

颜色生成算法（稳定、确定性）：

```rust
fn generate_preset_color(id: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    let hash = hasher.finish();
    
    // 使用 HSL 色彩空间，固定饱和度和亮度，只变色相
    let hue = (hash % 360) as u16;
    format!("hsl({}, 70%, 50%)", hue)
}
```

---

## 4. 前端同步修改

### 4.1 移除硬编码 Preset 列表

修改文件：`interfaces/webchat/src/views/settings/acp_harnesses.rs`

删除：
```rust
struct HarnessPreset {
    id: &'static str,
    name: &'static str,
    icon_color: &'static str,
}

const HARNESS_PRESETS: &[HarnessPreset] = &[
    HarnessPreset { id: "claude-code", name: "Claude Code", icon_color: "#F97316" },
    HarnessPreset { id: "codex", name: "Codex", icon_color: "#3B82F6" },
    HarnessPreset { id: "gemini", name: "Gemini CLI", icon_color: "#10B981" },
];
```

### 4.2 新增 Preset 元数据获取

新增 API 调用：

```rust
// 在 Effect::new 中加载
spawn_local(async move {
    if let Ok(presets) = AcpApi::presets_meta(&state).await {
        preset_meta.set(presets);
    }
});
```

### 4.3 动态渲染 Preset 列表

用从后端获取的 `preset_meta` 替换硬编码的 `HARNESS_PRESETS`：

```rust
// 替换前: HARNESS_PRESETS.iter().map(|preset| { ... })
// 替换后: preset_meta.get().iter().map(|preset| { ... })
```

### 4.4 新增 API 类型定义

修改文件：`interfaces/webchat/src/api/mod.rs`（或相关 api 文件）

```rust
#[derive(Clone, Debug)]
pub struct PresetMeta {
    pub id: String,
    pub display_name: String,
    pub icon_color: String,
}
```

---

## 5. 文件变更清单

### 5.1 新增文件

| 文件 | 说明 |
|---|---|
| `src/acp/harnesses/generic.rs` | GenericAcpHarness 实现 |
| `docs/superpowers/specs/2026-04-18-acp-harness-refactor.md` | 本设计文档 |

### 5.2 修改文件

| 文件 | 变更内容 |
|---|---|
| `src/config/types/acp.rs` | 新增 PresetSpec 常量数组，重构 preset 工厂方法 |
| `src/acp/manager.rs` | 重构 build_harness，简化 match 分支 |
| `src/acp/harnesses/mod.rs` | 导出新模块，移除旧 harness 导出 |
| `src/gateway/handlers/acp_config.rs` | 新增 acp.presets_meta handler |
| `interfaces/webchat/src/views/settings/acp_harnesses.rs` | 移除硬编码 preset，改为从后端获取 |
| `interfaces/webchat/src/api/*.rs` | 新增 PresetMeta 类型和 API 调用 |

### 5.3 删除文件

| 文件 | 说明 |
|---|---|
| `src/acp/harnesses/claude_code.rs` | 被 GenericAcpHarness 替代 |
| `src/acp/harnesses/codex.rs` | 被 GenericAcpHarness 替代 |
| `src/acp/harnesses/gemini.rs` | 被 GenericAcpHarness 替代 |

---

## 6. API 契约

### 6.1 保持不变的 API

以下 RPC 方法的请求/响应格式**完全不变**：

- `acp.list` — 返回 AcpHarnessInfo 数组
- `acp.get` — 返回单个 AcpHarnessInfo
- `acp.create` — 创建自定义 harness
- `acp.update` — 更新 harness 配置
- `acp.delete` — 删除自定义 harness
- `acp.test` — 测试 harness 可用性
- `acp.set_enabled` — 启用/禁用 harness
- `acp.presets` — 返回 preset 配置列表（格式不变，内容扩展）

### 6.2 新增 API

- `acp.presets_meta` — 返回 preset 元数据（ID、显示名、图标颜色）

请求：
```json
{ "jsonrpc": "2.0", "method": "acp.presets_meta", "id": 1 }
```

响应：
```json
{
  "jsonrpc": "2.0",
  "result": [
    { "id": "claude-code", "display_name": "Claude Code", "icon_color": "hsl(42, 70%, 50%)" },
    { "id": "codex", "display_name": "Codex", "icon_color": "hsl(123, 70%, 50%)" },
    { "id": "gemini", "display_name": "Gemini", "icon_color": "hsl(234, 70%, 50%)" },
    { "id": "opencode", "display_name": "OpenCode", "icon_color": "hsl(345, 70%, 50%)" }
  ],
  "id": 1
}
```

---

## 7. 测试策略

### 7.1 后端测试

1. **单元测试**：
   - `GenericAcpHarness::from_entry` 正确构建
   - `build_config` 根据 mode 返回正确的 args
   - `execute_oneshot` 的 PlainText 和 JsonField 解析
   - `PresetSpec` 到 `AcpHarnessEntry` 的转换

2. **集成测试**：
   - `AcpHarnessManager` 正确注册所有 16 个 presets
   - `acp.list` 返回 16 个 harnesses
   - `acp.presets` 返回 16 个 presets
   - `acp.presets_meta` 返回正确的元数据

3. **回归测试**：
   - 现有 3 个 preset 的行为不变（id、display_name、executable、args）
   - CustomHarness 不受影响
   - 配置序列化/反序列化格式不变

### 7.2 前端测试

1. Preset 列表正确从后端加载并渲染
2. 图标颜色稳定（相同 ID 总是相同颜色）
3. Custom harness 区域正常工作

---

## 8. 实施顺序

### Phase 1: 基础重构（worktree）

1. 创建 `GenericAcpHarness`
2. 重构 `AcpHarnessEntry` preset 工厂
3. 修改 `build_harness` 使用 Generic
4. 编译检查

### Phase 2: 批量添加 Agents

1. 在 `HARNESS_PRESETS` 常量数组中添加 13 个新 presets
2. 验证 `all_presets()` 返回 16 个
3. 编译检查

### Phase 3: 前端同步

1. 新增 `acp.presets_meta` RPC handler
2. 前端移除硬编码 preset，调用新 API
3. 验证前端正确渲染所有 preset

### Phase 4: 清理

1. 删除 `claude_code.rs`、`codex.rs`、`gemini.rs`
2. 更新 `harnesses/mod.rs` 导出
3. 更新测试

### Phase 5: 验证

1. `cargo check`
2. `cargo test -p alephcore --lib`（acp 相关测试）
3. `cargo test --test acp_probe`
4. 前端编译（`just wasm` 或相关命令）

---

## 9. 风险评估

| 风险 | 概率 | 影响 | 缓解措施 |
|---|---|---|---|
| GenericAcpHarness 无法覆盖某些特殊 agent | 中 | 中 | 保留 CustomHarness 作为退路；对特殊 agent 可保留专用实现 |
| 前端颜色生成与预期不符 | 低 | 低 | 使用稳定的哈希算法；可在后端配置固定颜色映射 |
| 配置序列化格式变化 | 低 | 高 | AcpHarnessEntry 字段不变，仅内部实现重构 |
| 新 agent executable 名称不准确 | 中 | 低 | 基于 acpx 的 AGENT_REGISTRY；实际使用时可通过配置覆盖 |

---

## 10. 回滚方案

如需回滚：
1. 恢复 `src/acp/harnesses/claude_code.rs`、`codex.rs`、`gemini.rs`
2. 恢复 `src/acp/manager.rs` 的 `build_harness` match 分支
3. 恢复 `src/config/types/acp.rs` 的旧 preset 工厂方法
4. 前端恢复硬编码 `HARNESS_PRESETS`

所有变更集中在独立 worktree，不影响 main 分支。

---

*文档版本: v1.0*
*作者: Sisyphus (AI Agent)*
*日期: 2026-04-18*
