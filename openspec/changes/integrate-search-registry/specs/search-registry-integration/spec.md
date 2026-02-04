# Search Registry Integration Spec

**Capability**: Persistent SearchRegistry in AlephCore with hot-reload support

**Relates to**: `search-capability`, `config-management`, `ai-routing`

---

## ADDED Requirements

### Requirement: AlephCore maintains persistent SearchRegistry

The AlephCore SHALL maintain a persistent SearchRegistry instance initialized from configuration.

**Rationale**: 避免每次请求重复创建 SearchRegistry 和 providers，提供 provider 测试 API 访问

**Priority**: Must Have

#### Scenario: SearchRegistry initializes from config

- **GIVEN** config has `[search]` section with `enabled = true`
- **WHEN** AlephCore is constructed
- **THEN** SearchRegistry is created from search config
- **AND** all configured backends are initialized as providers
- **AND** default provider is set from `search.default_provider`
- **AND** fallback chain is set from `search.fallback_providers`

#### Scenario: SearchRegistry is None when search disabled

- **GIVEN** config has `[search]` section with `enabled = false`
- **WHEN** AlephCore is constructed
- **THEN** search_registry field is `None`
- **AND** no providers are initialized

#### Scenario: SearchRegistry handles missing config

- **GIVEN** config has no `[search]` section
- **WHEN** AlephCore is constructed
- **THEN** search_registry field is `None`
- **AND** no errors are raised

---

### Requirement: AlephCore provides provider testing API

The AlephCore SHALL expose an async method to test search provider connections via UniFFI.

**Rationale**: 允许 Swift UI 验证 provider 配置并显示状态

**Priority**: Must Have

#### Scenario: test_search_provider() succeeds with valid provider

- **GIVEN** AlephCore has initialized SearchRegistry
- **WHEN** Swift calls `await core.testSearchProvider("tavily")`
- **THEN** SearchRegistry.test_search_provider() is called
- **AND** result contains success=true and latency_ms > 0
- **AND** result is cached for 5 minutes

#### Scenario: test_search_provider() fails when search disabled

- **GIVEN** AlephCore has search_registry = None
- **WHEN** Swift calls `await core.testSearchProvider("tavily")`
- **THEN** result contains success=false
- **AND** error_type = "config"
- **AND** error_message = "Search capability not enabled"

#### Scenario: test_search_provider() returns cached result

- **GIVEN** provider was tested less than 5 minutes ago
- **WHEN** Swift calls `await core.testSearchProvider("tavily")` again
- **THEN** cached result is returned immediately
- **AND** no actual provider query is executed

---

### Requirement: SearchRegistry supports config hot-reload

The AlephCore SHALL rebuild SearchRegistry when search configuration changes.

**Rationale**: 配置热重载时更新 provider 列表，无需重启应用

**Priority**: Should Have

#### Scenario: Config change triggers registry rebuild

- **GIVEN** AlephCore is running with SearchRegistry
- **WHEN** config watcher detects search config change
- **AND** `on_config_changed()` callback is triggered
- **THEN** SearchRegistry is rebuilt from new config
- **AND** old registry is replaced with write lock
- **AND** log event is emitted

---

## MODIFIED Requirements

### Requirement (Modified): CapabilityExecutor receives SearchRegistry

**Original**: CapabilityExecutor receives `search_registry: None` (TODO)
**Modified**: CapabilityExecutor receives shared reference to persistent SearchRegistry

**Changes**:
```rust
// Before (core.rs:354)
let executor = CapabilityExecutor::new(
    self.memory_db.as_ref().map(Arc::clone),
    Some(Arc::new(cfg.memory.clone())),
    None, // TODO: Add SearchRegistry from config
    None, // TODO: Add SearchOptions from config
    pii_enabled,
);

// After
let executor = CapabilityExecutor::new(
    self.memory_db.as_ref().map(Arc::clone),
    Some(Arc::new(cfg.memory.clone())),
    self.search_registry.read().unwrap().as_ref().map(Arc::clone),
    Some(self.get_search_options_from_config()),
    pii_enabled,
);
```

---

## REMOVED Requirements

*(None)*

---

## Validation Criteria

- [ ] AlephCore initializes SearchRegistry from config
- [ ] `test_search_provider()` works from Swift
- [ ] Provider test results are cached correctly
- [ ] Config hot-reload rebuilds SearchRegistry
- [ ] Search disabled case is handled gracefully
