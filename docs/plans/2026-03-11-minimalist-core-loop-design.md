# Minimalist Core Loop Design

> "把复杂留给模型，把简单留给系统。"

## Core Philosophy

Aleph's current architecture has 491k LOC, 189+ modules, 84+ traits, and 5 vertical layers + 4 cross-cutting layers. A single request passes through 20+ decision points.

Claude Code proves that a simple control loop + powerful system prompt can outperform complex middleware. The key insight:

**Don't replace what the LLM is good at. Amplify what the LLM can't do alone.**

### Decision Principle

For every module, ask: "Does this amplify the LLM, or replace its reasoning?"

- **Amplify** (keep): Gateway, Memory, Daemon, Soul, Providers, Tools, MCP, Extensions, Compression, Safety
- **Replace** (remove): Intent Detection, POE, Triple Filter, Context Aggregation, Dispatcher/Cortex, Resilience

The "intelligence" in removed modules doesn't disappear — it migrates into the system prompt, where the LLM handles it as part of its natural reasoning in a single inference call.

---

## Architecture: Three Layers

```
┌─────────────────────────────────────────────────┐
│           Gateway Layer (reuse existing)         │
│  WebSocket + JSON-RPC + 16 platform adapters     │
│  Responsibility: pure I/O, identity resolution   │
└──────────────────────┬──────────────────────────┘
                       │ RunRequest
┌──────────────────────▼──────────────────────────┐
│           Core Loop (~200 lines core)            │
│                                                  │
│  loop {                                          │
│      messages = build_prompt(session, memory,    │
│                              tools, soul);       │
│      response = provider.call(messages).await;   │
│      match response {                            │
│          Text(t)     => stream_to_client(t),     │
│          ToolUse(t)  => {                        │
│              guard_safety(t)?;                   │
│              let r = execute_tool(t).await;      │
│              messages.push(r);                   │
│          }                                       │
│          Stop        => break,                   │
│      }                                           │
│      if token_overflow { compress(messages); }   │
│  }                                               │
│                                                  │
│  Components:                                     │
│  - PromptBuilder (assemble system prompt)        │
│  - SafetyGuard (single-layer hard-coded rules)   │
│  - ContextCompressor (reuse existing)            │
│  - ModelRouter (reuse existing, cost routing)    │
└──────────────────────┬──────────────────────────┘
                       │ tool calls
┌──────────────────────▼──────────────────────────┐
│        Capability Layer (flat registry)           │
│                                                  │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐           │
│  │ Builtin │ │   MCP   │ │Extension│           │
│  │  Tools  │ │Transport│ │ Runtime │           │
│  └─────────┘ └─────────┘ └─────────┘           │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐           │
│  │ Memory  │ │ Daemon  │ │  Soul   │           │
│  │ (Hybrid)│ │(Percept)│ │(Persona)│           │
│  └─────────┘ └─────────┘ └─────────┘           │
│                                                  │
│  Unified: Tool trait (name + schema + execute)   │
│  Registry: flat HashMap<String, Box<dyn Tool>>   │
└─────────────────────────────────────────────────┘
```

### Key Metrics

| Dimension | Before | After |
|-----------|--------|-------|
| Vertical layers | 5 + 4 cross-cutting | 3, no cross-cutting |
| Core loop | OTAF 5-phase + Guards + State Machine | `think → act` 2-step |
| Tool dispatch | Dispatcher → Cortex → Filter×3 → Executor | `guard → execute` |
| Context build | 4-layer Context Aggregation | 1 PromptBuilder function |
| Intent understanding | Intent Detection L0-L2 rule engine | LLM via system prompt |
| Task evaluation | POE 55k LOC pipeline | LLM self-judgment |
| Trait count | 84+ | ~20-25 |
| Estimated LOC | 491k | ~200k |

---

## Core Loop Detail

### AgentLoop Structure

```rust
pub struct AgentLoop {
    provider: Arc<dyn AiProvider>,
    model_router: ModelRouter,
    tool_registry: ToolRegistry,
    prompt_builder: PromptBuilder,
    safety_guard: SafetyGuard,
    compressor: ContextCompressor,
    session: Session,
    config: LoopConfig, // max_iterations, token_budget, timeout
}
```

### Request Path Comparison

```
Before (one tool call):
  MessageBuilder → Memory::search_hybrid → ContextCompressor
  → Guards×5 → Thinker → PromptBuilder → ModelRouter → Provider
  → StreamingHandler → BlockStateManager → BlockReplyChunker
  → BlockCoalescer → DecisionParser → Dispatcher → RiskEvaluator
  → ToolFilter×3 → ConfirmationSystem → Executor → ToolRegistry
  → result → POE evaluation → Feedback → loop
  ~20+ intermediate steps

After:
  PromptBuilder → Provider → SafetyGuard → ToolRegistry.execute
  → result back to messages → loop
  ~4 steps
```

### SafetyGuard — Single Hard-Coded Filter

```rust
pub struct SafetyGuard {
    blocked_patterns: Vec<Regex>,          // absolute deny
    confirmation_required: HashSet<String>, // require user approval
}
```

No risk evaluation, no content classification, no multi-layer filtering. Simple pattern matching for security boundaries that must not be delegated to LLM reasoning.

### PromptBuilder — Intelligence Lives in the Prompt

```rust
pub struct PromptBuilder {
    soul: SoulManifest,
    capability_rules: String,  // tool usage rules (was ToolFilter logic)
    memory_summary: String,    // core memory auto-injection
}
```

One function assembles the complete system prompt:
1. Soul persona instructions
2. Available tools + usage rules (was triple filter, now text)
3. Core memory summary (auto-injected)
4. Session context
5. Behavioral instructions (was Intent Detection routing, now prompt directives)

---

## Capability Layer Detail

### Unified Tool Interface

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> JsonSchema;
    async fn execute(&self, input: Value) -> ToolResult;
}
```

Before: implementing a tool required `AlephTool + CapabilityStrategy + ToolFilter + Dispatcher registration` (4 touch points). Now: 1 trait, register to flat HashMap.

### Flat ToolRegistry

```rust
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}
```

No Dispatcher, no Cortex, no ToolIndex semantic retrieval, no triple Filter.

### Four Empowerment Modules

#### 1. Memory — Full Power, Tool Interface

Memory internal architecture untouched (Hybrid Retrieval, LanceDB vectors, Graph DB, Scoring Pipeline all preserved). Only the entry point changes: from "middleware auto-injection" to "tool + light auto-injection" dual channel.

```rust
// LLM can actively call memory.search for deep retrieval
pub struct MemorySearchTool { store: Arc<HybridMemoryStore> }
pub struct MemoryStoreTool { store: Arc<HybridMemoryStore> }
// PromptBuilder also auto-injects small core memories
```

#### 2. Daemon — Event Source, Not Intelligence

```
Before: Daemon → Perception → WorldModel → Dispatcher → Agent
After:  Daemon detects event → construct message → enter Core Loop
```

Events become messages. LLM decides how to respond. Remove WorldModel, Perception classifier, Daemon Dispatcher.

Expose tools for LLM to query/subscribe actively:
- `DaemonQueryTool` — query active events
- `DaemonSubscribeTool` — subscribe to new watch rules

#### 3. Soul — Pure Prompt Injection

```rust
pub struct SoulManifest {
    pub persona: String,
    pub tone: String,
    pub principles: Vec<String>,
    pub quirks: Vec<String>,
}
```

No SoulEngine, no EmbodimentManager. Soul is just a section of the system prompt.

#### 4. Extension/MCP — Reuse, Adapt to Tool Trait

MCP Transport and Extension Runtime internals unchanged. Only wrap with unified Tool trait.

---

## Module Disposition

### Remove (LLM does this better)

| Module | LOC | Reason |
|--------|-----|--------|
| `intent/` | ~15k | LLM understands intent via prompt |
| `poe/` | ~55k | LLM self-evaluates completion |
| `resilience/` | ~25k | Not needed for personal assistant |
| `dispatcher/` (Cortex) | ~45k | Flat ToolRegistry replaces |
| `memory/evolution/` | ~10k | Part of POE |
| `suggestion/` | ~5k | LLM decides autonomously |

### Rewrite / Simplify

| Module | Action |
|--------|--------|
| `agent_loop/` | Rewrite: 5-phase → 2-step loop |
| `thinker/` | Simplify: keep streaming, remove DecisionParser |
| `executor/` | Merge into ToolRegistry.execute |
| `daemon/` | Simplify: keep event source, remove WorldModel/Perception |
| `domain/` | Simplify: keep core types |
| `components/` | Simplify: keep necessary UI components |

### Preserve (Empowerment layers)

| Module | Reason |
|--------|--------|
| `gateway/` | Multi-protocol I/O |
| `providers/` | Multi-vendor LLM |
| `memory/store/` | Hybrid Retrieval core |
| `memory/hybrid_retrieval/` | Search quality |
| `memory/scoring_pipeline/` | Retrieval ranking |
| `compressor/` | Token limit management |
| `mcp/` | External service access |
| `extension/` | Plugin ecosystem |
| `config/` | Hot-reload configuration |
| `tools/` (builtin) | Built-in capabilities |
| `group_chat/` | Multi-persona conversations |
| `generation/` | Media generation tools |
| `a2a/` | Agent-to-agent (evaluate) |

---

## Migration Strategy

Phase-by-phase, each phase independently deployable:

### Phase 1: New Core Loop (parallel build)
- Build new `AgentLoop` alongside old one
- Feature flag to switch between old/new
- Validate with single provider + builtin tools

### Phase 2: Tool Flattening
- Adapt existing tools to new `Tool` trait
- Build flat `ToolRegistry`
- Wire MCP/Extension tools through new trait

### Phase 3: Prompt Migration
- Build `PromptBuilder` with Soul + rules + memory
- Migrate Intent Detection logic → prompt directives
- Migrate POE evaluation → prompt instructions

### Phase 4: Module Removal
- Remove `dispatcher/`, `intent/`, `poe/`, `resilience/`
- Simplify `daemon/`, `thinker/`, `executor/`
- Clean up unused traits and types

### Phase 5: Validation
- Full regression testing across all channels
- Performance benchmarks (latency, token usage)
- Memory retrieval quality comparison
