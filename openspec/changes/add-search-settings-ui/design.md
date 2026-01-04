# Search Settings UI - Design Document

## Overview

This document outlines the architectural decisions for adding a comprehensive Search Settings UI to Aether, enabling users to configure search providers through a graphical interface instead of manual TOML editing.

## Context

### Current State

**Search Capability (✅ Implemented)**:
- 6 search providers supported: Tavily, SearXNG, Google, Bing, Brave, Exa
- `SearchRegistry` in Rust core manages providers with fallback mechanism
- `SearchConfig` and `SearchBackendConfig` in TOML configuration
- Integration with Structured Context Protocol via `AgentPayload`

**Routing System (✅ Implemented)**:
- `RoutingRule` with regex matching and `strip_prefix` support
- `Router::strip_command_prefix()` removes matched command prefixes
- Intent classification: `BuiltinSearch`, `BuiltinMcp`, `Skills`, `Custom`, `GeneralChat`

**Settings UI (Partial)**:
- Existing settings tabs: General, Shortcuts, Behavior, Memory, Routing Rules, Providers
- PII scrubbing currently in Behavior settings
- No dedicated Search settings tab

### Problem

1. **Configuration Complexity**: Users must manually edit `~/.config/aether/config.toml` to configure search providers
2. **No Validation Feedback**: No way to test if API keys and endpoints are valid
3. **Command Ambiguity**: Slash commands like `/search` can conflict with custom commands like `/se` due to prefix matching
4. **Scattered UI**: Search-related PII settings are in Behavior tab, not with other search configuration

## Goals

### Primary: Search Settings UI

Create a dedicated "Search" settings tab with:

1. **Provider Configuration Cards** (6 providers)
   - Name, icon, status indicator
   - Configuration fields (API key, base URL, search depth, etc.)
   - "Test Connection" button with real-time validation
   - Documentation link

2. **Provider Status Management**
   - Real-time status: ⚠️ Not Configured / ✅ Available / ❌ Offline / 🔄 Testing
   - Auto-check on settings view load
   - Manual test via button click

3. **Preset Templates**
   - Default configurations for each provider
   - Provider-specific field definitions (e.g., SearXNG needs `base_url`, not `api_key`)
   - Default values (e.g., SearXNG → `https://searx.be`)

4. **Fallback Order Management**
   - Drag-to-reorder list of fallback providers
   - Visual indicator of current order
   - Sync with `config.toml` `fallback_providers` array

5. **PII Scrubbing Migration**
   - Move PII settings from Behavior tab to Search tab
   - Maintain existing functionality
   - Update localization keys

### Secondary: Command Validation

Enhance slash command system to prevent conflicts:

1. **Space Requirement**
   - Commands MUST have space after prefix: `/search query` ✅ vs `/searchquery` ❌
   - Enforced at Router level before intent classification

2. **Precise Matching**
   - Token-based matching instead of prefix matching
   - `/se custom prompt` won't match `/search` pattern
   - Enables custom commands with short prefixes

3. **Halo Validation Hints**
   - When user types `/searchquery` (no space), show Halo hint: "Add space after command: /search query"
   - Non-blocking visual feedback
   - Dismiss after 2 seconds or user correction

## Architecture Decisions

### Decision 1: Command Validation Strategy

**Options Considered**:

A. **Strict Whitespace Enforcement** (Chosen)
   - Require space: `/command<space>arguments`
   - Validate in `Router::route()` before regex matching
   - Return `ValidationError` with hint text for UI

B. **Delimiter-based Parsing**
   - Use delimiter like `:` → `/command:arguments`
   - More explicit but breaks existing user patterns
   - Rejected: Breaking change

C. **Greedy Prefix Matching** (Current behavior)
   - `/search` matches `/se` as substring
   - No way to prevent conflicts
   - Rejected: Does not solve the problem

**Rationale**: Option A preserves natural language flow while enabling precise command detection. Users already expect space after commands in terminal/shell contexts.

**Implementation**:
```rust
// Pseudo-code
impl Router {
    fn validate_command_format(&self, input: &str) -> Result<(), ValidationError> {
        if input.starts_with('/') {
            let parts: Vec<&str> = input.splitn(2, ' ').collect();
            if parts.len() == 1 && input.len() > 1 {
                // No space found, input is `/commandargs`
                return Err(ValidationError::MissingSpace {
                    suggestion: format!("{} <your query>", parts[0])
                });
            }
        }
        Ok(())
    }
}
```

### Decision 2: Provider Testing API

**Options Considered**:

A. **Rust Core with UniFFI Export** (Chosen)
   - Add `test_search_provider(name: String) -> ProviderTestResult` to `AetherCore`
   - Export via UniFFI to Swift
   - Reuse existing `SearchRegistry` infrastructure

B. **Swift-Side HTTP Testing**
   - Implement provider-specific test logic in Swift
   - Direct HTTP calls from SwiftUI
   - Rejected: Duplicates provider logic, breaks DRY

C. **Background Auto-Testing**
   - Automatically test all providers on app launch
   - Rejected: Slow startup, unnecessary API calls

**Rationale**: Option A centralizes provider logic in Rust, maintains single source of truth, and leverages UniFFI for seamless Swift integration.

**UniFFI Definition**:
```idl
enum ProviderTestResult {
  "Success" { latency_ms: u32 },
  "AuthError" { message: string },
  "NetworkError" { message: string },
  "InvalidConfig" { message: string }
};

interface AetherCore {
  async ProviderTestResult test_search_provider(string provider_name);
};
```

### Decision 3: UI Component Structure

**Component Hierarchy**:
```
SearchSettingsView (Tab Root)
├─ ProviderConfigSection
│  ├─ ProviderCard (x6 - one per provider)
│  │  ├─ ProviderHeader (name, icon, status badge)
│  │  ├─ ProviderConfigFields (API key, base URL, etc.)
│  │  └─ ProviderActions (Test button, Docs link)
│  └─ FallbackOrderList (drag-to-reorder)
│
├─ SearchBehaviorSection
│  ├─ DefaultProviderPicker
│  └─ MaxResultsSlider
│
└─ PIIScrubbingSection (migrated from BehaviorSettingsView)
   ├─ PIIToggle
   └─ PIITypeCheckboxes
```

**Rationale**: Follows existing settings UI patterns (e.g., `ProviderSettingsView`, `MemorySettingsView`) for consistency. Reusable `ProviderCard` component reduces code duplication.

### Decision 4: Configuration Sync Strategy

**Options Considered**:

A. **Dual State (UI + Config)** (Chosen)
   - `@State` variables in SwiftUI track UI state
   - `UnifiedSaveBar` pattern for save/discard actions
   - Batch update to Rust core on save

B. **Direct Binding to Rust**
   - SwiftUI bindings directly mutate Rust state via UniFFI
   - Rejected: No undo capability, instant persistence feels janky

C. **Local-First with Periodic Sync**
   - Save to local Swift state, sync to Rust on timer
   - Rejected: Data loss risk, complex state reconciliation

**Rationale**: Option A matches existing settings tabs (Behavior, Memory, etc.), provides familiar UX with save/discard, and prevents accidental config corruption.

### Decision 5: Preset Templates Data Structure

**Options Considered**:

A. **Hardcoded Swift Structs** (Chosen)
   - Define `SearchProviderPreset` struct array in Swift
   - Contains: name, icon, required fields, default values, docs URL
   - Simple, fast, no external dependencies

B. **JSON Config File**
   - Load presets from `presets.json` in app bundle
   - More flexible but slower, needs error handling

C. **Rust-Side Presets**
   - Define in Rust, export via UniFFI
   - Rejected: Adds complexity, UI data doesn't need backend logic

**Rationale**: Option A is simplest, presets are static data that won't change at runtime. Swift is appropriate layer for UI-specific constants.

**Example**:
```swift
struct SearchProviderPreset {
    let id: String  // "tavily"
    let displayName: String  // "Tavily Search"
    let icon: String  // "magnifyingglass.circle"
    let fields: [PresetField]
    let docsURL: URL
}

struct PresetField {
    let key: String  // "api_key"
    let displayName: String  // "API Key"
    let type: FieldType  // .secureText, .text, .picker
    let required: Bool
    let defaultValue: String?
    let options: [String]?  // For pickers
}

let searchPresets: [SearchProviderPreset] = [
    SearchProviderPreset(
        id: "tavily",
        displayName: "Tavily Search",
        icon: "magnifyingglass.circle",
        fields: [
            PresetField(key: "api_key", displayName: "API Key", type: .secureText, required: true),
            PresetField(key: "search_depth", displayName: "Search Depth", type: .picker,
                       options: ["basic", "advanced"], defaultValue: "basic")
        ],
        docsURL: URL(string: "https://tavily.com/docs")!
    ),
    // ... 5 more providers
]
```

## Data Flow

### Provider Configuration Flow

```
User edits API key in UI
    ↓
SwiftUI @State updated
    ↓
UnifiedSaveBar appears (hasUnsavedChanges = true)
    ↓
User clicks "Save"
    ↓
Swift: core.updateSearchConfig(config)
    ↓
Rust: Config::save() writes to config.toml
    ↓
Rust: SearchRegistry reloads providers
    ↓
Rust → Swift callback: on_config_updated()
    ↓
Swift: Refresh UI state, hide SaveBar
```

### Provider Testing Flow

```
User clicks "Test Connection"
    ↓
Swift: Button shows loading spinner
    ↓
Swift: await core.testSearchProvider("tavily")
    ↓
Rust: SearchRegistry.get_provider("tavily")
    ↓
Rust: provider.search("test query", 1)
    ↓ (async HTTP request)
Rust: Measure latency, check response
    ↓
Rust: Return ProviderTestResult
    ↓
Swift: Update status badge, show toast
```

### Command Validation Flow

```
User types "/search query"
    ↓
AetherCore processes clipboard
    ↓
Router::validate_command_format(input)
    ↓ (if no space after slash)
Rust → Swift: on_halo_show_hint("Add space: /search query")
    ↓
Swift: Halo displays hint with yellow border
    ↓ (2 second timer)
Swift: Auto-dismiss hint
```

## Implementation Phases

### Phase 1: Backend API (Rust Core)
- Add `test_search_provider()` method to `SearchRegistry`
- Add `ProviderTestResult` enum
- Export via UniFFI
- Unit tests for testing logic

### Phase 2: Command Validation
- Add `validate_command_format()` to `Router`
- Add `ValidationError` type
- Integrate into `AetherCore::process_clipboard()`
- Add Halo hint callback

### Phase 3: Swift UI Components
- Create `SearchSettingsView.swift`
- Create `ProviderCard.swift` component
- Implement preset templates
- Integrate `UnifiedSaveBar`

### Phase 4: Configuration Migration
- Move PII settings from `BehaviorSettingsView` to `SearchSettingsView`
- Update localization keys
- Add fallback order UI (drag-to-reorder)

### Phase 5: Integration & Testing
- Add Search tab to `SettingsView`
- Update default routing rules for `/search`, `/mcp`, `/skill`
- Manual testing with all 6 providers
- Update documentation

## Risk Analysis

### Risk 1: UniFFI Async Limitations

**Risk**: UniFFI async functions may have callback limitations with Swift Concurrency
**Mitigation**: Use `Task {}` wrapper in Swift to handle async UniFFI calls
**Fallback**: Implement callback-based approach with `AetherEventHandler`

### Risk 2: Provider Test Quota Limits

**Risk**: Repeated testing could exhaust API quotas (e.g., Tavily free tier)
**Mitigation**:
- Cache test results for 5 minutes
- Show warning before re-testing
- Use minimal test queries (single result, simple keyword)

### Risk 3: Configuration Race Conditions

**Risk**: User could modify config.toml manually while UI is open
**Mitigation**:
- Add file watcher in Rust core
- Emit `on_config_file_changed()` callback
- Show alert: "Config changed externally, reload?"

## Open Questions

1. **Q**: Should we auto-test providers on settings view load?
   **A**: Yes, but cache results. Only re-test if config changed or user clicks button.

2. **Q**: How to handle provider-specific options (e.g., Tavily's `search_depth`)?
   **A**: Use preset templates with optional fields. Advanced users can edit TOML directly.

3. **Q**: Should we allow disabling specific providers without removing them?
   **A**: Yes, add `enabled: bool` field to `SearchBackendConfig`. Update UI to show toggle.

4. **Q**: How to handle deprecated providers (e.g., Google Custom Search API changes)?
   **A**: Show deprecation warning in UI, link to migration guide. Don't remove from presets.

## Success Metrics

1. **User Experience**:
   - Time to configure first search provider: < 2 minutes (vs 10+ minutes with TOML)
   - Provider test success rate: > 95% for valid credentials
   - Zero crashes during provider testing

2. **Code Quality**:
   - All new code covered by unit tests
   - No regressions in existing search functionality
   - Validation passes with `--strict` mode

3. **Performance**:
   - Settings view load time: < 500ms
   - Provider test latency: < 3 seconds (depends on API)
   - No UI freezing during async operations

## References

- [Structured Context Protocol Spec](../../specs/structured-context-protocol/spec.md)
- [Search Capability Integration Spec](../../specs/search-capability/spec.md)
- [Routing System Spec](../../specs/ai-routing/spec.md)
- [UniFFI Documentation](https://mozilla.github.io/uniffi-rs/)
- [SwiftUI Concurrency Best Practices](https://developer.apple.com/documentation/swift/concurrency)
