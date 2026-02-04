# Session Compaction

This document describes Aleph's session compaction system, which manages token usage while preserving conversation context.

## Overview

Session compaction is the mechanism for controlling token usage while preserving essential conversation context. It implements a three-layer approach matching OpenCode's architecture:

1. **Smart Pruning**: Remove old tool outputs while protecting recent and important ones
2. **LLM Summarization**: Generate intelligent summaries of conversation history
3. **Boundary Detection**: Mark compaction points for history filtering

```
┌─────────────────────────────────────────────────────────────────┐
│                    Session Compaction Flow                       │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   Token Overflow Detected                                        │
│         │                                                        │
│         ▼                                                        │
│   ┌───────────────────┐                                         │
│   │  1. Smart Prune   │ ─── Remove old tool outputs             │
│   │                   │     (respect protection rules)          │
│   └─────────┬─────────┘                                         │
│             │                                                    │
│             ▼                                                    │
│   ┌───────────────────┐                                         │
│   │  2. Summarize     │ ─── LLM or template-based summary       │
│   │                   │     (preserve key context)              │
│   └─────────┬─────────┘                                         │
│             │                                                    │
│             ▼                                                    │
│   ┌───────────────────┐                                         │
│   │  3. Replace       │ ─── Insert CompactionMarker + Summary   │
│   │                   │     (create filter boundary)            │
│   └─────────┬─────────┘                                         │
│             │                                                    │
│             ▼                                                    │
│   SessionCompacted event published                               │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Configuration

### CompactionConfig

```rust
/// Configuration for session compaction behavior
pub struct CompactionConfig {
    /// Enable automatic compaction when overflow detected
    pub auto_compact: bool,
    /// Enable pruning of old tool outputs
    pub prune_enabled: bool,
    /// Minimum tokens to save before pruning (default: 20,000)
    pub prune_minimum: u64,
    /// Protect this many tokens of recent tool outputs (default: 40,000)
    pub prune_protect: u64,
    /// Tools that should never have their outputs pruned
    pub protected_tools: Vec<String>,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            auto_compact: true,
            prune_enabled: true,
            prune_minimum: 20_000,
            prune_protect: 40_000,
            protected_tools: vec!["skill".to_string()],
        }
    }
}
```

### TOML Configuration (Planned)

```toml
[compaction]
auto_compact = true          # Enable automatic compaction
prune_enabled = true         # Enable tool output pruning
prune_minimum = 20000        # Minimum tokens before pruning
prune_protect = 40000        # Protect recent N tokens
protected_tools = ["skill"]  # Tools that are never pruned
```

---

## Key Components

### SessionCompactor (`components/session_compactor.rs`)

Main compaction logic handler:

```rust
pub struct SessionCompactor {
    /// Token tracker for managing limits
    token_tracker: TokenTracker,
    /// Number of recent tool calls to keep with full output
    keep_recent_tools: usize,
    /// Compaction configuration
    config: CompactionConfig,
}
```

**Key methods**:

| Method | Description |
|--------|-------------|
| `prune_with_thresholds()` | Smart pruning with protection mechanisms |
| `generate_llm_summary()` | LLM-driven context summarization |
| `filter_compacted()` | History boundary detection |
| `compact()` | Full compaction pipeline |
| `check_and_compact()` | Conditional compaction check |

### EnhancedTokenUsage

Cache-aware token tracking matching OpenCode's approach:

```rust
pub struct EnhancedTokenUsage {
    /// Input tokens consumed
    pub input: u64,
    /// Output tokens generated
    pub output: u64,
    /// Reasoning tokens (for models that support it)
    pub reasoning: u64,
    /// Tokens read from cache (reduces cost)
    pub cache_read: u64,
    /// Tokens written to cache
    pub cache_write: u64,
}

impl EnhancedTokenUsage {
    /// Calculate total tokens for overflow detection
    /// OpenCode formula: input + cache.read + output
    pub fn total_for_overflow(&self) -> u64 {
        self.input + self.cache_read + self.output
    }

    /// Calculate billable tokens (cache reads are cheaper)
    pub fn total_billable(&self) -> u64 {
        self.input + self.output + self.reasoning
    }
}
```

### TokenTracker

Model-specific limit management:

```rust
pub struct TokenTracker {
    model_limits: HashMap<String, ModelLimit>,
}
```

**Preset model limits**:

| Model | Context Limit | Compaction Threshold (80%) |
|-------|---------------|---------------------------|
| Claude 3 Opus | 200K | 160K |
| Claude 3.5 Sonnet | 200K | 160K |
| GPT-4 Turbo | 128K | 102.4K |
| GPT-4o | 128K | 102.4K |
| Gemini 1.5 Pro | 1M | 800K |

### CompactionMarker

Marks compaction boundaries in session:

```rust
/// Marker for compaction boundary
pub struct CompactionMarker {
    /// When compaction occurred
    pub timestamp: i64,
    /// Whether this was automatic or user-triggered
    pub auto: bool,
}
```

---

## Compaction Flow

### 1. Overflow Detection

Before each iteration, check if tokens exceed threshold:

```rust
// Check if we need compaction based on token count
let limit = token_tracker.get_model_limit(&session.model);
if session.total_tokens >= limit.compaction_threshold() {
    // Trigger compaction
}
```

### 2. Smart Pruning

The `prune_with_thresholds()` method implements OpenCode-style pruning:

**Algorithm**:
1. Iterate backward through parts from most recent
2. Skip until reaching 2+ user turns (to preserve recent context)
3. Find completed tool calls (excluding protected tools)
4. Accumulate tool outputs until exceeding `prune_protect` threshold
5. If total accumulated tokens > `prune_minimum`, mark outputs as pruned

```rust
// Example: Tool output after pruning
"[Output pruned to save context - compacted at 1706140800]"
```

**Protection rules**:
- **Protected tools**: Never prune outputs from tools in `protected_tools` list (e.g., "skill")
- **Recent context**: Always protect tool outputs within the last 2 user turns
- **Token threshold**: Only prune if savings exceed `prune_minimum` (default 20K tokens)

### 3. LLM Summarization

Generate context-aware summary using LLM:

```rust
const COMPACTION_PROMPT: &str = r#"You are a helpful AI assistant tasked with summarizing conversations.

Provide a detailed prompt for continuing our conversation above. Focus on information that would be helpful for continuing the conversation:
- What was done
- What is currently being worked on
- Which files are being modified
- What needs to be done next
- Key user requests, constraints, or preferences
- Important technical decisions and why they were made

Write in a way that allows a new session to continue seamlessly without access to the full conversation history."#;
```

If no LLM is available, falls back to template-based summary.

### 4. Boundary Creation

Insert `CompactionMarker` before replacing parts with summary:

```rust
pub fn insert_compaction_marker(&self, session: &mut ExecutionSession, auto: bool) {
    let marker = CompactionMarker {
        timestamp: chrono::Utc::now().timestamp(),
        auto,
    };
    session.parts.push(SessionPart::CompactionMarker(marker));
}
```

---

## History Filtering with filter_compacted()

The `filter_compacted()` method creates natural breakpoints at summary points:

```rust
/// Filter session parts to only include those after the last compaction boundary
pub fn filter_compacted(&self, session: &ExecutionSession) -> Vec<SessionPart> {
    let mut result: Vec<SessionPart> = Vec::new();
    let mut found_completed_summary = false;

    // Iterate backward to find the boundary
    for part in session.parts.iter().rev() {
        match part {
            SessionPart::Summary(s) if s.compacted_at > 0 => {
                // Found a completed summary
                found_completed_summary = true;
                result.push(part.clone());
            }
            SessionPart::CompactionMarker(_) if found_completed_summary => {
                // Found the compaction marker - this is the boundary
                break;
            }
            _ => {
                result.push(part.clone());
            }
        }
    }

    result.reverse();
    result
}
```

**Example**:

```
Before filter_compacted():
┌─────────────────────────────────────────────────┐
│ [Old User Input]                                │
│ [Old Tool Call: output pruned]                  │
│ [CompactionMarker: timestamp=1000]              │  ← Boundary
│ [Summary: "Previous context..."]                │
│ [New User Input]                                │
│ [New Tool Call: full output]                    │
└─────────────────────────────────────────────────┘

After filter_compacted():
┌─────────────────────────────────────────────────┐
│ [Summary: "Previous context..."]                │
│ [New User Input]                                │
│ [New Tool Call: full output]                    │
└─────────────────────────────────────────────────┘
```

---

## Events

### SessionCompacted Event

Published when compaction occurs:

```rust
pub struct CompactionInfo {
    pub session_id: String,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub timestamp: i64,
}
```

**Event flow**:

```
SessionCompactor::check_and_compact()
    │
    ├─ Compaction performed
    │
    └─ Publish AlephEvent::SessionCompacted(CompactionInfo)
           │
           ▼
    CallbackBridge handles event
           │
           └─ Notify UI of compaction
```

---

## Integration with Agent Loop

The SessionCompactor integrates with the agent loop as an EventHandler:

```rust
#[async_trait]
impl EventHandler for SessionCompactor {
    fn name(&self) -> &'static str {
        "SessionCompactor"
    }

    fn subscriptions(&self) -> Vec<EventType> {
        vec![EventType::ToolCallCompleted, EventType::LoopContinue]
    }

    async fn handle(
        &self,
        event: &AetherEvent,
        _ctx: &EventContext,
    ) -> Result<Vec<AetherEvent>, HandlerError> {
        match event {
            AlephEvent::LoopContinue(loop_state) => {
                // Check if we need compaction based on token count
                let limit = self.token_tracker.get_model_limit(&loop_state.model);
                if loop_state.total_tokens >= limit.compaction_threshold() {
                    // Trigger compaction check
                }
                Ok(vec![])
            }
            AlephEvent::ToolCallCompleted(_) => {
                // Trigger pruning check after tool completion
                Ok(vec![])
            }
            _ => Ok(vec![]),
        }
    }
}
```

**Integration points**:

| Event | Trigger Point | Action |
|-------|---------------|--------|
| `LoopContinue` | Before each iteration | Check overflow, trigger compaction |
| `ToolCallCompleted` | After tool execution | Trigger pruning check |
| Session end | At loop completion | Final pruning pass |

---

## Usage Examples

### Basic Compaction

```rust
let compactor = SessionCompactor::new();
let mut session = ExecutionSession::new().with_model("claude-3-opus");

// Add session parts...
session.parts.push(SessionPart::UserInput(...));
session.parts.push(SessionPart::ToolCall(...));

// Check and compact if needed
if let Some(info) = compactor.check_and_compact(&mut session).await {
    println!("Compacted: {} -> {} tokens", info.tokens_before, info.tokens_after);
}
```

### Custom Configuration

```rust
let config = CompactionConfig {
    auto_compact: true,
    prune_enabled: true,
    prune_minimum: 10_000,  // Lower threshold
    prune_protect: 30_000,
    protected_tools: vec!["skill".to_string(), "read_file".to_string()],
};

let compactor = SessionCompactor::with_config(config);
```

### LLM-Driven Summary

```rust
// Create LLM callback
let llm_callback: LlmCallback = Box::new(|system_prompt, user_content| {
    Box::pin(async move {
        // Call your LLM provider here
        let response = your_llm_client.complete(system_prompt, user_content).await?;
        Ok(response)
    })
});

// Generate summary
let summary = compactor.generate_llm_summary(&session, Some(&llm_callback)).await;
```

### Filtered Session for LLM

```rust
// Get session with only parts after last compaction boundary
let filtered = compactor.get_filtered_session(&session);

// Use filtered session for LLM context
let messages = build_messages_from_session(&filtered);
```

---

## Testing

```bash
# Run all compaction tests
cd core && cargo test session_compactor

# Run specific test categories
cargo test test_compaction_config           # Config tests
cargo test test_enhanced_token_usage        # Token tracking tests
cargo test test_prune_with_thresholds       # Pruning tests
cargo test test_filter_compacted            # Boundary detection tests
cargo test test_generate_llm_summary        # Summary tests
```

---

## References

- [OpenCode](https://github.com/opencode-ai/opencode) - Inspiration for compaction architecture
- [AGENT_LOOP.md](./AGENT_LOOP.md) - Agent loop integration
- [ARCHITECTURE.md](./ARCHITECTURE.md) - Overall system architecture
