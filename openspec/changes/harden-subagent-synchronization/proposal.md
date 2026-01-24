# Change: Harden Sub-Agent Synchronization

## Why

Comparing Aether with OpenCode (Claude Code's open-source implementation) reveals critical gaps in sub-agent task execution. OpenCode achieves high efficiency through synchronous result collection and real-time progress tracking, while Aether's fire-and-forget event model means parent agents cannot reliably wait for or collect sub-agent results. This blocks proper multi-agent task orchestration—a core Agent capability.

## What Changes

- **ADDED**: `ExecutionCoordinator` component for synchronous wait capability
- **ADDED**: `ResultCollector` component for tool call aggregation
- **ADDED**: `dispatch_sync()` and `dispatch_parallel_sync()` methods in SubAgentDispatcher
- **MODIFIED**: `SubAgentHandler` to subscribe to tool call events and collect results
- **MODIFIED**: `SubAgentResult` to include mandatory tool summary and execution metadata
- **MODIFIED**: Event types to include tool call progress events
- **ADDED**: Configuration section `[subagent]` for timeout and concurrency settings

## Impact

- **Affected specs**: `subagent-execution` (new capability)
- **Affected code**:
  - `core/src/agents/sub_agents/` (dispatcher, traits, new coordinator)
  - `core/src/components/subagent_handler.rs`
  - `core/src/event/types.rs`
  - Configuration system

---

## Detailed Analysis

### Current State Analysis

Comparing Aether with OpenCode (Claude Code's open-source implementation) reveals critical gaps:

| Feature | OpenCode | Aether | Gap Severity |
|---------|----------|--------|--------------|
| **Result Aggregation** | `SessionPrompt.prompt()` synchronously waits and collects all tool executions | SubAgentHandler only tracks sessions, doesn't collect results | 🔴 Critical |
| **Synchronous Wait** | Parent blocks until child session completes | No wait mechanism; events are fire-and-forget | 🔴 Critical |
| **Result Ordering** | Returns `(request_id, result)` tuples for correlation | `dispatch_parallel()` returns results in execution order | 🔴 Critical |
| **Progress Events** | `PartUpdated` events for real-time UI updates | Events exist but no consumer integration | 🟡 Moderate |
| **Context Inheritance** | Full session context + permissions passed to child | `ExecutionContextInfo` rarely populated | 🟡 Moderate |
| **Tool Summary** | Collects all tool calls with status/title from child | `SubAgentResult.tools_called` exists but not aggregated | 🟡 Moderate |

### Key OpenCode Patterns Missing

1. **Synchronous Wait Pattern** (task.ts:145):
   ```typescript
   const result = await SessionPrompt.prompt({
     sessionID: session.id,
     messageID,
     model,
     agent: agent.name,
     parts: promptParts,
   })
   // Parent BLOCKS here until child completes
   ```

2. **Event Subscription for Progress** (task.ts:111):
   ```typescript
   const unsub = Bus.subscribe(MessageV2.Event.PartUpdated, async (evt) => {
     if (evt.properties.part.sessionID !== session.id) return
     parts[part.id] = { id, tool, state: { status } }
     ctx.metadata({ summary: Object.values(parts) })  // Real-time update
   })
   ```

3. **Result Aggregation** (task.ts:162):
   ```typescript
   const messages = await Session.messages({ sessionID: session.id })
   const summary = messages
     .filter(x => x.info.role === "assistant")
     .flatMap(msg => msg.parts.filter(x => x.type === "tool"))
     .map(part => ({ id, tool, state: { status, title } }))
   ```

## Proposed Solution

### Core Changes

1. **Result Collector Component** - New component that aggregates sub-agent results
2. **Execution Coordinator** - Manages synchronous wait and result ordering
3. **Enhanced SubAgentHandler** - Extends existing handler with result storage
4. **Progress Event Consumer** - Subscribes to tool events for real-time updates

### Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                    Enhanced Sub-Agent Execution                      │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│   Parent Agent                                                       │
│       │                                                              │
│       ▼                                                              │
│   ┌────────────────────────┐                                        │
│   │ ExecutionCoordinator   │◄─── NEW: Manages sync wait            │
│   │ - create_execution()   │                                        │
│   │ - wait_for_result()    │                                        │
│   │ - wait_for_all()       │                                        │
│   └──────────┬─────────────┘                                        │
│              │                                                       │
│              ▼                                                       │
│   ┌────────────────────────┐    ┌────────────────────────┐         │
│   │ SubAgentDispatcher     │───▶│ SubAgent (MCP/Skill)   │         │
│   └──────────┬─────────────┘    └──────────┬─────────────┘         │
│              │                              │                        │
│              │                              │ Tool Execution         │
│              │                              ▼                        │
│              │                   ┌────────────────────────┐         │
│              │                   │ Event: ToolCallResult  │         │
│              │                   └──────────┬─────────────┘         │
│              │                              │                        │
│              ▼                              ▼                        │
│   ┌────────────────────────┐    ┌────────────────────────┐         │
│   │ SubAgentHandler        │◄───│ ResultCollector        │◄─── NEW │
│   │ (session tracking)     │    │ - collect_tool_result()│         │
│   └────────────────────────┘    │ - get_summary()        │         │
│                                 │ - get_artifacts()      │         │
│                                 └────────────────────────┘         │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

## Scope

### In Scope
- Synchronous wait mechanism for sub-agent results
- Result aggregation and tool execution summary
- Request-result correlation for parallel execution
- Progress event integration
- Context propagation enhancement

### Out of Scope
- UI changes for displaying sub-agent progress (separate change)
- New sub-agent types (existing MCP/Skill agents enhanced)
- Session persistence across restarts

## Success Metrics

1. **Functional**: Parent agent can reliably wait for and receive all sub-agent results
2. **Ordering**: Parallel dispatch returns results correlated with original request IDs
3. **Completeness**: All tool calls from sub-agents are collected in summary
4. **Performance**: < 10ms overhead per sub-agent execution for coordination

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Deadlock if sub-agent hangs | Timeout on wait operations (configurable, default 5min) |
| Memory growth with many results | TTL-based cleanup of completed results (default 1 hour) |
| Race conditions on result access | Use `RwLock` with async-aware primitives |

## Dependencies

- Existing `agent_loop` module
- Existing `agents/sub_agents/` module
- Existing `event` system
- Existing `components/subagent_handler.rs`

## Related Changes

- `implement-dispatcher-layer` (completed)
- `add-smart-tool-discovery` (in progress)
- `enable-intelligent-tool-invocation` (in progress)
