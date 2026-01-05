# ai-routing Specification Delta

## MODIFIED Requirements

### Requirement: Two-Type Routing Rule System
The system SHALL distinguish between Command Rules and Keyword Rules.

#### Scenario: Classify rule by type
- **WHEN** loading a routing rule from config
- **THEN** rule is classified as "command" or "keyword"
- **AND** if `rule_type` is explicitly set, use that value
- **AND** if `rule_type` is not set and regex starts with `^/`, classify as "command"
- **AND** if `rule_type` is not set and regex does not start with `/`, classify as "keyword"
- **AND** classification is stored in `RoutingRule.rule_type` field

#### Scenario: Command rule requires provider
- **WHEN** rule is classified as "command"
- **THEN** rule MUST have `provider` field specified
- **AND** if `provider` is missing, return `AetherError::InvalidConfig`
- **AND** rule MAY have `system_prompt` field
- **AND** `strip_prefix` defaults to true for command rules

#### Scenario: Keyword rule requires prompt only
- **WHEN** rule is classified as "keyword"
- **THEN** rule MUST have `system_prompt` field specified
- **AND** `provider` field is ignored if present
- **AND** keyword rules do not affect provider selection
- **AND** `strip_prefix` is ignored for keyword rules

### Requirement: Command Rule Matching (First-Match-Stops)
The system SHALL match command rules using first-match-stops logic.

#### Scenario: Match first command rule
- **WHEN** `router.match_rules(input)` is called
- **THEN** command rules are evaluated in order (by config position)
- **AND** first matching command rule stops command matching
- **AND** subsequent command rules are NOT evaluated
- **AND** matched command rule provides `provider_name` and `system_prompt`

#### Scenario: Strip command prefix from input
- **WHEN** command rule matches input
- **THEN** matched prefix is stripped from input
- **AND** stripped input is stored in `MatchedCommandRule.cleaned_input`
- **AND** cleaned input does NOT include the command (e.g., `/draw`)
- **AND** cleaned input is trimmed of leading whitespace
- **AND** original input is preserved for logging

#### Scenario: Validate non-empty cleaned input
- **WHEN** command rule matches and prefix is stripped
- **AND** cleaned input is empty or whitespace-only
- **THEN** trigger Halo error state with localized message
- **AND** message key is "error.command.empty" ("指令需要内容")
- **AND** do NOT proceed with AI processing
- **AND** Halo shows error animation and dismisses after timeout

### Requirement: Keyword Rule Matching (All-Match)
The system SHALL match keyword rules using all-match logic.

#### Scenario: Match all keyword rules
- **WHEN** `router.match_rules(input)` is called
- **THEN** all keyword rules are evaluated against input
- **AND** all matching keyword rules are collected
- **AND** matching does NOT stop after first match
- **AND** matched keyword rules provide `system_prompt` only

#### Scenario: Keyword matching is independent of command matching
- **WHEN** input matches both command and keyword rules
- **THEN** command matching and keyword matching are independent
- **AND** both command rule and all keyword rules can match simultaneously
- **AND** keyword rules are evaluated against ORIGINAL input (not cleaned)

### Requirement: Prompt Assembly
The system SHALL combine prompts from command and keyword rules.

#### Scenario: Assemble combined prompt
- **WHEN** `RoutingMatch.assemble_prompt()` is called
- **THEN** command rule prompt (if any) is added first
- **AND** all keyword rule prompts are added in order
- **AND** prompts are joined with `\n\n` (double newline)
- **AND** empty prompts are skipped

#### Scenario: Command-only prompt
- **WHEN** only command rule matches
- **AND** command rule has system_prompt
- **THEN** assembled prompt = command rule's system_prompt

#### Scenario: Keyword-only prompts
- **WHEN** no command rule matches
- **AND** keyword rules match
- **THEN** assembled prompt = all keyword prompts joined with ", "

#### Scenario: Combined prompts
- **WHEN** command rule matches with prompt "A"
- **AND** keyword rules match with prompts "B" and "C"
- **THEN** assembled prompt = "A, B, C"

### Requirement: Provider Selection
The system SHALL determine provider from command rule or default.

#### Scenario: Provider from command rule
- **WHEN** command rule matches
- **THEN** use provider from matched command rule
- **AND** ignore `default_provider` setting

#### Scenario: Provider from default
- **WHEN** no command rule matches
- **AND** `default_provider` is configured
- **THEN** use `default_provider`
- **AND** keyword rules do not affect provider selection

#### Scenario: No provider available
- **WHEN** no command rule matches
- **AND** `default_provider` is not configured
- **THEN** return `AetherError::NoProviderAvailable`
- **AND** log error with helpful message

### Requirement: Builtin Command Rules
The system SHALL provide preset command rules that cannot be deleted.

#### Scenario: Preset commands are always available
- **WHEN** loading configuration
- **THEN** builtin commands (/search, /mcp, /skill) are merged with user rules
- **AND** builtin commands use `default_provider` if available
- **AND** user can override builtin commands by creating rule with same regex

#### Scenario: Builtin command prefix stripping
- **WHEN** builtin command like `/search what is AI` matches
- **THEN** prefix `/search ` is stripped
- **AND** AI receives only "what is AI"
- **AND** builtin command's system_prompt is applied

#### Scenario: Builtin commands visible in Settings UI
- **WHEN** user opens routing rules settings
- **THEN** builtin commands (/search, /mcp, /skill) are displayed
- **AND** builtin commands are marked with "内置指令" badge
- **AND** builtin commands show lock icon (🔒) indicating read-only
- **AND** user can view but not edit/delete builtin commands
- **AND** builtin commands serve as usage guide for users

## ADDED Requirements

### Requirement: RoutingMatch Return Type
The system SHALL return structured match results.

#### Scenario: RoutingMatch structure
- **WHEN** `router.match_rules(input)` completes
- **THEN** return `RoutingMatch` struct containing:
  - `command_rule: Option<MatchedCommandRule>`
  - `keyword_rules: Vec<MatchedKeywordRule>`
- **AND** `MatchedCommandRule` contains:
  - `provider_name: String`
  - `system_prompt: Option<String>`
  - `cleaned_input: String`
- **AND** `MatchedKeywordRule` contains:
  - `system_prompt: String`

### Requirement: Rule Type Auto-Detection
The system SHALL auto-detect rule type for backward compatibility.

#### Scenario: Auto-detect command rule
- **WHEN** rule has no `rule_type` field
- **AND** regex starts with `^/`
- **THEN** classify as "command" rule
- **AND** log info about auto-detection

#### Scenario: Auto-detect keyword rule
- **WHEN** rule has no `rule_type` field
- **AND** regex does not start with `^/` or `/`
- **THEN** classify as "keyword" rule
- **AND** log info about auto-detection

#### Scenario: Explicit rule type takes precedence
- **WHEN** rule has explicit `rule_type` field
- **THEN** use explicit value regardless of regex pattern
- **AND** validate that command rules have provider

### Requirement: Keyword Match Limit
The system SHALL limit keyword rule matches for performance.

#### Scenario: Warn on excessive keyword matches
- **WHEN** more than 10 keyword rules match
- **THEN** log warning about performance impact
- **AND** all matched rules are still included
- **AND** warning includes count of matched rules

## REMOVED Requirements

(None - this is an enhancement, not removal)
