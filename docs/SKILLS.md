# Claude Agent Skills Integration

Aether supports the [Claude Agent Skills](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/overview) open standard, enabling users to extend AI capabilities through structured instruction injection.

## Table of Contents

- [Overview](#overview)
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

When enabled in configuration, Aether can automatically match user intent to appropriate skills based on the skill's description.

```toml
[skills]
enabled = true
auto_match_enabled = true  # Default: false
```

---

## Installing Skills

Aether provides three methods to install Skills:

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
~/.config/aether/skills/
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

Skills are stored in `~/.config/aether/skills/`. Each skill has its own directory:

```
~/.config/aether/skills/
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
│                 CompositeCapabilityExecutor                      │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌────────┐ │
│  │ Memory   │ │ Search   │ │ MCP      │ │ Video    │ │ Skills │ │
│  │ Strategy │ │ Strategy │ │ Strategy │ │ Strategy │ │Strategy│ │
│  │ (0)      │ │ (1)      │ │ (2)      │ │ (3)      │ │ (4)    │ │
│  └──────────┘ └──────────┘ └──────────┘ └──────────┘ └────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

### Rust Core Components

| Component | Location | Description |
|-----------|----------|-------------|
| `Skill` | `skills/mod.rs` | Skill data structure with YAML/Markdown parsing |
| `SkillInfo` | `skills/mod.rs` | FFI-safe skill information for UI |
| `SkillsRegistry` | `skills/registry.rs` | Manages skill loading and lookup |
| `SkillsInstaller` | `skills/installer.rs` | GitHub/ZIP installation |
| `SkillsStrategy` | `capability/strategies/skills.rs` | Capability execution |
| `Capability::Skills` | `payload/capability.rs` | Priority 4 capability variant |

### Execution Flow

1. User invokes `/skill <name> <input>`
2. Router identifies skill command, extracts skill name
3. `SkillsStrategy` looks up skill in `SkillsRegistry`
4. Skill instructions are injected into `AgentPayload.context.skill_instructions`
5. `PromptAssembler` formats instructions into system prompt
6. AI provider receives augmented prompt

### Integration Points

**Router** (`router/mod.rs`):
- Parses `/skill <name>` command
- Sets `Intent::BuiltinSkills`
- Extracts skill name for Strategy

**PromptAssembler** (`payload/assembler.rs`):
- Formats skill instructions into system prompt
- Supports Markdown format (default)

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
~/.config/aether/
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

Aether includes three built-in skills that are installed on first launch:

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
mkdir -p ~/.config/aether/skills/my-skill
```

### Step 2: Create SKILL.md

```bash
cat > ~/.config/aether/skills/my-skill/SKILL.md << 'EOF'
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
1. Check skill exists in `~/.config/aether/skills/<name>/SKILL.md`
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

---

**Last Updated**: 2026-01-08
**Implemented In**: Aether v0.1.0
**OpenSpec Change**: `add-skills-capability`
