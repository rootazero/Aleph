# Tasks: Add Skills Capability

## 1. Rust Core: Skills Data Types

- [ ] 1.1 Create `skills/mod.rs` module
- [ ] 1.2 Add `SkillFrontmatter` struct: `name`, `description`, `allowed_tools`
- [ ] 1.3 Add `Skill` struct: `id`, `frontmatter`, `instructions`
- [ ] 1.4 Implement `Skill::parse()` for SKILL.md parsing (YAML frontmatter + markdown)
- [ ] 1.5 Add `Capability::Skills = 5` variant in `payload/capability.rs`
- [ ] 1.6 Add unit tests for Skill parsing

## 2. Rust Core: Skills Registry

- [ ] 2.1 Create `skills/registry.rs`
- [ ] 2.2 Implement `SkillsRegistry::new(skills_dir: PathBuf)`
- [ ] 2.3 Implement `load_all()` to scan skills directory
- [ ] 2.4 Implement `get_skill(name: &str) -> Option<&Skill>`
- [ ] 2.5 Implement `find_matching(input: &str) -> Option<&Skill>` (keyword matching)
- [ ] 2.6 Implement `list_skills() -> Vec<&Skill>`
- [ ] 2.7 Add hot-reload support (directory watcher)
- [ ] 2.8 Add unit tests for registry

## 3. Rust Core: Skills Executor

- [ ] 3.1 Add `execute_skills()` method to `CapabilityExecutor`
- [ ] 3.2 Load skill from registry by `payload.meta.skill_id`
- [ ] 3.3 Extract and store instructions in `payload.context.skill_instructions`
- [ ] 3.4 Add integration test for executor

## 4. Rust Core: Router Integration

- [ ] 4.1 Add `/skill <name>` builtin command detection
- [ ] 4.2 Add auto-matching logic (check against skill descriptions)
- [ ] 4.3 Set `intent_type = "skills:<name>"` when skill matched
- [ ] 4.4 Update PayloadBuilder to detect `skills:` intent and add capability
- [ ] 4.5 Add unit tests for routing

## 5. Rust Core: Prompt Assembly

- [ ] 5.1 Add `skill_instructions: Option<String>` to `PayloadContext`
- [ ] 5.2 Update `PromptAssembler` to inject skill instructions
- [ ] 5.3 Position skill instructions at end of system prompt
- [ ] 5.4 Add unit tests for assembly

## 6. UniFFI Interface

- [ ] 6.1 Add `Skill` dictionary to `aether.udl`
- [ ] 6.2 Add `list_skills()` method to `AetherCore`
- [ ] 6.3 Add `get_skill(name: String)` method to `AetherCore`
- [ ] 6.4 Regenerate UniFFI bindings

## 7. Built-in Skills

- [ ] 7.1 Create `Resources/skills/refine-text/SKILL.md`
- [ ] 7.2 Create `Resources/skills/translate/SKILL.md`
- [ ] 7.3 Create `Resources/skills/summarize/SKILL.md`
- [ ] 7.4 Add first-launch copy logic to user skills directory
- [ ] 7.5 Verify skills are not overwritten if already exist

## 8. Swift Integration

- [ ] 8.1 Add `Skill` type from UniFFI
- [ ] 8.2 Update Command Mode to show available skills
- [ ] 8.3 Add skill commands to command completion list

## 9. Testing & Validation

- [ ] 9.1 E2E test: `/skill refine-text` explicit call
- [ ] 9.2 E2E test: Auto-match "polish this text"
- [ ] 9.3 E2E test: Skill + Memory combination
- [ ] 9.4 E2E test: Unknown skill error handling
- [ ] 9.5 E2E test: Invalid SKILL.md handling
- [ ] 9.6 Manual test: All 3 built-in skills

## 10. Documentation

- [ ] 10.1 Add Skills section to CLAUDE.md
- [ ] 10.2 Document SKILL.md format
- [ ] 10.3 Add example: Creating a custom skill

## Dependencies

- Task 3 depends on Tasks 1-2 (data types and registry)
- Task 5 depends on Task 3 (executor populates context)
- Task 8 depends on Task 6 (UniFFI interface)
- Task 9 depends on all previous tasks

## Parallelizable

- Tasks 1-2 can be done in parallel (data types + registry structure)
- Task 7 (built-in skills) can be done independently
- Task 10 (documentation) can be done independently
