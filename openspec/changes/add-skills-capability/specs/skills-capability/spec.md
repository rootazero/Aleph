# Skills Capability Specification

This specification defines the Skills system for Aether, implementing the Claude Agent Skills open standard for dynamic instruction injection.

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

### Requirement: Skills Registry

The system SHALL maintain a registry of available Skills loaded from the skills directory.

#### Scenario: Scan skills directory on startup

- **GIVEN** the Aether application starts
- **WHEN** the skills directory `~/.config/aether/skills/` exists
- **THEN** the system SHALL scan all subdirectories for SKILL.md files
- **AND** load valid skills into the registry

#### Scenario: Handle missing skills directory

- **GIVEN** the skills directory does not exist
- **WHEN** the application starts
- **THEN** the system SHALL create the directory
- **AND** copy built-in skills to the directory

#### Scenario: Get skill by name

- **GIVEN** a skill with name "refine-text" is loaded
- **WHEN** `get_skill("refine-text")` is called
- **THEN** the system SHALL return the skill
- **AND** include frontmatter and instructions

#### Scenario: List all skills

- **GIVEN** multiple skills are loaded in the registry
- **WHEN** `list_skills()` is called
- **THEN** the system SHALL return all loaded skills
- **AND** sort alphabetically by name

#### Scenario: Hot-reload on directory change

- **GIVEN** the skills directory is being watched
- **WHEN** a new SKILL.md file is added or modified
- **THEN** the system SHALL reload the affected skill
- **AND** update the registry without restart

### Requirement: Skill Matching

The system SHALL support both explicit skill invocation and automatic description-based matching.

#### Scenario: Explicit skill invocation

- **GIVEN** user input starts with `/skill refine-text`
- **WHEN** the Router processes the input
- **THEN** it SHALL extract the skill name "refine-text"
- **AND** set `intent_type = "skills:refine-text"`

#### Scenario: Extract remaining input after skill command

- **GIVEN** user input is `/skill translate Hello world`
- **WHEN** the Router processes the input
- **THEN** it SHALL set skill name to "translate"
- **AND** pass "Hello world" as the actual user input

#### Scenario: Auto-match by description

- **GIVEN** a skill with description containing "refine" and "polish"
- **WHEN** user input contains "polish this paragraph"
- **THEN** the system MAY match this skill automatically
- **AND** set appropriate intent_type

#### Scenario: No skill match

- **GIVEN** user input does not match any skill description
- **WHEN** no explicit `/skill` command is used
- **THEN** the system SHALL proceed without skill injection
- **AND** process as normal conversation

#### Scenario: Unknown skill name

- **GIVEN** user invokes `/skill unknown-skill`
- **WHEN** the skill is not in the registry
- **THEN** the system SHALL return an error
- **AND** suggest available skills

### Requirement: Skills Capability Execution

The system SHALL execute Skills through the CapabilityExecutor with priority 5.

#### Scenario: Add Skills to capability chain

- **GIVEN** a routing match with `intent_type = "skills:refine-text"`
- **WHEN** the PayloadBuilder processes the match
- **THEN** it SHALL add `Capability::Skills` to payload capabilities
- **AND** set `skill_id = "refine-text"` in metadata

#### Scenario: Execute skill and extract instructions

- **GIVEN** payload has `Capability::Skills` and `skill_id`
- **WHEN** the CapabilityExecutor executes Skills capability
- **THEN** it SHALL load the skill from registry
- **AND** set `payload.context.skill_instructions` to the skill's instructions

#### Scenario: Handle missing skill during execution

- **GIVEN** payload has `skill_id` that doesn't exist in registry
- **WHEN** the executor attempts to load
- **THEN** it SHALL log a warning
- **AND** continue without skill injection (graceful degradation)

### Requirement: Prompt Assembly with Skills

The system SHALL inject skill instructions into the final system prompt.

#### Scenario: Inject skill instructions

- **GIVEN** payload has `skill_instructions` populated
- **WHEN** the PromptAssembler builds the system prompt
- **THEN** it SHALL include skill instructions in the prompt
- **AND** position them after memory and search context

#### Scenario: Combine skill with other context

- **GIVEN** a request uses both Memory and Skills capabilities
- **WHEN** the prompt is assembled
- **THEN** memory context SHALL appear before skill instructions
- **AND** both SHALL be included in the final prompt

#### Scenario: Skill overrides rule system prompt

- **GIVEN** a routing rule has a system_prompt AND a skill is matched
- **WHEN** the prompt is assembled
- **THEN** the skill instructions SHALL be added to the rule's system_prompt
- **AND** both SHALL contribute to the final prompt

### Requirement: Built-in Skills

The system SHALL provide default Skills bundled with the application.

#### Scenario: Copy built-in skills on first launch

- **GIVEN** the user launches Aether for the first time
- **WHEN** the skills directory is empty
- **THEN** the system SHALL copy built-in skills to `~/.config/aether/skills/`
- **AND** include: refine-text, translate, summarize

#### Scenario: Preserve user-modified skills

- **GIVEN** a built-in skill exists in the user's directory
- **WHEN** the user has modified the SKILL.md
- **THEN** the system SHALL NOT overwrite the modification
- **AND** respect user customizations

#### Scenario: Built-in skills content

- **GIVEN** the refine-text skill is loaded
- **THEN** its instructions SHALL guide text improvement
- **AND** include principles for clarity, conciseness, and flow

### Requirement: UniFFI Interface for Skills

The system SHALL expose Skills operations through the UniFFI interface.

#### Scenario: List skills via UniFFI

- **GIVEN** Swift code calls `core.list_skills()`
- **WHEN** the method executes
- **THEN** it SHALL return a list of Skill objects
- **AND** each object SHALL have name, description, and id

#### Scenario: Get skill via UniFFI

- **GIVEN** Swift code calls `core.get_skill("refine-text")`
- **WHEN** the skill exists
- **THEN** it SHALL return the Skill object
- **AND** include the full instructions

### Requirement: Performance

The system SHALL maintain acceptable performance for Skills operations.

#### Scenario: Registry loading time

- **GIVEN** the skills directory contains 20 skills
- **WHEN** the registry scans on startup
- **THEN** all skills SHALL be loaded within 200ms
- **AND** not block application startup

#### Scenario: Skill matching latency

- **GIVEN** the registry has 20 skills
- **WHEN** auto-matching is performed
- **THEN** the match SHALL complete within 10ms
