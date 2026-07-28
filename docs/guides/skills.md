# Skills Configuration Guide

## File Paths
- Global skills: `~/.aleph/skills/`
- Project skills: `.aleph/skills/` or `.claude/skills/` (in project root)

## Operation Rules
1. Skills are discovered on next tool registry scan (no restart needed)
2. Use the Aleph Hub store for registry installs (handles download + extraction)
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
description: One-line description for LLM matching — put the trigger words/phrases HERE
when-to-use: Optional extra hint shown in the skill list
---

<!-- Only `name` and `description` are required. There is NO `trigger` field —
     matching is driven entirely by `description`. Other optional fields:
     scope, user-invocable, disable-model-invocation, bound-tool,
     eligibility, install, primary-env, homepage, emoji, when-to-use. -->

# Skill Title

Instructions for the LLM when this skill is activated.
Include step-by-step guidance, rules, and examples.
```

## Common Operations

### Install from the Aleph Hub store
`hub_catalog_sync` to refresh the local catalog, then `hub_install_run(id="aleph-hub:...")`
to install a catalog entry (trust-gated). The Panel Extensions store drives the same path.

### Manual install
1. Create directory: `mkdir -p ~/.aleph/skills/my-skill`
2. Create SKILL.md with frontmatter (`name`, `description` — trigger words go in `description`)
3. Add instruction content

### Remove a skill
Delete the skill directory: `rm -rf ~/.aleph/skills/my-skill`

### List installed skills
Use the `skill_status` tool or `ls ~/.aleph/skills/`

## Caveats
- SKILL.md is required — directory without it is ignored
- Project-level skills (`.aleph/skills/`) take precedence over global
- Skill names must be valid directory names (no spaces, special chars)
