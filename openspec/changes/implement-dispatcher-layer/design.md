# Design: Intelligent Dispatcher Layer (Aether Cortex)

## Context

Aether currently has a functional routing system (`Router`) that matches user input against regex rules. However, it lacks:

1. **Unified tool discovery** - Tools are scattered across McpClient, SkillsRegistry, and hardcoded Native handlers
2. **Intelligent intent detection** - No LLM-based fallback for ambiguous inputs
3. **User confirmation** - No way to preview and confirm high-impact operations
4. **Context awareness** - Conversation history not used for pronoun resolution

This design extends the existing architecture to add a **Dispatcher Layer** without breaking backward compatibility.

## Goals

1. Create a unified tool registry that aggregates all tool sources
2. Implement three-layer routing (L1: regex, L2: semantic, L3: LLM)
3. Add ActionProposal with confidence scoring and parameter extraction
4. Integrate Halo confirmation flow for low-confidence matches
5. Enable dynamic prompt generation for L3 routing

## Non-Goals

1. Replace the existing Router - we extend it
2. Implement RAG-based tool selection - deferred to future
3. Support multi-step tool chains - single tool per request only
4. Create new UI components - use existing Halo/clarification

## Decisions

### Decision 1: Extend RoutingMatch vs. Create ActionProposal

**Options:**
- A) Add fields to existing `RoutingMatch` struct
- B) Create new `ActionProposal` struct that wraps `RoutingMatch`

**Decision: Option A** - Extend `RoutingMatch`

**Rationale:**
- `RoutingMatch` already contains most needed data (provider, capabilities, cleaned_input)
- Adding `confidence`, `parameters`, `reason` is minimal change
- Avoids duplication and keeps the data model simple
- Downstream code (CapabilityExecutor, PromptAssembler) already uses RoutingMatch

**Changes to RoutingMatch:**
```rust
pub struct RoutingMatch {
    pub command_rule: Option<MatchedCommandRule>,
    pub keyword_rules: Vec<MatchedKeywordRule>,
    pub confidence: f32,                          // NEW: 0.0 - 1.0
    pub extracted_parameters: Option<Value>,      // NEW: LLM-extracted params
    pub routing_reason: Option<String>,           // NEW: Human-readable explanation
    pub routing_layer: RoutingLayer,              // NEW: Which layer matched
}

pub enum RoutingLayer {
    L1Rule,      // Regex match
    L2Semantic,  // Keyword/similarity
    L3Inference, // LLM-based
    Default,     // No match, using default
}
```

### Decision 2: Where to Place UnifiedToolRegistry

**Options:**
- A) Inside `core/src/dispatcher/` module
- B) Inside `core/src/services/` module
- C) Top-level `core/src/unified_tool.rs`

**Decision: Option A** - `core/src/dispatcher/registry.rs`

**Rationale:**
- Registry is primarily used by Dispatcher for prompt building
- Keeps dispatcher-related code cohesive
- Can be exposed via `pub use` if needed elsewhere

### Decision 3: When to Trigger Halo Confirmation

**Options:**
- A) Always confirm before tool execution
- B) Only when confidence < threshold
- C) Only for specific tool types (e.g., destructive operations)

**Decision: Option B** - Confidence-based triggering

**Rationale:**
- Slash commands (L1) should execute immediately (confidence = 1.0)
- Semantic matches (L2) may need confirmation (confidence = 0.5-0.9)
- LLM inference (L3) often needs confirmation (confidence varies)
- Threshold is configurable (default: 0.8)

**Configuration:**
```toml
[dispatcher]
enabled = true
confirmation_threshold = 0.8  # 0.0 = always confirm, 1.0 = never confirm
l3_enabled = true             # Enable LLM-based routing
l3_model = "gpt-4o-mini"      # Fast model for routing
```

### Decision 4: L3 Routing Model Selection

**Options:**
- A) Use the same provider as the final request
- B) Use a dedicated fast model (gpt-4o-mini, haiku)
- C) Use local model (Ollama)

**Decision: Option B** - Dedicated fast model

**Rationale:**
- Routing needs speed, not depth (200-500ms target)
- gpt-4o-mini is cost-effective and fast
- Separates routing concern from content generation
- Configurable for users who prefer local models

### Decision 5: Dynamic Prompt Format

**Options:**
- A) JSON array of tool definitions
- B) Markdown list with inline descriptions
- C) XML-like structured format

**Decision: Option B** - Markdown list

**Rationale:**
- LLMs handle markdown well
- Concise and human-readable
- Easy to debug and log

**Format Example:**
```
You are an intent classifier. Analyze the user's input and determine which tool to use.

### Available Tools

- **search**: Search the web for real-time information. Args: query (string), limit (number, optional)
- **git_status** [MCP:github]: Check repository status. Args: repo (string)
- **refine-text** [Skill]: Improve and polish writing. Args: none

### Instructions
1. Output JSON: {"tool": "name", "parameters": {...}, "confidence": 0.0-1.0, "reason": "..."}
2. If no tool matches, output: {"tool": null, "confidence": 1.0, "reason": "General conversation"}
```

### Decision 6: Tool Source Representation

**Decision:** Use enum with payload for MCP/Skill sources

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolSource {
    Native,                    // Built-in: Search, Video
    Mcp { server: String },    // MCP server name
    Skill { id: String },      // Skill directory name
    Custom { rule_index: usize }, // User-defined slash command
}
```

### Decision 7: Conversation Context Injection

**How to inject context for pronoun resolution:**

```rust
pub struct RoutingContext {
    pub input: String,
    pub conversation_summary: Option<String>,  // Last 3 turns summarized
    pub mentioned_entities: Vec<String>,       // Extracted entities (names, topics)
}
```

**L3 Prompt Enhancement:**
```
### Conversation Context
- User mentioned: "Keanu Reeves" (2 turns ago)
- Topic: Discussing movie actors

### Current Input
"What movies did he star in?"
```

## Architecture Diagram

```
                    ┌─────────────────────────────────────────┐
                    │              User Input                 │
                    └─────────────────┬───────────────────────┘
                                      │
                    ┌─────────────────▼───────────────────────┐
                    │           Dispatcher Layer              │
                    │                                         │
                    │  ┌─────────────────────────────────┐   │
                    │  │      UnifiedToolRegistry        │   │
                    │  │  ┌──────┬──────┬──────┬──────┐ │   │
                    │  │  │Native│ MCP  │Skills│Custom│ │   │
                    │  │  └──────┴──────┴──────┴──────┘ │   │
                    │  └────────────────┬────────────────┘   │
                    │                   │                     │
                    │  ┌────────────────▼────────────────┐   │
                    │  │        MultiLayerRouter         │   │
                    │  │  ┌────────────────────────────┐ │   │
                    │  │  │ L1: RuleRouter (existing)  │ │   │
                    │  │  │     < 10ms, regex match    │ │   │
                    │  │  └────────────┬───────────────┘ │   │
                    │  │               │ miss            │   │
                    │  │  ┌────────────▼───────────────┐ │   │
                    │  │  │ L2: SemanticMatcher        │ │   │
                    │  │  │     200-500ms, keywords    │ │   │
                    │  │  └────────────┬───────────────┘ │   │
                    │  │               │ miss/low conf   │   │
                    │  │  ┌────────────▼───────────────┐ │   │
                    │  │  │ L3: AiRouter (new)         │ │   │
                    │  │  │     > 1s, LLM inference    │ │   │
                    │  │  └────────────┬───────────────┘ │   │
                    │  └───────────────┼─────────────────┘   │
                    │                  │                      │
                    │  ┌───────────────▼─────────────────┐   │
                    │  │     RoutingMatch (extended)     │   │
                    │  │  - tool, parameters, confidence │   │
                    │  │  - routing_layer, reason        │   │
                    │  └───────────────┬─────────────────┘   │
                    │                  │                      │
                    │         if confidence < threshold       │
                    │                  │                      │
                    │  ┌───────────────▼─────────────────┐   │
                    │  │     Halo Confirmation           │   │
                    │  │  [🔍 Search] "today's news"    │   │
                    │  │  [Enter: Execute] [Esc: Cancel] │   │
                    │  └───────────────┬─────────────────┘   │
                    │                  │                      │
                    └──────────────────┼──────────────────────┘
                                       │
                    ┌──────────────────▼──────────────────────┐
                    │         Execution Layer (existing)       │
                    │                                          │
                    │  CapabilityExecutor → PromptAssembler   │
                    │         ↓                                │
                    │  Memory → Search → MCP → Video → Skills │
                    │         ↓                                │
                    │     AI Provider.process()               │
                    └──────────────────────────────────────────┘
```

## Data Flow

### Normal Flow (L1 Match, High Confidence)

```
1. User: "/search today's news"
2. L1 RuleRouter: Match "/search" → confidence: 1.0
3. Confirmation: Skip (confidence >= threshold)
4. CapabilityExecutor: Execute Search capability
5. Result: Search results injected into prompt
```

### Confirmation Flow (L2/L3 Match, Low Confidence)

```
1. User: "Help me find news about AI"
2. L1 RuleRouter: No match
3. L2 SemanticMatcher: "find" + "news" → Search, confidence: 0.7
4. Confirmation: Trigger Halo
   - Display: [🔍 Search] "AI news"
   - User presses Enter
5. CapabilityExecutor: Execute Search
6. Result: Search results injected
```

### Context-Aware Flow (L3 with Pronouns)

```
1. Previous turn: "I love Keanu Reeves movies"
2. User: "Search for his latest film"
3. L1: No match
4. L2: "Search for" → Search, but "his" unresolved
5. L3 AiRouter:
   - Context: User mentioned "Keanu Reeves"
   - Infer: "his" = "Keanu Reeves"
   - Output: Search("Keanu Reeves latest film"), confidence: 0.85
6. Confirmation: Skip (0.85 >= 0.8)
7. Execute Search with resolved query
```

## Risks / Trade-offs

### Risk 1: L3 Latency
- **Problem**: LLM routing adds 1-2s latency
- **Mitigation**: L3 only triggered when L1/L2 fail; use fast model
- **Fallback**: If L3 times out, treat as general chat

### Risk 2: Confirmation Fatigue
- **Problem**: Too many confirmations annoy users
- **Mitigation**: Configurable threshold; learn from user patterns (future)
- **Monitoring**: Track confirmation accept/reject ratio

### Risk 3: Registry Synchronization
- **Problem**: Tool list may become stale if MCP reconnects
- **Mitigation**: Refresh on MCP connection; config hot-reload triggers refresh

### Risk 4: Prompt Injection in L3
- **Problem**: User input could manipulate routing decision
- **Mitigation**: Separate routing prompt from content; sanitize input

## Migration Plan

### Phase 1: Foundation (No Breaking Changes)
1. Create `dispatcher/` module structure
2. Implement `UnifiedTool` and `ToolRegistry`
3. Extend `RoutingMatch` with new fields
4. Add dispatcher configuration section

### Phase 2: L2 Enhancement
1. Enhance `SemanticMatcher` with confidence scoring
2. Integrate `RoutingLayer` tracking
3. Add unit tests for semantic matching

### Phase 3: L3 Integration
1. Wire `AiIntentDetector` into routing flow
2. Implement `DynamicPromptBuilder`
3. Add conversation context injection
4. Integration tests for L3 routing

### Phase 4: Halo Confirmation
1. Implement confirmation trigger logic
2. Create confirmation UI format (ClarificationRequest)
3. Handle user response (execute/cancel/edit)
4. E2E tests for confirmation flow

### Rollback
- Set `[dispatcher].enabled = false` to disable entirely
- Set `[dispatcher].l3_enabled = false` to disable L3 only
- No data migration needed

## Open Questions

1. **Should we cache L3 routing decisions?**
   - Pro: Faster repeat queries
   - Con: Context changes may invalidate cache
   - Proposal: No caching in MVP, add later if needed

2. **How to handle multi-language intent detection?**
   - Current: Rely on LLM's multilingual capability
   - Future: Add language-specific keyword hints

3. **Should confirmation support parameter editing?**
   - MVP: No, only confirm/cancel
   - Future: Add inline parameter editing UI
