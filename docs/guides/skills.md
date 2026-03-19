# Skills Configuration Guide

## File Paths
- Global skills: `~/.aleph/skills/`
- Project skills: `.aleph/skills/` or `.claude/skills/` (in project root)
- ClawHub metadata: `~/.aleph/skills/<skill>/clawhub.json`

## Operation Rules
1. Skills are discovered on next tool registry scan (no restart needed)
2. Use `clawhub` tool for registry installs (handles download + extraction)
3. Manual install: create directory + SKILL.md file

## Directory Layout

```
~/.aleph/skills/
├── my-skill/
│   ├── SKILL.md           # Required — main instructions
│   ├── ADVANCED.md        # Optional — deep-dive content
│   ├── REFERENCE.md       # Optional — reference material
│   └── CHECKLIST.md       # Optional — step-by-step checklist
└── another-skill/
    └── SKILL.md
```

## SKILL.md Format

```markdown
---
name: my-skill
description: One-line description for LLM matching
trigger: keyword or phrase that activates this skill
---

# Skill Title

Instructions for the LLM when this skill is activated.
Include step-by-step guidance, rules, and examples.
```

## Common Operations

### Install from ClawHub
Use the `clawhub` tool: `clawhub(action="install", skill_id="skill-name")`

### Manual install
1. Create directory: `mkdir -p ~/.aleph/skills/my-skill`
2. Create SKILL.md with frontmatter (name, description, trigger)
3. Add instruction content

### Remove a skill
Delete the skill directory: `rm -rf ~/.aleph/skills/my-skill`

### List installed skills
Use `list_skills` tool or `ls ~/.aleph/skills/`

## Caveats
- SKILL.md is required — directory without it is ignored
- Project-level skills (`.aleph/skills/`) take precedence over global
- ClawHub installs write `.clawhub.json` for version tracking
- Skill names must be valid directory names (no spaces, special chars)
