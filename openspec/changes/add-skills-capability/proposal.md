# Change: Add Skills Capability (Claude Agent Skills Standard)

**Status**: Deployed
**Author**: AI Assistant
**Created**: 2026-01-07
**Updated**: 2026-01-08
**Deployed**: 2026-01-08

## Why

[Claude Agent Skills](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview) 是 Anthropic 发布的开放标准，用于扩展 AI Agent 的能力。Skills 不是可执行代码，而是**动态指令注入**——教 Claude 如何完成特定任务的结构化指南。

OpenAI、GitHub Copilot 等已采用相同规范。Aether 作为 OS-Level AI 中间件，需要支持此开放标准：
- 用户可以创建/分享 Skills
- 与 Claude Code、GitHub Copilot 等工具兼容
- 扩展 Aether 的任务处理能力

## What Changes

本提案分为两大部分：**Rust Core 集成** 和 **Swift UI 管理界面**。

---

### Part A: Rust Core Architecture Integration

采用现有的 **Strategy Pattern** 架构，将 Skills 作为一个独立的 Capability 实现：

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

#### New Rust Components

1. **Capability::Skills (payload/capability.rs)**
   - 添加 `Skills = 4` 变体（Video 之后执行）

2. **SkillsStrategy (capability/strategies/skills.rs)**
   - 实现 `CapabilityStrategy` trait
   - 从 `SkillsRegistry` 加载匹配的 Skill

3. **SkillsRegistry (skills/registry.rs)**
   - 管理 Skills 目录 (`~/.aether/skills/`)
   - 解析 SKILL.md 文件（YAML frontmatter + Markdown body）
   - 支持热重载

4. **SkillsInstaller (skills/installer.rs)** — NEW
   - 从 GitHub 仓库克隆/下载 Skills
   - 解压 ZIP 文件安装 Skills
   - 验证 SKILL.md 格式

---

### Part B: Skills Settings UI (Swift)

在设置界面添加 **Skills 管理标签页**，提供完整的 Skills 管理功能：

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        Settings - Skills                                 │
├─────────────────────────────────────────────────────────────────────────┤
│ ┌─────────────────────────────────────────────────────────────────────┐ │
│ │  搜索栏: [🔍 Search skills...]                                      │ │
│ └─────────────────────────────────────────────────────────────────────┘ │
│                                                                         │
│ ┌─ Installed Skills ────────────────────────────────────────────────┐   │
│ │ ┌─────────────────────────────────────────────────────────────┐   │   │
│ │ │ 📝 refine-text                                         [🗑️] │   │   │
│ │ │ Improve and polish writing                                   │   │   │
│ │ └─────────────────────────────────────────────────────────────┘   │   │
│ │ ┌─────────────────────────────────────────────────────────────┐   │   │
│ │ │ 🌐 translate                                           [🗑️] │   │   │
│ │ │ Translate text between languages                             │   │   │
│ │ └─────────────────────────────────────────────────────────────┘   │   │
│ │ ┌─────────────────────────────────────────────────────────────┐   │   │
│ │ │ 📋 summarize                                           [🗑️] │   │   │
│ │ │ Summarize long content into concise form                     │   │   │
│ │ └─────────────────────────────────────────────────────────────┘   │   │
│ └───────────────────────────────────────────────────────────────────┘   │
│                                                                         │
│ ┌─ Install Options ─────────────────────────────────────────────────┐   │
│ │ [📦 Install Official Skills]  Download from anthropics/skills     │   │
│ │                                                                   │   │
│ │ [🔗 Install from URL]  github.com/user/skill-repo                │   │
│ │                                                                   │   │
│ │ [📁 Upload ZIP]  Click to select ZIP file                        │   │
│ └───────────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────────┘
```

#### UI Features

1. **Skills 列表展示**
   - 显示所有已安装的 Skills
   - 卡片式布局（名称、描述）
   - 支持搜索过滤
   - 删除操作（带确认对话框）

2. **官方 Skills 一键安装**
   - 按钮触发从 `anthropics/skills` 仓库下载
   - 显示下载进度
   - 自动解析并安装到 `~/.aether/skills/`

3. **第三方 Skills 安装（URL）**
   - URL 输入框（支持 GitHub 仓库地址）
   - 格式：`github.com/user/skill-name` 或完整 URL
   - 自动下载并安装

4. **ZIP 文件上传安装**
   - 文件选择器选择 ZIP 文件
   - 自动解压到 skills 目录
   - 验证 SKILL.md 存在

---

## Impact

- **Affected specs**:
  - `skills-capability` (MODIFIED) - 添加 installer 和 UI 规范
  - `skills-settings-ui` (NEW) - Skills UI 规范

- **Affected code** (按依赖顺序):

  ```
  Phase 1-4: Rust Core (已有)
  ├── Aether/core/src/skills/mod.rs
  ├── Aether/core/src/skills/registry.rs
  ├── Aether/core/src/capability/strategies/skills.rs
  └── ...

  Phase 5: Skills Installer (Rust)
  ├── Aether/core/src/skills/installer.rs (NEW)
  └── Aether/core/src/skills/mod.rs (ADD installer exports)

  Phase 6: UniFFI Interface
  ├── Aether/core/src/aether.udl (ADD Skill types and methods)
  └── Aether/Sources/Generated/aether.swift (REGENERATE)

  Phase 7: Skills Settings UI (Swift)
  ├── Aether/Sources/SkillsSettingsView.swift (NEW)
  ├── Aether/Sources/Components/Molecules/SkillCard.swift (NEW)
  └── Aether/Sources/SettingsView.swift (ADD .skills tab)

  Phase 8: Localization
  └── Aether/Resources/Localizations/*.lproj/Localizable.strings
  ```

- **Breaking changes**: None（增量添加）

---

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

### 官方 Skills 仓库

- **地址**: https://github.com/anthropics/skills
- **结构**:
  ```
  anthropics/skills/
  ├── skills/           # 分类 Skills
  ├── spec/             # 规范文档
  └── template/         # 创建模板
  ```
- **安装方式**: 克隆仓库或下载 ZIP

---

## Success Criteria

### Core Functionality
1. ✅ 支持 Claude Agent Skills 标准格式（SKILL.md）
2. ✅ 支持 `/skill <name>` 显式调用
3. ✅ 支持基于描述的自动匹配（可配置开关）
4. ✅ 内置 3 个 Skills 可用（refine-text, translate, summarize）
5. ✅ Skill 指令正确注入到 system prompt
6. ✅ 与现有 Capability 系统协同工作
7. ✅ Skills 热加载

### UI Management
8. ✅ Settings 界面显示 Skills 列表
9. ✅ 一键安装官方 Skills
10. ✅ URL 输入安装第三方 Skills
11. ✅ ZIP 文件上传安装
12. ✅ Skills 删除功能（带确认对话框）
13. ✅ 搜索过滤 Skills

---

## Design Decisions

### Decision 1: Capability 优先级

**选择**：`Skills = 4`（在 Video 之后）

**理由**：
- Skills 是指令增强，应在所有上下文收集完成后执行
- 与 Claude Code 的 Skills 执行顺序一致

### Decision 2: Strategy Pattern 集成

**选择**：实现 `CapabilityStrategy` trait

**理由**：
- 与现有架构（MemoryStrategy, SearchStrategy, VideoStrategy）保持一致
- 低耦合高内聚

### Decision 3: 自动匹配默认关闭

**选择**：`auto_match_enabled = false` 为默认值

**理由**：
- 防止误匹配导致意外行为
- 用户需要显式启用

### Decision 4: Skills UI 标签页位置

**选择**：在 Search 之后，作为独立标签页

**理由**：
- Skills 是高级功能，但常用
- 与 Providers、Routing、Search 等配置平级
- 不嵌入其他设置以保持清晰

### Decision 5: 安装方式优先级

**选择**：
1. 官方一键安装（推荐）
2. GitHub URL 安装
3. ZIP 上传安装

**理由**：
- 从最简单到最灵活的渐进式体验
- 官方 Skills 经过验证，最安全
- 不提供编辑功能，保持 UI 简洁

### Decision 6: 不提供编辑功能

**选择**：Settings UI 只提供安装和删除功能，不提供编辑

**理由**：
- 减少 UI 复杂度
- Skills 创建/编辑是高级操作，可直接编辑 `~/.aether/skills/<name>/SKILL.md`
- 保持设置界面轻量化

---

## References

- [Claude Agent Skills Overview](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview)
- [Anthropic Skills GitHub](https://github.com/anthropics/skills)
- [Simon Willison: Claude Skills](https://simonwillison.net/2025/Oct/16/claude-skills/)
- Current CapabilityStrategy: `Aether/core/src/capability/strategy.rs`
- Existing Settings UI: `Aether/Sources/SettingsView.swift`
- UI Components: `Aether/Sources/Components/`
