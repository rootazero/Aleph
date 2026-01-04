# Implementation Status: Add Search Settings UI

## Summary

This document tracks the implementation status of the `add-search-settings-ui` proposal.

**Current Status**: Phase 1, 5, and 7 completed via `integrate-search-registry` proposal

**Last Updated**: 2026-01-04

---

## Completed Phases

### ✅ Phase 1: Rust Core - Provider Testing API (Completed via integrate-search-registry)

**Tasks Completed**:
- [x] Created `ProviderTestResult` struct in `Aether/core/src/search/mod.rs`
- [x] Implemented `test_search_provider()` method in `SearchRegistry`
  - [x] Minimal test query execution (`"test"` with `max_results = 1`)
  - [x] Latency measurement with `Instant::now()`
  - [x] 5-minute TTL caching to avoid API quota abuse
  - [x] Error classification (auth, network, config)
- [x] Exported `ProviderTestResult` via UniFFI (dictionary definition in `.udl`)
- [x] Integrated `SearchRegistry` into `AetherCore` as persistent field
- [x] Implemented `test_search_provider()` method in `AetherCore`
- [x] Regenerated Swift bindings with `uniffi-bindgen generate`

**Files Modified**:
- `Aether/core/src/search/mod.rs` - Added `ProviderTestResult` struct
- `Aether/core/src/search/registry.rs` - Implemented `test_search_provider()`
- `Aether/core/src/core.rs` - Added `search_registry` field and `test_search_provider()` method
- `Aether/core/src/aether.udl` - Added `ProviderTestResult` dictionary and async method
- `Aether/core/src/lib.rs` - Exported `ProviderTestResult`

**Commits**: 1f28fa6, 47aea26 (via integrate-search-registry)

---

### ✅ Phase 5: Configuration & Migration (Completed via integrate-search-registry)

**Tasks Completed**:
- [x] Added `PIIConfig` struct to `config.rs`
- [x] Added `pii: PIIConfig` field to `SearchConfig` and `SearchConfigInternal`
- [x] Implemented config migration in `Config::load_from_file()`
  - [x] Detect `behavior.pii_scrubbing_enabled`
  - [x] Migrate to `search.pii.enabled`
  - [x] Auto-save migrated config
- [x] Updated PII scrubbing to read from new location (with backward compatibility)
- [x] Exported `PIIConfig` via UniFFI

**Files Modified**:
- `Aether/core/src/config/mod.rs` - Added `PIIConfig`, migration logic
- `Aether/core/src/core.rs` - Updated PII config reading
- `Aether/core/src/aether.udl` - Added `PIIConfig` dictionary
- `Aether/core/src/lib.rs` - Exported `PIIConfig`

**Commits**: 6853f15 (via integrate-search-registry)

**Note**: PII UI migration to SearchSettingsView is pending (Swift UI work)

---

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

**Dependencies**: Phase 1 completed ✅

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
  - Provider testing tests (basic tests exist)
  - Config migration tests
- [ ] Manual testing with all 6 search providers
- [ ] Update documentation:
  - `docs/ARCHITECTURE.md` - Add Search Settings UI section
  - `CLAUDE.md` - Add command validation rules
  - `docs/ui-design-guide.md` - Add screenshots

**Estimated Time**: 6 hours

---

## Next Steps

### Immediate:

1. **Phase 3: Swift UI Provider Presets** (3h)
   - Create SearchProviderPreset model
   - Define 6 provider configurations with fields and docs URLs

2. **Phase 4: Swift UI Components** (8h)
   - Build ProviderCard component
   - Create SearchSettingsView
   - Migrate PII UI from BehaviorSettingsView

### Medium-term:

3. **Phase 2: Command Validation** (4h)
   - Add ValidationError type
   - Implement validation logic
   - Add UniFFI callback

4. **Phase 6: Halo Validation Hints** (2h)
   - Update HaloView for validation state
   - Implement Swift callback

### Long-term:

5. **Phase 8: Testing & Documentation** (6h)
   - Write unit tests
   - Manual testing
   - Update docs

---

## Architecture Notes

### SearchRegistry Integration (✅ Completed)

**Current Architecture** (after integrate-search-registry):
```rust
pub struct AetherCore {
    event_handler: Arc<dyn AetherEventHandler>,
    runtime: Arc<Runtime>,
    config: Arc<Mutex<Config>>,
    memory_db: Option<Arc<VectorDatabase>>,
    router: Arc<RwLock<Option<Arc<Router>>>>,
    search_registry: Arc<RwLock<Option<Arc<SearchRegistry>>>>, // ✅ ADDED
}

impl AetherCore {
    // ✅ Async method for provider testing
    pub async fn test_search_provider(&self, provider_name: String) -> ProviderTestResult {
        let registry_arc = {
            let registry_guard = self.search_registry.read().unwrap_or_else(|e| e.into_inner());
            registry_guard.as_ref().map(Arc::clone)
        };

        match registry_arc {
            Some(reg) => reg.test_search_provider(&provider_name).await,
            None => ProviderTestResult {
                success: false,
                latency_ms: 0,
                error_message: "Search capability not enabled in configuration".to_string(),
                error_type: "config".to_string(),
            }
        }
    }
}
```

---

## Known Issues

1. **Fallback Order UI**: Drag-to-reorder not yet designed/implemented (placeholder in Phase 4)
2. **PII UI Migration**: Swift UI component migration from BehaviorSettingsView to SearchSettingsView pending

---

## References

- Proposal: `openspec/changes/add-search-settings-ui/proposal.md`
- Design: `openspec/changes/add-search-settings-ui/design.md`
- Tasks: `openspec/changes/add-search-settings-ui/tasks.md`
- Related Proposal: `openspec/changes/integrate-search-registry/proposal.md` (completed)
- Commits: bc20fa4, 1f28fa6, 6853f15, 47aea26
