# Implementation Tasks

## Phase 1: Rust Core - Provider Testing API (5h)

### Task 1.1: Add ProviderTestResult enum (1h)
- [ ] Create `ProviderTestResult` enum in `Aether/core/src/search/mod.rs`
  - Variants: `Success { latency_ms: u32 }`, `AuthError { message: String }`, `NetworkError { message: String }`, `InvalidConfig { message: String }`
- [ ] Derive UniFFI traits for enum
- [ ] Add to `aether.udl` interface definition

**Validation**: Enum compiles and generates Swift code via `uniffi-bindgen`

---

### Task 1.2: Implement test_search_provider() in SearchRegistry (2h)
- [ ] Add `test_search_provider(&self, name: &str) -> Result<ProviderTestResult>` to `SearchRegistry`
- [ ] Implementation:
  - Get provider from registry by name
  - Execute minimal test query: `"test"`` with `max_results = 1`
  - Measure latency with `Instant::now()`
  - Map provider errors to `ProviderTestResult` variants
- [ ] Add result caching with 5-minute TTL (use `HashMap<String, (ProviderTestResult, Instant)>`)

**Validation**: Cargo test with mock provider succeeds

---

### Task 1.3: Export test API via UniFFI (1h)
- [ ] Add `test_search_provider()` method to `AetherCore` struct
- [ ] Update `aether.udl`:
  ```idl
  interface AetherCore {
      [Async]
      ProviderTestResult test_search_provider(string provider_name);
  };
  ```
- [ ] Regenerate Swift bindings: `cargo run --bin uniffi-bindgen generate ...`
- [ ] Copy `libaethecore.dylib` to `Aether/Frameworks/`

**Validation**: Swift can call `await core.testSearchProvider("tavily")`

---

### Task 1.4: Unit tests for provider testing (1h)
- [ ] Create `Aether/core/src/search/tests/test_provider_testing.rs`
- [ ] Test cases:
  - `test_search_provider_success()` - Valid provider returns Success
  - `test_search_provider_auth_error()` - Invalid API key returns AuthError
  - `test_search_provider_not_found()` - Unknown provider returns error
  - `test_result_caching()` - Second call within 5 min returns cached result

**Validation**: `cargo test search::tests::test_provider` passes

---

## Phase 2: Rust Core - Command Validation (4h)

### Task 2.1: Add ValidationError type (1h)
- [ ] Create `ValidationError` enum in `Aether/core/src/error.rs`:
  - `MissingSpace { command: String, suggestion: String }`
- [ ] Implement `Display` and `Error` traits
- [ ] Add to `AetherError` enum as variant

**Validation**: Error compiles and can be constructed

---

### Task 2.2: Implement validate_command_format() (2h)
- [ ] Add `validate_command_format(&self, input: &str) -> Result<(), ValidationError>` to `Router`
- [ ] Logic:
  ```rust
  if input.starts_with('/') {
      let parts: Vec<&str> = input.splitn(2, ' ').collect();
      if parts.len() == 1 && input.len() > 1 {
          return Err(ValidationError::MissingSpace {
              command: parts[0].to_string(),
              suggestion: format!("{} <your query>", parts[0])
          });
      }
  }
  Ok(())
  ```
- [ ] Integrate into `AetherCore::process_clipboard()` before routing

**Validation**: Unit tests for valid/invalid formats pass

---

### Task 2.3: Add Halo validation hints (1h)
- [ ] Add `on_validation_hint(message: String, suggestion: String)` callback to `AetherEventHandler` in `aether.udl`
- [ ] Call callback from `AetherCore` when `ValidationError` occurs
- [ ] Regenerate UniFFI bindings

**Validation**: Validation error triggers Swift callback

---

## Phase 3: Swift UI - Provider Presets (3h) ✅ COMPLETED

### Task 3.1: Create SearchProviderPreset struct (1h) ✅
- [x] Create `Aether/Sources/Models/SearchProviderPreset.swift`
- [x] Define structs:
  ```swift
  struct SearchProviderPreset {
      let id: String
      let displayName: String
      let iconName: String
      let color: String
      let providerType: String
      let fields: [SearchPresetField]
      let docsURL: URL
      let description: String
  }

  struct SearchPresetField {
      let key: String
      let displayName: String
      let type: SearchFieldType
      let required: Bool
      let defaultValue: String?
      let options: [String]?
      let placeholder: String?
  }

  enum SearchFieldType {
      case secureText, text, picker
  }
  ```

**Validation**: ✅ Struct compiles

---

### Task 3.2: Define 6 provider presets (2h) ✅
- [x] Create `SearchProviderPresets.all: [SearchProviderPreset]` array with:
  1. **Tavily**: API Key (required), Search Depth (picker: basic/advanced)
  2. **SearXNG**: Instance URL (required, default: `https://searx.be`)
  3. **Google**: API Key (required), Custom Search Engine ID (required)
  4. **Bing**: API Key (required)
  5. **Brave**: API Key (required)
  6. **Exa**: API Key (required)
- [x] Add documentation URLs for each provider
- [x] Add icons and colors for each provider
- [x] Add descriptions for each provider

**Validation**: ✅ All 6 presets defined with correct fields

**Commit**: 7101141

---

## Phase 4: Swift UI - Components (8h)

### Task 4.1: Create ProviderCard component (3h)
- [ ] Create `Aether/Sources/Components/ProviderCard.swift`
- [ ] UI structure:
  - Header: Icon, Name, Status Badge (⚠️/✅/❌/🔄)
  - Body: Dynamic fields based on preset (SecureField, TextField, Picker)
  - Footer: Test Connection button, Documentation link
- [ ] State management:
  ```swift
  @State private var apiKey: String = ""
  @State private var baseURL: String = ""
  @State private var testingStatus: ProviderStatus = .notConfigured
  ```

**Validation**: Card renders correctly in preview

---

### Task 4.2: Implement provider testing logic (2h)
- [ ] Add `testConnection()` async function to ProviderCard
- [ ] Logic:
  ```swift
  testingStatus = .testing
  do {
      let result = await core.testSearchProvider(preset.id)
      switch result {
      case .success(let latency):
          testingStatus = .available(latency: latency)
      case .authError(let msg):
          testingStatus = .error(msg)
      // ...
      }
  } catch {
      testingStatus = .error(error.localizedDescription)
  }
  ```
- [ ] Show toast notification with result

**Validation**: Test button triggers API call and updates status

---

### Task 4.3: Create SearchSettingsView (3h)
- [ ] Create `Aether/Sources/SearchSettingsView.swift`
- [ ] UI structure:
  ```
  ScrollView
  ├─ Provider Configuration Section
  │  ├─ ProviderCard (Tavily)
  │  ├─ ProviderCard (SearXNG)
  │  └─ ... (4 more)
  ├─ Fallback Order Section
  │  └─ DraggableList (not implemented in Phase 4, placeholder)
  └─ PII Scrubbing Section (migrated from BehaviorSettingsView)
  ```
- [ ] Integrate `UnifiedSaveBar` for unsaved changes

**Validation**: View renders with all 6 provider cards

---

## Phase 5: Configuration & Migration (4h)

### Task 5.1: Add PIIConfig to SearchConfig (1h)
- [ ] Update `Aether/core/src/config/mod.rs`:
  ```rust
  #[derive(Debug, Clone, Serialize, Deserialize, Default)]
  pub struct PIIConfig {
      pub enabled: bool,
      pub scrub_email: bool,
      pub scrub_phone: bool,
      pub scrub_ssn: bool,
      pub scrub_credit_card: bool,
  }
  ```
- [ ] Add `pub pii: PIIConfig` field to `SearchConfig`
- [ ] Update UniFFI exports

**Validation**: Cargo builds successfully

---

### Task 5.2: Implement config migration (2h)
- [ ] Add `migrate_pii_config()` function in `Config::load()`:
  ```rust
  if let Some(behavior) = &config.behavior {
      if let Some(pii_enabled) = behavior.pii_scrubbing_enabled {
          config.search.pii.enabled = pii_enabled;
          // Clear old field
          config.behavior.pii_scrubbing_enabled = None;
          config.save()?;
      }
  }
  ```
- [ ] Run migration only if `search.pii` is empty and `behavior.pii_scrubbing_enabled` exists

**Validation**: Existing config with PII in behavior migrates correctly

---

### Task 5.3: Move PII UI to SearchSettingsView (1h)
- [ ] Copy PII scrubbing card from `BehaviorSettingsView.swift` to `SearchSettingsView.swift`
- [ ] Update state bindings to read from `searchConfig.pii` instead of `behaviorConfig`
- [ ] Remove PII card from `BehaviorSettingsView.swift`
- [ ] Update localization keys: `settings.behavior.pii_*` → `settings.search.pii_*`

**Validation**: PII settings appear in Search tab, not in Behavior tab

---

## Phase 6: Halo Validation Hints UI (2h)

### Task 6.1: Add ValidationHint state to HaloView (1h)
- [ ] Update `HaloView.swift` to handle validation hint state
- [ ] Add amber border color for validation hints:
  ```swift
  var borderColor: Color {
      switch state {
      case .validationHint: return DesignTokens.Colors.warning
      case .error: return DesignTokens.Colors.error
      case .success: return DesignTokens.Colors.success
      default: return DesignTokens.Colors.primary
      }
  }
  ```
- [ ] Add auto-dismiss timer (2 seconds)

**Validation**: Halo shows amber border for validation hints

---

### Task 6.2: Implement on_validation_hint callback (1h)
- [ ] Update `EventHandler.swift` to implement `on_validation_hint()`:
  ```swift
  func onValidationHint(message: String, suggestion: String) {
      showHalo(message: "\(message)\n\(suggestion)", state: .validationHint)

      DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) {
          hideHalo()
      }
  }
  ```

**Validation**: Typing `/searchquery` shows validation hint

---

## Phase 7: Preset Routing Rules (2h)

### Task 7.1: Add default routing rules to Config::default() (1h)
- [ ] Update `Config::default()` in `Aether/core/src/config/mod.rs`:
  ```rust
  rules: vec![
      RoutingRuleConfig {
          regex: r"^/search\s+".to_string(),
          provider: "openai".to_string(),
          system_prompt: Some("You are a search assistant.".to_string()),
          strip_prefix: Some(true),
          capabilities: Some(vec!["search".to_string()]),
          intent_type: Some("builtin_search".to_string()),
          // ... rest None
      },
      // /mcp and /skill rules
  ],
  ```

**Validation**: Fresh config includes 3 default rules

---

### Task 7.2: Add config comments for command format (1h)
- [ ] Update TOML serialization to include comments above routing rules:
  ```toml
  # Slash commands require space after prefix: /search <query>
  [[rules]]
  regex = "^/search\\s+"
  provider = "openai"
  ```
- [ ] Add comment generation logic in `Config::save()`

**Validation**: Saved config.toml includes helpful comments

---

## Phase 8: Testing & Documentation (6h)

### Task 8.1: Rust unit tests (2h)
- [ ] Command validation tests (`router::tests::test_command_validation`)
- [ ] Provider testing tests (already done in Phase 1)
- [ ] Config migration tests (`config::tests::test_pii_migration`)

**Validation**: `cargo test` passes all tests

---

### Task 8.2: Manual testing all 6 providers (3h)
- [ ] Test Tavily with valid/invalid API key
- [ ] Test SearXNG with public instance
- [ ] Test Google with valid credentials
- [ ] Test Bing with valid API key
- [ ] Test Brave with valid API key
- [ ] Test Exa with valid API key
- [ ] Verify status indicators update correctly
- [ ] Verify fallback mechanism works

**Validation**: All providers work as expected

---

### Task 8.3: Update documentation (1h)
- [ ] Update `docs/ARCHITECTURE.md` with Search Settings UI section
- [ ] Update `CLAUDE.md` with command validation rules
- [ ] Add screenshots to `docs/ui-design-guide.md`

**Validation**: Documentation is complete and accurate

---

## Total Estimated Time: ~34 hours

**Phase Breakdown**:
- Phase 1: Rust Core - Provider Testing API (5h)
- Phase 2: Rust Core - Command Validation (4h)
- Phase 3: Swift UI - Provider Presets (3h)
- Phase 4: Swift UI - Components (8h)
- Phase 5: Configuration & Migration (4h)
- Phase 6: Halo Validation Hints UI (2h)
- Phase 7: Preset Routing Rules (2h)
- Phase 8: Testing & Documentation (6h)

**Dependencies**:
- Phase 2 can run in parallel with Phase 1
- Phase 3 can start after Phase 1 Task 1.3 (UniFFI export)
- Phase 4 requires Phase 3 completion
- Phase 5 can run in parallel with Phase 4
- Phase 6 requires Phase 2 completion
- Phase 7 is independent, can start anytime
- Phase 8 requires all previous phases
