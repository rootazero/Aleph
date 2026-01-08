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

## Phase 5: Skills Installer (Rust)

### 5.1 Installer Module
- [ ] 5.1.1 Create `skills/installer.rs`
- [ ] 5.1.2 Implement `SkillsInstaller::new(skills_dir: PathBuf)`
- [ ] 5.1.3 Implement `normalize_github_url()` helper
- [ ] 5.1.4 Implement `is_valid_skill_name()` validator
- [ ] 5.1.5 Implement `extract_skill_dir_name()` from ZIP path

### 5.2 Installation Methods
- [ ] 5.2.1 Implement `install_official_skills()` - download from anthropics/skills
- [ ] 5.2.2 Implement `install_from_github(url)` - download from any GitHub repo
- [ ] 5.2.3 Implement `install_from_zip(path)` - local ZIP file
- [ ] 5.2.4 Add ZIP extraction logic with SKILL.md validation
- [ ] 5.2.5 Skip existing skills (no overwrite)
- [ ] 5.2.6 Return list of installed skill names

### 5.3 CRUD Operations
- [ ] 5.3.1 Implement `create_skill(name, content)` - validate and save
- [ ] 5.3.2 Implement `update_skill(name, content)` - validate and update
- [ ] 5.3.3 Implement `delete_skill(id)` - remove directory
- [ ] 5.3.4 Add unit tests for installer operations

### 5.4 Dependencies
- [ ] 5.4.1 Add `zip` crate to Cargo.toml
- [ ] 5.4.2 Add `uuid` crate for temp file naming
- [ ] 5.4.3 Verify `reqwest` is available for HTTP downloads

## Phase 6: Built-in Skills and Resources

### 6.1 Built-in Skill Files
- [ ] 6.1.1 Create `Resources/skills/refine-text/SKILL.md`
- [ ] 6.1.2 Create `Resources/skills/translate/SKILL.md`
- [ ] 6.1.3 Create `Resources/skills/summarize/SKILL.md`
- [ ] 6.1.4 Verify SKILL.md files parse correctly

### 6.2 First-Launch Copy Logic
- [ ] 6.2.1 Implement skills directory initialization
- [ ] 6.2.2 Copy built-in skills to user directory if not exist
- [ ] 6.2.3 Never overwrite existing user skills
- [ ] 6.2.4 Log skill initialization results

## Phase 7: UniFFI Interface

### 7.1 UDL Definitions
- [ ] 7.1.1 Add `SkillInfo` dictionary type to `aether.udl`
- [ ] 7.1.2 Add `list_skills()` method to AetherCore
- [ ] 7.1.3 Add `get_skill(id)` method
- [ ] 7.1.4 Add `reload_skills()` method

### 7.2 Installer Methods
- [ ] 7.2.1 Add `install_official_skills()` async method
- [ ] 7.2.2 Add `install_skill_from_url(url)` async method
- [ ] 7.2.3 Add `install_skill_from_zip(path)` async method
- [ ] 7.2.4 Add `create_skill(name, content)` method
- [ ] 7.2.5 Add `update_skill(name, content)` method
- [ ] 7.2.6 Add `delete_skill(id)` method

### 7.3 Generate Bindings
- [ ] 7.3.1 Run `uniffi-bindgen generate` to update Swift bindings
- [ ] 7.3.2 Verify generated `aether.swift` compiles
- [ ] 7.3.3 Test Swift calls to new methods

## Phase 8: Module Exports and lib.rs

### 8.1 Public API
- [ ] 8.1.1 Add `pub mod skills` in `lib.rs`
- [ ] 8.1.2 Re-export `Skill`, `SkillsRegistry`, `SkillsInstaller` types
- [ ] 8.1.3 Ensure `Capability::Skills` is accessible

## Phase 9: Skills Settings UI (Swift)

### 9.1 SettingsTab Extension
- [ ] 9.1.1 Add `case skills` to `SettingsTab` enum
- [ ] 9.1.2 Add skills icon and label to sidebar
- [ ] 9.1.3 Wire up `SkillsSettingsView` in content switch

### 9.2 SkillsSettingsView
- [ ] 9.2.1 Create `SkillsSettingsView.swift`
- [ ] 9.2.2 Implement toolbar section (search, create, install buttons)
- [ ] 9.2.3 Implement skills list section with `SkillCard`
- [ ] 9.2.4 Implement install options section (3 buttons)
- [ ] 9.2.5 Implement empty state view
- [ ] 9.2.6 Add loading state handling
- [ ] 9.2.7 Wire up `saveBarState` (reset on appear)

### 9.3 SkillCard Component
- [ ] 9.3.1 Create `Components/Molecules/SkillCard.swift`
- [ ] 9.3.2 Implement skill icon, name, description layout
- [ ] 9.3.3 Implement hover-to-show edit/delete actions
- [ ] 9.3.4 Implement delete confirmation dialog
- [ ] 9.3.5 Add skill-specific icons (refine-text, translate, summarize)

### 9.4 SkillEditorPanel
- [ ] 9.4.1 Create `Components/Organisms/SkillEditorPanel.swift`
- [ ] 9.4.2 Implement name field (readonly when editing)
- [ ] 9.4.3 Implement description field
- [ ] 9.4.4 Implement Markdown instructions editor
- [ ] 9.4.5 Implement preview toggle (raw vs rendered)
- [ ] 9.4.6 Generate SKILL.md content from form
- [ ] 9.4.7 Implement save/cancel buttons

### 9.5 SkillInstallSheet
- [ ] 9.5.1 Create `Components/Organisms/SkillInstallSheet.swift`
- [ ] 9.5.2 Implement URL input field
- [ ] 9.5.3 Implement install progress indicator
- [ ] 9.5.4 Display installed skills list
- [ ] 9.5.5 Handle errors with user-friendly messages

### 9.6 Install Actions
- [ ] 9.6.1 Implement `installOfficialSkills()` action
- [ ] 9.6.2 Implement `uploadZipFile()` with NSOpenPanel
- [ ] 9.6.3 Implement URL install via sheet
- [ ] 9.6.4 Show Toast notifications for success/failure

### 9.7 CRUD Actions
- [ ] 9.7.1 Implement `loadSkills()` from core
- [ ] 9.7.2 Implement `saveSkill()` (create/update)
- [ ] 9.7.3 Implement `deleteSkill()` with confirmation

## Phase 10: Localization

### 10.1 English Strings
- [ ] 10.1.1 Add `settings.skills.*` keys to en.lproj
- [ ] 10.1.2 Keys: search, create, install, installed, empty, etc.
- [ ] 10.1.3 Keys: official, from_url, upload_zip descriptions
- [ ] 10.1.4 Keys: delete_confirm, installed_count

### 10.2 Chinese Strings
- [ ] 10.2.1 Add `settings.skills.*` keys to zh-Hans.lproj
- [ ] 10.2.2 Translate all skill-related strings

## Phase 11: Testing and Validation

### 11.1 Rust Unit Tests
- [ ] 11.1.1 Test: Skill parsing with valid SKILL.md
- [ ] 11.1.2 Test: Skill parsing with invalid format
- [ ] 11.1.3 Test: Registry loads all skills from directory
- [ ] 11.1.4 Test: Registry returns None for missing skill
- [ ] 11.1.5 Test: SkillsStrategy executes correctly
- [ ] 11.1.6 Test: PromptAssembler includes skill instructions
- [ ] 11.1.7 Test: Installer URL normalization
- [ ] 11.1.8 Test: Installer name validation
- [ ] 11.1.9 Test: Installer create/update/delete

### 11.2 Integration Tests
- [ ] 11.2.1 E2E test: `/skill refine-text` explicit call
- [ ] 11.2.2 E2E test: Auto-match (when enabled)
- [ ] 11.2.3 E2E test: Skill + Memory combination
- [ ] 11.2.4 E2E test: Unknown skill error handling
- [ ] 11.2.5 E2E test: Empty skills directory

### 11.3 UI Manual Testing
- [ ] 11.3.1 Test: Skills list displays correctly
- [ ] 11.3.2 Test: Search filters skills
- [ ] 11.3.3 Test: Create new skill via editor
- [ ] 11.3.4 Test: Edit existing skill
- [ ] 11.3.5 Test: Delete skill with confirmation
- [ ] 11.3.6 Test: Install official skills button
- [ ] 11.3.7 Test: Install from URL
- [ ] 11.3.8 Test: Upload ZIP file
- [ ] 11.3.9 Test: Empty state and error handling

## Phase 12: Documentation

### 12.1 Code Documentation
- [ ] 12.1.1 Add module-level documentation for skills/
- [ ] 12.1.2 Document public API (Skill, SkillsRegistry, SkillsInstaller)
- [ ] 12.1.3 Document SkillsStrategy

### 12.2 User Documentation
- [ ] 12.2.1 Add Skills section to CLAUDE.md
- [ ] 12.2.2 Document SKILL.md format
- [ ] 12.2.3 Add example: Creating a custom skill
- [ ] 12.2.4 Document `/skill` command usage
- [ ] 12.2.5 Document Settings UI skill management

---

## Dependencies

```
Phase 1 (Foundation)
    │
    ├──► Phase 2 (Strategy) ──► Phase 3 (Integration) ──► Phase 4 (Router)
    │                              │
    │                              ├──► Phase 5 (Installer) ──► Phase 7 (UniFFI)
    │                              │                              │
    │                              └──► Phase 8 (Exports)         │
    │                                                             │
    └──► Phase 6 (Built-in Skills)                               │
                                                                  │
Phase 7 (UniFFI) ──► Phase 9 (UI) ──► Phase 10 (Localization)   │
                                                                  │
Phase 11 (Testing) depends on all phases                         │
Phase 12 (Documentation) can start after Phase 9                 │
```

## Parallelizable Work

| Phase | Can Run in Parallel With |
|-------|-------------------------|
| 1.1 (Data Types) | - |
| 1.2 (Registry) | 1.1 (after struct definitions) |
| 1.3 (Capability Enum) | 1.1, 1.2 |
| 5.x (Installer) | After Phase 1, parallel with Phase 2-4 |
| 6.x (Built-in Skills) | All phases |
| 10.x (Localization) | After Phase 9.2 |
| 12.x (Documentation) | After Phase 9 |

## Estimated Scope

| Phase | Files Changed | Lines Added (est.) |
|-------|--------------|-------------------|
| 1 | 3 new, 1 modified | ~300 |
| 2 | 2 new, 2 modified | ~250 |
| 3 | 2 modified | ~100 |
| 4 | 1 modified | ~50 |
| 5 | 1 new, 1 modified | ~250 |
| 6 | 3 new files | ~100 |
| 7 | 2 modified | ~100 |
| 8 | 1 modified | ~10 |
| 9 | 4 new files, 1 modified | ~600 |
| 10 | 2 modified | ~50 |
| 11 | 2 modified | ~200 |
| 12 | 2 modified | ~150 |
| **Total** | ~18 files | ~2160 lines |

## Completion Criteria

### Core
- [ ] All unit tests pass (`cargo test`)
- [ ] All integration tests pass
- [ ] `openspec validate add-skills-capability --strict` passes
- [ ] `/skill refine-text` works in manual testing
- [ ] Built-in skills copy correctly on first launch

### Installer
- [ ] Official skills install successfully
- [ ] GitHub URL install works
- [ ] ZIP upload installs correctly
- [ ] Create/update/delete operations work

### UI
- [ ] Skills tab visible in Settings
- [ ] Skills list displays with search
- [ ] All install options work
- [ ] Editor creates valid SKILL.md
- [ ] Delete has confirmation dialog

### Documentation
- [ ] CLAUDE.md updated with Skills section
- [ ] API documentation complete
