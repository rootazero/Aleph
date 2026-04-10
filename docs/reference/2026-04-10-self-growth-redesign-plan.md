# Self-Growth Module Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Aleph's algorithm-heavy memory processing with LLM-driven skill extraction via reflection, storing skills as `FactType::Skill` facts in MemoryStore.

**Architecture:** Skills are extracted by the LLM during conversation reflection, stored as facts in MemoryStore with full RAG/graph/decay support, and recalled via hybrid_retrieval with a fact_type filter. Algorithmic dreaming stages (DBSCAN, 8D scoring) are deleted; remaining stages are wired to LLM.

**Tech Stack:** Rust, SQLite (existing MemoryStore), serde, schemars, async-trait, tokio

**Spec:** `docs/reference/2026-04-10-self-growth-redesign.md`

---

## File Structure

### New Files

| File | Responsibility |
|------|---------------|
| `src/skill/mod.rs` | Module exports, `SkillExtraction` struct |
| `src/skill/recaller.rs` | Skill recall via hybrid_retrieval wrapper |
| `src/skill/tools/mod.rs` | Tool sub-module exports |
| `src/skill/tools/manage.rs` | `SkillManageTool` — create/patch/delete/list skills |
| `src/skill/tools/search.rs` | `SkillSearchTool` — semantic skill search |

### Modified Files

| File | Change |
|------|--------|
| `src/memory/context/enums.rs` | Add `FactType::Skill` variant |
| `src/memory/reflection/prompt.rs` | Extend Skills section prompt |
| `src/memory/reflection/parser.rs` | Add `SkillExtraction` YAML parsing |
| `src/memory/reflection/mapper.rs` | Map skills to `FactType::Skill` facts |
| `src/memory/dreaming/mod.rs` | Remove Collect/Cluster from pipeline |
| `src/memory/dreaming/stages/mod.rs` | Remove collect/cluster exports |
| `src/memory/dreaming/stages/consolidate.rs` | Remove promotion_scorer dependency, add LLM |
| `src/lib.rs` or `src/main.rs` | Register `pub mod skill` |

### Deleted Files

| File | Reason |
|------|--------|
| `src/memory/dreaming/stages/collect.rs` | No-op since SessionStore removal |
| `src/memory/dreaming/stages/cluster.rs` | DBSCAN — algorithmic semantic judgment |
| `src/memory/consolidation/promotion_scorer.rs` | 8D scoring — algorithmic semantic judgment |
| `src/memory/value_estimator/signals.rs` | Hardcoded signal detection |

---

## Track A: Skill System

### Task 1: Add `FactType::Skill` Variant

**Files:**
- Modify: `src/memory/context/enums.rs:14-41` (FactType enum)

- [ ] **Step 1: Add Skill variant to FactType enum**

In `src/memory/context/enums.rs`, add the `Skill` variant after `Lesson`:

```rust
    /// Lesson learned from experience (symptom → cause → fix).
    Lesson,
    /// Reusable procedural knowledge extracted by LLM self-growth.
    Skill,
    /// Other facts that don't fit above categories
    #[default]
    Other,
```

- [ ] **Step 2: Update `as_str()` match arm**

Add in the `as_str()` method:

```rust
            FactType::Skill => "skill",
```

- [ ] **Step 3: Update `default_path()` match arm**

```rust
            FactType::Skill => "aleph://skills/",
```

- [ ] **Step 4: Update `default_category()` match arm**

```rust
            FactType::Skill => MemoryCategory::Patterns,
```

- [ ] **Step 5: Update `FromStr` implementation**

Add in the `from_str` match:

```rust
            "skill" => Ok(FactType::Skill),
```

- [ ] **Step 6: Run tests to verify compilation**

Run: `cargo test -p alephcore --lib -- memory::context`
Expected: All existing tests pass, no compilation errors.

- [ ] **Step 7: Commit**

```bash
git add src/memory/context/enums.rs
git commit -m "memory: add FactType::Skill variant for self-growth"
```

---

### Task 2: Create Skill Module Foundation

**Files:**
- Create: `src/skill/mod.rs`

- [ ] **Step 1: Create skill module directory and mod.rs**

Create `src/skill/mod.rs`:

```rust
//! Self-growth skill system.
//!
//! Skills are reusable procedural knowledge extracted by the LLM during
//! conversation reflection. They are stored as `FactType::Skill` facts
//! in MemoryStore and recalled via hybrid retrieval.

pub mod recaller;
pub mod tools;

/// Structured skill extraction from reflection output.
#[derive(Debug, Clone)]
pub struct SkillExtraction {
    /// Kebab-case identifier (e.g., "rust-lifetime-debugging")
    pub name: String,
    /// Category: coding, debugging, workflow, knowledge, communication
    pub category: String,
    /// One-line description (max 100 chars)
    pub description: String,
    /// Full markdown body (When to Use, Steps, Pitfalls)
    pub content: String,
    /// True if this is an update to an existing skill
    pub is_update: bool,
}

impl SkillExtraction {
    /// Build the VFS path for this skill.
    pub fn vfs_path(&self) -> String {
        format!("aleph://skills/{}/{}/", self.category, self.name)
    }

    /// Build the full fact content with description header.
    pub fn fact_content(&self) -> String {
        format!("{}\n\n{}", self.description, self.content)
    }
}

/// Valid skill categories.
pub const SKILL_CATEGORIES: &[&str] = &[
    "coding",
    "debugging",
    "workflow",
    "knowledge",
    "communication",
];

/// Validate a skill category.
pub fn is_valid_category(category: &str) -> bool {
    SKILL_CATEGORIES.contains(&category)
}

/// Validate a skill name (kebab-case, non-empty, ASCII alphanumeric + hyphens).
pub fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_skill_names() {
        assert!(is_valid_skill_name("rust-lifetime-debugging"));
        assert!(is_valid_skill_name("git-rebase"));
        assert!(is_valid_skill_name("a"));
    }

    #[test]
    fn invalid_skill_names() {
        assert!(!is_valid_skill_name(""));
        assert!(!is_valid_skill_name("-leading"));
        assert!(!is_valid_skill_name("trailing-"));
        assert!(!is_valid_skill_name("has spaces"));
        assert!(!is_valid_skill_name("UpperCase"));
    }

    #[test]
    fn valid_categories() {
        assert!(is_valid_category("coding"));
        assert!(is_valid_category("debugging"));
        assert!(!is_valid_category("invalid"));
    }

    #[test]
    fn vfs_path_format() {
        let skill = SkillExtraction {
            name: "rust-lifetime-debugging".to_string(),
            category: "coding".to_string(),
            description: "Debug lifetime errors".to_string(),
            content: "# Steps\n1. Check borrow scope".to_string(),
            is_update: false,
        };
        assert_eq!(skill.vfs_path(), "aleph://skills/coding/rust-lifetime-debugging/");
    }
}
```

- [ ] **Step 2: Create stub files for sub-modules**

Create `src/skill/tools/mod.rs`:

```rust
//! Skill management tools exposed to the LLM.

pub mod manage;
pub mod search;
```

Create `src/skill/tools/manage.rs`:

```rust
//! SkillManageTool — create, patch, delete, list learned skills.
// Implementation in Task 4.
```

Create `src/skill/tools/search.rs`:

```rust
//! SkillSearchTool — semantic search over learned skills.
// Implementation in Task 5.
```

Create `src/skill/recaller.rs`:

```rust
//! Skill recall via hybrid_retrieval for prompt assembly.
// Implementation in Task 6.
```

- [ ] **Step 3: Register skill module in crate root**

Find the crate root (`src/lib.rs` or wherever modules are declared) and add:

```rust
pub mod skill;
```

- [ ] **Step 4: Run compilation check**

Run: `cargo check -p alephcore`
Expected: Compiles successfully.

- [ ] **Step 5: Commit**

```bash
git add src/skill/
git commit -m "skill: add module foundation with SkillExtraction struct"
```

---

### Task 3: Extend Reflection for Skill Extraction

**Files:**
- Modify: `src/memory/reflection/prompt.rs:4-33`
- Modify: `src/memory/reflection/parser.rs:1-86`
- Modify: `src/memory/reflection/mapper.rs:38-108`

- [ ] **Step 1: Write test for new skill extraction parsing**

Add to `src/memory/reflection/parser.rs` tests module:

```rust
    #[test]
    fn parse_structured_skill_yaml() {
        let md = r#"## Invariants
- User prefers Rust

## Skills
```yaml
- name: rust-lifetime-debugging
  category: coding
  description: Debug Rust lifetime errors systematically
  content: |
    # Rust Lifetime Debugging

    ## When to Use
    When encountering lifetime errors.

    ## Steps
    1. Check borrow scope
    2. Use explicit lifetime annotations

    ## Pitfalls
    - Avoid cloning to fix lifetimes
```

## Open Loops
- (none)
"#;
        let out = parse_reflection(md);
        // Structured skills should be parsed into skills_structured
        assert!(!out.skills_structured.is_empty());
        assert_eq!(out.skills_structured[0].name, "rust-lifetime-debugging");
        assert_eq!(out.skills_structured[0].category, "coding");
        assert!(out.skills_structured[0].content.contains("Check borrow scope"));
    }

    #[test]
    fn parse_old_format_skill_still_works() {
        let md = "\
## Skills
- FTS5 search: build index, group by session, return context
";
        let out = parse_reflection(md);
        assert_eq!(out.skills.len(), 1);
        assert!(out.skills_structured.is_empty());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib -- memory::reflection::parser::tests::parse_structured_skill_yaml`
Expected: FAIL — `skills_structured` field does not exist.

- [ ] **Step 3: Add `skills_structured` field to `ReflectionOutput`**

In `src/memory/reflection/parser.rs`, add the import and field:

```rust
use crate::skill::SkillExtraction;

#[derive(Debug, Clone, Default)]
pub struct ReflectionOutput {
    pub invariants: Vec<String>,
    pub derived: Vec<String>,
    pub lessons: Vec<LessonItem>,
    pub skills: Vec<String>,
    pub skills_structured: Vec<SkillExtraction>,
    pub open_loops: Vec<String>,
}
```

- [ ] **Step 4: Add YAML skill block parsing to `parse_reflection()`**

Update the `parse_reflection` function to detect YAML code blocks within the Skills section. Replace the existing parser with:

```rust
pub fn parse_reflection(text: &str) -> ReflectionOutput {
    let mut out = ReflectionOutput::default();
    let mut section = Section::Unknown;
    let mut in_yaml_block = false;
    let mut yaml_lines: Vec<String> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();

        // Detect YAML code block boundaries
        if trimmed.starts_with("```yaml") && section == Section::Skills {
            in_yaml_block = true;
            yaml_lines.clear();
            continue;
        }
        if trimmed.starts_with("```") && in_yaml_block {
            in_yaml_block = false;
            // Parse accumulated YAML
            let yaml_text = yaml_lines.join("\n");
            if let Some(extractions) = parse_skill_yaml(&yaml_text) {
                out.skills_structured.extend(extractions);
            }
            yaml_lines.clear();
            continue;
        }
        if in_yaml_block {
            yaml_lines.push(line.to_string());
            continue;
        }

        // Detect section headers
        if let Some(header) = trimmed.strip_prefix("## ") {
            let lower = header.to_lowercase();
            section = if lower == "invariants" {
                Section::Invariants
            } else if lower == "derived" {
                Section::Derived
            } else if lower.starts_with("lessons") {
                Section::Lessons
            } else if lower.starts_with("skills") {
                Section::Skills
            } else if lower.starts_with("open loops") {
                Section::OpenLoops
            } else {
                Section::Unknown
            };
            continue;
        }

        // Collect bullet items
        if let Some(item) = trimmed.strip_prefix("- ") {
            let item = item.trim();
            if is_placeholder(item) {
                continue;
            }
            match section {
                Section::Invariants => out.invariants.push(item.to_string()),
                Section::Derived => out.derived.push(item.to_string()),
                Section::Lessons => out.lessons.push(parse_lesson(item)),
                Section::Skills => out.skills.push(item.to_string()),
                Section::OpenLoops => out.open_loops.push(item.to_string()),
                Section::Unknown => {}
            }
        }
    }

    out
}

/// Parse a YAML block containing skill extraction entries.
fn parse_skill_yaml(yaml_text: &str) -> Option<Vec<SkillExtraction>> {
    // Parse as a YAML sequence of skill entries
    #[derive(serde::Deserialize)]
    struct RawSkill {
        name: String,
        category: String,
        description: String,
        content: String,
    }

    let skills: Vec<RawSkill> = serde_yaml::from_str(yaml_text).ok()?;
    Some(
        skills
            .into_iter()
            .map(|s| SkillExtraction {
                name: s.name,
                category: s.category,
                description: s.description,
                content: s.content.trim().to_string(),
                is_update: false,
            })
            .collect(),
    )
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib -- memory::reflection::parser::tests`
Expected: All tests pass including new ones.

- [ ] **Step 6: Update reflection prompt**

In `src/memory/reflection/prompt.rs`, replace the Skills section in `reflection_system_prompt()`:

```rust
## Skills
For any non-trivial, reusable knowledge discovered this session (5+ steps,
likely to recur, or hard-won insight), output a complete skill definition
inside a ```yaml code block:

```yaml
- name: kebab-case-name
  category: coding | debugging | workflow | knowledge | communication
  description: One-line description (max 100 chars)
  content: |
    # Skill Title

    ## When to Use
    Trigger conditions...

    ## Steps
    1. ...

    ## Pitfalls
    - ...
```

Rules for skills:
- Only extract if the knowledge is REUSABLE across sessions
- If an existing skill was used and found outdated, output it with updated content
- If a skill was used and confirmed correct, do NOT re-output it
- Maximum 3 skills per reflection
- You may also use the old bullet format (- skill name: description) for simple notes
```

- [ ] **Step 7: Update prompt test**

Update the `system_prompt_contains_all_sections` test to check for the new format:

```rust
    #[test]
    fn system_prompt_contains_all_sections() {
        let prompt = reflection_system_prompt();
        assert!(prompt.contains("## Invariants"));
        assert!(prompt.contains("## Derived"));
        assert!(prompt.contains("## Lessons"));
        assert!(prompt.contains("## Skills"));
        assert!(prompt.contains("## Open Loops"));
        assert!(prompt.contains("```yaml"));
        assert!(prompt.contains("category:"));
    }
```

- [ ] **Step 8: Write test for mapper skill→fact conversion**

Add to `src/memory/reflection/mapper.rs` tests:

```rust
    #[test]
    fn maps_structured_skills_to_skill_facts() {
        use crate::skill::SkillExtraction;

        let output = ReflectionOutput {
            skills_structured: vec![SkillExtraction {
                name: "rust-lifetime-debugging".to_string(),
                category: "coding".to_string(),
                description: "Debug lifetime errors".to_string(),
                content: "# Steps\n1. Check scope".to_string(),
                is_update: false,
            }],
            ..Default::default()
        };
        let facts = map_to_facts(&output);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].fact_type, FactType::Skill);
        assert_eq!(facts[0].tier, MemoryTier::ShortTerm);
        assert_eq!(facts[0].path, "aleph://skills/coding/rust-lifetime-debugging/");
        assert!(facts[0].content.contains("Debug lifetime errors"));
        assert!(facts[0].content.contains("Check scope"));
    }
```

- [ ] **Step 9: Update mapper to handle structured skills**

In `src/memory/reflection/mapper.rs`, add handling for `skills_structured` after the existing skills block:

```rust
    // Structured Skills → ShortTerm tier, Skill type, skills/ path prefix
    for skill in &output.skills_structured {
        let path = skill.vfs_path();
        let content = skill.fact_content();
        let fact = MemoryFact::new(content, FactType::Skill, Vec::new())
            .with_confidence(0.80)
            .with_tier(MemoryTier::ShortTerm)
            .with_scope(MemoryScope::Persona)
            .with_layer(MemoryLayer::L1Overview)
            .with_category(MemoryCategory::Patterns)
            .with_path(path)
            .with_fact_source(FactSource::Extracted);
        facts.push(fact);
    }
```

Add the import at the top of mapper.rs:

```rust
use crate::skill::SkillExtraction;
```

Note: `ReflectionOutput` must be updated to include `skills_structured` — this was done in Step 3.

- [ ] **Step 10: Run all reflection tests**

Run: `cargo test -p alephcore --lib -- memory::reflection`
Expected: All tests pass.

- [ ] **Step 11: Commit**

```bash
git add src/memory/reflection/
git commit -m "reflection: extend skill extraction with structured YAML format"
```

---

### Task 4: Implement SkillManageTool

**Files:**
- Modify: `src/skill/tools/manage.rs`

- [ ] **Step 1: Write tests for skill manage operations**

Add to `src/skill/tools/manage.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_skill_name() {
        assert!(validate_args_for_create("rust-debug", "coding", "desc", "content").is_ok());
        assert!(validate_args_for_create("", "coding", "desc", "content").is_err());
        assert!(validate_args_for_create("Has Spaces", "coding", "desc", "content").is_err());
    }

    #[test]
    fn validates_category() {
        assert!(validate_args_for_create("name", "coding", "desc", "content").is_ok());
        assert!(validate_args_for_create("name", "invalid", "desc", "content").is_err());
    }

    #[test]
    fn builds_fact_from_args() {
        let fact = build_skill_fact("rust-debug", "coding", "Debug Rust errors", "# Steps\n1. Read error");
        assert_eq!(fact.fact_type, FactType::Skill);
        assert_eq!(fact.path, "aleph://skills/coding/rust-debug/");
        assert_eq!(fact.tier, MemoryTier::ShortTerm);
        assert!(fact.content.contains("Debug Rust errors"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib -- skill::tools::manage`
Expected: FAIL — functions not defined.

- [ ] **Step 3: Implement SkillManageTool**

Write `src/skill/tools/manage.rs`:

```rust
//! SkillManageTool — create, patch, delete, list learned skills.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::memory::context::{
    FactSource, FactType, MemoryCategory, MemoryFact, MemoryLayer, MemoryScope, MemoryTier,
};
use crate::skill::{is_valid_category, is_valid_skill_name};

/// Actions supported by the skill_manage tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SkillAction {
    Create,
    Patch,
    Delete,
    List,
}

/// Arguments for the skill_manage tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SkillManageArgs {
    /// Action to perform.
    pub action: SkillAction,
    /// Skill name (kebab-case). Required for create, patch, delete.
    pub name: Option<String>,
    /// Skill category. Required for create.
    pub category: Option<String>,
    /// Skill scope: "global" or "persona" (default: persona).
    pub scope: Option<String>,
    /// One-line description. Required for create.
    pub description: Option<String>,
    /// Full skill markdown content. Required for create.
    pub content: Option<String>,
    /// Old text to find (for patch action).
    pub old_text: Option<String>,
    /// New text to replace with (for patch action).
    pub new_text: Option<String>,
}

/// Result of a skill_manage operation.
#[derive(Debug, Clone, Serialize)]
pub struct SkillManageResult {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<SkillListEntry>>,
}

/// Entry in skill list output.
#[derive(Debug, Clone, Serialize)]
pub struct SkillListEntry {
    pub name: String,
    pub category: String,
    pub description: String,
    pub path: String,
}

/// Validate arguments for create action.
pub fn validate_args_for_create(
    name: &str,
    category: &str,
    description: &str,
    content: &str,
) -> Result<(), String> {
    if !is_valid_skill_name(name) {
        return Err(format!(
            "Invalid skill name '{}': must be non-empty kebab-case (lowercase ASCII + hyphens)",
            name
        ));
    }
    if !is_valid_category(category) {
        return Err(format!(
            "Invalid category '{}': must be one of: coding, debugging, workflow, knowledge, communication",
            category
        ));
    }
    if description.is_empty() {
        return Err("Description cannot be empty".to_string());
    }
    if content.is_empty() {
        return Err("Content cannot be empty".to_string());
    }
    Ok(())
}

/// Build a MemoryFact from skill creation arguments.
pub fn build_skill_fact(
    name: &str,
    category: &str,
    description: &str,
    content: &str,
) -> MemoryFact {
    let path = format!("aleph://skills/{}/{}/", category, name);
    let full_content = format!("{}\n\n{}", description, content);

    MemoryFact::new(full_content, FactType::Skill, Vec::new())
        .with_confidence(0.80)
        .with_tier(MemoryTier::ShortTerm)
        .with_scope(MemoryScope::Persona)
        .with_layer(MemoryLayer::L1Overview)
        .with_category(MemoryCategory::Patterns)
        .with_path(path)
        .with_fact_source(FactSource::Extracted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_skill_name() {
        assert!(validate_args_for_create("rust-debug", "coding", "desc", "content").is_ok());
        assert!(validate_args_for_create("", "coding", "desc", "content").is_err());
        assert!(validate_args_for_create("Has Spaces", "coding", "desc", "content").is_err());
    }

    #[test]
    fn validates_category() {
        assert!(validate_args_for_create("name", "coding", "desc", "content").is_ok());
        assert!(validate_args_for_create("name", "invalid", "desc", "content").is_err());
    }

    #[test]
    fn builds_fact_from_args() {
        let fact = build_skill_fact(
            "rust-debug",
            "coding",
            "Debug Rust errors",
            "# Steps\n1. Read error",
        );
        assert_eq!(fact.fact_type, FactType::Skill);
        assert_eq!(fact.path, "aleph://skills/coding/rust-debug/");
        assert_eq!(fact.tier, MemoryTier::ShortTerm);
        assert!(fact.content.contains("Debug Rust errors"));
        assert!(fact.content.contains("Read error"));
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib -- skill::tools::manage`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/skill/tools/manage.rs
git commit -m "skill: implement SkillManageTool with validation and fact builder"
```

---

### Task 5: Implement SkillSearchTool

**Files:**
- Modify: `src/skill/tools/search.rs`

- [ ] **Step 1: Implement SkillSearchTool**

Write `src/skill/tools/search.rs`:

```rust
//! SkillSearchTool — semantic search over learned skills.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::memory::context::FactType;
use crate::memory::store::types::SearchFilter;

/// Arguments for the skill_search tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SkillSearchArgs {
    /// Natural language query to search skills.
    pub query: String,
    /// Optional scope filter: "global" or "persona".
    pub scope: Option<String>,
    /// Maximum results (default: 5).
    pub limit: Option<usize>,
}

/// Build a SearchFilter for skill-only queries.
pub fn build_skill_filter() -> SearchFilter {
    SearchFilter::default()
        .with_fact_type(FactType::Skill)
        .with_valid_only()
}

/// Result entry for skill search.
#[derive(Debug, Clone, Serialize)]
pub struct SkillSearchResult {
    pub name: String,
    pub description: String,
    pub path: String,
    pub relevance: f32,
}

/// Extract skill name from VFS path (e.g., "aleph://skills/coding/rust-debug/" → "rust-debug").
pub fn skill_name_from_path(path: &str) -> Option<&str> {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_skill_name_from_path() {
        assert_eq!(
            skill_name_from_path("aleph://skills/coding/rust-debug/"),
            Some("rust-debug")
        );
        assert_eq!(
            skill_name_from_path("aleph://skills/workflow/git-rebase/"),
            Some("git-rebase")
        );
    }

    #[test]
    fn builds_filter_with_skill_type() {
        let filter = build_skill_filter();
        assert_eq!(filter.fact_type, Some(FactType::Skill));
        assert_eq!(filter.is_valid, Some(true));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib -- skill::tools::search`
Expected: All tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/skill/tools/search.rs
git commit -m "skill: implement SkillSearchTool with filter builder"
```

---

### Task 6: Implement Skill Recaller

**Files:**
- Modify: `src/skill/recaller.rs`

- [ ] **Step 1: Implement SkillRecaller**

Write `src/skill/recaller.rs`:

```rust
//! Skill recall via hybrid_retrieval for prompt assembly.
//!
//! Retrieves relevant skills from MemoryStore using semantic search
//! and formats them for injection into the system prompt.

use crate::memory::context::MemoryFact;
use crate::skill::tools::search::skill_name_from_path;

/// Default maximum skills to inject per conversation.
pub const DEFAULT_SKILL_RECALL_LIMIT: usize = 5;

/// Format retrieved skill facts as a system prompt fragment.
///
/// Returns `None` if no skills are provided.
pub fn format_skills_prompt(skills: &[MemoryFact]) -> Option<String> {
    if skills.is_empty() {
        return None;
    }

    let mut parts = vec![
        "## Learned Skills (auto-retrieved)".to_string(),
        "The following skills were learned from past sessions and may be relevant.".to_string(),
        "Follow them if applicable. If a skill is outdated, update it via skill_manage.\n".to_string(),
    ];

    for fact in skills {
        let name = skill_name_from_path(&fact.path).unwrap_or("unknown");
        parts.push(format!("### {}", name));
        parts.push(fact.content.clone());
        parts.push(String::new()); // blank line between skills
    }

    Some(parts.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::context::{FactType, MemoryFact};

    fn make_skill_fact(name: &str, content: &str) -> MemoryFact {
        let path = format!("aleph://skills/coding/{}/", name);
        let mut fact = MemoryFact::new(content.to_string(), FactType::Skill, Vec::new());
        fact.path = path;
        fact
    }

    #[test]
    fn formats_empty_skills_as_none() {
        assert!(format_skills_prompt(&[]).is_none());
    }

    #[test]
    fn formats_single_skill() {
        let facts = vec![make_skill_fact("rust-debug", "# Steps\n1. Read error message")];
        let result = format_skills_prompt(&facts).unwrap();
        assert!(result.contains("### rust-debug"));
        assert!(result.contains("Read error message"));
        assert!(result.contains("Learned Skills"));
    }

    #[test]
    fn formats_multiple_skills() {
        let facts = vec![
            make_skill_fact("rust-debug", "Debug content"),
            make_skill_fact("git-rebase", "Rebase content"),
        ];
        let result = format_skills_prompt(&facts).unwrap();
        assert!(result.contains("### rust-debug"));
        assert!(result.contains("### git-rebase"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib -- skill::recaller`
Expected: All tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/skill/recaller.rs
git commit -m "skill: implement SkillRecaller for prompt assembly"
```

---

## Track B: Dreaming Pipeline Cleanup

### Task 7: Delete Deprecated Stages and Algorithmic Code

**Files:**
- Delete: `src/memory/dreaming/stages/collect.rs`
- Delete: `src/memory/dreaming/stages/cluster.rs`
- Delete: `src/memory/consolidation/promotion_scorer.rs`
- Delete: `src/memory/value_estimator/signals.rs`
- Modify: `src/memory/dreaming/stages/mod.rs`
- Modify: `src/memory/dreaming/mod.rs:93-107`
- Modify: `src/memory/consolidation/mod.rs`
- Modify: `src/memory/value_estimator/mod.rs`

- [ ] **Step 1: Delete collect.rs and cluster.rs**

```bash
rm src/memory/dreaming/stages/collect.rs
rm src/memory/dreaming/stages/cluster.rs
```

- [ ] **Step 2: Update stages/mod.rs — remove collect and cluster exports**

Remove `mod collect;` and `mod cluster;` declarations, and all `pub use` lines referencing `CollectStage`, `ClusterStage`, `MemoryCluster`, `MetadataGroupKey`.

Keep exports for: `ConsolidateStage`, `DecayStage`, `DriftDetectStage`, `SummarizeStage`, `DeepSynthesisStage`, `TunnelDiscoveryStage`, and their supporting types (`DriftAction`, `MemoryDecayReport`).

- [ ] **Step 3: Update DreamPipeline::daily() — remove Collect and Cluster stages**

In `src/memory/dreaming/mod.rs`, update the `daily()` method:

```rust
    /// Build the standard daily pipeline (5 stages).
    pub fn daily() -> Self {
        Self::new()
            .stage(SummarizeStage)
            .stage(DriftDetectStage)
            .stage(ConsolidateStage)
            .stage(TunnelDiscoveryStage)
            .stage(DecayStage)
    }
```

Also remove any `use` imports for `CollectStage` and `ClusterStage` at the top of the file.

- [ ] **Step 4: Delete promotion_scorer.rs**

```bash
rm src/memory/consolidation/promotion_scorer.rs
```

Update `src/memory/consolidation/mod.rs` — remove `pub mod promotion_scorer;` and its `pub use` line.

- [ ] **Step 5: Delete signals.rs**

```bash
rm src/memory/value_estimator/signals.rs
```

Update `src/memory/value_estimator/mod.rs` — remove `pub mod signals;` and its `pub use` line (`Signal`, `SignalDetector`).

- [ ] **Step 6: Fix any compilation errors from removed dependencies**

Run: `cargo check -p alephcore`

Fix any remaining references to deleted types. Common places to check:
- `consolidate.rs` may reference `PromotionScorer` — remove those references (will be replaced in Task 8)
- Other files may import `CollectStage`, `ClusterStage`, `MemoryCluster` — remove those imports

- [ ] **Step 7: Run all tests**

Run: `cargo test -p alephcore --lib`
Expected: All tests pass. Some tests that tested deleted code will have been removed with their files.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "dreaming: delete deprecated stages (collect, cluster) and algorithmic scorers"
```

---

### Task 8: Simplify Consolidate Stage

**Files:**
- Modify: `src/memory/dreaming/stages/consolidate.rs`

- [ ] **Step 1: Refactor consolidate to remove promotion_scorer dependency**

The consolidate stage should now use simple rule-based candidate filtering without the 8-dimensional scorer. Replace the scoring logic with:

1. Query ShortTerm facts older than 24 hours
2. Filter to those with `signal_count >= 3` and at least 2 unique query contexts
3. These become promotion candidates (actual promotion will be LLM-driven in a future task)
4. Keep the existing pruning logic: invalidate non-Core facts with `strength < 0.1`

The LLM integration for promotion decisions is a separate concern that will be wired in when the LLM calling infrastructure is connected to dreaming stages. For now, the simplified rule filter replaces the 8D scorer.

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib -- memory::dreaming::stages::consolidate`
Expected: Pass (tests may need updating to match simplified logic).

- [ ] **Step 3: Commit**

```bash
git add src/memory/dreaming/stages/consolidate.rs
git commit -m "consolidate: replace 8D promotion scorer with simple rule filter"
```

---

### Task 9: Final Integration Verification

**Files:** None (verification only)

- [ ] **Step 1: Full compilation check**

Run: `cargo check -p alephcore`
Expected: Clean compilation with no errors.

- [ ] **Step 2: Run all tests**

Run: `cargo test -p alephcore`
Expected: All tests pass.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: No warnings.

- [ ] **Step 4: Verify deleted files are gone**

```bash
ls src/memory/dreaming/stages/collect.rs 2>&1  # Should: No such file
ls src/memory/dreaming/stages/cluster.rs 2>&1   # Should: No such file
ls src/memory/consolidation/promotion_scorer.rs 2>&1  # Should: No such file
ls src/memory/value_estimator/signals.rs 2>&1    # Should: No such file
```

- [ ] **Step 5: Verify new module structure**

```bash
ls src/skill/
# Expected: mod.rs  recaller.rs  tools/
ls src/skill/tools/
# Expected: mod.rs  manage.rs  search.rs
```

- [ ] **Step 6: Commit (if any fixups needed)**

```bash
git add -A
git commit -m "chore: integration verification fixups"
```

---

## Task Dependency Graph

```
Task 1 (FactType::Skill)
    ↓
Task 2 (Skill Module Foundation)
    ↓
    ├── Task 3 (Reflection Extension) ← depends on SkillExtraction from Task 2
    ├── Task 4 (SkillManageTool)
    ├── Task 5 (SkillSearchTool)
    └── Task 6 (SkillRecaller)

Task 7 (Delete Deprecated Code) ← independent of Track A
    ↓
Task 8 (Simplify Consolidate)

Task 9 (Integration Verification) ← depends on all above
```

Tasks 3-6 can run in parallel after Task 2. Task 7 is independent of Track A. Task 9 runs last.

---

## Out of Scope (Future Work)

These items are noted in the spec but intentionally deferred:

1. **LLM wiring for dreaming stages** (drift, synthesis, tunnel, summarize) — requires LLM calling infrastructure in dreaming context; separate PR
2. **SKILL.md export tool** — optional human-readable export; implement when users request it
3. **AlephTool trait implementation** for skill_manage/skill_search — requires `AlephToolServer` integration; separate PR after core logic is proven
4. **Prompt assembly integration** — wiring SkillRecaller into the actual prompt builder; requires understanding the prompt assembly pipeline; separate PR
