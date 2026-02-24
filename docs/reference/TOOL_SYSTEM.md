# Tool System

> AlephTool trait, built-in tools, and tool development guide

---

## Overview

Aleph's tool system provides:
- Type-safe tool definitions with automatic schema generation
- Built-in tools for common operations
- MCP (Model Context Protocol) integration
- Extension tools via WASM/Node.js plugins

**Location**: `core/src/tools/`, `core/src/builtin_tools/`

---

## AlephTool Trait

### Static Dispatch (Compile-time)

```rust
pub trait AlephTool: Clone + Send + Sync + 'static {
    /// Tool name (used in LLM tool_use)
    const NAME: &'static str;

    /// Tool description for LLM
    const DESCRIPTION: &'static str;

    /// Argument type (auto JSON Schema via schemars)
    type Args: Serialize + DeserializeOwned + JsonSchema + Send;

    /// Return type
    type Output: Serialize + Send;

    /// Execute the tool
    async fn call(&self, args: Self::Args) -> Result<Self::Output>;

    /// JSON interface (auto-implemented)
    async fn call_json(&self, args: Value) -> Result<Value> {
        let typed_args: Self::Args = serde_json::from_value(args)?;
        let result = self.call(typed_args).await?;
        Ok(serde_json::to_value(result)?)
    }

    /// Get tool definition (auto-implemented)
    fn definition() -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: Self::DESCRIPTION.to_string(),
            input_schema: schema_for!(Self::Args),
        }
    }
}
```

### Dynamic Dispatch (Runtime)

```rust
pub trait AlephToolDyn: Send + Sync {
    fn name(&self) -> &str;
    fn definition(&self) -> ToolDefinition;
    fn call(&self, args: Value) -> BoxFuture<'_, Result<Value>>;
}

// Blanket impl: Any AlephTool is also AlephToolDyn
impl<T: AlephTool> AlephToolDyn for T { ... }
```

---

## Built-in Tools

**Location**: `core/src/builtin_tools/`

### File Operations

| Tool | Description | Args |
|------|-------------|------|
| `file_read` | Read file content | `path`, `encoding?` |
| `file_write` | Write file | `path`, `content` |
| `file_list` | List directory | `path`, `recursive?` |
| `file_delete` | Delete file/dir | `path` |
| `file_mkdir` | Create directory | `path` |
| `file_chmod` | Change permissions | `path`, `mode` |

### Code Execution

| Tool | Description | Args |
|------|-------------|------|
| `bash_exec` | Run bash command | `command`, `timeout?` |
| `code_exec` | Execute code snippet | `language`, `code` |
| `python_exec` | Run Python | `code`, `requirements?` |

### Web & Search

| Tool | Description | Args |
|------|-------------|------|
| `web_fetch` | Fetch URL content | `url`, `method?`, `headers?` |
| `web_search` | Search the web | `query`, `engine?` |
| `youtube_extract` | Extract video info | `url` |

### Generation

| Tool | Description | Args |
|------|-------------|------|
| `image_generate` | Generate image | `prompt`, `provider?`, `size?` |
| `pdf_generate` | Generate PDF | `content`, `template?` |

### Perception

| Tool | Description | Args |
|------|-------------|------|
| `snapshot_capture` | Capture AX tree + optional OCR | `target`, `region?`, `include_ax?`, `include_vision?`, `include_image?` |

### Session Tools

| Tool | Description | Args |
|------|-------------|------|
| `sessions_spawn` | Spawn sub-agent | `model?`, `thinking?`, `prompt` |
| `sessions_send` | Send to sub-agent | `session_key`, `message` |
| `sessions_list` | List sub-agents | - |

### Memory Tools

| Tool | Description | Args |
|------|-------------|------|
| `memory_store` | Store fact | `content`, `tags?` |
| `memory_search` | Search memory with hybrid retrieval | `query`, `max_results?` |
| `memory_forget` | Delete fact | `fact_id` |

#### memory_search Tool

**Purpose**: Search personal memory for relevant facts and conversation history with intelligent redundancy elimination.

**Features**:
- Hybrid retrieval: Searches both compressed facts and raw transcripts
- Post-retrieval arbitration: Eliminates redundancy between facts and transcripts
- Priority-based selection: Higher similarity scores selected first
- Token budget management: Fits results within context window
- Importance scoring: Can filter low-value content (when integrated with ValueEstimator)

**Arguments**:
```rust
pub struct MemorySearchArgs {
    /// Search query (natural language)
    pub query: String,
    /// Maximum results to return (default: 10)
    pub max_results: usize,
}
```

**Output**:
```rust
pub struct MemorySearchOutput {
    /// Compressed facts (deduplicated)
    pub facts: Vec<FactResult>,
    /// Raw conversation transcripts (deduplicated)
    pub transcripts: Vec<TranscriptResult>,
    /// Original query
    pub query: String,
    /// Tokens saved through deduplication
    pub tokens_saved: usize,
}
```

**Example Usage**:
```json
{
  "tool": "memory_search",
  "args": {
    "query": "What are my coding preferences?",
    "max_results": 10
  }
}
```

**Architecture**:
```
memory_search(query)
  → FactRetrieval.retrieve(query)
    → Hybrid search (facts + raw memories fallback)
  → ContextComptroller.arbitrate(results, budget)
    → Detect redundancy via cosine similarity (threshold: 0.95)
    → Remove redundant transcripts when facts exist
    → Sort by similarity score (descending)
    → Trim to fit token budget
  → Return deduplicated results
```

**Configuration**:
- Similarity threshold: 0.95 (configurable via ComptrollerConfig)
- Token estimation: 4 chars per token
- Retention mode: Hybrid (facts prioritized, redundant transcripts removed)
- Max facts: 10 (configurable via FactRetrievalConfig)
- Max raw fallback: 10 (configurable via FactRetrievalConfig)

### Meta Tools

| Tool | Description | Args |
|------|-------------|------|
| `skill_read` | Read skill definition | `skill_name` |
| `ask_user` | Ask user question | `question`, `options?` |
| `canvas_show` | Display in canvas | `content`, `type` |

---

## Tool Definition

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Schema,  // JSON Schema
}

// Sent to LLM as:
{
  "type": "function",
  "function": {
    "name": "file_read",
    "description": "Read content from a file",
    "parameters": {
      "type": "object",
      "properties": {
        "path": { "type": "string", "description": "File path" },
        "encoding": { "type": "string", "default": "utf-8" }
      },
      "required": ["path"]
    }
  }
}
```

---

## Tool Server

**Location**: `core/src/tools/server.rs`

The Tool Server manages tool execution:

```rust
pub struct ToolServer {
    builtin_tools: HashMap<String, Arc<dyn AlephToolDyn>>,
    mcp_clients: HashMap<String, McpClient>,
    extension_tools: HashMap<String, ExtensionTool>,
}

impl ToolServer {
    pub async fn execute(
        &self,
        tool_name: &str,
        args: Value,
    ) -> Result<ToolResult> {
        // 1. Check builtin tools
        if let Some(tool) = self.builtin_tools.get(tool_name) {
            return tool.call(args).await;
        }

        // 2. Check MCP tools
        if let Some((server, tool)) = self.find_mcp_tool(tool_name) {
            return self.mcp_clients[server].call(tool, args).await;
        }

        // 3. Check extension tools
        if let Some(ext_tool) = self.extension_tools.get(tool_name) {
            return ext_tool.call(args).await;
        }

        Err(Error::ToolNotFound(tool_name))
    }
}
```

---

## MCP Integration

**Location**: `core/src/mcp/`

Model Context Protocol for external tool servers:

```rust
pub struct McpClient {
    transport: Transport,  // Stdio, WebSocket, or HTTP
    tools: Vec<ToolDefinition>,
}

impl McpClient {
    pub async fn list_tools(&self) -> Result<Vec<ToolDefinition>>;
    pub async fn call_tool(&self, name: &str, args: Value) -> Result<Value>;
}
```

### MCP Configuration

```json5
{
  "mcp": {
    "servers": [
      {
        "name": "filesystem",
        "command": "npx",
        "args": ["-y", "@anthropic/mcp-server-filesystem"],
        "env": { "HOME": "/Users/user" }
      }
    ]
  }
}
```

---

## Tool Development Guide

### Step 1: Define Arguments

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MyToolArgs {
    /// Description shown to LLM
    pub required_field: String,

    /// Optional with default
    #[serde(default)]
    pub optional_field: Option<String>,
}
```

### Step 2: Implement Tool

```rust
use crate::tools::AlephTool;

#[derive(Clone)]
pub struct MyTool {
    // Any shared state
}

impl AlephTool for MyTool {
    const NAME: &'static str = "my_tool";
    const DESCRIPTION: &'static str = "Does something useful";

    type Args = MyToolArgs;
    type Output = String;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Implementation
        Ok(format!("Processed: {}", args.required_field))
    }
}
```

### Step 3: Register Tool

```rust
// In builtin_tools/mod.rs
pub fn register_builtins(server: &mut ToolServer) {
    server.register(MyTool::new());
}
```

---

## Tool Filtering

**Location**: `core/src/thinker/tool_filter.rs`

Control which tools are available:

```rust
pub struct ToolFilter {
    /// Whitelist (if set, only these tools available)
    pub allowed: Option<HashSet<String>>,

    /// Blacklist (always excluded)
    pub blocked: HashSet<String>,

    /// Require confirmation for these
    pub require_confirmation: HashSet<String>,
}
```

### Configuration

```json5
{
  "tools": {
    "allowed": ["file_read", "web_fetch", "memory_*"],
    "blocked": ["bash_exec"],
    "requireConfirmation": ["file_write", "file_delete"]
  }
}
```

---

## Tool Result

```rust
#[derive(Debug, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: Value,
    pub error: Option<String>,
    pub duration_ms: u64,
}
```

---

## Memory System Components (Phase 2)

### TranscriptIndexer

**Purpose**: Near-realtime indexing of conversation transcripts with chunking support.

**Features**:
- Sliding window chunking for long conversations
- Configurable chunk size and overlap
- Sentence-boundary aware splitting
- Token estimation (4 chars per token)

**Configuration**:
```rust
pub struct TranscriptIndexerConfig {
    pub max_tokens_per_chunk: usize,  // Default: 400
    pub overlap_tokens: usize,         // Default: 80
    pub enable_chunking: bool,         // Default: true
}
```

**Usage**:
```rust
let indexer = TranscriptIndexer::new(database, embedder);
let chunks = indexer.chunk_text(&long_conversation);
```

### ValueEstimator

**Purpose**: Importance scoring for memory entries to filter low-value content.

**Features**:
- Signal-based detection (8 signal types)
- Score range: 0.0 (low value) to 1.0 (high value)
- Length bonus for longer conversations
- Batch estimation support

**Signals**:
- **Positive**: UserPreference (+0.25), Decision (+0.20), PersonalInfo (+0.30), FactualInfo (+0.15)
- **Negative**: Greeting (-0.30), SmallTalk (-0.20)
- **Neutral**: Question, Answer (combined +0.10)

**Usage**:
```rust
let estimator = ValueEstimator::new();
let score = estimator.estimate(&memory_entry).await?;

if score > 0.7 {
    // High-value content, prioritize for compression
}
```

### ContextComptroller

**Purpose**: Post-retrieval arbitration to eliminate redundancy and manage token budget.

**Features**:
- Redundancy detection via cosine similarity (threshold: 0.95)
- Priority-based selection (similarity score descending)
- Token budget enforcement
- Three retention modes: PreferTranscript, PreferFact, Hybrid

**Configuration**:
```rust
pub struct ComptrollerConfig {
    pub similarity_threshold: f32,     // Default: 0.95
    pub retention_mode: RetentionMode, // Default: Hybrid
}
```

**Usage**:
```rust
let comptroller = ContextComptroller::new(config);
let budget = TokenBudget::new(10000);
let arbitrated = comptroller.arbitrate(retrieval_result, budget);

// arbitrated.facts: Deduplicated facts
// arbitrated.raw_memories: Deduplicated transcripts
// arbitrated.tokens_saved: Tokens saved through deduplication
```

### CompressionDaemon

**Purpose**: Background scheduler for periodic memory compression.

**Features**:
- Configurable check interval (default: 1 hour)
- Idle detection (default: 5 minutes idle required)
- Activity tracking
- Graceful start/stop
- Error handling and logging

**Configuration**:
```rust
pub struct CompressionDaemonConfig {
    pub check_interval_seconds: u64,   // Default: 3600 (1 hour)
    pub idle_threshold_seconds: u64,   // Default: 300 (5 minutes)
    pub enabled: bool,                  // Default: true
}
```

**Usage**:
```rust
let daemon = Arc::new(CompressionDaemon::new(config, || async {
    compression_service.compress().await
        .map_err(|e| e.to_string())
}));

// Start daemon
let handle = daemon.start();

// Record activity to reset idle timer
daemon.record_activity();

// Stop daemon
daemon.stop();
```

**Integration Example**:
```rust
// Create compression service
let compression_service = Arc::new(CompressionService::new(
    database.clone(),
    provider.clone(),
    embedder.clone(),
    CompressionConfig::default(),
));

// Create daemon with compression callback
let daemon_config = CompressionDaemonConfig::default();
let daemon = Arc::new(CompressionDaemon::new(daemon_config, {
    let service = compression_service.clone();
    move || {
        let service = service.clone();
        async move {
            service.compress().await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
    }
}));

// Start background compression
daemon.start();
```

---

## See Also

- [Architecture](ARCHITECTURE.md) - System overview
- [Agent System](AGENT_SYSTEM.md) - How tools are invoked
- [Extension System](EXTENSION_SYSTEM.md) - Plugin-based tools
- [Security](SECURITY.md) - Tool execution safety
- [Memory System](MEMORY_SYSTEM.md) - Memory architecture and design
