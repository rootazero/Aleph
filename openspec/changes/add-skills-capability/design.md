# Design: Skills Capability (Claude Agent Skills Standard)

## Context

Claude Agent Skills 是 Anthropic 发布的开放标准，用于教 Claude 如何完成特定任务。Skills 本质上是**动态 system prompt 注入**，而非可执行代码。

> "Skills are not executable code. They do NOT run Python or JavaScript. Instead of executing discrete actions and returning results, skills inject comprehensive instruction sets that modify how Claude reasons about and approaches the task."

### Stakeholders

- **终端用户**：需要扩展 AI 能力，创建/使用 Skills
- **开发者**：需要与 Claude Code、GitHub Copilot 兼容的格式
- **Aether 架构**：需要与现有 Capability Strategy Pattern 集成

### Constraints

1. **遵循开放标准**：SKILL.md 格式与 Anthropic 规范一致
2. **Strategy Pattern 集成**：作为 `CapabilityStrategy` 实现
3. **低耦合高内聚**：与现有模块（Memory, Search, Video）保持相同设计模式
4. **热加载**：Skills 变更无需重启应用

## Goals / Non-Goals

### Goals

- 实现 Claude Agent Skills 标准（SKILL.md 格式）
- 作为 `CapabilityStrategy` 集成到现有架构
- 支持显式调用（`/skill <name>`）和自动匹配
- 提供内置 Skills（refine-text, translate, summarize）

### Non-Goals

- 工具执行（`allowed-tools` 预留给 MCP 集成）
- Skills 市场/分享平台
- Skills 版本管理
- 多 Skill 组合（MVP 只支持单个 Skill）

## Architecture Overview

### System Integration

```
┌─────────────────────────────────────────────────────────────────────────┐
│                              AetherCore                                  │
│                                                                         │
│  ┌────────────────────────────────────────────────────────────────────┐ │
│  │                    CompositeCapabilityExecutor                      │ │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌───────────┐ │ │
│  │  │ Memory   │ │ Search   │ │ MCP      │ │ Video    │ │  Skills   │ │ │
│  │  │ Strategy │ │ Strategy │ │ Strategy │ │ Strategy │ │  Strategy │ │ │
│  │  │ (0)      │ │ (1)      │ │ (2)      │ │ (3)      │ │  (4)      │ │ │
│  │  └────┬─────┘ └────┬─────┘ └────┬─────┘ └────┬─────┘ └─────┬─────┘ │ │
│  │       │            │            │            │              │       │ │
│  │       └────────────┴────────────┴────────────┴──────────────┘       │ │
│  │                                │                                    │ │
│  │                        AgentPayload                                 │ │
│  │                    (context enrichment)                             │ │
│  └────────────────────────────────────────────────────────────────────┘ │
│                                   │                                     │
│                                   ▼                                     │
│  ┌────────────────────────────────────────────────────────────────────┐ │
│  │                       PromptAssembler                               │ │
│  │  system_prompt = base + memory + search + video + skill_instructions│ │
│  └────────────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────────┘
```

### Data Flow

```
User Input: "/skill refine-text Fix this text"
         │
         ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                              Router                                      │
│  1. Detect /skill command                                               │
│  2. Extract skill_name = "refine-text"                                  │
│  3. Set RoutingDecision.capabilities = [Skills]                         │
│  4. Set payload.meta.skill_id = "refine-text"                           │
└──────────────────────────────┬──────────────────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                        PayloadBuilder                                    │
│  Create AgentPayload with:                                              │
│  - meta.skill_id = "refine-text"                                        │
│  - config.capabilities = [Skills]                                       │
│  - user_input = "Fix this text" (prefix stripped)                       │
└──────────────────────────────┬──────────────────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                   CompositeCapabilityExecutor                            │
│                                                                         │
│  for capability in [Memory, Search, MCP, Video, Skills]:                │
│      if strategy.is_enabled_for(payload):                               │
│          payload = strategy.execute(payload)                            │
│                                                                         │
│  SkillsStrategy.execute():                                              │
│      1. skill = registry.get_skill(payload.meta.skill_id)               │
│      2. payload.context.skill_instructions = skill.instructions         │
│      3. return payload                                                  │
└──────────────────────────────┬──────────────────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                        PromptAssembler                                   │
│                                                                         │
│  final_system_prompt =                                                  │
│      [Base system prompt]                                               │
│      + [Memory context]                                                 │
│      + [Search results]                                                 │
│      + [Video transcript]                                               │
│      + ## Skill Instructions                                            │
│        {skill.instructions}                                             │
└──────────────────────────────┬──────────────────────────────────────────┘
                               ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                          AI Provider                                     │
│  Process with enhanced system prompt                                    │
└─────────────────────────────────────────────────────────────────────────┘
```

## Module Structure

```
Aether/core/src/
├── skills/                      # NEW: Skills module
│   ├── mod.rs                   # Module exports, Skill struct
│   └── registry.rs              # SkillsRegistry implementation
│
├── capability/
│   ├── strategy.rs              # CapabilityStrategy trait (existing)
│   └── strategies/
│       ├── mod.rs               # ADD: export skills
│       ├── memory.rs            # Existing
│       ├── search.rs            # Existing
│       ├── mcp.rs               # Existing
│       ├── video.rs             # Existing
│       └── skills.rs            # NEW: SkillsStrategy
│
├── payload/
│   ├── mod.rs                   # ADD: skill_instructions to AgentContext
│   ├── capability.rs            # ADD: Capability::Skills
│   └── assembler.rs             # ADD: skill_instructions injection
│
└── config/
    └── mod.rs                   # ADD: SkillsConfig
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

/// YAML frontmatter from SKILL.md
#[derive(Debug, Clone, Deserialize)]
pub struct SkillFrontmatter {
    /// Skill name (must match directory name)
    pub name: String,
    /// Description for auto-matching
    pub description: String,
    /// Allowed tools (reserved for MCP)
    #[serde(default)]
    pub allowed_tools: Vec<String>,
}

/// A parsed Skill from SKILL.md
#[derive(Debug, Clone)]
pub struct Skill {
    /// Directory name (unique identifier)
    pub id: String,
    /// Parsed frontmatter
    pub frontmatter: SkillFrontmatter,
    /// Markdown body (instructions)
    pub instructions: String,
}

impl Skill {
    /// Parse SKILL.md content
    pub fn parse(id: &str, content: &str) -> Result<Self> {
        // Split frontmatter and body
        let parts: Vec<&str> = content.splitn(3, "---").collect();
        if parts.len() < 3 {
            return Err(AetherError::invalid_config("Invalid SKILL.md format"));
        }

        let frontmatter: SkillFrontmatter = serde_yaml::from_str(parts[1])?;
        let instructions = parts[2].trim().to_string();

        Ok(Self {
            id: id.to_string(),
            frontmatter,
            instructions,
        })
    }
}
```

## SkillsStrategy Implementation

```rust
use crate::capability::strategy::CapabilityStrategy;
use crate::error::Result;
use crate::payload::{AgentPayload, Capability};
use crate::skills::SkillsRegistry;
use async_trait::async_trait;
use std::sync::Arc;

/// Skills capability strategy
///
/// Loads skill instructions from the registry and injects them into
/// the payload context for prompt assembly.
pub struct SkillsStrategy {
    /// Skills registry
    registry: Option<Arc<SkillsRegistry>>,
    /// Enable auto-matching by description
    auto_match_enabled: bool,
}

impl SkillsStrategy {
    pub fn new(registry: Option<Arc<SkillsRegistry>>, auto_match_enabled: bool) -> Self {
        Self {
            registry,
            auto_match_enabled,
        }
    }
}

#[async_trait]
impl CapabilityStrategy for SkillsStrategy {
    fn capability_type(&self) -> Capability {
        Capability::Skills
    }

    fn priority(&self) -> u32 {
        4 // After Video (3)
    }

    fn is_available(&self) -> bool {
        self.registry.is_some()
    }

    async fn execute(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
        let Some(registry) = &self.registry else {
            return Ok(payload);
        };

        // Get skill by explicit ID or auto-match
        let skill = if let Some(ref skill_id) = payload.meta.skill_id {
            // Explicit skill ID from /skill command
            registry.get_skill(skill_id)
        } else if self.auto_match_enabled {
            // Auto-match by description
            registry.find_matching(&payload.user_input)
        } else {
            None
        };

        if let Some(skill) = skill {
            tracing::info!(
                skill_id = %skill.id,
                skill_name = %skill.frontmatter.name,
                "Loading skill instructions"
            );
            payload.context.skill_instructions = Some(skill.instructions.clone());
        }

        Ok(payload)
    }
}
```

## SkillsRegistry Implementation

```rust
use crate::error::Result;
use crate::skills::Skill;
use notify::{Watcher, RecursiveMode, Event};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Skills registry with hot-reload support
pub struct SkillsRegistry {
    /// Skills directory path
    skills_dir: PathBuf,
    /// Loaded skills (id -> Skill)
    skills: RwLock<HashMap<String, Skill>>,
}

impl SkillsRegistry {
    /// Create a new registry and load all skills
    pub fn new(skills_dir: PathBuf) -> Result<Self> {
        let registry = Self {
            skills_dir,
            skills: RwLock::new(HashMap::new()),
        };
        registry.load_all()?;
        Ok(registry)
    }

    /// Load all skills from the skills directory
    pub fn load_all(&self) -> Result<()> {
        let mut skills = self.skills.write().unwrap();
        skills.clear();

        if !self.skills_dir.exists() {
            return Ok(());
        }

        for entry in std::fs::read_dir(&self.skills_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let skill_file = path.join("SKILL.md");
                if skill_file.exists() {
                    let id = path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string();

                    let content = std::fs::read_to_string(&skill_file)?;
                    match Skill::parse(&id, &content) {
                        Ok(skill) => {
                            skills.insert(id, skill);
                        }
                        Err(e) => {
                            tracing::warn!(
                                skill_id = %id,
                                error = %e,
                                "Failed to parse SKILL.md"
                            );
                        }
                    }
                }
            }
        }

        tracing::info!(
            skills_count = skills.len(),
            "Loaded skills from registry"
        );

        Ok(())
    }

    /// Get a skill by ID (directory name)
    pub fn get_skill(&self, id: &str) -> Option<Skill> {
        self.skills.read().unwrap().get(id).cloned()
    }

    /// Find a skill matching the input (by description keywords)
    pub fn find_matching(&self, input: &str) -> Option<Skill> {
        let input_lower = input.to_lowercase();
        let skills = self.skills.read().unwrap();

        for skill in skills.values() {
            let desc_lower = skill.frontmatter.description.to_lowercase();
            // Simple keyword matching: check if description keywords appear in input
            let keywords: Vec<&str> = desc_lower
                .split_whitespace()
                .filter(|w| w.len() > 3)
                .collect();

            let matches = keywords.iter().filter(|k| input_lower.contains(*k)).count();
            if matches >= 2 {
                return Some(skill.clone());
            }
        }

        None
    }

    /// List all loaded skills
    pub fn list_skills(&self) -> Vec<Skill> {
        self.skills.read().unwrap().values().cloned().collect()
    }
}
```

## Configuration

### config.toml

```toml
[skills]
enabled = true
skills_dir = "~/.config/aether/skills"  # Default path
auto_match_enabled = false              # Disable auto-matching by default
```

### SkillsConfig (Rust)

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct SkillsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default = "default_skills_dir")]
    pub skills_dir: String,

    #[serde(default)]
    pub auto_match_enabled: bool,
}

fn default_true() -> bool { true }

fn default_skills_dir() -> String {
    dirs::config_dir()
        .map(|p| p.join("aether").join("skills"))
        .unwrap_or_else(|| PathBuf::from("~/.config/aether/skills"))
        .to_string_lossy()
        .to_string()
}
```

## PayloadMeta Extension

Add `skill_id` field to `PayloadMeta`:

```rust
/// Payload metadata
#[derive(Debug, Clone)]
pub struct PayloadMeta {
    pub intent: Intent,
    pub timestamp: i64,
    pub context_anchor: ContextAnchor,
    /// Explicit skill ID from /skill command
    pub skill_id: Option<String>,  // NEW
}
```

## AgentContext Extension

Add `skill_instructions` field:

```rust
/// Agent context (extension area)
#[derive(Debug, Clone, Default)]
pub struct AgentContext {
    pub memory_snippets: Option<Vec<MemoryEntry>>,
    pub search_results: Option<Vec<SearchResult>>,
    pub mcp_resources: Option<HashMap<String, serde_json::Value>>,
    pub workflow_state: Option<WorkflowState>,
    pub attachments: Option<Vec<MediaAttachment>>,
    pub video_transcript: Option<VideoTranscript>,
    pub skill_instructions: Option<String>,  // NEW
}
```

## PromptAssembler Extension

```rust
impl PromptAssembler {
    pub fn assemble_system_prompt(&self, payload: &AgentPayload, base_prompt: &str) -> String {
        let mut parts = vec![base_prompt.to_string()];

        // Memory context
        if let Some(ref memories) = payload.context.memory_snippets {
            if !memories.is_empty() {
                parts.push(self.format_memories(memories));
            }
        }

        // Search results
        if let Some(ref results) = payload.context.search_results {
            if !results.is_empty() {
                parts.push(self.format_search_results(results));
            }
        }

        // Video transcript
        if let Some(ref transcript) = payload.context.video_transcript {
            parts.push(self.format_video_transcript(transcript));
        }

        // Skill instructions (NEW - at the end for prominence)
        if let Some(ref instructions) = payload.context.skill_instructions {
            parts.push(format!("## Skill Instructions\n\n{}", instructions));
        }

        parts.join("\n\n")
    }
}
```

## Built-in Skills

### refine-text/SKILL.md

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

### translate/SKILL.md

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

### summarize/SKILL.md

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

## Decisions

### Decision 1: Capability 优先级

**选择**：`Skills = 4`（在 Video 之后）

**权衡**：
- 优点：Skills 可以引用其他 Capability 的上下文
- 缺点：Skills 执行时间稍晚

**理由**：Skills 是指令增强，应在所有上下文收集完成后执行

### Decision 2: 自动匹配策略

**选择**：MVP 使用简单关键词匹配，默认关闭

**权衡**：
- 优点：实现简单，无需额外依赖
- 缺点：匹配精度不如 embedding 相似度

**理由**：用户可以通过 `/skill <name>` 显式调用绕过自动匹配

### Decision 3: PayloadMeta.skill_id vs Intent

**选择**：添加 `skill_id` 字段到 `PayloadMeta`

**权衡**：
- 方案 A：使用 `Intent::Skills(skill_id)` 变体
- 方案 B：添加独立字段 `skill_id`

**理由**：方案 B 更灵活，允许 Skill 与其他 Intent 组合使用

### Decision 4: 热加载实现

**选择**：MVP 使用简单的 `load_all()` 重载，不使用 notify

**权衡**：
- 优点：实现简单，依赖少
- 缺点：需要手动触发重载

**理由**：MVP 阶段简化实现，后续可添加 file watcher

## Risks / Trade-offs

### Risk 1: Auto-matching 误匹配

**缓解**：
- 默认关闭自动匹配
- 提供 `/skill <name>` 显式调用
- 未来：让用户确认自动匹配的 Skill

### Risk 2: Skill 指令与用户输入冲突

**缓解**：
- Skill 指令放在 system prompt 末尾
- 用户可以在输入中覆盖 Skill 行为

### Risk 3: Skills 目录不存在

**缓解**：
- 首次启动时创建目录
- 复制内置 Skills 到用户目录（如不存在）

## Open Questions

1. **Q: 是否支持 Skill 参数？**
   - A: MVP 不支持。Skill 指令是静态的。如需参数化，用户可在输入中指定。

2. **Q: 是否支持多 Skill 组合？**
   - A: MVP 只支持单个 Skill。未来可支持多 Skill 指令合并。

3. **Q: 是否与 Claude Code Skills 目录结构完全兼容？**
   - A: 目标是兼容 SKILL.md 格式。目录结构可能有差异（Aether 使用 `~/.config/aether/skills/`）。

4. **Q: 如何处理 `allowed-tools` 字段？**
   - A: MVP 解析但不强制执行。预留给 MCP 集成。
