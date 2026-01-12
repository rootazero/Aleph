# Proposal: enhance-l3-agent-planning

## Summary

Enhance the L3 Router to evolve from a "single tool selector" into a "task planner" capable of decomposing complex multi-step user requests into executable plans, while maintaining backward compatibility with L1/L2 routing layers.

## Motivation

### Problem Statement

Current L3 Router limitations:
- Can only select **ONE** tool per request
- Cannot handle multi-step tasks like "translate this document, summarize it, and send to John"
- Users must manually break complex tasks into separate commands

### User Impact

Users currently need to:
1. Issue multiple separate commands for workflow-like tasks
2. Manually coordinate intermediate results between steps
3. Lose context between steps, reducing AI effectiveness

### Proposed Solution

Extend the existing L3 Router with:
1. **Task Analyzer**: Detects if input requires single-tool or multi-step execution
2. **Plan Generator**: Produces an `ExecutionPlan` with ordered steps
3. **Plan Executor**: Executes steps sequentially with result passing
4. **Plan Confirmation UI**: Shows plan preview before execution

## Design Decisions

### Decision 1: Extend Existing Architecture (Not Introduce rig-core)

**Context**: The original proposal suggested introducing `rig-core` crate.

**Decision**: Extend existing `IntentAction`, `IntentSignal`, and `L3Router` instead.

**Rationale**:
- Aether already has a mature routing pipeline with L1/L2/L3 layers
- Adding external dependency increases maintenance burden
- rig-core's value (LLM abstraction, pipeline composition) overlaps with existing code
- Tighter integration with existing `UnifiedTool` and `ToolRegistry`

### Decision 2: Sequential Execution First (No Parallel)

**Context**: Original proposal included parallel step execution with dependency graphs.

**Decision**: MVP supports only linear sequential execution.

**Rationale**:
- Dependency graph adds significant complexity
- LLM-generated dependencies are unreliable (format inconsistency)
- Sequential execution covers 90%+ of real use cases
- Parallel execution can be added in future phase

### Decision 3: Single LLM Call for Task Analysis + Planning

**Context**: Original proposal had separate LLM calls for task type detection and plan generation.

**Decision**: Combine into single prompt that returns either `SingleTool` or `ExecutionPlan`.

**Rationale**:
- Reduces latency (one API call vs two)
- Avoids redundant context sending
- Quick heuristics can still gate the LLM call

### Decision 4: Prevention Over Rollback

**Context**: Original proposal had elaborate rollback mechanisms.

**Decision**: Focus on pre-execution confirmation for high-risk operations; implement minimal rollback for reversible-only operations.

**Rationale**:
- Most "irreversible" operations (send email, external API) cannot rollback
- File operations can use simple backup-based rollback
- Better UX to prevent mistakes than recover from them
- Reduces implementation complexity significantly

### Decision 5: Use schemars for Tool Schema Generation (Not rig-core)

**Context**: Tool parameter definitions require JSON Schema for LLM function calling. Options:
1. Hand-write JSON Schema (current approach - error-prone, verbose)
2. Use rig-core's Tool trait (heavy dependency)
3. Use schemars derive macro (lightweight, type-safe)

**Decision**: Adopt `schemars` crate for automatic JSON Schema generation from Rust structs.

**Rationale**:
- **Lightweight**: schemars is ~50KB, derive-macro only, zero runtime overhead
- **Type-safe**: Parameter definitions are Rust structs with compiler validation
- **Auto-documentation**: `/// doc comments` automatically become `description` fields
- **Serde compatible**: Works seamlessly with existing serde ecosystem
- **Control retained**: Keep custom agent loop, no framework lock-in
- **Minimal change**: Only affects tool definition, not core architecture

**Trade-offs**:
- Requires migrating existing hand-written schemas (one-time cost)
- Adds one new dependency (but removes manual schema maintenance burden)

### Decision 6: Custom Agent Loop with Tool Calling

**Context**: Need to support multi-turn tool execution within a single user request.

**Decision**: Implement custom agent loop that:
1. Sends request with tool definitions to LLM
2. Processes `tool_calls` in response
3. Executes tools and feeds results back
4. Loops until LLM produces final response (or max turns reached)

**Rationale**:
- Full control over execution flow and error handling
- Can integrate with existing Aether event system
- Supports both single-tool and multi-step execution
- No external framework dependency

## Scope

### In Scope

1. **Task Analyzer** with quick heuristics (multi-verb/connector detection)
2. **Extended IntentAction** enum with `ExecutePlan` variant
3. **TaskPlan** and **PlanStep** data structures
4. **Sequential PlanExecutor** with step-by-step result passing
5. **Plan Confirmation UI** (Swift) showing step preview
6. **Tool Safety Levels** (ReadOnly, Reversible, Irreversible)
7. **Basic rollback** for reversible operations only
8. **Configuration options** in `[dispatcher.agent]` section
9. **schemars integration** for type-safe tool parameter definitions
10. **ToolParams trait** as marker trait for parameter structs
11. **Custom Agent Loop** with multi-turn tool calling support

### Out of Scope (Future Phases)

- Parallel step execution
- Dependency graph optimization
- Advanced rollback strategies (partial, selective)
- Memory integration for plan context
- MCP tool integration in plans
- Step editing UI

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| LLM generates invalid plans | Medium | High | Strict JSON schema validation, fallback to single-tool mode |
| Plan execution timeout | Low | Medium | Per-step timeout with graceful degradation |
| User confusion with new UI | Medium | Medium | Clear plan preview, simple confirm/cancel flow |
| Performance regression | Low | Medium | Heuristics gate expensive LLM calls |

## Success Criteria

1. Multi-step tasks (2-5 steps) execute successfully with >90% accuracy
2. L3 planning latency <2s additional overhead vs single-tool routing
3. Plan confirmation UI receives positive user feedback
4. Zero regressions in existing L1/L2/single-tool L3 routing

## Related Changes

- `enhance-intent-routing-pipeline` (prerequisite - mostly complete)
- `unify-tool-registry` (prerequisite - mostly complete)
- `implement-dispatcher-layer` (prerequisite - mostly complete)

## Timeline Considerations

This is a multi-phase enhancement:
- Phase 1: Foundation (Data structures + IntentAction extension + schemars integration)
- Phase 2: L3 Task Planner + Agent Loop with tool calling
- Phase 3: PlanExecutor + Step result passing
- Phase 4: Swift UI integration (Confirmation + Progress views)
- Phase 5: Configuration, safety features, and testing
