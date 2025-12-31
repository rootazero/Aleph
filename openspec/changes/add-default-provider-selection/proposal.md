# Change Proposal: add-default-provider-selection

## Metadata
- **ID**: add-default-provider-selection
- **Title**: Add Default Provider Selection and Menubar Quick Switch
- **Type**: Feature Addition
- **Status**: Draft
- **Created**: 2025-12-31

## Overview

### Problem Statement
Currently, Aether's routing system uses `general.default_provider` in config.toml for fallback routing, but:

1. **No UI control**: Users cannot visually select or change the default provider through Settings UI
2. **No visual indication**: There's no way to see which provider is currently set as default
3. **No quick switching**: Users must go to Settings to modify config.toml to change the default provider
4. **Menu bar complexity**: The menu bar shows all providers, not just active/enabled ones

### Proposed Solution
Implement a comprehensive default provider management system:

1. **Settings UI Enhancement**:
   - Add a "Default" badge/indicator next to the selected default provider in ProvidersView
   - Add "Set as Default" button in the provider edit panel
   - Only allow enabled providers to be set as default

2. **Menu Bar Quick Switch**:
   - Display only enabled providers in the menu bar
   - Show a checkmark (✓) next to the current default provider
   - Allow users to click a provider in the menu to set it as default
   - Update both the config and active routing immediately

3. **Config Integration**:
   - Ensure `general.default_provider` is always in sync with UI selections
   - Validate that the default provider exists and is enabled
   - Handle edge cases (default provider disabled, deleted, etc.)

### Success Criteria
- [ ] Users can visually identify the default provider in Settings UI
- [ ] Users can set any enabled provider as default from edit panel
- [ ] Menu bar shows only enabled providers
- [ ] Users can quickly switch default provider from menu bar
- [ ] Config changes are persisted and survive app restarts
- [ ] Routing system respects the selected default provider

## Impact Analysis

### User Experience
- **Positive**: Significantly improved UX for managing default provider
- **Positive**: Quick access to provider switching without opening Settings
- **Positive**: Visual clarity on which provider is currently active by default

### Technical Complexity
- **Medium**: Requires changes across Swift UI, Rust config, and UniFFI bridge
- **Low Risk**: Most changes are additive, existing routing logic remains intact

### Dependencies
- Affects `ai-routing` spec (default provider selection)
- Affects `settings-ui-layout` spec (default provider UI indicator)
- Affects `provider-active-state` spec (filtering active providers)
- New capability: `default-provider-management`

## Alternatives Considered

### Alternative 1: Config-only approach
- Keep default provider selection in config.toml only
- **Rejected**: Poor UX, requires manual file editing

### Alternative 2: Settings UI only (no menu bar)
- Add default provider selection to Settings UI only
- **Rejected**: Misses opportunity for quick switching, common user need

## Open Questions
1. Should we allow setting an inactive provider as default and auto-enable it?
   - **Proposed**: No, only enabled providers can be default. If current default is disabled, show warning.

2. What happens if the default provider is deleted?
   - **Proposed**: Clear `general.default_provider`, show warning in UI, use first enabled provider as fallback.

3. Should menu bar show provider status (online/offline)?
   - **Proposed**: Out of scope for this change, focus on default selection only.

## Affected Capabilities
- `default-provider-management` (NEW)
- `ai-routing` (MODIFIED)
- `settings-ui-layout` (MODIFIED)
- `provider-active-state` (MODIFIED)
