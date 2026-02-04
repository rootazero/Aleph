# Change: Implement Intelligent Dispatcher Layer (Aleph Cortex)

## Status
- **Stage**: Deployed
- **Created**: 2026-01-09
- **Deployed**: 2026-01-09

## Why

The current tool invocation system in Aleph is too simplistic:

1. **No unified tool discovery**: Native tools, MCP tools, Skills, and custom commands are scattered across different registries with no unified interface
2. **Missing intelligent routing**: The system lacks LLM-based intent detection for ambiguous user inputs
3. **No user confirmation flow**: High-impact tool calls execute without user awareness or confirmation
4. **Limited context awareness**: The system doesn't leverage conversation history for pronoun resolution (e.g., "search for him" where "him" refers to a previously mentioned person)
5. **Static prompt generation**: Tool descriptions are not dynamically injected into the routing LLM's system prompt

This proposal introduces a **Dispatcher Layer** that sits between user input and tool execution, providing intelligent intent classification, parameter extraction, and user confirmation through the Halo UI.

## What Changes

### Core Architecture

1. **Unified Tool Registry** (`unified_tool.rs`)
   - `UnifiedTool` struct: Standard metadata for all tool types
   - `ToolRegistry`: Aggregates Native/MCP/Skills/Custom tools
   - Dynamic refresh on configuration changes

2. **Three-Layer Routing** (extend existing `Router`)
   - **L1 (Rule-based)**: Existing regex matching (<10ms)
   - **L2 (Semantic)**: Enhanced keyword/similarity matching (200-500ms)
   - **L3 (AI-based)**: LLM inference with context awareness (>1s)

3. **ActionProposal** (extend `RoutingMatch`)
   - Add `confidence: f32` field
   - Add `extracted_parameters: Option<serde_json::Value>` field
   - Add `reason: Option<String>` field for explainability

4. **Halo Confirmation Flow**
   - Trigger when `confidence < threshold` (configurable, default 0.8)
   - Use existing `on_clarification_needed()` callback
   - Support "Execute" / "Cancel" / "Edit parameters" options

5. **Dynamic Prompt Builder**
   - Inject tool metadata into L3 router's system prompt
   - Format: Tool name, description, parameter hints
   - Support for tool count optimization (future: RAG-based selection)

### Files to Create/Modify

**New Files:**
- `core/src/dispatcher/mod.rs` - Dispatcher module entry
- `core/src/dispatcher/registry.rs` - UnifiedTool + ToolRegistry
- `core/src/dispatcher/proposal.rs` - ActionProposal struct
- `core/src/dispatcher/prompt_builder.rs` - Dynamic prompt generation
- `core/src/dispatcher/confirmation.rs` - Halo confirmation logic

**Modified Files:**
- `core/src/router/mod.rs` - Integrate L2/L3 routing
- `core/src/core.rs` - Wire up Dispatcher in process flow
- `core/src/aleph.udl` - Add confirmation-related types
- `core/src/config/mod.rs` - Add dispatcher configuration

## Impact

### Affected Specs
- `ai-routing` - Extended with multi-layer routing
- New spec: `dispatcher` - Core dispatcher requirements
- New spec: `unified-tool-registry` - Tool aggregation requirements

### Affected Code
- `core/src/router/` - Enhanced with confidence scoring
- `core/src/core.rs` - Process flow modification
- `core/src/intent/` - AiIntentDetector integration
- `core/src/mcp/` - Tool metadata export
- `core/src/skills/` - Skill metadata export

### Breaking Changes
- **None** - This is an additive change
- Existing slash commands and routing rules continue to work
- L1 routing behavior unchanged

### Migration
- No migration required
- New features are opt-in via configuration
- Default behavior: L1 + L2 routing (same as current)

## Design Decisions

### Why extend existing Router instead of creating new Dispatcher?
- **Low coupling**: Dispatcher wraps Router, doesn't replace it
- **Backward compatibility**: L1 routing behavior preserved
- **Incremental adoption**: L3 can be disabled if not needed

### Why not use schemars for JSON Schema generation?
- Native tools are few (<5), handwritten schema is simpler
- MCP already provides standard JSON Schema
- Avoids new dependency complexity

### Why confidence threshold instead of always confirming?
- User experience: Frequent confirmations are annoying
- High-confidence matches (slash commands) should execute immediately
- Threshold is configurable for user preference

### Why not RAG-based tool selection now?
- Current tool count is manageable (<20)
- RAG adds latency and complexity
- Marked as future enhancement when tool count exceeds threshold

## Success Criteria

1. User can type natural language like "search for today's news" and system proposes Search tool
2. Ambiguous inputs trigger Halo confirmation with tool preview
3. All tool types (Native/MCP/Skills/Custom) appear in unified registry
4. L3 routing correctly resolves pronouns using conversation context
5. Performance: L1 < 10ms, L2 < 500ms, L3 < 2s

## References

- Design document: `DispatcherLayer.md` (user-provided)
- Related code: `core/src/router/mod.rs`, `core/src/intent/ai_detector.rs`
- Related specs: `ai-routing`, `hotkey-detection`
