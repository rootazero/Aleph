# Search Settings UI Spec

**Capability**: Graphical interface for search provider configuration

**Relates to**: `settings-ui-layout`, `ai-routing`

---

## ADDED Requirements

### Requirement: Settings UI provides Search configuration page

The Settings UI SHALL provide a dedicated Search tab for graphical configuration of search providers.

**Rationale**: 使用户能够通过 GUI 配置搜索供应商，无需手动编辑 TOML

**Priority**: Must Have

#### Scenario: Search settings tab is accessible

- **GIVEN** user opens Settings window
- **WHEN** user navigates to tabs
- **THEN** "Search" tab is visible
- **AND** tab contains provider configuration cards

#### Scenario: Provider card displays configuration fields

- **GIVEN** user opens Tavily provider card
- **WHEN** card expands
- **THEN** API Key field is shown (SecureField)
- **AND** Search Depth picker is shown
- **AND** Test Connection button is enabled

#### Scenario: Test connection validates provider config

- **GIVEN** user enters valid Tavily API key
- **WHEN** user clicks "Test Connection"
- **THEN** provider status changes to "Testing"
- **AND** after 2-3 seconds, status shows "✅ Available"
- **AND** success toast displays latency

---

### Requirement: All 6 search providers have preset templates

The system SHALL provide preset configuration templates for all 6 supported search providers (Tavily, SearXNG, Google, Bing, Brave, Exa).

**Rationale**: 为每个供应商提供正确的默认配置，减少用户配置错误

**Priority**: Must Have

#### Scenario: Preset templates define required fields

- **GIVEN** Tavily preset template
- **WHEN** template is loaded
- **THEN** API Key field is defined (required)
- **AND** Search Depth field has options: ["basic", "advanced"]
- **AND** Documentation URL points to tavily.com/docs

#### Scenario: SearXNG template includes base URL

- **GIVEN** SearXNG preset
- **WHEN** user configures SearXNG
- **THEN** Instance URL field is shown (not API key)
- **AND** Default value is "https://searx.be"

---

### Requirement: Provider status is displayed and updated

The UI SHALL display real-time status indicators for each search provider showing configuration and availability state.

**Rationale**: 用户需要知道哪些供应商已配置、哪些可用

**Priority**: Must Have

#### Scenario: Unconfigured provider shows warning

- **GIVEN** Bing provider has no API key
- **WHEN** settings view loads
- **THEN** Bing card shows "⚠️ Not Configured" status
- **AND** Test Connection button is disabled

#### Scenario: Configured provider shows status

- **GIVEN** Tavily has valid API key
- **WHEN** settings view loads
- **THEN** provider status is checked automatically
- **AND** status shows "✅ Available" or "❌ Offline"

---

## MODIFIED Requirements

### Requirement (Modified): Config includes search UI preferences

**Original**: SearchConfig only has backend configs
**Modified**: Add UI state like fallback_order, enabled flags

**Changes**:
```toml
[search]
default_provider = "tavily"
fallback_providers = ["searxng", "google"]  # NEW: ordered list
enabled_builtins = ["search"]               # NEW: which builtins enabled
```

---

## REMOVED Requirements

*(None)*

---

## Validation Criteria

- [ ] All 6 providers render correctly
- [ ] Test connection works for each provider
- [ ] Config saves and loads correctly
- [ ] Status indicators update in real-time
- [ ] Documentation links open correctly
