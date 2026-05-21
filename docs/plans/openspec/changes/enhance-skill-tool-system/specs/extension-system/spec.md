# Extension System - Skill Tool Enhancement

## ADDED Requirements

### Requirement: Skill Tool Invocation
The system SHALL provide a `invoke_skill_tool()` method that allows LLM to dynamically invoke Skills as Tools, returning structured results including content, base directory, and metadata.

#### Scenario: Successful skill invocation
- **WHEN** LLM calls skill tool with valid skill name and arguments
- **THEN** system loads skill content, renders template, and returns `SkillToolResult`

#### Scenario: Skill not found
- **WHEN** LLM calls skill tool with non-existent skill name
- **THEN** system returns `SkillNotFound` error

#### Scenario: Permission denied
- **WHEN** agent permissions deny access to requested skill
- **THEN** system returns `PermissionDenied` error without loading skill content

### Requirement: Instance-Level Caching
The system SHALL cache loaded extension components at instance level and provide lazy-loading via `ensure_loaded()` method to avoid repeated filesystem scanning.

#### Scenario: First access triggers load
- **WHEN** `ensure_loaded()` called for first time
- **THEN** system scans filesystem and populates cache

#### Scenario: Subsequent access uses cache
- **WHEN** `ensure_loaded()` called after initial load
- **THEN** system returns immediately without filesystem scan

#### Scenario: Force reload
- **WHEN** `reload()` called
- **THEN** system clears cache and rescans filesystem

### Requirement: Template File References
The system SHALL support `@./path` and `@/path` syntax in skill content to reference and inline file contents relative to skill source directory.

#### Scenario: Relative file reference
- **WHEN** skill content contains `@./config.json`
- **THEN** system inlines content of `config.json` from skill's directory

#### Scenario: Absolute file reference
- **WHEN** skill content contains `@/etc/hosts`
- **THEN** system inlines content of `/etc/hosts`

#### Scenario: File not found
- **WHEN** referenced file does not exist
- **THEN** system returns error with file path

#### Scenario: Path security validation
- **WHEN** relative path attempts directory traversal beyond skill root
- **THEN** system returns error rejecting the path

### Requirement: Skill Permission Integration
The system SHALL check agent permissions before loading skill content, supporting `allow`, `deny`, and `ask` actions.

#### Scenario: Permission explicitly allowed
- **WHEN** agent permission rule for skill is `allow`
- **THEN** skill invocation proceeds without prompting

#### Scenario: Permission explicitly denied
- **WHEN** agent permission rule for skill is `deny`
- **THEN** skill invocation fails with `PermissionDenied` error

#### Scenario: Permission requires ask
- **WHEN** agent permission rule for skill is `ask`
- **THEN** system requests user confirmation before proceeding

#### Scenario: No matching rule
- **WHEN** no permission rule matches the skill
- **THEN** system defaults to `ask` behavior

### Requirement: Structured Skill Result
The system SHALL return `SkillToolResult` containing title, rendered content, base directory path, and metadata when skill is successfully invoked.

#### Scenario: Result structure
- **WHEN** skill invocation succeeds
- **THEN** result contains:
  - `title`: "Loaded skill: {name}"
  - `content`: rendered skill content with templates expanded
  - `base_dir`: source directory of skill file
  - `metadata.name`: skill name
  - `metadata.qualified_name`: plugin:skill or skill
  - `metadata.source`: discovery source (Global/Project)
