# Halo Validation Hints Spec

**Capability**: Visual feedback for command format errors via Halo overlay

**Relates to**: `halo-ui`, `command-validation`

---

## ADDED Requirements

### Requirement: Halo displays command validation errors

The Halo overlay SHALL display validation error messages with visual differentiation from normal processing states.

**Rationale**: 提供即时、非阻塞的视觉反馈，帮助用户修正命令格式而无需切换上下文

**Priority**: Must Have

#### Scenario: Validation hint appears at cursor

- **GIVEN** user types `/searchquery` and triggers hotkey
- **WHEN** Router detects missing space
- **THEN** Halo appears at cursor position
- **AND** Halo shows message "Add space: /search <your query>"
- **AND** Halo border color is amber (warning state)

#### Scenario: Validation hint auto-dismisses

- **GIVEN** validation hint is displayed
- **WHEN** 2 seconds elapse
- **THEN** Halo fades out automatically
- **AND** original input remains in clipboard

#### Scenario: User can dismiss hint early

- **GIVEN** validation hint is displayed
- **WHEN** user presses Escape key
- **THEN** Halo dismisses immediately
- **AND** no processing occurs

---

### Requirement: Validation hints have distinct visual style

The UI SHALL visually distinguish validation hints from success/error/processing states.

**Rationale**: 清晰区分验证提示（用户可修正）和系统错误（需调查）

**Priority**: Must Have

#### Scenario: Validation hint uses warning colors

- **GIVEN** validation error occurs
- **WHEN** Halo is rendered
- **THEN** border color is `DesignTokens.Colors.warning` (amber)
- **AND** icon is `exclamationmark.triangle`
- **AND** background has subtle amber tint

#### Scenario: Validation hint is distinct from API errors

- **GIVEN** API error occurs (e.g., 401 Unauthorized)
- **WHEN** Halo shows error state
- **THEN** border color is `DesignTokens.Colors.error` (red)
- **AND** icon is `xmark.circle`
- **AND** message is API-specific (not validation hint)

---

### Requirement: Validation hints do not block processing

The system SHALL allow users to retry immediately after validation hint dismissal.

**Rationale**: 避免强制等待，让用户控制工作流节奏

**Priority**: Must Have

#### Scenario: User can retry after hint

- **GIVEN** validation hint is shown for `/searchquery`
- **WHEN** user dismisses hint (Escape or timeout)
- **AND** user corrects input to `/search query`
- **AND** user triggers hotkey again
- **THEN** validation passes
- **AND** normal processing begins

#### Scenario: Hint does not consume clipboard

- **GIVEN** user input is `/searchquery`
- **WHEN** validation hint is shown
- **THEN** clipboard content is NOT modified
- **AND** original text remains available for correction

---

## MODIFIED Requirements

### Requirement (Modified): HaloState includes validation variant

**Original**: `HaloState` enum has `Idle, Listening, Processing, Success, Error`
**Modified**: Add `ValidationHint` state

**Changes**:
```rust
pub enum HaloState {
    Idle,
    Listening,
    Processing,
    Success,
    Error(String),
    ValidationHint(String),  // NEW: Validation error message
}
```

**Behavior**:
- `ValidationHint` auto-dismisses after 2 seconds
- Does NOT trigger full error handling flow
- Does NOT log to error telemetry

---

### Requirement (Modified): AetherEventHandler supports validation hints

**Original**: `on_error(message: String)` callback for all errors
**Modified**: Add `on_validation_hint(message: String, suggestion: String)`

**Changes**:
```idl
callback interface AetherEventHandler {
    void on_state_changed(HaloState state);
    void on_halo_show(HaloPosition position, string? provider_color);
    void on_halo_hide();
    void on_error(string message);
    void on_validation_hint(string message, string suggestion);  // NEW
};
```

---

## REMOVED Requirements

*(None)*

---

## Validation Criteria

- [ ] Validation hints appear at cursor position
- [ ] Amber border color distinguishes from errors
- [ ] Auto-dismisses after 2 seconds
- [ ] User can dismiss with Escape key
- [ ] Does not modify clipboard content
- [ ] Does not block retry attempts
