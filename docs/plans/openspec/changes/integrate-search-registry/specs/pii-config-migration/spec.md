# PII Config Migration Spec

**Capability**: Automatic migration of PII settings from Behavior to Search configuration

**Relates to**: `config-management`, `search-capability`, `privacy`

---

## ADDED Requirements

### Requirement: SearchConfig includes PII settings

The SearchConfig SHALL include a dedicated PII configuration section for privacy-related settings.

**Rationale**: PII 过滤主要用于搜索场景，配置应与搜索功能聚合

**Priority**: Must Have

#### Scenario: PIIConfig struct defines scrubbing options

- **GIVEN** SearchConfig is being constructed
- **WHEN** PIIConfig is initialized
- **THEN** struct includes `enabled: bool` field
- **AND** struct includes scrubbing flags for email, phone, SSN, credit card

#### Scenario: PIIConfig serializes to TOML correctly

- **GIVEN** SearchConfig with PII settings
- **WHEN** config is saved to TOML
- **THEN** section appears as `[search.pii]`
- **AND** all boolean flags are preserved

---

### Requirement: Config automatically migrates PII settings

The Config loader SHALL automatically migrate old PII settings from behavior to search section.

**Rationale**: 保证现有用户升级时不丢失 PII 配置

**Priority**: Must Have

#### Scenario: Migration detects old PII config

- **GIVEN** config TOML has `[behavior]` with `pii_scrubbing_enabled = true`
- **AND** config TOML has NO `[search.pii]` section
- **WHEN** Config::load() is called
- **THEN** migration logic is triggered

#### Scenario: Migration creates search.pii from behavior

- **GIVEN** migration is triggered
- **WHEN** migrating PII settings
- **THEN** `search.pii.enabled` is set from `behavior.pii_scrubbing_enabled`
- **AND** old `behavior.pii_scrubbing_enabled` field is removed
- **AND** config is auto-saved to disk

#### Scenario: Migration is idempotent

- **GIVEN** config already has `[search.pii]` section
- **WHEN** Config::load() is called
- **THEN** no migration occurs
- **AND** existing `search.pii` values are preserved

---

### Requirement: PII scrubbing uses new config location

The system SHALL read PII settings from search.pii instead of behavior.pii_scrubbing_enabled.

**Rationale**: 统一配置位置，符合功能分组原则

**Priority**: Must Have

#### Scenario: CapabilityExecutor reads from search.pii

- **GIVEN** config has `[search.pii]` with `enabled = true`
- **WHEN** CapabilityExecutor is initialized
- **THEN** pii_scrubbing_enabled is set from `search.pii.enabled`
- **AND** NOT from `behavior.pii_scrubbing_enabled`

#### Scenario: Backward compatibility with old configs

- **GIVEN** config has `behavior.pii_scrubbing_enabled = true`
- **AND** config has NO `[search.pii]` section
- **WHEN** CapabilityExecutor is initialized
- **THEN** migration runs first
- **AND** pii_scrubbing_enabled is read from migrated `search.pii.enabled`

---

## MODIFIED Requirements

### Requirement (Modified): BehaviorConfig no longer includes PII field

**Original**: `BehaviorConfig` has `pii_scrubbing_enabled: bool` field
**Modified**: Field is removed after migration

**Changes**:
```rust
// Before
pub struct BehaviorConfig {
    pub input_mode: String,
    pub output_mode: String,
    pub typing_speed: u32,
    pub pii_scrubbing_enabled: bool,  // REMOVE THIS
}

// After
pub struct BehaviorConfig {
    pub input_mode: String,
    pub output_mode: String,
    pub typing_speed: u32,
    // pii_scrubbing_enabled removed
}
```

**Impact**: Breaking change for old configs, but migration handles it automatically

---

## REMOVED Requirements

*(None)*

---

## Validation Criteria

- [ ] PIIConfig struct compiles and serializes correctly
- [ ] Migration logic runs on old configs
- [ ] Migrated configs save to disk automatically
- [ ] PII scrubbing uses new config location
- [ ] Backward compatibility maintained during migration
- [ ] Idempotent migration (no double-migration)
