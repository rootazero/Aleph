# Halo Error Feedback Specification

## ADDED Requirements

### Requirement: Display Typed Error Information with Actions
Error states SHALL display typed error information with actionable UI elements.

#### Scenario: Network error shows retry button

**Given** user triggers hotkey
**When** network request fails with timeout (30s)
**Then** Rust calls on_error_typed(.timeout, "Request timed out")
**And** HaloState updates to .error(type: .timeout, message: "Request timed out")
**And** ErrorActionView renders with WiFi slash icon
**And** error message displays below icon
**And** "Retry" button appears
**And** shake animation plays (3 cycles, 16px offset)

---

#### Scenario: Permission error shows settings button

**Given** app lacks Accessibility permission
**When** user triggers hotkey
**Then** Rust detects permission denied
**And** calls on_error_typed(.permission, "Accessibility permission required")
**And** ErrorActionView renders lock shield icon
**And** "Open Settings" button appears
**And** clicking button launches System Settings to Privacy pane

---

### Requirement: UniFFI ErrorType Enum Definition
UniFFI SHALL define ErrorType enum with Network, Permission, Quota, Timeout, Unknown variants.

#### Scenario: Rust categorizes errors into typed variants

**Given** AI provider returns HTTP 429 (rate limit)
**When** Rust receives response
**Then** error mapped to ErrorType::Quota
**And** on_error_typed(.quota, "API quota exceeded") called
**And** Swift receives correct error type

---

### Requirement: Automatic Retry Logic with Exponential Backoff
Retry logic SHALL attempt up to 2 automatic retries with exponential backoff before showing manual retry button.

#### Scenario: Automatic retry with backoff

**Given** network request fails with timeout
**When** first failure occurs at t=0s
**Then** Rust waits 2s (2^0 * 2s)
**And** retries request at t=2s
**When** second failure occurs
**Then** Rust waits 4s (2^1 * 2s)
**And** retries request at t=6s
**When** third failure occurs
**Then** Rust stops auto-retrying
**And** calls on_error_typed(.network, "Network unavailable - Retry manually")
**And** manual "Retry" button appears in Halo

---

### Requirement: Distinct Icon Display for Error Types
Each error type SHALL display a distinct icon from SF Symbols.

#### Scenario: Icon mapping for error types

**Given** error type is `.network`
**Then** icon is "wifi.slash"

**Given** error type is `.permission`
**Then** icon is "lock.shield"

**Given** error type is `.quota`
**Then** icon is "exclamationmark.triangle"

**Given** error type is `.timeout`
**Then** icon is "clock.badge.xmark"

**Given** error type is `.unknown`
**Then** icon is "xmark.circle"

---

### Requirement: Error Action Button Handlers
Error action buttons SHALL trigger appropriate handler methods in EventHandler.

#### Scenario: Retry button calls Rust retry method

**Given** error state with "Retry" button visible
**When** user clicks "Retry" button
**Then** Swift calls core.retryLastRequest()
**And** Rust re-attempts last clipboard processing
**And** Halo transitions to .processing state
**And** if successful, transitions to .success
**And** if failed, shows error again (no infinite loop)

---

## Cross-References

- **Related Specs**: `uniffi-bridge` (ErrorType enum), `event-handler` (callbacks)
- **Depends On**: Phase 2 error handling foundation
- **Blocks**: None
