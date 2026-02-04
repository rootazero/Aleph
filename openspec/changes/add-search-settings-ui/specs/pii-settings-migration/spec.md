# PII Settings Migration Spec

**Capability**: Move PII scrubbing configuration from Behavior settings to Search settings

**Relates to**: `settings-ui-layout`, `search-settings-ui`, `behavior-config`

---

## ADDED Requirements

### Requirement: Search settings include PII scrubbing section

The Search Settings view SHALL include PII scrubbing configuration previously located in Behavior settings.

**Rationale**: PII 过滤主要用于搜索场景（防止搜索结果泄露敏感信息），放在 Search 设置更符合逻辑分组

**Priority**: Must Have

#### Scenario: PII section appears in Search settings

- **GIVEN** user opens Search settings tab
- **WHEN** view loads
- **THEN** "Privacy & PII Scrubbing" section is visible
- **AND** section includes enable toggle
- **AND** section includes PII type checkboxes (email, phone, SSN, credit card)

#### Scenario: PII settings use same UI components

- **GIVEN** PII UI is moved to Search tab
- **WHEN** rendering PII section
- **THEN** uses existing `PIIType` enum
- **AND** uses existing toggle/checkbox patterns
- **AND** maintains visual consistency with other sections

---

### Requirement: BehaviorSettingsView no longer shows PII options

The Behavior Settings view SHALL remove PII scrubbing UI after migration.

**Rationale**: 避免配置重复，减少用户困惑

**Priority**: Must Have

#### Scenario: PII section is removed from Behavior tab

- **GIVEN** user opens Behavior settings tab
- **WHEN** view loads
- **THEN** PII scrubbing card is NOT rendered
- **AND** only Input Mode, Output Mode, Typing Speed cards are shown

#### Scenario: BehaviorConfig no longer includes PII field

- **GIVEN** `BehaviorConfig` struct in Rust
- **WHEN** config is serialized to TOML
- **THEN** `pii_scrubbing_enabled` field is NOT present
- **AND** PII config is in `[search.pii]` section instead

---

### Requirement: Configuration is migrated on first load

The system SHALL automatically migrate existing PII settings from `[behavior]` to `[search.pii]` section on config load.

**Rationale**: 保证现有用户的 PII 配置不丢失，无缝升级

**Priority**: Must Have

#### Scenario: Existing PII config is migrated

- **GIVEN** user has `pii_scrubbing_enabled = true` in `[behavior]` section
- **WHEN** Aleph loads config for the first time after update
- **THEN** config migration logic runs
- **AND** value is moved to `[search.pii.enabled]`
- **AND** old `[behavior.pii_scrubbing_enabled]` is removed
- **AND** config is automatically saved

#### Scenario: Migration is idempotent

- **GIVEN** PII config already exists in `[search.pii]`
- **WHEN** config is loaded again
- **THEN** no migration occurs
- **AND** existing `[search.pii]` values are preserved

---

## MODIFIED Requirements

### Requirement (Modified): SearchConfig includes PII settings

**Original**: `SearchConfig` only has provider and timeout settings
**Modified**: Add `pii` subsection for scrubbing configuration

**Changes**:
```toml
[search]
enabled = true
default_provider = "tavily"

[search.pii]
enabled = false
scrub_email = false
scrub_phone = false
scrub_ssn = false
scrub_credit_card = false
```

**Rust**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    pub enabled: bool,
    pub default_provider: String,
    // ... existing fields ...
    #[serde(default)]
    pub pii: PIIConfig,  // NEW
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PIIConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub scrub_email: bool,
    #[serde(default)]
    pub scrub_phone: bool,
    #[serde(default)]
    pub scrub_ssn: bool,
    #[serde(default)]
    pub scrub_credit_card: bool,
}
```

---

### Requirement (Modified): Localization keys are renamed

**Original**: `settings.behavior.pii_*` keys
**Modified**: `settings.search.pii_*` keys

**Changes**:
- `settings.behavior.pii_scrubbing` → `settings.search.pii_scrubbing`
- `settings.behavior.pii_scrubbing_enable` → `settings.search.pii_scrubbing_enable`
- `settings.behavior.pii_scrubbing_description` → `settings.search.pii_scrubbing_description`
- (and all `pii_type_*` and `pii_example_*` keys)

**Migration**: Keep old keys as fallback for 1 release to avoid missing translations

---

## REMOVED Requirements

### Requirement (Removed): BehaviorConfig includes PII settings

**Removed**: `pii_scrubbing_enabled` field from `BehaviorConfig`

**Rationale**: Moved to `SearchConfig::pii` instead

**Impact**: Breaking change, requires config migration

---

## Validation Criteria

- [ ] PII settings appear in Search tab
- [ ] PII settings removed from Behavior tab
- [ ] Config migration runs automatically on first load
- [ ] Existing PII values are preserved after migration
- [ ] Localization keys updated without missing strings
- [ ] BehaviorConfig no longer has PII field
