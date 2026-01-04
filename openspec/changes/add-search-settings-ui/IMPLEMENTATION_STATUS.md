# Implementation Status: Add Search Settings UI

## Summary

This document tracks the implementation status of the `add-search-settings-ui` proposal.

**Current Status**: Partially Implemented (Phase 7 completed)

**Last Updated**: 2026-01-04

---

## Completed Phases

### ✅ Phase 7: Preset Routing Rules (Completed)

**Tasks Completed**:
- [x] Added preset routing rules to `Config::default()` for:
  - `/search` command (web search capability)
  - `/mcp` command (Model Context Protocol - reserved)
  - `/skill` command (Skills workflows - reserved)
- [x] All rules include proper configuration:
  - `regex` pattern with `\s+` (whitespace enforcement)
  - `strip_prefix = true` (removes command prefix)
  - `capabilities` array for search capability
  - `intent_type` for logging and routing
  - `context_format = "markdown"` for prompt assembly

**Files Modified**:
- `Aether/core/src/config/mod.rs` - Updated `Config::default()` to include 3 preset rules

**Commit**: bc20fa4

---

## Partially Completed Phases

### ⚠️ Phase 1: Rust Core - Provider Testing API (Partially Completed)

**Completed**:
- [x] Created `ProviderTestResult` struct in `Aether/core/src/search/mod.rs`
- [x] Implemented `test_search_provider()` method in `SearchRegistry`
  - [x] Minimal test query execution (`"test"` with `max_results = 1`)
  - [x] Latency measurement with `Instant::now()`
  - [x] 5-minute TTL caching to avoid API quota abuse
  - [x] Error classification (auth, network, config)
- [x] Exported `ProviderTestResult` via UniFFI (dictionary definition in `.udl`)

**Pending**:
- [ ] Integrate `SearchRegistry` into `AetherCore`
  - Currently `SearchRegistry` is created temporarily in capability executor
  - Need to add as persistent field in `AetherCore` struct
- [ ] Implement `test_search_provider()` method in `AetherCore`
  - Requires access to `SearchRegistry` instance
  - Should delegate to `registry.test_search_provider(name).await`
- [ ] Regenerate Swift bindings with `uniffi-bindgen generate`
- [ ] Unit tests for provider testing logic

**Blockers**:
- `SearchRegistry` is not yet integrated into `AetherCore` as a persistent field
- Current architecture creates `SearchRegistry` on-demand in capability executor
- Refactoring needed to store `SearchRegistry` in `AetherCore` for testing access

**Files Modified**:
- `Aether/core/src/search/mod.rs` - Added `ProviderTestResult` struct
- `Aether/core/src/search/registry.rs` - Implemented `test_search_provider()`
- `Aether/core/src/aether.udl` - Added `ProviderTestResult` dictionary (commented out method)
- `Aether/core/src/lib.rs` - Exported `ProviderTestResult`

---

## Pending Phases

### 📋 Phase 2: Rust Core - Command Validation

**Status**: Not Started

**Required Work**:
- [ ] Add `ValidationError` type to `error.rs`
- [ ] Implement `validate_command_format()` in `Router`
  - Parse slash commands with whitespace requirement
  - Return `ValidationError::MissingSpace` if no space after command
- [ ] Add `on_validation_hint()` callback to `AetherEventHandler` in `.udl`
- [ ] Integrate validation into `AetherCore::process_input()`
- [ ] Unit tests for command validation logic

**Estimated Time**: 4 hours

---

### 📋 Phase 3: Swift UI - Provider Presets

**Status**: Not Started

**Required Work**:
- [ ] Create `SearchProviderPreset.swift` model
- [ ] Define `PresetField` enum (SecureText, Text, Picker)
- [ ] Create preset array with 6 providers:
  - Tavily (API Key, Search Depth)
  - SearXNG (Instance URL)
  - Google (API Key, Engine ID)
  - Bing (API Key)
  - Brave (API Key)
  - Exa (API Key)
- [ ] Add documentation URLs for each provider

**Estimated Time**: 3 hours

---

### 📋 Phase 4: Swift UI - Components

**Status**: Not Started

**Required Work**:
- [ ] Create `ProviderCard.swift` component
  - Header: Icon, Name, Status Badge
  - Body: Dynamic fields from preset
  - Footer: Test button, Docs link
- [ ] Implement `testConnection()` async function
  - Call `core.testSearchProvider(name)`
  - Update status badge based on result
  - Show toast notification
- [ ] Create `SearchSettingsView.swift`
  - Provider configuration section (6 cards)
  - Fallback order section (placeholder)
  - PII scrubbing section (migrated from Behavior)
  - Integrate `UnifiedSaveBar`

**Estimated Time**: 8 hours

**Dependencies**: Phase 1 must be completed (test_search_provider API)

---

### 📋 Phase 5: Configuration & Migration

**Status**: Not Started

**Required Work**:
- [ ] Add `PIIConfig` struct to `config.rs`
- [ ] Add `pii: PIIConfig` field to `SearchConfig`
- [ ] Implement config migration in `Config::load()`
  - Detect `behavior.pii_scrubbing_enabled`
  - Migrate to `search.pii.enabled`
  - Save migrated config
- [ ] Move PII UI from `BehaviorSettingsView` to `SearchSettingsView`
- [ ] Update localization keys: `settings.behavior.pii_*` → `settings.search.pii_*`

**Estimated Time**: 4 hours

---

### 📋 Phase 6: Halo Validation Hints UI

**Status**: Not Started

**Required Work**:
- [ ] Add `ValidationHint` state to `HaloView.swift`
- [ ] Add amber border color for validation hints
- [ ] Implement 2-second auto-dismiss timer
- [ ] Implement `on_validation_hint()` callback in Swift
  - Show Halo with amber border
  - Display validation message
  - Auto-dismiss after timeout

**Estimated Time**: 2 hours

**Dependencies**: Phase 2 must be completed (validation callback)

---

### 📋 Phase 8: Testing & Documentation

**Status**: Not Started

**Required Work**:
- [ ] Rust unit tests:
  - Command validation tests
  - Provider testing tests
  - Config migration tests
- [ ] Manual testing with all 6 search providers
- [ ] Update documentation:
  - `docs/ARCHITECTURE.md` - Add Search Settings UI section
  - `CLAUDE.md` - Add command validation rules
  - `docs/ui-design-guide.md` - Add screenshots

**Estimated Time**: 6 hours

---

## Architecture Notes

### SearchRegistry Integration

**Current Architecture**:
```rust
// AetherCore does NOT have SearchRegistry as field
pub struct AetherCore {
    event_handler: Arc<dyn AetherEventHandler>,
    runtime: Arc<Runtime>,
    config: Arc<Mutex<Config>>,
    memory_db: Option<Arc<VectorDatabase>>,
    router: Arc<RwLock<Option<Arc<Router>>>>,
    // SearchRegistry missing!
}

// SearchRegistry created on-demand in capability executor
impl CapabilityExecutor {
    pub fn execute_search(&self, query: &str) -> Result<String> {
        let registry = Self::create_search_registry_from_config(config)?;
        registry.search(query, &options).await?
    }
}
```

**Required Refactoring**:
```rust
pub struct AetherCore {
    // ... existing fields ...
    search_registry: Arc<RwLock<Option<Arc<SearchRegistry>>>>, // ADD THIS
}

impl AetherCore {
    pub fn new(event_handler: Box<dyn AetherEventHandler>) -> Result<Self> {
        // ... existing code ...

        // Initialize SearchRegistry from config
        let search_registry = {
            let cfg = config.lock().unwrap();
            if let Some(search_config) = &cfg.search {
                if search_config.enabled {
                    Some(Arc::new(Self::create_search_registry(search_config)?))
                } else {
                    None
                }
            } else {
                None
            }
        };

        Ok(Self {
            // ... existing fields ...
            search_registry: Arc::new(RwLock::new(search_registry)),
        })
    }

    // NEW: Async method for provider testing
    pub async fn test_search_provider(&self, provider_name: String) -> ProviderTestResult {
        let registry = self.search_registry.read().unwrap();
        match registry.as_ref() {
            Some(reg) => reg.test_search_provider(&provider_name).await,
            None => ProviderTestResult {
                success: false,
                latency_ms: 0,
                error_message: "Search capability not enabled".to_string(),
                error_type: "config".to_string(),
            }
        }
    }
}
```

---

## Next Steps

### Immediate (for next session):

1. **Refactor AetherCore to include SearchRegistry**:
   - Add `search_registry` field to struct
   - Initialize from config in constructor
   - Implement `test_search_provider()` method
   - Update `.udl` to uncomment method declaration

2. **Complete Phase 1**:
   - Regenerate Swift bindings
   - Write unit tests for provider testing

3. **Start Phase 3 (Swift UI Presets)**:
   - Create preset models
   - Define 6 provider configurations

### Medium-term:

4. **Implement Phase 2 (Command Validation)**
5. **Implement Phase 4 (Swift UI Components)**
6. **Implement Phase 5 (Configuration Migration)**

### Long-term:

7. **Complete Phase 6 (Halo Validation Hints)**
8. **Complete Phase 8 (Testing & Documentation)**

---

## Known Issues

1. **UniFFI Async Methods**: Need to verify Swift async/await compatibility with UniFFI-generated code
2. **Config Hot-Reload**: SearchRegistry won't update if config changes - need reload mechanism
3. **Fallback Order UI**: Drag-to-reorder not yet designed/implemented (placeholder in Phase 4)

---

## References

- Proposal: `openspec/changes/add-search-settings-ui/proposal.md`
- Design: `openspec/changes/add-search-settings-ui/design.md`
- Tasks: `openspec/changes/add-search-settings-ui/tasks.md`
- Commit: bc20fa4
