# Skill Trigger Enhancement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `when_to_use` field to SkillManifest so the LLM knows when to proactively invoke each skill, with XML output and proactive trigger guidance.

**Architecture:** Extend SkillManifest with one optional field, parse it from YAML frontmatter, include it in XML prompt output, and update deferred loading guidance to instruct proactive invocation.

**Tech Stack:** Rust, serde_yaml for frontmatter parsing, existing SkillManifest/PromptLayer patterns.

---

### Task 1: Add when_to_use field to SkillManifest

**Files:**
- Modify: `src/domain/skill.rs:377-568`

- [ ] **Step 1: Write failing tests**

Add to the existing `#[cfg(test)]` module in `src/domain/skill.rs` (find the test module — it's further down in the file):

```rust
#[test]
fn test_skill_manifest_when_to_use_default() {
    let manifest = SkillManifest::new(
        "test",
        "Test Skill",
        "A test skill",
        SkillContent::new("content"),
        SkillSource::Bundled,
    );
    assert!(manifest.when_to_use().is_none());
}

#[test]
fn test_skill_manifest_set_when_to_use() {
    let mut manifest = SkillManifest::new(
        "test",
        "Test Skill",
        "A test skill",
        SkillContent::new("content"),
        SkillSource::Bundled,
    );
    manifest.set_when_to_use("When code needs review".to_string());
    assert_eq!(manifest.when_to_use(), Some("When code needs review"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib domain::skill::tests::test_skill_manifest_when_to_use_default`
Expected: FAIL — `when_to_use` method does not exist

- [ ] **Step 3: Add field to SkillManifest struct**

In `src/domain/skill.rs`, add after the `emoji` field (line 406):

```rust
    /// Trigger hint — describes when this skill should be proactively invoked.
    when_to_use: Option<String>,
```

- [ ] **Step 4: Update constructor**

In `SkillManifest::new()` (line 418-434), add after `emoji: None,`:

```rust
            when_to_use: None,
```

- [ ] **Step 5: Add getter**

After the `emoji()` getter (line 499-501), add:

```rust
    /// When this skill should be proactively invoked.
    pub fn when_to_use(&self) -> Option<&str> {
        self.when_to_use.as_deref()
    }
```

- [ ] **Step 6: Add setter**

After the `set_emoji()` setter (line 566-568), add:

```rust
    /// Set the when_to_use trigger hint.
    pub fn set_when_to_use(&mut self, hint: String) {
        self.when_to_use = Some(hint);
    }
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib domain::skill`
Expected: ALL PASS

- [ ] **Step 8: Commit**

```bash
git add src/domain/skill.rs
git commit -m "feat(skill): add when_to_use field to SkillManifest"
```

---

### Task 2: Parse when-to-use from YAML frontmatter

**Files:**
- Modify: `src/skill/manifest.rs:64-85` (RawFrontmatter) and `src/skill/manifest.rs:134-241` (parse_skill_content)

- [ ] **Step 1: Write failing test**

Add to the existing tests module in `src/skill/manifest.rs`:

```rust
#[test]
fn parse_when_to_use_from_frontmatter() {
    let content = r#"---
name: Code Review
description: Reviews code for quality
when-to-use: When code has been written or modified and needs quality review
---
Review instructions."#;

    let manifest = parse_skill_content(content, SkillSource::Bundled).unwrap();
    assert_eq!(
        manifest.when_to_use(),
        Some("When code has been written or modified and needs quality review")
    );
}

#[test]
fn parse_when_to_use_absent() {
    let content = r#"---
name: Simple Skill
description: No trigger hint
---
Content."#;

    let manifest = parse_skill_content(content, SkillSource::Bundled).unwrap();
    assert!(manifest.when_to_use().is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib skill::manifest::tests::parse_when_to_use_from_frontmatter`
Expected: FAIL — `when_to_use` is None because it's not parsed from frontmatter yet

- [ ] **Step 3: Add field to RawFrontmatter**

In `src/skill/manifest.rs`, in the `RawFrontmatter` struct (line 64-85), add after the `emoji` field (line 84):

```rust
    #[serde(default)]
    when_to_use: Option<String>,
```

Note: The struct uses `#[serde(rename_all = "kebab-case")]` so the YAML key `when-to-use` maps to `when_to_use` automatically.

- [ ] **Step 4: Add parsing logic in parse_skill_content**

In `src/skill/manifest.rs`, in `parse_skill_content()`, add after the emoji handling (after line 237 `manifest.set_emoji(emoji);` and its closing brace):

```rust
    if let Some(when) = raw.when_to_use {
        manifest.set_when_to_use(when);
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib skill::manifest`
Expected: ALL PASS (existing + 2 new)

- [ ] **Step 6: Commit**

```bash
git add src/skill/manifest.rs
git commit -m "feat(skill): parse when-to-use from YAML frontmatter"
```

---

### Task 3: XML output and proactive trigger guidance

**Files:**
- Modify: `src/skill/prompt.rs`

- [ ] **Step 1: Write failing tests**

Add to the existing tests module in `src/skill/prompt.rs`:

```rust
#[test]
fn xml_includes_when_to_use() {
    let mut skill = make_skill("Code Review", "Reviews code quality");
    skill.set_when_to_use("When code has been modified".to_string());
    let xml = build_skills_prompt_xml(&[&skill]);

    assert!(xml.contains("<when>When code has been modified</when>"));
}

#[test]
fn xml_omits_when_tag_if_none() {
    let skill = make_skill("Simple", "A simple skill");
    let xml = build_skills_prompt_xml(&[&skill]);

    assert!(!xml.contains("<when>"));
    assert!(xml.contains("<name>Simple</name>"));
}

#[test]
fn xml_escapes_when_to_use() {
    let mut skill = make_skill("Test", "Test skill");
    skill.set_when_to_use("When <user> asks & needs help".to_string());
    let xml = build_skills_prompt_xml(&[&skill]);

    assert!(xml.contains("<when>When &lt;user&gt; asks &amp; needs help</when>"));
}

#[test]
fn deferred_loading_guidance_includes_proactive_trigger() {
    assert!(
        DEFERRED_LOADING_GUIDANCE.contains("proactively"),
        "Guidance should mention proactive invocation"
    );
    assert!(
        DEFERRED_LOADING_GUIDANCE.contains("<when>"),
        "Guidance should reference the <when> trigger tag"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib skill::prompt::tests::xml_includes_when_to_use`
Expected: FAIL — `<when>` tag not generated yet

- [ ] **Step 3: Update build_skills_prompt_xml to include when tag**

In `src/skill/prompt.rs`, in `build_skills_prompt_xml()`, add after the description closing tag line (`buf.push_str("</description>\n");` around line 39):

```rust
        if let Some(when) = skill.when_to_use() {
            buf.push_str("    <when>");
            buf.push_str(&escape_xml(when));
            buf.push_str("</when>\n");
        }
```

- [ ] **Step 4: Update DEFERRED_LOADING_GUIDANCE**

In `src/skill/prompt.rs`, replace the `DEFERRED_LOADING_GUIDANCE` constant (lines 7-10) with:

```rust
pub const DEFERRED_LOADING_GUIDANCE: &str =
    "To use a skill, first call the `skill_read` tool with the skill name \
     to load its full instructions, then follow those instructions. \
     Use `skill_list` to discover available skills if needed.\n\n\
     When a user's request matches a skill's <when> trigger, proactively \
     invoke that skill without waiting for an explicit request.";
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib skill::prompt`
Expected: ALL PASS (existing + 4 new)

- [ ] **Step 6: Run full cargo check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add src/skill/prompt.rs
git commit -m "feat(skill): add <when> tag to XML output and proactive trigger guidance"
```

---

### Task 4: Integration verification

**Files:** None (read-only)

- [ ] **Step 1: Run full cargo check**

Run: `cargo check -p alephcore`
Expected: PASS

- [ ] **Step 2: Run all tests**

Run: `cargo test -p alephcore --lib`
Expected: ALL PASS

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p alephcore`
Expected: No new warnings from our changes
