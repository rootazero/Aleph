# Tasks: Add Phantom Flow Interaction

## 1. Rust Core: Data Types

- [ ] 1.1 Create `clarification/mod.rs` module
- [ ] 1.2 Add `ClarificationType` enum: `Select`, `Text`
- [ ] 1.3 Add `ClarificationOption` struct: `label`, `value`, `description`
- [ ] 1.4 Add `ClarificationRequest` struct with all fields
- [ ] 1.5 Add `ClarificationResult` enum: `Selected`, `TextInput`, `Cancelled`, `Timeout`
- [ ] 1.6 Add unit tests for data types

## 2. UniFFI Interface

- [ ] 2.1 Add `ClarificationType` enum to `aether.udl`
- [ ] 2.2 Add `ClarificationOption` dictionary to `aether.udl`
- [ ] 2.3 Add `ClarificationRequest` dictionary to `aether.udl`
- [ ] 2.4 Add `ClarificationResultType` enum to `aether.udl`
- [ ] 2.5 Add `ClarificationResult` dictionary to `aether.udl`
- [ ] 2.6 Add `on_clarification_needed()` to `AetherEventHandler` callback
- [ ] 2.7 Regenerate UniFFI bindings

## 3. Swift: Data Types & Manager

- [ ] 3.1 Verify UniFFI-generated Swift types
- [ ] 3.2 Create `ClarificationManager.swift`
- [ ] 3.3 Implement `handleClarificationRequest()` with async/await
- [ ] 3.4 Add timeout mechanism (default 60s)
- [ ] 3.5 Connect to `EventHandler` class

## 4. Swift: HaloState Extension

- [ ] 4.1 Add `HaloState.clarification(request:onResult:)` case
- [ ] 4.2 Update `HaloState` Equatable conformance
- [ ] 4.3 Add window size calculation for clarification mode

## 5. Swift: ClarificationView Component

- [ ] 5.1 Create `ClarificationView.swift` in Components/
- [ ] 5.2 Implement `SelectionListView` for select mode
- [ ] 5.3 Implement `TextInputView` for text mode
- [ ] 5.4 Add keyboard navigation (↑↓⏎⎋)
- [ ] 5.5 Add selection highlight animation
- [ ] 5.6 Add hint bar (keyboard shortcuts)

## 6. Swift: HaloView Integration

- [ ] 6.1 Add `.clarification` case to HaloView switch
- [ ] 6.2 Render ClarificationView with correct props
- [ ] 6.3 Handle keyboard events in clarification mode

## 7. Swift: HaloWindow Behavior

- [ ] 7.1 Update window sizing for clarification mode
- [ ] 7.2 Set window size: 350x280 (select), 350x180 (text)
- [ ] 7.3 Ensure no focus stealing

## 8. Integration Testing

- [ ] 8.1 Create test helper: `test_clarification_select()`
- [ ] 8.2 Create test helper: `test_clarification_text()`
- [ ] 8.3 E2E test: Select mode with keyboard navigation
- [ ] 8.4 E2E test: Text mode with placeholder
- [ ] 8.5 E2E test: Cancel with Escape
- [ ] 8.6 E2E test: Timeout behavior

## 9. Documentation

- [ ] 9.1 Add Phantom Flow section to CLAUDE.md
- [ ] 9.2 Document API usage for future feature developers
- [ ] 9.3 Add example: How to trigger clarification from Rust

## Dependencies

- Task 3 depends on Task 2 (UniFFI types)
- Task 4-7 depend on Task 3 (Swift manager)
- Task 8 depends on Task 7 (full integration)

## Parallelizable

- Tasks 1 and 2 can be done in parallel (Rust types + UniFFI definition)
- Tasks 5-7 can be done in parallel after Task 4
