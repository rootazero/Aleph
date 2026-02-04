# Skills Capability Specification

This specification defines the Skills system for Aleph, implementing the Claude Agent Skills open standard for dynamic instruction injection. Skills integrate with the existing CapabilityStrategy pattern.

## ADDED Requirements

### Requirement: SKILL.md File Format

The system SHALL support Skills defined in SKILL.md files following the Claude Agent Skills standard format.

#### Scenario: Parse valid SKILL.md

- **GIVEN** a SKILL.md file with YAML frontmatter and markdown body
- **WHEN** the system parses the file
- **THEN** it SHALL extract `name`, `description`, `allowed-tools` from frontmatter
- **AND** extract the markdown body as `instructions`

#### Scenario: Handle missing required fields

- **GIVEN** a SKILL.md file missing `name` or `description`
- **WHEN** the system attempts to parse
- **THEN** it SHALL reject the file with validation error
- **AND** log the specific missing field

#### Scenario: Handle empty instructions

- **GIVEN** a SKILL.md file with only frontmatter
- **WHEN** the system parses the file
- **THEN** it SHALL accept the file with empty `instructions`
- **AND** the skill SHALL still be usable

#### Scenario: Handle optional allowed-tools

- **GIVEN** a SKILL.md file without `allowed-tools` field
- **WHEN** the system parses the file
- **THEN** it SHALL default `allowed_tools` to empty list
- **AND** the skill SHALL load successfully

---

### Requirement: Skills Registry

The system SHALL maintain a registry of available Skills loaded from the skills directory.

#### Scenario: Scan skills directory on startup

- **GIVEN** the Aleph application starts
- **WHEN** the skills directory `~/.aleph/skills/` exists
- **THEN** the system SHALL scan all subdirectories for SKILL.md files
- **AND** load valid skills into the registry

#### Scenario: Handle missing skills directory

- **GIVEN** the skills directory does not exist
- **WHEN** the application starts
- **THEN** the system SHALL create the directory
- **AND** copy built-in skills to the directory

#### Scenario: Get skill by ID

- **GIVEN** a skill with directory name "refine-text" is loaded
- **WHEN** `get_skill("refine-text")` is called
- **THEN** the system SHALL return the skill
- **AND** include frontmatter and instructions

#### Scenario: List all skills

- **GIVEN** multiple skills are loaded in the registry
- **WHEN** `list_skills()` is called
- **THEN** the system SHALL return all loaded skills

#### Scenario: Skill auto-matching

- **GIVEN** auto_match_enabled = true in configuration
- **AND** a skill with description "Improve and polish writing"
- **WHEN** `find_matching("please polish this text")` is called
- **THEN** the system SHALL return the matching skill

---

### Requirement: Capability::Skills Enum

The system SHALL add Skills as a capability type in the Capability enum.

#### Scenario: Parse capability from string

- **GIVEN** a capability string "skills"
- **WHEN** `Capability::parse()` is called
- **THEN** it SHALL return `Capability::Skills`

#### Scenario: Capability priority

- **GIVEN** `Capability::Skills`
- **WHEN** priority is queried via sort order
- **THEN** Skills SHALL have priority 4 (after Video = 3)

#### Scenario: Capability as_str

- **GIVEN** `Capability::Skills`
- **WHEN** `as_str()` is called
- **THEN** it SHALL return "skills"

---

### Requirement: SkillsStrategy Implementation

The system SHALL implement SkillsStrategy following the CapabilityStrategy trait pattern.

#### Scenario: Strategy capability_type

- **GIVEN** a SkillsStrategy instance
- **WHEN** `capability_type()` is called
- **THEN** it SHALL return `Capability::Skills`

#### Scenario: Strategy priority

- **GIVEN** a SkillsStrategy instance
- **WHEN** `priority()` is called
- **THEN** it SHALL return 4

#### Scenario: Strategy is_available with registry

- **GIVEN** a SkillsStrategy with a valid SkillsRegistry
- **WHEN** `is_available()` is called
- **THEN** it SHALL return true

#### Scenario: Strategy is_available without registry

- **GIVEN** a SkillsStrategy with registry = None
- **WHEN** `is_available()` is called
- **THEN** it SHALL return false

#### Scenario: Strategy execution with explicit skill_id

- **GIVEN** a payload with `meta.skill_id = Some("refine-text")`
- **AND** the registry contains the skill
- **WHEN** `execute()` is called
- **THEN** `payload.context.skill_instructions` SHALL be set
- **AND** contain the skill's markdown instructions

#### Scenario: Strategy execution with auto-matching

- **GIVEN** a payload without explicit skill_id
- **AND** auto_match_enabled = true
- **AND** user_input matches a skill description
- **WHEN** `execute()` is called
- **THEN** `payload.context.skill_instructions` SHALL be set

#### Scenario: Strategy execution with no match

- **GIVEN** a payload without skill_id
- **AND** no skill description matches user_input
- **WHEN** `execute()` is called
- **THEN** `payload.context.skill_instructions` SHALL remain None
- **AND** execution SHALL complete without error

---

### Requirement: Payload Extensions

The system SHALL extend payload structures for Skills support.

#### Scenario: PayloadMeta skill_id field

- **GIVEN** PayloadMeta structure
- **THEN** it SHALL include `skill_id: Option<String>` field

#### Scenario: AgentContext skill_instructions field

- **GIVEN** AgentContext structure
- **THEN** it SHALL include `skill_instructions: Option<String>` field

#### Scenario: PayloadBuilder skill_id support

- **GIVEN** PayloadBuilder
- **WHEN** building a payload with skill_id
- **THEN** it SHALL set `meta.skill_id` appropriately

---

### Requirement: Prompt Assembly

The system SHALL inject skill instructions into the final system prompt via PromptAssembler.

#### Scenario: Inject skill instructions

- **GIVEN** payload has `context.skill_instructions = Some("...")`
- **WHEN** PromptAssembler builds the system prompt
- **THEN** skill instructions SHALL appear at the end
- **AND** be prefixed with "## Skill Instructions"

#### Scenario: No skill instructions

- **GIVEN** payload has `context.skill_instructions = None`
- **WHEN** PromptAssembler builds the system prompt
- **THEN** no skill instructions section SHALL be added

#### Scenario: Combine with other capabilities

- **GIVEN** payload has memory_snippets, search_results, AND skill_instructions
- **WHEN** PromptAssembler builds the system prompt
- **THEN** skill instructions SHALL appear after all other context
- **AND** the order SHALL be: base → memory → search → video → skills

---

### Requirement: Skills Configuration

The system SHALL support Skills configuration in config.toml.

#### Scenario: Default configuration

- **GIVEN** no `[skills]` section in config.toml
- **WHEN** Config is loaded
- **THEN** `skills.enabled = true`
- **AND** `skills.skills_dir = "~/.aleph/skills"`
- **AND** `skills.auto_match_enabled = false`

#### Scenario: Custom skills directory

- **GIVEN** `[skills]` with `skills_dir = "/custom/path"`
- **WHEN** Config is loaded
- **THEN** registry SHALL use the custom path

#### Scenario: Auto-matching toggle

- **GIVEN** `[skills]` with `auto_match_enabled = true`
- **WHEN** Config is loaded
- **THEN** SkillsStrategy SHALL perform auto-matching

---

### Requirement: Router /skill Command

The system SHALL support the `/skill <name>` builtin command.

#### Scenario: Explicit skill invocation

- **GIVEN** user input "/skill refine-text Fix this paragraph"
- **WHEN** Router processes the input
- **THEN** `Capability::Skills` SHALL be added to capabilities
- **AND** `payload.meta.skill_id = Some("refine-text")`
- **AND** `payload.user_input = "Fix this paragraph"`

#### Scenario: Skill command prefix stripping

- **GIVEN** user input "/skill translate Hello world"
- **WHEN** Router strips the command prefix
- **THEN** the remaining input SHALL be "Hello world"

#### Scenario: Unknown skill command

- **GIVEN** user input "/skill nonexistent"
- **WHEN** Router processes the input
- **THEN** `payload.meta.skill_id = Some("nonexistent")`
- **AND** SkillsStrategy SHALL log warning when skill not found

---

### Requirement: Built-in Skills

The system SHALL provide built-in Skills bundled with the application.

#### Scenario: First-launch initialization

- **GIVEN** `~/.aleph/skills/` does not exist or is empty
- **WHEN** AlephCore initializes
- **THEN** directory SHALL be created
- **AND** built-in skills SHALL be copied (refine-text, translate, summarize)

#### Scenario: Preserve user modifications

- **GIVEN** user has modified a built-in skill's SKILL.md
- **WHEN** AlephCore initializes
- **THEN** user's SKILL.md SHALL NOT be overwritten

#### Scenario: refine-text skill content

- **GIVEN** the refine-text skill
- **THEN** its instructions SHALL include principles for:
  - Clarity
  - Conciseness
  - Flow
  - Grammar

---

### Requirement: CompositeCapabilityExecutor Registration

The system SHALL register SkillsStrategy with CompositeCapabilityExecutor.

#### Scenario: Strategy registration

- **GIVEN** AlephCore initializes
- **WHEN** capability executor is built
- **THEN** SkillsStrategy SHALL be registered
- **AND** appear after VideoStrategy in priority order

#### Scenario: Execution ordering

- **GIVEN** a request with capabilities [Memory, Skills]
- **WHEN** executor.execute_all() runs
- **THEN** MemoryStrategy SHALL execute first
- **THEN** SkillsStrategy SHALL execute after

---

### Requirement: Skills Installer

The system SHALL provide a SkillsInstaller for downloading and managing skills.

#### Scenario: Install official skills

- **GIVEN** the user requests official skills installation
- **WHEN** `install_official_skills()` is called
- **THEN** the system SHALL download from `anthropics/skills` repository
- **AND** extract and install valid SKILL.md files

#### Scenario: Install from GitHub URL

- **GIVEN** a GitHub URL `github.com/user/skill-repo`
- **WHEN** `install_from_github(url)` is called
- **THEN** the system SHALL normalize the URL
- **AND** download the repository as ZIP
- **AND** install valid skills

#### Scenario: Install from local ZIP

- **GIVEN** a local ZIP file path
- **WHEN** `install_from_zip(path)` is called
- **THEN** the system SHALL extract SKILL.md files
- **AND** validate and install each skill

#### Scenario: Skip existing skills

- **GIVEN** a skill already exists in the skills directory
- **WHEN** the same skill is being installed
- **THEN** the existing skill SHALL NOT be overwritten
- **AND** the system SHALL log that it was skipped

#### Scenario: Delete skill

- **GIVEN** an existing skill
- **WHEN** `delete_skill(id)` is called
- **THEN** the skill directory SHALL be removed

---

### Requirement: UniFFI Skills Interface

The system SHALL expose skills operations through UniFFI.

#### Scenario: List skills via UniFFI

- **GIVEN** Swift code calls `core.listSkills()`
- **WHEN** the method executes
- **THEN** it SHALL return a list of SkillInfo objects

#### Scenario: Install skills via UniFFI

- **GIVEN** Swift code calls `core.installOfficialSkills()`
- **WHEN** the async method completes
- **THEN** it SHALL return list of installed skill names

#### Scenario: Delete skill via UniFFI

- **GIVEN** Swift code calls `core.deleteSkill(id)`
- **WHEN** the operation succeeds
- **THEN** the skill SHALL be removed from `listSkills()`

---

## Cross-References

- **core-library**: AlephCore initialization and capability executor setup
- **ai-routing**: Router /skill command detection
- **skills-settings-ui**: Skills management UI specification
- Existing Strategies: `Aleph/core/src/capability/strategies/` (memory.rs, search.rs, video.rs)
- CapabilityStrategy trait: `Aleph/core/src/capability/strategy.rs`
