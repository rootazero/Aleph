# Skill Trigger Enhancement Design

> Date: 2026-04-06
> Status: Approved
> Scope: SkillManifest when_to_use + XML output + proactive trigger guidance

## Problem

SkillInstructionsLayer injects skill name + description into the system prompt, but the LLM lacks trigger context — it doesn't know *when* to proactively invoke a skill. Users must explicitly request skills instead of the model recognizing matching scenarios.

Claude Code solves this with detailed "when to use" annotations and keyword triggers in its skill/agent lists, making the model proactively match user requests to available skills.

## Design Approach

Extend SkillManifest with `when_to_use` field, enhance XML output with `<when>` tag, and update deferred loading guidance to instruct proactive invocation. Follows R8 (LLM Sovereignty) — no keyword matching logic, let the model do semantic matching.

## Part 1: SkillManifest Extension

### Changes to `src/domain/skill.rs`

Add field to `SkillManifest` struct (after `emoji` at line 406):

```rust
    /// Trigger hint — describes when this skill should be proactively invoked
    when_to_use: Option<String>,
```

Update `SkillManifest::new()` to initialize: `when_to_use: None`.

Add getter:

```rust
    /// When this skill should be proactively invoked.
    pub fn when_to_use(&self) -> Option<&str> {
        self.when_to_use.as_deref()
    }
```

Add setter:

```rust
    /// Set the when_to_use trigger hint.
    pub fn set_when_to_use(&mut self, hint: String) {
        self.when_to_use = Some(hint);
    }
```

### Changes to `src/skill/manifest.rs` (YAML frontmatter parsing)

In `parse_skill_content()`, after existing frontmatter field handling, add:

```rust
    if let Some(when) = raw.when_to_use {
        manifest.set_when_to_use(when);
    }
```

In `RawFrontmatter` struct, add:

```rust
    #[serde(rename = "when-to-use")]
    when_to_use: Option<String>,
```

This enables SKILL.md files to declare:

```yaml
---
name: Code Review
description: Reviews code for quality issues
when-to-use: When code has been written or modified and needs quality review
---
```

## Part 2: XML Output Enhancement

### Changes to `src/skill/prompt.rs`

Update `build_skills_prompt_xml()` to include `<when>` tag when present:

```rust
pub fn build_skills_prompt_xml(skills: &[&SkillManifest]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut buf = String::from("<available_skills>\n");

    for skill in skills {
        buf.push_str("  <skill>\n");
        buf.push_str("    <name>");
        buf.push_str(&escape_xml(skill.name()));
        buf.push_str("</name>\n");
        buf.push_str("    <description>");
        buf.push_str(&escape_xml(skill.description()));
        buf.push_str("</description>\n");
        if let Some(when) = skill.when_to_use() {
            buf.push_str("    <when>");
            buf.push_str(&escape_xml(when));
            buf.push_str("</when>\n");
        }
        buf.push_str("  </skill>\n");
    }

    buf.push_str("</available_skills>");
    buf
}
```

Output example:

```xml
<available_skills>
  <skill>
    <name>Code Review</name>
    <description>Reviews code for quality issues</description>
    <when>When code has been written or modified and needs quality review</when>
  </skill>
  <skill>
    <name>Docker Build</name>
    <description>Builds Docker images</description>
  </skill>
</available_skills>
```

Skills without `when_to_use` omit the `<when>` tag (backward compatible).

## Part 3: Proactive Trigger Guidance

### Changes to `src/skill/prompt.rs`

Update `DEFERRED_LOADING_GUIDANCE` constant:

```rust
pub const DEFERRED_LOADING_GUIDANCE: &str =
    "To use a skill, first call the `skill_read` tool with the skill name \
     to load its full instructions, then follow those instructions. \
     Use `skill_list` to discover available skills if needed.\n\n\
     When a user's request matches a skill's <when> trigger, proactively \
     invoke that skill without waiting for an explicit request.";
```

This single addition tells the LLM to:
1. Check skill triggers against user requests
2. Proactively load and follow matching skills
3. Not wait for explicit "use skill X" commands

## File Change Summary

### Modified Files (3)

| File | Change |
|------|--------|
| `src/domain/skill.rs` | Add `when_to_use` field, getter, setter |
| `src/skill/manifest.rs` | Parse `when-to-use` from YAML frontmatter |
| `src/skill/prompt.rs` | XML `<when>` tag + updated DEFERRED_LOADING_GUIDANCE |

### Zero Breaking Changes

- `when_to_use` defaults to `None` in constructor
- XML output only adds `<when>` when field is present
- Existing SKILL.md files without `when-to-use` frontmatter work unchanged
- DEFERRED_LOADING_GUIDANCE is additive

## Testing Strategy

- **SkillManifest**: test field default, getter, setter
- **YAML parsing**: test frontmatter with and without `when-to-use`
- **XML output**: test skills with/without `when_to_use` produce correct XML
- **Guidance**: test DEFERRED_LOADING_GUIDANCE contains proactive trigger text
