# Multi-Agent Code Redundancy Cleanup Plan

> **Status**: Pending — A2A protocol implementation complete, cleanup not yet started
> **Priority**: Medium — not blocking, but reduces maintenance burden
> **Estimated effort**: 3-5 days
> **Prerequisite**: A2A protocol fully integrated and stable

## Background

With the A2A protocol (v0.3) now fully implemented in Aleph, several older multi-agent subsystems have significant overlap with the new A2A infrastructure. This document identifies redundancies and proposes a phased cleanup plan.

## Redundancy Analysis

### 1. Authentication & Authorization

| Old System | New A2A Equivalent | Overlap |
|---|---|---|
| `AgentToAgentPolicy` (`gateway/a2a_policy.rs`) | `TieredAuthenticator` (`a2a/adapter/server/auth.rs`) | Both enforce inter-agent trust levels. Old uses string-based policy rules; new uses tiered cascade (localhost → Bearer → OAuth2) |
| `SubagentAuthority` (`agents/sub_agents/authority.rs`) | A2A `TrustLevel` + `SecurityScheme` | Both define what sub-agents can/cannot do. Old is Rust-native enum; new follows A2A spec |

**Recommendation**: Migrate `AgentToAgentPolicy` consumers to `TieredAuthenticator`. Keep `SubagentAuthority` only if non-A2A sub-agents (MCP, Skill) still need it.

### 2. Routing & Discovery

| Old System | New A2A Equivalent | Overlap |
|---|---|---|
| `delegate` tool routing logic | `SmartRouter` (3-tier: name → skill → LLM semantic) | Both resolve "which agent handles this?" Old uses hardcoded rules; new uses tiered matching |
| `SubAgentDispatcher` routing | `SmartRouter` + `A2ASubAgent.can_handle()` | Dispatcher iterates sub-agents; A2ASubAgent internally uses SmartRouter |

**Recommendation**: `SubAgentDispatcher` remains as the top-level dispatcher (it routes to MCP, Skill, AND A2A sub-agents). But `delegate` tool's internal routing should defer to SmartRouter when target is A2A-capable.

### 3. Inter-Agent Communication Tools

| Old System | New A2A Equivalent | Overlap |
|---|---|---|
| `sessions_send` tool (`builtin_tools/sessions/send_tool.rs`) | `a2a_send_message` (via A2ASubAgent) | Both send messages to other agents. Old uses internal session routing; new uses A2A protocol |
| `sessions_list` tool | A2A `tasks/list` JSON-RPC method | Both enumerate active conversations/tasks |

**Recommendation**: Keep `sessions_send` for intra-process agent communication (same Aleph instance). Use A2A for cross-process/cross-host communication. Document the boundary clearly.

### 4. Result Collection

| Old System | New A2A Equivalent | Overlap |
|---|---|---|
| `ResultCollector` (`agents/sub_agents/result_collector.rs`) | A2A task history (`Task.history` + `TaskStatus`) | Both aggregate sub-agent responses. Old is Rust-native; new follows A2A Task lifecycle |

**Recommendation**: For A2A tasks, use task history directly. `ResultCollector` may still serve non-A2A sub-agents. Evaluate after A2A stabilizes.

### 5. Group Chat / Multi-Agent Orchestration

| Old System | New A2A Equivalent | Overlap |
|---|---|---|
| `group_chat/` module (orchestrator, channel, protocol) | A2A multi-agent coordination | Partial overlap. Group chat provides real-time multi-party coordination; A2A is request-response per agent |
| `GatewayContext` multi-agent fields | A2A `port/service` layers | Both manage agent lifecycle and state |

**Recommendation**: Group chat and A2A serve different interaction patterns. Group chat = synchronous multi-party. A2A = async delegation. Keep both, but ensure group chat can delegate to A2A agents when appropriate.

### 6. Configuration

| Old System | New A2A Equivalent | Overlap |
|---|---|---|
| `config/types/group_chat.rs` | A2A agent card + trust config | Both configure agent capabilities and trust |
| `tools/sessions/policy.rs` | A2A security schemes | Both define communication policies |

**Recommendation**: Unify policy definitions where possible. A2A security schemes should be the canonical format for cross-agent policies.

## Cleanup Phases

### Phase 1: Documentation & Boundary Clarification (1 day)
- Document which system handles which communication pattern
- Add inline comments marking deprecated paths
- Update `AGENT_SYSTEM.md` and `TOOL_SYSTEM.md` with A2A integration notes

### Phase 2: Authentication Consolidation (1 day)
- Migrate `AgentToAgentPolicy` consumers to `TieredAuthenticator`
- Evaluate if `SubagentAuthority` can be simplified or merged
- Remove dead code paths

### Phase 3: Routing Unification (1 day)
- Make `delegate` tool route through `SmartRouter` for A2A-capable agents
- Ensure `SubAgentDispatcher` cleanly separates local vs A2A routing
- Remove duplicate routing logic

### Phase 4: Tool Deduplication (1-2 days)
- Define clear boundary: `sessions_send` = intra-process, A2A = inter-process
- Consolidate `ResultCollector` with A2A task history where appropriate
- Remove or deprecate redundant tools with migration path

## Files Affected

**Likely to modify or remove:**
- `core/src/gateway/a2a_policy.rs`
- `core/src/agents/sub_agents/authority.rs`
- `core/src/builtin_tools/sessions/send_tool.rs`
- `core/src/agents/sub_agents/result_collector.rs`
- `core/src/tools/sessions/policy.rs`
- `core/src/components/subagent_handler.rs`

**Likely to update:**
- `core/src/agents/sub_agents/dispatcher.rs`
- `core/src/gateway/context.rs`
- `core/src/group_chat/orchestrator.rs`
- `docs/reference/AGENT_SYSTEM.md`
- `docs/reference/TOOL_SYSTEM.md`

## Decision Log

| Decision | Rationale |
|---|---|
| Keep `SubAgentDispatcher` | It's the top-level router for ALL sub-agent types (MCP, Skill, A2A), not just A2A |
| Keep `group_chat/` | Different interaction pattern (sync multi-party vs async delegation) |
| Keep `sessions_send` for now | Intra-process communication still useful; deprecate after A2A proves stable |
| Consolidate auth first | Highest impact, lowest risk — clear duplication with clear winner |
