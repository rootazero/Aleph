# intent-routing-pipeline Specification

## Purpose

Define requirements for a unified intent routing pipeline that improves AI tool invocation accuracy, reduces latency through caching, and provides seamless clarification flows.

## ADDED Requirements

### Requirement: Pipeline Architecture

The system SHALL implement a multi-layer intent routing pipeline.

#### Scenario: Pipeline initialization
- **WHEN** AetherCore initializes with `[routing.pipeline].enabled = true`
- **THEN** IntentRoutingPipeline is created with all components
- **AND** IntentCache is initialized with configured capacity
- **AND** ConfidenceCalibrator loads tool-specific configs
- **AND** LayerExecutionEngine is ready for routing

#### Scenario: Pipeline disabled fallback
- **WHEN** `[routing.pipeline].enabled = false`
- **THEN** existing DispatcherIntegration is used for routing
- **AND** no pipeline components are initialized
- **AND** behavior matches pre-pipeline implementation

### Requirement: Intent Signal Collection

The system SHALL collect intent signals from multiple routing layers.

#### Scenario: L1 regex signal generation
- **WHEN** user input starts with command prefix (e.g., `/search`)
- **THEN** L1 layer generates IntentSignal with confidence 1.0
- **AND** signal includes matched tool and extracted content
- **AND** signal latency is less than 10ms

#### Scenario: L2 semantic signal generation
- **WHEN** user input contains keywords matching tool capabilities
- **THEN** L2 layer generates IntentSignal with confidence 0.5-0.9
- **AND** signal includes matched keywords list
- **AND** signal latency is 200-500ms

#### Scenario: L3 AI inference signal generation
- **WHEN** L1 and L2 produce low-confidence signals
- **THEN** L3 layer invokes AI provider for routing inference
- **AND** AI returns tool, confidence, and parameters as JSON
- **AND** signal includes AI reasoning
- **AND** signal respects configured timeout

#### Scenario: Empty signal handling
- **WHEN** no layer produces a match signal
- **THEN** pipeline returns GeneralChat action
- **AND** input is routed to default AI provider
- **AND** no tool is executed

### Requirement: Intent Cache

The system SHALL cache successful intent matches for fast path routing.

#### Scenario: Cache hit fast path
- **WHEN** normalized input hash matches cache entry
- **AND** cached confidence with decay is greater than or equal to cache_auto_execute_threshold
- **THEN** cached intent is used without layer execution
- **AND** cache hit count is incremented
- **AND** response latency is less than 50ms

#### Scenario: Cache time decay
- **WHEN** cache entry is accessed
- **THEN** confidence is decayed based on age
- **AND** decay formula uses exponential decay with configurable half-life
- **AND** entries below threshold are not used for fast path

#### Scenario: Cache success recording
- **WHEN** tool execution completes successfully
- **AND** user did not cancel
- **THEN** cache entry success_count is incremented
- **AND** confidence may be boosted for future lookups

#### Scenario: Cache failure recording
- **WHEN** user cancels tool execution
- **OR** tool execution fails
- **THEN** cache entry failure_count is incremented
- **AND** entries with high failure rate are evicted

#### Scenario: Cache eviction
- **WHEN** cache size exceeds max_size
- **THEN** least recently used entries are evicted
- **AND** eviction count is tracked in metrics

### Requirement: Confidence Calibration

The system SHALL calibrate raw confidence scores for consistent behavior.

#### Scenario: Layer-specific calibration
- **WHEN** IntentSignal is received from a layer
- **THEN** calibration applies layer-specific adjustment
- **AND** L1 signals are not adjusted as they are already calibrated
- **AND** L2 signals receive slight dampening for non-exact matches
- **AND** L3 signals receive model-specific correction

#### Scenario: Tool-specific calibration
- **WHEN** signal matches a tool with custom config
- **THEN** tool min_threshold is applied
- **AND** tool auto_execute_threshold is used for action
- **AND** unconfigured tools use global thresholds

#### Scenario: Context-based boost
- **WHEN** matched tool was used recently in conversation
- **THEN** confidence receives small boost of 0.05 per use
- **AND** boost is capped at 0.15 maximum

#### Scenario: History-based boost
- **WHEN** similar input pattern succeeded previously
- **THEN** confidence receives boost based on success rate
- **AND** boost is proportional to historical success rate

### Requirement: Intent Aggregation

The system SHALL aggregate signals into a single routing decision.

#### Scenario: Single signal aggregation
- **WHEN** only one layer produces a signal
- **THEN** that signal becomes the primary intent
- **AND** no alternatives are listed

#### Scenario: Multiple signal aggregation
- **WHEN** multiple layers produce signals
- **THEN** signals are sorted by calibrated confidence
- **AND** highest confidence becomes primary intent
- **AND** others become alternatives

#### Scenario: Conflict detection
- **WHEN** multiple signals match different tools
- **AND** both have confidence greater than 0.7
- **THEN** conflict flag is set
- **AND** action requires confirmation regardless of confidence

#### Scenario: Action determination Execute
- **WHEN** calibrated confidence is greater than or equal to auto_execute_threshold
- **AND** no conflict detected
- **AND** parameters are complete
- **THEN** action is Execute

#### Scenario: Action determination RequestConfirmation
- **WHEN** calibrated confidence is between requires_confirmation and auto_execute thresholds
- **OR** conflict detected
- **THEN** action is RequestConfirmation

#### Scenario: Action determination RequestClarification
- **WHEN** matched tool has required parameters
- **AND** parameters are missing from input
- **THEN** action is RequestClarification
- **AND** clarification prompt is generated for first missing param

#### Scenario: Action determination GeneralChat
- **WHEN** calibrated confidence is below no_match threshold
- **OR** no tool matched
- **THEN** action is GeneralChat

### Requirement: Parameter Completeness

The system SHALL verify parameter completeness before execution.

#### Scenario: Complete parameters
- **WHEN** tool has required parameters in schema
- **AND** all required parameters are provided
- **THEN** parameters_complete is true
- **AND** no clarification is needed

#### Scenario: Missing required parameter
- **WHEN** tool has required parameter in schema
- **AND** parameter is not provided in input
- **THEN** parameters_complete is false
- **AND** missing_parameters list includes the parameter
- **AND** action is RequestClarification

#### Scenario: Clarification prompt generation
- **WHEN** parameter is missing
- **THEN** clarification prompt uses parameter description
- **AND** suggestions may be included if available

### Requirement: Clarification Flow

The system SHALL handle clarification without losing context.

#### Scenario: Start clarification
- **WHEN** action is RequestClarification
- **THEN** session ID is generated
- **AND** original context is preserved
- **AND** ClarificationRequest is sent to UI
- **AND** pipeline returns PendingClarification

#### Scenario: Resume with user input
- **WHEN** user provides clarification input
- **AND** session exists and not expired
- **THEN** original context is restored
- **AND** input augments missing parameter
- **AND** routing continues without re-running layers

#### Scenario: Clarification timeout
- **WHEN** user does not respond within timeout_seconds
- **THEN** session is expired
- **AND** resume returns ClarificationError Timeout
- **AND** expired session is cleaned up

#### Scenario: Multiple missing parameters
- **WHEN** tool has multiple missing parameters
- **THEN** clarification is requested for first missing
- **AND** after resume remaining parameters are checked
- **AND** process repeats until all parameters provided

### Requirement: Layer Execution Modes

The system SHALL support multiple layer execution strategies.

#### Scenario: Sequential mode
- **WHEN** execution_mode is sequential
- **THEN** L2 runs after L1
- **AND** L3 runs only if L2 confidence is below l2_skip_l3_threshold
- **AND** total latency is sum of executed layers

#### Scenario: Parallel mode
- **WHEN** execution_mode is parallel
- **THEN** L2 and L3 run concurrently after L1
- **AND** total latency is max of L2 and L3
- **AND** both signals are aggregated

#### Scenario: L1-only mode
- **WHEN** execution_mode is l1_only
- **THEN** only L1 regex matching is performed
- **AND** non-command inputs return GeneralChat
- **AND** latency is less than 10ms

#### Scenario: L1 early exit
- **WHEN** L1 produces signal with confidence greater than or equal to l1_auto_accept
- **THEN** L2 and L3 are skipped
- **AND** aggregation uses L1 signal only
- **AND** latency is minimal

### Requirement: Tool Execution

The system SHALL execute tools based on aggregated intent.

#### Scenario: Direct execution
- **WHEN** action is Execute
- **THEN** tool is invoked with extracted parameters
- **AND** result is returned to caller
- **AND** success is recorded to cache

#### Scenario: Confirmed execution
- **WHEN** action is RequestConfirmation
- **AND** user confirms via UI
- **THEN** tool is invoked
- **AND** success is recorded to cache

#### Scenario: Cancelled execution
- **WHEN** user cancels confirmation
- **THEN** tool is not invoked
- **AND** failure is recorded to cache
- **AND** pipeline returns Cancelled

### Requirement: Metrics and Observability

The system SHALL emit metrics for monitoring and tuning.

#### Scenario: Cache metrics
- **WHEN** cache operations occur
- **THEN** hit_count miss_count eviction_count are tracked
- **AND** hit_rate percentage is calculable
- **AND** metrics are exportable

#### Scenario: Latency metrics
- **WHEN** routing completes
- **THEN** per-layer latency is recorded
- **AND** total pipeline latency is recorded
- **AND** histogram distribution is maintained

#### Scenario: Confidence metrics
- **WHEN** aggregation completes
- **THEN** final confidence is recorded
- **AND** calibration factors are logged
- **AND** confidence distribution is tracked

#### Scenario: L3 reduction metrics
- **WHEN** routing completes
- **THEN** whether L3 was invoked is recorded
- **AND** L3 call rate is calculable
- **AND** target is less than 50 percent reduction vs baseline

### Requirement: Configuration

The system SHALL be configurable via TOML.

#### Scenario: Pipeline enable disable
- **WHEN** routing.pipeline.enabled changes
- **THEN** pipeline is enabled or disabled accordingly
- **AND** no restart required for runtime toggle

#### Scenario: Cache configuration
- **WHEN** routing.pipeline.cache section is present
- **THEN** cache uses configured max_size
- **AND** cache uses configured ttl_seconds
- **AND** cache uses configured decay_half_life_seconds

#### Scenario: Confidence thresholds
- **WHEN** routing.pipeline.confidence section is present
- **THEN** no_match threshold is applied
- **AND** requires_confirmation threshold is applied
- **AND** auto_execute threshold is applied

#### Scenario: Tool-specific overrides
- **WHEN** routing.pipeline.tools section is present for a tool
- **THEN** tool uses custom min_threshold
- **AND** tool uses custom auto_execute_threshold
- **AND** tool uses custom repeat_boost setting
