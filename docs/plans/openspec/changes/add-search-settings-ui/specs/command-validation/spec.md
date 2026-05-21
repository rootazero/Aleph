# Command Validation Spec

**Capability**: Slash command format validation with whitespace enforcement

**Relates to**: `ai-routing`, `halo-ui`

---

## ADDED Requirements

### Requirement: Slash commands require whitespace separator

The Router SHALL enforce whitespace separation between slash commands and their arguments to prevent command conflicts.

**Rationale**: 允许精准命令匹配，避免短命令（如 `/se`）与长命令（如 `/search`）因前缀匹配而冲突

**Priority**: Must Have

#### Scenario: Valid command format is accepted

- **GIVEN** user input is `/search quantum computing`
- **WHEN** Router validates command format
- **THEN** validation passes
- **AND** command token is `/search`
- **AND** arguments are `quantum computing`

#### Scenario: Missing whitespace is rejected

- **GIVEN** user input is `/searchquery`
- **WHEN** Router validates command format
- **THEN** validation fails with `ValidationError::MissingSpace`
- **AND** error message is "Add space after command: /search <your query>"

#### Scenario: Non-command input is allowed

- **GIVEN** user input is `What is quantum computing?`
- **WHEN** Router validates command format
- **THEN** validation passes
- **AND** input is treated as general chat

---

### Requirement: Command tokens enable precise matching

The Router SHALL use token-based matching to distinguish between similar command prefixes.

**Rationale**: 使用户能够定义短命令（如 `/se` 用于 "summarize"）而不会与系统命令 `/search` 冲突

**Priority**: Must Have

#### Scenario: Short custom command does not match long builtin

- **GIVEN** routing rules include `/search` (builtin) and `/se` (custom)
- **WHEN** user input is `/se analyze this code`
- **THEN** Router matches custom `/se` rule
- **AND** does NOT match `/search` rule
- **AND** command prefix `/se` is stripped correctly

#### Scenario: Exact command match takes precedence

- **GIVEN** routing rules include `/s` and `/search`
- **WHEN** user input is `/s hello world`
- **THEN** Router matches `/s` rule (first match)
- **AND** does NOT continue to check `/search`

---

### Requirement: Validation errors provide actionable hints

The Router SHALL return structured error information with user-friendly correction suggestions.

**Rationale**: 帮助用户快速修正命令格式错误，无需查阅文档

**Priority**: Should Have

#### Scenario: Error includes detected command

- **GIVEN** user input is `/translatehello`
- **WHEN** validation fails
- **THEN** error contains detected command `/translate`
- **AND** suggestion is `/translate hello`

#### Scenario: Error distinguishes unknown commands

- **GIVEN** user input is `/unknowncommand test`
- **WHEN** no routing rule matches
- **THEN** Router falls back to default provider
- **AND** no validation error is raised (command is valid format)

---

## MODIFIED Requirements

### Requirement (Modified): RoutingRule strips prefix after validation

**Original**: `strip_matched_prefix()` strips regex match directly
**Modified**: Only strip after whitespace validation passes

**Changes**:
- Add pre-validation step before `strip_matched_prefix()`
- Ensure stripped result doesn't include command token
- Preserve behavior for non-command patterns (regex without `^/`)

**Example**:
```rust
// Before
input: "/search query"
rule.strip_matched_prefix(input) → "query"  // Direct strip

// After
input: "/search query"
Router::validate_command_format(input) → Ok(())  // Check space first
rule.strip_matched_prefix(input) → "query"       // Then strip
```

---

## REMOVED Requirements

*(None)*

---

## Validation Criteria

- [ ] All slash commands require space after prefix
- [ ] Token-based matching prevents prefix conflicts
- [ ] Validation errors include actionable suggestions
- [ ] Non-command input passes validation
- [ ] Existing routing behavior is preserved
