# Config Management Specification

## ADDED Requirements

### Requirement: Config File Watching
The Rust core **SHALL** monitor config.toml for external modifications and trigger hot-reload.

#### Scenario: Detect config file change
- **GIVEN** Aether is running with config watcher initialized
- **WHEN** user edits `~/.config/aether/config.toml` in external editor
- **AND** saves file
- **THEN** FSEvents notifies Rust watcher within 500ms
- **AND** watcher debounces rapid changes (waits 500ms for more events)
- **AND** watcher parses new config.toml
- **AND** validates new config
- **WHEN** validation passes
- **THEN** watcher updates internal config state
- **AND** calls `handler.onConfigChanged(new_config)` callback to notify Swift UI

#### Scenario: Handle invalid config during hot-reload
- **GIVEN** config watcher is running
- **WHEN** user edits config.toml with invalid TOML syntax (e.g., unmatched bracket)
- **AND** saves file
- **THEN** watcher attempts to parse
- **AND** parsing fails with error
- **AND** watcher logs error: "Failed to reload config: Invalid TOML syntax at line 42"
- **AND** watcher retains previous valid config (does not update)
- **AND** calls `handler.onConfigError("Invalid TOML syntax at line 42")` to notify UI

---

### Requirement: Atomic Config Writes
The Rust core **SHALL** write config.toml atomically to prevent corruption during concurrent writes.

#### Scenario: Atomic write with temp file
- **WHEN** Swift calls `core.updateProvider(provider)` to save new provider config
- **THEN** Rust serializes full config to TOML string
- **AND** creates temp file: `~/.config/aether/config.toml.tmp`
- **AND** writes TOML content to temp file
- **AND** calls `fsync()` to flush to disk
- **AND** renames temp file to `config.toml` (atomic operation)
- **AND** old config.toml is replaced atomically

#### Scenario: Concurrent write conflict
- **GIVEN** Two processes attempt to write config simultaneously
- **WHEN** Process A writes to config.toml.tmp
- **AND** Process B writes to config.toml.tmp (overwrites A's temp file)
- **AND** Process B renames to config.toml (wins)
- **AND** Process A tries to rename (fails because temp file is gone)
- **THEN** Process A retries write operation
- **AND** eventually succeeds with retry

---

### Requirement: Config Validation
The Rust core **SHALL** validate all config modifications before persisting to file.

#### Scenario: Validate regex patterns in routing rules
- **WHEN** Swift calls `core.updateRoutingRules(rules)` with rule containing pattern "[invalid"
- **THEN** Rust calls `Regex::new("[invalid")`
- **AND** compilation fails with error "Unclosed character class"
- **AND** Rust returns `Err(ConfigError::InvalidRegex { pattern: "[invalid", error: "Unclosed character class" })`
- **AND** config.toml is NOT modified

#### Scenario: Validate provider configuration
- **WHEN** Swift calls `core.updateProvider(provider)` with provider name "unknown"
- **AND** "unknown" is not in allowed providers list
- **THEN** Rust returns `Err(ConfigError::UnknownProvider { name: "unknown" })`
- **AND** config.toml is NOT modified

#### Scenario: Validate hotkey format
- **WHEN** Swift calls `core.updateShortcuts(shortcuts)` with hotkey "InvalidKey"
- **AND** hotkey cannot be parsed into valid key combo
- **THEN** Rust returns `Err(ConfigError::InvalidHotkey { key: "InvalidKey", error: "Unknown key name" })`
- **AND** config.toml is NOT modified

---

### Requirement: Config Backup and Restore
The Rust core **SHALL** support backup and restore of configuration files.

#### Scenario: Automatic backup before write
- **GIVEN** valid config.toml exists at `~/.config/aether/config.toml`
- **WHEN** user saves new provider configuration
- **THEN** Rust copies current config.toml to `~/.config/aether/backups/config.toml.{timestamp}`
- **AND** keeps last 5 backups (deletes older backups)
- **AND** proceeds with atomic write of new config

#### Scenario: Restore from backup
- **WHEN** user's config.toml becomes corrupted
- **AND** user calls `core.restoreConfigBackup(timestamp)`
- **THEN** Rust copies backup file to config.toml
- **AND** validates restored config
- **WHEN** validation passes
- **THEN** Rust reloads config
- **AND** calls `handler.onConfigChanged(restored_config)` to notify UI

---

### Requirement: Config Migration
The Rust core **SHALL** support automatic migration of config schema across versions.

#### Scenario: Migrate from v1 to v2 config schema
- **GIVEN** user has config.toml from Aether v0.1.0 (schema v1)
- **AND** schema v1 does not have `[memory]` section
- **WHEN** user upgrades to Aether v0.2.0 (schema v2 requires `[memory]` section)
- **AND** Rust loads config.toml
- **THEN** Rust detects missing `[memory]` section
- **AND** adds default `[memory]` section:
  ```toml
  [memory]
  enabled = true
  embedding_model = "all-MiniLM-L6-v2"
  max_context_items = 5
  retention_days = 90
  ```
- **AND** writes migrated config to disk with atomic write
- **AND** logs: "Migrated config from v1 to v2"

#### Scenario: Reject unsupported future config version
- **GIVEN** user has config.toml from Aether v0.3.0 (schema v3)
- **WHEN** user downgrades to Aether v0.2.0 (only supports schema v2)
- **AND** Rust loads config.toml
- **THEN** Rust detects `config_version = 3` in TOML
- **AND** returns error: "Config version 3 is not supported by this version of Aether (max: 2)"
- **AND** refuses to start until user resolves config incompatibility

---

### Requirement: Keychain API Integration
The Rust core **SHALL** integrate with macOS Keychain via FFI for secure API key storage.

#### Scenario: Store API key in Keychain (Rust → Swift → Keychain)
- **WHEN** user saves OpenAI API key "sk-test123" via Settings UI
- **THEN** Swift calls Rust: `core.updateProvider(provider)` with `api_key = "sk-test123"`
- **AND** Rust detects API key field
- **AND** Rust calls Swift FFI: `saveAPIKeyToKeychain(service: "com.aether.openai", key: "sk-test123")`
- **AND** Swift calls `Security.SecAddGenericPassword()` to store in Keychain
- **AND** Rust writes config.toml with reference: `api_key = "keychain:com.aether.openai"`
- **NOT** plain text key in config.toml

#### Scenario: Retrieve API key from Keychain at runtime
- **GIVEN** config.toml has `api_key = "keychain:com.aether.openai"`
- **WHEN** Rust provider needs API key for OpenAI request
- **THEN** Rust parses "keychain:" prefix
- **AND** Rust calls Swift FFI: `loadAPIKeyFromKeychain(service: "com.aether.openai")`
- **AND** Swift calls `Security.SecCopyItemMatching()` to retrieve key
- **AND** Swift returns decrypted key "sk-test123" to Rust
- **AND** Rust uses key for API authentication

#### Scenario: Delete API key from Keychain
- **WHEN** user deletes OpenAI provider via Settings UI
- **THEN** Swift calls Rust: `core.deleteProvider("openai")`
- **AND** Rust calls Swift FFI: `deleteAPIKeyFromKeychain(service: "com.aether.openai")`
- **AND** Swift calls `Security.SecDeleteItemMatching()` to remove from Keychain
- **AND** Rust removes provider entry from config.toml

---

### Requirement: Config Default Values
The Rust core **SHALL** provide sensible defaults for all config options when not specified.

#### Scenario: Initialize config with defaults on first launch
- **GIVEN** user launches Aether for first time
- **AND** no config.toml exists at `~/.config/aether/config.toml`
- **WHEN** Rust initializes config system
- **THEN** Rust creates default config:
  ```toml
  [general]
  theme = "cyberpunk"
  default_provider = "openai"

  [shortcuts]
  summon = "Command+Grave"
  cancel = "Escape"

  [behavior]
  input_mode = "cut"
  output_mode = "typewriter"
  typing_speed = 50

  [memory]
  enabled = true
  embedding_model = "all-MiniLM-L6-v2"
  max_context_items = 5
  retention_days = 90
  similarity_threshold = 0.7
  vector_db = "sqlite-vec"
  ```
- **AND** Rust writes default config to disk with atomic write

#### Scenario: Merge partial config with defaults
- **GIVEN** user has minimal config.toml with only:
  ```toml
  [general]
  theme = "zen"
  ```
- **WHEN** Rust loads config
- **THEN** Rust merges with defaults for missing sections
- **AND** `[shortcuts]`, `[behavior]`, `[memory]` use default values
- **AND** Rust does NOT overwrite user's `theme = "zen"` setting

---

### Requirement: Config Error Reporting
The Rust core **SHALL** provide detailed error messages for config validation failures.

#### Scenario: Report specific validation error
- **WHEN** user saves routing rule with invalid regex "[incomplete"
- **AND** Rust validation fails
- **THEN** Rust returns structured error:
  ```rust
  ConfigError::InvalidRegex {
      pattern: "[incomplete",
      line: 42,
      error: "Unclosed character class starting at position 0"
  }
  ```
- **AND** Swift displays error in UI: "Invalid regex pattern on line 42: Unclosed character class"

#### Scenario: Report TOML parsing error
- **WHEN** user edits config.toml with invalid TOML: `provider = OpenAI` (missing quotes)
- **AND** Rust attempts to parse
- **THEN** Rust returns:
  ```rust
  ConfigError::ParseError {
      line: 15,
      column: 12,
      error: "Expected string literal, found identifier"
  }
  ```
- **AND** Swift displays error: "Config syntax error at line 15, column 12: Expected string literal"

---

### Requirement: UniFFI Config API
The Rust core **SHALL** expose config operations via UniFFI for Swift integration.

#### Scenario: UniFFI method signatures
- **GIVEN** aether.udl defines config operations
- **THEN** following methods are available in Swift:
  ```swift
  // Config CRUD operations
  func getConfig() throws -> Config
  func updateProvider(provider: ProviderConfig) throws
  func deleteProvider(name: String) throws
  func updateRoutingRules(rules: [RoutingRule]) throws
  func updateShortcuts(shortcuts: ShortcutsConfig) throws
  func updateBehavior(behavior: BehaviorConfig) throws
  func updateMemoryConfig(config: MemoryConfig) throws

  // Config validation
  func validateRegex(pattern: String) throws -> Bool
  func validateHotkey(key: String) throws -> Bool

  // Config backup/restore
  func listConfigBackups() throws -> [String]  // Returns timestamps
  func restoreConfigBackup(timestamp: String) throws

  // Provider testing
  func testProviderConnection(provider: String) async throws -> String
  ```
