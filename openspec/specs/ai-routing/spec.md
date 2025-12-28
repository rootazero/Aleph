# ai-routing Specification

## Purpose
TBD - created by archiving change integrate-ai-providers. Update Purpose after archive.
## Requirements
### Requirement: Routing Rule Definition
The system SHALL define routing rules that map input patterns to AI providers.

#### Scenario: Define rule with regex pattern
- **WHEN** routing rule is created
- **THEN** rule includes `regex` field with valid regex pattern
- **AND** rule includes `provider` field with provider name
- **AND** rule optionally includes `system_prompt` override
- **AND** regex is compiled once at initialization

#### Scenario: Load rules from config
- **WHEN** loading `[[rules]]` array from config.toml
- **THEN** each rule is parsed into `RoutingRule` struct
- **AND** rules are stored in order (first-match priority)
- **AND** invalid regex causes `AetherError::InvalidConfig`

#### Scenario: Validate rule provider references
- **WHEN** rule references a provider name
- **THEN** provider must exist in `[providers]` section
- **AND** missing provider causes `AetherError::InvalidConfig`
- **AND** error message lists available providers

### Requirement: Router Initialization
The system SHALL initialize router with providers and rules.

#### Scenario: Create router with providers
- **WHEN** `Router::new(providers, rules)` is called
- **THEN** providers are stored in HashMap by name
- **AND** rules are stored in Vec preserving order
- **AND** all regex patterns are pre-compiled
- **AND** initialization validates all references

#### Scenario: Set default provider
- **WHEN** config includes `default_provider = "openai"`
- **THEN** default provider is stored in router
- **AND** default is used when no rule matches
- **AND** missing default provider is allowed (returns error on no match)

### Requirement: Pattern Matching
The system SHALL match input against rules using regex.

#### Scenario: First-match priority
- **WHEN** multiple rules match the input
- **THEN** first matching rule in config order is selected
- **AND** subsequent rules are ignored
- **AND** rule order is critical for user

#### Scenario: Exact prefix matching
- **WHEN** rule regex is `^/code`
- **AND** input is `/code write a function`
- **THEN** rule matches
- **AND** provider from rule is selected

#### Scenario: Case-insensitive matching
- **WHEN** rule regex is `(?i)^/draw`
- **AND** input is `/DRAW a picture`
- **THEN** rule matches
- **AND** case is ignored

#### Scenario: Catch-all fallback rule
- **WHEN** last rule regex is `.*`
- **THEN** it matches any input
- **AND** acts as default if no earlier rule matches
- **AND** common pattern for fallback behavior

#### Scenario: No matching rule
- **WHEN** no rule matches the input
- **AND** no default provider is configured
- **THEN** `route()` returns `None`
- **AND** caller receives `AetherError::NoProviderAvailable`

### Requirement: Provider Selection
The system SHALL return the appropriate provider for matched input.

#### Scenario: Return provider and system prompt
- **WHEN** `router.route("input")` is called
- **THEN** return type is `Option<(&dyn AiProvider, Option<&str>)>`
- **AND** provider reference is from registered providers
- **AND** system prompt is from matched rule (if specified)

#### Scenario: Use default provider on no match
- **WHEN** no rule matches
- **AND** default provider is configured
- **THEN** default provider is returned
- **AND** system prompt is `None`

#### Scenario: Override system prompt per rule
- **WHEN** rule includes `system_prompt = "You are a code expert"`
- **THEN** matched rule returns this system prompt
- **AND** provider's default system prompt is ignored
- **AND** prompt is passed to `AiProvider::process()`

### Requirement: Router API
The system SHALL provide clean API for routing operations.

#### Scenario: Route input to provider
- **WHEN** calling `router.route(&input)`
- **THEN** method is fast (O(n) with n = number of rules)
- **AND** regex matching is efficient
- **AND** no allocations during routing

#### Scenario: Query available providers
- **WHEN** calling `router.providers()`
- **THEN** returns iterator of provider names
- **AND** names are sorted alphabetically
- **AND** useful for debugging and UI

#### Scenario: Get provider by name
- **WHEN** calling `router.get_provider("openai")`
- **THEN** returns `Option<&dyn AiProvider>`
- **AND** None if provider not registered
- **AND** useful for testing and debugging

### Requirement: Rule Testing Utility
The system SHALL provide utility to test rules against sample inputs.

#### Scenario: Test input against rules
- **WHEN** calling `router.test_match(&input)`
- **THEN** returns matched rule index and provider name
- **AND** returns None if no match
- **AND** useful for debugging regex patterns

#### Scenario: List all rules
- **WHEN** calling `router.rules()`
- **THEN** returns slice of all rules
- **AND** rules are in original config order
- **AND** useful for UI display

### Requirement: Error Handling
The system SHALL handle routing errors gracefully.

#### Scenario: Invalid regex in config
- **WHEN** rule regex is syntactically invalid
- **THEN** `Router::new()` returns `AetherError::InvalidConfig`
- **AND** error message includes regex pattern
- **AND** error message includes regex parse error

#### Scenario: Provider not found
- **WHEN** rule references non-existent provider
- **THEN** `Router::new()` returns `AetherError::InvalidConfig`
- **AND** error message lists available providers
- **AND** error message includes rule pattern

#### Scenario: No providers configured
- **WHEN** router is created with empty provider map
- **THEN** initialization succeeds
- **AND** all route attempts return None
- **AND** warning is logged

### Requirement: Performance Optimization
The system SHALL optimize routing performance for common cases.

#### Scenario: Pre-compile regex patterns
- **WHEN** router is initialized
- **THEN** all regex patterns are compiled once
- **AND** compiled Regex objects are stored
- **AND** no regex compilation during routing

#### Scenario: Fast path for exact prefix
- **WHEN** rule uses simple prefix pattern like `^/code`
- **THEN** consider optimizing with string prefix check
- **AND** fallback to regex if prefix matches
- **AND** optimization is transparent to user

#### Scenario: Benchmark routing performance
- **WHEN** routing with 10 rules
- **THEN** routing completes in <1ms
- **AND** no significant GC pressure
- **AND** scales linearly with rule count

### Requirement: Configuration Validation
The system SHALL validate routing configuration at load time.

#### Scenario: Detect duplicate provider names
- **WHEN** config has duplicate `[providers.name]` sections
- **THEN** error type is `AetherError::InvalidConfig`
- **AND** error message lists duplicate names

#### Scenario: Validate required rule fields
- **WHEN** rule is missing `regex` or `provider` field
- **THEN** error type is `AetherError::InvalidConfig`
- **AND** error message indicates missing field

#### Scenario: Warn on unreachable rules
- **WHEN** rule after catch-all `.*` pattern exists
- **THEN** warning is logged (not error)
- **AND** message indicates rule will never match
- **AND** user is advised to reorder rules

### Requirement: Logging and Debugging
The system SHALL log routing decisions for debugging.

#### Scenario: Log routing decision
- **WHEN** input is routed to provider
- **THEN** log includes matched pattern, provider name
- **AND** log includes input length (not full input for privacy)
- **AND** log level is DEBUG

#### Scenario: Log no match
- **WHEN** no rule matches input
- **THEN** log indicates no match found
- **AND** log includes input length
- **AND** log level is WARNING

#### Scenario: Log rule evaluation
- **WHEN** input is tested against rules
- **THEN** log includes number of rules evaluated
- **AND** log includes matched rule index
- **AND** useful for performance profiling

### Requirement: Memory Integration
The system SHALL work with memory module for context-aware routing.

#### Scenario: Route with context
- **WHEN** routing considers past context
- **THEN** context is retrieved before routing
- **AND** context is appended to input for matching
- **AND** routing sees full context-augmented input

#### Scenario: Route original input only
- **WHEN** memory module is disabled
- **THEN** routing sees only original user input
- **AND** no context retrieval occurs
- **AND** routing is faster

### Requirement: Future Extensibility
The system SHALL support future routing enhancements.

#### Scenario: Placeholder for semantic routing
- **WHEN** future semantic routing is added
- **THEN** Router trait can support additional routing strategies
- **AND** regex-based routing remains default
- **AND** strategies can be composed

#### Scenario: Placeholder for dynamic rules
- **WHEN** future hot-reload is added
- **THEN** router can reload rules without restart
- **AND** in-flight requests use old rules
- **AND** new requests use new rules after reload

