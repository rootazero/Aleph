# Implementation Tasks

## Phase 1: SearchRegistry Integration (4h)

### Task 1.1: Add SearchRegistry field to AlephCore (1h)
- [ ] Add `search_registry: Arc<RwLock<Option<Arc<SearchRegistry>>>>` field
- [ ] Update constructor to initialize from config
- [ ] Handle config.search = None case
- [ ] Add helper method `create_search_registry_from_config()`

**Validation**: AlephCore compiles with new field

---

### Task 1.2: Implement search registry initialization (1.5h)
- [ ] Create `create_search_registry_from_config()` method
  - Parse `SearchConfig` from config
  - Create providers for each backend
  - Set default provider and fallback chain
- [ ] Call from constructor if search enabled
- [ ] Handle provider creation errors gracefully

**Validation**: SearchRegistry initializes successfully with valid config

---

### Task 1.3: Implement test_search_provider() in AlephCore (1h)
- [ ] Add async method `pub async fn test_search_provider(&self, provider_name: String) -> ProviderTestResult`
- [ ] Acquire read lock on search_registry
- [ ] Delegate to `registry.test_search_provider(name).await`
- [ ] Handle case where search is disabled
- [ ] Uncomment UniFFI method declaration in aleph.udl

**Validation**: Method compiles and returns ProviderTestResult

---

### Task 1.4: Config hot-reload for SearchRegistry (0.5h)
- [ ] Update `on_config_changed()` callback handler
- [ ] Rebuild SearchRegistry when search config changes
- [ ] Acquire write lock to replace old registry
- [ ] Log registry reload event

**Validation**: Config changes trigger registry rebuild

---

## Phase 2: SearchOptions Configuration (1.5h)

### Task 2.1: Extract SearchOptions from config (0.5h)
- [ ] Add helper method `get_search_options_from_config()`
- [ ] Read `max_results` from `search.max_results`
- [ ] Read `timeout_seconds` from `search.timeout_seconds`
- [ ] Return default `SearchOptions` if search disabled

**Validation**: SearchOptions correctly extracted from config

---

### Task 2.2: Pass SearchOptions to CapabilityExecutor (0.5h)
- [ ] Update `CapabilityExecutor::new()` call in `build_enriched_payload()`
- [ ] Replace `None` with `Some(search_options)`
- [ ] Update line 355 comment to remove TODO

**Validation**: CapabilityExecutor receives SearchOptions from config

---

### Task 2.3: Update CapabilityExecutor to use SearchOptions (0.5h)
- [ ] Update `CapabilityExecutor` constructor signature
- [ ] Change `search_options: SearchOptions` parameter from `Option` to direct value
- [ ] Use provided options in search capability execution

**Validation**: Search capability uses configured options

---

## Phase 3: PII Configuration Migration (2.5h)

### Task 3.1: Add PIIConfig to SearchConfig (1h)
- [ ] Create `PIIConfig` struct in `config.rs`:
  ```rust
  pub struct PIIConfig {
      pub enabled: bool,
      pub scrub_email: bool,
      pub scrub_phone: bool,
      pub scrub_ssn: bool,
      pub scrub_credit_card: bool,
  }
  ```
- [ ] Add `pub pii: PIIConfig` field to `SearchConfig`
- [ ] Add `pub pii: Option<PIIConfig>` to `SearchConfigInternal`
- [ ] Implement `Default` for `PIIConfig`

**Validation**: Struct compiles and serializes correctly

---

### Task 3.2: Implement config migration logic (1h)
- [ ] Add `migrate_pii_config()` method to `Config`
- [ ] Detect `behavior.pii_scrubbing_enabled` field
- [ ] Create `search.pii` if missing
- [ ] Copy value to `search.pii.enabled`
- [ ] Remove old field from behavior config
- [ ] Auto-save migrated config

**Validation**: Old configs migrate automatically on load

---

### Task 3.3: Update PII scrubbing to use new config (0.5h)
- [ ] Update `CapabilityExecutor` initialization
- [ ] Read from `search.pii.enabled` instead of `behavior.pii_scrubbing_enabled`
- [ ] Update line 358-362 in `core.rs`
- [ ] Maintain backward compatibility (fallback to behavior if search missing)

**Validation**: PII scrubbing uses new config location

---

## Phase 4: Swift Bindings & Testing (2h)

### Task 4.1: Regenerate Swift bindings (0.5h)
- [ ] Run `cargo build --release` to rebuild Rust library
- [ ] Run `uniffi-bindgen generate` to regenerate Swift bindings
- [ ] Copy updated `aleph.swift` to Swift source directory
- [ ] Copy `libalephcore.dylib` to Frameworks directory

**Validation**: Swift project compiles with new bindings

---

### Task 4.2: Unit tests for SearchRegistry integration (1h)
- [ ] Test SearchRegistry initialization from config
- [ ] Test `test_search_provider()` with mock provider
- [ ] Test hot-reload mechanism
- [ ] Test PII config migration

**Validation**: All tests pass

---

### Task 4.3: Manual testing (0.5h)
- [ ] Test with real search config (Tavily API key)
- [ ] Call `test_search_provider("tavily")` from Swift
- [ ] Verify latency measurement
- [ ] Test error cases (invalid key, network error)

**Validation**: Provider testing works end-to-end

---

## Total Estimated Time: ~10 hours

**Phase Breakdown**:
- Phase 1: SearchRegistry Integration (4h)
- Phase 2: SearchOptions Configuration (1.5h)
- Phase 3: PII Configuration Migration (2.5h)
- Phase 4: Swift Bindings & Testing (2h)

**Dependencies**:
- Phase 2 can run in parallel with Phase 1
- Phase 3 is independent, can start anytime
- Phase 4 requires all previous phases to complete
