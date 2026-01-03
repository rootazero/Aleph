# Proposal: Enhance Routing Rule System

## Change ID
`enhance-routing-rule-system`

## Why

当前路由系统存在以下限制：
1. **系统提示词不够灵活**：虽然规则可以指定 `system_prompt`，但实际匹配逻辑和提示词管理不够完善
2. **匹配输入不完整**：路由规则只匹配剪贴板内容，没有考虑窗口上下文信息
3. **规则顺序管理混乱**：新增规则的位置不明确，用户难以控制匹配优先级
4. **缺少明确的兜底策略**：当没有规则匹配时的行为不够清晰

这些限制导致：
- 用户无法基于完整上下文（窗口+剪贴板）进行智能路由
- 无法为不同场景预设不同的系统提示词
- 规则管理混乱，难以维护
- 兜底行为不可预测

## Overview

重新定义和增强路由规则系统，使其能够：
1. 基于完整上下文（当前窗口内容 + 最近剪贴板内容）进行匹配
2. 为每个规则预设独立的系统提示词
3. 明确的从上到下顺序匹配机制（第一个匹配即停止）
4. 清晰的兜底规则（无匹配时使用默认供应商和默认提示词）
5. 新增规则自动插入顶部，确保优先级

## Motivation

改进路由系统可以：
- **更智能的上下文感知**：结合窗口标题和剪贴板内容，让 AI 了解用户意图
- **场景化的 AI 行为**：不同规则可以有不同的"人格"（系统提示词）
- **可预测的匹配行为**：清晰的顺序匹配规则，避免混乱
- **更好的用户体验**：规则管理更直观，新增规则自动获得最高优先级

这个变更是实现"上下文感知 AI 路由"的关键一步，为未来的高级路由策略（如基于语义的路由）打下基础。

## Goals

1. **上下文组合匹配**：路由规则匹配"窗口上下文 + 剪贴板内容"的组合字符串
2. **规则级系统提示词**：每个规则可以预设独立的 `system_prompt`，覆盖供应商默认提示词
3. **顺序匹配机制**：明确规则从上到下匹配，第一个匹配即停止
4. **兜底策略**：无匹配时使用 `default_provider` 及其默认系统提示词
5. **规则顺序管理**：新增规则插入列表顶部（最高优先级）

## Scope

### In Scope
- 修改 `Router::route()` 方法，接受组合上下文输入
- 更新 `RoutingRule::matches()` 匹配逻辑
- 完善系统提示词的传递和使用
- 明确规则匹配顺序和停止条件
- 实现兜底规则逻辑
- 更新配置文件格式文档
- 添加规则顺序管理的 API 方法
- 更新相关测试用例

### Out of Scope (Future Enhancements)
- 基于语义的路由（使用 embedding 匹配）
- 规则的条件表达式（如 AND/OR 组合）
- 规则的启用/禁用开关
- 规则的分组管理
- 图形化规则编辑器

## Dependencies
- **Requires**:
  - `ai-routing` spec（需修改）
  - `context-capture` spec（需要窗口上下文数据）
  - `clipboard-management` spec（需要剪贴板内容）
- **Blocks**: 无
- **Related**:
  - `memory-augmentation` spec（记忆检索在路由之后进行）
  - Settings UI 规则编辑界面（未来需要）

## What Changes

这个提案将对以下文件和模块进行修改：

### Core Changes
1. **`Aether/core/src/router/mod.rs`**
   - 修改 `Router::route()` 方法，接受完整上下文字符串
   - 实现明确的从上到下首次匹配逻辑
   - 完善系统提示词优先级处理
   - 改进日志输出

2. **`Aether/core/src/core.rs`**
   - 新增 `build_routing_context()` 函数
   - 修改 `process_clipboard()` 方法，构建完整上下文后再路由

3. **`Aether/core/src/config/mod.rs`**
   - 新增 `add_rule_at_top()` 方法
   - 新增 `remove_rule()` 方法
   - 新增 `move_rule()` 方法
   - 新增 `get_rule()` 方法
   - 增强 `validate()` 方法，添加缺失配置警告

### Test Changes
4. **`Aether/core/src/router/mod.rs` (tests)**
   - 新增上下文匹配测试
   - 新增首次匹配停止测试
   - 新增系统提示词优先级测试
   - 新增兜底规则测试

### Documentation Changes
5. **`CLAUDE.md`**
   - 更新配置示例
   - 说明上下文字符串格式
   - 说明规则匹配顺序

6. **`docs/CONFIGURATION.md`** (新建)
   - 详细说明路由规则配置
   - 提供多种配置示例
   - 说明最佳实践

### Configuration Changes
7. **`config.example.toml`** (新建或更新)
   - 添加上下文匹配示例
   - 添加系统提示词示例
   - 添加规则顺序说明

## Affected Capabilities

这个变更将修改以下现有 capabilities：
1. **`ai-routing`** (MODIFIED) - 增强路由匹配逻辑和系统提示词管理
2. **`core-library`** (MODIFIED) - 更新 `process_clipboard()` 方法传递完整上下文

不需要新增 capability，这是对现有路由系统的增强。

## Risks and Mitigations

### Risk 1: 组合上下文字符串过长
- **Impact**: "窗口上下文 + 剪贴板内容" 可能超过某些模型的输入限制
- **Mitigation**:
  - 限制窗口上下文长度（如最多 500 字符）
  - 路由匹配只用于选择供应商，完整内容发送给 AI 时再处理
  - 在文档中说明最佳实践

### Risk 2: 规则顺序混乱
- **Impact**: 用户可能不理解"从上到下"的匹配顺序
- **Mitigation**:
  - 在文档中明确说明匹配规则
  - Settings UI 中显示规则编号（1, 2, 3...）
  - 提供规则重排功能（未来）

### Risk 3: 兜底规则缺失
- **Impact**: 如果没有配置 `default_provider`，无匹配时会报错
- **Mitigation**:
  - 配置验证时警告缺少 `default_provider`
  - 返回清晰的错误信息
  - 文档中强调配置 `default_provider` 的重要性

### Risk 4: 系统提示词冲突
- **Impact**: 规则的 `system_prompt` 与供应商默认提示词可能冲突
- **Mitigation**:
  - 明确规则提示词优先级高于供应商默认
  - 如果规则未指定 `system_prompt`，则使用供应商默认
  - 在文档中说明提示词优先级规则

### Risk 5: 向后兼容性
- **Impact**: 现有配置文件可能需要更新
- **Mitigation**:
  - 新逻辑向后兼容现有配置格式
  - 不强制要求所有规则都有 `system_prompt`
  - 提供迁移指南（如果需要）

## Testing Strategy

### Unit Tests
- Router 匹配组合上下文字符串
- 顺序匹配逻辑（第一个匹配即停止）
- 兜底规则逻辑
- 系统提示词覆盖逻辑
- 规则插入顺序（新规则在顶部）

### Integration Tests
- 端到端测试：窗口上下文 → 路由 → AI 处理
- 配置加载和验证
- 规则编辑和保存

### Manual Tests
- 在真实场景中测试不同窗口的路由结果
- 验证系统提示词是否正确应用
- 验证新增规则的优先级

## Implementation Phases

### Phase 1: Core Routing Logic Enhancement (1-2 days)
1. 修改 `Router::route()` 接受完整上下文
2. 更新匹配逻辑
3. 完善系统提示词传递
4. 实现兜底规则

### Phase 2: Configuration and API Updates (1 day)
1. 更新配置文件格式文档
2. 添加规则管理 API 方法
3. 配置验证增强

### Phase 3: Testing and Documentation (1 day)
1. 编写单元测试和集成测试
2. 更新用户文档
3. 添加示例配置

## Success Criteria

1. ✅ 路由规则能够匹配"窗口上下文 + 剪贴板内容"的组合
2. ✅ 每个规则可以预设独立的系统提示词
3. ✅ 规则按从上到下顺序匹配，第一个匹配即停止
4. ✅ 无匹配时使用 `default_provider` 和默认系统提示词
5. ✅ 新增规则自动插入列表顶部
6. ✅ 所有测试通过，覆盖率 > 80%
7. ✅ 文档更新完整，包含示例配置

## Open Questions

1. **窗口上下文格式**：是只用窗口标题，还是包含 bundle ID？
   - 建议：使用格式化字符串 `[AppName] WindowTitle\n` 作为前缀

2. **剪贴板历史深度**："10秒内最后一个剪切板内容" 是否足够？
   - 建议：从 10 秒开始，未来可配置

3. **规则重排功能**：是否需要在 Phase 1 实现？
   - 建议：不在本变更范围内，留待 Settings UI 阶段

4. **规则测试工具**：是否需要提供 "测试规则匹配" 的 CLI 命令？
   - 建议：可作为 nice-to-have，不阻塞主要功能

## Alternatives Considered

### Alternative 1: 分离窗口上下文和剪贴板匹配
- **Approach**: 规则分为两种类型：窗口规则和内容规则
- **Rejected Because**: 增加复杂度，不如统一匹配简单直观

### Alternative 2: 基于配置文件顺序匹配
- **Approach**: 完全依赖配置文件中的顺序
- **Rejected Because**: 配置文件可能被文本编辑器重排，不可靠

### Alternative 3: 规则优先级数字
- **Approach**: 每个规则有 `priority` 字段（数字越小优先级越高）
- **Rejected Because**: 增加配置复杂度，顺序匹配更直观

## References

- [CLAUDE.md](../../../CLAUDE.md) - Project overview and architecture
- [ai-routing spec](../../specs/ai-routing/spec.md) - Current routing specification
- [context-capture spec](../../specs/context-capture/spec.md) - Window context capture
- [Conventional Commits](https://www.conventionalcommits.org/) - Commit message format
