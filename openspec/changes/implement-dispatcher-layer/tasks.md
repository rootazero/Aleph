# Tasks: Implement Dispatcher Layer

## Phase 1: Foundation (UnifiedTool + Registry) ✅

### 1.1 Data Structures
- [x] 1.1.1 Create `core/src/dispatcher/mod.rs` module entry
- [x] 1.1.2 Define `ToolSource` enum in `dispatcher/types.rs`
- [x] 1.1.3 Define `UnifiedTool` struct in `dispatcher/types.rs`
- [x] 1.1.4 Define `ToolRegistry` struct in `dispatcher/registry.rs`
- [x] 1.1.5 Add `Arc<RwLock<HashMap>>` for thread-safe storage
- [x] 1.1.6 Unit tests for data structures

### 1.2 Registry Implementation
- [x] 1.2.1 Implement `ToolRegistry::new()` initialization
- [x] 1.2.2 Implement `register_native_tools()` for Search, Video
- [x] 1.2.3 Implement `register_mcp_tools()` from McpClient
- [x] 1.2.4 Implement `register_skill_tools()` from SkillsRegistry
- [x] 1.2.5 Implement `register_custom_tools()` from config rules
- [x] 1.2.6 Implement `refresh_all_tools()` aggregation
- [x] 1.2.7 Unit tests for registration

### 1.3 Query API
- [x] 1.3.1 Implement `list_all()` method
- [x] 1.3.2 Implement `list_by_source()` filter
- [x] 1.3.3 Implement `get_by_id()` lookup
- [x] 1.3.4 Implement `get_by_name()` lookup
- [x] 1.3.5 Implement `search()` fuzzy search
- [x] 1.3.6 Unit tests for query methods

### 1.4 Core Integration
- [x] 1.4.1 Add `ToolRegistry` field to `AlephCore`
- [x] 1.4.2 Initialize registry in `AlephCore::new()`
- [x] 1.4.3 Refresh registry on config reload
- [x] 1.4.4 Refresh registry on MCP connection changes
- [x] 1.4.5 Integration test for registry initialization

## Phase 2: Extended RoutingMatch ✅

### 2.1 Data Structure Extensions
- [x] 2.1.1 Define `RoutingLayer` enum in `dispatcher/types.rs` (reused from Phase 1)
- [x] 2.1.2 Add `confidence: f32` to `RoutingMatch`
- [x] 2.1.3 Add `routing_layer: RoutingLayer` to `RoutingMatch`
- [x] 2.1.4 Add `extracted_parameters: Option<Value>` to `RoutingMatch`
- [x] 2.1.5 Add `routing_reason: Option<String>` to `RoutingMatch`
- [x] 2.1.6 Update `RoutingMatch::default()` with new fields

### 2.2 L1 Confidence Integration
- [x] 2.2.1 Update `Router::match_rules()` to set confidence = 1.0 for regex match
- [x] 2.2.2 Set `routing_layer = L1Rule` for regex matches
- [x] 2.2.3 Set `routing_layer = Default` when using default provider
- [x] 2.2.4 Unit tests for L1 confidence (8 new tests)

## Phase 3: Dynamic Prompt Builder ✅

### 3.1 Prompt Builder Implementation
- [x] 3.1.1 Create `dispatcher/prompt_builder.rs`
- [x] 3.1.2 Implement `build_tool_list()` method with multiple formats
- [x] 3.1.3 Format tools as markdown/compact/xml/json list
- [x] 3.1.4 Include name, description, source, parameters
- [x] 3.1.5 Handle tool filtering (ToolFilter: active_only, source_types, exclude_ids, max_tools)
- [x] 3.1.6 Unit tests for prompt generation (16 tests)

### 3.2 L3 System Prompt Template
- [x] 3.2.1 Create `build_l3_routing_prompt()` template
- [x] 3.2.2 Dynamic tool list injection
- [x] 3.2.3 JSON output format instructions
- [x] 3.2.4 Confidence scoring guidelines (0.0-1.0 scale)
- [x] 3.2.5 `L3RoutingResponse` struct + `parse_l3_response()` helper
- [x] 3.2.6 `build_l3_routing_prompt_minimal()` for low latency
- [x] 3.2.7 `build_parameter_extraction_prompt()` for parameter extraction

## Phase 4: L2 Semantic Enhancement ✅

### 4.1 SemanticMatcher Improvements
- [x] 4.1.1 Add confidence scoring to `SemanticMatcher` (MatchResult struct extended)
- [x] 4.1.2 Return confidence based on keyword match quality
- [x] 4.1.3 Set `routing_layer = L2Semantic` for semantic matches (all layer files updated)
- [x] 4.1.4 Extract basic parameters from keywords (matched_keywords field added)
- [x] 4.1.5 Unit tests for semantic matching with confidence (777 tests pass)

## Phase 5: L3 AI Router Integration ✅

### 5.1 AiIntentDetector Integration
- [x] 5.1.1 Wire `AiIntentDetector` into routing flow (L3Router created)
- [x] 5.1.2 Call L3 when L1/L2 fail or have low confidence (route() method)
- [x] 5.1.3 Parse L3 JSON response into RoutingMatch (L3RoutingResult)
- [x] 5.1.4 Extract parameters from L3 response (extract_parameters() method)
- [x] 5.1.5 Set confidence from L3 response (L3RoutingResponse.confidence)
- [x] 5.1.6 Handle L3 timeout gracefully (tokio::time::timeout)

### 5.2 Conversation Context
- [x] 5.2.1 Inject conversation summary into L3 prompt (build_l3_context_summary())
- [x] 5.2.2 Extract mentioned entities for pronoun resolution (extract_entity_hints())
- [x] 5.2.3 Test pronoun resolution scenarios (12 new tests)
- [x] 5.2.4 Integration test for context-aware routing (L3RoutingOptions)

## Phase 6: Halo Confirmation Flow ✅

### 6.1 Confirmation Logic
- [x] 6.1.1 Create `dispatcher/confirmation.rs`
- [x] 6.1.2 Implement `should_confirm()` threshold check
- [x] 6.1.3 Build `ClarificationRequest` for tool confirmation
- [x] 6.1.4 Format tool preview (icon, name, parameters)
- [x] 6.1.5 Add "Execute", "Cancel" options

### 6.2 Response Handling
- [x] 6.2.1 Handle "Execute" response - proceed to capability
- [x] 6.2.2 Handle "Cancel" response - fallback to GeneralChat
- [x] 6.2.3 Handle timeout - abort with error event
- [x] 6.2.4 Integration test for confirmation flow (22 tests)

### 6.3 Core Flow Integration
- [x] 6.3.1 Create `dispatcher/integration.rs` with DispatcherIntegration
- [x] 6.3.2 Implement `route_with_confirmation()` method
- [x] 6.3.3 Wait for user response before execution via `on_clarification_needed()`
- [x] 6.3.4 Unit tests for integration layer (12 tests)

## Phase 7: Configuration ✅

### 7.1 Config Schema
- [x] 7.1.1 Add `[dispatcher]` section to config schema (`DispatcherConfigToml`)
- [x] 7.1.2 Add `enabled: bool` field
- [x] 7.1.3 Add `confirmation_threshold: f32` field
- [x] 7.1.4 Add `l3_enabled: bool` field
- [x] 7.1.5 Add `l3_timeout_ms: u64` field
- [x] 7.1.6 Update config parsing (serde defaults, TOML integration)

### 7.2 Config Validation
- [x] 7.2.1 Validate threshold range (>= 0.0, warn if > 1.0)
- [x] 7.2.2 Validate timeout range (> 0)
- [x] 7.2.3 Default values if not specified (via serde defaults)
- [x] 7.2.4 Unit tests for config validation (10 tests)

## Phase 8: UniFFI Bridge ✅

### 8.1 Swift Interface
- [x] 8.1.1 Expose `ToolSource` enum via UniFFI (as `ToolSourceType`)
- [x] 8.1.2 Expose `UnifiedTool` struct via UniFFI (as `UnifiedToolInfo`)
- [x] 8.1.3 Expose `list_tools()` method (+ `list_tools_by_source()`, `search_tools()`, `refresh_tools()`)
- [x] 8.1.4 Update `aleph.udl` with new types
- [x] 8.1.5 Regenerate Swift bindings

## Phase 9: Documentation & Testing ✅

### 9.1 Documentation
- [x] 9.1.1 Update CLAUDE.md with dispatcher section
- [x] 9.1.2 Add dispatcher configuration examples
- [x] 9.1.3 Document L1/L2/L3 routing behavior

### 9.2 Integration Tests (849 tests pass)
- [x] 9.2.1 Test L1 → L2 → L3 cascade (`test_dispatcher_result_combinations`)
- [x] 9.2.2 Test confirmation trigger at threshold (`test_confirmation_trigger_at_threshold`)
- [x] 9.2.3 Test confirmation skip above threshold (`test_confirmation_skip_above_threshold`)
- [x] 9.2.4 Test registry refresh on config change (covered in registry tests)
- [x] 9.2.5 Test context-aware pronoun resolution (covered in L3 router tests)

### 9.3 Manual Testing Checklist
- [ ] 9.3.1 Test slash commands still work (L1) - `/search`, `/translate`
- [ ] 9.3.2 Test natural language tool invocation (L2/L3) - "search for X"
- [ ] 9.3.3 Test Halo confirmation UI appearance - low confidence scenarios
- [ ] 9.3.4 Test cancellation returns to chat - press Cancel in confirmation
- [ ] 9.3.5 Verify no performance regression on L1 - <10ms response

## Dependencies

- Phase 1 (Foundation) is independent
- Phase 2 (RoutingMatch) depends on Phase 1
- Phase 3 (Prompt Builder) depends on Phase 1
- Phase 4 (L2 Enhancement) depends on Phase 2
- Phase 5 (L3 Integration) depends on Phase 2, 3
- Phase 6 (Confirmation) depends on Phase 2
- Phase 7 (Configuration) is independent
- Phase 8 (UniFFI) depends on Phase 1
- Phase 9 (Testing) depends on all phases

## Parallelizable Work

These can be done in parallel:
- Phase 1 + Phase 7 (Foundation + Config)
- Phase 3 + Phase 4 (Prompt Builder + L2 Enhancement)
- Phase 8 + Phase 9.1 (UniFFI + Documentation)
