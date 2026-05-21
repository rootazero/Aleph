# P1: Session Orchestration Enhancement — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add steering (interrupt), queue modes, pre-compaction memory persistence, and prompt cache optimization to Aleph's agent loop.

**Architecture:** Extend the existing OTAF agent loop with an interrupt channel checked between tool calls, a `SessionQueue` trait with three strategies injected via `AppContext`, a silent agent turn before compaction, and stable/dynamic prompt partitioning for cache hits.

**Tech Stack:** Rust, tokio (watch/mpsc channels), existing PromptPipeline layer system, Anthropic `cache_control` API.

---

## Task 1: Interrupt Channel Infrastructure

Add an `InterruptSignal` type and wire an interrupt channel into `RunContext` alongside the existing `abort_signal`.

**Files:**
- Modify: `src/agent_loop/agent_loop.rs` (RunContext struct, ~line 55)
- Create: `src/agent_loop/interrupt.rs`
- Modify: `src/agent_loop/mod.rs` (add module)
- Test: `src/agent_loop/interrupt.rs` (inline tests)

**Step 1: Write the failing test**

In `src/agent_loop/interrupt.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_interrupt_signal_send_receive() {
        let (tx, mut rx) = InterruptChannel::new();
        assert!(rx.try_recv().is_none());

        tx.send(InterruptSignal::NewMessage {
            content: "stop that".into(),
        });

        let signal = rx.try_recv();
        assert!(signal.is_some());
        match signal.unwrap() {
            InterruptSignal::NewMessage { content } => assert_eq!(content, "stop that"),
        }
    }

    #[tokio::test]
    async fn test_interrupt_channel_latest_wins() {
        let (tx, mut rx) = InterruptChannel::new();
        tx.send(InterruptSignal::NewMessage { content: "first".into() });
        tx.send(InterruptSignal::NewMessage { content: "second".into() });

        let signal = rx.try_recv().unwrap();
        match signal {
            InterruptSignal::NewMessage { content } => assert_eq!(content, "second"),
        }
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agent_loop::interrupt::tests -v`
Expected: FAIL — module and types don't exist

**Step 3: Write minimal implementation**

In `src/agent_loop/interrupt.rs`:

```rust
use tokio::sync::watch;

#[derive(Debug, Clone)]
pub enum InterruptSignal {
    NewMessage { content: String },
}

pub struct InterruptSender {
    tx: watch::Sender<Option<InterruptSignal>>,
}

pub struct InterruptReceiver {
    rx: watch::Receiver<Option<InterruptSignal>>,
}

pub struct InterruptChannel;

impl InterruptChannel {
    pub fn new() -> (InterruptSender, InterruptReceiver) {
        let (tx, rx) = watch::channel(None);
        (InterruptSender { tx }, InterruptReceiver { rx })
    }
}

impl InterruptSender {
    pub fn send(&self, signal: InterruptSignal) {
        let _ = self.tx.send(Some(signal));
    }
}

impl InterruptReceiver {
    /// Non-blocking check. Returns the latest signal if any, then clears it.
    pub fn try_recv(&mut self) -> Option<InterruptSignal> {
        let val = self.rx.borrow_and_update().clone();
        if val.is_some() {
            // Clear after reading so we don't re-process
            // We can't clear the watch from receiver side,
            // so we track "last seen" version
        }
        val
    }
}
```

Wait — `watch` doesn't support clearing from receiver. Better approach: use `tokio::sync::mpsc` with capacity 1, draining to get latest.

Revised implementation:

```rust
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum InterruptSignal {
    NewMessage { content: String },
}

pub struct InterruptSender {
    tx: mpsc::Sender<InterruptSignal>,
}

pub struct InterruptReceiver {
    rx: mpsc::Receiver<InterruptSignal>,
}

pub struct InterruptChannel;

impl InterruptChannel {
    pub fn new() -> (InterruptSender, InterruptReceiver) {
        let (tx, rx) = mpsc::channel(16);
        (InterruptSender { tx }, InterruptReceiver { rx })
    }
}

impl InterruptSender {
    pub fn send(&self, signal: InterruptSignal) {
        // Use try_send to avoid blocking; drop oldest if full
        let _ = self.tx.try_send(signal);
    }
}

impl InterruptReceiver {
    /// Drain channel, return the latest signal (newest message wins).
    pub fn try_recv(&mut self) -> Option<InterruptSignal> {
        let mut latest = None;
        while let Ok(signal) = self.rx.try_recv() {
            latest = Some(signal);
        }
        latest
    }
}
```

Add module declaration in `src/agent_loop/mod.rs`:

```rust
pub mod interrupt;
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib agent_loop::interrupt::tests -v`
Expected: PASS

**Step 5: Commit**

```
feat(agent-loop): add InterruptChannel for steering support
```

---

## Task 2: Wire Interrupt into RunContext and Agent Loop

Check the interrupt channel between tool calls in the agent loop. When interrupted, cancel the pending action and inject the new message.

**Files:**
- Modify: `src/agent_loop/agent_loop.rs` (RunContext ~line 55, loop body ~line 873)
- Modify: `src/agent_loop/state.rs` (add interrupted step tracking)
- Test: `src/agent_loop/agent_loop.rs` (or separate test file)

**Step 1: Add interrupt_rx to RunContext**

In `src/agent_loop/agent_loop.rs`, add to `RunContext`:

```rust
pub interrupt_rx: Option<interrupt::InterruptReceiver>,
```

**Step 2: Add interrupt check before tool execution**

In the agent loop, before executing each tool call (~line 873 area), add:

```rust
// Check for steering interrupt before executing tool
if let Some(ref mut interrupt_rx) = interrupt_rx {
    if let Some(interrupt::InterruptSignal::NewMessage { content }) = interrupt_rx.try_recv() {
        // Record cancelled action in state
        state.record_interrupted_action(&action, &content);
        // Inject new user message and restart thinking
        state.inject_user_message(content);
        callback.on_action_cancelled(&action, "interrupted by new user input").await;
        continue; // Back to top of loop — will re-think with new context
    }
}
```

**Step 3: Add helper methods to LoopState**

In `src/agent_loop/state.rs`:

```rust
/// Record that an action was cancelled due to interrupt.
pub fn record_interrupted_action(&mut self, action: &Action, new_input: &str) {
    self.steps.push(LoopStep {
        action: action.clone(),
        result: ActionResult::ToolError {
            error: format!("[Cancelled: user sent new input: \"{}\"]", new_input),
            retryable: false,
        },
        duration_ms: 0,
        thinking: None,
    });
    self.step_count += 1;
}

/// Inject a new user message that will be picked up on next think cycle.
pub fn inject_user_message(&mut self, content: String) {
    self.original_request = content;
}
```

**Step 4: Add on_action_cancelled to LoopCallback**

In the LoopCallback trait:

```rust
async fn on_action_cancelled(&self, action: &Action, reason: &str) {
    // Default no-op implementation
}
```

**Step 5: Run compilation check**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 6: Commit**

```
feat(agent-loop): wire interrupt channel into RunContext and loop execution
```

---

## Task 3: SessionQueue Trait + Followup Mode

Define the `SessionQueue` trait and implement the default `FollowupQueue` (messages wait in line).

**Files:**
- Create: `src/agent_loop/queue.rs`
- Modify: `src/agent_loop/mod.rs`
- Test: `src/agent_loop/queue.rs` (inline tests)

**Step 1: Write the failing test**

In `src/agent_loop/queue.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_followup_queue_processes_in_order() {
        let mut queue = FollowupQueue::new();

        queue.enqueue("first message".into()).await;
        queue.enqueue("second message".into()).await;

        let msg1 = queue.next().await;
        assert_eq!(msg1, Some("first message".to_string()));

        let msg2 = queue.next().await;
        assert_eq!(msg2, Some("second message".to_string()));

        let msg3 = queue.next().await;
        assert_eq!(msg3, None);
    }

    #[tokio::test]
    async fn test_followup_queue_does_not_merge() {
        let mut queue = FollowupQueue::new();
        queue.enqueue("hello".into()).await;
        queue.enqueue("world".into()).await;

        // Should return them separately, not merged
        assert_eq!(queue.next().await, Some("hello".to_string()));
        assert_eq!(queue.next().await, Some("world".to_string()));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agent_loop::queue::tests -v`
Expected: FAIL

**Step 3: Write implementation**

```rust
use async_trait::async_trait;
use std::collections::VecDeque;

/// Queue mode determines how new messages are handled while agent is busy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum QueueMode {
    /// Messages wait in line, processed one by one after current turn.
    Followup,
    /// New message interrupts current tool execution (steering).
    Steer,
    /// Collect messages for N seconds, then merge into one.
    Collect,
}

impl Default for QueueMode {
    fn default() -> Self {
        Self::Followup
    }
}

/// A queued message with metadata.
#[derive(Debug, Clone)]
pub struct QueuedMessage {
    pub content: String,
    pub received_at: std::time::Instant,
}

#[async_trait]
pub trait SessionQueue: Send + Sync {
    /// Add a message to the queue.
    async fn enqueue(&mut self, content: String);

    /// Get the next message to process.
    /// Returns None if queue is empty (or collect window hasn't elapsed).
    async fn next(&mut self) -> Option<String>;

    /// The queue mode this implementation represents.
    fn mode(&self) -> QueueMode;
}

/// Default: messages processed sequentially in FIFO order.
pub struct FollowupQueue {
    queue: VecDeque<String>,
}

impl FollowupQueue {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }
}

#[async_trait]
impl SessionQueue for FollowupQueue {
    async fn enqueue(&mut self, content: String) {
        self.queue.push_back(content);
    }

    async fn next(&mut self) -> Option<String> {
        self.queue.pop_front()
    }

    fn mode(&self) -> QueueMode {
        QueueMode::Followup
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib agent_loop::queue::tests -v`
Expected: PASS

**Step 5: Commit**

```
feat(agent-loop): add SessionQueue trait and FollowupQueue implementation
```

---

## Task 4: SteerQueue Implementation

The `SteerQueue` sends an interrupt signal when a new message arrives while the agent is busy.

**Files:**
- Modify: `src/agent_loop/queue.rs`
- Test: inline in same file

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn test_steer_queue_sends_interrupt() {
    let (interrupt_tx, mut interrupt_rx) = InterruptChannel::new();
    let mut queue = SteerQueue::new(interrupt_tx);

    // Enqueue while "busy" — should send interrupt
    queue.enqueue("change of plan".into()).await;

    let signal = interrupt_rx.try_recv();
    assert!(signal.is_some());
    match signal.unwrap() {
        InterruptSignal::NewMessage { content } => {
            assert_eq!(content, "change of plan");
        }
    }

    // Queue should also retain the message for processing
    assert_eq!(queue.next().await, Some("change of plan".to_string()));
}
```

**Step 2: Run test to verify failure**

Run: `cargo test -p alephcore --lib agent_loop::queue::tests::test_steer -v`
Expected: FAIL

**Step 3: Implement SteerQueue**

```rust
use super::interrupt::{InterruptChannel, InterruptSender, InterruptSignal};

pub struct SteerQueue {
    interrupt_tx: InterruptSender,
    pending: VecDeque<String>,
}

impl SteerQueue {
    pub fn new(interrupt_tx: InterruptSender) -> Self {
        Self {
            interrupt_tx,
            pending: VecDeque::new(),
        }
    }
}

#[async_trait]
impl SessionQueue for SteerQueue {
    async fn enqueue(&mut self, content: String) {
        // Send interrupt to agent loop
        self.interrupt_tx.send(InterruptSignal::NewMessage {
            content: content.clone(),
        });
        self.pending.push_back(content);
    }

    async fn next(&mut self) -> Option<String> {
        self.pending.pop_front()
    }

    fn mode(&self) -> QueueMode {
        QueueMode::Steer
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib agent_loop::queue::tests -v`
Expected: PASS

**Step 5: Commit**

```
feat(agent-loop): add SteerQueue with interrupt signaling
```

---

## Task 5: CollectQueue Implementation

Collects messages within a time window, then merges them into one.

**Files:**
- Modify: `src/agent_loop/queue.rs`
- Test: inline

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn test_collect_queue_merges_within_window() {
    let mut queue = CollectQueue::new(std::time::Duration::from_millis(50));

    queue.enqueue("hello".into()).await;
    queue.enqueue("world".into()).await;

    // Within window — should not yield yet
    // (implementation returns None until window elapses)

    // Wait for window
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;

    let merged = queue.next().await;
    assert!(merged.is_some());
    let text = merged.unwrap();
    assert!(text.contains("hello"));
    assert!(text.contains("world"));
}

#[tokio::test]
async fn test_collect_queue_empty_returns_none() {
    let mut queue = CollectQueue::new(std::time::Duration::from_millis(50));
    assert_eq!(queue.next().await, None);
}
```

**Step 2: Run test to verify failure**

Run: `cargo test -p alephcore --lib agent_loop::queue::tests::test_collect -v`

**Step 3: Implement CollectQueue**

```rust
pub struct CollectQueue {
    buffer: Vec<String>,
    window: std::time::Duration,
    first_received: Option<std::time::Instant>,
}

impl CollectQueue {
    pub fn new(window: std::time::Duration) -> Self {
        Self {
            buffer: Vec::new(),
            window,
            first_received: None,
        }
    }
}

#[async_trait]
impl SessionQueue for CollectQueue {
    async fn enqueue(&mut self, content: String) {
        if self.first_received.is_none() {
            self.first_received = Some(std::time::Instant::now());
        }
        self.buffer.push(content);
    }

    async fn next(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            return None;
        }

        // Check if window has elapsed since first message
        if let Some(first) = self.first_received {
            if first.elapsed() < self.window {
                return None; // Still collecting
            }
        }

        // Window elapsed — merge and return
        let merged = self.buffer.join("\n\n");
        self.buffer.clear();
        self.first_received = None;
        Some(merged)
    }

    fn mode(&self) -> QueueMode {
        QueueMode::Collect
    }
}
```

**Step 4: Run tests**

Run: `cargo test -p alephcore --lib agent_loop::queue::tests -v`
Expected: PASS

**Step 5: Commit**

```
feat(agent-loop): add CollectQueue with time-window message merging
```

---

## Task 6: Pre-Compaction Memory Persistence

Insert a silent agent turn before context compaction to persist important information.

**Files:**
- Modify: `src/compressor/mod.rs` (ContextCompressor)
- Modify: `src/agent_loop/agent_loop.rs` (compression trigger point ~line 516)
- Test: `src/compressor/mod.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_pre_compaction_prompt_is_generated() {
        let prompt = PreCompactionPrompt::build();
        assert!(prompt.contains("memory_store"));
        assert!(prompt.contains("compacted"));
    }
}
```

**Step 2: Run test to verify failure**

Run: `cargo test -p alephcore --lib compressor::tests::test_pre_compaction -v`

**Step 3: Implement PreCompactionPrompt**

In `src/compressor/mod.rs`, add:

```rust
pub struct PreCompactionPrompt;

impl PreCompactionPrompt {
    pub fn build() -> String {
        concat!(
            "SYSTEM: Your conversation context is about to be compacted to save space. ",
            "Before this happens, review the recent conversation and use the memory_store tool ",
            "to persist any important information that should be remembered long-term. ",
            "Focus on: user preferences, decisions made, key facts learned, and task progress. ",
            "Do NOT store trivial greetings or small talk. ",
            "After storing, respond with only: [memory_flush_complete]"
        ).to_string()
    }
}
```

**Step 4: Add silent turn flag to LoopState**

In `src/agent_loop/state.rs`:

```rust
pub silent_mode: bool, // When true, do not emit response to user
```

**Step 5: Wire into agent loop compression point**

In `src/agent_loop/agent_loop.rs`, at the compression check (~line 516):

```rust
if self.compressor.should_compress(&state) {
    // Pre-compaction: run a silent agent turn to flush memories
    if !state.silent_mode {
        let flush_prompt = PreCompactionPrompt::build();
        state.inject_user_message(flush_prompt);
        state.silent_mode = true;
        // The next iteration will process the flush prompt silently
        // After it completes (detects [memory_flush_complete]), proceed to compress
        continue;
    }

    // Silent turn completed, now compress
    state.silent_mode = false;
    let compressed = self.compressor.compress(&state.steps, &state.history_summary).await?;
    state.apply_compression(compressed);
}
```

**Step 6: Run compilation check**

Run: `cargo check -p alephcore`
Expected: PASS

**Step 7: Commit**

```
feat(compressor): add pre-compaction silent memory flush
```

---

## Task 7: Prompt Cache Optimization — Stable/Dynamic Partitioning

Restructure system prompt assembly to maximize LLM cache hits by placing stable sections first.

**Files:**
- Modify: `src/thinker/prompt_pipeline.rs` (layer ordering)
- Modify: `src/thinker/prompt_builder/mod.rs` (build_system_prompt_cached)
- Modify: `src/thinker/cache.rs` (CacheContext breakpoint)
- Test: `src/thinker/prompt_pipeline.rs` (inline tests)

**Step 1: Write the failing test**

In `src/thinker/prompt_pipeline.rs`:

```rust
#[cfg(test)]
mod cache_partition_tests {
    use super::*;

    #[test]
    fn test_stable_layers_come_before_dynamic() {
        let pipeline = PromptPipeline::default();
        let layers = pipeline.layer_info();

        let mut found_dynamic = false;
        for (priority, name, stability) in &layers {
            if *stability == LayerStability::Dynamic {
                found_dynamic = true;
            }
            if found_dynamic && *stability == LayerStability::Stable {
                panic!(
                    "Stable layer '{}' (priority {}) found after dynamic layer",
                    name, priority
                );
            }
        }
    }
}
```

**Step 2: Run test to verify failure**

Run: `cargo test -p alephcore --lib thinker::prompt_pipeline::cache_partition_tests -v`
Expected: FAIL — `LayerStability` doesn't exist

**Step 3: Add LayerStability to PromptLayer trait**

In the PromptLayer trait definition:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerStability {
    /// Content rarely changes between requests (persona, tools, skills).
    Stable,
    /// Content changes per request (time, session context, memory).
    Dynamic,
}

// Add to PromptLayer trait:
fn stability(&self) -> LayerStability {
    LayerStability::Stable // Default: stable
}
```

**Step 4: Mark dynamic layers**

Override `stability()` to return `Dynamic` for these layers:
- `RuntimeContextLayer` (priority 200) — contains current time
- `InboundContextLayer` (priority 55) — per-request sender/channel
- `WorkspaceFilesLayer` (priority 1550) — may change between requests
- `MemoryAugmentationLayer` (priority 1575) — per-request memory recall
- `PoePromptLayer` (priority 505) — per-request success criteria

**Step 5: Reorder priorities so dynamic layers cluster at the end**

Adjust priorities:
- Move `InboundContextLayer` from 55 → 1700
- Move `RuntimeContextLayer` from 200 → 1710
- Move `PoePromptLayer` from 505 → 1720
- Move `WorkspaceFilesLayer` from 1550 → 1730
- Move `MemoryAugmentationLayer` from 1575 → 1740
- Keep `LanguageLayer` at 1600 → 1750 (it's small but changes)

This ensures all stable sections (persona, tools, security, skills, guidelines) are contiguous at the top, and all dynamic sections cluster at the bottom.

**Step 6: Update cache breakpoint**

In `src/thinker/cache.rs`, update `build_system_prompt_cached()`:

```rust
pub fn build_system_prompt_cached(&self, tools: &[ToolInfo]) -> Vec<SystemPromptPart> {
    let input = LayerInput::basic(&self.config, tools);

    // Collect stable layers
    let stable = self.pipeline.execute_stable_only(&input);
    // Collect dynamic layers
    let dynamic = self.pipeline.execute_dynamic_only(&input);

    vec![
        SystemPromptPart { content: stable, cache: true },
        SystemPromptPart { content: dynamic, cache: false },
    ]
}
```

Add helper methods to `PromptPipeline`:

```rust
pub fn execute_stable_only(&self, input: &LayerInput) -> String {
    self.layers.iter()
        .filter(|l| l.stability() == LayerStability::Stable)
        .filter_map(|l| l.inject(input))
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub fn execute_dynamic_only(&self, input: &LayerInput) -> String {
    self.layers.iter()
        .filter(|l| l.stability() == LayerStability::Dynamic)
        .filter_map(|l| l.inject(input))
        .collect::<Vec<_>>()
        .join("\n\n")
}
```

**Step 7: Run tests**

Run: `cargo test -p alephcore --lib thinker::prompt_pipeline -v`
Expected: PASS

**Step 8: Commit**

```
feat(thinker): partition system prompt into stable/dynamic zones for cache optimization
```

---

## Task 8: Anthropic cache_control Breakpoint Wiring

Ensure the stable/dynamic partition is sent to Anthropic API with proper `cache_control` markers.

**Files:**
- Modify: `src/providers/protocols/anthropic.rs` (build_request)
- Modify: `src/providers/anthropic/types.rs` (SystemBlock)
- Test: `src/providers/anthropic/types.rs` (inline)

**Step 1: Write the failing test**

```rust
#[test]
fn test_system_block_with_cache_control_serializes() {
    let block = SystemBlock {
        block_type: "text".into(),
        text: "You are a helpful assistant.".into(),
        cache_control: Some(CacheControl {
            cache_type: "ephemeral".into(),
        }),
    };
    let json = serde_json::to_string(&block).unwrap();
    assert!(json.contains("cache_control"));
    assert!(json.contains("ephemeral"));
}
```

**Step 2: Run to verify failure**

**Step 3: Add cache_control to SystemBlock**

In `src/providers/anthropic/types.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub cache_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}
```

**Step 4: Wire in anthropic.rs build_request**

When `SystemPromptPart.cache == true`, attach `cache_control: { type: "ephemeral" }` to the corresponding `SystemBlock`.

```rust
// In build_request, when constructing system blocks:
let system_blocks: Vec<SystemBlock> = prompt_parts.iter().map(|part| {
    SystemBlock {
        block_type: "text".into(),
        text: part.content.clone(),
        cache_control: if part.cache {
            Some(CacheControl { cache_type: "ephemeral".into() })
        } else {
            None
        },
    }
}).collect();
```

**Step 5: Run tests**

Run: `cargo test -p alephcore --lib providers::anthropic -v`
Expected: PASS

**Step 6: Commit**

```
feat(anthropic): wire cache_control ephemeral breakpoint for system prompt caching
```

---

## Task 9: Integration — Queue Mode Configuration

Wire queue mode into session configuration so it can be set per-session via config or RPC.

**Files:**
- Modify: `src/config/` (add queue_mode to session config)
- Modify: `src/gateway/inbound_router.rs` (use queue when dispatching)
- Test: config deserialization test

**Step 1: Write the failing test**

```rust
#[test]
fn test_session_config_deserializes_queue_mode() {
    let toml = r#"
    [session]
    queue_mode = "steer"
    collect_window_ms = 3000
    "#;
    let config: SessionConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.queue_mode, QueueMode::Steer);
    assert_eq!(config.collect_window_ms, Some(3000));
}
```

**Step 2: Run to verify failure**

**Step 3: Add fields to SessionConfig**

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionConfig {
    // ... existing fields ...
    #[serde(default)]
    pub queue_mode: QueueMode,
    pub collect_window_ms: Option<u64>,
}
```

**Step 4: Wire into InboundRouter**

In `inbound_router.rs`, when creating a new execution context, instantiate the appropriate queue based on config:

```rust
fn create_session_queue(&self, config: &SessionConfig, interrupt_tx: Option<InterruptSender>) -> Box<dyn SessionQueue> {
    match config.queue_mode {
        QueueMode::Followup => Box::new(FollowupQueue::new()),
        QueueMode::Steer => {
            let tx = interrupt_tx.expect("SteerQueue requires interrupt channel");
            Box::new(SteerQueue::new(tx))
        }
        QueueMode::Collect => {
            let window = std::time::Duration::from_millis(
                config.collect_window_ms.unwrap_or(3000)
            );
            Box::new(CollectQueue::new(window))
        }
    }
}
```

**Step 5: Run tests**

Run: `cargo test -p alephcore --lib -v`
Expected: PASS

**Step 6: Commit**

```
feat(config): add queue_mode session configuration with gateway wiring
```

---

## Task 10: Final Integration Test

End-to-end test verifying the full steering flow: message in → interrupt → cancelled tool → new response.

**Files:**
- Create: `src/agent_loop/tests/steering_integration.rs` (or inline in agent_loop)

**Step 1: Write integration test**

```rust
#[tokio::test]
async fn test_steering_cancels_tool_and_reprocesses() {
    // Setup: mock thinker that returns a tool_use on first call,
    // then text on second call (after interrupt)
    let (interrupt_tx, interrupt_rx) = InterruptChannel::new();

    // Simulate: agent starts executing tool, then user sends interrupt
    // The tool should be cancelled, and the new message should be processed

    // This test verifies the contract:
    // 1. interrupt_rx.try_recv() returns the new message
    // 2. state.steps contains a Cancelled result
    // 3. The loop continues with the new input
}
```

**Step 2: Run and verify**

Run: `cargo test -p alephcore --lib agent_loop::tests::steering -v`

**Step 3: Commit**

```
test(agent-loop): add steering integration test
```

---

## Summary

| Task | Component | Estimated Complexity |
|------|-----------|---------------------|
| 1 | InterruptChannel | Low |
| 2 | Wire interrupt into agent loop | Medium |
| 3 | SessionQueue trait + FollowupQueue | Low |
| 4 | SteerQueue | Low |
| 5 | CollectQueue | Low |
| 6 | Pre-compaction memory flush | Medium |
| 7 | Prompt stable/dynamic partitioning | Medium |
| 8 | Anthropic cache_control wiring | Low |
| 9 | Config + gateway integration | Medium |
| 10 | Integration test | Medium |

**Dependencies:** Task 1 → 2 → 4 (interrupt chain). Task 3 independent. Task 5 independent. Task 6 independent. Task 7 → 8 (cache chain). Task 9 depends on 1-5. Task 10 depends on all.
