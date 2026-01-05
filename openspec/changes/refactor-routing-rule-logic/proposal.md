# Proposal: Refactor Routing Rule Logic

## Change ID
`refactor-routing-rule-logic`

## Status
Draft

## Why

当前路由规则系统将所有规则视为同一类型，但实际使用中存在两种不同的使用模式：

1. **指令模式**：用户输入 `/draw 画一幅山水画`，期望触发特定的 AI provider 和预设 prompt
2. **关键词增强模式**：用户输入包含某些关键词时，自动附加额外的 prompt 引导

当前系统的问题：
- 无法区分这两种使用模式
- 指令性规则（如 `/draw`）匹配后，指令本身会被传递给 AI，造成混乱
- 关键词规则无法叠加使用（当前是 first-match-stops）
- 关键词规则不应该覆盖 provider 选择

## Overview

将路由规则重构为两种类型：

### 1. 指令性规则 (Command Rules)
- 以 `/` 开头的正则匹配（如 `^/draw`、`^/translate`）
- 包含软件预设指令和用户自定义指令
- **每次只能匹配一个**（first-match-stops）
- 提供完整配置：provider + system_prompt
- 匹配后**自动清洗掉指令前缀**再传递给 AI

### 2. 关键词规则 (Keyword Rules)
- 非 `/` 开头的正则匹配（如 `翻译成英文`、`代码优化`）
- 只能是用户自定义的
- **每次可以匹配多个**（all-match）
- 只提供 system_prompt，**不提供 provider 设置**
- 匹配到的多个 prompt 会被合并

### 3. Prompt 合并逻辑
同一次 AI 对话可以使用：
- 0 或 1 个指令性规则
- 0 到多个关键词规则

最终 prompt = 指令性规则的 prompt + 所有匹配的关键词规则 prompt

## Motivation

这个重构解决了以下痛点：

1. **指令清洗**：`/draw 画一幅山水画` → AI 只收到 "画一幅山水画"，不收到 "/draw"
2. **多关键词叠加**：用户可以定义多个关键词规则，同时生效
3. **关注点分离**：指令选 provider，关键词增强 prompt
4. **更直观的配置**：两种规则类型各司其职

## Goals

1. 定义两种规则类型：CommandRule 和 KeywordRule
2. 实现指令前缀自动清洗
3. 实现关键词规则多重匹配和 prompt 合并
4. 保持向后兼容（现有配置可继续工作）
5. 更新 Swift UI 的规则编辑界面

## Scope

### In Scope
- 重构 `RoutingRuleConfig` 数据结构，添加 `rule_type` 字段
- 修改 `Router` 匹配逻辑，区分两种规则类型
- 实现指令前缀清洗逻辑
- 实现关键词 prompt 合并逻辑
- 更新 UniFFI 绑定
- 更新 Swift UI 规则编辑界面
- 添加测试用例

### Out of Scope
- 内置指令的可视化编辑（保持硬编码）
- 关键词规则的优先级排序
- 基于语义的关键词匹配

## Dependencies

- **Requires**:
  - `ai-routing` spec (MODIFIED)
  - `settings-ui-layout` spec (MODIFIED)
- **Blocks**: None
- **Related**:
  - `enhance-routing-rule-system` (已完成，本次是进一步重构)

## What Changes

### Data Structure Changes

#### 1. `RoutingRuleConfig` (Rust + UniFFI)
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRuleConfig {
    /// Rule type: "command" or "keyword"
    /// - "command": Starts with /, provides provider + prompt, first-match-stops
    /// - "keyword": Non-/ patterns, provides prompt only, all-match
    #[serde(default = "default_rule_type")]
    pub rule_type: String,

    /// Regex pattern to match against user input
    pub regex: String,

    /// Provider name (required for command rules, ignored for keyword rules)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

    /// System prompt (required for both types)
    #[serde(default)]
    pub system_prompt: Option<String>,

    /// Whether to strip the matched prefix (auto-true for command rules)
    #[serde(default)]
    pub strip_prefix: Option<bool>,

    // ... other existing fields ...
}

fn default_rule_type() -> String {
    // Auto-detect: patterns starting with ^/ are command rules
    // This is a fallback; actual detection happens at load time
    "command".to_string()
}
```

#### 2. New Matching Result Type
```rust
/// Result of routing rule matching
pub struct RoutingMatch {
    /// Matched command rule (if any)
    pub command_rule: Option<MatchedCommandRule>,
    /// All matched keyword rules
    pub keyword_rules: Vec<MatchedKeywordRule>,
}

pub struct MatchedCommandRule {
    pub provider_name: String,
    pub system_prompt: Option<String>,
    /// Input with command prefix stripped
    pub cleaned_input: String,
}

pub struct MatchedKeywordRule {
    pub system_prompt: String,
}
```

### Logic Changes

#### 1. `Router::route()` → `Router::match_rules()`
```rust
impl Router {
    /// Match input against all rules and return combined result
    pub fn match_rules(&self, input: &str) -> RoutingMatch {
        let mut result = RoutingMatch::default();

        // Phase 1: Find command rule (first-match-stops)
        for rule in &self.command_rules {
            if rule.matches(input) {
                result.command_rule = Some(MatchedCommandRule {
                    provider_name: rule.provider_name.clone(),
                    system_prompt: rule.system_prompt.clone(),
                    cleaned_input: rule.strip_command_prefix(input),
                });
                break; // First match stops for command rules
            }
        }

        // Phase 2: Find all matching keyword rules (all-match)
        for rule in &self.keyword_rules {
            if rule.matches(input) {
                result.keyword_rules.push(MatchedKeywordRule {
                    system_prompt: rule.system_prompt.clone().unwrap_or_default(),
                });
            }
        }

        result
    }
}
```

#### 2. Prompt Assembly
```rust
impl RoutingMatch {
    /// Combine all prompts into final system prompt
    pub fn assemble_prompt(&self) -> String {
        let mut prompts = Vec::new();

        // Add command rule prompt first
        if let Some(ref cmd) = self.command_rule {
            if let Some(ref prompt) = cmd.system_prompt {
                prompts.push(prompt.as_str());
            }
        }

        // Add all keyword rule prompts
        for keyword in &self.keyword_rules {
            prompts.push(&keyword.system_prompt);
        }

        // Join with double newline for clear separation
        prompts.join("\n\n")
    }
}
```

### Example Scenarios

#### Scenario 1: Command Rule Only
**Input**: `/draw 一幅山水画`
**Matched**:
- Command rule: `^/draw` → provider=google-gemini, prompt="请根据提示画一幅画"
- Keyword rules: none

**Result**:
- Provider: google-gemini
- Cleaned input: "一幅山水画"
- Final prompt: "请根据提示画一幅画"

#### Scenario 2: Keyword Rules Only
**Input**: `请帮我翻译成英文：你好世界`
**Matched**:
- Command rule: none
- Keyword rules:
  - `翻译成英文` → prompt="翻译目标语言为英文"
  - `请帮我` → prompt="语气友好礼貌"

**Result**:
- Provider: default_provider
- Cleaned input: "请帮我翻译成英文：你好世界" (unchanged)
- Final prompt:
  ```
  翻译目标语言为英文

  语气友好礼貌
  ```

#### Scenario 3: Command + Keywords
**Input**: `/draw 一幅山水画，翻译成英文`
**Matched**:
- Command rule: `^/draw` → provider=google-gemini, prompt="请根据提示画一幅画"
- Keyword rules:
  - `翻译成英文` → prompt="翻译目标语言为英文"

**Result**:
- Provider: google-gemini
- Cleaned input: "一幅山水画，翻译成英文"
- Final prompt:
  ```
  请根据提示画一幅画

  翻译目标语言为英文
  ```

#### Scenario 4: Empty Command Content (Error Case)
**Input**: `/draw`
**Matched**:
- Command rule: `^/draw` matches

**Result**:
- Cleaned input: "" (empty after stripping)
- **Error**: Show Halo error popup with message "指令需要内容" / "Command requires content"
- AI processing is NOT triggered

### Config File Format

```toml
# Command rules (with provider)
[[rules]]
rule_type = "command"
regex = "^/draw\\s+"
provider = "google-gemini"
system_prompt = "请根据提示画一幅画"
strip_prefix = true

[[rules]]
rule_type = "command"
regex = "^/translate\\s+"
provider = "openai"
system_prompt = "你是一个翻译专家"
strip_prefix = true

# Keyword rules (prompt only, no provider)
[[rules]]
rule_type = "keyword"
regex = "翻译成英文"
system_prompt = "翻译目标语言为英文"

[[rules]]
rule_type = "keyword"
regex = "翻译成中文"
system_prompt = "翻译目标语言为中文"

[[rules]]
rule_type = "keyword"
regex = "请帮我"
system_prompt = "语气友好礼貌"
```

## UI Design Guidelines

### Design Philosophy
路由规则 UI 需要让用户**一看就懂**，核心原则：
1. **两种规则类型视觉区分明显**
2. **内置指令作为使用指南展示**
3. **实时预览帮助理解效果**

### Rules List View (规则列表)

```
┌─────────────────────────────────────────────────────────────┐
│  路由规则                                            [+ 添加] │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ── 指令规则 (/command) ──────────────────────────────────  │
│  💡 输入以 / 开头触发，选择 AI 供应商和预设提示词              │
│                                                             │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ 🔒 /search     │ 默认供应商  │ 网络搜索助手           │   │
│  │    内置指令                                    [查看] │   │
│  └─────────────────────────────────────────────────────┘   │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ 🔒 /mcp        │ 默认供应商  │ MCP 集成 (即将推出)    │   │
│  │    内置指令                                    [查看] │   │
│  └─────────────────────────────────────────────────────┘   │
│  ┌─────────────────────────────────────────────────────┐   │
│  │    /draw       │ Gemini     │ 请根据提示画一幅画      │   │
│  │    用户自定义                          [编辑] [删除] │   │
│  └─────────────────────────────────────────────────────┘   │
│                                                             │
│  ── 关键词规则 (keyword) ─────────────────────────────────  │
│  💡 输入包含关键词时自动附加提示词，可同时匹配多个             │
│                                                             │
│  ┌─────────────────────────────────────────────────────┐   │
│  │    翻译成英文   │ 翻译目标语言为英文                   │   │
│  │    用户自定义                          [编辑] [删除] │   │
│  └─────────────────────────────────────────────────────┘   │
│  ┌─────────────────────────────────────────────────────┐   │
│  │    请帮我       │ 语气友好礼貌                        │   │
│  │    用户自定义                          [编辑] [删除] │   │
│  └─────────────────────────────────────────────────────┘   │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Key UI Elements

#### 1. 分组显示 (Grouped Display)
- 指令规则和关键词规则分开显示
- 每组有**简短说明**帮助理解
- 视觉上用分隔线或背景色区分

#### 2. 内置指令标识 (Builtin Badge)
- 🔒 图标 + "内置指令" 标签
- 不可编辑/删除，但可查看详情
- 作为**使用示例**帮助用户理解

#### 3. 规则卡片信息
**指令规则卡片**:
- 指令名称 (如 `/draw`)
- 供应商名称 (如 `Gemini`)
- 提示词预览 (截断显示)

**关键词规则卡片**:
- 关键词模式 (如 `翻译成英文`)
- 提示词预览 (截断显示)
- 无供应商显示（因为使用默认）

### Add/Edit Rule Sheet

```
┌─────────────────────────────────────────────────────────────┐
│  添加规则                                              [取消] │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  规则类型                                                    │
│  ┌─────────────────┐ ┌─────────────────┐                    │
│  │  ✓ 指令规则     │ │    关键词规则    │                    │
│  │  /command       │ │    keyword      │                    │
│  └─────────────────┘ └─────────────────┘                    │
│                                                             │
│  📖 指令规则：用户输入 /xxx 触发，指定 AI 供应商              │
│     关键词规则：输入包含关键词时附加提示词，可多个同时生效     │
│                                                             │
│  ─────────────────────────────────────────────────────────  │
│                                                             │
│  匹配模式 (正则表达式)                                       │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ ^/draw\s+                                           │   │
│  └─────────────────────────────────────────────────────┘   │
│  💡 指令规则建议以 ^/ 开头                                   │
│                                                             │
│  AI 供应商                           [仅指令规则显示此项]    │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ Gemini                                          ▼   │   │
│  └─────────────────────────────────────────────────────┘   │
│                                                             │
│  系统提示词                                                  │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ 请根据提示画一幅画                                    │   │
│  │                                                     │   │
│  │                                                     │   │
│  └─────────────────────────────────────────────────────┘   │
│                                                             │
│                                              [保存规则]     │
└─────────────────────────────────────────────────────────────┘
```

### Usage Example Panel (使用示例面板)

在规则列表底部或侧边显示实时匹配预览：

```
┌─────────────────────────────────────────────────────────────┐
│  📝 测试匹配                                                 │
├─────────────────────────────────────────────────────────────┤
│  输入测试文本：                                              │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ /draw 一幅山水画，翻译成英文                          │   │
│  └─────────────────────────────────────────────────────┘   │
│                                                             │
│  匹配结果：                                                  │
│  ✓ 指令规则: /draw → Gemini                                 │
│  ✓ 关键词: 翻译成英文                                        │
│                                                             │
│  清洗后输入: 一幅山水画，翻译成英文                           │
│                                                             │
│  合并提示词:                                                 │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ 请根据提示画一幅画                                    │   │
│  │                                                     │   │
│  │ 翻译目标语言为英文                                    │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### Localization Keys

```swift
// 规则类型
"rules.type.command" = "指令规则"
"rules.type.keyword" = "关键词规则"
"rules.type.builtin" = "内置指令"

// 说明文字
"rules.command.hint" = "输入以 / 开头触发，选择 AI 供应商和预设提示词"
"rules.keyword.hint" = "输入包含关键词时自动附加提示词，可同时匹配多个"

// 错误提示 (Halo 弹窗)
"error.command.empty" = "指令需要内容"
"error.command.empty.detail" = "请在指令后输入要处理的内容"
```

## Risks and Mitigations

### Risk 1: Backward Compatibility
- **Impact**: Existing configs without `rule_type` field may break
- **Mitigation**:
  - Auto-detect rule_type based on regex pattern (^/ → command, else → keyword)
  - Treat existing rules with `provider` field as command rules
  - Treat existing rules without `provider` as keyword rules

### Risk 2: Prompt Assembly Confusion
- **Impact**: Users may not understand how prompts are combined
- **Mitigation**:
  - Clear documentation with examples
  - Show combined prompt preview in Settings UI
  - Use natural language joining (comma + space)

### Risk 3: Too Many Keyword Matches
- **Impact**: Too many keywords matching may create overly long prompts
- **Mitigation**:
  - Limit keyword rule count (suggest max 5-10)
  - Truncate combined prompt if too long
  - Log warning when many keywords match

### Risk 4: Command Prefix Edge Cases
- **Impact**: Edge cases like `/draw` without content may cause issues
- **Mitigation**:
  - Validate that stripped input is not empty
  - Return error if command has no content after stripping

## Testing Strategy

### Unit Tests
1. Command rule matching and first-match-stops behavior
2. Keyword rule matching and all-match behavior
3. Command prefix stripping
4. Prompt assembly from multiple rules
5. Backward compatibility with existing configs
6. Edge cases (empty input, no matches, etc.)

### Integration Tests
1. Full routing flow with mixed rule types
2. Config loading with new and old formats
3. UniFFI binding correctness

### Manual Tests
1. Test in real scenarios with various inputs
2. Verify prompt appears correctly in AI responses
3. Test Settings UI rule editing

## Implementation Tasks

See [tasks.md](./tasks.md) for detailed implementation steps.

## Success Criteria

1. Command rules match first-match-stops, keyword rules match all-match
2. Command prefix is automatically stripped before sending to AI
3. Multiple keyword prompts are combined correctly
4. Backward compatible with existing config files
5. Settings UI updated to show rule type
6. All tests passing
7. Documentation updated

## Design Decisions

1. **Prompt separator**: Use `\n\n` (double newline) for clear separation between prompts

2. **Keyword rule limit**: Soft limit of 10, log warning if exceeded

3. **Builtin command rules**: Show in Settings UI as usage guide, marked as "builtin"

4. **Empty command content**: Show Halo error popup to user with localized message

## References

- [CLAUDE.md](../../../CLAUDE.md) - Project overview
- [ai-routing spec](../../specs/ai-routing/spec.md) - Current routing spec
- [enhance-routing-rule-system](../enhance-routing-rule-system/) - Previous enhancement (completed)
