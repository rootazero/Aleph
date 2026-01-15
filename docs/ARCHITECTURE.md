# Aether Core Architecture

This document describes the internal architecture of Aether's Rust core, particularly the **Structured Context Protocol** for intelligent request routing and context injection.

## Table of Contents

- [Overview](#overview)
- [Structured Context Protocol](#structured-context-protocol)
- [Request Processing Flow](#request-processing-flow)
- [Core Components](#core-components)
- [Data Structures](#data-structures)
- [Configuration Reference](#configuration-reference)

---

## Overview

Aether's core uses a **structured payload-based architecture** that replaces simple string concatenation with a rich, type-safe data flow system. This enables:

- **Intelligent routing** based on user intent and context
- **Dynamic capability execution** (Memory ✅, Search ✅, MCP tools 🔮)
- **Flexible context formatting** (Markdown, XML, JSON)
- **Transparent memory augmentation** (local RAG)
- **Web search integration** with 6 providers and fallback mechanism

---

## Structured Context Protocol

### Design Principles

1. **Payload-First**: All data flows through `AgentPayload` structures
2. **Declarative Configuration**: Capabilities and intents defined in config files
3. **Separation of Concerns**: Router → Executor → Assembler → Provider
4. **Transparent Memory**: Context injection invisible to AI providers

### Architecture Diagram

```
User Input
    ↓
[Router] ──────────────────────┐
    ↓                          │
[RoutingDecision]              │
    ↓                          │
[PayloadBuilder] ←─────────────┘
    ↓
[AgentPayload]
    ↓
[CapabilityExecutor]
    ├─→ [Memory] ──────→ Vector DB
    ├─→ [Search] ──────→ SearchRegistry → Providers (Tavily/SearXNG/Google/Bing/Brave/Exa)
    └─→ [MCP] ─────────→ (Future)
    ↓
[AgentPayload + Context]
    ↓
[PromptAssembler]
    ↓
Formatted System Prompt
    ↓
[AiProvider]
```

---

## Request Processing Flow

### 1. Routing Phase

```rust
// User presses hotkey, Router analyzes input
let decision = router.make_decision(user_input)?;

// RoutingDecision contains:
// - provider_name: "openai"
// - intent: Intent::Custom("translation")
// - capabilities: vec![Capability::Memory]
// - context_format: ContextFormat::Markdown
// - processed_input: cleaned user text
```

### 2. Payload Building Phase

```rust
// PayloadBuilder constructs structured payload
let payload = PayloadBuilder::new()
    .config(provider, capabilities, format)
    .meta(intent, timestamp, app_context)
    .user_input(processed_input)
    .build()?;
```

### 3. Capability Execution Phase

```rust
// CapabilityExecutor runs capabilities in priority order
executor.execute_all(&mut payload)?;

// Execution sequence (sorted by Capability::Memory(0) < Search(1) < Mcp(2)):
// 1. Memory: Retrieve similar conversations from vector DB
// 2. Search: Execute web search via SearchRegistry (Tavily/SearXNG/Google/Bing/Brave/Exa)
// 3. MCP: (Reserved) Tool/resource calls
```

### 4. Context Assembly Phase

```rust
// PromptAssembler formats context into system prompt
let assembler = PromptAssembler::new(ContextFormat::Markdown);
let final_prompt = assembler.assemble_system_prompt(
    &base_system_prompt,
    &payload
);

// Result (example):
// "You are a helpful assistant.
//
// ### Context Information
//
// **Relevant History**:
// 1. Conversation at 2024-01-15 10:30:00 UTC
//    App: com.apple.Notes
//    Window: Translation.txt
//    User: How to say hello in French?
//    AI: "Bonjour" or "Salut"
//    Relevance: 0.89
// ..."
```

### 5. Provider Invocation Phase

```rust
// AI Provider receives assembled prompt
let response = provider.chat(
    &final_prompt,
    &payload.user_input,
    temperature,
    max_tokens
).await?;
```

---

## Core Components

### Router

**Location**: `core/src/router/mod.rs`

**Responsibilities**:
- Match user input against routing rules (regex patterns)
- Infer intent from rule configuration
- Select appropriate AI provider
- Build initial `AgentPayload`

**Key Methods**:
```rust
pub fn route(
    &self,
    user_input: &str,
    context: &CapturedContext
) -> Result<(Arc<dyn AiProvider>, AgentPayload)>
```

**Data Stored**:
- `rules: Vec<RoutingRule>` - Compiled regex patterns
- `rule_configs: Vec<RoutingRuleConfig>` - Full config with capabilities
- `providers: HashMap<String, Arc<dyn AiProvider>>` - Provider instances

---

### CapabilityExecutor

**Location**: `core/src/capability/mod.rs`

**Responsibilities**:
- Execute capabilities in sorted priority order
- Populate `payload.context` fields with retrieved data
- Handle failures gracefully (warn but don't block)

**Capability Priority**:
```rust
pub enum Capability {
    Memory = 0,  // Highest priority
    Search = 1,
    Mcp = 2,     // Lowest priority
}
```

**Execution Logic**:
```rust
pub async fn execute_all(&self, mut payload: AgentPayload) -> Result<AgentPayload> {
    for capability in sorted_capabilities {
        payload = match capability {
            Capability::Memory => self.execute_memory(payload).await?,
            Capability::Search => self.execute_search(payload).await?,
            Capability::Mcp => {
                warn!("MCP not implemented");
                payload
            },
        }
    }
    Ok(payload)
}
```

---

### PromptAssembler

**Location**: `core/src/payload/assembler.rs`

**Responsibilities**:
- Format context data into LLM-readable text
- Support multiple output formats (Markdown, XML, JSON)
- Truncate long content (200 char limit per entry)
- Format timestamps as human-readable strings

**Format Support**:
- ✅ **Markdown**: Fully implemented (MVP)
- 🔮 **XML**: Reserved for Claude (structured tags)
- 🔮 **JSON**: Reserved for function calling

**Markdown Format Example**:
```markdown
### Context Information

**Relevant History**:
1. Conversation at 2024-01-15 10:30:00 UTC
   App: com.apple.Notes
   Window: Translation.txt
   User: How to say hello in French?
   AI: "Bonjour" or "Salut"
   Relevance: 0.89
```

---

### Memory Integration

**Location**: `core/src/memory/`

**How It Works**:
1. User input is embedded using `bge-small-zh-v1.5` model (512-dim, Chinese-optimized)
2. Vector DB (`sqlite-vec`) searches for similar past conversations
3. Top-k entries above threshold are retrieved
4. Entries include similarity scores and context metadata
5. PromptAssembler formats them into readable context

**Configuration**:
```toml
[memory]
enabled = true
max_context_items = 5          # Max number of entries to retrieve
similarity_threshold = 0.7     # Min cosine similarity (0.0-1.0)
```

**Privacy**:
- All data stored locally in `~/.config/aether/memory.db`
- No raw memory data sent to cloud LLMs
- Only formatted context snippets injected into prompts

---

### Cowork Task Orchestration

**Location**: `core/src/cowork/`

**Purpose**: Decomposes complex user requests into DAG-structured task graphs and executes them with parallel scheduling.

**Components**:
- `TaskPlanner` - LLM-based task decomposition
- `DAGScheduler` - Topological sort execution with parallelism
- `ExecutorRegistry` - Extensible task execution backends
- `TaskMonitor` - Real-time progress tracking
- `CoworkEngine` - Unified API

**Execution Flow**:
```
User Request → TaskPlanner → TaskGraph → User Confirmation → DAGScheduler → Results
                   ↓              ↓                              ↓
               LLM Inference   Validation                  Parallel Execution
```

**Configuration**:
```toml
[cowork]
enabled = true
require_confirmation = true
max_parallelism = 4
dry_run = false
```

**Task Categories**:
- `FileOperation` - File system operations
- `CodeExecution` - Script/code execution
- `DocumentGeneration` - Document creation
- `AppAutomation` - AppleScript automation
- `AiInference` - AI model calls

See [COWORK.md](./COWORK.md) for detailed documentation.

---

## Data Structures

### AgentPayload

**Location**: `core/src/payload/mod.rs`

The central data structure for all request processing:

```rust
pub struct AgentPayload {
    /// Configuration (provider, capabilities, format)
    pub config: PayloadConfig,

    /// Metadata (intent, timestamp, app context)
    pub meta: PayloadMeta,

    /// User's original input
    pub user_input: String,

    /// Retrieved context data from capabilities
    pub context: AgentContext,

    /// Optional override settings
    pub overrides: Option<PayloadOverrides>,
}
```

### AgentContext

Context data populated by capability executors:

```rust
pub struct AgentContext {
    /// Memory: Retrieved conversation history
    pub memory_snippets: Option<Vec<MemoryEntry>>,

    /// Search: Web/knowledge base results (reserved)
    pub search_results: Option<Vec<SearchResult>>,

    /// MCP: Tool/resource outputs (reserved)
    pub mcp_resources: Option<Vec<McpResource>>,
}
```

### Intent Classification

Inferred from configuration to guide routing:

```rust
pub enum Intent {
    /// Built-in search feature ("/search")
    BuiltinSearch,

    /// Built-in MCP tool calls ("/mcp")
    BuiltinMcp,

    /// User-defined workflows (reserved for skills)
    Skills(String),

    /// Custom user intents ("translation", "summarization")
    Custom(String),

    /// Default: General conversation
    GeneralChat,
}
```

### Capability Enum

Ordered by priority for execution:

```rust
#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub enum Capability {
    Memory = 0,  // Execute first
    Search = 1,
    Mcp = 2,     // Execute last
}
```

---

## Configuration Reference

### Routing Rule with Capabilities

```toml
[[rules]]
regex = "^/translate"
provider = "openai"
system_prompt = "You are a translator."

# New fields for Structured Context Protocol
capabilities = ["memory"]           # Enable Memory capability
intent_type = "translation"         # Custom intent classification
context_format = "markdown"         # Output format for context
```

### Field Descriptions

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `regex` | String | Required | Regex pattern to match user input |
| `provider` | String | Required | AI provider name (openai, claude, etc.) |
| `system_prompt` | String | Optional | Base system prompt for this rule |
| `capabilities` | Array | `[]` | Capabilities to enable: `["memory", "search", "mcp"]` |
| `intent_type` | String | `null` | Intent classification (used for routing logic) |
| `context_format` | String | `"markdown"` | Context format: `"markdown"`, `"xml"`, `"json"` |

### Intent Mapping

Configuration `intent_type` maps to Rust `Intent` enum:

| Config Value | Rust Enum | Description |
|-------------|-----------|-------------|
| `"search"` | `Intent::BuiltinSearch` | Built-in search feature |
| `"mcp"` | `Intent::BuiltinMcp` | Built-in MCP integration |
| `"translation"` | `Intent::Custom("translation")` | Custom intent |
| (not set) | `Intent::GeneralChat` | Default general conversation |

### Capability Parsing

Invalid capability strings are logged and skipped:

```toml
# Valid
capabilities = ["memory", "search"]

# Invalid entry "foo" is skipped with warning
capabilities = ["memory", "foo", "search"]
# Result: vec![Capability::Memory, Capability::Search]
```

---

## Extension Points

### Adding New Capabilities

1. Add variant to `Capability` enum with priority number:
```rust
pub enum Capability {
    Memory = 0,
    Search = 1,
    Mcp = 2,
    YourCapability = 3,  // Lower priority = executes later
}
```

2. Implement execution logic in `CapabilityExecutor`:
```rust
match capability {
    Capability::YourCapability => self.execute_your_capability(payload)?,
    // ...
}
```

3. Add parsing support in `Capability::parse()`:
```rust
pub fn parse(s: &str) -> Result<Self, String> {
    match s.to_lowercase().as_str() {
        "your_capability" => Ok(Capability::YourCapability),
        // ...
    }
}
```

### Adding New Context Formats

1. Add variant to `ContextFormat` enum:
```rust
pub enum ContextFormat {
    Markdown,
    Xml,
    Json,
    YourFormat,
}
```

2. Implement formatting in `PromptAssembler`:
```rust
match self.format {
    ContextFormat::YourFormat => self.format_your_format(payload),
    // ...
}
```

---

## Intent Routing Pipeline

**Location**: `core/src/routing/`

The Intent Routing Pipeline is an enhanced multi-layer routing system that optimizes intent detection through caching, confidence calibration, and intelligent layer execution.

### Architecture Overview

```
User Input
    ↓
┌──────────────────────────────────────────────────────────────┐
│                 Intent Routing Pipeline                       │
│                                                               │
│  ┌─────────────┐    ┌──────────────────────────────────────┐ │
│  │ IntentCache │◄───│ Check cache for matching intent      │ │
│  └─────────────┘    └──────────────────────────────────────┘ │
│         │ miss                                                │
│         ▼                                                     │
│  ┌──────────────────────────────────────────────────────────┐│
│  │              LayerExecutionEngine                        ││
│  │                                                          ││
│  │  ┌─────────┐   ┌─────────┐   ┌─────────┐                ││
│  │  │   L1    │──▶│   L2    │──▶│   L3    │                ││
│  │  │ Regex   │   │Semantic │   │LLM Infer│                ││
│  │  └─────────┘   └─────────┘   └─────────┘                ││
│  │      │              │             │                      ││
│  │      ▼              ▼             ▼                      ││
│  │  IntentSignal  IntentSignal  IntentSignal               ││
│  └──────────────────────────────────────────────────────────┘│
│         │                                                     │
│         ▼                                                     │
│  ┌──────────────────────────────────────────────────────────┐│
│  │              IntentAggregator                            ││
│  │  • Sort by calibrated confidence                         ││
│  │  • Detect conflicts                                       ││
│  │  • Check parameter completeness                          ││
│  │  • Determine action (Execute/Confirm/Clarify/Chat)       ││
│  └──────────────────────────────────────────────────────────┘│
│         │                                                     │
│         ▼                                                     │
│  ┌──────────────────────────────────────────────────────────┐│
│  │  Action Handling                                         ││
│  │  • Execute: Run tool directly                            ││
│  │  • RequestConfirmation: Show confirmation UI             ││
│  │  • RequestClarification: Ask for missing parameters      ││
│  │  • GeneralChat: Fall back to AI chat                     ││
│  └──────────────────────────────────────────────────────────┘│
└──────────────────────────────────────────────────────────────┘
```

### Core Components

#### IntentCache

**Location**: `core/src/routing/cache.rs`

LRU-based cache with time decay for fast-path routing:

```rust
pub struct IntentCache {
    cache: Arc<RwLock<LruCache<u64, CachedIntent>>>,
    config: CacheConfig,
    metrics: CacheMetricsTracker,
}

pub struct CachedIntent {
    pub tool_name: String,
    pub parameters: Value,
    pub confidence: f32,
    pub hit_count: u32,
    pub success_count: u32,
    pub failure_count: u32,
}
```

**Features**:
- Time-based confidence decay (configurable half-life)
- Success/failure tracking for learning
- Automatic eviction of failed entries
- Thread-safe operations with RwLock

#### LayerExecutionEngine

**Location**: `core/src/routing/engine.rs`

Orchestrates L1/L2/L3 layer execution with early exit optimization:

```rust
pub enum ExecutionMode {
    Sequential,  // L1 → L2 → L3 in order
    Parallel,    // L1 + L2 concurrent, then L3
}
```

**Layer Cascade**:
1. **L1 (Regex)**: Exact command matching (`/search`, `/translate`)
2. **L2 (Semantic)**: Keyword-based intent detection
3. **L3 (LLM)**: AI inference for ambiguous queries

**Early Exit**:
- If L1 matches with confidence ≥ 0.9, skip L2/L3
- If L2 matches with confidence ≥ `l2_skip_l3_threshold`, skip L3

#### ConfidenceCalibrator

**Location**: `core/src/routing/calibrator.rs`

Adjusts raw confidence scores using multiple factors:

```rust
pub struct ConfidenceCalibrator {
    config: CalibratorConfig,
    history: CalibrationHistory,
}

pub struct CalibrationFactor {
    pub name: String,
    pub value: f32,
    pub weight: f32,
}
```

**Calibration Factors**:
- Layer-specific adjustments (L1 boosted, L3 reduced)
- Tool-specific history (success rate boost)
- Context-based adjustments (app context, conversation)

#### IntentAggregator

**Location**: `core/src/routing/aggregator.rs`

Combines signals from multiple layers:

```rust
pub struct AggregatedIntent {
    pub primary_signal: IntentSignal,
    pub alternatives: Vec<IntentSignal>,
    pub action: IntentAction,
    pub final_confidence: f32,
    pub has_conflict: bool,
    pub missing_parameters: Vec<ParameterRequirement>,
}

pub enum IntentAction {
    Execute,
    RequestConfirmation,
    RequestClarification { prompt: String, suggestions: Vec<String> },
    GeneralChat,
}
```

**Conflict Detection**:
- If top two signals have different tools but similar confidence (within threshold), mark as conflict
- Conflicts trigger confirmation instead of auto-execute

#### ClarificationIntegrator

**Location**: `core/src/routing/clarification.rs`

Manages multi-turn clarification flows:

```rust
pub struct ClarificationIntegrator {
    pending: Arc<RwLock<HashMap<String, PendingClarification>>>,
    config: ClarificationConfig,
}

pub struct PendingClarification {
    pub session_id: String,
    pub original_input: String,
    pub partial_intent: AggregatedIntent,
    pub collected_params: Value,
    pub missing_params: Vec<ParameterRequirement>,
    pub created_at: Instant,
}
```

**Flow**:
1. Start clarification: Store pending state, return ClarificationRequest
2. Resume: User provides input, merge with collected params
3. Check completeness: If all params provided, continue to execution
4. Timeout: Cleanup expired sessions (configurable TTL)

### Configuration

```toml
[routing.pipeline]
enabled = true                    # Enable Intent Routing Pipeline

[routing.pipeline.cache]
enabled = true
max_size = 1000
ttl_seconds = 3600
decay_half_life_seconds = 600
cache_auto_execute_threshold = 0.85

[routing.pipeline.layers]
execution_mode = "sequential"
l1_enabled = true
l2_enabled = true
l3_enabled = true
l3_timeout_ms = 5000
l2_skip_l3_threshold = 0.85

[routing.pipeline.confidence]
auto_execute = 0.9
requires_confirmation = 0.6
no_match = 0.3

[routing.pipeline.clarification]
enabled = true
timeout_seconds = 300
max_turns = 5

[[routing.pipeline.tools]]
name = "search"
min_threshold = 0.5
auto_execute_threshold = 0.85
repeat_boost = 0.1
```

### Performance

| Operation | Target | Typical |
|-----------|--------|---------|
| Cache hit | < 50ms | ~10ms |
| L1 only | < 100ms | ~20ms |
| L1 + L2 | < 200ms | ~50ms |
| Full cascade | < 500ms | ~200ms |
| L3 inference | < 5s | ~1-3s |

### Testing

Integration tests: `core/src/tests/pipeline_integration.rs` (23 tests)
Performance benchmarks: `core/benches/pipeline_bench.rs`

```bash
# Run pipeline tests
cargo test tests::pipeline_integration --lib

# Run benchmarks
cargo bench --bench pipeline_bench
```

---

## Performance Considerations

### Latency Targets

Based on design estimates:

| Operation | Target | Measured |
|-----------|--------|----------|
| Payload building | < 20ms | ~10ms |
| Memory retrieval | < 50ms | ~30ms |
| Context assembly | < 10ms | ~5ms |
| **Total overhead** | **< 80ms** | **~45ms** |

### Optimization Strategies

1. **Lazy Loading**: Embedding model loaded on first use
2. **Connection Pooling**: Reuse DB connections
3. **Parallel Execution**: Capabilities could run concurrently (future)
4. **Caching**: LRU cache for frequent queries (future)

---

## Testing

All core components have comprehensive test coverage:

- **Payload Builder**: 8 tests (builder pattern, validation)
- **PromptAssembler**: 7 tests (formatting, truncation, timestamps)
- **Capability Executor**: 4 tests (execution order, error handling)
- **Router Integration**: 20+ tests (matching, decision building)

Run tests:
```bash
cd Aether/core
cargo test --lib payload router capability
```

---

## Future Enhancements

### Search Capability

**Status**: ✅ Implemented (2026-01-04)

**Implementation Details**:
- **6 search providers**: Tavily, SearXNG, Google CSE, Bing, Brave, Exa.ai
- **Provider fallback**: Automatic retry with configurable fallback chain
- **PII scrubbing**: Integrated with global PII settings
- **Timeout protection**: Configurable timeout (default: 10s)
- **Result formatting**: Markdown format for LLM consumption

**Architecture**:
- `SearchProvider` trait for provider abstraction
- `SearchRegistry` for provider management and fallback
- `CapabilityExecutor::execute_search()` for execution
- `PromptAssembler::format_search_results_markdown()` for formatting

**Data Structure**:
```rust
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub full_content: Option<String>,      // Full page content (Exa/Tavily)
    pub source_type: Option<String>,       // article/video/forum
    pub provider: Option<String>,          // Provider name
    pub published_date: Option<i64>,       // Unix timestamp
    pub relevance_score: Option<f32>,      // 0.0-1.0
}
```

**Documentation**: See [SEARCH_INTEGRATION_COMPLETE.md](./SEARCH_INTEGRATION_COMPLETE.md)

### MCP Integration

**Status**: Interface reserved, not implemented

**Planned Architecture**:
- MCP server connections via stdio/HTTP
- Tool discovery and schema validation
- Resource caching and lifecycle management

**Data Structure**:
```rust
pub struct McpResource {
    pub tool_name: String,
    pub parameters: serde_json::Value,
    pub result: String,
}
```

### Skills System

**Status**: ✅ Implemented (2026-01-08)

**Implementation Details**:
- **Claude Agent Skills standard**: SKILL.md format with YAML frontmatter + Markdown body
- **Strategy Pattern integration**: `SkillsStrategy` implements `CapabilityStrategy` trait
- **Installation methods**: GitHub URL, ZIP file upload, manual file placement
- **Hot reload**: Skills changes take effect immediately
- **Built-in skills**: refine-text, translate, summarize

**Architecture**:
- `Skill` struct for skill data parsing
- `SkillsRegistry` for skill management and lookup
- `SkillsInstaller` for GitHub/ZIP installation
- `SkillsStrategy` (priority 4) for capability execution
- `PromptAssembler::format_skill_instructions_markdown()` for formatting

**Data Structure**:
```rust
pub struct Skill {
    pub id: String,
    pub frontmatter: SkillFrontmatter,  // name, description, allowed_tools
    pub body: String,                    // Markdown instructions
}
```

**Usage**:
```
/skill refine-text Please improve this paragraph...
/skill translate Convert this to French...
/skill summarize Give me a summary of...
```

**Documentation**: See [SKILLS.md](./SKILLS.md)

---

## References

- **OpenSpec Proposal**: `openspec/changes/implement-structured-context-protocol/`
- **Design Document**: `openspec/changes/implement-structured-context-protocol/design.md`
- **Spec Deltas**: `openspec/changes/implement-structured-context-protocol/specs/`
- **Original Design**: `agentstructure.md` (deprecated, see proposal)

---

**Last Updated**: 2026-01-11
**Implemented In**: Aether v0.1.0
**OpenSpec Changes**: `implement-structured-context-protocol`, `add-skills-capability`, `enhance-intent-routing-pipeline`
