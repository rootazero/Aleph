# Design: Redesign Providers UI Layout

## Architecture Overview

This change focuses on the **presentation layer** (SwiftUI components) without modifying the underlying data model or business logic in the Rust core.

### Key Design Decisions

#### 1. Window Sizing Strategy
**Decision**: Increase minimum window size from 1000x700 to 1200x800

**Rationale**:
- Reference design (`uisample.png`) shows a spacious layout with room for detailed provider information
- Current 1000x700 window forces Edit Panel to compress content, reducing readability
- Larger window aligns with modern macOS application standards (e.g., System Settings)
- Users with smaller screens can still resize down, but default should be generous

**Trade-offs**:
- **Pro**: Better content visibility, less scrolling required
- **Con**: May not fit well on 13" MacBook Air (1440x900 screen)
- **Mitigation**: Window remains resizable, users can shrink if needed

**Implementation Approach**:
```swift
// SettingsView.swift - Update frame modifier
.frame(minWidth: 1200, minHeight: 800, idealWidth: 1400, idealHeight: 900)
.windowResizability(.contentSize)
```

---

#### 2. Active/Inactive State Management
**Decision**: Use API key presence as proxy for "active" state initially, with option to add explicit `enabled` field later

**Rationale**:
- Adding a new `enabled: bool` field to `ProviderConfig` (Rust schema) requires:
  - Updating `Aleph/core/src/config.rs`
  - Regenerating UniFFI bindings
  - Testing serialization/deserialization (TOML)
  - Handling backward compatibility for existing config files
- This adds significant complexity for a UI-focused change
- Current heuristic: "Has API key = Active" is semantically correct for most cases

**Trade-offs**:
- **Pro**: Minimal backend changes, faster implementation
- **Con**: Cannot have a provider with API key but intentionally disabled
- **Future Enhancement**: Add explicit `enabled` field when more advanced provider management is needed (e.g., temporary disable for debugging)

**Implementation Approach**:
```swift
// ProvidersView.swift - Derive active state
private func isProviderActive(_ provider: ProviderConfigEntry) -> Bool {
    // Check if provider has API key configured
    if let apiKey = provider.config.apiKey, apiKey.starts(with: "keychain:") {
        return (try? keychainManager.hasApiKey(provider: provider.name)) ?? false
    }
    // Ollama doesn't need API key
    return provider.config.providerType == "ollama"
}
```

**Future Migration Path**:
If we add `enabled` field to Rust schema:
1. Update `ProviderConfig` struct in `Aleph/core/src/config.rs`
2. Add `enabled: Option<bool>` (default `None` for backward compat)
3. Regenerate UniFFI bindings: `cargo run --bin uniffi-bindgen generate`
4. Update SwiftUI to use `provider.config.enabled ?? isProviderActive(provider)`

---

#### 3. Inline Test Results vs. Modal/Toast
**Decision**: Display connection test results as small caption text below the "Test Connection" button

**Rationale**:
- Reference design shows inline feedback (see "Use with Claude Code" section)
- Current implementation uses large card-style views (lines 423-450 in ProviderEditPanel.swift) which take up significant vertical space
- Inline display keeps the user's focus in one area (no need to hunt for toast notification)
- Aligns with modern design patterns (e.g., GitHub form validation)

**Trade-offs**:
- **Pro**: More compact, less visual clutter, immediate feedback
- **Con**: Long error messages need truncation + tooltip
- **Con**: May be harder to notice success state (green text is subtle)

**Implementation Approach**:
```swift
// ProviderEditPanel.swift - Replace testResultView()
@ViewBuilder
private var inlineTestResult: some View {
    if let result = testResult {
        HStack(spacing: 6) {
            Image(systemName: result.isSuccess ? "checkmark.circle.fill" : "xmark.circle.fill")
                .foregroundColor(result.isSuccess ? .green : .red)
                .font(.system(size: 12))

            Text(result.message)
                .font(DesignTokens.Typography.caption)
                .foregroundColor(result.isSuccess ? .green : .red)
                .lineLimit(2)
                .truncationMode(.tail)
                .help(result.fullMessage) // Tooltip for long errors
        }
        .padding(.top, 8)
    }
}
```

**Accessibility Consideration**:
- Use `.accessibilityLabel()` to announce full error message to VoiceOver
- Success state should be announced prominently (not just color change)

---

#### 4. Fixed Footer vs. Scrollable Buttons
**Decision**: Use fixed footer for action buttons (outside ScrollView)

**Rationale**:
- Reference design shows "Close" and "Save" buttons anchored to bottom-right, not scrolling with content
- If buttons scroll away, users might not realize they need to scroll to save
- Common macOS pattern: Fixed toolbar/footer for primary actions (e.g., Mail compose window)

**Trade-offs**:
- **Pro**: Buttons always visible, clear call-to-action
- **Con**: Reduces scrollable area by ~60pt (button height + padding)
- **Con**: Requires restructuring ProviderEditPanel's VStack

**Implementation Approach**:
```swift
// ProviderEditPanel.swift - Restructure body
var body: some View {
    VStack(spacing: 0) {
        // Scrollable content
        ScrollView {
            VStack(alignment: .leading, spacing: DesignTokens.Spacing.lg) {
                // Form fields here
            }
            .padding(DesignTokens.Spacing.lg)
        }

        // Fixed footer
        Divider()
        HStack {
            Spacer()
            HStack(spacing: DesignTokens.Spacing.sm) {
                ActionButton("Test Connection", ...)
                ActionButton("Cancel", ...)
                ActionButton("Save", ...)
            }
        }
        .padding(DesignTokens.Spacing.lg)
        .background(DesignTokens.Colors.contentBackground)
    }
}
```

**Alternative Considered**: `.safeAreaInset(edge: .bottom)` modifier
- Rejected because it adds visual separation (floating button bar), not desired in reference design

---

#### 5. Provider Card Layout: Active Indicator Position
**Decision**: Place active indicator (blue dot) in the top-right corner of provider card

**Rationale**:
- Reference design shows blue dots next to provider names in left panel
- Top-right corner is a common macOS pattern for status indicators (e.g., notification badges)
- Doesn't interfere with provider icon (left side) or text content (center)

**Trade-offs**:
- **Pro**: Clear visual separation from other card elements
- **Con**: Users might associate it with "unread" status (like notification badges)
- **Mitigation**: Use distinct styling (filled circle vs. badge number)

**Implementation Approach**:
```swift
// ProviderCard.swift - Add overlay
.overlay(alignment: .topTrailing) {
    if isActive {
        Circle()
            .fill(Color(hex: "#007AFF"))
            .frame(width: 8, height: 8)
            .padding(12) // Inset from corner
    } else {
        Circle()
            .strokeBorder(DesignTokens.Colors.textSecondary.opacity(0.3), lineWidth: 1)
            .frame(width: 8, height: 8)
            .padding(12)
    }
}
```

---

## Component Interaction Diagram

```
┌─────────────────────────────────────────────────────────┐
│                    SettingsView                         │
│  ┌─────────────┐  ┌──────────────────────────────────┐ │
│  │  Sidebar    │  │       Content Area               │ │
│  │             │  │  ┌───────────────────────────┐   │ │
│  │ - General   │  │  │    ProvidersView           │   │ │
│  │ - Providers │◄─┼─►│  ┌─────────┬──────────────┐│   │ │
│  │ - Routing   │  │  │  │ List    │ EditPanel    ││   │ │
│  │ - ...       │  │  │  │         │              ││   │ │
│  └─────────────┘  │  │  │ Card 1  │ [Form]       ││   │ │
│                   │  │  │ Card 2  │   Active: ☑  ││   │ │
│  1200 x 800 min   │  │  │ Card 3  │   [Fields]   ││   │ │
│                   │  │  │         │   ---        ││   │ │
│                   │  │  │         │   [Test][✗][✓]│   │ │
│                   │  │  └─────────┴──────────────┘│   │ │
│                   │  └───────────────────────────┘   │ │
│                   └──────────────────────────────────┘ │
└─────────────────────────────────────────────────────────┘

Data Flow:
1. User selects provider card → ProvidersView sets selectedProvider
2. ProvidersView passes selectedProvider to ProviderEditPanel
3. ProviderEditPanel loads config from core.loadConfig()
4. User edits form → Local @State variables updated
5. User clicks Save → ProviderEditPanel calls core.updateProvider()
6. Core saves to disk → ProvidersView reloads → UI updates
```

---

## Performance Considerations

### Rendering Optimization
- **Issue**: Re-rendering all provider cards when one is selected
- **Solution**: Use `.id(provider.name)` to minimize diff calculations
- **Impact**: Negligible for <20 providers, important for 50+ providers

### State Management
- **Issue**: Two sources of truth (Swift @State + Rust config)
- **Solution**: Rust config is source of truth, SwiftUI state is ephemeral
- **Pattern**: Load on appear, save on commit, discard on cancel

### Animation Performance
- **Issue**: Card hover effects + selection animations may stutter
- **Solution**: Use `.animation(.easeInOut(duration: 0.2), value: isSelected)` (explicit value binding)
- **Fallback**: Disable animations on older Macs via `DesignTokens.Animation.quick`

---

## Testing Strategy

### Unit Tests (Not Required for UI Changes)
- No new business logic in this change
- Existing tests in `AlephTests/ConfigPersistenceTests.swift` cover config save/load

### Manual Testing Checklist
See `tasks.md` Phase 8 for comprehensive integration tests

### Visual Regression Testing
- **Approach**: Side-by-side comparison with `uisample.png`
- **Tools**: Take screenshots at key states:
  1. Providers list with 3 providers (1 active, 2 inactive)
  2. Edit panel in view mode
  3. Edit panel in edit mode (form filled)
  4. Connection test success state
  5. Connection test failure state
- **Acceptance Criteria**: Visual parity within 5% (minor spacing differences acceptable)

---

## Migration Path

### Backward Compatibility
- No config file format changes required
- Existing `config.toml` files work without modification
- Users will see active state derived from API key presence

### Forward Compatibility
If we add `enabled` field in future:
```toml
# config.toml - Future schema
[providers.openai]
provider_type = "openai"
api_key = "keychain:openai"
model = "gpt-4o"
enabled = true  # NEW FIELD (optional, defaults to true if api_key present)
```

**Migration Script** (if needed):
```rust
// Aleph/core/src/config.rs - Auto-migrate on load
impl ProviderConfig {
    fn migrate_v1_to_v2(mut self) -> Self {
        if self.enabled.is_none() {
            // Auto-enable if API key exists
            self.enabled = Some(self.api_key.is_some());
        }
        self
    }
}
```

---

## Security Considerations

### No New Security Risks
- This change is purely UI/presentation layer
- No new API endpoints, no new permissions required
- Keychain integration unchanged (still using `KeychainManagerImpl`)

### Audit Recommendations
- Verify test connection results don't leak API keys in error messages
- Ensure truncated error messages in UI don't hide security warnings
- Confirm active/inactive toggle doesn't bypass authentication checks

---

## Accessibility Considerations

### VoiceOver Support
- All new UI elements must have `.accessibilityLabel()`
- Active indicator: "Active" / "Inactive" announced with provider name
- Test result: Full error message in accessibility tree (not truncated)

### Keyboard Navigation
- Tab order: Provider list → Edit panel form → Buttons
- Space bar toggles active switch
- Enter key triggers "Save" button when valid

### Color Contrast
- Active indicator blue: #007AFF (WCAG AA compliant on white background)
- Success green: #28A745 (WCAG AA compliant)
- Error red: #DC3545 (WCAG AA compliant)

---

## Open Questions (Resolved)

### ~~Q1: Should we persist active state to Rust config or keep it UI-only?~~
**Resolution**: Use API key presence as proxy initially. Add explicit `enabled` field in future if needed.

### ~~Q2: Should "Test Connection" be in the fixed footer or scroll with form?~~
**Resolution**: Include in fixed footer for visibility. Reference design shows buttons grouped at bottom.

### ~~Q3: How to handle inactive providers in routing rules?~~
**Resolution**: Router should skip inactive providers and fall back to next rule. This is out of scope for this change but should be tracked separately.

---

## Future Enhancements (Out of Scope)

1. **Batch Operations**: Select multiple providers, enable/disable all at once
2. **Provider Templates**: Quick-add common providers (OpenAI, Claude) with pre-filled defaults
3. **Connection Auto-Test**: Test all providers on Settings open, show status in list
4. **Provider Groups**: Organize providers by category (Cloud, Local, Custom)
5. **Export/Import**: Share provider configs (without API keys) between machines
