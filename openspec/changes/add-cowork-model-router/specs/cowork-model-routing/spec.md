# cowork-model-routing Specification Delta

## ADDED Requirements

### Requirement: Model Profile Definition
The system SHALL define model profiles that describe AI model capabilities.

#### Scenario: Define model profile with capabilities
- **WHEN** model profile is created
- **THEN** profile includes unique `id` identifier
- **AND** profile includes `provider` name (anthropic, openai, google, ollama)
- **AND** profile includes `model` name for API calls
- **AND** profile includes `capabilities` list of capability tags
- **AND** profile includes `cost_tier` (free, low, medium, high)
- **AND** profile includes `latency_tier` (fast, medium, slow)
- **AND** profile optionally includes `max_context` window size
- **AND** profile optionally includes `local` flag for local models

#### Scenario: Load profiles from config
- **WHEN** loading `[cowork.model_profiles.*]` sections from config.toml
- **THEN** each section is parsed into `ModelProfile` struct
- **AND** profiles are stored in HashMap by ID
- **AND** invalid profile causes `AetherError::InvalidConfig`
- **AND** duplicate profile ID causes `AetherError::InvalidConfig`

#### Scenario: Validate profile provider references
- **WHEN** profile references a provider name
- **THEN** provider must exist in `[providers]` section
- **AND** missing provider causes `AetherError::InvalidConfig`
- **AND** error message lists available providers

### Requirement: Capability Tags
The system SHALL support capability tags for model characterization.

#### Scenario: Define standard capabilities
- **WHEN** capability tag is used
- **THEN** tag is one of predefined values:
  - `code_generation` - Code creation and completion
  - `code_review` - Code analysis and review
  - `text_analysis` - Text understanding and summarization
  - `image_understanding` - Vision/image analysis
  - `video_understanding` - Video analysis
  - `long_context` - Large context window support
  - `reasoning` - Complex reasoning tasks
  - `local_privacy` - Local execution for privacy
  - `fast_response` - Low latency response
  - `simple_task` - Simple task handling
  - `long_document` - Long document processing

#### Scenario: Assign multiple capabilities
- **WHEN** model supports multiple capabilities
- **THEN** capabilities list contains all applicable tags
- **AND** order in list indicates primary strength first

### Requirement: Model Routing Rules
The system SHALL define routing rules that map task types to model profiles.

#### Scenario: Define task type to model mapping
- **WHEN** routing rule is created
- **THEN** rule includes `task_type` identifier
- **AND** rule includes `model_profile_id` to route to
- **AND** profile ID must reference existing profile

#### Scenario: Load routing rules from config
- **WHEN** loading `[cowork.model_routing]` section
- **THEN** task type mappings are parsed
- **AND** default_model is set
- **AND** cost_strategy is set (cheapest, balanced, best_quality)
- **AND** invalid profile reference causes `AetherError::InvalidConfig`

#### Scenario: Define cost strategy
- **WHEN** cost_strategy is set
- **THEN** routing considers cost_tier in tie-breaking
- **AND** `cheapest` prefers lowest cost_tier
- **AND** `best_quality` prefers highest cost_tier
- **AND** `balanced` considers both cost and capability match

### Requirement: Model Matcher
The system SHALL implement model matching logic for task routing.

#### Scenario: Route task by type
- **WHEN** `model_matcher.route(task)` is called
- **AND** task.task_type has explicit mapping in rules
- **THEN** mapped model profile is returned
- **AND** routing completes in O(1) time

#### Scenario: Route task by capability
- **WHEN** task.task_type has no explicit mapping
- **THEN** matcher examines task requirements
- **AND** finds profile with matching capabilities
- **AND** applies cost_strategy for tie-breaking

#### Scenario: Route image task
- **WHEN** task has `has_images = true`
- **THEN** matcher finds profile with `image_understanding` capability
- **AND** returns profile like gpt-4o or gemini-pro

#### Scenario: Route privacy-sensitive task
- **WHEN** task has `requires_privacy = true`
- **THEN** matcher finds profile with `local_privacy` capability
- **AND** prefers local models (ollama)

#### Scenario: Use default model fallback
- **WHEN** no specific rule or capability match found
- **AND** default_model is configured
- **THEN** default model profile is returned

#### Scenario: Handle no match
- **WHEN** no profile matches task requirements
- **AND** no default_model is configured
- **THEN** `route()` returns `AetherError::NoModelAvailable`
- **AND** error includes task type and required capabilities

### Requirement: Pipeline Executor
The system SHALL support multi-model pipeline execution.

#### Scenario: Execute pipeline stages
- **WHEN** `pipeline_executor.execute_pipeline(stages)` is called
- **THEN** stages are executed in order
- **AND** each stage routes to appropriate model
- **AND** results are collected in `Vec<StageResult>`

#### Scenario: Pass context between stages
- **WHEN** stage depends on previous stage
- **THEN** dependency output is injected into stage input
- **AND** context is truncated if exceeds model max_context
- **AND** context injection is transparent to user

#### Scenario: Track pipeline cost
- **WHEN** pipeline completes
- **THEN** `PipelineContext` includes total tokens used
- **AND** includes estimated cost
- **AND** includes per-stage breakdown

#### Scenario: Handle stage failure
- **WHEN** stage execution fails
- **THEN** error is recorded in StageResult
- **AND** dependent stages are skipped
- **AND** partial results are returned

### Requirement: Memory Integration
The system SHALL integrate with Memory module for context persistence.

#### Scenario: Store task result in memory
- **WHEN** task completes successfully
- **THEN** result is stored in memory with key `cowork:{graph_id}:{task_id}`
- **AND** metadata includes source, task_type, timestamp
- **AND** content is serialized JSON

#### Scenario: Retrieve dependency context
- **WHEN** task has dependencies
- **THEN** context manager retrieves dependency results from memory
- **AND** builds TaskContext with all dependency outputs
- **AND** missing dependencies are logged but not fatal

#### Scenario: Clean up completed graph
- **WHEN** task graph completes
- **THEN** memory entries can be retained or cleaned based on config
- **AND** retention policy is configurable

### Requirement: Model Routing API
The system SHALL provide clean API for model routing operations.

#### Scenario: Get model profiles
- **WHEN** calling `get_model_profiles()`
- **THEN** returns Vec of all configured profiles
- **AND** profiles are sorted by ID
- **AND** useful for UI display

#### Scenario: Get routing rules
- **WHEN** calling `get_routing_rules()`
- **THEN** returns ModelRoutingRules struct
- **AND** includes all task type mappings
- **AND** includes cost_strategy and default_model

#### Scenario: Update model profile
- **WHEN** calling `update_model_profile(profile)`
- **THEN** profile is validated
- **AND** profile is persisted to config
- **AND** router is updated without restart

#### Scenario: Update routing rule
- **WHEN** calling `update_routing_rule(task_type, model_id)`
- **THEN** rule is validated
- **AND** rule is persisted to config
- **AND** router is updated without restart

### Requirement: Configuration Validation
The system SHALL validate model routing configuration at load time.

#### Scenario: Detect invalid profile reference
- **WHEN** routing rule references non-existent profile ID
- **THEN** error type is `AetherError::InvalidConfig`
- **AND** error message includes rule and available profiles

#### Scenario: Detect duplicate profile IDs
- **WHEN** config has duplicate profile IDs
- **THEN** error type is `AetherError::InvalidConfig`
- **AND** error message lists duplicate IDs

#### Scenario: Validate default model exists
- **WHEN** default_model is set
- **AND** profile with that ID doesn't exist
- **THEN** error type is `AetherError::InvalidConfig`
- **AND** error message lists available profiles

### Requirement: Settings UI
The system SHALL provide SwiftUI settings for model routing configuration.

#### Scenario: Display model profiles
- **WHEN** user opens Model Routing settings
- **THEN** all configured profiles are listed
- **AND** each shows provider, model, capabilities
- **AND** edit button opens profile editor

#### Scenario: Edit model profile
- **WHEN** user edits profile
- **THEN** form shows provider picker, model input
- **AND** capability multi-select
- **AND** cost/latency tier pickers
- **AND** save updates config and router

#### Scenario: Configure routing rules
- **WHEN** user views routing rules
- **THEN** task type to model mappings are listed
- **AND** each can be changed via picker
- **AND** cost strategy is configurable
- **AND** default model is configurable
