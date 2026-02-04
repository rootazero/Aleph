# Session Compaction Enhancement Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enhance Aleph's session compaction system to match OpenCode's capabilities for effective token control and context preservation.

**Architecture:** Implement three-layer compaction: (1) Smart pruning with protection mechanisms, (2) LLM-driven summarization with dedicated compaction agent, (3) filterCompacted history boundary detection. Add cache-aware token tracking and configurable compaction policies.

**Tech Stack:** Rust, async-trait, serde, chrono, existing Thinker/AiProvider infrastructure

---

## Gap Analysis: OpenCode vs Aleph

| Feature | OpenCode | Aleph | Gap |
|---------|----------|--------|-----|
| **Overflow Detection** | Real-time check before each iteration | Stubbed EventHandler | Critical |
| **History Filtering** | filterCompacted() creates natural breakpoints | None | Critical |
| **Summarization** | LLM-driven "compaction" agent | Template-based string concat | Major |
| **Prune Protection** | PRUNE_PROTECTED_TOOLS, PRUNE_PROTECT thresholds | None | Major |
| **Token Tracking** | input + output + cache.read + reasoning | input + output only | Moderate |
| **Configuration** | compaction.auto, compaction.prune | Hardcoded | Moderate |
| **Compacted Marker** | part.state.time.compacted timestamp | Direct content replacement | Minor |

---

## Task 1: Add Compaction Configuration

**Files:**
- Modify: `core/src/config.rs` (add CompactionConfig struct)
- Create: `core/src/components/compaction_config.rs`
- Test: `core/src/components/session_compactor.rs` (add config tests)

**Step 1: Read existing config structure**

Run: Review `core/src/config.rs` to understand current config patterns

**Step 2: Write failing test for compaction config**

```rust
#[cfg(test)]
mod compaction_config_tests {
    use super::*;

    #[test]
    fn test_compaction_config_default() {
        let config = CompactionConfig::default();
        assert!(config.auto_compact);
        assert!(config.prune_enabled);
        assert_eq!(config.prune_minimum, 20_000);
        assert_eq!(config.prune_protect, 40_000);
        assert!(config.protected_tools.contains(&"skill".to_string()));
    }

    #[test]
    fn test_compaction_config_disabled() {
        let config = CompactionConfig {
            auto_compact: false,
            prune_enabled: false,
            ..Default::default()
        };
        assert!(!config.auto_compact);
        assert!(!config.prune_enabled);
    }
}
```

**Step 3: Run test to verify it fails**

Run: `cd core && cargo test compaction_config_tests`
Expected: FAIL with "cannot find type `CompactionConfig`"

**Step 4: Implement CompactionConfig**

```rust
// In session_compactor.rs, add near top

/// Configuration for session compaction behavior
#[derive(Debug, Clone)]
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

**Step 5: Run test to verify it passes**

Run: `cd core && cargo test compaction_config_tests`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/components/session_compactor.rs
git commit -m "feat(compactor): add CompactionConfig for configurable compaction behavior"
```

---

## Task 2: Enhanced Token Tracking with Cache Awareness

**Files:**
- Modify: `core/src/components/session_compactor.rs` (TokenUsage struct)
- Modify: `core/src/event/types.rs` (update TokenUsage)
- Test: In same file

**Step 1: Write failing test for cache-aware token tracking**

```rust
#[test]
fn test_enhanced_token_usage() {
    let usage = EnhancedTokenUsage {
        input: 1000,
        output: 500,
        reasoning: 200,
        cache_read: 300,
        cache_write: 100,
    };

    // Total for overflow check = input + cache_read + output
    assert_eq!(usage.total_for_overflow(), 1800);

    // Total billable (excluding cache reads which are cheaper)
    assert_eq!(usage.total_billable(), 1700);
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test test_enhanced_token_usage`
Expected: FAIL with "cannot find type `EnhancedTokenUsage`"

**Step 3: Implement EnhancedTokenUsage**

```rust
/// Enhanced token usage tracking with cache awareness
#[derive(Debug, Clone, Default)]
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

    /// Calculate billable tokens (cache reads are cheaper, often excluded)
    pub fn total_billable(&self) -> u64 {
        self.input + self.output + self.reasoning
    }

    /// Add another usage to this one
    pub fn add(&mut self, other: &EnhancedTokenUsage) {
        self.input += other.input;
        self.output += other.output;
        self.reasoning += other.reasoning;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cd core && cargo test test_enhanced_token_usage`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/components/session_compactor.rs
git commit -m "feat(compactor): add EnhancedTokenUsage with cache awareness"
```

---

## Task 3: Smart Pruning with Protection Mechanism

**Files:**
- Modify: `core/src/components/session_compactor.rs`
- Test: In same file

**Step 1: Write failing test for protected tool pruning**

```rust
#[test]
fn test_prune_respects_protected_tools() {
    let config = CompactionConfig {
        protected_tools: vec!["skill".to_string(), "read_file".to_string()],
        ..Default::default()
    };
    let compactor = SessionCompactor::with_config(config.clone());
    let mut session = create_test_session_with_skill_calls();

    compactor.prune_old_tool_outputs(&mut session);

    // Skill tool outputs should NOT be pruned
    let skill_outputs: Vec<_> = session.parts.iter()
        .filter_map(|p| match p {
            SessionPart::ToolCall(tc) if tc.tool_name == "skill" => tc.output.as_ref(),
            _ => None,
        })
        .collect();

    for output in skill_outputs {
        assert!(!output.contains("pruned"), "Skill outputs should not be pruned");
    }
}

#[test]
fn test_prune_with_thresholds() {
    let config = CompactionConfig {
        prune_minimum: 1000,
        prune_protect: 2000,
        ..Default::default()
    };
    let compactor = SessionCompactor::with_config(config);
    let mut session = create_large_test_session();

    let pruned_info = compactor.prune_with_thresholds(&mut session);

    // Should only prune if exceeds prune_minimum
    assert!(pruned_info.tokens_pruned >= 1000 || pruned_info.tokens_pruned == 0);
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test test_prune_respects_protected_tools`
Expected: FAIL

**Step 3: Implement smart pruning**

```rust
/// Information about pruning operation
#[derive(Debug, Clone)]
pub struct PruneInfo {
    pub tokens_pruned: u64,
    pub parts_pruned: usize,
    pub parts_protected: usize,
}

impl SessionCompactor {
    /// Create compactor with custom config
    pub fn with_config(config: CompactionConfig) -> Self {
        Self {
            token_tracker: TokenTracker::new(),
            keep_recent_tools: 10,
            config,
        }
    }

    /// Prune old tool outputs with protection mechanisms
    ///
    /// Algorithm (matches OpenCode):
    /// 1. Iterate backward through messages from most recent
    /// 2. Skip until reaching 2+ user turns
    /// 3. Find completed tool calls (excluding protected tools)
    /// 4. Accumulate tool outputs until exceeding prune_protect
    /// 5. If total > prune_minimum, mark outputs with compacted timestamp
    pub fn prune_with_thresholds(&self, session: &mut ExecutionSession) -> PruneInfo {
        let mut total_tokens: u64 = 0;
        let mut to_prune: Vec<usize> = Vec::new();
        let mut user_turns = 0;
        let mut parts_protected = 0;

        // Iterate backward
        for (idx, part) in session.parts.iter().enumerate().rev() {
            if matches!(part, SessionPart::UserInput(_)) {
                user_turns += 1;
            }

            // Skip until we've seen at least 2 user turns
            if user_turns < 2 {
                continue;
            }

            if let SessionPart::ToolCall(tc) = part {
                // Check if tool is protected
                if self.config.protected_tools.contains(&tc.tool_name) {
                    parts_protected += 1;
                    continue;
                }

                // Already pruned? Stop here
                if tc.output.as_ref().map_or(false, |o| o.contains("[Output pruned")) {
                    break;
                }

                if let Some(ref output) = tc.output {
                    let estimate = TokenTracker::estimate_tokens(output);
                    total_tokens += estimate;

                    // Only mark for pruning after exceeding protect threshold
                    if total_tokens > self.config.prune_protect {
                        to_prune.push(idx);
                    }
                }
            }
        }

        // Calculate actual tokens to be pruned
        let tokens_pruned: u64 = to_prune.iter()
            .filter_map(|&idx| {
                if let SessionPart::ToolCall(tc) = &session.parts[idx] {
                    tc.output.as_ref().map(|o| TokenTracker::estimate_tokens(o))
                } else {
                    None
                }
            })
            .sum();

        // Only prune if exceeds minimum threshold
        if tokens_pruned >= self.config.prune_minimum {
            let timestamp = chrono::Utc::now().timestamp();
            for &idx in &to_prune {
                if let SessionPart::ToolCall(ref mut tc) = session.parts[idx] {
                    tc.output = Some(format!(
                        "[Output pruned at {} to save context]",
                        timestamp
                    ));
                }
            }
        }

        PruneInfo {
            tokens_pruned: if tokens_pruned >= self.config.prune_minimum { tokens_pruned } else { 0 },
            parts_pruned: if tokens_pruned >= self.config.prune_minimum { to_prune.len() } else { 0 },
            parts_protected,
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cd core && cargo test test_prune_respects_protected_tools test_prune_with_thresholds`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/components/session_compactor.rs
git commit -m "feat(compactor): implement smart pruning with protection mechanisms"
```

---

## Task 4: LLM-Driven Summarization

**Files:**
- Modify: `core/src/components/session_compactor.rs`
- Modify: `core/src/thinker/mod.rs` (if needed)
- Test: In same file

**Step 1: Write failing test for LLM summarization**

```rust
#[tokio::test]
async fn test_generate_llm_summary() {
    let compactor = SessionCompactor::new();
    let session = create_test_session();

    // Mock thinker that returns a summary
    let summary_result = compactor.generate_llm_summary(&session, None).await;

    assert!(summary_result.is_ok());
    let summary = summary_result.unwrap();

    // Summary should focus on continuation context
    assert!(summary.contains("Original Request") || summary.contains("Summary"));
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test test_generate_llm_summary`
Expected: FAIL

**Step 3: Implement LLM summarization**

```rust
/// Compaction summary prompt (matches OpenCode's compaction.txt)
const COMPACTION_PROMPT: &str = r#"You are a helpful AI assistant tasked with summarizing conversations.

Provide a detailed prompt for continuing our conversation above. Focus on information that would be helpful for continuing the conversation:
- What was done
- What is currently being worked on
- Which files are being modified
- What needs to be done next
- Key user requests, constraints, or preferences
- Important technical decisions and why they were made

Write in a way that allows a new session to continue seamlessly without access to the full conversation history."#;

impl SessionCompactor {
    /// Generate a summary using LLM (like OpenCode's compaction agent)
    pub async fn generate_llm_summary(
        &self,
        session: &ExecutionSession,
        thinker: Option<&dyn Thinker>,
    ) -> Result<String, AlephError> {
        // Build context from session parts
        let context = self.build_summary_context(session);

        // If no thinker provided, fall back to template-based summary
        let Some(thinker) = thinker else {
            return Ok(self.generate_summary(session));
        };

        // Create messages for summarization
        let messages = vec![
            Message {
                role: MessageRole::System,
                content: COMPACTION_PROMPT.to_string(),
            },
            Message {
                role: MessageRole::User,
                content: context,
            },
        ];

        // Call thinker for summary
        let response = thinker.think(&messages, None).await?;

        Ok(response.content)
    }

    /// Build context string from session for summarization
    fn build_summary_context(&self, session: &ExecutionSession) -> String {
        let mut context_parts = Vec::new();

        for part in &session.parts {
            match part {
                SessionPart::UserInput(input) => {
                    context_parts.push(format!("User: {}", input.text));
                }
                SessionPart::AiResponse(response) => {
                    context_parts.push(format!("Assistant: {}", response.content));
                }
                SessionPart::ToolCall(tc) => {
                    let status = if tc.error.is_some() { "failed" } else { "completed" };
                    context_parts.push(format!("Tool {}: {} ({})", tc.tool_name, status,
                        tc.output.as_ref().map(|o|
                            if o.len() > 200 { format!("{}...", &o[..200]) } else { o.clone() }
                        ).unwrap_or_default()
                    ));
                }
                SessionPart::Summary(s) => {
                    context_parts.push(format!("[Previous Summary]: {}", s.content));
                }
                _ => {}
            }
        }

        context_parts.join("\n\n")
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cd core && cargo test test_generate_llm_summary`
Expected: PASS (may need mock thinker)

**Step 5: Commit**

```bash
git add core/src/components/session_compactor.rs
git commit -m "feat(compactor): add LLM-driven summarization for context preservation"
```

---

## Task 5: Filter Compacted History Boundary Detection

**Files:**
- Modify: `core/src/components/session_compactor.rs`
- Modify: `core/src/components/types.rs` (add CompactionMarker)
- Test: In same file

**Step 1: Write failing test for filterCompacted**

```rust
#[test]
fn test_filter_compacted_creates_boundary() {
    let mut session = ExecutionSession::new();

    // Add some history before compaction
    session.parts.push(SessionPart::UserInput(UserInputPart {
        text: "Old request".to_string(),
        context: None,
        timestamp: 1000,
    }));

    // Add compaction marker
    session.parts.push(SessionPart::CompactionMarker(CompactionMarker {
        timestamp: 2000,
        auto: true,
    }));

    // Add summary
    session.parts.push(SessionPart::Summary(SummaryPart {
        content: "Summary of old context".to_string(),
        original_count: 5,
        compacted_at: 2000,
    }));

    // Add new history after compaction
    session.parts.push(SessionPart::UserInput(UserInputPart {
        text: "New request".to_string(),
        context: None,
        timestamp: 3000,
    }));

    let compactor = SessionCompactor::new();
    let filtered = compactor.filter_compacted(&session);

    // Should only return parts after the compaction boundary (summary + new)
    assert_eq!(filtered.len(), 2);
    assert!(matches!(filtered[0], SessionPart::Summary(_)));
    assert!(matches!(filtered[1], SessionPart::UserInput(_)));
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test test_filter_compacted_creates_boundary`
Expected: FAIL with "cannot find type `CompactionMarker`"

**Step 3: Add CompactionMarker to types**

```rust
// In components/types.rs, add to SessionPart enum:

/// Marker for compaction boundary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionMarker {
    /// When compaction occurred
    pub timestamp: i64,
    /// Whether this was automatic or user-triggered
    pub auto: bool,
}

// Update SessionPart enum:
pub enum SessionPart {
    UserInput(UserInputPart),
    AiResponse(AiResponsePart),
    ToolCall(ToolCallPart),
    Reasoning(ReasoningPart),
    PlanCreated(PlanPart),
    SubAgentCall(SubAgentPart),
    Summary(SummaryPart),
    CompactionMarker(CompactionMarker), // NEW
}
```

**Step 4: Implement filter_compacted**

```rust
impl SessionCompactor {
    /// Filter session parts to only include those after the last compaction boundary
    ///
    /// This matches OpenCode's filterCompacted() function:
    /// - Iterates backward through messages
    /// - Finds completed summary messages
    /// - Breaks at compaction markers
    /// - Returns only parts after the boundary
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
                    // Found the compaction marker after seeing its summary
                    // This is the boundary - stop collecting
                    break;
                }
                SessionPart::UserInput(_) if found_completed_summary => {
                    // Check if this user input has a compaction marker
                    // If the next part is a compaction marker, this is the boundary
                    result.push(part.clone());
                }
                _ => {
                    result.push(part.clone());
                }
            }
        }

        // Reverse to get chronological order
        result.reverse();
        result
    }

    /// Get messages for model, respecting compaction boundaries
    pub fn get_filtered_session(&self, session: &ExecutionSession) -> ExecutionSession {
        let mut filtered = session.clone();
        filtered.parts = self.filter_compacted(session);
        filtered
    }
}
```

**Step 5: Run test to verify it passes**

Run: `cd core && cargo test test_filter_compacted_creates_boundary`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/components/types.rs core/src/components/session_compactor.rs
git commit -m "feat(compactor): add filterCompacted boundary detection for context windowing"
```

---

## Task 6: Integrate with Agent Loop

**Files:**
- Modify: `core/src/agent_loop/mod.rs` (or main loop file)
- Modify: `core/src/components/session_compactor.rs` (activate EventHandler)
- Test: Integration test

**Step 1: Write failing integration test**

```rust
#[tokio::test]
async fn test_compactor_integration_with_loop() {
    use crate::event::{EventBus, LoopState};

    let bus = EventBus::new();
    let compactor = SessionCompactor::new();

    // Register compactor as handler
    bus.register_handler(Box::new(compactor.clone()));

    // Simulate loop state with high token count
    let loop_state = LoopState {
        session_id: "test-session".to_string(),
        iteration: 5,
        total_tokens: 150_000, // Above threshold
        last_tool: Some("search".to_string()),
    };

    // Publish event
    bus.publish(AetherEvent::LoopContinue(loop_state)).await;

    // Should trigger compaction check
    // (Full integration would check for SessionCompacted event)
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test test_compactor_integration_with_loop`
Expected: FAIL

**Step 3: Implement full EventHandler integration**

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
        ctx: &EventContext,
    ) -> Result<Vec<AetherEvent>, HandlerError> {
        // Check if auto-compaction is enabled
        if !self.config.auto_compact {
            return Ok(vec![]);
        }

        match event {
            AlephEvent::LoopContinue(loop_state) => {
                // Check if we need compaction based on token count
                let limit = self.token_tracker.get_model_limit(&loop_state.model);

                if loop_state.total_tokens >= limit.compaction_threshold() {
                    // Get session from context and compact
                    if let Some(session) = ctx.get_session(&loop_state.session_id).await {
                        let mut session = session.write().await;
                        if let Some(info) = self.check_and_compact(&mut session).await {
                            return Ok(vec![AetherEvent::SessionCompacted(info)]);
                        }
                    }
                }
                Ok(vec![])
            }
            AlephEvent::ToolCallCompleted(_) => {
                // Always prune after tool completion if enabled
                if self.config.prune_enabled {
                    // Trigger pruning (lightweight operation)
                }
                Ok(vec![])
            }
            _ => Ok(vec![]),
        }
    }
}
```

**Step 4: Update LoopState to include model info**

```rust
// In event/types.rs, update LoopState:
pub struct LoopState {
    pub session_id: String,
    pub iteration: u32,
    pub total_tokens: u64,
    pub last_tool: Option<String>,
    pub model: String, // Add model identifier
}
```

**Step 5: Run test to verify it passes**

Run: `cd core && cargo test test_compactor_integration_with_loop`
Expected: PASS

**Step 6: Commit**

```bash
git add core/src/components/session_compactor.rs core/src/event/types.rs
git commit -m "feat(compactor): activate EventHandler integration for real-time compaction"
```

---

## Task 7: Add Compaction Trigger Points to Agent Loop

**Files:**
- Modify: `core/src/agent_loop/mod.rs` (add compaction checks)
- Test: Integration test

**Step 1: Identify agent loop entry points**

Read the agent loop implementation to find where to add compaction checks:
- Before each agentic iteration (like OpenCode prompt.ts:498-511)
- After tool execution (like OpenCode result === "compact")
- At session end (like OpenCode SessionCompaction.prune line 627)

**Step 2: Add compaction check before iteration**

```rust
// In agent_loop/mod.rs, in the main loop:

// Before processing next iteration
if session.iteration_count > 0 {
    let tokens = EnhancedTokenUsage {
        input: session.total_input_tokens,
        output: session.total_output_tokens,
        cache_read: session.cache_tokens_read,
        ..Default::default()
    };

    if compactor.is_overflow_enhanced(&tokens, &session.model) {
        // Trigger compaction
        compactor.compact(&mut session);

        // Publish event
        event_bus.publish(AetherEvent::SessionCompacted(CompactionInfo {
            session_id: session.id.clone(),
            tokens_before: session.total_tokens,
            tokens_after: compactor.recalculate_tokens(&session),
            timestamp: chrono::Utc::now().timestamp(),
        })).await;
    }
}
```

**Step 3: Add prune at session end**

```rust
// At the end of the agent loop (when session completes):

// Always prune at session end (matches OpenCode)
if compactor.config.prune_enabled {
    compactor.prune_with_thresholds(&mut session);
}
```

**Step 4: Commit**

```bash
git add core/src/agent_loop/mod.rs
git commit -m "feat(agent-loop): add compaction trigger points for token control"
```

---

## Task 8: UI Callback for Compaction Events

**Files:**
- Modify: `core/src/components/callback_bridge.rs`
- Modify: `core/src/ffi/agent_loop_adapter.rs` (if needed)
- Test: In callback_bridge tests

**Step 1: Write failing test for compaction callback**

```rust
#[test]
fn test_compaction_callback() {
    let bridge = CallbackBridge::new();
    let mut received_events = Vec::new();

    bridge.set_callback(|event| {
        received_events.push(event);
    });

    bridge.notify_compaction(CompactionInfo {
        session_id: "test".to_string(),
        tokens_before: 150_000,
        tokens_after: 50_000,
        timestamp: 12345,
    });

    assert_eq!(received_events.len(), 1);
    assert!(matches!(received_events[0], CallbackEvent::SessionCompacted(_)));
}
```

**Step 2: Run test to verify it fails**

Run: `cd core && cargo test test_compaction_callback`
Expected: FAIL

**Step 3: Implement compaction callback**

```rust
// In callback_bridge.rs:

/// Events that can be sent to UI
#[derive(Debug, Clone)]
pub enum CallbackEvent {
    PartAdded(PartData),
    PartUpdated(PartUpdateData),
    PartRemoved(String),
    SessionCompacted(CompactionInfo), // NEW
}

impl CallbackBridge {
    /// Notify UI of compaction event
    pub fn notify_compaction(&self, info: CompactionInfo) {
        if let Some(callback) = &self.callback {
            callback(CallbackEvent::SessionCompacted(info));
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cd core && cargo test test_compaction_callback`
Expected: PASS

**Step 5: Commit**

```bash
git add core/src/components/callback_bridge.rs
git commit -m "feat(callback): add SessionCompacted event for UI notification"
```

---

## Task 9: Update Documentation

**Files:**
- Modify: `docs/AGENT_LOOP.md`
- Create: `docs/SESSION_COMPACTION.md`

**Step 1: Create session compaction documentation**

```markdown
# Session Compaction

## Overview

Session compaction is the mechanism for controlling token usage while preserving conversation context.
It implements a three-layer approach matching OpenCode's architecture.

## Configuration

```toml
[compaction]
auto_compact = true          # Enable automatic compaction
prune_enabled = true         # Enable tool output pruning
prune_minimum = 20000        # Minimum tokens before pruning
prune_protect = 40000        # Protect recent N tokens
protected_tools = ["skill"]  # Tools that are never pruned
```

## Compaction Flow

1. **Overflow Detection**: Before each iteration, check if tokens exceed threshold
2. **Pruning**: Remove old tool outputs (respecting protection rules)
3. **Summarization**: Generate LLM summary of context
4. **Boundary Creation**: Mark compaction point for history filtering

## Key Components

- `CompactionConfig`: Configuration for compaction behavior
- `EnhancedTokenUsage`: Cache-aware token tracking
- `SessionCompactor`: Main compaction logic
- `filter_compacted()`: History boundary detection

## Events

- `SessionCompacted`: Published when compaction occurs
- Contains: session_id, tokens_before, tokens_after, timestamp
```

**Step 2: Commit**

```bash
git add docs/SESSION_COMPACTION.md docs/AGENT_LOOP.md
git commit -m "docs: add session compaction documentation"
```

---

## Summary of Changes

| Task | Component | Description |
|------|-----------|-------------|
| 1 | CompactionConfig | Configurable compaction policies |
| 2 | EnhancedTokenUsage | Cache-aware token tracking |
| 3 | Smart Pruning | Protection mechanisms, thresholds |
| 4 | LLM Summarization | Context-aware summaries |
| 5 | filterCompacted | History boundary detection |
| 6 | EventHandler | Real-time compaction triggers |
| 7 | Agent Loop Integration | Compaction trigger points |
| 8 | UI Callback | Compaction event notifications |
| 9 | Documentation | Complete documentation |

## Execution

After implementation:
1. Run full test suite: `cd core && cargo test`
2. Build and verify: `./scripts/build-core.sh macos`
3. Manual testing with long conversations
