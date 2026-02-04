# Design Document: add-default-provider-selection

## Overview
This document describes the technical architecture and design decisions for implementing default provider selection and menu bar quick switch functionality.

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                         User Interactions                        │
├─────────────────────────────────────────────────────────────────┤
│  Settings UI                           Menu Bar                  │
│  - Edit Panel: "Set as Default" Btn   - Click Provider Name     │
│  - Visual: "Default" Badge            - Checkmark (✓) Indicator │
│                                        - Only Enabled Providers  │
└─────────────────────┬───────────────────────┬───────────────────┘
                      │                       │
                      ▼                       ▼
              ┌───────────────────────────────────────┐
              │      Swift UI Layer (ProvidersView)   │
              │  - Track defaultProviderId state      │
              │  - Render badges and indicators       │
              │  - Handle user actions                │
              └──────────────┬────────────────────────┘
                             │ UniFFI Calls
                             ▼
              ┌───────────────────────────────────────┐
              │      Rust Core (AlephCore)           │
              │  - getDefaultProvider()               │
              │  - setDefaultProvider(name)           │
              │  - getEnabledProviders()              │
              └──────────────┬────────────────────────┘
                             │
                             ▼
              ┌───────────────────────────────────────┐
              │      Config Layer (config/mod.rs)     │
              │  - Validate default_provider          │
              │  - Ensure enabled = true              │
              │  - Atomic save to config.toml         │
              └──────────────┬────────────────────────┘
                             │
                             ▼
              ┌───────────────────────────────────────┐
              │      Router Layer (router/mod.rs)     │
              │  - Use default on no rule match       │
              │  - Fallback to first enabled          │
              │  - Handle disabled/missing default    │
              └───────────────────────────────────────┘
```

## Component Design

### 1. Rust Core - Config Validation

**File**: `Aleph/core/src/config/mod.rs`

**Design Decision**: Centralize default provider validation in the config layer rather than router.

**Rationale**:
- Config layer is the single source of truth for all provider settings
- Validation should happen at config load time, not routing time (performance)
- Easier to maintain validation logic in one place

**Implementation**:
```rust
impl Config {
    pub fn validate(&self) -> Result<()> {
        // Existing validation...

        // NEW: Validate default provider
        if let Some(ref default_provider) = self.general.default_provider {
            // Check provider exists
            if !self.providers.contains_key(default_provider) {
                return Err(AlephError::invalid_config(
                    format!("Default provider '{}' not found", default_provider)
                ));
            }

            // Check provider is enabled
            if let Some(provider_config) = self.providers.get(default_provider) {
                if !provider_config.enabled {
                    warn!("Default provider '{}' is disabled", default_provider);
                    // Don't fail validation, just warn (allow manual config edits)
                }
            }
        }
        Ok(())
    }

    pub fn get_default_provider(&self) -> Option<String> {
        // Return default only if it exists and is enabled
        self.general.default_provider.as_ref().and_then(|name| {
            self.providers.get(name).and_then(|config| {
                if config.enabled {
                    Some(name.clone())
                } else {
                    None
                }
            })
        })
    }

    pub fn set_default_provider(&mut self, name: &str) -> Result<()> {
        // Validate provider exists and is enabled
        match self.providers.get(name) {
            Some(config) if config.enabled => {
                self.general.default_provider = Some(name.to_string());
                Ok(())
            }
            Some(_) => Err(AlephError::invalid_config(
                format!("Provider '{}' is not enabled", name)
            )),
            None => Err(AlephError::invalid_config(
                format!("Provider '{}' not found", name)
            )),
        }
    }
}
```

### 2. Rust Core - UniFFI Bridge

**File**: `Aleph/core/src/aleph.udl`

**Design Decision**: Expose minimal, high-level API through UniFFI.

**Rationale**:
- Keep UniFFI surface area small (easier to maintain)
- Encapsulate validation logic in Rust (safer than Swift)
- Swift layer is purely presentational

**Implementation**:
```idl
interface AlephCore {
    // Existing methods...

    // NEW: Default provider management
    string? get_default_provider();
    void set_default_provider(string provider_name);
    sequence<string> get_enabled_providers();
};
```

**File**: `Aleph/core/src/core.rs`

```rust
impl AlephCore {
    pub fn get_default_provider(&self) -> Option<String> {
        let config = self.config.lock().unwrap();
        config.get_default_provider()
    }

    pub fn set_default_provider(&self, provider_name: String) -> Result<()> {
        let mut config = self.config.lock().unwrap();
        config.set_default_provider(&provider_name)?;
        config.save()?;

        // Notify event handler (optional, for UI updates)
        if let Some(ref handler) = self.event_handler {
            handler.on_config_changed()?;
        }

        Ok(())
    }

    pub fn get_enabled_providers(&self) -> Vec<String> {
        let config = self.config.lock().unwrap();
        config.providers
            .iter()
            .filter(|(_, cfg)| cfg.enabled)
            .map(|(name, _)| name.clone())
            .collect()
    }
}
```

### 3. Router - Fallback Logic

**File**: `Aleph/core/src/router/mod.rs`

**Design Decision**: Implement graceful fallback in router initialization.

**Rationale**:
- Router should be resilient to config changes (defensive programming)
- Avoid runtime panics if default provider is disabled/deleted
- Provide clear logging for debugging

**Implementation**:
```rust
impl Router {
    pub fn new(config: &Config) -> Result<Self> {
        // Existing initialization...

        // NEW: Validate and store default provider with fallback
        let default_provider = config.get_default_provider().or_else(|| {
            // Fallback: use first enabled provider
            config.providers
                .iter()
                .find(|(_, cfg)| cfg.enabled)
                .map(|(name, _)| name.clone())
        });

        if let Some(ref default) = default_provider {
            info!("Default provider set to: {}", default);
        } else {
            warn!("No enabled providers found, routing will fail");
        }

        Ok(Self {
            providers,
            rules,
            default_provider,
        })
    }
}
```

### 4. Swift UI - ProvidersView State Management

**File**: `Aleph/Sources/ProvidersView.swift`

**Design Decision**: Use separate `@State` for default provider ID, reload on config changes.

**Rationale**:
- Decouple default provider state from provider list state
- Allow independent updates (e.g., menu bar changes default without reloading full list)
- Consistent with existing pattern (`selectedProviderId`, `isAddingNew`, etc.)

**Implementation**:
```swift
struct ProvidersView: View {
    // Existing state...

    // NEW: Track current default provider
    @State private var defaultProviderId: String?

    var body: some View {
        // Existing UI...
    }

    private func loadDefaultProvider() {
        defaultProviderId = try? core.getDefaultProvider()
    }

    private func setAsDefault(_ providerId: String) {
        do {
            try core.setDefaultProvider(providerId: providerId)
            loadDefaultProvider()  // Refresh state
            toastData = ToastData(
                message: "Default provider set to \(providerId)",
                type: .success
            )
        } catch {
            toastData = ToastData(
                message: "Failed to set default: \(error.localizedDescription)",
                type: .error
            )
        }
    }

    private func isDefault(_ preset: PresetProvider) -> Bool {
        return defaultProviderId == preset.id
    }
}
```

### 5. Menu Bar - Dynamic Provider Menu

**File**: `Aleph/Sources/AppDelegate.swift`

**Design Decision**: Rebuild menu on config changes using observer pattern.

**Rationale**:
- NSMenu is not reactive, must be rebuilt manually
- Observer pattern ensures menu stays in sync with config
- Performance is acceptable (menu rebuild is fast, <1ms)

**Implementation**:
```swift
class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusItem: NSStatusItem?
    private var providersMenuSection: NSMenu?

    private func setupMenuBar() {
        // Existing menu setup...

        rebuildProvidersMenu()

        // Observe config changes
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(onConfigChanged),
            name: .configDidChange,
            object: nil
        )
    }

    private func rebuildProvidersMenu() {
        guard let core = core else { return }

        let menu = statusItem?.menu ?? NSMenu()

        // Remove old provider items (between separators)
        // ... removal logic ...

        // Add enabled providers
        let enabledProviders = core.getEnabledProviders()
        if !enabledProviders.isEmpty {
            menu.addItem(NSMenuItem.separator())

            let defaultProvider = try? core.getDefaultProvider()
            for providerName in enabledProviders.sorted() {
                let item = NSMenuItem(
                    title: providerName,
                    action: #selector(selectDefaultProvider(_:)),
                    keyEquivalent: ""
                )
                item.state = (providerName == defaultProvider) ? .on : .off
                menu.addItem(item)
            }

            menu.addItem(NSMenuItem.separator())
        }
    }

    @objc private func selectDefaultProvider(_ sender: NSMenuItem) {
        let providerName = sender.title
        do {
            try core?.setDefaultProvider(providerName: providerName)
            rebuildProvidersMenu()  // Update checkmarks
        } catch {
            // Show error alert
        }
    }

    @objc private func onConfigChanged() {
        rebuildProvidersMenu()
    }
}
```

## Design Trade-offs

### Trade-off 1: Validation Strictness
**Options**:
1. **Strict**: Prevent app from starting if default provider is disabled (fail fast)
2. **Permissive**: Allow disabled default, use fallback (graceful degradation)

**Decision**: Permissive approach (Option 2)

**Rationale**:
- Better UX: app still works even if config is manually edited incorrectly
- Users might temporarily disable default provider for testing
- Warning messages guide user to fix the issue
- Failing to start would be frustrating for power users

### Trade-off 2: Auto-enable on Set as Default
**Options**:
1. **Auto-enable**: Automatically enable a disabled provider when set as default
2. **Error**: Prevent setting disabled provider as default with error message

**Decision**: Error approach (Option 2) with optional confirmation dialog

**Rationale**:
- More explicit: user understands what's happening
- Avoids unexpected state changes (principle of least surprise)
- Confirmation dialog (optional) provides best of both worlds
- Simpler implementation for MVP

### Trade-off 3: Menu Bar Update Frequency
**Options**:
1. **Polling**: Check config every N seconds and rebuild menu if changed
2. **Observer**: Rebuild menu only when config change notification is received

**Decision**: Observer pattern (Option 2)

**Rationale**:
- More efficient: no unnecessary CPU usage
- Instant updates: responds immediately to config changes
- macOS best practice: use NotificationCenter for this pattern

### Trade-off 4: Default Badge Position
**Options**:
1. **Top-right corner**: Consistent with "Active" indicator position
2. **Next to provider name**: More prominent, easier to scan
3. **Separate column**: Most structured, but takes more space

**Decision**: Top-right corner (Option 1)

**Rationale**:
- Consistent with existing "Active" indicator
- Doesn't interfere with provider name (clickable area)
- Compact layout, suitable for 240px wide sidebar

## Security Considerations

1. **Config File Permissions**: Already handled by atomic write (chmod 600)
2. **API Key Exposure**: No changes, API keys remain in config only
3. **Input Validation**: Provider names are validated against existing providers (no injection risk)

## Performance Considerations

1. **Config Validation**: Added validation is O(1) lookup in HashMap (negligible)
2. **Menu Rebuild**: Rebuilding 5-10 menu items is <1ms (acceptable)
3. **UniFFI Calls**: get_default_provider() is fast (single lock + lookup)
4. **UI Rerender**: Only affected components re-render (badge, checkmark)

## Migration Path

**No migration needed**: This is an additive feature.

- Existing configs without `default_provider` continue to work (None = fallback to first enabled)
- Existing routing logic is preserved
- No breaking changes to UniFFI API

## Testing Strategy

### Unit Tests (Rust)
- Config validation with enabled/disabled default provider
- Router fallback logic when default is missing/disabled
- UniFFI method behavior (get/set default provider)

### Integration Tests (Swift + Rust)
- Set default from UI, verify config.toml updated
- Restart app, verify default persists
- Disable default provider, verify fallback works

### UI Tests (Manual)
- Visual inspection of "Default" badge
- Context menu interaction
- Menu bar provider list and checkmark
- Edge cases (no providers, all disabled, rapid switching)

## Open Questions & Future Work

1. **Question**: Should we show provider status (online/offline) in menu bar?
   - **Answer**: Out of scope for this change. Consider for future enhancement.

2. **Question**: Should default provider selection sync across multiple devices?
   - **Answer**: Not applicable. Config is local to each machine.

3. **Future Work**: Keyboard shortcuts for switching providers (e.g., Cmd+1, Cmd+2)
4. **Future Work**: Recent providers list in menu bar (quick access to last 3 used)

## Summary

This design provides a robust, user-friendly default provider management system with:
- ✅ Clear visual indicators (badge, checkmark)
- ✅ Multiple ways to set default (Settings, menu bar)
- ✅ Graceful fallback for edge cases
- ✅ Minimal performance impact
- ✅ No breaking changes
- ✅ Comprehensive validation

The implementation is straightforward and follows existing patterns in the codebase.
