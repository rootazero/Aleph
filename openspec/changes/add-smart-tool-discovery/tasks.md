# Tasks: Smart Tool Discovery System

## 1. Phase 1: Two-Stage Tool Discovery

### 1.1 Tool Index System
- [x] 1.1.1 Define `ToolIndexEntry` struct in `dispatcher/types.rs`
- [x] 1.1.2 Add `to_index_entry()` method to `UnifiedTool`
- [x] 1.1.3 Add `generate_tool_index()` method to `ToolRegistry`
- [x] 1.1.4 Implement `to_index_prompt()` for compact LLM format
- [x] 1.1.5 Add `generate_smart_prompt()` method for filtered tool selection

### 1.2 Meta Tools
- [x] 1.2.1 Create `meta_tools` module in `core/src/rig_tools/`
- [x] 1.2.2 Implement `ListToolsTool` - list available tools/categories
- [x] 1.2.3 Implement `GetToolSchemaTool` - get full tool definition
- [ ] 1.2.4 Implement `RequestToolsTool` - request tool category loading (deferred)
- [x] 1.2.5 Register meta tools in `BuiltinToolRegistry`

### 1.3 On-Demand Schema Loading
- [ ] 1.3.1 Add `ToolSchemaCache` for loaded schemas (deferred - handled by LLM context)
- [x] 1.3.2 Modify PromptBuilder to support two-stage tool discovery (tool_index config)
- [ ] 1.3.3 Add `expand_tool_schema()` method for runtime expansion (handled by get_tool_schema)
- [ ] 1.3.4 Implement schema injection into tool call context (handled by LLM)

## 2. Phase 2: Intent-Based Pre-filtering

### 2.1 Tool Filter Module
- [x] 2.1.1 Create `dispatcher/tool_filter.rs` module
- [x] 2.1.2 Define `ToolFilterConfig` with core tools and limits
- [x] 2.1.3 Implement `category_to_keywords()` mapping (TaskCategory → keywords)
- [x] 2.1.4 Implement `category_to_tool_names()` mapping (TaskCategory → tool names)

### 2.2 Tool Pre-filtering
- [x] 2.2.1 Implement `filter_by_category()` for single TaskCategory filtering
- [x] 2.2.2 Implement `filter_by_categories()` for multi-category filtering
- [x] 2.2.3 Implement keyword matching for tool relevance (name + description)
- [x] 2.2.4 Configure max_filtered_tools limit (default: 10)

### 2.3 Core Tool Set
- [x] 2.3.1 Define core tools in ToolFilterConfig (search, file_ops, list_tools, get_tool_schema)
- [x] 2.3.2 Implement `is_core_tool()` method for checking
- [x] 2.3.3 Ensure core_tools always included in FilterResult

### 2.4 Filter Result
- [x] 2.4.1 Define `FilterResult` struct with core/filtered/indexed tools
- [x] 2.4.2 Implement `full_schema_tools()` helper method
- [x] 2.4.3 Implement `full_schema_count()` and `indexed_count()` helpers

### 2.5 Tests
- [x] 2.5.1 Unit tests for ToolFilterConfig
- [x] 2.5.2 Unit tests for filter_by_category (WebSearch, MediaDownload, ImageGeneration)
- [x] 2.5.3 Unit tests for filter_by_categories (multiple categories)
- [x] 2.5.4 Unit tests for max_filtered_tools limit

### 2.6 Thinker Integration
- [x] 2.6.1 Integrate dispatcher::tool_filter with thinker::tool_filter
- [x] 2.6.2 Add `with_intent_filter()` constructor to thinker::ToolFilter
- [x] 2.6.3 Add `pre_filter_by_category()` method for intent-based pre-filtering
- [x] 2.6.4 Re-export IntentFilterConfig and IntentFilterResult from thinker

## 3. Phase 3: Sub-Agent Delegation

### 3.1 Sub-Agent Framework
- [x] 3.1.1 Design `SubAgent` trait with capabilities, can_handle, execute
- [x] 3.1.2 Implement `SubAgentRequest` and `SubAgentResult` types
- [x] 3.1.3 Create `SubAgentDispatcher` for routing requests

### 3.2 Specialized Agents
- [x] 3.2.1 Create `McpSubAgent` for MCP tool discovery
- [x] 3.2.2 Create `SkillSubAgent` for skill discovery
- [x] 3.2.3 Create `DelegateTool` implementing rig-core Tool trait

### 3.3 Types and Exports
- [x] 3.3.1 Define `SubAgentCapability` enum
- [x] 3.3.2 Define `SubAgentType` enum
- [x] 3.3.3 Define `ToolCallRecord` and `Artifact` types
- [x] 3.3.4 Export all types from `agents::sub_agents` module

### 3.4 Tests
- [x] 3.4.1 Unit tests for SubAgentRequest/SubAgentResult (3 tests)
- [x] 3.4.2 Unit tests for McpSubAgent (3 tests)
- [x] 3.4.3 Unit tests for SkillSubAgent (3 tests)
- [x] 3.4.4 Unit tests for SubAgentDispatcher (6 tests)
- [x] 3.4.5 Unit tests for DelegateTool (4 tests)

### 3.5 Integration
- [x] 3.5.1 Register DelegateTool in BuiltinToolRegistry
- [x] 3.5.2 Add context passing between agents (ExecutionContextInfo, StepContextInfo)
- [x] 3.5.3 Implement result merging in agent loop (ResultMerger, MergedResult)

## 4. Testing & Validation

- [x] 4.1 Unit tests for ToolIndex generation
- [x] 4.2 Unit tests for meta tools
- [x] 4.3 Integration tests for two-stage discovery
- [x] 4.4 Benchmark: token consumption comparison
- [x] 4.5 Benchmark: latency comparison
- [x] 4.6 End-to-end test with 50+ tools
