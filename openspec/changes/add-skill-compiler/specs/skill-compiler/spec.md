# skill-compiler Specification

## Purpose
Provide a Skill Compiler that converts repeated successful patterns into reusable skills or tool-backed automations with explicit user approval.

## ADDED Requirements
### Requirement: Execution Tracking for Patterns
The system SHALL record repeated execution patterns with status, context, and metrics for solidification.

Each execution record MUST include:
- pattern_id (string)
- session_id (string)
- invoked_at (unix timestamp)
- duration_ms (integer)
- status (success | partial_success | failed | error)
- context (string)
- input_summary (string)

#### Scenario: Log successful execution
- **GIVEN** a completed execution with status=success
- **WHEN** the execution is recorded
- **THEN** the system inserts a record for the pattern_id
- **AND** updates aggregated metrics (success rate, last_used)

#### Scenario: Log failed execution
- **GIVEN** a completed execution with status=failed
- **WHEN** the execution is recorded
- **THEN** the system inserts a record for the pattern_id
- **AND** updates failure rate metrics

---

### Requirement: Solidification Detection
The system SHALL detect solidification candidates using configurable thresholds.

Thresholds MUST include:
- min_success_count
- min_success_rate
- min_age_days
- max_idle_days

#### Scenario: Candidate meets thresholds
- **GIVEN** a pattern with success_count >= min_success_count
- **AND** success_rate >= min_success_rate
- **AND** age_days >= min_age_days
- **AND** idle_days <= max_idle_days
- **WHEN** detection runs
- **THEN** the pattern is returned as a solidification candidate

#### Scenario: Candidate does not meet thresholds
- **GIVEN** a pattern with success_count < min_success_count
- **WHEN** detection runs
- **THEN** the pattern is not returned

---

### Requirement: Suggestion Generation
The system SHALL generate a solidification suggestion for each candidate.

Each suggestion MUST include:
- suggested_name (kebab-case)
- suggested_description
- instructions_preview (markdown)
- confidence (0.0 - 1.0)
- sample_contexts

#### Scenario: AI-assisted suggestion
- **GIVEN** an AI provider is available
- **WHEN** suggestion generation runs
- **THEN** the system uses the provider to produce name, description, and instructions

#### Scenario: Fallback suggestion
- **GIVEN** no AI provider is available
- **WHEN** suggestion generation runs
- **THEN** the system generates a heuristic suggestion

---

### Requirement: User Approval Workflow
The system SHALL require explicit user approval before persisting a generated skill or tool.

#### Scenario: User approves suggestion
- **GIVEN** a suggestion is presented to the user
- **WHEN** the user approves (and optionally edits name/description/instructions)
- **THEN** the compiler proceeds to generate the skill

#### Scenario: User declines suggestion
- **GIVEN** a suggestion is presented
- **WHEN** the user declines
- **THEN** no files are created
- **AND** the suggestion is suppressed for a cooldown period

---

### Requirement: Skill Generation and Registration
The system SHALL generate a SKILL.md file and register the skill on approval.

#### Scenario: Create new skill
- **GIVEN** an approved suggestion with name "convert-csv-currency"
- **WHEN** generation runs
- **THEN** the system creates `~/.aether/skills/convert-csv-currency/SKILL.md`
- **AND** returns a diff preview
- **AND** reloads the SkillsRegistry to include the new skill

#### Scenario: Skill already exists
- **GIVEN** a skill directory already exists for the suggested name
- **WHEN** generation runs
- **THEN** the system does not overwrite the existing skill
- **AND** returns an AlreadyExists result

---

### Requirement: Tool-Backed Skill Generation (Optional)
When enabled, the system SHALL support generating tool-backed skills for deterministic transformations.

A tool-backed skill package MUST include:
- `tool_definition.json` (tool name, description, input schema)
- `entrypoint.py` (or other supported runtime)
- optional `tests.json` (sample inputs/expected outputs)

#### Scenario: Generate tool-backed skill
- **GIVEN** tool_generation_enabled = true
- **AND** a suggestion is marked tool_candidate
- **WHEN** generation runs
- **THEN** the tool package is created
- **AND** the tool is registered with ToolServer on successful tests

---

### Requirement: Tool Self-Test and Safety Gating
The system SHALL run a self-test for tool-backed skills and gate first use behind confirmation.

#### Scenario: Self-test failure
- **GIVEN** a tool-backed skill with tests.json
- **WHEN** the tests fail
- **THEN** the tool is not registered
- **AND** generated files are removed or marked invalid

#### Scenario: First-run confirmation
- **GIVEN** a generated tool is registered
- **WHEN** the tool is invoked for the first time
- **THEN** the system requests user confirmation before execution

---

### Requirement: Compiler Configuration
The system SHALL expose configuration for the Skill Compiler.

Config fields MUST include:
- enabled (bool)
- min_success_count (int)
- min_success_rate (float)
- min_age_days (int)
- max_idle_days (int)
- auto_suggest (bool)
- tool_generation_enabled (bool)
- test_timeout_ms (int)

#### Scenario: Defaults
- **WHEN** no compiler config is provided
- **THEN** enabled = true
- **AND** min_success_count = 3
- **AND** min_success_rate = 0.8
- **AND** auto_suggest = true
- **AND** tool_generation_enabled = false
