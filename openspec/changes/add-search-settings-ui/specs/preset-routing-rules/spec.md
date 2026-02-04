# Preset Routing Rules Spec

**Capability**: Default routing rules for builtin slash commands

**Relates to**: `ai-routing`, `search-capability`, `skills`, `mcp-integration`

---

## ADDED Requirements

### Requirement: Default config includes builtin slash commands

The system SHALL provide default routing rules for `/search`, `/mcp`, and `/skill` commands in the configuration template.

**Rationale**: 新用户开箱即用，无需手动配置即可使用内置功能

**Priority**: Must Have

#### Scenario: /search rule is preconfigured

- **GIVEN** user installs Aleph for the first time
- **WHEN** default `config.toml` is generated
- **THEN** routing rules include `/search` pattern
- **AND** rule has `intent_type = "builtin_search"`
- **AND** rule has `strip_prefix = true`
- **AND** rule has `capabilities = ["search"]`

#### Scenario: /mcp rule is preconfigured

- **GIVEN** default config is loaded
- **WHEN** user inspects routing rules
- **THEN** rules include `/mcp` pattern (reserved for future)
- **AND** rule has `intent_type = "builtin_mcp"`
- **AND** rule has placeholder system prompt

#### Scenario: /skill rule is preconfigured

- **GIVEN** default config is loaded
- **WHEN** user inspects routing rules
- **THEN** rules include `/skill` pattern (reserved for future)
- **AND** rule has `intent_type = "skills"`
- **AND** rule has `strip_prefix = true`

---

### Requirement: Preset rules match whitespace-enforced format

Default routing rules SHALL use patterns that enforce the whitespace requirement for slash commands.

**Rationale**: 保证预设规则符合命令验证要求，作为用户配置的参考示例

**Priority**: Must Have

#### Scenario: Preset patterns require space

- **GIVEN** default `/search` rule
- **WHEN** pattern is inspected
- **THEN** regex is `^/search\\s+` (enforces space)
- **OR** validation logic is applied separately
- **AND** pattern does NOT match `/searchquery` (no space)

#### Scenario: Preset rules demonstrate best practices

- **GIVEN** user views default `config.toml`
- **WHEN** reading routing rules section
- **THEN** comments explain whitespace requirement
- **AND** examples show correct format: `/search <query>`

---

### Requirement: Builtin rules do not conflict with custom rules

The Router SHALL allow users to override builtin rules with custom configurations without breaking system functionality.

**Rationale**: 高级用户可能想自定义 `/search` 行为（如使用不同的 provider），系统应允许覆盖

**Priority**: Should Have

#### Scenario: User can override /search provider

- **GIVEN** user adds custom rule `^/search` with `provider = "claude"`
- **WHEN** config is loaded
- **THEN** custom rule takes precedence (first-match wins)
- **AND** search capability is still invoked
- **AND** Claude is used instead of default OpenAI

#### Scenario: User can add similar command without conflict

- **GIVEN** user adds custom rule `^/se` for "summarize" intent
- **WHEN** user types `/se analyze this`
- **THEN** custom `/se` rule matches (token-based)
- **AND** builtin `/search` does NOT match
- **AND** no validation error occurs

---

## MODIFIED Requirements

### Requirement (Modified): Config::default() includes builtin rules

**Original**: `Config::default()` has empty `rules: Vec::new()`
**Modified**: Include 3 preset rules for `/search`, `/mcp`, `/skill`

**Changes**:
```rust
impl Default for Config {
    fn default() -> Self {
        Self {
            // ... other fields ...
            rules: vec![
                RoutingRuleConfig {
                    regex: r"^/search\s+".to_string(),
                    provider: "openai".to_string(),
                    system_prompt: Some("You are a helpful search assistant.".to_string()),
                    strip_prefix: Some(true),
                    capabilities: Some(vec!["search".to_string()]),
                    intent_type: Some("builtin_search".to_string()),
                    // ... other fields None ...
                },
                // /mcp and /skill rules...
            ],
            // ... other fields ...
        }
    }
}
```

---

## REMOVED Requirements

*(None)*

---

## Validation Criteria

- [ ] Default config includes `/search`, `/mcp`, `/skill` rules
- [ ] Preset patterns enforce whitespace requirement
- [ ] Users can override builtin rules without breaking functionality
- [ ] Config comments document command format
- [ ] Preset rules demonstrate best practices
