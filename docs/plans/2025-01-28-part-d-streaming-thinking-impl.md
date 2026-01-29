# Part D: Streaming Thinking - Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement thinking level configuration, streaming think block detection, and provider-agnostic streaming callbacks.

**Architecture:** Create a `thinking` module for level/mode definitions, and a `streaming` submodule for think block parsing and event handling. Provider adapters transform thinking levels into provider-specific request parameters.

**Tech Stack:** Rust, serde, regex, lazy_static

---

### Task 1: Create thinking module with level definitions

**Files:**
- Create: `core/src/thinking/mod.rs`
- Create: `core/src/thinking/level.rs`
- Modify: `core/src/lib.rs`

**Step 1: Create `core/src/thinking/mod.rs`**

```rust
//! Thinking level and reasoning mode definitions.
//!
//! Provides configuration for LLM thinking/reasoning:
//! - Six thinking levels (off/minimal/low/medium/high/xhigh)
//! - Three reasoning display modes (off/on/stream)
//! - Provider-specific support detection

pub mod level;

pub use level::{ReasoningMode, ThinkLevel};
```

**Step 2: Create `core/src/thinking/level.rs`**

```rust
//! Thinking level and reasoning mode definitions.

use serde::{Deserialize, Serialize};

/// Thinking level for LLM requests
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkLevel {
    /// Disable thinking
    #[default]
    Off,
    /// Minimal thinking (budget: 1024)
    Minimal,
    /// Low-level thinking (budget: 4096)
    Low,
    /// Medium-level thinking (budget: 8192)
    Medium,
    /// High-level thinking (budget: 16384)
    High,
    /// Extra-high thinking for reasoning models (budget: 32768)
    XHigh,
}

impl ThinkLevel {
    /// Parse thinking level from string
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "off" | "false" | "0" | "none" => Some(Self::Off),
            "on" | "enable" | "enabled" => Some(Self::Low),
            "min" | "minimal" => Some(Self::Minimal),
            "low" | "thinkhard" | "think-hard" => Some(Self::Low),
            "mid" | "med" | "medium" | "harder" => Some(Self::Medium),
            "high" | "ultra" | "max" => Some(Self::High),
            "xhigh" | "x-high" | "x_high" | "extra" => Some(Self::XHigh),
            _ => None,
        }
    }

    /// Get thinking budget in tokens
    pub fn budget_tokens(&self) -> u32 {
        match self {
            Self::Off => 0,
            Self::Minimal => 1024,
            Self::Low => 4096,
            Self::Medium => 8192,
            Self::High => 16384,
            Self::XHigh => 32768,
        }
    }

    /// Get available levels for a provider/model
    pub fn available_levels(provider: Option<&str>, model: Option<&str>) -> Vec<Self> {
        let mut levels = vec![Self::Off, Self::Minimal, Self::Low, Self::Medium, Self::High];

        if supports_xhigh(provider, model) {
            levels.push(Self::XHigh);
        }

        levels
    }

    /// Check if thinking is enabled
    pub fn is_enabled(&self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// Reasoning display mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningMode {
    /// Don't show reasoning
    #[default]
    Off,
    /// Show complete reasoning after completion
    On,
    /// Stream reasoning in real-time
    Stream,
}

impl ReasoningMode {
    /// Parse reasoning mode from string
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "off" | "false" | "0" | "none" | "hide" => Some(Self::Off),
            "on" | "true" | "1" | "show" | "complete" => Some(Self::On),
            "stream" | "streaming" | "realtime" | "live" => Some(Self::Stream),
            _ => None,
        }
    }

    /// Check if reasoning should be shown
    pub fn should_show(&self) -> bool {
        !matches!(self, Self::Off)
    }

    /// Check if reasoning should be streamed
    pub fn should_stream(&self) -> bool {
        matches!(self, Self::Stream)
    }
}

/// Check if provider/model supports xhigh thinking level
fn supports_xhigh(provider: Option<&str>, model: Option<&str>) -> bool {
    // Models known to support extended reasoning
    const XHIGH_MODELS: &[&str] = &[
        "o1", "o3", "gpt-5", // OpenAI reasoning models
        "gemini-2", // Google reasoning models
    ];

    model
        .map(|m| XHIGH_MODELS.iter().any(|x| m.contains(x)))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_think_level_default() {
        assert_eq!(ThinkLevel::default(), ThinkLevel::Off);
    }

    #[test]
    fn test_think_level_parse() {
        assert_eq!(ThinkLevel::parse("off"), Some(ThinkLevel::Off));
        assert_eq!(ThinkLevel::parse("minimal"), Some(ThinkLevel::Minimal));
        assert_eq!(ThinkLevel::parse("low"), Some(ThinkLevel::Low));
        assert_eq!(ThinkLevel::parse("medium"), Some(ThinkLevel::Medium));
        assert_eq!(ThinkLevel::parse("high"), Some(ThinkLevel::High));
        assert_eq!(ThinkLevel::parse("xhigh"), Some(ThinkLevel::XHigh));
        assert_eq!(ThinkLevel::parse("on"), Some(ThinkLevel::Low));
        assert_eq!(ThinkLevel::parse("invalid"), None);
    }

    #[test]
    fn test_think_level_budget() {
        assert_eq!(ThinkLevel::Off.budget_tokens(), 0);
        assert_eq!(ThinkLevel::Minimal.budget_tokens(), 1024);
        assert_eq!(ThinkLevel::Low.budget_tokens(), 4096);
        assert_eq!(ThinkLevel::Medium.budget_tokens(), 8192);
        assert_eq!(ThinkLevel::High.budget_tokens(), 16384);
        assert_eq!(ThinkLevel::XHigh.budget_tokens(), 32768);
    }

    #[test]
    fn test_think_level_is_enabled() {
        assert!(!ThinkLevel::Off.is_enabled());
        assert!(ThinkLevel::Low.is_enabled());
        assert!(ThinkLevel::High.is_enabled());
    }

    #[test]
    fn test_reasoning_mode_default() {
        assert_eq!(ReasoningMode::default(), ReasoningMode::Off);
    }

    #[test]
    fn test_reasoning_mode_parse() {
        assert_eq!(ReasoningMode::parse("off"), Some(ReasoningMode::Off));
        assert_eq!(ReasoningMode::parse("on"), Some(ReasoningMode::On));
        assert_eq!(ReasoningMode::parse("stream"), Some(ReasoningMode::Stream));
        assert_eq!(ReasoningMode::parse("invalid"), None);
    }

    #[test]
    fn test_reasoning_mode_should_show() {
        assert!(!ReasoningMode::Off.should_show());
        assert!(ReasoningMode::On.should_show());
        assert!(ReasoningMode::Stream.should_show());
    }

    #[test]
    fn test_reasoning_mode_should_stream() {
        assert!(!ReasoningMode::Off.should_stream());
        assert!(!ReasoningMode::On.should_stream());
        assert!(ReasoningMode::Stream.should_stream());
    }

    #[test]
    fn test_available_levels() {
        let levels = ThinkLevel::available_levels(None, None);
        assert_eq!(levels.len(), 5); // No xhigh without reasoning model

        let levels = ThinkLevel::available_levels(Some("openai"), Some("o1-preview"));
        assert_eq!(levels.len(), 6); // Includes xhigh
    }
}
```

**Step 3: Add thinking module to lib.rs**

Find `pub mod exec;` in `core/src/lib.rs` and add after it:

```rust
pub mod thinking; // Thinking level and reasoning mode
```

**Step 4: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo check 2>&1 | head -30`

**Step 5: Commit**

```bash
git add core/src/thinking/mod.rs core/src/thinking/level.rs core/src/lib.rs
git commit -m "thinking: add level and reasoning mode definitions"
```

---

### Task 2: Create streaming module with events

**Files:**
- Create: `core/src/thinking/streaming/mod.rs`
- Create: `core/src/thinking/streaming/events.rs`
- Modify: `core/src/thinking/mod.rs`

**Step 1: Create `core/src/thinking/streaming/mod.rs`**

```rust
//! Streaming thinking block processing.
//!
//! Provides:
//! - Stream event definitions
//! - Thinking tag detection and parsing
//! - Callback-based stream subscription

pub mod events;

pub use events::{StreamEvent, TokenUsage};
```

**Step 2: Create `core/src/thinking/streaming/events.rs`**

```rust
//! Stream event definitions.

use serde::{Deserialize, Serialize};

/// Events emitted during streaming
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Assistant message started
    AssistantStart {
        message_index: u32,
    },

    /// Text content delta
    TextDelta {
        delta: String,
        accumulated: String,
    },

    /// Thinking content delta
    ThinkingDelta {
        delta: String,
        accumulated: String,
    },

    /// Thinking block completed
    ThinkingComplete {
        content: String,
    },

    /// Tool execution started
    ToolStart {
        tool_id: String,
        tool_name: String,
    },

    /// Tool execution completed
    ToolComplete {
        tool_id: String,
        result: serde_json::Value,
    },

    /// Block reply (for TTS/chunked output)
    BlockReply {
        text: String,
        is_final: bool,
    },

    /// Assistant message completed
    AssistantComplete {
        content: String,
        thinking: Option<String>,
        usage: Option<TokenUsage>,
    },

    /// Error occurred
    Error {
        message: String,
        recoverable: bool,
    },
}

/// Token usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Input tokens consumed
    pub input_tokens: u32,

    /// Output tokens generated
    pub output_tokens: u32,

    /// Thinking tokens used (if available)
    pub thinking_tokens: Option<u32>,

    /// Tokens read from cache
    pub cache_read_tokens: Option<u32>,

    /// Tokens written to cache
    pub cache_write_tokens: Option<u32>,
}

impl TokenUsage {
    /// Create new usage stats
    pub fn new(input: u32, output: u32) -> Self {
        Self {
            input_tokens: input,
            output_tokens: output,
            ..Default::default()
        }
    }

    /// Set thinking tokens
    pub fn with_thinking(mut self, tokens: u32) -> Self {
        self.thinking_tokens = Some(tokens);
        self
    }

    /// Total tokens used
    pub fn total(&self) -> u32 {
        self.input_tokens + self.output_tokens + self.thinking_tokens.unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_event_serialize() {
        let event = StreamEvent::TextDelta {
            delta: "Hello".into(),
            accumulated: "Hello".into(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"text_delta""#));
        assert!(json.contains(r#""delta":"Hello""#));
    }

    #[test]
    fn test_thinking_delta_serialize() {
        let event = StreamEvent::ThinkingDelta {
            delta: "Let me think...".into(),
            accumulated: "Let me think...".into(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"thinking_delta""#));
    }

    #[test]
    fn test_token_usage() {
        let usage = TokenUsage::new(100, 50).with_thinking(25);

        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.thinking_tokens, Some(25));
        assert_eq!(usage.total(), 175);
    }

    #[test]
    fn test_assistant_complete_serialize() {
        let event = StreamEvent::AssistantComplete {
            content: "Hello!".into(),
            thinking: Some("I should greet the user.".into()),
            usage: Some(TokenUsage::new(10, 5)),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"assistant_complete""#));
        assert!(json.contains(r#""thinking""#));
    }
}
```

**Step 3: Update thinking/mod.rs**

```rust
//! Thinking level and reasoning mode definitions.
//!
//! Provides configuration for LLM thinking/reasoning:
//! - Six thinking levels (off/minimal/low/medium/high/xhigh)
//! - Three reasoning display modes (off/on/stream)
//! - Provider-specific support detection
//! - Streaming event definitions

pub mod level;
pub mod streaming;

pub use level::{ReasoningMode, ThinkLevel};
pub use streaming::{StreamEvent, TokenUsage};
```

**Step 4: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test --lib thinking 2>&1 | tail -30`

**Step 5: Commit**

```bash
git add core/src/thinking/streaming/mod.rs core/src/thinking/streaming/events.rs core/src/thinking/mod.rs
git commit -m "thinking: add streaming event definitions"
```

---

### Task 3: Implement inline code state tracking

**Files:**
- Create: `core/src/thinking/streaming/inline_code.rs`
- Modify: `core/src/thinking/streaming/mod.rs`

**Step 1: Create `core/src/thinking/streaming/inline_code.rs`**

```rust
//! Inline code state tracking.
//!
//! Tracks backtick state to avoid misdetecting thinking tags inside code blocks.

/// State for tracking inline code blocks
#[derive(Debug, Clone, Default)]
pub struct InlineCodeState {
    /// Currently inside backticks
    pub in_backtick: bool,

    /// Number of consecutive backticks (for fenced blocks)
    pub backtick_count: usize,

    /// Pending backtick characters
    pending_backticks: usize,
}

impl InlineCodeState {
    /// Create new state
    pub fn new() -> Self {
        Self::default()
    }

    /// Update state based on character
    pub fn update(&mut self, ch: char) {
        match ch {
            '`' => {
                self.pending_backticks += 1;
            }
            _ => {
                if self.pending_backticks > 0 {
                    self.process_backticks();
                }
                self.pending_backticks = 0;
            }
        }
    }

    /// Update state based on string chunk
    pub fn update_chunk(&mut self, chunk: &str) {
        for ch in chunk.chars() {
            self.update(ch);
        }
        // Finalize any pending backticks at end of chunk
        if self.pending_backticks > 0 {
            self.process_backticks();
            self.pending_backticks = 0;
        }
    }

    /// Process pending backtick sequence
    fn process_backticks(&mut self) {
        let count = self.pending_backticks;

        if self.in_backtick {
            // We're inside a code block, check if this closes it
            if count >= self.backtick_count {
                self.in_backtick = false;
                self.backtick_count = 0;
            }
        } else {
            // Start a new code block
            self.in_backtick = true;
            self.backtick_count = count;
        }
    }

    /// Check if currently inside code block
    pub fn is_in_code(&self) -> bool {
        self.in_backtick
    }

    /// Reset state
    pub fn reset(&mut self) {
        self.in_backtick = false;
        self.backtick_count = 0;
        self.pending_backticks = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_backtick() {
        let mut state = InlineCodeState::new();

        state.update_chunk("Hello `code");
        assert!(state.is_in_code());

        state.update_chunk("` world");
        assert!(!state.is_in_code());
    }

    #[test]
    fn test_triple_backtick() {
        let mut state = InlineCodeState::new();

        state.update_chunk("```rust\nlet x = 1;\n");
        assert!(state.is_in_code());

        state.update_chunk("```\n");
        assert!(!state.is_in_code());
    }

    #[test]
    fn test_nested_backticks() {
        let mut state = InlineCodeState::new();

        // Triple backtick block containing single backticks
        state.update_chunk("```\nuse `trait`\n```");
        assert!(!state.is_in_code());
    }

    #[test]
    fn test_no_code() {
        let mut state = InlineCodeState::new();

        state.update_chunk("Hello world <think>test</think>");
        assert!(!state.is_in_code());
    }

    #[test]
    fn test_reset() {
        let mut state = InlineCodeState::new();

        state.update_chunk("```code");
        assert!(state.is_in_code());

        state.reset();
        assert!(!state.is_in_code());
    }
}
```

**Step 2: Update streaming/mod.rs**

```rust
//! Streaming thinking block processing.
//!
//! Provides:
//! - Stream event definitions
//! - Thinking tag detection and parsing
//! - Callback-based stream subscription

pub mod events;
pub mod inline_code;

pub use events::{StreamEvent, TokenUsage};
pub use inline_code::InlineCodeState;
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test --lib thinking 2>&1 | tail -30`

**Step 4: Commit**

```bash
git add core/src/thinking/streaming/inline_code.rs core/src/thinking/streaming/mod.rs
git commit -m "thinking: add inline code state tracking"
```

---

### Task 4: Implement thinking tag parser (block_state)

**Files:**
- Create: `core/src/thinking/streaming/block_state.rs`
- Modify: `core/src/thinking/streaming/mod.rs`

**Step 1: Create `core/src/thinking/streaming/block_state.rs`**

```rust
//! Thinking block state machine.
//!
//! Detects and extracts content from thinking tags during streaming.

use super::inline_code::InlineCodeState;
use once_cell::sync::Lazy;
use regex::Regex;

/// Regex for thinking tags: <think>, </think>, <thinking>, </thinking>, etc.
static THINKING_TAG_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"<\s*(/?)\s*(?:think(?:ing)?|thought|antthinking)\s*>").unwrap()
});

/// Regex for final tags: <final>, </final>
static FINAL_TAG_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<\s*(/?)\s*final\s*>").unwrap());

/// State machine for tracking thinking blocks
#[derive(Debug, Clone, Default)]
pub struct BlockState {
    /// Currently inside a thinking block
    pub in_thinking: bool,

    /// Currently inside a final answer block
    pub in_final: bool,

    /// Inline code tracking
    pub inline_code: InlineCodeState,
}

/// Result of processing a delta
#[derive(Debug, Clone, Default)]
pub struct ProcessedDelta {
    /// Content from thinking block
    pub thinking: Option<String>,

    /// Content from final answer block
    pub final_answer: Option<String>,

    /// Regular content
    pub regular: Option<String>,
}

impl ProcessedDelta {
    /// Check if delta has any content
    pub fn is_empty(&self) -> bool {
        self.thinking.is_none() && self.final_answer.is_none() && self.regular.is_none()
    }
}

impl BlockState {
    /// Create new block state
    pub fn new() -> Self {
        Self::default()
    }

    /// Process a content delta and categorize it
    pub fn process_delta(&mut self, delta: &str) -> ProcessedDelta {
        let mut thinking_content = String::new();
        let mut final_content = String::new();
        let mut regular_content = String::new();

        let mut remaining = delta;

        while !remaining.is_empty() {
            // Update inline code state
            self.inline_code.update_chunk(remaining);

            // Don't parse tags inside code blocks
            if self.inline_code.is_in_code() {
                if self.in_thinking {
                    thinking_content.push_str(remaining);
                } else if self.in_final {
                    final_content.push_str(remaining);
                } else {
                    regular_content.push_str(remaining);
                }
                break;
            }

            // Try to find thinking tag
            if let Some(m) = THINKING_TAG_RE.find(remaining) {
                let before = &remaining[..m.start()];
                let tag = m.as_str();
                remaining = &remaining[m.end()..];

                // Add content before tag to appropriate buffer
                self.append_content(before, &mut thinking_content, &mut final_content, &mut regular_content);

                // Toggle thinking state
                let is_closing = tag.contains('/');
                self.in_thinking = !is_closing;
                continue;
            }

            // Try to find final tag
            if let Some(m) = FINAL_TAG_RE.find(remaining) {
                let before = &remaining[..m.start()];
                let tag = m.as_str();
                remaining = &remaining[m.end()..];

                // Add content before tag to appropriate buffer
                self.append_content(before, &mut thinking_content, &mut final_content, &mut regular_content);

                // Toggle final state
                let is_closing = tag.contains('/');
                self.in_final = !is_closing;
                continue;
            }

            // No tags found, add all remaining content
            self.append_content(remaining, &mut thinking_content, &mut final_content, &mut regular_content);
            break;
        }

        ProcessedDelta {
            thinking: if thinking_content.is_empty() { None } else { Some(thinking_content) },
            final_answer: if final_content.is_empty() { None } else { Some(final_content) },
            regular: if regular_content.is_empty() { None } else { Some(regular_content) },
        }
    }

    /// Append content to appropriate buffer based on current state
    fn append_content(
        &self,
        content: &str,
        thinking: &mut String,
        final_answer: &mut String,
        regular: &mut String,
    ) {
        if content.is_empty() {
            return;
        }

        if self.in_thinking {
            thinking.push_str(content);
        } else if self.in_final {
            final_answer.push_str(content);
        } else {
            regular.push_str(content);
        }
    }

    /// Reset state
    pub fn reset(&mut self) {
        self.in_thinking = false;
        self.in_final = false;
        self.inline_code.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_tags() {
        let mut state = BlockState::new();
        let result = state.process_delta("Hello world");

        assert_eq!(result.regular, Some("Hello world".to_string()));
        assert!(result.thinking.is_none());
        assert!(result.final_answer.is_none());
    }

    #[test]
    fn test_thinking_block() {
        let mut state = BlockState::new();

        let result = state.process_delta("<think>Let me think");
        assert_eq!(result.thinking, Some("Let me think".to_string()));
        assert!(state.in_thinking);

        let result = state.process_delta(" about this</think>");
        assert_eq!(result.thinking, Some(" about this".to_string()));
        assert!(!state.in_thinking);
    }

    #[test]
    fn test_thinking_with_whitespace() {
        let mut state = BlockState::new();

        let result = state.process_delta("< think >content</ think >");
        assert_eq!(result.thinking, Some("content".to_string()));
        assert!(!state.in_thinking);
    }

    #[test]
    fn test_antthinking_tag() {
        let mut state = BlockState::new();

        let result = state.process_delta("<antthinking>reasoning</antthinking>");
        assert_eq!(result.thinking, Some("reasoning".to_string()));
    }

    #[test]
    fn test_thought_tag() {
        let mut state = BlockState::new();

        let result = state.process_delta("<thought>hmm</thought>");
        assert_eq!(result.thinking, Some("hmm".to_string()));
    }

    #[test]
    fn test_final_block() {
        let mut state = BlockState::new();

        let result = state.process_delta("<final>Answer</final>");
        assert_eq!(result.final_answer, Some("Answer".to_string()));
    }

    #[test]
    fn test_mixed_content() {
        let mut state = BlockState::new();

        let result = state.process_delta("Start <think>thinking</think> end");
        assert_eq!(result.regular, Some("Start  end".to_string()));
        assert_eq!(result.thinking, Some("thinking".to_string()));
    }

    #[test]
    fn test_tag_in_code_block() {
        let mut state = BlockState::new();

        let result = state.process_delta("`<think>not a tag</think>`");
        assert_eq!(result.regular, Some("`<think>not a tag</think>`".to_string()));
        assert!(result.thinking.is_none());
    }

    #[test]
    fn test_streaming_chunks() {
        let mut state = BlockState::new();

        // Simulate streaming in chunks
        let r1 = state.process_delta("Hello ");
        assert_eq!(r1.regular, Some("Hello ".to_string()));

        let r2 = state.process_delta("<think>I need to");
        assert_eq!(r2.thinking, Some("I need to".to_string()));
        assert!(state.in_thinking);

        let r3 = state.process_delta(" think</think> done");
        assert_eq!(r3.thinking, Some(" think".to_string()));
        assert_eq!(r3.regular, Some(" done".to_string()));
        assert!(!state.in_thinking);
    }
}
```

**Step 2: Update streaming/mod.rs**

```rust
//! Streaming thinking block processing.
//!
//! Provides:
//! - Stream event definitions
//! - Thinking tag detection and parsing
//! - Callback-based stream subscription

pub mod block_state;
pub mod events;
pub mod inline_code;

pub use block_state::{BlockState, ProcessedDelta};
pub use events::{StreamEvent, TokenUsage};
pub use inline_code::InlineCodeState;
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test --lib thinking 2>&1 | tail -40`

**Step 4: Commit**

```bash
git add core/src/thinking/streaming/block_state.rs core/src/thinking/streaming/mod.rs
git commit -m "thinking: add thinking tag parser state machine"
```

---

### Task 5: Implement block reply chunker

**Files:**
- Create: `core/src/thinking/streaming/chunker.rs`
- Modify: `core/src/thinking/streaming/mod.rs`

**Step 1: Create `core/src/thinking/streaming/chunker.rs`**

```rust
//! Block reply chunking for TTS and streaming output.

use serde::{Deserialize, Serialize};

/// Break mode for block replies
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockReplyBreak {
    /// Break at end of text only
    #[default]
    TextEnd,
    /// Break at paragraph boundaries
    Paragraph,
    /// Break at sentence boundaries
    Sentence,
}

/// Chunker for block reply output
#[derive(Debug, Clone)]
pub struct BlockChunker {
    break_mode: BlockReplyBreak,
    buffer: String,
    min_chunk_size: usize,
}

impl BlockChunker {
    /// Create new chunker
    pub fn new(break_mode: BlockReplyBreak) -> Self {
        Self {
            break_mode,
            buffer: String::new(),
            min_chunk_size: 10,
        }
    }

    /// Set minimum chunk size
    pub fn with_min_size(mut self, size: usize) -> Self {
        self.min_chunk_size = size;
        self
    }

    /// Add text and get any complete chunks
    pub fn add(&mut self, text: &str) -> Vec<String> {
        self.buffer.push_str(text);

        match self.break_mode {
            BlockReplyBreak::TextEnd => vec![],
            BlockReplyBreak::Paragraph => self.extract_paragraphs(),
            BlockReplyBreak::Sentence => self.extract_sentences(),
        }
    }

    /// Flush any remaining content
    pub fn flush(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.buffer))
        }
    }

    /// Extract complete paragraphs
    fn extract_paragraphs(&mut self) -> Vec<String> {
        let mut chunks = Vec::new();

        while let Some(pos) = self.buffer.find("\n\n") {
            let chunk = self.buffer[..pos].trim().to_string();
            if !chunk.is_empty() && chunk.len() >= self.min_chunk_size {
                chunks.push(chunk);
            }
            self.buffer = self.buffer[pos + 2..].to_string();
        }

        chunks
    }

    /// Extract complete sentences
    fn extract_sentences(&mut self) -> Vec<String> {
        let mut chunks = Vec::new();

        // Simple sentence detection: ., !, ? followed by space or end
        let sentence_ends = [". ", "! ", "? ", ".\n", "!\n", "?\n"];

        loop {
            let mut found_pos = None;

            for end in &sentence_ends {
                if let Some(pos) = self.buffer.find(end) {
                    let end_pos = pos + end.len();
                    if found_pos.map(|p| end_pos < p).unwrap_or(true) {
                        found_pos = Some(end_pos);
                    }
                }
            }

            match found_pos {
                Some(pos) if pos >= self.min_chunk_size => {
                    let chunk = self.buffer[..pos].trim().to_string();
                    if !chunk.is_empty() {
                        chunks.push(chunk);
                    }
                    self.buffer = self.buffer[pos..].trim_start().to_string();
                }
                _ => break,
            }
        }

        chunks
    }

    /// Check if buffer is empty
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_end_mode() {
        let mut chunker = BlockChunker::new(BlockReplyBreak::TextEnd);

        assert!(chunker.add("Hello world. ").is_empty());
        assert!(chunker.add("More text.\n\n").is_empty());

        let final_chunk = chunker.flush();
        assert_eq!(final_chunk, Some("Hello world. More text.\n\n".to_string()));
    }

    #[test]
    fn test_paragraph_mode() {
        let mut chunker = BlockChunker::new(BlockReplyBreak::Paragraph)
            .with_min_size(5);

        let chunks = chunker.add("First paragraph.\n\nSecond paragraph.\n\n");
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], "First paragraph.");
        assert_eq!(chunks[1], "Second paragraph.");
    }

    #[test]
    fn test_sentence_mode() {
        let mut chunker = BlockChunker::new(BlockReplyBreak::Sentence)
            .with_min_size(5);

        let chunks = chunker.add("First sentence. Second sentence! Third? ");
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], "First sentence.");
        assert_eq!(chunks[1], "Second sentence!");
        assert_eq!(chunks[2], "Third?");
    }

    #[test]
    fn test_min_chunk_size() {
        let mut chunker = BlockChunker::new(BlockReplyBreak::Sentence)
            .with_min_size(20);

        // Short sentences should be buffered
        let chunks = chunker.add("Hi. ");
        assert!(chunks.is_empty());

        let chunks = chunker.add("This is a longer sentence. ");
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn test_streaming_paragraphs() {
        let mut chunker = BlockChunker::new(BlockReplyBreak::Paragraph);

        chunker.add("Start of para");
        assert!(chunker.add("graph one").is_empty());

        let chunks = chunker.add(".\n\nParagraph two.\n\n");
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn test_flush() {
        let mut chunker = BlockChunker::new(BlockReplyBreak::Paragraph);

        chunker.add("Incomplete paragraph");
        assert_eq!(chunker.flush(), Some("Incomplete paragraph".to_string()));
        assert!(chunker.is_empty());
    }
}
```

**Step 2: Update streaming/mod.rs**

```rust
//! Streaming thinking block processing.
//!
//! Provides:
//! - Stream event definitions
//! - Thinking tag detection and parsing
//! - Block reply chunking
//! - Callback-based stream subscription

pub mod block_state;
pub mod chunker;
pub mod events;
pub mod inline_code;

pub use block_state::{BlockState, ProcessedDelta};
pub use chunker::{BlockChunker, BlockReplyBreak};
pub use events::{StreamEvent, TokenUsage};
pub use inline_code::InlineCodeState;
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test --lib thinking 2>&1 | tail -40`

**Step 4: Commit**

```bash
git add core/src/thinking/streaming/chunker.rs core/src/thinking/streaming/mod.rs
git commit -m "thinking: add block reply chunker"
```

---

### Task 6: Implement stream subscriber with callbacks

**Files:**
- Create: `core/src/thinking/streaming/subscriber.rs`
- Modify: `core/src/thinking/streaming/mod.rs`

**Step 1: Create `core/src/thinking/streaming/subscriber.rs`**

```rust
//! Stream subscription with callbacks.

use super::block_state::BlockState;
use super::chunker::{BlockChunker, BlockReplyBreak};
use super::events::{StreamEvent, TokenUsage};
use crate::thinking::level::ReasoningMode;
use std::sync::Arc;

/// Configuration for stream subscriber
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// How to display reasoning
    pub reasoning_mode: ReasoningMode,

    /// How to break block replies
    pub block_reply_break: BlockReplyBreak,

    /// Minimum chunk size for block replies
    pub min_chunk_size: usize,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            reasoning_mode: ReasoningMode::Off,
            block_reply_break: BlockReplyBreak::TextEnd,
            min_chunk_size: 10,
        }
    }
}

/// Internal state for stream processing
#[derive(Debug, Default)]
struct StreamState {
    text_buffer: String,
    thinking_buffer: String,
    message_index: u32,
}

/// Callback type aliases
pub type TextCallback = Arc<dyn Fn(&str) + Send + Sync>;
pub type ThinkingCallback = Arc<dyn Fn(&str) + Send + Sync>;
pub type BlockReplyCallback = Arc<dyn Fn(&str, bool) + Send + Sync>;
pub type CompleteCallback = Arc<dyn Fn(&str, Option<&str>, Option<&TokenUsage>) + Send + Sync>;
pub type ErrorCallback = Arc<dyn Fn(&str, bool) + Send + Sync>;

/// Callbacks for stream events
#[derive(Default, Clone)]
pub struct StreamCallbacks {
    pub on_text_delta: Option<TextCallback>,
    pub on_thinking_delta: Option<ThinkingCallback>,
    pub on_thinking_complete: Option<ThinkingCallback>,
    pub on_block_reply: Option<BlockReplyCallback>,
    pub on_complete: Option<CompleteCallback>,
    pub on_error: Option<ErrorCallback>,
}

/// Stream subscriber that processes provider events
pub struct StreamSubscriber {
    config: StreamConfig,
    state: StreamState,
    block_state: BlockState,
    chunker: BlockChunker,
    callbacks: StreamCallbacks,
}

impl StreamSubscriber {
    /// Create new subscriber
    pub fn new(config: StreamConfig, callbacks: StreamCallbacks) -> Self {
        let chunker = BlockChunker::new(config.block_reply_break)
            .with_min_size(config.min_chunk_size);

        Self {
            config,
            state: StreamState::default(),
            block_state: BlockState::default(),
            chunker,
            callbacks,
        }
    }

    /// Process a content delta from provider
    pub fn process_content_delta(&mut self, delta: &str) {
        let processed = self.block_state.process_delta(delta);

        // Handle thinking content
        if let Some(thinking) = processed.thinking {
            self.state.thinking_buffer.push_str(&thinking);

            if self.config.reasoning_mode == ReasoningMode::Stream {
                if let Some(cb) = &self.callbacks.on_thinking_delta {
                    cb(&thinking);
                }
            }
        }

        // Handle regular content
        if let Some(regular) = processed.regular {
            self.state.text_buffer.push_str(&regular);

            if let Some(cb) = &self.callbacks.on_text_delta {
                cb(&regular);
            }

            // Emit block replies if configured
            let chunks = self.chunker.add(&regular);
            if let Some(cb) = &self.callbacks.on_block_reply {
                for chunk in chunks {
                    cb(&chunk, false);
                }
            }
        }
    }

    /// Signal message end
    pub fn process_message_end(&mut self, usage: Option<TokenUsage>) {
        // Emit thinking complete if there was thinking content
        if !self.state.thinking_buffer.is_empty() {
            if self.config.reasoning_mode != ReasoningMode::Off {
                if let Some(cb) = &self.callbacks.on_thinking_complete {
                    cb(&self.state.thinking_buffer);
                }
            }
        }

        // Flush remaining content as final block reply
        if let Some(remaining) = self.chunker.flush() {
            if let Some(cb) = &self.callbacks.on_block_reply {
                cb(&remaining, true);
            }
        }

        // Emit complete callback
        if let Some(cb) = &self.callbacks.on_complete {
            let thinking = if self.state.thinking_buffer.is_empty() {
                None
            } else {
                Some(self.state.thinking_buffer.as_str())
            };
            cb(&self.state.text_buffer, thinking, usage.as_ref());
        }
    }

    /// Process an error
    pub fn process_error(&mut self, message: &str, recoverable: bool) {
        if let Some(cb) = &self.callbacks.on_error {
            cb(message, recoverable);
        }
    }

    /// Get accumulated text
    pub fn text(&self) -> &str {
        &self.state.text_buffer
    }

    /// Get accumulated thinking
    pub fn thinking(&self) -> &str {
        &self.state.thinking_buffer
    }

    /// Reset subscriber state
    pub fn reset(&mut self) {
        self.state = StreamState::default();
        self.block_state.reset();
    }

    /// Build stream event for current state
    pub fn build_complete_event(&self, usage: Option<TokenUsage>) -> StreamEvent {
        StreamEvent::AssistantComplete {
            content: self.state.text_buffer.clone(),
            thinking: if self.state.thinking_buffer.is_empty() {
                None
            } else {
                Some(self.state.thinking_buffer.clone())
            },
            usage,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_basic_text_streaming() {
        let text_count = Arc::new(AtomicUsize::new(0));
        let text_count_clone = text_count.clone();

        let callbacks = StreamCallbacks {
            on_text_delta: Some(Arc::new(move |_| {
                text_count_clone.fetch_add(1, Ordering::SeqCst);
            })),
            ..Default::default()
        };

        let mut subscriber = StreamSubscriber::new(StreamConfig::default(), callbacks);

        subscriber.process_content_delta("Hello ");
        subscriber.process_content_delta("world!");

        assert_eq!(text_count.load(Ordering::SeqCst), 2);
        assert_eq!(subscriber.text(), "Hello world!");
    }

    #[test]
    fn test_thinking_extraction() {
        let thinking_count = Arc::new(AtomicUsize::new(0));
        let thinking_count_clone = thinking_count.clone();

        let config = StreamConfig {
            reasoning_mode: ReasoningMode::Stream,
            ..Default::default()
        };

        let callbacks = StreamCallbacks {
            on_thinking_delta: Some(Arc::new(move |_| {
                thinking_count_clone.fetch_add(1, Ordering::SeqCst);
            })),
            ..Default::default()
        };

        let mut subscriber = StreamSubscriber::new(config, callbacks);

        subscriber.process_content_delta("<think>Let me ");
        subscriber.process_content_delta("think</think>Answer");

        assert_eq!(thinking_count.load(Ordering::SeqCst), 2);
        assert_eq!(subscriber.thinking(), "Let me think");
        assert_eq!(subscriber.text(), "Answer");
    }

    #[test]
    fn test_block_reply_paragraphs() {
        let block_count = Arc::new(AtomicUsize::new(0));
        let block_count_clone = block_count.clone();

        let config = StreamConfig {
            block_reply_break: BlockReplyBreak::Paragraph,
            min_chunk_size: 5,
            ..Default::default()
        };

        let callbacks = StreamCallbacks {
            on_block_reply: Some(Arc::new(move |_, _| {
                block_count_clone.fetch_add(1, Ordering::SeqCst);
            })),
            ..Default::default()
        };

        let mut subscriber = StreamSubscriber::new(config, callbacks);

        subscriber.process_content_delta("Para one.\n\nPara two.\n\n");
        subscriber.process_message_end(None);

        assert_eq!(block_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_complete_callback() {
        let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let completed_clone = completed.clone();

        let callbacks = StreamCallbacks {
            on_complete: Some(Arc::new(move |_, _, _| {
                completed_clone.store(true, Ordering::SeqCst);
            })),
            ..Default::default()
        };

        let mut subscriber = StreamSubscriber::new(StreamConfig::default(), callbacks);

        subscriber.process_content_delta("Done");
        subscriber.process_message_end(Some(TokenUsage::new(10, 5)));

        assert!(completed.load(Ordering::SeqCst));
    }

    #[test]
    fn test_build_complete_event() {
        let mut subscriber = StreamSubscriber::new(
            StreamConfig::default(),
            StreamCallbacks::default(),
        );

        subscriber.process_content_delta("<think>thinking</think>answer");

        let event = subscriber.build_complete_event(Some(TokenUsage::new(10, 5)));

        match event {
            StreamEvent::AssistantComplete { content, thinking, usage } => {
                assert_eq!(content, "answer");
                assert_eq!(thinking, Some("thinking".to_string()));
                assert!(usage.is_some());
            }
            _ => panic!("Expected AssistantComplete event"),
        }
    }
}
```

**Step 2: Update streaming/mod.rs**

```rust
//! Streaming thinking block processing.
//!
//! Provides:
//! - Stream event definitions
//! - Thinking tag detection and parsing
//! - Block reply chunking
//! - Callback-based stream subscription

pub mod block_state;
pub mod chunker;
pub mod events;
pub mod inline_code;
pub mod subscriber;

pub use block_state::{BlockState, ProcessedDelta};
pub use chunker::{BlockChunker, BlockReplyBreak};
pub use events::{StreamEvent, TokenUsage};
pub use inline_code::InlineCodeState;
pub use subscriber::{StreamCallbacks, StreamConfig, StreamSubscriber};
```

**Step 3: Run tests**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test --lib thinking 2>&1 | tail -40`

**Step 4: Commit**

```bash
git add core/src/thinking/streaming/subscriber.rs core/src/thinking/streaming/mod.rs
git commit -m "thinking: add stream subscriber with callbacks"
```

---

### Task 7: Full test pass and module exports

**Files:**
- Modify: `core/src/thinking/mod.rs`
- Modify: `core/src/lib.rs`

**Step 1: Update thinking/mod.rs with full exports**

```rust
//! Thinking level and reasoning mode definitions.
//!
//! Provides configuration for LLM thinking/reasoning:
//! - Six thinking levels (off/minimal/low/medium/high/xhigh)
//! - Three reasoning display modes (off/on/stream)
//! - Provider-specific support detection
//! - Streaming event definitions
//! - Thinking tag detection and parsing
//! - Callback-based stream subscription

pub mod level;
pub mod streaming;

// Level exports
pub use level::{ReasoningMode, ThinkLevel};

// Streaming exports
pub use streaming::{
    BlockChunker, BlockReplyBreak, BlockState, InlineCodeState, ProcessedDelta,
    StreamCallbacks, StreamConfig, StreamEvent, StreamSubscriber, TokenUsage,
};
```

**Step 2: Run full test suite**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo test --lib thinking 2>&1 | tail -50`

**Step 3: Add exports to lib.rs**

Find the exec exports section and add after it:

```rust
// Thinking level and streaming exports
pub use crate::thinking::{
    // Level
    ThinkLevel, ReasoningMode,
    // Streaming events
    StreamEvent, TokenUsage,
    // Block state
    BlockState, ProcessedDelta, InlineCodeState,
    // Chunker
    BlockChunker, BlockReplyBreak,
    // Subscriber
    StreamSubscriber, StreamConfig, StreamCallbacks,
};
```

**Step 4: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aether/core && cargo check 2>&1 | tail -20`

**Step 5: Final commit**

```bash
git add core/src/thinking/mod.rs core/src/lib.rs
git commit -m "thinking: export streaming types from lib.rs"
```

---

## Summary

| Task | Description | Files | Tests |
|------|-------------|-------|-------|
| 1 | Level definitions | `thinking/mod.rs`, `level.rs`, `lib.rs` | 10 |
| 2 | Stream events | `streaming/mod.rs`, `events.rs` | 4 |
| 3 | Inline code state | `streaming/inline_code.rs` | 5 |
| 4 | Block state parser | `streaming/block_state.rs` | 10 |
| 5 | Block chunker | `streaming/chunker.rs` | 6 |
| 6 | Stream subscriber | `streaming/subscriber.rs` | 5 |
| 7 | Full test + exports | `thinking/mod.rs`, `lib.rs` | - |

**Note:** This implementation provides the type definitions and streaming infrastructure. Provider adapters (Anthropic, OpenAI, Google) will be implemented when the provider client modules are created.
