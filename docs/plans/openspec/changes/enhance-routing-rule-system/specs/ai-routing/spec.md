# ai-routing Specification Delta

## Purpose
This spec delta modifies the `ai-routing` specification to support context-aware routing with window context, clear system prompt priority, and explicit first-match-stops logic.

## MODIFIED Requirements

### Requirement: Pattern Matching
The system SHALL match input against rules using regex on combined context string.

#### Scenario: Match combined context string
- **WHEN** routing is triggered with window context and clipboard content
- **THEN** router builds context string as `[AppName] WindowTitle\nClipboardContent`
- **AND** matches this combined string against all rules
- **AND** rules can match window context, clipboard, or both

#### Scenario: First-match priority (UPDATED)
- **WHEN** multiple rules match the context
- **THEN** first matching rule in config order is selected
- **AND** subsequent rules are ignored (first-match-stops)
- **AND** rule order is critical for user
- **AND** this behavior is logged for debugging

#### Scenario: Match window context prefix
- **WHEN** rule regex is `^\\[VSCode\\]`
- **AND** context is `[VSCode] main.rs\nfn main() {}`
- **THEN** rule matches
- **AND** provider from rule is selected

#### Scenario: Match clipboard content
- **WHEN** rule regex is `/translate`
- **AND** context is `[Notes] Doc.txt\n/translate hello`
- **THEN** rule matches
- **AND** `/translate` is found in clipboard portion

#### Scenario: Match hybrid pattern
- **WHEN** rule regex is `^\\[Notes\\].*TODO`
- **AND** context is `[Notes] Tasks.txt\nTODO: Finish project`
- **THEN** rule matches window AND content pattern
- **AND** provider from rule is selected

#### Scenario: Empty window context
- **WHEN** window title is empty
- **AND** context is `[AppName] \nClipboard content`
- **THEN** rules can still match `[AppName]` or clipboard
- **AND** routing continues normally

#### Scenario: Empty clipboard
- **WHEN** clipboard is empty
- **AND** context is `[Notes] Document.txt\n`
- **THEN** rules can match window context only
- **AND** routing continues normally

### Requirement: Provider Selection (UPDATED)
The system SHALL return the appropriate provider and system prompt for matched input.

#### Scenario: Return provider with rule's system prompt
- **WHEN** `router.route(context)` matches a rule
- **AND** rule has `system_prompt` field defined
- **THEN** return `(&dyn AiProvider, Some(rule.system_prompt))`
- **AND** rule's prompt overrides provider's default prompt

#### Scenario: Return provider without custom prompt
- **WHEN** `router.route(context)` matches a rule
- **AND** rule has NO `system_prompt` field
- **THEN** return `(&dyn AiProvider, None)`
- **AND** provider will use its own default prompt

#### Scenario: Use default provider on no match (CLARIFIED)
- **WHEN** no rule matches the context
- **AND** default provider is configured
- **THEN** default provider is returned
- **AND** system prompt is `None` (provider uses its default)

#### Scenario: No provider available
- **WHEN** no rule matches
- **AND** no default provider is configured
- **THEN** `route()` returns `None`
- **AND** caller receives `AlephError::NoProviderAvailable`

### Requirement: Router API (UPDATED)
The system SHALL provide clean API for routing operations with context awareness.

#### Scenario: Route context to provider
- **WHEN** calling `router.route(&context)`
- **AND** context is formatted as `[AppName] WindowTitle\nClipboardContent`
- **THEN** method is fast (O(n) with n = number of rules)
- **AND** regex matching completes in < 1ms for typical workloads
- **AND** no allocations during routing

#### Scenario: Log routing decision
- **WHEN** routing completes
- **THEN** log matched rule index (or "default" if fallback)
- **AND** log selected provider name
- **AND** log whether custom system prompt is used
- **AND** log context length (not full content for privacy)

## ADDED Requirements

### Requirement: Context String Format
The system SHALL format context string consistently for routing.

#### Scenario: Build context from window and clipboard
- **WHEN** `build_routing_context()` is called
- **AND** window context has `bundle_id` and `window_title`
- **AND** clipboard has content string
- **THEN** extract app name from bundle ID (e.g., "com.apple.Notes" → "Notes")
- **AND** format as `[AppName] WindowTitle\nClipboardContent`
- **AND** return formatted string

#### Scenario: Handle missing window title
- **WHEN** window title is empty string
- **THEN** format as `[AppName] \nClipboardContent`
- **AND** space after `]` is preserved for consistency

#### Scenario: Handle unknown app
- **WHEN** bundle ID is empty or invalid
- **THEN** use `[Unknown]` as app name
- **AND** routing continues normally

### Requirement: System Prompt Priority
The system SHALL enforce clear priority order for system prompts.

#### Scenario: Rule prompt overrides provider default
- **WHEN** matched rule has `system_prompt` defined
- **AND** provider has default system prompt
- **THEN** use rule's prompt
- **AND** provider's default is ignored

#### Scenario: Provider default when rule has no prompt
- **WHEN** matched rule has NO `system_prompt`
- **AND** provider has default system prompt
- **THEN** return `None` from router
- **AND** provider uses its own default when processing

#### Scenario: No system prompt anywhere
- **WHEN** matched rule has NO `system_prompt`
- **AND** provider has NO default system prompt
- **THEN** return `None` from router
- **AND** AI request has no system message

### Requirement: Rule Order Management
The system SHALL provide API methods to manage rule order.

#### Scenario: Add rule at top
- **WHEN** `Config::add_rule_at_top(rule)` is called
- **THEN** rule is inserted at index 0
- **AND** existing rules shift down
- **AND** new rule has highest priority

#### Scenario: Remove rule by index
- **WHEN** `Config::remove_rule(index)` is called
- **AND** index is valid (< rules.len())
- **THEN** rule at index is removed
- **AND** subsequent rules shift up

#### Scenario: Move rule position
- **WHEN** `Config::move_rule(from, to)` is called
- **AND** both indices are valid
- **THEN** rule at `from` is moved to `to`
- **AND** other rules shift accordingly

#### Scenario: Get rule by index
- **WHEN** `Config::get_rule(index)` is called
- **THEN** return `Some(&RoutingRuleConfig)` if exists
- **AND** return `None` if index out of bounds

### Requirement: Enhanced Configuration Validation
The system SHALL warn about missing configuration that affects routing.

#### Scenario: Warn about missing default provider
- **WHEN** `Config::validate()` is called
- **AND** `general.default_provider` is `None`
- **THEN** log warning: "No default_provider configured"
- **AND** continue validation (not an error)

#### Scenario: Warn about empty rules
- **WHEN** `Config::validate()` is called
- **AND** `rules` array is empty
- **THEN** log warning: "No routing rules configured"
- **AND** note all requests will use default provider

## REMOVED Requirements

None - all existing requirements are preserved or enhanced.

## Relationships

### Depends On
- **context-capture**: Window context must be captured before routing
- **clipboard-management**: Clipboard content must be available

### Affects
- **core-library**: `AlephCore::process_clipboard()` must build context string
- **memory-augmentation**: Memory retrieval happens AFTER routing (order matters)

### Future Extensions
- **Semantic routing**: Use embeddings for context-aware matching
- **Conditional rules**: AND/OR logic for complex patterns
- **Rule groups**: Organize rules by category
