# ai-routing Specification Delta

## MODIFIED Requirements

### Requirement: Pattern Matching

The system SHALL match input against rules using regex with confidence scoring.

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

#### Scenario: L1 match with confidence
- **WHEN** a slash command pattern matches
- **THEN** confidence SHALL be 1.0
- **AND** routing_layer SHALL be `L1Rule`

### Requirement: Provider Selection

The system SHALL return the appropriate provider for matched input with extended metadata.

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

#### Scenario: Return extended RoutingMatch
- **WHEN** routing completes via any layer
- **THEN** RoutingMatch SHALL include `confidence` field
- **AND** RoutingMatch SHALL include `routing_layer` field
- **AND** RoutingMatch SHALL include optional `extracted_parameters`

### Requirement: Future Extensibility

The system SHALL support future routing enhancements including multi-layer routing.

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

#### Scenario: Multi-layer routing integration
- **WHEN** Dispatcher Layer is enabled
- **THEN** L1 (regex) routing integrates with existing Router
- **AND** L2 (semantic) and L3 (AI) layers extend Router capability
- **AND** confidence scoring is returned for all layers

## ADDED Requirements

### Requirement: RoutingMatch Extension

The system SHALL extend RoutingMatch with dispatcher-specific fields.

#### Scenario: Confidence field
- **GIVEN** a routing operation completes
- **WHEN** RoutingMatch is constructed
- **THEN** it SHALL include `confidence: f32` in range 0.0 - 1.0
- **AND** L1 matches SHALL have confidence 1.0
- **AND** L2/L3 matches SHALL have variable confidence

#### Scenario: Routing layer tracking
- **GIVEN** a routing operation completes
- **WHEN** RoutingMatch is constructed
- **THEN** it SHALL include `routing_layer: RoutingLayer` enum
- **AND** possible values are `L1Rule`, `L2Semantic`, `L3Inference`, `Default`

#### Scenario: Extracted parameters
- **GIVEN** L3 routing extracts parameters from natural language
- **WHEN** RoutingMatch is constructed
- **THEN** it MAY include `extracted_parameters: Option<serde_json::Value>`
- **AND** parameters follow the tool's JSON Schema

#### Scenario: Routing reason
- **GIVEN** L2 or L3 routing makes a decision
- **WHEN** RoutingMatch is constructed
- **THEN** it MAY include `routing_reason: Option<String>`
- **AND** reason explains why this tool was selected

### Requirement: RoutingLayer Enum

The system SHALL define an enum to track which routing layer produced the match.

```rust
pub enum RoutingLayer {
    L1Rule,      // Regex pattern match
    L2Semantic,  // Keyword/similarity match
    L3Inference, // LLM-based inference
    Default,     // No match, using default provider
}
```

#### Scenario: L1Rule assignment
- **WHEN** input matches a regex rule
- **THEN** routing_layer SHALL be `L1Rule`

#### Scenario: L2Semantic assignment
- **WHEN** semantic matcher finds keywords
- **THEN** routing_layer SHALL be `L2Semantic`

#### Scenario: L3Inference assignment
- **WHEN** LLM router determines intent
- **THEN** routing_layer SHALL be `L3Inference`

#### Scenario: Default assignment
- **WHEN** no layer matches and default provider is used
- **THEN** routing_layer SHALL be `Default`
