# ai-routing Specification Delta

## MODIFIED Requirements

### Requirement: Set default provider
The router SHALL validate and use the default provider with fallback support for disabled or missing providers.

- **WHEN** config includes `default_provider = "openai"`
- **THEN** default provider SHALL be stored in router
- **AND** default provider MUST be enabled (`enabled = true` in config)
- **AND** default provider MUST exist in providers map
- **AND** if default provider is disabled, router SHALL use first enabled provider as fallback
- **AND** if default provider is missing, router SHALL log warning and use first enabled provider
- **AND** missing default provider is allowed (returns None on no match)

#### Scenario: Fallback when default provider is disabled
- **GIVEN** config has `default_provider = "claude"`
- **AND** "claude" provider has `enabled = false`
- **WHEN** Router is initialized
- **THEN** the router SHALL:
  - Log warning: "Default provider 'claude' is disabled, using fallback"
  - Select the first provider from providers map where `enabled = true`
  - Use that provider as the effective default
  - NOT clear `general.default_provider` from config (user preference preserved)

#### Scenario: Fallback when default provider is deleted
- **GIVEN** config has `default_provider = "nonexistent"`
- **AND** "nonexistent" does not exist in `providers` section
- **WHEN** Router is initialized
- **THEN** the router SHALL:
  - Log warning: "Default provider 'nonexistent' not found in config"
  - Select the first enabled provider as fallback
  - Suggest clearing `general.default_provider` in config
  - Continue routing with fallback provider

#### Scenario: Validate default provider on config reload
- **GIVEN** the config file is modified externally
- **AND** the app detects the change via ConfigWatcher
- **WHEN** Router is reloaded with new config
- **THEN** the router SHALL:
  - Re-validate that default_provider exists and is enabled
  - Update routing behavior to use new default
  - Log info: "Default provider updated to: <provider_name>"

### Requirement: No matching rule
The router SHALL return the default provider when no rule matches, or provide a clear error if no providers are available.

- **WHEN** no rule matches the input
- **AND** a valid enabled default provider is configured
- **THEN** `route()` SHALL return the default provider with `None` system prompt
- **AND** if no enabled default provider exists
- **THEN** `route()` SHALL return `None`
- **AND** caller SHALL receive `AetherError::NoProviderAvailable`
- **AND** error message SHALL suggest enabling a provider in Settings

#### Scenario: Improved error message when no providers enabled
- **GIVEN** all providers are disabled
- **WHEN** user triggers hotkey with input "Hello world"
- **AND** no rule matches
- **AND** no enabled default provider exists
- **THEN** the system SHALL:
  - Return `AetherError::NoProviderAvailable`
  - Error message: "No active providers available. Please enable at least one provider in Settings."
  - NOT attempt to route to disabled providers
  - Display error to user via Halo or notification
