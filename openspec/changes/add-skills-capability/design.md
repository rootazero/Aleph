# Design: Skills Capability (Claude Agent Skills Standard)

## Context

Claude Agent Skills 是 Anthropic 发布的开放标准，用于教 Claude 如何完成特定任务。Skills 本质上是**动态 system prompt 注入**，而非可执行代码。

> "Skills are not executable code. They do NOT run Python or JavaScript. Instead of executing discrete actions and returning results, skills inject comprehensive instruction sets that modify how Claude reasons about and approaches the task."

### Stakeholders

- **终端用户**：需要扩展 AI 能力，创建/使用 Skills
- **开发者**：需要与 Claude Code、GitHub Copilot 兼容的格式
- **Aether 架构**：需要与 Capability 系统集成

### Constraints

1. **遵循开放标准**：SKILL.md 格式与 Anthropic 规范一致
2. **与 Capability 系统集成**：作为 `Capability::Skills` 执行
3. **与 Phantom Flow 解耦**：Skills 可以使用 Phantom Flow，但不依赖它
4. **热加载**：Skills 变更无需重启应用

## Goals / Non-Goals

### Goals

- 实现 Claude Agent Skills 标准
- 支持显式调用（`/skill <name>`）和自动匹配
- 与现有 Capability 管道集成
- 提供内置 Skills

### Non-Goals

- 工具执行（`allowed-tools` 预留给 MCP 集成）
- Skills 市场/分享平台
- Skills 版本管理

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                         User Input                               │
│  "Refine this paragraph" or "/skill refine-text ..."             │
└──────────────────────────────┬──────────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│                      Router (Rust Core)                          │
│  1. Check /skill command → explicit skill name                   │
│  2. OR match input against skill descriptions → auto-match       │
│  3. Set intent_type = "skills:<skill-name>"                      │
└──────────────────────────────┬──────────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│                   PayloadBuilder (Rust Core)                     │
│  4. Detect intent_type starts with "skills:"                     │
│  5. Add Capability::Skills to payload.config.capabilities        │
│  6. Set payload.meta.skill_id = "<skill-name>"                   │
└──────────────────────────────┬──────────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│                CapabilityExecutor (Rust Core)                    │
│  Execute order: Memory → Search → MCP → Video → Skills           │
│                                                                  │
│  7. execute_skills():                                            │
│     a. Load Skill from registry by ID                            │
│     b. Parse SKILL.md (frontmatter + body)                       │
│     c. Extract instructions (markdown body)                      │
│     d. Set payload.context.skill_instructions = instructions     │
└──────────────────────────────┬──────────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│                   PromptAssembler (Rust Core)                    │
│  8. Build final system prompt:                                   │
│     [Base system prompt]                                         │
│     [Memory context]                                             │
│     [Search results]                                             │
│     [Skill instructions]  ← injected here                        │
└──────────────────────────────┬──────────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────────┐
│                       AI Provider                                │
│  9. Process with enhanced system prompt                          │
└─────────────────────────────────────────────────────────────────┘
```

## SKILL.md Parsing

### File Structure

```
~/.config/aether/skills/
├── refine-text/
│   └── SKILL.md
├── translate/
│   └── SKILL.md
└── summarize/
    └── SKILL.md
```

### SKILL.md Format

```markdown
---
name: refine-text
description: Improve and polish writing. Use when asked to refine, improve, or enhance text.
allowed-tools:
  - Read
  - Edit
---

# Instructions

[Markdown content that will be injected into system prompt]
```

### Parsing Logic (Rust)

```rust
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
}

pub struct Skill {
    pub frontmatter: SkillFrontmatter,
    pub instructions: String, // Markdown body
}

impl Skill {
    pub fn parse(content: &str) -> Result<Self> {
        // Split frontmatter and body
        let parts: Vec<&str> = content.splitn(3, "---").collect();
        if parts.len() < 3 {
            return Err("Invalid SKILL.md format");
        }

        let frontmatter: SkillFrontmatter = serde_yaml::from_str(parts[1])?;
        let instructions = parts[2].trim().to_string();

        Ok(Self { frontmatter, instructions })
    }
}
```

## Skill Matching

### Explicit Matching (`/skill <name>`)

```rust
// Router checks for /skill command
if input.starts_with("/skill ") {
    let skill_name = input[7..].split_whitespace().next();
    return Some(RoutingMatch {
        intent_type: Some(format!("skills:{}", skill_name)),
        ...
    });
}
```

### Auto-Matching (Description-based)

```rust
// Registry checks each skill's description
impl SkillsRegistry {
    pub fn find_matching_skill(&self, input: &str) -> Option<&Skill> {
        let input_lower = input.to_lowercase();

        for skill in &self.skills {
            // Simple keyword matching (MVP)
            // Future: Use embedding similarity
            let desc_lower = skill.frontmatter.description.to_lowercase();
            if Self::matches_description(&input_lower, &desc_lower) {
                return Some(skill);
            }
        }
        None
    }

    fn matches_description(input: &str, description: &str) -> bool {
        // Extract keywords from description
        // Check if input contains any of them
        // MVP: Simple substring matching
        description.split_whitespace()
            .filter(|w| w.len() > 3)
            .any(|keyword| input.contains(keyword))
    }
}
```

## Decisions

### Decision 1: Skill 文件命名

**选择**：`SKILL.md`（全大写）

**理由**：
- 与 Anthropic 官方规范一致
- 与 README.md、LICENSE 等约定一致
- 易于在目录中识别

### Decision 2: 自动匹配策略

**选择**：MVP 使用简单关键词匹配

**理由**：
- 实现简单，无需额外依赖
- 未来可升级为 embedding 相似度匹配
- 用户可以通过 `/skill <name>` 显式调用绕过自动匹配

### Decision 3: 与 Phantom Flow 的关系

**选择**：完全解耦

**理由**：
- Skills 本身是指令注入，不需要用户交互
- 如果 Skill 需要参数，可以独立调用 Phantom Flow
- 保持两个系统的单一职责

### Decision 4: `allowed-tools` 处理

**选择**：MVP 解析但不强制执行

**理由**：
- 工具执行是 MCP 的职责
- MVP 只存储字段，预留给未来 MCP 集成
- 避免过早设计复杂的权限系统

## Data Types

### Rust Core

```rust
// skills/mod.rs
pub mod registry;
pub mod loader;

// skills/registry.rs
pub struct SkillsRegistry {
    skills_dir: PathBuf,
    skills: HashMap<String, Skill>,
}

impl SkillsRegistry {
    pub fn new(skills_dir: PathBuf) -> Self;
    pub fn load_all(&mut self) -> Result<()>;
    pub fn get_skill(&self, name: &str) -> Option<&Skill>;
    pub fn find_matching(&self, input: &str) -> Option<&Skill>;
    pub fn list_skills(&self) -> Vec<&Skill>;
}

// skills/loader.rs
pub struct Skill {
    pub id: String,           // Directory name
    pub name: String,         // From frontmatter
    pub description: String,  // From frontmatter
    pub allowed_tools: Vec<String>,
    pub instructions: String, // Markdown body
}
```

### PayloadContext Extension

```rust
// payload/context.rs
pub struct PayloadContext {
    pub memory_snippets: Option<Vec<MemoryEntry>>,
    pub search_results: Option<Vec<SearchResult>>,
    pub mcp_resources: Option<Vec<McpResource>>,
    pub video_transcript: Option<VideoTranscript>,
    pub skill_instructions: Option<String>,  // NEW
}
```

## Prompt Assembly

```rust
// payload/assembler.rs
impl PromptAssembler {
    pub fn assemble_system_prompt(&self, payload: &AgentPayload) -> String {
        let mut parts = Vec::new();

        // 1. Base system prompt (from rule or default)
        if let Some(base) = &payload.config.system_prompt {
            parts.push(base.clone());
        }

        // 2. Memory context
        if let Some(memories) = &payload.context.memory_snippets {
            parts.push(self.format_memories(memories));
        }

        // 3. Search results
        if let Some(results) = &payload.context.search_results {
            parts.push(self.format_search_results(results));
        }

        // 4. Skill instructions (injected at end for prominence)
        if let Some(instructions) = &payload.context.skill_instructions {
            parts.push(format!("## Skill Instructions\n\n{}", instructions));
        }

        parts.join("\n\n")
    }
}
```

## Built-in Skills

### refine-text

```markdown
---
name: refine-text
description: Improve and polish writing. Use when asked to refine, improve, edit, or enhance text quality.
allowed-tools: []
---

# Refine Text

When refining text, follow these principles:

1. **Clarity**: Remove ambiguity and improve readability
2. **Conciseness**: Eliminate redundancy without losing meaning
3. **Flow**: Ensure logical progression of ideas
4. **Grammar**: Fix errors in grammar, spelling, and punctuation

Preserve the original meaning and intent. Maintain consistent voice and style.
```

### translate

```markdown
---
name: translate
description: Translate text between languages. Use when asked to translate content.
allowed-tools: []
---

# Translate

When translating:

1. Preserve the original meaning and nuance
2. Adapt idioms and cultural references appropriately
3. Maintain the original tone and formality level
4. If target language is not specified, translate to English

Output only the translated text without explanations.
```

### summarize

```markdown
---
name: summarize
description: Summarize long content into concise form. Use when asked to summarize or condense text.
allowed-tools: []
---

# Summarize

When summarizing:

1. Identify the main ideas and key points
2. Preserve essential information
3. Remove redundancy and filler content
4. Maintain logical flow

Default to 2-3 paragraphs unless length is specified.
```

## Risks / Trade-offs

### Risk 1: Auto-matching 误匹配

**缓解**：
- 提供 `/skill <name>` 显式调用
- 未来：让用户确认自动匹配的 Skill

### Risk 2: Skill 指令与用户输入冲突

**缓解**：
- Skill 指令放在 system prompt 末尾
- 用户可以在输入中覆盖 Skill 行为

## Open Questions

1. **Q: 是否支持 Skill 参数？**
   - A: MVP 不支持。如需参数，可调用 Phantom Flow，但 Skills 本身不定义参数。

2. **Q: 是否支持多 Skill 组合？**
   - A: MVP 只支持单个 Skill。未来可支持多 Skill 指令合并。

3. **Q: 是否与 Claude Code Skills 目录结构完全兼容？**
   - A: 目标是兼容 SKILL.md 格式。目录结构可能有差异。
