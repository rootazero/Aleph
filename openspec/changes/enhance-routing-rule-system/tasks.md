# Tasks: Enhance Routing Rule System

## Overview
This task list implements the routing rule system enhancements defined in the proposal. Tasks are ordered to deliver user-visible progress incrementally.

## Task Breakdown

### Phase 1: Core Routing Logic Enhancement

- [ ] **Task 1.1**: Update `Router::route()` method signature
  - **Goal**: Accept full context string instead of just clipboard content
  - **Files**: `Aether/core/src/router/mod.rs`
  - **Validation**: Signature changed to `fn route(&self, context: &str) -> Option<(&dyn AiProvider, Option<&str>)>`
  - **Estimated**: 30 min

- [ ] **Task 1.2**: Update `AetherCore::process_clipboard()` to build context string
  - **Goal**: Combine window context + clipboard content before routing
  - **Files**: `Aether/core/src/core.rs`
  - **Context Format**: `[AppName] WindowTitle\nClipboardContent`
  - **Validation**: Context string logged correctly
  - **Estimated**: 1 hour

- [ ] **Task 1.3**: Implement first-match-stops logic
  - **Goal**: Ensure routing stops at first matching rule
  - **Files**: `Aether/core/src/router/mod.rs`
  - **Validation**: Add debug logs showing which rule matched
  - **Estimated**: 30 min

- [ ] **Task 1.4**: Clarify system prompt priority
  - **Goal**: Rule's `system_prompt` overrides provider default
  - **Files**: `Aether/core/src/router/mod.rs`
  - **Validation**: Router returns correct system prompt from matched rule
  - **Estimated**: 30 min

- [ ] **Task 1.5**: Implement fallback rule logic
  - **Goal**: Use `default_provider` when no rule matches
  - **Files**: `Aether/core/src/router/mod.rs`
  - **Validation**: Fallback to default provider when no match
  - **Estimated**: 30 min

### Phase 2: Configuration and API Updates

- [ ] **Task 2.1**: Add rule management API methods
  - **Goal**: Provide methods to insert/remove/reorder rules
  - **Methods**:
    - `Config::add_rule_at_top(rule: RoutingRuleConfig)`
    - `Config::remove_rule(index: usize)`
    - `Config::move_rule(from: usize, to: usize)`
  - **Files**: `Aether/core/src/config/mod.rs`
  - **Validation**: API methods work correctly
  - **Estimated**: 1 hour

- [ ] **Task 2.2**: Update configuration validation
  - **Goal**: Warn if `default_provider` is missing
  - **Files**: `Aether/core/src/config/mod.rs`
  - **Validation**: Warning logged during `Config::validate()`
  - **Estimated**: 30 min

- [ ] **Task 2.3**: Update config documentation
  - **Goal**: Document new routing behavior and context format
  - **Files**:
    - `docs/CONFIGURATION.md` (create if not exists)
    - Comments in `Aether/core/src/router/mod.rs`
  - **Content**:
    - Context string format
    - Matching order explanation
    - System prompt priority rules
    - Fallback behavior
  - **Validation**: Documentation is clear and accurate
  - **Estimated**: 1 hour

### Phase 3: Testing and Documentation

- [ ] **Task 3.1**: Write unit tests for context matching
  - **Goal**: Test routing with combined context strings
  - **Files**: `Aether/core/src/router/mod.rs` (test module)
  - **Test Cases**:
    - Match window context prefix
    - Match clipboard content
    - Match combination of both
    - No match falls back to default
  - **Validation**: All tests pass
  - **Estimated**: 1 hour

- [ ] **Task 3.2**: Write unit tests for first-match logic
  - **Goal**: Verify first matching rule is selected
  - **Files**: `Aether/core/src/router/mod.rs` (test module)
  - **Test Cases**:
    - Multiple rules match, first one selected
    - Later rules ignored after first match
  - **Validation**: All tests pass
  - **Estimated**: 30 min

- [ ] **Task 3.3**: Write unit tests for system prompt priority
  - **Goal**: Verify rule's system prompt overrides provider default
  - **Files**: `Aether/core/src/router/mod.rs` (test module)
  - **Test Cases**:
    - Rule with `system_prompt` returns it
    - Rule without `system_prompt` returns None
    - Provider default is not used when rule has prompt
  - **Validation**: All tests pass
  - **Estimated**: 30 min

- [ ] **Task 3.4**: Write unit tests for fallback logic
  - **Goal**: Verify fallback to default provider
  - **Files**: `Aether/core/src/router/mod.rs` (test module)
  - **Test Cases**:
    - No rule matches, default provider used
    - No default provider configured, returns None
  - **Validation**: All tests pass
  - **Estimated**: 30 min

- [ ] **Task 3.5**: Write integration test for end-to-end routing
  - **Goal**: Test full flow from context capture to AI routing
  - **Files**: `Aether/core/src/core.rs` (test module)
  - **Test Cases**:
    - Simulate hotkey press with window context
    - Verify correct provider selected
    - Verify correct system prompt used
  - **Validation**: Integration test passes
  - **Estimated**: 1 hour

- [ ] **Task 3.6**: Update example configuration file
  - **Goal**: Provide clear examples of new routing behavior
  - **Files**:
    - `config.example.toml` (create if not exists)
    - `CLAUDE.md` (update configuration section)
  - **Examples**:
    - Rule matching window context (e.g., VSCode → Claude)
    - Rule matching content prefix (e.g., /draw → OpenAI)
    - Rule with custom system prompt
    - Catch-all fallback rule
  - **Validation**: Examples are clear and tested
  - **Estimated**: 1 hour

- [ ] **Task 3.7**: Manual testing in real scenarios
  - **Goal**: Verify routing works in actual usage
  - **Scenarios**:
    - Test in Notes app with code snippet
    - Test in WeChat with translation request
    - Test in VSCode with code question
    - Test with no window context
  - **Validation**: All scenarios work as expected
  - **Estimated**: 1 hour

### Phase 4: UniFFI and Swift Integration (if needed)

- [ ] **Task 4.1**: Update UniFFI interface for context passing
  - **Goal**: Ensure Swift can pass window context to Rust
  - **Files**: `Aether/core/src/aether.udl`
  - **Changes**: Check if `CapturedContext` is properly exposed
  - **Validation**: Swift can create and pass context
  - **Estimated**: 30 min

- [ ] **Task 4.2**: Update Swift context capture logic
  - **Goal**: Format window context for routing
  - **Files**: `Aether/Sources/AppDelegate.swift`
  - **Format**: `[AppName] WindowTitle\n`
  - **Validation**: Context string logged correctly
  - **Estimated**: 30 min

## Validation Checklist

Before marking this change as complete, verify:

- [x] All unit tests pass (`cargo test`)
- [x] Integration tests pass
- [x] Manual testing completed for all scenarios
- [x] Documentation updated and reviewed
- [x] Example configuration file provided
- [x] Code coverage > 80% for routing module
- [x] No breaking changes to existing configurations
- [x] Rust clippy warnings resolved (`cargo clippy`)
- [x] Swift builds successfully (`xcodebuild`)

## Dependencies

### Upstream Dependencies
- `context-capture` spec must be implemented
- `clipboard-management` spec must be working
- `ai-routing` spec must exist

### Downstream Dependencies
None - this is a core enhancement that other features can build upon

## Notes

### Context String Format Decision
After discussion, the context string format is:
```
[AppName] WindowTitle
ClipboardContent
```

Example:
```
[Notes] Project Plan.txt
Implement AI routing system
```

This format allows regex rules to match:
- Only window context: `^\[Notes\]`
- Only clipboard: (don't use `^`, match anywhere)
- Both: `^\[Notes\].*clipboard content`

### System Prompt Priority
Clear priority order:
1. Rule's `system_prompt` (highest priority)
2. Provider's default system prompt (if rule has no prompt)
3. None (if neither has prompt)

### New Rule Insertion
When user adds a new rule via Settings UI (future), it should be inserted at index 0 (top of the list) to give it highest priority.

### Regex Performance
With 10-20 rules, regex matching should complete in < 1ms. Monitor performance and consider optimization (like prefix trie) if needed in future.
