# Remove FFI: Gateway-Only Architecture

**Date:** 2026-01-28
**Status:** Draft
**Goal:** Remove UniFFI, Swift communicates with Rust only via WebSocket Gateway

---

## Background

The macOS Swift layer currently uses two communication paths:
1. **Gateway WebSocket** — For agent execution, streaming
2. **UniFFI FFI** — For config, memory, plugins, settings, etc.

This dual-path architecture adds complexity. We will unify to Gateway-only.

### Performance Consideration

OCR (`extractText`) requires transmitting large image data. Analysis shows:
- FFI: ~0ms overhead
- WebSocket (Base64 + JSON): ~35ms overhead for 2MB image
- OCR operation itself: 200ms - 2s

**Conclusion:** 35ms (~2-15%) overhead is acceptable for architectural simplicity.

---

## Implementation Plan

### Phase 1: Rust Gateway RPC Implementation

Add 45 new RPC methods across 10 handler files.

#### File Structure

```
core/src/gateway/handlers/
├── mod.rs              # Extend registration
├── agent.rs            # Extend with 3 methods
├── config.rs           # Extend with sub-domains
├── memory.rs           # 🆕 7 methods
├── plugins.rs          # 🆕 6 methods
├── skills.rs           # 🆕 4 methods
├── mcp.rs              # 🆕 6 methods
├── generation.rs       # 🆕 4 methods
├── logs.rs             # 🆕 3 methods
├── ocr.rs              # 🆕 1 method
└── commands.rs         # 🆕 1 method
```

#### RPC Method Definitions

##### providers (6 methods)

| Method | Params | Returns | Description |
|--------|--------|---------|-------------|
| `providers.list` | - | `Provider[]` | List all AI providers |
| `providers.get` | `name: string` | `Provider` | Get single provider config |
| `providers.update` | `name: string, config: ProviderConfig` | `ok` | Update provider |
| `providers.delete` | `name: string` | `ok` | Delete provider |
| `providers.test` | `config: ProviderConfig` | `TestResult` | Test connection |
| `providers.setDefault` | `name: string` | `ok` | Set as default |

##### memory (7 methods)

| Method | Params | Returns | Description |
|--------|--------|---------|-------------|
| `memory.search` | `query?, appBundleId?, limit?` | `Memory[]` | Search memories |
| `memory.delete` | `id: string` | `ok` | Delete single |
| `memory.clear` | `appBundleId?, windowTitle?` | `deletedCount` | Batch clear |
| `memory.clearFacts` | - | `deletedCount` | Clear facts |
| `memory.stats` | - | `MemoryStats` | Statistics |
| `memory.compress` | - | `CompressionResult` | Trigger compression |
| `memory.appList` | - | `string[]` | Available app list |

##### plugins (6 methods)

| Method | Params | Returns | Description |
|--------|--------|---------|-------------|
| `plugins.list` | - | `Plugin[]` | List plugins |
| `plugins.install` | `url: string` | `Plugin` | Install from Git |
| `plugins.installFromZip` | `data: base64` | `string[]` | Install from Zip |
| `plugins.uninstall` | `name: string` | `ok` | Uninstall |
| `plugins.enable` | `name: string` | `ok` | Enable |
| `plugins.disable` | `name: string` | `ok` | Disable |

##### skills (4 methods)

| Method | Params | Returns | Description |
|--------|--------|---------|-------------|
| `skills.list` | - | `Skill[]` | List skills |
| `skills.install` | `url: string` | `Skill` | Install |
| `skills.installFromZip` | `data: base64` | `string[]` | Install from Zip |
| `skills.delete` | `id: string` | `ok` | Delete |

##### mcp (6 methods)

| Method | Params | Returns | Description |
|--------|--------|---------|-------------|
| `mcp.list` | - | `McpServer[]` | List all MCP servers |
| `mcp.add` | `config: McpServerConfig` | `ok` | Add server |
| `mcp.update` | `config: McpServerConfig` | `ok` | Update server |
| `mcp.delete` | `id: string` | `ok` | Delete server |
| `mcp.status` | `id: string` | `McpServerStatus` | Get run status |
| `mcp.logs` | `id: string, maxLines?: number` | `string[]` | Get logs |

##### generation (4 methods)

| Method | Params | Returns | Description |
|--------|--------|---------|-------------|
| `generation.listProviders` | - | `GenerationProvider[]` | List generation providers |
| `generation.getProvider` | `name: string` | `GenerationProviderConfig` | Get config |
| `generation.updateProvider` | `name: string, config` | `ok` | Update config |
| `generation.testProvider` | `name: string, config` | `TestResult` | Test connection |

##### logs (3 methods)

| Method | Params | Returns | Description |
|--------|--------|---------|-------------|
| `logs.getLevel` | - | `string` | Current log level |
| `logs.setLevel` | `level: string` | `ok` | Set level |
| `logs.getDirectory` | - | `string` | Log directory path |

##### ocr (1 method)

| Method | Params | Returns | Description |
|--------|--------|---------|-------------|
| `ocr.extractText` | `image: base64` | `ExtractResult` | Extract text from image |

##### commands (1 method)

| Method | Params | Returns | Description |
|--------|--------|---------|-------------|
| `commands.list` | - | `Command[]` | List all registered commands |

##### agent (extend 3 methods)

| Method | Params | Returns | Description |
|--------|--------|---------|-------------|
| `agent.confirmPlan` | `planId: string, confirmed: bool` | `bool` | Confirm/reject task plan |
| `agent.respondToInput` | `requestId: string, response: string` | `bool` | Respond to user input request |
| `agent.generateTitle` | `userInput: string, aiResponse: string` | `string` | Generate conversation title |

##### config sub-domains (14 methods)

| Method | Params | Returns | Description |
|--------|--------|---------|-------------|
| `config.reload` | - | `ok` | Reload config file |
| `behavior.get` | - | `BehaviorConfig` | Get behavior config |
| `behavior.update` | `config: BehaviorConfig` | `ok` | Update behavior |
| `search.get` | - | `SearchConfig` | Get search config |
| `search.update` | `config: SearchConfig` | `ok` | Update search |
| `search.test` | `config: SearchConfig` | `TestResult` | Test search provider |
| `policies.get` | - | `PoliciesConfig` | Get policies |
| `policies.update` | `config: PoliciesConfig` | `ok` | Update policies |
| `shortcuts.get` | - | `ShortcutsConfig` | Get shortcuts |
| `shortcuts.update` | `config: ShortcutsConfig` | `ok` | Update shortcuts |
| `triggers.get` | - | `TriggerConfig` | Get triggers |
| `triggers.update` | `config: TriggerConfig` | `ok` | Update triggers |
| `security.getCodeExec` | - | `CodeExecConfig` | Code exec config |
| `security.updateCodeExec` | `config` | `ok` | Update |
| `security.getFileOps` | - | `FileOpsConfig` | File ops config |
| `security.updateFileOps` | `config` | `ok` | Update |
| `modelProfiles.update` | `profile: ModelProfile` | `ok` | Update model profile |

---

### Phase 2: Swift Migration (9 Batches)

#### Batch 1: Core Execution Path (High Priority)

| File | Migration |
|------|-----------|
| `MultiTurnCoordinator.swift` | `process` → `agent.run` (exists), `generateTopicTitle` → `agent.generateTitle` |
| `UnifiedConversationViewModel.swift` | `confirmTaskPlan` → `agent.confirmPlan`, `respondToUserInput` → `agent.respondToInput`, `deleteMemoriesByTopicId` → `memory.clear`, `getRootCommandsFromRegistry` → `commands.list` |
| `EventHandler.swift` | `confirmTaskPlan`, `respondToUserInput` |

#### Batch 2: Settings - Providers

| File | Migration |
|------|-----------|
| `ProvidersView.swift` | `loadConfig` → `providers.list`, `getDefaultProvider`, `testProviderConnection` |
| `ProviderConfigView.swift` | `updateProvider` → `providers.update`, `testProviderConnectionWithConfig` |
| `ProviderEditPanel.swift` | Same as above |
| `AppDelegate.swift` | `getEnabledProviders`, `setDefaultProvider` |

#### Batch 3: Settings - Memory

| File | Migration |
|------|-----------|
| `MemoryView.swift` | All memory calls → `memory.*` |

#### Batch 4: Settings - Plugins & Skills

| File | Migration |
|------|-----------|
| `PluginsSettingsView.swift` | All → `plugins.*` |
| `SkillsSettingsView.swift` | All → `skills.*` |

#### Batch 5: Settings - MCP

| File | Migration |
|------|-----------|
| `McpSettingsView.swift` | All → `mcp.*` |

#### Batch 6: Settings - Behavior & Search

| File | Migration |
|------|-----------|
| `BehaviorSettingsView.swift` | → `behavior.*` |
| `SearchSettingsView.swift` | → `search.*` |

#### Batch 7: Settings - Security & Policies

| File | Migration |
|------|-----------|
| `SecuritySettingsView.swift` | → `security.*` |
| `PoliciesSettingsView.swift` | → `policies.*` |

#### Batch 8: Settings - Others

| File | Migration |
|------|-----------|
| `ShortcutsView.swift` | → `shortcuts.*`, `triggers.*` |
| `GenerationProvidersView.swift` | → `generation.*` |
| `ModelProfileEditSheet.swift` | → `modelProfiles.update` |
| `SettingsView.swift` | → `config.get` |

#### Batch 9: Utilities

| File | Migration |
|------|-----------|
| `HotkeyService.swift` | `loadConfig` → `config.get` |
| `LogViewerView.swift` | → `logs.*` |
| `ScreenCaptureCoordinator.swift` | → `ocr.extractText` |
| `CommandCompletionManager.swift` | → `commands.list` |
| `RootContentView.swift` | `loadConfig` → `config.get` |

---

### Phase 3: Remove UniFFI

#### Rust Side Cleanup

| Action | Description |
|--------|-------------|
| Remove `uniffi` dependency | Delete from `Cargo.toml` |
| Delete `#[uniffi::export]` macros | All FFI export functions |
| Delete UDL files | `*.udl` definition files |
| Delete FFI wrapper code | Adapter layer for FFI |
| Simplify build scripts | Remove uniffi-bindgen steps |

#### Swift Side Cleanup

| Action | Description |
|--------|-------------|
| Delete `AetherCore` type references | Global search/replace |
| Delete generated binding files | `AetherCoreFFI.swift` etc. |
| Remove `DependencyContainer.core` | No longer needed |
| Update `initCore()` calls | Change to init Gateway connection |
| Clean Xcode project config | Remove FFI framework linking |

#### Build Flow Simplification

**Before:**
```
Rust build → UniFFI bindgen → Generate Swift bindings →
Link framework → Swift build
```

**After:**
```
Rust build (Gateway binary) → Swift build (independent)
```

---

## Benefits

1. **Unified Architecture** — Single communication channel, easier debugging
2. **Simplified Build** — Remove UniFFI bindgen step
3. **Cross-platform Consistency** — Tauri/iOS use same Gateway protocol
4. **Decoupling** — Swift and Rust can build and release independently

---

## Summary

| Phase | Content | Output |
|-------|---------|--------|
| **Phase 1** | Rust Gateway implements 45 RPC methods | 10 new handler files |
| **Phase 2** | Swift migrates in 9 batches | ~25 Swift files modified |
| **Phase 3** | Remove UniFFI dependency | Simplified build flow |

**Total: 45 new RPC methods, 9 migration batches**
