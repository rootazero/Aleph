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
- **Dynamic capability execution** (Memory ✅, Search ✅, MCP tools ✅)
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
    └─→ [MCP] ─────────→ MCP Servers (Local/Remote)
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
// 3. MCP: Tool/resource calls via MCP servers
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

**Location**: `core/src/intent/decision/router.rs` (IntentRouter)

**Responsibilities**:
- Match user input against routing rules
- Route based on intent classification results
- Select appropriate execution path
- Build routing decisions with context signals

**Key Methods**:
```rust
pub fn route(
    &self,
    input: &str,
    context: Option<&ContextSignals>
) -> RouteResult
```

**Related Components**:
- `intent/detection/` - L1/L2/L3 intent classification
- `intent/decision/` - Routing decision logic
- `dispatcher/model_router/` - Model selection routing

---

### CapabilityExecutor

**Location**: `core/src/capability/` (CapabilitySystem in `system.rs`)

**Responsibilities**:
- Execute capabilities via Strategy pattern
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
            Capability::Mcp => self.execute_mcp(payload).await?,
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

### Sub-Agent Synchronization

**Location**: `core/src/agents/sub_agents/`

**Purpose**: Enables synchronous delegation to specialized sub-agents with result aggregation.

#### Components

| Component | Location | Description |
|-----------|----------|-------------|
| **ExecutionCoordinator** | `coordinator.rs` | Manages synchronous wait using oneshot channels |
| **ResultCollector** | `result_collector.rs` | Aggregates tool calls and artifacts |
| **SubAgentDispatcher** | `dispatcher.rs` | Routes requests to specialized sub-agents |

#### Execution Flow

```
Main Agent
    │
    ▼ dispatch_sync()
┌──────────────────────────────────────────────────────┐
│               SubAgentDispatcher                      │
│                                                       │
│  1. Initialize ResultCollector for request            │
│  2. Start ExecutionCoordinator tracking               │
│  3. Spawn sub-agent execution                         │
│  4. Wait for completion (with timeout)                │
│  5. Collect tool summaries and artifacts              │
│  6. Return enriched SubAgentResult                    │
└──────────────────────────────────────────────────────┘
    │
    ▼
SubAgentResult {
    request_id, success, summary,
    tools_called: Vec<ToolCallRecord>,
    artifacts: Vec<Artifact>
}
```

#### Key Features

- **Synchronous Wait**: Block until sub-agent completes or timeout
- **Parallel Execution**: `dispatch_parallel_sync()` for multiple sub-agents
- **Result Aggregation**: Automatic collection of tool calls and artifacts
- **Context Propagation**: Pass parent context to sub-agents
- **Concurrency Control**: Semaphore-based limiting

#### Configuration

```toml
[subagent]
execution_timeout_ms = 300000  # 5 minutes
result_ttl_ms = 3600000        # 1 hour retention
max_concurrent = 5             # Max parallel sub-agents
progress_events_enabled = true # Emit progress events
```

---

### Cowork Task Orchestration

**Location**: `core/src/dispatcher/`

**Purpose**: Decomposes complex user requests into DAG-structured task graphs and executes them with parallel scheduling.

**Components**:
- `dispatcher/planner/` - LLM-based task decomposition
- `dispatcher/scheduler/` - DAG scheduling with topological sort
- `dispatcher/executor/` - Task execution backends
- `dispatcher/monitor/` - Real-time progress tracking
- `dispatcher/cowork_types/` - DAG task definitions (Task, TaskGraph, TaskDependency)
- `dispatcher/engine.rs` - CoworkEngine unified API

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

See [DISPATCHER.md](./DISPATCHER.md#task-orchestration-cowork) for detailed documentation.

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

**Location**: `core/src/intent/`

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

**Location**: `core/src/intent/support/cache.rs`

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

**Location**: `core/src/intent/detection/classifier.rs`

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

**Location**: `core/src/intent/decision/calibrator.rs`

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

**Location**: `core/src/intent/decision/aggregator.rs`

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

**Location**: `core/src/clarification/`

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

Integration tests: `core/src/tests/intent_integration.rs`
Performance benchmarks: `core/benches/` (if available)

```bash
# Run intent integration tests
cargo test tests::intent_integration --lib

# Run all tests
cargo test --lib
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
cd core
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

**Status**: ✅ Fully Implemented (Enhanced 2026-01-24)

**Location**: `core/src/mcp/`

**Implementation Details**:
- **Transport Abstraction**: `McpTransport` trait with multiple implementations
  - `StdioTransport` - Local servers via subprocess stdio
  - `HttpTransport` - Remote servers via HTTP POST
  - `SseTransport` - Remote servers via HTTP + Server-Sent Events
- **Client**: `McpClient` for JSON-RPC 2.0 protocol with remote server support
- **Server management**: Start/stop/restart both local and remote MCP servers
- **Tool discovery**: Automatic tool registration from MCP servers
- **Resource management**: `McpResourceManager` for files, data, content
- **Prompt templates**: `McpPromptManager` for reusable prompts
- **Notifications**: `McpNotificationRouter` for server events
- **OAuth 2.0 Authentication**: Complete auth flow for remote servers
  - PKCE (Proof Key for Code Exchange) support
  - Dynamic Client Registration (RFC 7591)
  - Secure credential storage with Unix permissions
  - Token refresh and expiration handling
  - Lightweight HTTP callback server for authorization codes

**Architecture**:
```
mcp/
├── mod.rs                    # Module exports
├── client.rs                 # McpClient, McpClientBuilder
├── types.rs                  # MCP protocol types, configs
├── notifications.rs          # McpNotificationRouter, McpEvent
├── prompts.rs                # McpPromptManager
├── resources.rs              # McpResourceManager
├── jsonrpc/                  # JSON-RPC 2.0 protocol
├── external/                 # Server connection management
│   ├── connection.rs         # McpServerConnection
│   └── runtime.rs            # Runtime detection (node, python, bun)
├── transport/                # Transport layer
│   ├── traits.rs             # McpTransport trait
│   ├── stdio.rs              # StdioTransport (local)
│   ├── http.rs               # HttpTransport (remote)
│   └── sse.rs                # SseTransport (bidirectional)
└── auth/                     # OAuth authentication
    ├── storage.rs            # OAuthStorage, OAuthTokens
    ├── provider.rs           # OAuthProvider (PKCE flow)
    └── callback.rs           # CallbackServer
```

**Key Types**:
```rust
// Transport trait for different connection methods
#[async_trait]
pub trait McpTransport: Send + Sync {
    async fn send_request(&self, request: &JsonRpcRequest) -> Result<JsonRpcResponse>;
    async fn send_notification(&self, notification: &JsonRpcNotification) -> Result<()>;
    async fn is_alive(&self) -> bool;
    async fn close(&self) -> Result<()>;
    fn server_name(&self) -> &str;
}

// Remote server configuration
pub struct McpRemoteServerConfig {
    pub name: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub transport: TransportPreference,  // Auto, Http, Sse
    pub timeout_seconds: Option<u64>,
}

// OAuth tokens for authenticated servers
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub scope: Option<String>,
}
```

### Extension System (Plugin System v2)

**Status**: ✅ Implemented (2026-01-24, Migrated from plugins/)

**Location**: `core/src/discovery/`, `core/src/extension/`, `core/src/ffi/async_extension.rs`

**Implementation Details**:
- **Claude Code compatible**: Follows Claude Code plugin manifest format (`.claude-plugin/plugin.json`)
- **Multi-level discovery**: Reads from `~/.claude/`, `~/.aether/`, `.claude/` (current project)
- **Plugin types**: Skills, commands, agents, MCP servers, hooks
- **Configuration**: `aether.jsonc` with multi-source merging
- **Async FFI**: Native async/await via UniFFI 0.31+ (Swift, Kotlin, Python)
- **Hot reload**: Plugins loaded and activated at runtime

**Architecture**:
```
discovery/              # Multi-level component discovery
├── scanner.rs         # Directory scanning (DiscoveryManager)
├── paths.rs           # Path utilities (aether_home, git_root)
└── types.rs           # DiscoveredComponent, DiscoverySource

extension/              # Extension system
├── loader.rs          # ComponentLoader (skills, commands, agents, plugins)
├── registry.rs        # ComponentRegistry (state management)
├── config/            # ConfigManager (aether.jsonc merging)
├── hooks/             # HookExecutor (PreToolUse, PostToolUse, Stop)
├── runtime/           # Node.js runtime (fnm, npm install)
└── sync_api.rs        # SyncExtensionManager (legacy sync wrapper)

ffi/
├── async_extension.rs # Async FFI exports (extension_load_all, etc.)
└── plugins.rs         # Sync FFI exports (legacy AetherCore methods)
```

**Key Types**:
- `ExtensionManager` - Main async entry point
- `SyncExtensionManager` - Sync wrapper for legacy FFI
- `ExtensionSkill`, `ExtensionCommand`, `ExtensionAgent`, `ExtensionPlugin`
- `HookExecutor` - Event-driven hook execution

**Swift Async API** (UniFFI 0.31+):
```swift
let summary = try await extensionLoadAll()
let plugins = try await extensionListPlugins()
let skills = try await extensionListSkills()
let result = try await extensionExecuteSkill(qualifiedName: "plugin:skill", arguments: "")
```

### Skills System

**Status**: ✅ Implemented (2026-01-08, Redesigned 2026-01-21)

**Implementation Details**:
- **Claude Agent Skills standard**: SKILL.md format with YAML frontmatter + Markdown body
- **Progressive Disclosure**: Three-level loading (Metadata → Instructions → Resources)
- **Hybrid Mode**: Slash commands pre-load, general chat uses read_skill tool
- **Installation methods**: GitHub URL, ZIP file upload, manual file placement
- **Hot reload**: Skills changes take effect immediately
- **Built-in skills**: refine-text, translate, summarize

**Architecture**:
- `ReadSkillTool` (`rig_tools/skill_reader.rs`) - Agent-initiated skill reading
- `ListSkillsTool` (`rig_tools/skill_reader.rs`) - List available skills
- `SkillsRegistry` for skill management and lookup
- `SkillsInstaller` for GitHub/ZIP installation
- Path traversal security checks

**Three-Level Loading**:
| Level | Timing | Token Cost | Content |
|-------|--------|------------|---------|
| Level 1: Metadata | Startup | ~100/skill | name + description (YAML frontmatter) |
| Level 2: Instructions | On-demand | <5k | SKILL.md body via `read_skill` |
| Level 3: Resources | On-demand | Unlimited | ADVANCED.md, REFERENCE.md, etc. |

**Data Flow**:
```
User: "help me refine this text"
  → Agent sees "Available Skills: refine-text..."
  → Agent calls read_skill(skill_id="refine-text")
  → Tool returns full SKILL.md content
  → Agent treats it as TASK DIRECTIVE (not reference)
  → Agent executes instructions strictly
```

**Usage**:
```
/refine-text Please improve this paragraph...  # Pre-load mode
"帮我润色这段文字"                              # Progressive disclosure mode
```

**Documentation**: See [SKILLS.md](./SKILLS.md)

---

### Smart Tool Discovery System

**Status**: ✅ Implemented (2026-01-21)

**Location**: `core/src/ffi/tool_discovery.rs`

**Purpose**: Intelligent tool filtering to reduce token consumption by only sending relevant tools to the LLM.

**Components**:
- `ToolCategory` enum: FileOps, Search, WebFetch, YouTube, ImageGen, VideoGen, AudioGen, SpeechGen
- `infer_required_tools()`: Analyze skill instructions + user request to determine needed tools
- `filter_tools_by_categories()`: Filter tool list based on inferred categories
- `get_builtin_tool_descriptions()`: Get tool descriptions with dynamic generation tools

**Two-Stage Tool Discovery**:

```
Stage 1: Intent Inference (Before Agent Loop)
┌─────────────────────────────────────────┐
│ infer_required_tools(instructions, req) │
│ → Keyword analysis for tool categories  │
│   • "文件" → FileOps                     │
│   • "搜索" → Search                      │
│   • "图像" → ImageGen                    │
└─────────────────────────────────────────┘
           ↓
Stage 2: Tool Index (Smart Discovery Mode)
┌─────────────────────────────────────────┐
│ System Prompt:                          │
│ ## Available Tools (full schema)        │
│   - file_ops, search, etc.              │
│ ## Additional Tools (index only)        │
│   - Use get_tool_schema(name) to get    │
│     full parameters before calling      │
└─────────────────────────────────────────┘
```

**Keyword Matching Rules**:
| Category | Keywords |
|----------|----------|
| FileOps | file, read, write, save, 文件, 保存, 目录 |
| Search | search, 搜索, 查找, look up |
| WebFetch | fetch, url, http, 网页, 链接 |
| YouTube | youtube, video, transcript, 视频 |
| ImageGen | image, picture, 图像, graph, 可视化 |
| VideoGen | generate video, 生成视频 |
| AudioGen | generate audio, 生成音乐 |
| SpeechGen | speech, tts, 语音 |

**Performance Benefits**:
- Token savings: >50% (via tool indexing instead of full schema)
- Response latency: <10ms (fast filtering)
- Supports 50+ tools without pressure

---

## FFI Layer Architecture

**Location**: `core/src/ffi/`

The FFI layer provides the bridge between Rust core and platform UI (Swift/Tauri).

### Key Modules

| Module | Purpose |
|--------|---------|
| `agent_loop_adapter.rs` | FfiLoopCallback for agent loop events and Part publishing |
| `tool_discovery.rs` | Smart tool filtering and category inference |
| `dag_executor.rs` | DAG task execution for generation tasks |
| `prompt_helpers.rs` | Prompt building utilities |
| `provider_factory.rs` | AI provider creation from config |
| `processing.rs` | Request processing pipeline |
| `config.rs` | FFI configuration handling |
| `mod.rs` | FFI types including Part event structures |

### Message Flow System

**Status**: ✅ Implemented (2026-01-24)

The Message Flow System enables Claude Code-style real-time UI updates for tool calls, streaming responses, and sub-agent progress.

#### Architecture

```
Agent Loop                          FFI Layer                    Platform UI
    │                                   │                            │
    ├─ on_action_start() ──────────────▶│ PartAdded event           │
    │                                   ├───────────────────────────▶│ Tool call starts
    │                                   │                            │
    ├─ on_thinking_stream() ───────────▶│ PartUpdated event (delta) │
    │                                   ├───────────────────────────▶│ Streaming text
    │                                   │                            │
    ├─ on_action_done() ───────────────▶│ PartUpdated event         │
    │                                   ├───────────────────────────▶│ Tool call completes
    │                                   │                            │
```

#### Part Event Types

**Location**: `core/src/components/types.rs`

```rust
pub enum PartEventType {
    Added,    // New part created
    Updated,  // Part state changed (status, delta, etc.)
    Removed,  // Part removed
}

pub struct PartUpdateData {
    pub session_id: String,
    pub part_id: String,
    pub part_type: String,    // "tool_call", "ai_response", etc.
    pub event_type: PartEventType,
    pub part_json: String,    // Full part state as JSON
    pub delta: Option<String>, // Incremental text for streaming
    pub timestamp: i64,
}
```

#### FFI Callback

**Location**: `core/src/ffi/mod.rs`

```rust
pub trait AetherEventHandler: Send + Sync {
    // ... existing callbacks ...

    /// Part update callback for real-time UI rendering
    fn on_part_update(&self, event: PartUpdateEventFfi);
}

pub struct PartUpdateEventFfi {
    pub session_id: String,
    pub part_id: String,
    pub part_type: String,
    pub event_type: PartEventTypeFfi,
    pub part_json: String,
    pub delta: Option<String>,
    pub timestamp: i64,
}
```

#### Event Publishing

Part events are published via EventBus and forwarded through CallbackBridge:

1. **FfiLoopCallback** (`ffi/agent_loop_adapter.rs`) tracks active tool calls and publishes:
   - `PartAdded` when tool call starts
   - `PartUpdated` when tool call completes or streaming text arrives

2. **CallbackBridge** (`components/callback_bridge.rs`) subscribes to Part events and converts them to FFI format

3. **Platform UI** receives `on_part_update` callback and updates the message flow display

#### Swift UI Integration

**Location**: `platforms/macos/Aether/Sources/`

| File | Purpose |
|------|---------|
| `MultiTurn/Models/PartModels.swift` | Swift models for Parts |
| `MultiTurn/Views/ToolCallPartView.swift` | Collapsible tool call UI |
| `EventHandler.swift` | `onPartUpdate` callback implementation |
| `MultiTurn/UnifiedConversationViewModel.swift` | Part state management |

**Part Status Flow**:
```
Pending → Running → Completed/Failed/Aborted
```

**UI Display Styles**:
- **Collapsed**: One-line summary (e.g., "search completed (150ms)")
- **Expanded**: Full input/output details with syntax highlighting

### tool_discovery.rs

Provides intelligent tool filtering:

```rust
pub fn infer_required_tools(
    skill_instructions: &str,
    user_request: &str,
) -> Vec<ToolCategory>

pub fn filter_tools_by_categories(
    all_tools: Vec<ToolDescription>,
    categories: &[ToolCategory],
) -> Vec<ToolDescription>

pub fn get_builtin_tool_descriptions(
    generation_config: &GenerationConfig,
) -> Vec<ToolDescription>
```

### prompt_helpers.rs

Utilities for prompt construction:

```rust
pub fn format_generation_models_for_prompt(config) -> Option<String>
pub fn build_history_summary_from_conversations(histories, topic_id) -> String
pub fn extract_attachment_text(attachments) -> Option<String>
pub fn response_needs_user_input(response) -> bool
```

### dag_executor.rs

Executes DAG tasks for media generation:

```rust
pub struct DagTaskExecutor {
    provider: Arc<dyn AiProvider>,
    generation_registry: Arc<RwLock<GenerationProviderRegistry>>,
}

// Task execution methods
execute_image_generation()  // → Actual generation API
execute_video_generation()
execute_audio_generation()
execute_llm_task()          // → LLM completion
```

---

## Thinker Layer

**Location**: `core/src/thinker/`

The Thinker layer is the LLM decision-making component of the Agent Loop.

### Components

| Component | File | Purpose |
|-----------|------|---------|
| PromptBuilder | `prompt_builder.rs` | Build system prompts with tools, runtimes, models |
| DecisionParser | `decision_parser.rs` | Parse LLM responses into actions |
| ToolFilter | `tool_filter.rs` | Filter tools based on observation context |

### PromptConfig

Configuration for prompt building:

```rust
pub struct PromptConfig {
    pub persona: Option<String>,
    pub language: Option<String>,
    pub custom_instructions: Option<String>,
    pub max_tool_description_tokens: usize,
    pub runtime_capabilities: Option<String>,   // Available runtimes
    pub generation_models: Option<String>,      // Available models
    pub tool_index: Option<String>,             // Smart discovery index
    pub skill_mode: bool,                       // Strict workflow mode
}
```

### System Prompt Structure

```
1. Role Definition
   "You are an AI assistant executing tasks step by step."

2. Available Runtimes (optional)
   Python, Node.js, FFmpeg, etc.

3. Available Tools
   - Full schema tools (immediate use)
   - Tool index (use get_tool_schema first)

4. Media Generation Models (optional)
   DALL-E 3, Midjourney, etc.

5. Special Actions
   complete, ask_user, fail

6. Response Format (JSON)
   { "reasoning": "...", "action": {...} }

7. Skill Execution Mode (optional)
   Strict workflow completion requirements
```

---

## GlobalBus - Cross-Agent Event System

**Status**: Implemented (2026-01-24)

**Location**: `core/src/event/global_bus.rs`, `core/src/event/filter.rs`

The GlobalBus provides a centralized event aggregation system for cross-agent communication. It enables parent agents to monitor sub-agent progress, facilitates system-wide event observation, and supports filtered subscriptions for efficient event routing.

### Architecture

```
+-----------------------------------------------------------------+
|                         GlobalBus                                |
+-----------------------------------------------------------------+
|                                                                  |
|   +-----------------------------------------------------------+ |
|   |                   Broadcast Channel                        | |
|   |   (tokio::broadcast with 1024 buffer)                     | |
|   +-----------------------------------------------------------+ |
|                              |                                   |
|        +---------------------+---------------------+            |
|        v                     v                     v            |
|   +---------+          +---------+          +---------+        |
|   |EventBus |          |EventBus |          |EventBus |        |
|   |Agent-1  |          |Agent-2  |          |Agent-3  |        |
|   +---------+          +---------+          +---------+        |
|        |                     |                     |            |
|   +----v---------------------v---------------------v--------+  |
|   |              Filtered Subscriptions                      |  |
|   |                                                          |  |
|   |  +------------------------------------------------------+|  |
|   |  | EventFilter                                          ||  |
|   |  |  - by session_id: ["session-1", "session-2"]        ||  |
|   |  |  - by agent_id: ["agent-1"]                          ||  |
|   |  |  - by event_type: [LoopStop, ToolCallCompleted]     ||  |
|   |  +------------------------------------------------------+|  |
|   +----------------------------------------------------------+  |
|                                                                  |
+-----------------------------------------------------------------+
```

### Key Components

#### GlobalBus

The singleton event aggregator that receives events from all EventBus instances:

```rust
use aether_core::event::{GlobalBus, EventFilter};

// Get the global singleton
let global_bus = GlobalBus::global();

// Subscribe to all events
let mut receiver = global_bus.subscribe_broadcast();

// Subscribe with filter (async callback)
let filter = EventFilter::new(vec![EventType::LoopStop])
    .with_session("session-1")
    .with_agent("agent-1");

let sub_id = global_bus.subscribe_async(filter, |event| {
    println!("Received: {:?}", event);
}).await;

// Unsubscribe
global_bus.unsubscribe(&sub_id).await;
```

#### EventFilter

Flexible filtering for subscription-based event routing:

```rust
// Filter by event type
let filter = EventFilter::new(vec![
    EventType::ToolCallStarted,
    EventType::ToolCallCompleted,
]);

// Filter by session
let filter = EventFilter::all()
    .with_session("session-1");

// Filter by agent
let filter = EventFilter::all()
    .with_agent("sub-agent-1");

// Combined filters (AND logic)
let filter = EventFilter::new(vec![EventType::LoopStop])
    .with_session("session-1")
    .with_agent("agent-1");
```

#### EventBus Integration

EventBus instances can optionally connect to GlobalBus for automatic event broadcasting:

```rust
use aether_core::event::{EventBus, GlobalBus};

let bus = EventBus::new()
    .with_agent_id("agent-1")
    .with_session_id("session-1")
    .with_global_bus(GlobalBus::global());

// All events published to this bus are automatically
// broadcast to GlobalBus with agent/session context
bus.publish(AetherEvent::LoopStop(StopReason::Completed)).await;
```

### GlobalEvent

Events in GlobalBus are wrapped with source context:

```rust
pub struct GlobalEvent {
    pub source_agent_id: String,
    pub source_session_id: String,
    pub event: AetherEvent,
    pub sequence: u64,
    pub timestamp: i64,
}
```

### Use Cases

1. **Parent Agent Monitoring Sub-Agent Progress**
   - Subscribe to sub-agent's session events
   - Wait for `LoopStop` event to know when complete
   - Collect tool call results from `ToolCallCompleted` events

2. **System-Wide Event Logging**
   - Subscribe with `EventFilter::all()` to capture all events
   - Log to file or monitoring system

3. **Cross-Agent Coordination**
   - Multiple agents can subscribe to each other's events
   - Enables reactive workflows based on agent completion

### FFI Interface

For Swift/Kotlin integration (via UniFFI):

```rust
// FFI-friendly subscription
pub async fn global_bus_subscribe(
    filter_json: String,
    callback_id: String,
) -> Result<String, AetherError>

// FFI-friendly unsubscribe
pub async fn global_bus_unsubscribe(
    subscription_id: String,
) -> Result<(), AetherError>
```

---

## References

- **Agent Loop**: [AGENT_LOOP.md](./AGENT_LOOP.md) - Doom loop detection, retry mechanism, tool repair, output truncation, smart compaction
- **Dispatcher**: [DISPATCHER.md](./DISPATCHER.md) - Tool routing and confirmation flow
- **OpenSpec Proposal**: `openspec/changes/implement-structured-context-protocol/`
- **Design Document**: `openspec/changes/implement-structured-context-protocol/design.md`
- **Spec Deltas**: `openspec/changes/implement-structured-context-protocol/specs/`
- **Original Design**: `agentstructure.md` (deprecated, see proposal)

---

**Last Updated**: 2026-01-24
**Implemented In**: Aether v0.1.0
**OpenSpec Changes**: `implement-structured-context-protocol`, `add-skills-capability`, `enhance-intent-routing-pipeline`, `smart-tool-discovery`, `event-bus-smart-compaction`
