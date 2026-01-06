# Code Quality Patterns Spec Delta

## MODIFIED Requirements

### Requirement: Code Duplication Avoidance
Duplicated code patterns MUST be consolidated into shared helper functions. The system SHALL NOT contain identical logic patterns repeated more than twice.

#### Scenario: Search Provider Validation Consolidation
Given the `test_search_provider_with_config()` method in `core.rs`
When validating search provider configurations for 6 different provider types
Then a single helper function `validate_search_provider_config()` should handle all validation
And the repeated validation pattern should be replaced with helper calls
And the estimated code reduction should be 80-100 lines

#### Scenario: OpenAI Text Content Builder Extraction
Given the `build_image_request()` and `build_multimodal_request()` methods in `openai.rs`
When building text content with prepend mode logic
Then a single helper function `build_text_content()` should handle the logic
And the 4-level nested conditional should be flattened
And the estimated code reduction should be 30 lines

#### Scenario: NSAlert Creation Consolidation
Given 6+ inline NSAlert creation patterns in `AppDelegate.swift`
When displaying alerts for various conditions
Then the existing `AlertHelper.swift` should be enhanced
And all inline alert creations should use the helper
And the estimated code reduction should be 80 lines

### Requirement: Permission Code Single Source of Truth
Permission checking logic MUST exist in one location only. The system SHALL NOT duplicate permission checking code across multiple classes.

#### Scenario: IOHIDManager Logic Consolidation
Given `PermissionChecker.hasInputMonitoringViaHID()` and `PermissionManager.checkInputMonitoringViaHID()`
When checking Input Monitoring permission via HID
Then only one implementation should exist (PermissionManager)
And PermissionChecker should delegate or be removed
And the estimated code reduction should be 45 lines

#### Scenario: ContextCapture Permission Methods Removal
Given deprecated permission methods in `ContextCapture.swift`
When checking accessibility permissions
Then the deprecated methods should be removed
And callers should use PermissionChecker or PermissionManager directly
And the estimated code reduction should be 35 lines

### Requirement: Dead Code Removal
The codebase MUST NOT contain unused code branches, deprecated methods, or unreachable code. Dead code SHALL be removed during refactoring passes.

#### Scenario: Accessibility Text Reader Strategy Cleanup
Given 4 reading strategies in `AccessibilityTextReader.swift`
When only `readEntireContents` is actually used
Then the unused strategies should be removed
And the estimated code reduction should be 40 lines

#### Scenario: Deprecated API Test Removal
Given tests for deprecated `is_listening()` API
When the hotkey monitoring has moved to Swift layer
Then the deprecated tests should be removed
And the estimated test reduction should be 3 tests

### Requirement: Test Code Quality
Test suites MUST contain focused, non-redundant tests that provide value. Boilerplate tests SHALL be consolidated using parameterized testing patterns.

#### Scenario: Error Type Test Parameterization
Given 12 similar error type creation tests
When each test follows the same pattern
Then tests should be consolidated using parameterized testing
And the estimated test reduction should be 10 tests

#### Scenario: Redundant Serialization Test Removal
Given JSON serialization tests for TOML-based configuration
When the application only uses TOML format
Then JSON serialization tests should be removed
And only TOML round-trip tests should be kept
And the estimated test reduction should be 5-8 tests

### Requirement: Helper Function Extraction for Nested Logic
Deeply nested logic (3+ levels) MUST be extracted to named helper functions. The system SHALL NOT contain logic nesting deeper than 3 levels without extraction.

#### Scenario: Hot-Reload Initialization Extraction
Given duplicated router/registry initialization in `core.rs`
When both initial setup and hot-reload paths use identical logic
Then a single `initialize_router_and_registry()` helper should be extracted
And the estimated code reduction should be 45 lines

#### Scenario: Is-Empty Prompt Pattern Extraction
Given the "describe image if empty" pattern repeated across providers
When building prompts with optional user input
Then a single `format_prompt_with_input()` utility should be created
And the estimated code reduction should be 25 lines
