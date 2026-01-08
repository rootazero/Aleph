# Change: Add Skills Capability (Claude Agent Skills Standard)

**Status**: Draft
**Author**: AI Assistant
**Created**: 2026-01-07
**Updated**: 2026-01-08

## Why

[Claude Agent Skills](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview) 是 Anthropic 发布的开放标准，用于扩展 AI Agent 的能力。Skills 不是可执行代码，而是**动态指令注入**——教 Claude 如何完成特定任务的结构化指南。

OpenAI、GitHub Copilot 等已采用相同规范。Aether 作为 OS-Level AI 中间件，需要支持此开放标准：
- 用户可以创建/分享 Skills
- 与 Claude Code、GitHub Copilot 等工具兼容
- 扩展 Aether 的任务处理能力

## What Changes

### Architecture Integration

本提案采用现有的 **Strategy Pattern** 架构，将 Skills 作为一个独立的 Capability 实现：

```
┌─────────────────────────────────────────────────────────────────┐
│                 CompositeCapabilityExecutor                      │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌────────┐ │
│  │ Memory   │ │ Search   │ │ MCP      │ │ Video    │ │ Skills │ │
│  │ Strategy │ │ Strategy │ │ Strategy │ │ Strategy │ │Strategy│ │
│  │ (0)      │ │ (1)      │ │ (2)      │ │ (3)      │ │ (4)    │ │
│  └──────────┘ └──────────┘ └──────────┘ └──────────┘ └────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

### New Components

1. **Capability::Skills (payload/capability.rs)**
   - 添加 `Skills = 4` 变体（Video 之后执行）
   - Skills 在所有其他 Capability 之后执行，以便收集完整上下文

2. **SkillsStrategy (capability/strategies/skills.rs)**
   - 实现 `CapabilityStrategy` trait
   - 从 `SkillsRegistry` 加载匹配的 Skill
   - 将 Skill 指令注入 `payload.context.skill_instructions`

3. **SkillsRegistry (skills/registry.rs)**
   - 管理 Skills 目录 (`~/.config/aether/skills/`)
   - 解析 SKILL.md 文件（YAML frontmatter + Markdown body）
   - 支持热重载（目录变更检测）

4. **Skill Data Types (skills/mod.rs)**
   - `SkillFrontmatter`: name, description, allowed_tools
   - `Skill`: frontmatter + instructions (markdown body)
   - `SkillsConfig`: enabled, skills_dir, auto_match_enabled

### Modified Components

1. **AgentContext (payload/mod.rs)**
   - 添加 `skill_instructions: Option<String>` 字段

2. **PromptAssembler (payload/assembler.rs)**
   - 在 system prompt 末尾注入 skill_instructions
   - 格式: `## Skill Instructions\n\n{instructions}`

3. **Config (config/mod.rs)**
   - 添加 `[skills]` 配置节
   - 支持 `enabled`, `skills_dir`, `auto_match_enabled` 选项

4. **Router (router/mod.rs)**
   - 检测 `/skill <name>` 命令（显式调用）
   - 支持基于 description 的自动匹配（可选）

## Impact

- **Affected specs**:
  - `skills-capability` (NEW) - Skills 系统规范

- **Affected code** (按依赖顺序):
  ```
  Phase 1: 数据类型和注册表
  ├── Aether/core/src/skills/mod.rs (NEW)
  ├── Aether/core/src/skills/registry.rs (NEW)
  └── Aether/core/src/payload/capability.rs (ADD Skills variant)

  Phase 2: Strategy 实现
  ├── Aether/core/src/capability/strategies/skills.rs (NEW)
  ├── Aether/core/src/capability/strategies/mod.rs (ADD export)
  └── Aether/core/src/payload/mod.rs (ADD skill_instructions field)

  Phase 3: 集成和配置
  ├── Aether/core/src/config/mod.rs (ADD SkillsConfig)
  ├── Aether/core/src/core.rs (REGISTER SkillsStrategy)
  └── Aether/core/src/payload/assembler.rs (ADD skill injection)

  Phase 4: 路由和命令
  ├── Aether/core/src/router/mod.rs (ADD /skill command)
  └── Aether/core/src/lib.rs (RE-EXPORT skills module)
  ```

- **Breaking changes**: None（增量添加）

## Claude Agent Skills Standard

### SKILL.md 格式

```markdown
---
name: refine-text
description: Improve and polish writing. Use when asked to refine, improve, or enhance text.
allowed-tools:
  - Read
  - Edit
---

# Refine Text Skill

When refining text, follow these principles:

1. **Clarity**: Remove ambiguity and improve readability
2. **Conciseness**: Eliminate redundancy without losing meaning
3. **Tone**: Match the intended audience and purpose
4. **Flow**: Ensure logical progression of ideas

## Guidelines

- Preserve the original meaning and intent
- Maintain consistent voice and style
- Fix grammar and punctuation errors
- Improve sentence structure where needed
```

### 关键特性

1. **不是可执行代码**：Skills 是指令，不是程序
2. **描述匹配**：系统根据 `description` 自动匹配适用的 Skill
3. **工具约束**：`allowed-tools` 限制 Skill 可使用的工具（预留给 MCP 集成）
4. **可组合**：多个 Skills 可以同时生效

## Success Criteria

1. ✅ 支持 Claude Agent Skills 标准格式（SKILL.md）
2. ✅ 支持 `/skill <name>` 显式调用
3. ✅ 支持基于描述的自动匹配（可配置开关）
4. ✅ 内置 3 个 Skills 可用（refine-text, translate, summarize）
5. ✅ Skill 指令正确注入到 system prompt
6. ✅ 与现有 Capability 系统（Memory/Search/Video）协同工作
7. ✅ Skills 热加载（目录变更检测）
8. ✅ 遵循 Strategy Pattern 架构

## Design Decisions

### Decision 1: Capability 优先级

**选择**：`Skills = 4`（在 Video 之后）

**理由**：
- Skills 是指令增强，应在所有上下文收集（Memory, Search, Video）完成后执行
- 这样 Skill 指令可以引用其他 Capability 提供的上下文
- 与 Claude Code 的 Skills 执行顺序一致

### Decision 2: Strategy Pattern 集成

**选择**：实现 `CapabilityStrategy` trait

**理由**：
- 与现有架构（MemoryStrategy, SearchStrategy, VideoStrategy）保持一致
- 低耦合：SkillsStrategy 可独立测试和替换
- 高内聚：所有 Skill 相关逻辑封装在 skills/ 模块

### Decision 3: 自动匹配默认关闭

**选择**：`auto_match_enabled = false` 为默认值

**理由**：
- 防止误匹配导致意外行为
- 用户需要显式启用才会触发自动匹配
- 始终可以通过 `/skill <name>` 显式调用

### Decision 4: 与 Phantom Flow 解耦

**选择**：完全解耦

**理由**：
- Skills 本身是指令注入，不需要用户交互
- 如果 Skill 需要参数，可以独立调用 Phantom Flow
- 保持两个系统的单一职责

## References

- [Claude Agent Skills Overview](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview)
- [Anthropic Skills GitHub](https://github.com/anthropics/skills)
- [Simon Willison: Claude Skills](https://simonwillison.net/2025/Oct/16/claude-skills/)
- Current CapabilityStrategy: `Aether/core/src/capability/strategy.rs`
- Existing Strategies: `Aether/core/src/capability/strategies/`
