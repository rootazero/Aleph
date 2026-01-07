# Change: Add Skills Capability (Claude Agent Skills Standard)

**Status**: Draft
**Author**: AI Assistant
**Created**: 2026-01-07

## Why

[Claude Agent Skills](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview) 是 Anthropic 发布的开放标准，用于扩展 AI Agent 的能力。Skills 不是可执行代码，而是**动态指令注入**——教 Claude 如何完成特定任务的结构化指南。

OpenAI、GitHub Copilot 等已采用相同规范。Aether 作为 OS-Level AI 中间件，需要支持此开放标准：
- 用户可以创建/分享 Skills
- 与 Claude Code、GitHub Copilot 等工具兼容
- 扩展 Aether 的任务处理能力

## What Changes

### New Capabilities

1. **Skills Registry（Rust Core）**
   - 扫描 `~/.config/aether/skills/` 目录
   - 解析 `SKILL.md` 文件（YAML frontmatter + Markdown instructions）
   - 支持 Claude Agent Skills 标准字段：`name`, `description`, `allowed-tools`

2. **Skills Loader（Rust Core）**
   - 根据用户输入匹配 Skill（基于 `description` 字段）
   - 支持显式调用：`/skill <name>`
   - 支持自动匹配：当输入匹配 Skill 描述时自动加载

3. **Skills Injection（Rust Core）**
   - 将匹配的 Skill 指令注入到 system prompt
   - 处理 `allowed-tools` 约束
   - 与 Memory/Search 等 Capability 协同工作

4. **Built-in Skills（Resources）**
   - 内置常用 Skills：`refine-text`, `translate`, `summarize`
   - 首次启动时复制到用户目录

### Modified Capabilities

- **Capability 枚举**：添加 `Skills = 5` 变体
- **CapabilityExecutor**：添加 `execute_skills()` 方法
- **PayloadContext**：添加 `skill_instructions: Option<String>` 字段
- **PromptAssembler**：处理 Skill 指令注入

## Impact

- **Affected specs**:
  - `skills-capability` (NEW) - Skills 系统规范

- **Affected code**:
  - `Aether/core/src/skills/` - 新增模块
  - `Aether/core/src/capability/mod.rs` - 添加 Skills 执行器
  - `Aether/core/src/payload/capability.rs` - 添加 Skills 变体
  - `Aether/core/src/payload/context.rs` - 添加 skill_instructions 字段
  - `Aether/core/src/payload/assembler.rs` - 处理 Skill 注入
  - `Aether/Resources/skills/` - 内置 Skills 资源

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

1. 支持 Claude Agent Skills 标准格式（SKILL.md）
2. 支持 `/skill <name>` 显式调用
3. 支持基于描述的自动匹配
4. 内置 3 个 Skills 可用
5. Skill 指令正确注入到 system prompt
6. 与 Memory/Search Capability 协同工作
7. Skills 热加载（目录变更检测）

## References

- [Claude Agent Skills Overview](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview)
- [Anthropic Skills GitHub](https://github.com/anthropics/skills)
- [Simon Willison: Claude Skills](https://simonwillison.net/2025/Oct/16/claude-skills/)
