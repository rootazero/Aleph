# Tasks: Add Cowork Multi-Model Router

## 1. Core Data Structures

- [x] 1.1 Create `cowork/model_router/mod.rs` with module exports
- [x] 1.2 Implement `ModelProfile` struct in `profiles.rs`
  - [x] 1.2.1 Define `Capability` enum with all capability types
  - [x] 1.2.2 Define `CostTier` enum (Free, Low, Medium, High)
  - [x] 1.2.3 Define `LatencyTier` enum (Fast, Medium, Slow)
  - [x] 1.2.4 Implement Serialize/Deserialize for all types
  - [x] 1.2.5 Add unit tests for profile parsing
- [x] 1.3 Implement `ModelRoutingRules` struct
  - [x] 1.3.1 Define `CostStrategy` enum
  - [x] 1.3.2 Implement task_type_mappings HashMap
  - [x] 1.3.3 Implement capability_mappings HashMap
  - [x] 1.3.4 Add validation for rule references
- [x] 1.4 Write unit tests for data structures

## 2. Configuration Integration

- [x] 2.1 Extend `cowork.rs` config types
  - [x] 2.1.1 Add `model_profiles` section to CoworkConfig
  - [x] 2.1.2 Add `model_routing` section to CoworkConfig
  - [x] 2.1.3 Implement default config values
- [x] 2.2 Implement config parsing in `config.rs`
  - [x] 2.2.1 Parse model profiles from TOML
  - [x] 2.2.2 Parse routing rules from TOML
  - [x] 2.2.3 Validate profile references in rules
  - [x] 2.2.4 Handle missing optional fields
- [x] 2.3 Add configuration validation
  - [x] 2.3.1 Validate provider exists for each profile
  - [x] 2.3.2 Validate model ID uniqueness
  - [x] 2.3.3 Validate default_model exists
- [x] 2.4 Update default config.toml template (via serde defaults)
- [x] 2.5 Write integration tests for config loading

## 3. ModelMatcher Implementation

- [x] 3.1 Define `ModelRouter` trait in `matcher.rs`
  - [x] 3.1.1 Define `route(&self, task: &Task) -> Result<ModelProfile>`
  - [x] 3.1.2 Define `get_profile(&self, id: &str) -> Option<&ModelProfile>`
  - [x] 3.1.3 Define `profiles(&self) -> &[ModelProfile]`
  - [x] 3.1.4 Define `supports_capability(&self, profile_id: &str, capability: &Capability) -> bool`
- [x] 3.2 Implement `ModelMatcher` struct
  - [x] 3.2.1 Store profiles in HashMap by ID
  - [x] 3.2.2 Store routing rules
  - [x] 3.2.3 Implement profile lookup cache (capability_index)
- [x] 3.3 Implement routing logic
  - [x] 3.3.1 Match by task type first
  - [x] 3.3.2 Fall back to capability matching
  - [x] 3.3.3 Apply cost strategy for tie-breaking
  - [x] 3.3.4 Use default model as final fallback
- [x] 3.4 Implement capability-based routing
  - [x] 3.4.1 `find_best_for(capability: Capability) -> Option<ModelProfile>`
  - [x] 3.4.2 `find_balanced() -> Option<ModelProfile>`
  - [x] 3.4.3 `find_cheapest_with(capability: Capability) -> Option<ModelProfile>`
- [x] 3.5 Add routing hints from Task
  - [x] 3.5.1 Check `task.model_preference` for override
  - [x] 3.5.2 Check `task.requires_privacy` flag
  - [x] 3.5.3 Check `task.has_images` flag
  - [x] 3.5.4 Check `task.context_length` for long context
- [x] 3.6 Write unit tests for ModelMatcher
  - [x] 3.6.1 Test task type routing
  - [x] 3.6.2 Test capability routing
  - [x] 3.6.3 Test cost strategy
  - [x] 3.6.4 Test fallback behavior

## 4. Pipeline Executor

- [x] 4.1 Define pipeline types in `pipeline.rs`
  - [x] 4.1.1 Define `PipelineStage` struct
  - [x] 4.1.2 Define `PipelineContext` struct
  - [x] 4.1.3 Define `StageResult` struct
- [x] 4.2 Implement `PipelineExecutor`
  - [x] 4.2.1 Accept `ModelRouter` dependency
  - [x] 4.2.2 Store `ProviderAdapter` for execution
- [x] 4.3 Implement `execute_pipeline()` method
  - [x] 4.3.1 Iterate through stages with topological sort
  - [x] 4.3.2 Route each stage to optimal model
  - [x] 4.3.3 Enrich task with context from dependencies
  - [x] 4.3.4 Execute with selected provider
  - [x] 4.3.5 Store result in context
  - [x] 4.3.6 Accumulate tokens and cost
- [x] 4.4 Implement provider execution wrapper
  - [x] 4.4.1 ProviderAdapter trait for abstraction
  - [x] 4.4.2 Execute with proper model parameter
  - [x] 4.4.3 Handle provider errors
  - [x] 4.4.4 Track token usage
- [x] 4.5 Add pipeline control features
  - [x] 4.5.1 Support stage cancellation
  - [x] 4.5.2 Support pause/resume
  - [x] 4.5.3 Emit progress events via PipelineProgressHandler
- [x] 4.6 Write integration tests for pipeline execution

## 5. Memory Integration

- [x] 5.1 Implement `TaskContextManager` in `context.rs`
  - [x] 5.1.1 Accept optional VectorDatabase dependency
  - [x] 5.1.2 Track current graph ID
- [x] 5.2 Implement `store_result()` method
  - [x] 5.2.1 Create StoredTaskResult with metadata
  - [x] 5.2.2 Serialize task result to JSON
  - [x] 5.2.3 Add metadata (model, provider, timestamp, tokens)
  - [x] 5.2.4 Store in-memory with optional persistence
- [x] 5.3 Implement `get_context()` method
  - [x] 5.3.1 Query in-memory results for dependencies
  - [x] 5.3.2 Build TaskContext from results
  - [x] 5.3.3 Handle missing dependencies gracefully
- [x] 5.4 Implement context enrichment
  - [x] 5.4.1 `enrich_task(task: &Task, dependencies: &[String]) -> Task`
  - [x] 5.4.2 Inject dependency outputs into task parameters
  - [x] 5.4.3 Truncate context if exceeds max_context_size
- [x] 5.5 Write integration tests (12 unit tests)

## 6. CoworkEngine Integration

- [x] 6.1 Add ModelRouter to CoworkEngine
  - [x] 6.1.1 Initialize ModelMatcher from config
  - [x] 6.1.2 Store provider reference for execution
- [x] 6.2 Update task execution flow
  - [x] 6.2.1 Route AiInference tasks through ModelRouter
  - [x] 6.2.2 Pass selected profile to provider
  - [x] 6.2.3 Track model usage in TaskResult
- [x] 6.3 Implement multi-model task graph execution
  - [x] 6.3.1 execute_with_routing() method
  - [x] 6.3.2 TaskContextManager for tracking results
  - [x] 6.3.3 Aggregate results with context summary
- [x] 6.4 Add model selection helpers
  - [x] 6.4.1 route_task() method
  - [x] 6.4.2 model_profiles() getter
- [x] 6.5 Write end-to-end tests (5 new tests)

## 7. UniFFI Exports

- [x] 7.1 Add model router types to aleph.udl
  - [x] 7.1.1 Export ModelProfile
  - [x] 7.1.2 Export Capability enum
  - [x] 7.1.3 Export CostTier and LatencyTier enums
  - [x] 7.1.4 Export StageResult
- [x] 7.2 Add model router functions to aleph.udl
  - [x] 7.2.1 `get_model_profiles() -> Vec<ModelProfile>`
  - [x] 7.2.2 `get_routing_rules() -> ModelRoutingRules`
  - [x] 7.2.3 `update_model_profile(profile: ModelProfile) -> Result<()>`
  - [x] 7.2.4 `update_routing_rule(task_type: String, model_id: String) -> Result<()>`
- [x] 7.3 Regenerate Swift bindings
- [x] 7.4 Test bindings from Swift

## 8. Settings UI (Swift)

- [ ] 8.1 Create ModelProfilesSettingsView
  - [ ] 8.1.1 List all configured model profiles
  - [ ] 8.1.2 Show capabilities for each profile
  - [ ] 8.1.3 Show cost/latency tiers
  - [ ] 8.1.4 Edit profile button
- [ ] 8.2 Create ModelProfileEditSheet
  - [ ] 8.2.1 Provider picker
  - [ ] 8.2.2 Model name input
  - [ ] 8.2.3 Capability multi-select
  - [ ] 8.2.4 Cost/latency tier pickers
  - [ ] 8.2.5 Max context input
  - [ ] 8.2.6 Local toggle
- [ ] 8.3 Create ModelRoutingSettingsView
  - [ ] 8.3.1 Task type to model mapping list
  - [ ] 8.3.2 Edit each mapping
  - [ ] 8.3.3 Cost strategy picker
  - [ ] 8.3.4 Default model picker
  - [ ] 8.3.5 Enable pipelines toggle
- [ ] 8.4 Integrate into CoworkSettingsView
  - [ ] 8.4.1 Add "Model Routing" section
  - [ ] 8.4.2 Navigation to profiles and routing views
- [ ] 8.5 Add localization strings
  - [ ] 8.5.1 English strings
  - [ ] 8.5.2 Chinese strings

## 9. Documentation

- [ ] 9.1 Update docs/COWORK.md with model routing section
- [ ] 9.2 Update docs/CONFIGURATION.md with new config options
- [ ] 9.3 Add model routing examples to config.toml template

## 10. Testing

- [ ] 10.1 Unit tests
  - [ ] 10.1.1 ModelProfile parsing
  - [ ] 10.1.2 ModelMatcher routing logic
  - [ ] 10.1.3 PipelineExecutor flow
  - [ ] 10.1.4 TaskContextManager
- [ ] 10.2 Integration tests
  - [ ] 10.2.1 Config loading with model profiles
  - [ ] 10.2.2 End-to-end pipeline execution
  - [ ] 10.2.3 Memory integration
- [ ] 10.3 Manual testing
  - [ ] 10.3.1 Settings UI functionality
  - [ ] 10.3.2 Multi-model task execution
  - [ ] 10.3.3 Cost tracking accuracy
