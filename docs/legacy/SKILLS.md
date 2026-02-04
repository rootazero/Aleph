# Claude Agent Skills Integration

Aleph supports the [Claude Agent Skills](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview) open standard, enabling users to extend AI capabilities through structured instruction injection.

## Table of Contents

- [Overview](#overview)
- [Progressive Disclosure Architecture](#progressive-disclosure-architecture)
- [Hybrid Mode](#hybrid-mode)
- [SKILL.md Format](#skillmd-format)
- [Using Skills](#using-skills)
- [Installing Skills](#installing-skills)
- [Managing Skills](#managing-skills)
- [Architecture](#architecture)
- [Configuration](#configuration)
- [Built-in Skills](#built-in-skills)
- [Creating Custom Skills](#creating-custom-skills)

---

## Overview

Claude Agent Skills are **not executable code** but rather **structured instruction guides** that teach AI how to handle specific tasks. Skills are defined in `SKILL.md` files with YAML frontmatter and Markdown body.

**Key Benefits:**
- **Extensible**: Users can create and share Skills
- **Interoperable**: Compatible with Claude Code, GitHub Copilot, and other tools using the same standard
- **Safe**: No code execution - only instruction injection
- **Portable**: Simple text files, easy to backup and share
- **Token Efficient**: Progressive Disclosure reduces unnecessary token consumption

---

## Progressive Disclosure Architecture

Aleph implements Claude's official **Progressive Disclosure** pattern for skills:

### Three-Level Loading

| Level | Timing | Token Cost | Content |
|-------|--------|------------|---------|
| **Level 1: Metadata** | Startup | ~100 tokens/skill | name + description (YAML frontmatter) |
| **Level 2: Instructions** | On-demand | <5k tokens | SKILL.md body via `read_skill` tool |
| **Level 3: Resources** | On-demand | Unlimited | ADVANCED.md, REFERENCE.md, scripts |

### Why Progressive Disclosure?

**Problem with Pre-loading**:
- Full instructions injected into system prompt = "background context"
- LLM may treat as "reference information" and ignore
- Wastes tokens for unused skills

**Progressive Disclosure Solution**:
- System prompt only contains metadata (name + description)
- Agent actively calls `read_skill(skill_id)` when needed
- Instructions returned from tool = **task directive** (must follow)
- Agent treats tool results as commands, not suggestions

### Mental Model Comparison

```
❌ Pre-loading (Old):
┌─────────────────────────────────────────────────────┐
│ System Prompt + Context:                            │
│   ## Skill Instructions                             │  ← As context
│   [Full SKILL.md content]                           │  ← Mixed with Memory etc.
└─────────────────────────────────────────────────────┘
                    ↓
Agent: "This is reference info, I can choose to ignore" ← Problem!


✅ Progressive Disclosure (Current):
┌─────────────────────────────────────────────────────┐
│ System Prompt: "Available Skills: refine-text..."   │  ← Only metadata
└─────────────────────────────────────────────────────┘
                    ↓
Agent: "I'll use the refine-text skill"
→ Decision: UseTool { read_skill, {id: "refine-text"} }  ← Agent decides
                    ↓
┌─────────────────────────────────────────────────────┐
│ Tool Result: [SKILL.md full content]                │
│ → Agent treats as "task directive to execute"       │  ← Task instruction
└─────────────────────────────────────────────────────┘
```

---

## Hybrid Mode

Aleph uses a **hybrid approach** that combines the best of both modes:

### 1. Slash Command Mode (Pre-load)

When user explicitly invokes a skill via slash command:

```
/refine-text Please improve this paragraph...
```

- Instructions are **pre-loaded** into context
- Immediate execution without extra tool call
- Agent knows user explicitly wants this skill

### 2. General Chat Mode (Progressive Disclosure)

When user's intent implies a skill:

```
"帮我润色这段文字"
```

- System prompt shows skill metadata only
- Agent sees "Available Skills: refine-text - Improve and polish writing"
- Agent decides to call `read_skill("refine-text")`
- Returns full instructions → Agent executes

### Mode Selection Logic

```rust
if context.is_slash_command {
    // Pre-load instructions immediately
    SkillLoadMode::PreLoad { instructions }
} else {
    // Let agent discover and load on-demand
    SkillLoadMode::Progressive
}
```

---

## Skill Tools

### read_skill

Read skill instructions or additional resources:

```json
{
  "name": "read_skill",
  "description": "Read the instructions of an installed skill...",
  "parameters": {
    "skill_id": "string (required)",
    "file_name": "string (optional, default: SKILL.md)"
  }
}
```

**Examples**:
- `read_skill(skill_id="refine-text")` → Read SKILL.md
- `read_skill(skill_id="code-review", file_name="CHECKLIST.md")` → Read Level 3 resource

### list_skills

List all available skills with metadata:

```json
{
  "name": "list_skills",
  "description": "List all available skills installed on the system...",
  "parameters": {
    "filter": "string (optional)"
  }
}
```

**Output**:
```json
{
  "success": true,
  "count": 3,
  "skills": [
    {
      "id": "refine-text",
      "name": "Refine Text",
      "description": "Improve and polish writing",
      "triggers": ["refine", "polish"],
      "files": ["SKILL.md", "EXAMPLES.md"]
    }
  ]
}
```

---

## SKILL.md Format

Skills are defined using the Claude Agent Skills standard format:

```markdown
---
name: skill-name
description: Brief description of what this skill does and when to use it.
allowed-tools:
  - Read
  - Edit
  - Write
---

# Skill Title

Detailed instructions for the AI to follow when this skill is activated.

## Guidelines

- Guideline 1
- Guideline 2

## Examples

Example usage patterns...
```

### Frontmatter Fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique identifier for the skill (lowercase, hyphens allowed) |
| `description` | Yes | Brief description shown in UI and used for matching |
| `allowed-tools` | No | List of tools the skill can use (informational) |

### Markdown Body

The body contains detailed instructions that are injected into the system prompt when the skill is activated. This can include:

- Guidelines and principles
- Step-by-step procedures
- Examples and templates
- Constraints and boundaries

---

## Using Skills

### Explicit Invocation

Use the `/skill` command followed by the skill name:

```
/skill refine-text Please improve this paragraph...
```

### Auto-Matching (Optional)

When enabled in configuration, Aleph can automatically match user intent to appropriate skills based on the skill's description.

```toml
[skills]
enabled = true
auto_match_enabled = true  # Default: false
```

---

## Installing Skills

Aleph provides three methods to install Skills:

### 1. GitHub URL Installation

From Settings > Skills, click "Install Skill" and enter a GitHub URL:

- Short format: `user/repo`
- Medium format: `github.com/user/repo`
- Full URL: `https://github.com/user/repo`

The installer will download the repository and extract any valid Skills.

### 2. ZIP File Installation

1. Click "Install Skill" in Settings > Skills
2. Switch to "ZIP File" tab
3. Click "Browse..." to select a ZIP file
4. The installer extracts and validates Skills

**ZIP Structure:**
```
my-skills.zip
├── skill-one/
│   └── SKILL.md
├── skill-two/
│   └── SKILL.md
└── skill-three/
    └── SKILL.md
```

### 3. Manual Installation

Copy skill folders directly to the skills directory:

```bash
~/.aleph/skills/
├── skill-name-1/
│   └── SKILL.md
├── skill-name-2/
│   └── SKILL.md
└── ...
```

---

## Managing Skills

### Settings UI

Navigate to **Settings > Skills** to:

- View all installed skills
- See skill usage hints (`/skill <name>`)
- Delete skills (with confirmation)
- Refresh the skills list

### File System

Skills are stored in `~/.aleph/skills/`. Each skill has its own directory:

```
~/.aleph/skills/
├── refine-text/
│   └── SKILL.md
├── translate/
│   └── SKILL.md
└── summarize/
    └── SKILL.md
```

To edit a skill, modify the `SKILL.md` file directly. Changes take effect immediately (hot reload).

---

## Architecture

### Component Overview

```
┌─────────────────────────────────────────────────────────────────┐
│                        System Prompt                             │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │ ## Available Skills                                        │  │
│  │ - **refine-text**: Improve and polish writing              │  │
│  │ - **translate**: Translate text between languages          │  │
│  │                                                            │  │
│  │ To use a skill:                                            │  │
│  │ 1. Call read_skill(skill_id) to load its instructions     │  │
│  │ 2. Follow the instructions exactly                        │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│                        Tool Registry                             │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐  │
│  │   read_skill    │  │   list_skills   │  │   file_ops      │  │
│  │                 │  │                 │  │                 │  │
│  │ Read SKILL.md   │  │ List available  │  │ File operations │  │
│  │ or resources    │  │ skills          │  │                 │  │
│  └─────────────────┘  └─────────────────┘  └─────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────┐
│                     Skills Directory                             │
│  ~/.aleph/skills/                                        │
│  ├── refine-text/                                                │
│  │   ├── SKILL.md           # Level 2: Instructions             │
│  │   └── EXAMPLES.md        # Level 3: Resources                │
│  ├── translate/                                                  │
│  │   └── SKILL.md                                                │
│  └── summarize/                                                  │
│      └── SKILL.md                                                │
└─────────────────────────────────────────────────────────────────┘
```

### Rust Core Components

| Component | Location | Description |
|-----------|----------|-------------|
| `ReadSkillTool` | `rig_tools/skill_reader.rs` | Read skill instructions (Progressive Disclosure) |
| `ListSkillsTool` | `rig_tools/skill_reader.rs` | List available skills with metadata |
| `Skill` | `skills/mod.rs` | Skill data structure with YAML/Markdown parsing |
| `SkillInfo` | `skills/mod.rs` | FFI-safe skill information for UI |
| `SkillsRegistry` | `skills/registry.rs` | Manages skill loading and lookup |
| `SkillsInstaller` | `skills/installer.rs` | GitHub/ZIP installation |
| `PromptConfig.skill_mode` | `thinker/prompt_builder.rs` | Strict workflow execution mode |

### Execution Flow (Progressive Disclosure)

```
1. Agent Loop starts
   ├─ Load SkillsRegistry
   ├─ Extract all skill metadata
   └─ Inject into "Available Skills" section of system prompt

2. User request: "帮我润色这段文字"
   ↓
3. Thinker analyzes
   ├─ Sees "Available Skills" includes refine-text
   ├─ Decision: UseTool { read_skill, { skill_id: "refine-text" } }
   └─ Issues tool call

4. ReadSkillTool executes
   ├─ Read ~/.aleph/skills/refine-text/SKILL.md
   ├─ Return full content
   └─ ActionResult::ToolSuccess { output: { content: "..." } }

5. Agent receives tool result
   ├─ Treats returned instructions as "task directive"
   ├─ Follows instructions strictly
   └─ Completes task
```

### Security Mechanisms

```rust
// Path traversal prevention
fn validate_skill_id(&self, skill_id: &str) -> Result<()> {
    // ✗ Reject: "..", "/", "\\"
    // ✗ Reject: hidden files (".")
    // ✓ Allow: alphanumeric + hyphen
}

// File size limit
const MAX_FILE_SIZE: u64 = 5 * 1024 * 1024; // 5MB
```

### Integration Points

**PromptBuilder** (`thinker/prompt_builder.rs`):
- Injects skill metadata into system prompt
- Supports `skill_mode` for strict workflow execution

**BuiltinToolRegistry** (`executor/builtin_registry.rs`):
- Registers `read_skill` and `list_skills` tools
- Handles tool execution

**UniFFI Interface** (`aether.udl`):
- `list_installed_skills()` - List all skills
- `delete_skill(skill_id)` - Remove a skill
- `install_skill_from_url(url)` - Install from GitHub
- `install_skills_from_zip(path)` - Install from ZIP

---

## Configuration

### config.toml

```toml
[skills]
enabled = true              # Enable/disable Skills capability
auto_match_enabled = false  # Enable automatic skill matching (default: false)
```

### Directory Structure

```
~/.aleph/
├── config.toml
├── skills/
│   ├── refine-text/
│   │   └── SKILL.md
│   ├── translate/
│   │   └── SKILL.md
│   └── summarize/
│       └── SKILL.md
└── ...
```

---

## Built-in Skills

Aleph includes three built-in skills that are installed on first launch:

### refine-text

**Purpose**: Improve and polish writing

**Usage**: `/skill refine-text <text to refine>`

**Guidelines**:
- Improve clarity and readability
- Eliminate redundancy
- Fix grammar and punctuation
- Maintain original meaning

### translate

**Purpose**: Translate text between languages

**Usage**: `/skill translate <text to translate>`

**Guidelines**:
- Detect source language automatically
- Target language specified by user
- Preserve formatting and structure
- Handle idioms and cultural context

### summarize

**Purpose**: Summarize long content concisely

**Usage**: `/skill summarize <text to summarize>`

**Guidelines**:
- Extract key points
- Maintain logical structure
- Preserve important details
- Adapt length to content

---

## Creating Custom Skills

### Step 1: Create Directory

```bash
mkdir -p ~/.aleph/skills/my-skill
```

### Step 2: Create SKILL.md

```bash
cat > ~/.aleph/skills/my-skill/SKILL.md << 'EOF'
---
name: my-skill
description: Description of what this skill does.
---

# My Custom Skill

Instructions for the AI...

## Guidelines

1. First guideline
2. Second guideline

## Examples

Example usage...
EOF
```

### Step 3: Verify Installation

The skill appears immediately in Settings > Skills and can be used with:

```
/skill my-skill <input>
```

### Best Practices

1. **Clear Description**: Write a description that accurately triggers auto-matching
2. **Specific Instructions**: Be precise about expected behavior
3. **Examples**: Include examples of good outputs
4. **Constraints**: Define boundaries and limitations
5. **Testing**: Test with various inputs before sharing

---

## Troubleshooting

### Skill Not Found

**Symptoms**: `/skill <name>` returns "Skill not found"

**Solutions**:
1. Check skill exists in `~/.aleph/skills/<name>/SKILL.md`
2. Verify SKILL.md has valid YAML frontmatter
3. Refresh skills list in Settings > Skills
4. Check logs for parsing errors

### Installation Failed

**Symptoms**: GitHub/ZIP installation fails

**Solutions**:
1. Verify URL format is correct
2. Check network connectivity
3. Ensure ZIP contains valid skill directories
4. Check each directory has a SKILL.md file

### Skill Not Matching

**Symptoms**: Auto-match doesn't trigger expected skill

**Solutions**:
1. Ensure `auto_match_enabled = true` in config
2. Improve skill description to better match intent
3. Use explicit `/skill <name>` invocation

---

## References

- [Claude Agent Skills Overview](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview)
- [Anthropic Skills GitHub](https://github.com/anthropics/skills)
- [Simon Willison: Claude Skills](https://simonwillison.net/2025/Oct/16/claude-skills/)
- OpenSpec Proposal: `openspec/changes/add-skills-capability/`
- Design Doc: [SKILLS_ARCHITECTURE_REDESIGN.md](./SKILLS_ARCHITECTURE_REDESIGN.md)

---

**Last Updated**: 2026-01-23
**Implemented In**: Aleph v0.1.0
**OpenSpec Changes**: `add-skills-capability`, `skills-progressive-disclosure`
