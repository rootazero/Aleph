# Rig-Core Migration Design

> **Date**: 2026-01-13
> **Status**: Approved
> **Author**: Claude + User collaborative design

## Overview

This document describes the architectural changes to replace Aleph's current 3-layer routing system with the `rig-core` library, leveraging its built-in Memory, Vector Store, and Tool Calling capabilities.

## Decision Summary

| Decision Point | Choice |
|----------------|--------|
| Routing System | Eliminate completely, rig Agent makes autonomous decisions |
| Memory/Vector Store | Use rig-sqlite, start fresh (no data migration) |
| Tool Calling | Migrate all to rig `#[tool]` macro |
| AI Provider | Use rig built-in providers entirely |
| UniFFI Interface | Simplified interface, hide rig internals |
| Configuration | Redesign based on rig concepts |
| Special Features | All implemented as rig Tools |
| Implementation | One-time rewrite |

## Architecture

### New Architecture Diagram

```
User Input
    ↓
┌─────────────────────────────────────────┐
│           AlephCore (UniFFI)           │
│  ┌───────────────────────────────────┐  │
│  │         RigAgentManager           │  │
│  │  ┌─────────────────────────────┐  │  │
│  │  │      rig::Agent<M>          │  │  │
│  │  │  ┌─────────┬─────────────┐  │  │  │
│  │  │  │ Tools   │ RAG Context │  │  │  │
│  │  │  │ (static)│ (dynamic)   │  │  │  │
│  │  │  └─────────┴─────────────┘  │  │  │
│  │  └─────────────────────────────┘  │  │
│  └───────────────────────────────────┘  │
│                    ↓                     │
│  ┌───────────────────────────────────┐  │
│  │    rig-sqlite (VectorStore)       │  │
│  └───────────────────────────────────┘  │
└─────────────────────────────────────────┘
    ↓
Swift UI (via simplified UniFFI interface)
```

### Component Changes

| Current Component | New Architecture |
|-------------------|------------------|
| L1/L2/L3 Router | Delete, Agent autonomous decisions |
| AiProvider trait | Delete, use rig providers |
| CapabilityExecutor | Delete, use rig tools |
| UnifiedToolExecutor | Delete, use rig tools |
| Memory module | Replace with rig-sqlite |
| PayloadBuilder | Delete, rig handles internally |

**Estimated code deletion**: ~15,000 lines (routing/, capability/, dispatcher/, payload/, etc.)

## Module Structure

### New `src/` Directory Structure

```
Aleph/core/src/
├── lib.rs                 # UniFFI export entry
├── aether.udl             # UniFFI interface (simplified)
├── error.rs               # Error types (keep)
│
├── agent/                 # New: Agent management
│   ├── mod.rs
│   ├── manager.rs         # RigAgentManager - core entry
│   └── config.rs          # Agent config parsing
│
├── tools/                 # New: rig Tool definitions
│   ├── mod.rs
│   ├── search.rs          # Search tool (Tavily, etc.)
│   ├── web_fetch.rs       # Web page fetching
│   ├── video.rs           # YouTube transcription
│   ├── file_ops.rs        # File operations
│   ├── screen.rs          # Screenshot/OCR
│   ├── memory.rs          # Memory query tool
│   └── pii.rs             # PII filtering tool
│
├── store/                 # New: Vector Store
│   ├── mod.rs
│   └── sqlite.rs          # rig-sqlite wrapper
│
├── config/                # Simplified: config parsing only
│   ├── mod.rs
│   └── types.rs
│
├── mcp/                   # Keep: MCP adapter
│   ├── mod.rs
│   └── adapter.rs         # MCP → rig Tool adapter
│
└── utils/                 # Keep: utilities
    ├── mod.rs
    └── pii.rs             # PII filtering logic
```

### Directories to Delete

```
Delete: routing/      (~3,000 lines)
Delete: dispatcher/   (~2,000 lines)
Delete: capability/   (~2,500 lines)
Delete: payload/      (~1,500 lines)
Delete: providers/    (~2,000 lines)
Delete: semantic/     (~1,500 lines)
Delete: memory/       (~3,000 lines) → replaced by store/
```

## Configuration

### New `config.toml` Structure

```toml
# ~/.aleph/config.toml

[agent]
provider = "openai"              # openai | anthropic | ollama | groq
model = "gpt-4o"
temperature = 0.7
max_tokens = 4096
system_prompt = """
You are Aleph, an intelligent assistant. You can use various tools to help users.
"""

[providers.openai]
api_key = "${OPENAI_API_KEY}"
base_url = "https://api.openai.com/v1"

[providers.anthropic]
api_key = "${ANTHROPIC_API_KEY}"

[providers.ollama]
base_url = "http://localhost:11434"
model = "llama3"

[memory]
enabled = true
db_path = "~/.aleph/memory.db"
embedding_model = "fastembed"
top_k = 5
similarity_threshold = 0.7

[tools]
enabled = ["search", "web_fetch", "video", "file_ops", "screen", "memory"]

[tools.search]
provider = "tavily"
api_key = "${TAVILY_API_KEY}"

[tools.search.fallback]
providers = ["searxng", "google"]

[mcp]
enabled = true

[[mcp.servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@anthropic/mcp-filesystem"]
```

### Removed Configuration

| Removed Config | Reason |
|----------------|--------|
| `[routing]` section | No longer need routing rules |
| `[dispatcher]` | No longer need multi-layer dispatch |
| `[[rules]]` array | Agent autonomous decisions |
| `capabilities` field | Replaced by rig tools |
| `confidence_threshold` | No longer need confidence |

## Tool Definitions

### Tool Implementation Pattern

```rust
// src/tools/search.rs
use rig::tool::Tool;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct SearchArgs {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize { 5 }

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Default)]
pub struct SearchTool {
    client: TavilyClient,
}

impl Tool for SearchTool {
    const NAME: &'static str = "search";
    const DESCRIPTION: &'static str =
        "Search the internet for latest information.";

    type Args = SearchArgs;
    type Output = Vec<SearchResult>;
    type Error = ToolError;

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let results = self.client.search(&args.query, args.limit).await?;
        Ok(results)
    }
}
```

### Tool Inventory

| Tool Name | Function | Parameters |
|-----------|----------|------------|
| `search` | Web search | query, limit |
| `web_fetch` | Fetch web page content | url |
| `video_transcript` | YouTube subtitles | video_url |
| `file_read` | Read file | path |
| `file_write` | Write file | path, content |
| `file_list` | List directory | path, pattern |
| `screen_capture` | Screenshot | region? |
| `memory_search` | Search memories | query, limit |
| `pii_filter` | Filter sensitive info | text |

### MCP Tool Adapter

```rust
// src/mcp/adapter.rs
pub struct McpToolAdapter {
    client: McpClient,
    tool_name: String,
    description: String,
    schema: serde_json::Value,
}

impl Tool for McpToolAdapter {
    // Dynamic implementation, forwards to MCP client
}
```

## Memory / Vector Store

### rig-sqlite Implementation

```rust
// src/store/sqlite.rs
use rig_sqlite::{SqliteVectorStore, SqliteVectorStoreTable};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub user_input: String,
    pub assistant_response: String,
    pub timestamp: i64,
    pub app_context: Option<String>,
}

impl SqliteVectorStoreTable for MemoryEntry {
    fn name() -> &'static str { "memories" }
    fn id(&self) -> String { self.id.clone() }
    fn text(&self) -> String {
        format!("User: {}\nAssistant: {}", self.user_input, self.assistant_response)
    }
}

pub struct MemoryStore {
    store: SqliteVectorStore<MemoryEntry>,
    embedding_model: fastembed::TextEmbedding,
}

impl MemoryStore {
    pub async fn new(db_path: &str) -> Result<Self> {
        let store = SqliteVectorStore::new(db_path).await?;
        let embedding_model = fastembed::TextEmbedding::try_new(
            fastembed::InitOptions::new(fastembed::EmbeddingModel::BGESmallZHV15)
        )?;
        Ok(Self { store, embedding_model })
    }

    pub async fn store(&self, entry: MemoryEntry) -> Result<()> {
        let embedding = self.embedding_model.embed(vec![entry.text()])?;
        self.store.insert(entry, embedding[0].clone()).await
    }

    pub async fn search(&self, query: &str, top_k: usize) -> Result<Vec<MemoryEntry>> {
        let query_embedding = self.embedding_model.embed(vec![query])?;
        self.store.search(&query_embedding[0], top_k).await
    }
}
```

### Agent Integration (RAG)

```rust
// src/agent/manager.rs
impl RigAgentManager {
    pub fn build_agent(&self) -> Agent<impl CompletionModel> {
        let client = openai::Client::from_env();

        client.agent("gpt-4o")
            .preamble(&self.config.system_prompt)
            .tool(SearchTool::new())
            .tool(WebFetchTool::new())
            .tool(VideoTool::new())
            .dynamic_context(self.memory_store.as_index())
            .build()
    }
}
```

## UniFFI Interface

### Simplified `aether.udl`

```webidl
namespace aether {
    [Throws=AlephError]
    AlephCore init(string config_path, AlephEventHandler handler);
};

[Error]
enum AlephError {
    "Config",
    "Provider",
    "Tool",
    "Memory",
    "Cancelled",
};

callback interface AlephEventHandler {
    void on_thinking();
    void on_tool_start(string tool_name);
    void on_tool_result(string tool_name, string result);
    void on_stream_chunk(string text);
    void on_complete(string response);
    void on_error(string message);
    void on_memory_stored();
};

interface AlephCore {
    [Throws=AlephError]
    void process(string input, ProcessOptions? options);

    void cancel();

    sequence<ToolInfo> list_tools();

    [Throws=AlephError]
    sequence<MemoryItem> search_memory(string query, u32 limit);

    [Throws=AlephError]
    void clear_memory();

    [Throws=AlephError]
    void reload_config();
};

dictionary ProcessOptions {
    string? app_context;
    string? window_title;
    boolean stream;
};

dictionary ToolInfo {
    string name;
    string description;
    string source;
};

dictionary MemoryItem {
    string id;
    string user_input;
    string assistant_response;
    i64 timestamp;
    string? app_context;
};
```

### Removed Interfaces

| Removed Interface | Reason |
|-------------------|--------|
| `RoutingDecision` | No longer need routing decisions |
| `AgentPayload` | rig handles internally |
| `Capability` enum | Replaced by tools |
| `Intent` enum | Agent autonomous decisions |
| `ConfirmationRequest` | Simplified flow, no confirmation |
| `listToolsBySource()` | Simplified to single `list_tools()` |

## Implementation Plan

### Phases

```
Phase 1: Infrastructure
    ├── Add rig dependencies to Cargo.toml
    ├── Create new module structure (agent/, tools/, store/)
    └── Implement new config parsing

Phase 2: Core Functionality
    ├── Implement MemoryStore (rig-sqlite)
    ├── Implement RigAgentManager
    └── Integrate rig providers (OpenAI/Anthropic/Ollama)

Phase 3: Tool Migration
    ├── Implement SearchTool
    ├── Implement WebFetchTool
    ├── Implement VideoTool
    ├── Implement FileOpsTool
    ├── Implement ScreenTool
    └── Implement McpToolAdapter

Phase 4: UniFFI Integration
    ├── Update aether.udl
    ├── Implement new AlephCore
    └── Generate Swift bindings

Phase 5: Cleanup
    ├── Delete old module code
    ├── Update documentation
    └── Test verification
```

### Cargo.toml Changes

```toml
[dependencies]
# Add
rig-core = "0.6"
rig-sqlite = "0.1"

# Keep
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uniffi = "0.28"
tracing = "0.1"
reqwest = { version = "0.12", features = ["json"] }
fastembed = "4"

# Remove
# rusqlite (replaced by rig-sqlite)
# sqlite-vec (replaced by rig-sqlite)
# regex (no longer need L1 routing)
```

### File Change List

| Action | File | Description |
|--------|------|-------------|
| Create | `agent/manager.rs` | RigAgentManager implementation |
| Create | `agent/config.rs` | Agent configuration |
| Create | `store/sqlite.rs` | MemoryStore implementation |
| Create | `tools/*.rs` | Tool implementations (~8 files) |
| Rewrite | `lib.rs` | UniFFI exports |
| Rewrite | `aether.udl` | Simplified interface |
| Rewrite | `config/mod.rs` | New config parsing |
| Delete | `routing/*` | Entire directory |
| Delete | `dispatcher/*` | Entire directory |
| Delete | `capability/*` | Entire directory |
| Delete | `payload/*` | Entire directory |
| Delete | `providers/*` | Entire directory |
| Delete | `semantic/*` | Entire directory |
| Delete | `memory/*` | Entire directory |

### Testing Strategy

```bash
# After each phase
cargo test                    # Unit tests
cargo build                   # Compile check
cargo run --example basic     # Basic functionality verification
```

## References

- [rig-core documentation](https://docs.rig.rs/)
- [rig-core crates.io](https://crates.io/crates/rig-core)
- [rig GitHub repository](https://github.com/0xPlaygrounds/rig)
- Current architecture: [docs/ARCHITECTURE.md](../ARCHITECTURE.md)
- Current dispatcher: [docs/DISPATCHER.md](../DISPATCHER.md)
