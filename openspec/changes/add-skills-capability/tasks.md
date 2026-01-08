# Tasks: Add Skills Capability

## Phase 1: Data Types and Registry (Foundation)

### 1.1 Skills Module and Data Types
- [ ] 1.1.1 Create `skills/mod.rs` module with exports
- [ ] 1.1.2 Define `SkillFrontmatter` struct: `name`, `description`, `allowed_tools`
- [ ] 1.1.3 Define `Skill` struct: `id`, `frontmatter`, `instructions`
- [ ] 1.1.4 Implement `Skill::parse()` for SKILL.md parsing (YAML frontmatter + markdown body)
- [ ] 1.1.5 Add unit tests for Skill parsing (valid/invalid SKILL.md)

### 1.2 Skills Registry
- [ ] 1.2.1 Create `skills/registry.rs`
- [ ] 1.2.2 Implement `SkillsRegistry::new(skills_dir: PathBuf)`
- [ ] 1.2.3 Implement `load_all()` to scan skills directory
- [ ] 1.2.4 Implement `get_skill(id: &str) -> Option<Skill>`
- [ ] 1.2.5 Implement `find_matching(input: &str) -> Option<Skill>` (keyword matching)
- [ ] 1.2.6 Implement `list_skills() -> Vec<Skill>`
- [ ] 1.2.7 Add unit tests for registry operations

### 1.3 Capability Enum Extension
- [ ] 1.3.1 Add `Skills = 4` variant in `payload/capability.rs`
- [ ] 1.3.2 Update `Capability::parse()` to handle "skills"
- [ ] 1.3.3 Update `Capability::as_str()` to return "skills"
- [ ] 1.3.4 Update existing tests for new variant

## Phase 2: Strategy Implementation (Core Logic)

### 2.1 SkillsStrategy
- [ ] 2.1.1 Create `capability/strategies/skills.rs`
- [ ] 2.1.2 Implement `CapabilityStrategy` trait for `SkillsStrategy`
- [ ] 2.1.3 Implement `capability_type()` → `Capability::Skills`
- [ ] 2.1.4 Implement `priority()` → `4` (after Video)
- [ ] 2.1.5 Implement `is_available()` → check registry existence
- [ ] 2.1.6 Implement `execute()` → load skill and inject instructions
- [ ] 2.1.7 Add to `capability/strategies/mod.rs` exports
- [ ] 2.1.8 Add unit tests for SkillsStrategy

### 2.2 Payload Extensions
- [ ] 2.2.1 Add `skill_id: Option<String>` to `PayloadMeta`
- [ ] 2.2.2 Add `skill_instructions: Option<String>` to `AgentContext`
- [ ] 2.2.3 Update `PayloadBuilder` to support `skill_id`
- [ ] 2.2.4 Update tests for new fields

## Phase 3: Integration (Wire Everything Together)

### 3.1 Configuration
- [ ] 3.1.1 Add `SkillsConfig` struct in `config/mod.rs`
- [ ] 3.1.2 Add `[skills]` section to `Config`
- [ ] 3.1.3 Support `enabled`, `skills_dir`, `auto_match_enabled` fields
- [ ] 3.1.4 Add default values for SkillsConfig
- [ ] 3.1.5 Update config loading/parsing

### 3.2 AetherCore Integration
- [ ] 3.2.1 Add `SkillsRegistry` initialization in `AetherCore::new()`
- [ ] 3.2.2 Register `SkillsStrategy` with `CompositeCapabilityExecutor`
- [ ] 3.2.3 Pass `SkillsConfig` to strategy creation
- [ ] 3.2.4 Add skills directory creation on first launch

### 3.3 PromptAssembler
- [ ] 3.3.1 Add skill_instructions injection in `assemble_system_prompt()`
- [ ] 3.3.2 Format: `## Skill Instructions\n\n{instructions}`
- [ ] 3.3.3 Position at end of system prompt (after video transcript)
- [ ] 3.3.4 Add unit tests for prompt assembly with skills

## Phase 4: Router and Commands (User Interface)

### 4.1 Router Integration
- [ ] 4.1.1 Add `/skill` builtin command detection (already exists as placeholder)
- [ ] 4.1.2 Extract skill_id from `/skill <name>` command
- [ ] 4.1.3 Set `payload.meta.skill_id` when skill command matched
- [ ] 4.1.4 Auto-enable `Capability::Skills` for `/skill` command
- [ ] 4.1.5 Strip `/skill <name>` prefix from user input
- [ ] 4.1.6 Add unit tests for /skill routing

### 4.2 Optional: Auto-matching
- [ ] 4.2.1 Implement auto-match hook in Router (when enabled)
- [ ] 4.2.2 Check `SkillsRegistry.find_matching()` for non-command inputs
- [ ] 4.2.3 Set skill_id automatically if match found
- [ ] 4.2.4 Add unit tests for auto-matching

## Phase 5: Built-in Skills and Resources

### 5.1 Built-in Skill Files
- [ ] 5.1.1 Create `Resources/skills/refine-text/SKILL.md`
- [ ] 5.1.2 Create `Resources/skills/translate/SKILL.md`
- [ ] 5.1.3 Create `Resources/skills/summarize/SKILL.md`
- [ ] 5.1.4 Verify SKILL.md files parse correctly

### 5.2 First-Launch Copy Logic
- [ ] 5.2.1 Implement skills directory initialization
- [ ] 5.2.2 Copy built-in skills to user directory if not exist
- [ ] 5.2.3 Never overwrite existing user skills
- [ ] 5.2.4 Log skill initialization results

## Phase 6: Module Exports and lib.rs

### 6.1 Public API
- [ ] 6.1.1 Add `pub mod skills` in `lib.rs`
- [ ] 6.1.2 Re-export `Skill`, `SkillsRegistry` types
- [ ] 6.1.3 Ensure `Capability::Skills` is accessible

## Phase 7: Testing and Validation

### 7.1 Unit Tests
- [ ] 7.1.1 Test: Skill parsing with valid SKILL.md
- [ ] 7.1.2 Test: Skill parsing with invalid format
- [ ] 7.1.3 Test: Registry loads all skills from directory
- [ ] 7.1.4 Test: Registry returns None for missing skill
- [ ] 7.1.5 Test: SkillsStrategy executes correctly
- [ ] 7.1.6 Test: PromptAssembler includes skill instructions

### 7.2 Integration Tests
- [ ] 7.2.1 E2E test: `/skill refine-text` explicit call
- [ ] 7.2.2 E2E test: Auto-match (when enabled)
- [ ] 7.2.3 E2E test: Skill + Memory combination
- [ ] 7.2.4 E2E test: Unknown skill error handling
- [ ] 7.2.5 E2E test: Empty skills directory

### 7.3 Manual Testing
- [ ] 7.3.1 Test all 3 built-in skills manually
- [ ] 7.3.2 Test creating custom skill
- [ ] 7.3.3 Test skill hot-reload (manual reload)
- [ ] 7.3.4 Test skill with Memory capability

## Phase 8: Documentation

### 8.1 Code Documentation
- [ ] 8.1.1 Add module-level documentation for skills/
- [ ] 8.1.2 Document public API (Skill, SkillsRegistry)
- [ ] 8.1.3 Document SkillsStrategy

### 8.2 User Documentation
- [ ] 8.2.1 Add Skills section to CLAUDE.md
- [ ] 8.2.2 Document SKILL.md format
- [ ] 8.2.3 Add example: Creating a custom skill
- [ ] 8.2.4 Document `/skill` command usage

---

## Dependencies

```
Phase 1 (Foundation)
    │
    ├──► Phase 2 (Strategy) ──► Phase 3 (Integration) ──► Phase 4 (Router)
    │                              │
    │                              └──► Phase 6 (Exports)
    │
    └──► Phase 5 (Built-in Skills)

Phase 7 (Testing) depends on all phases
Phase 8 (Documentation) can start after Phase 3
```

## Parallelizable Work

| Phase | Can Run in Parallel With |
|-------|-------------------------|
| 1.1 (Data Types) | - |
| 1.2 (Registry) | 1.1 (after struct definitions) |
| 1.3 (Capability Enum) | 1.1, 1.2 |
| 5.1 (Built-in Skills) | All phases |
| 8.x (Documentation) | After Phase 3 |

## Estimated Scope

| Phase | Files Changed | Lines Added (est.) |
|-------|--------------|-------------------|
| 1 | 3 new, 1 modified | ~300 |
| 2 | 2 new, 2 modified | ~250 |
| 3 | 2 modified | ~100 |
| 4 | 1 modified | ~50 |
| 5 | 3 new files | ~100 |
| 6 | 1 modified | ~10 |
| 7 | 2 modified | ~200 |
| 8 | 2 modified | ~100 |
| **Total** | ~10 files | ~1100 lines |

## Completion Criteria

- [ ] All unit tests pass (`cargo test`)
- [ ] All integration tests pass
- [ ] `openspec validate add-skills-capability --strict` passes
- [ ] `/skill refine-text` works in manual testing
- [ ] Built-in skills copy correctly on first launch
- [ ] Documentation updated
