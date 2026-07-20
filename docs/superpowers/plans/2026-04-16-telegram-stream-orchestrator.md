# Telegram Stream Orchestrator Implementation Plan

**Date**: 2026-04-16  
**Spec**: [Design Document](../specs/2026-04-16-telegram-stream-orchestrator-design.md)  
**Target**: `alephcore` crate, Telegram channel only

---

## Phase 1: StreamOrchestrator Foundation

**Goal**: Create the unified orchestration layer that manages multiple streaming lanes without modifying `Channel` trait or gateway core.

### 1.1 New Module Structure

Create `src/gateway/interfaces/telegram/streaming/` with:

```
streaming/
├── mod.rs                    # Public exports: StreamOrchestrator, LaneHandle
├── orchestrator.rs           # StreamOrchestrator struct & impl
├── lane_tracker.rs           # LaneDeliveryTracker with per-lane state
├── lane_handle.rs            # LaneHandle for stream writers
├── status_controller.rs      # StatusReactionController impl
└── draft_api.rs              # DraftStreamHandler for Telegram Bot API draft endpoints
```

### 1.2 Core Types

```rust
// streaming/lane_tracker.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LaneId {
    Answer,      // Main streaming response
    Reasoning,   // <think> content
    Draft,       // Experimental draft input box
}

#[derive(Debug)]
struct LaneState {
    preview_message_id: Option<i64>,
    final_message_id: Option<i64>,
    is_streaming: bool,
    last_update: Instant,
}

pub struct LaneDeliveryTracker {
    lanes: HashMap<LaneId, LaneState>,
    chat_id: i64,
    thread_id: Option<i64>,
}

// streaming/lane_handle.rs
pub struct LaneHandle {
    lane_id: LaneId,
    tracker: Arc<Mutex<LaneDeliveryTracker>>,
    delivery: TelegramDelivery,
    config: StreamingOptions,
}

impl LaneHandle {
    pub async fn write_chunk(&self, text: &str) -> Result<()>;
    pub async fn finalize(&self, final_text: &str) -> Result<i64>; // Returns final message_id
}

// streaming/orchestrator.rs
pub struct StreamOrchestrator {
    delivery: TelegramDelivery,
    tracker: Arc<Mutex<LaneDeliveryTracker>>,
    status_controller: StatusReactionController,
    config: StreamingOptions,
    event_rx: mpsc::Receiver<StreamEvent>,
}

impl StreamOrchestrator {
    pub fn new(
        delivery: TelegramDelivery,
        config: StreamingOptions,
    ) -> (Self, mpsc::Sender<StreamEvent>) {
        // Returns orchestrator + sender end for event injection
    }
    
    pub async fn run(mut self, initial_inbound: InboundMessage) -> Result<()> {
        // Main event loop: receives StreamEvents, routes to appropriate lane
        // Handles: ResponseChunk, ReasoningBlock, ToolStart, ToolEnd, RunComplete
    }
    
    pub fn get_lane(&self, lane_id: LaneId) -> LaneHandle;
}
```

### 1.3 Integration with Existing Delivery

Modify `src/gateway/interfaces/telegram/delivery.rs`:

```rust
// Add to TelegramDelivery impl
pub async fn send_streaming_orchestrated(
    &self,
    target: &MessageTarget,
    config: &StreamingOptions,
) -> Result<(StreamOrchestrator, mpsc::Sender<StreamEvent>)> {
    let orchestrator = StreamOrchestrator::new(self.clone(), config.clone());
    // Set up initial status reactions via status_controller
    Ok(orchestrator)
}
```

### 1.4 Config V2 Extension

Extend `src/gateway/interfaces/telegram/config_v2.rs`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct StreamingOptions {
    // Existing fields...
    
    /// Enable experimental Draft API support
    #[serde(default = "default_draft_enabled")]
    pub draft_api_enabled: bool,
    
    /// Enable reasoning lane extraction (<think> tags)
    #[serde(default = "default_reasoning_enabled")]
    pub reasoning_lane_enabled: bool,
    
    /// Status reaction configuration
    #[serde(default)]
    pub status_reactions: StatusReactionConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct StatusReactionConfig {
    /// Reaction to set when agent starts processing
    pub processing: Option<String>,  // default: "👀"
    
    /// Reaction to set when tools are executing  
    pub tool_active: Option<String>, // default: "🔧"
    
    /// Reaction to set when streaming is complete
    pub complete: Option<String>,    // default: "👍"
}
```

### 1.5 Phase 1 Deliverables

| File | Lines | Purpose |
|------|-------|---------|
| `streaming/lane_tracker.rs` | ~150 | LaneDeliveryTracker with per-lane message_id tracking |
| `streaming/lane_handle.rs` | ~100 | LaneHandle write API for stream consumers |
| `streaming/orchestrator.rs` | ~200 | Main orchestration loop, event routing |
| `streaming/mod.rs` | ~30 | Module exports |
| Updates to `delivery.rs` | ~30 | Integration hook |
| Updates to `config_v2.rs` | ~40 | New configuration options |

**Phase 1 Tests**:
- Unit test: `lane_tracker::test_lane_state_transitions`
- Unit test: `orchestrator::test_event_routing_to_lanes`
- Integration test: `test_stream_orchestrator_creation`

---

## Phase 2: StatusReactionController Integration

**Goal**: Link execution engine events to Telegram message reactions without leaking into gateway core.

### 2.1 StatusReactionController Implementation

```rust
// streaming/status_controller.rs
pub struct StatusReactionController {
    delivery: TelegramDelivery,
    config: StatusReactionConfig,
    current_reaction: Arc<Mutex<Option<String>>>,
}

impl StatusReactionController {
    pub fn new(delivery: TelegramDelivery, config: StatusReactionConfig) -> Self;
    
    pub async fn handle_event(&self, event: &StreamEvent, message_id: i64) -> Result<()> {
        match event {
            StreamEvent::ResponseChunk { .. } if self.is_first_chunk() => {
                self.set_reaction(&self.config.processing, message_id).await
            }
            StreamEvent::ToolStart { .. } => {
                self.set_reaction(&self.config.tool_active, message_id).await
            }
            StreamEvent::ToolEnd { .. } => {
                // Transition back to processing if more chunks expected
                self.set_reaction(&self.config.processing, message_id).await
            }
            StreamEvent::RunComplete { .. } => {
                self.set_reaction(&self.config.complete, message_id).await
            }
            _ => Ok(())
        }
    }
    
    async fn set_reaction(&self, emoji: &Option<String>, message_id: i64) -> Result<()> {
        // Use existing delivery.react() API
        // Track current reaction to avoid redundant API calls
    }
}
```

### 2.2 ReplyEmitter Integration Point

In `src/gateway/reply_emitter.rs`, modify the Telegram branch:

```rust
// Inside ReplyEmitter::handle_streaming
Channel::Telegram(telegram) => {
    if let Some(config) = telegram.get_streaming_config() {
        // New orchestrated path (if config.v2_streaming_enabled)
        let (orchestrator, event_tx) = telegram
            .send_streaming_orchestrated(&target, &config)
            .await?;
        
        // Spawn orchestrator in background
        tokio::spawn(orchestrator.run(inbound.clone()));
        
        // Return event sender for stream routing
        Ok(StreamChannel::Orchestrated(event_tx))
    } else {
        // Legacy path (existing StreamingController)
        // ... existing code
    }
}
```

### 2.3 Phase 2 Deliverables

| File | Lines | Purpose |
|------|-------|---------|
| `streaming/status_controller.rs` | ~120 | Status reaction state machine |
| Updates to `reply_emitter.rs` | ~40 | Integration with orchestrator |

**Phase 2 Tests**:
- Unit test: `status_controller::test_reaction_state_transitions`
- Mock test: `test_status_reactions_with_simulated_events`

---

## Phase 3: ReasoningLane Support

**Goal**: Extract `<think>` content into separate message stream.

### 3.1 Reasoning Extraction Refactor

Current code in `reply_emitter.rs` has `split_reasoning()` that returns `(reasoning, answer)`. We need to stream reasoning separately.

```rust
// streaming/reasoning_extractor.rs (optional helper)
pub struct ReasoningExtractor;

impl ReasoningExtractor {
    /// Returns (reasoning_chunk, answer_chunk, is_complete)
    /// Handles partial `<think>` tags across chunk boundaries
    pub fn process_chunk(
        &mut self, 
        chunk: &str
    ) -> (Option<String>, Option<String>, bool);
}
```

### 3.2 Orchestrator Integration

In `orchestrator.rs` event handling:

```rust
match event {
    StreamEvent::ResponseChunk { content, .. } => {
        if self.config.reasoning_lane_enabled {
            let (reasoning, answer, _) = self.reasoning_extractor.process_chunk(&content);
            if let Some(r) = reasoning {
                self.get_lane(LaneId::Reasoning).write_chunk(&r).await?;
            }
            if let Some(a) = answer {
                self.get_lane(LaneId::Answer).write_chunk(&a).await?;
            }
        } else {
            // Legacy: send everything to Answer lane
            self.get_lane(LaneId::Answer).write_chunk(&content).await?;
        }
    }
    StreamEvent::ReasoningBlock { content, .. } => {
        // Direct reasoning block (if engine provides it)
        self.get_lane(LaneId::Reasoning).write_chunk(&content).await?;
    }
    // ...
}
```

### 3.3 Backward Compatibility

If `reasoning_lane_enabled: false` (default), behavior matches current:
- All content goes to Answer lane
- `<think>` tags may be stripped or included based on existing `sanitize_llm_output`

### 3.4 Phase 3 Deliverables

| File | Lines | Purpose |
|------|-------|---------|
| `streaming/reasoning_extractor.rs` | ~80 | Incremental `<think>` parsing |
| Updates to `orchestrator.rs` | ~40 | Reasoning lane routing |

**Phase 3 Tests**:
- Unit test: `reasoning_extractor::test_partial_think_tags`
- Unit test: `reasoning_extractor::test_nested_think`
- Integration test: `test_reasoning_lane_separation`

---

## Phase 4: Draft API Support (Optional)

**Goal**: Experimental Draft Stream for input box preview (Telegram Bot API beta feature).

### 4.1 DraftStreamHandler

```rust
// streaming/draft_api.rs
pub struct DraftStreamHandler {
    bot_token: String,
    chat_id: i64,
    http_client: reqwest::Client,
}

impl DraftStreamHandler {
    /// Send message to user's input box as draft
    pub async fn send_draft(&self, text: &str) -> Result<()>;
    
    /// Clear the draft
    pub async fn clear_draft(&self) -> Result<()>;
}
```

**Note**: Draft API requires Telegram Bot API server support. Marked as `#[cfg(feature = "telegram-draft-api")]`.

### 4.2 Phase 4 Deliverables

| File | Lines | Purpose |
|------|-------|---------|
| `streaming/draft_api.rs` | ~100 | Draft API client |

---

## Phase 5: Bridge Code Cleanup

**Goal**: Remove deprecated bridge code after Phases 1-3 are stable in production.

### 5.1 Files to Remove

- `src/gateway/bridge/mod.rs`
- `src/gateway/bridge/supervisor.rs`
- `src/gateway/bridge/bridged_channel.rs`
- `src/gateway/bridge/types.rs`

### 5.2 Files to Update

- `src/gateway/mod.rs`: Remove `pub mod bridge`
- `src/gateway/link/manager.rs`: Remove bridge-dependent code paths
- `src/gateway/interfaces/telegram/mod.rs`: Remove legacy bridge integration if any

### 5.3 Verification

```bash
# Ensure no bridge references remain
grep -r "gateway::bridge" src/
grep -r "mod bridge" src/
```

### 5.4 Phase 5 Deliverables

| Action | Scope |
|--------|-------|
| Delete 4 files | `src/gateway/bridge/*` |
| Update imports | 2-3 files |
| Verify clean compile | `cargo check -p alephcore` |

---

## Testing Strategy

### Unit Tests (per module)

```rust
// In each module file
#[cfg(test)]
mod tests {
    use super::*;
    
    // Test state machines
    // Test async behavior with tokio::test
    // Mock TelegramDelivery for isolated tests
}
```

### Integration Tests

```rust
// tests/telegram_streaming_integration.rs
#[tokio::test]
async fn test_full_stream_orchestration() {
    // Setup mock Telegram server (or mock delivery)
    // Send StreamEvent sequence
    // Verify lane message_ids tracked correctly
    // Verify status reactions set in correct order
}
```

### Manual Verification

1. Start Aleph with Telegram channel
2. Send message that triggers streaming
3. Observe: 👀 appears on user message
4. Observe: 🔧 appears during tool execution
5. Observe: 👍 appears when complete
6. Check: reasoning appears in separate message (if enabled)

---

## Migration Path

### Config Migration

Existing `telegram.config.toml` works without changes:

```toml
# Old config (still valid)
[streaming]
enabled = true

# New config (optional additions)
[streaming]
enabled = true
reasoning_lane_enabled = true  # Opt-in to new feature

[streaming.status_reactions]
processing = "👀"
tool_active = "🔧"
complete = "👍"
```

### Feature Flags

```rust
// In Cargo.toml
[features]
default = []
telegram-draft-api = []  # Experimental
```

---

## Risk Mitigation

| Risk | Mitigation |
|------|------------|
| Breaking existing streaming | Keep legacy path as default; new path opt-in |
| Status reaction rate limits | Debounce rapid state changes; batch updates |
| Memory leak in LaneTracker | TTL on inactive lanes; max 3 lanes per chat |
| Draft API instability | Feature-gated; fallback to normal streaming |

---

## Build Commands

```bash
# Phase 1-4 development
cargo check -p alephcore
cargo clippy -p alephcore -- -D warnings
cargo test -p alephcore --lib streaming

# Phase 5 cleanup verification
cargo check -p alephcore --all-features
cargo test -p alephcore --lib
```

---

## Success Criteria

- [ ] `cargo check -p alephcore` passes (no warnings)
- [ ] `cargo test -p alephcore --lib telegram` all green
- [ ] Integration test: streaming with status reactions works
- [ ] Integration test: reasoning lane separation works
- [ ] Config: old configs load without modification
- [ ] Config: new features require explicit opt-in
- [ ] Bridge cleanup: zero references to `gateway::bridge`

---

*Plan Version: 1.0*  
*Next Step: Await user confirmation on execution approach (subagent-driven vs inline)*
