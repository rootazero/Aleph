# Tool System

> AlephTool trait, built-in tools, and tool development guide

---

## ToolService — production stack

Harness consumers depend on `Arc<dyn ToolService>` exclusively:

```rust
pub trait ToolService: Send + Sync + 'static {
    async fn execute(&self, name: &str, input: Value) -> Result<ToolOutput, ToolError>;
    async fn list(&self) -> Vec<ToolDefinition>;
    async fn describe(&self, name: &str) -> Option<ToolDefinition>;
    fn dispatcher_schema(&self) -> Arc<[crate::dispatcher::ToolDefinition]>;
}
```

Production impl: **`ScopedToolService`** (`src/tools/scoped.rs`). The Gateway
builds one per request via
`gateway::execution_engine::tool_service_builder::build_request_tool_service`
and supplies it as `FlowRequest::tool_service` so each turn sees an
allow-listed view over the shared `LoopToolRegistry`.

`ScopedToolService` carries the HITL seams natively:
- `with_confirmation(confirm_tools, requester)` — gates `requires_confirmation`
  tools through `ApprovalRequester` (wired at boot to
  `ChannelApprovalBridgeAdapter`).
- `with_turn_context(TurnContext)` — scopes the `TURN_CONTEXT` task-local for
  every tool call so HITL tools (`ask_user`, sandbox escalations, channel
  approval) can route back to the originating channel.

Tool authors implement `AlephTool` (typed) or `LoopTool` (untyped via
`RegistryToolAdapter`). The harness fallback `AgentHarnessRunner.tool_service`
is `NullToolService` (`src/tools/null.rs`) — production never reaches it
because Gateway always supplies the per-request override; a `NotFound` from
that service signals upstream wiring regression.

The pre-`ScopedToolService` Phase 2 decorator chain (`facade.rs` /
`dispatch.rs` / `registry.rs` / `middleware/` / `handlers/`, ~2700 lines) was
deleted in 2026-05-20; it was unreachable because every gateway request
overrode it. See
`docs/superpowers/specs/2026-05-19-hitl-loop-closure-design.md` §8.

---

## Overview

Aleph's tool system provides:
- Type-safe tool definitions with automatic schema generation
- Built-in tools for common operations
- MCP (Model Context Protocol) integration
- Extension tools via WASM/Node.js plugins

**Location**: `src/tools/`, `src/builtin_tools/`

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

**Location**: `src/builtin_tools/`

### File Operations

| Tool | Description | Args |
|------|-------------|------|
| `file_read` | Read file content (windowed: line limit **and** token budget, whichever binds; message reports a resumable `offset`) | `path`, `offset?`, `limit?` |
| `file_write` | Write file | `path`, `content` |
| `file_list` | List directory | `path`, `recursive?` |
| `file_delete` | Delete file/dir | `path` |
| `file_mkdir` | Create directory | `path` |
| `file_chmod` | Change permissions | `path`, `mode` |

### Tree Search

**Location**: `src/builtin_tools/file_search/` — `walk.rs` (the one answer to
"which files does this repository consider its own", plus the denylist floor a
byte-reading face must bind), `scan.rs` (pure line matching), `grep.rs`,
`find.rs`.

| Tool | Description | Args |
|------|-------------|------|
| `grep` | Content search across a tree. `.gitignore`-aware, skips `.git` and binaries, capped and pageable. Match lines are 240-char **locators** — follow one with `file_read{offset,limit}` | `pattern`, `path?`, `glob?`, `ignore_case?`, `literal?`, `context?`, `limit?`, `offset?`, `files_only?`, `no_ignore?` |
| `find` | File discovery by glob. Same walk, paths only, sorted and pageable | `pattern`, `path?`, `limit?`, `offset?`, `no_ignore?` |

Three things about this pair are load-bearing:

- **`pattern` is a regex, so several terms are ONE call** (`foo|bar|baz`).
  There is deliberately no `multi_grep`; a second verb would buy per-pattern
  grouping at the price of a second registration surface, ~700 B of description
  billed on every request, and one action answering to two names.
- **They replace `bash`.** A `grep -r` does not read `.gitignore`, so one
  recursive run pours every hit under `node_modules/`, `target/` and `dist/`
  into the context window. `bash`'s own DESCRIPTION says so, and
  `tools/scoped/search_steer.rs` repeats it at call time as a non-blocking
  `<system-reminder>` when a shell command duplicates one of them. `rg` and
  `fd` are never steered — they are the sanctioned shell fallback.
- **`file_ops{operation:"search"}` is a different face, not a duplicate.** That
  one is file *management* (returns size/type/extension, feeds
  `organize`/`batch_move`/`stats` over any directory); `find` is code
  *navigation*. What they must not fork on — "which files exist" — is answered
  once, by `walk`.

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
  → Hybrid search (see [Retrieval](../memory/RETRIEVAL.md))
    → Facts + raw memories fallback
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

#### Retrieval tools — three stores, no overlap

Aleph exposes three complementary BM25/hybrid retrieval tools. Each searches a
**different store**, so their descriptions stay sharp to avoid tool-choice confusion:

| Tool | Store | Use when |
|------|-------|----------|
| `ctx_search` | Offloaded tool **output** (FTS5 content index) | A tool result shows `[Full output persisted: … Indexed N sections …]` and you need only the relevant slice instead of re-reading the whole build log / big grep / web fetch. |
| `recall_events` | **This session's** event timeline (`session_events`, BM25) | An earlier tool action, result, or error in *this* session was dropped from context by compaction and you need to recover what already happened. |
| `session_search` | **Past conversations across sessions** (memory summaries + transcripts) | You need facts or decisions from *other* sessions (long-term memory), not the current run. |

Rule of thumb: `ctx_search` = "what did that tool print?", `recall_events` =
"what did I already do this session?", `session_search` = "what happened in past sessions?".

### Meta Tools

| Tool | Description | Args |
|------|-------------|------|
| `skill_read` | Read skill definition | `skill_name` |
| `ask_user` | Ask 1–4 questions, park until answered | `question` + `choices?`, **or** `questions[]` (`id?`/`header?`/`question`/`choices?`/`multi_select?`/`secret?`) |
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

**Location**: `src/tools/server.rs`

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

**Location**: `src/mcp/`

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

Registration is **not one place**, and the gap between "the model is told about
this tool" and "a call reaches it" has shipped as a bug four times
(`select_model`, `doctor`, `config_audit`, `plugin_manage` — the last one found
by a real-machine fixture, not by the 16k-test suite, because every in-process
test asked a *registration* surface whether the tool existed and every one of
them correctly said yes).

The sites, in the order a new tool needs them:

| # | File | What it buys |
|---|------|--------------|
| 1 | `executor/builtin_registry/definitions.rs` — `BUILTIN_TOOL_DEFINITIONS` | catalog row; the description starts being billed on every request |
| 2 | `definitions.rs` — `create_tool_boxed` | construction for `AlephToolServer` |
| 3 | `registry/tool_registry_impl.rs` — `execute_tool` match arm | **dispatch**; without it every call answers `Unknown tool` |
| 4 | `registry/struct_def.rs` | the instance field |
| 5 | `builder/constructor/mod.rs` | construction + struct init (pass the shared `ToolContext` handle if the tool resolves paths) |
| 6 | `builder/core_tools.rs` — `reg(...)` | registry-map row (`agent_init` completes the model's list from here) |
| 7 | `builtin_registry/groups.rs` | Panel display category |
| 8 | `config/types/tools.rs` — `default_core_tools()` | schema-resident vs collapsed behind a `get_tool_schema` round-trip |
| 9 | `tools/adapters/registry_adapter.rs` — `READ_ONLY_TOOLS` | read-only ⇒ idempotent ⇒ auto-retry ⇒ callable under the `Ask` / `Plan` tiers |
| 10 | `tools/fallback_registry.rs` — `ToolFamily::from_name` | which alternatives are suggested when it fails |

1–3 and 6 are enforced: `builtin_registry/dispatchable.rs` recovers both the
advertised set and the dispatchable set **from the source text** (not from a
list someone maintains) and fails naming the tool that is in one and not the
other. 4–5 are compile errors. 7–10 are silent if missed — a Panel row that
renders as a generic gear, a tool that costs a round-trip to call, a pure read
that the `Ask` tier stops to ask about.

---

## Tool Filtering

**Location**: `src/thinker/tool_filter.rs`

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

### Ingress: 结果在进入上下文之前要经过什么

唯一入口 `src/tool_output/ingress.rs::clean_for_ingress`，由
`tools/scoped/dispatch.rs::apply_layer_two` 调用一次：

```text
value (serde_json::Value —— 文本字段仍带真换行)
  │
  ├─ hoist_inline_images        图片 → 带外 vision 通道（含 MCP content 块）
  ├─ harvest_outbound_media     _media → 产物库
  ├─ 1. 每工具压缩              无条件，字段级
  ├─ 2. 内容类型清洗            仅在已超预算时，字段级
  └─ 3. 扁平化 → apply_result_budget（persist 原文 / 内联信号 / 截断）
```

**给写新工具或新 adapter 的人的四条硬约束**（每一条都对应一次真实的静默失效）：

1. **不要在交给 dispatcher 之前把自己的结果 `to_string`。** `Value::to_string()`
   转义每一个 `\n` 并把整个结果压成一行，而这条链上的**每一个**清洗器都按行路由——
   log / search / diff / json 四个缩减器、错误蒸馏器、以及 `compressor.rs` 的三个
   DevTools 策略。MCP adapter 曾这么做，结果那四条路全是死的，而且蒸馏器还会拿 JSON
   信封的前 400 字符冒充"关键错误"替换整个结果。
2. **图片要留在结构里、留在 `hoist_inline_images` 认得的位置。**
   支持 `{image_base64, format}`（顶层或 `data` 下）与 MCP 的
   `content[].{type:"image", data, mimeType}`。字符串里的 base64 找不回来，只会被
   当文本计费然后截断成解不开的片段。
3. **要重写文本字段就走 `tool_output::fence::rewrite_interior`**，别整体替换——
   不可信内容围栏是结构不是内容，见 [SECURITY.md](SECURITY.md#content-sanitization)。
4. **任何会丢内容的 stage 都欠调用方一份原文。** `clean_for_ingress` 的
   `full_original` 是 `apply_result_budget` 落盘的那一份，它决定被丢掉的行还能不能
   靠 `ctx_search` / `read_file` 挖回来。压缩器一度不设它，理由是"压缩产物按构造小于
   预算所以落盘走不到"——真实 `take_snapshot` 压缩后 13 585 token / 8 000 预算，当场
   走到，落盘的是压缩正文，被丢掉的 443 个节点就此不可恢复。**只有"从不删内容"的
   stage 才可以传 `None`**（剥 ANSI 转义是唯一的例子）。

**真机 QA（2026-08-04）**：隔离 `ALEPH_HOME` + 真实 `chrome-devtools-mcp` + 记录请求体的
mock LLM，实测确认 MCP 截图作为可解码 PNG 到达模型、三个工具结果围栏开闭配对、快照压缩
保住全部 660 个控件、落盘的是未压缩原文。**未覆盖**：`metadata.images` 只有 Anthropic
协议消费，OpenAI 系（`openai_chat` / `responses`）的 ToolResult 臂丢弃图片（API 约束，见
FEATURE_LOCATOR §3.14「已定位·未做」）。

详见 [FEATURE_LOCATOR §3.14](FEATURE_LOCATOR.md)。

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
let indexer = TranscriptIndexer::new(database);
let chunks = indexer.chunk_text(&long_conversation);
```

No embedder: the indexer writes plain `raw_memories` rows; recall over them
is the substring transcripts leg of `memory_search`.

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
