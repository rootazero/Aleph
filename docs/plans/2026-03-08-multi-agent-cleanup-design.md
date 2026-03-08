# Multi-Agent Code Redundancy Cleanup Plan

> **Status**: Pending — A2A protocol implementation complete, cleanup not yet started
> **Priority**: Medium — not blocking, but reduces maintenance burden
> **Estimated effort**: 3-5 days
> **Prerequisite**: A2A protocol fully integrated and stable

## Background

With the A2A protocol (v0.3) now fully implemented in Aleph, several older multi-agent subsystems have significant overlap with the new A2A infrastructure. This document identifies redundancies and proposes a phased cleanup plan.

## Design Decision: Unified Abstraction, Scoped Responsibility

**结论：旧系统与 A2A 不是降级关系，是分工关系。重叠部分直接清理，各管各的部分各自保留。**

### Why NOT keep old systems as fallback

1. **降级前提不成立** — 旧系统和 A2A 处理的不是同一类通信。`sessions_send` 发给同进程 agent，A2A 发给远端 agent。远端 agent 挂了，降级到本地发送没有意义——本地根本没有那个 agent。

2. **维护成本高于收益** — 两套并行系统意味着每次改动都要考虑两条路径，bug 排查要分辨走的哪条路，新人上手更困难。

3. **A2A 已有自己的降级机制** — `SmartRouter` 三层 fallback（精确名 → 技能匹配 → LLM 语义），`AgentHealth` 过滤不可达 agent，这才是有意义的降级。

### Communication scope model

| Scope | Transport | Latency | System |
|---|---|---|---|
| Intra-process (same Aleph instance) | Direct function call / memory channel | Microseconds | `SubAgentDispatcher` → MCP/Skill SubAgent |
| Inter-process / cross-host | HTTP JSON-RPC (A2A protocol) | Milliseconds~seconds | `SubAgentDispatcher` → `A2ASubAgent` → `SmartRouter` |

### Target architecture

```
SubAgentDispatcher (unified entry point)
  ├── McpSubAgent     — MCP tool invocations
  ├── SkillSubAgent   — Local skill execution
  ├── A2ASubAgent     — Remote agents (A2A protocol)
  └── (future) LocalSubAgent — Intra-process agents (if needed)
```

## Redundancy Analysis

### 1. Authentication & Authorization

| Old System | New A2A Equivalent | Overlap |
|---|---|---|
| `AgentToAgentPolicy` (`gateway/a2a_policy.rs`) | `TieredAuthenticator` (`a2a/adapter/server/auth.rs`) | Both enforce inter-agent trust levels. Old uses string-based policy rules; new uses tiered cascade (localhost → Bearer → OAuth2) |
| `SubagentAuthority` (`agents/sub_agents/authority.rs`) | A2A `TrustLevel` + `SecurityScheme` | Both define what sub-agents can/cannot do. Old is Rust-native enum; new follows A2A spec |

**Action**: **Clean up.** Migrate `AgentToAgentPolicy` consumers to `TieredAuthenticator`. Remove `SubagentAuthority` if non-A2A sub-agents (MCP, Skill) don't actually use it.

### 2. Routing & Discovery

| Old System | New A2A Equivalent | Overlap |
|---|---|---|
| `delegate` tool routing logic | `SmartRouter` (3-tier: name → skill → LLM semantic) | Both resolve "which agent handles this?" Old uses hardcoded rules; new uses tiered matching |
| `SubAgentDispatcher` routing | `SmartRouter` + `A2ASubAgent.can_handle()` | Dispatcher iterates sub-agents; A2ASubAgent internally uses SmartRouter |

**Action**: **Clean up** `delegate` tool's internal routing — it should defer to `SmartRouter`. **Keep** `SubAgentDispatcher` as the top-level dispatcher (routes to MCP, Skill, AND A2A).

### 3. Inter-Agent Communication Tools

| Old System | New A2A Equivalent | Overlap |
|---|---|---|
| `sessions_send` tool (`builtin_tools/sessions/send_tool.rs`) | `a2a_send_message` (via A2ASubAgent) | Both send messages to other agents. Old uses internal session routing; new uses A2A protocol |
| `sessions_list` tool | A2A `tasks/list` JSON-RPC method | Both enumerate active conversations/tasks |

**Action**: **Clean up** if `sessions_send` has no real intra-process use case. If it does, narrow its scope strictly to same-process communication and rename to clarify.

### 4. Result Collection

| Old System | New A2A Equivalent | Overlap |
|---|---|---|
| `ResultCollector` (`agents/sub_agents/result_collector.rs`) | A2A task history (`Task.history` + `TaskStatus`) | Both aggregate sub-agent responses. Old is Rust-native; new follows A2A Task lifecycle |

**Action**: **Keep** `ResultCollector` for aggregating local sub-agent results (MCP, Skill). A2A task history handles remote agent results. No overlap in practice.

### 5. Group Chat / Multi-Agent Orchestration

| Old System | New A2A Equivalent | Overlap |
|---|---|---|
| `group_chat/` module (orchestrator, channel, protocol) | A2A multi-agent coordination | Partial overlap. Group chat = synchronous multi-party discussion; A2A = async request-response delegation |
| `GatewayContext` multi-agent fields | A2A `port/service` layers | Both manage agent lifecycle and state |

**Action**: **Keep** `group_chat/` — fundamentally different interaction pattern. Ensure group chat can delegate to A2A agents when appropriate.

### 6. Configuration

| Old System | New A2A Equivalent | Overlap |
|---|---|---|
| `config/types/group_chat.rs` | A2A agent card + trust config | Both configure agent capabilities and trust |
| `tools/sessions/policy.rs` | A2A security schemes | Both define communication policies |

**Action**: **Clean up** `tools/sessions/policy.rs` — unify into A2A security schemes as canonical format.

## Cleanup Phases

### Phase 1: Documentation & Boundary Clarification (1 day)
- Document communication scope model (intra-process vs inter-process)
- Add inline comments marking deprecated paths
- Update `AGENT_SYSTEM.md` and `TOOL_SYSTEM.md` with A2A integration notes

### Phase 2: Authentication Consolidation (1 day)
- Remove `AgentToAgentPolicy`, migrate consumers to `TieredAuthenticator`
- Remove `SubagentAuthority` if unused by MCP/Skill sub-agents
- Remove `tools/sessions/policy.rs`, unify into A2A security schemes

### Phase 3: Routing Cleanup (1 day)
- Remove `delegate` tool's internal routing logic, defer to `SmartRouter`
- Ensure `SubAgentDispatcher` cleanly separates local vs A2A routing
- Remove duplicate routing code

### Phase 4: Tool Cleanup (1-2 days)
- Evaluate `sessions_send` real usage — remove or narrow scope
- Remove `sessions_list` if A2A `tasks/list` covers all use cases
- Clean up dead code paths and unused imports

## Files Affected

**Likely to remove:**
- `core/src/gateway/a2a_policy.rs` — replaced by `TieredAuthenticator`
- `core/src/tools/sessions/policy.rs` — replaced by A2A security schemes

**Likely to modify or remove (pending usage analysis):**
- `core/src/agents/sub_agents/authority.rs`
- `core/src/builtin_tools/sessions/send_tool.rs`
- `core/src/components/subagent_handler.rs`

**Keep but update:**
- `core/src/agents/sub_agents/dispatcher.rs` — clean separation of routing
- `core/src/agents/sub_agents/result_collector.rs` — scope to local only
- `core/src/gateway/context.rs` — remove redundant multi-agent fields
- `core/src/group_chat/orchestrator.rs` — add A2A delegation support
- `docs/reference/AGENT_SYSTEM.md`
- `docs/reference/TOOL_SYSTEM.md`

## Decision Log

| Decision | Rationale |
|---|---|
| **Not a fallback relationship** | Old systems and A2A handle different communication scopes (intra-process vs inter-process). Keeping old as "degradation" has no semantic meaning. |
| Clean up overlapping auth | `AgentToAgentPolicy` and `TieredAuthenticator` solve the same problem; keep the A2A-standard one |
| Clean up overlapping routing | `delegate` tool routing duplicates `SmartRouter`; single source of truth |
| Keep `SubAgentDispatcher` | Top-level router for ALL sub-agent types (MCP, Skill, A2A), not replaceable by A2A alone |
| Keep `group_chat/` | Synchronous multi-party discussion ≠ async delegation; different interaction patterns |
| Keep `ResultCollector` | Scoped to local sub-agent aggregation; A2A task history handles remote |
| Consolidate auth first | Highest impact, lowest risk — clear duplication with clear winner |
