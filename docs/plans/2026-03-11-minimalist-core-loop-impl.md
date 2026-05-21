# Minimalist Core Loop Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace Aleph's 5-layer OTAF agent loop with a Claude Code-inspired 2-step `think → act` loop, removing ~150k LOC of middleware that duplicates LLM reasoning capabilities.

**Architecture:** New `MinimalAgentLoop` built alongside existing `AgentLoop`, switchable via config flag. Uses native tool calling (`ProviderResponse.tool_calls`), flat `ToolRegistry`, single-layer `SafetyGuard`, and unified `PromptBuilder`. All "intelligence" moves from middleware code into system prompt.

**Tech Stack:** Rust, tokio, serde_json, async-trait. Reuses existing `AiProvider`, `ProviderResponse`, `RequestPayload`, `SoulManifest`, `SessionManager`, `HybridMemoryStore`, `McpTransport`, `ExtensionRuntime`.

**Principle:** Don't replace what the LLM is good at. Amplify what the LLM can't do alone.

---

## Phase 1: New Core Loop (Parallel Build)

### Task 1.1: Define MinimalTool Trait

**Files:**
- Create: `src/agent_loop/minimal/mod.rs`
- Create: `src/agent_loop/minimal/tool.rs`
- Test: `src/agent_loop/minimal/tool_tests.rs`

**Context:**
Current system has two traits (`AlephTool` static + `AlephToolDyn` dynamic) plus `CapabilityStrategy`. We unify to one simple trait.

Existing `AlephToolDyn` in `src/tools/traits.rs:187-198`:
```rust
pub trait AlephToolDyn: Send + Sync {
    fn name(&self) -> &str;
    fn definition(&self) -> ToolDefinition;
    fn call(&self, args: Value) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>>;
}
```

Existing `ToolDefinition` in `src/tools/traits.rs` — already has `name`, `description`, `parameters` (JsonSchema). We reuse this.

**Step 1: Write the failing test**

```rust
// src/agent_loop/minimal/tool_tests.rs
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct EchoTool;

    #[async_trait::async_trait]
    impl MinimalTool for EchoTool {
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "Echoes input" }
        fn schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"]
            })
        }
        async fn execute(&self, input: serde_json::Value) -> ToolResult {
            let text = input["text"].as_str().unwrap_or("").to_string();
            ToolResult::Success { output: json!({ "echo": text }) }
        }
    }

    #[tokio::test]
    async fn test_minimal_tool_execute() {
        let tool = EchoTool;
        let result = tool.execute(json!({ "text": "hello" })).await;
        match result {
            ToolResult::Success { output } => {
                assert_eq!(output["echo"], "hello");
            }
            _ => panic!("Expected success"),
        }
    }

    #[tokio::test]
    async fn test_minimal_tool_registry() {
        let mut registry = MinimalToolRegistry::new();
        registry.register(Box::new(EchoTool));

        assert_eq!(registry.len(), 1);
        assert!(registry.get("echo").is_some());
        assert!(registry.get("nonexistent").is_none());

        let result = registry.execute("echo", &json!({ "text": "world" })).await;
        match result {
            ToolResult::Success { output } => assert_eq!(output["echo"], "world"),
            _ => panic!("Expected success"),
        }

        let result = registry.execute("nonexistent", &json!({})).await;
        match result {
            ToolResult::Error { error, .. } => assert!(error.contains("Unknown tool")),
            _ => panic!("Expected error"),
        }
    }

    #[tokio::test]
    async fn test_registry_schemas() {
        let mut registry = MinimalToolRegistry::new();
        registry.register(Box::new(EchoTool));
        let schemas = registry.tool_definitions();
        assert_eq!(schemas.len(), 1);
        assert_eq!(schemas[0].name, "echo");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agent_loop::minimal::tool_tests -- -v`
Expected: FAIL with module not found

**Step 3: Write minimal implementation**

```rust
// src/agent_loop/minimal/mod.rs
pub mod tool;
#[cfg(test)]
mod tool_tests;

pub use tool::{MinimalTool, MinimalToolRegistry, ToolResult};
```

```rust
// src/agent_loop/minimal/tool.rs
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use crate::tools::traits::ToolDefinition;

/// Unified result type for tool execution
#[derive(Debug, Clone)]
pub enum ToolResult {
    Success { output: Value },
    Error { error: String, retryable: bool },
}

/// The ONE trait for all tools in the system.
/// Builtin, MCP, Extension, Skill — all implement this.
#[async_trait]
pub trait MinimalTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> Value;
    async fn execute(&self, input: Value) -> ToolResult;
}

/// Flat tool registry. No dispatcher, no cortex, no filters.
pub struct MinimalToolRegistry {
    tools: HashMap<String, Box<dyn MinimalTool>>,
}

impl MinimalToolRegistry {
    pub fn new() -> Self {
        Self { tools: HashMap::new() }
    }

    pub fn register(&mut self, tool: Box<dyn MinimalTool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<&dyn MinimalTool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub async fn execute(&self, name: &str, input: &Value) -> ToolResult {
        match self.tools.get(name) {
            Some(tool) => tool.execute(input.clone()).await,
            None => ToolResult::Error {
                error: format!("Unknown tool: {name}"),
                retryable: false,
            },
        }
    }

    /// Generate ToolDefinition list for LLM native tool calling
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| {
            ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.schema(),
            }
        }).collect()
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib agent_loop::minimal::tool_tests -- -v`
Expected: 3 tests PASS

**Step 5: Commit**

```bash
git add src/agent_loop/minimal/
git commit -m "minimal-loop: add MinimalTool trait and flat ToolRegistry"
```

---

### Task 1.2: Define SafetyGuard

**Files:**
- Create: `src/agent_loop/minimal/safety.rs`
- Create: `src/agent_loop/minimal/safety_tests.rs`

**Context:**
Current system has 3 filter layers (`ToolFilter` + `SmartFilter` + `ProfileFilter`) + `ToolSafetyLevel` enum + `ToolConfirmation` system. We collapse to one struct with pattern-based blocking + confirmation set.

Existing `ToolSafetyLevel` in `src/dispatcher/types/safety.rs` and keyword patterns in `src/config/types/policies/tool_safety.rs` provide the safety classification. We keep it simple: blocked patterns (regex) + confirmation-required tool names.

**Step 1: Write the failing test**

```rust
// src/agent_loop/minimal/safety_tests.rs
#[cfg(test)]
mod tests {
    use super::super::safety::*;
    use serde_json::json;

    #[test]
    fn test_blocked_pattern() {
        let guard = SafetyGuard::new(
            vec!["rm\\s+-rf\\s+/".to_string()],
            vec![],
        );
        let call = ToolCall { name: "shell".into(), input: json!({ "command": "rm -rf /" }) };
        assert!(matches!(guard.check(&call), Err(SafetyError::Blocked { .. })));
    }

    #[test]
    fn test_allowed_tool() {
        let guard = SafetyGuard::new(vec![], vec![]);
        let call = ToolCall { name: "search".into(), input: json!({ "query": "hello" }) };
        assert!(guard.check(&call).is_ok());
    }

    #[test]
    fn test_confirmation_required() {
        let guard = SafetyGuard::new(
            vec![],
            vec!["shell".to_string(), "file_write".to_string()],
        );
        let call = ToolCall { name: "shell".into(), input: json!({}) };
        assert!(matches!(guard.check(&call), Err(SafetyError::NeedsConfirmation { .. })));

        let call2 = ToolCall { name: "search".into(), input: json!({}) };
        assert!(guard.check(&call2).is_ok());
    }

    #[test]
    fn test_blocked_takes_priority_over_confirmation() {
        let guard = SafetyGuard::new(
            vec!["drop\\s+database".to_string()],
            vec!["shell".to_string()],
        );
        let call = ToolCall { name: "shell".into(), input: json!({ "command": "drop database prod" }) };
        assert!(matches!(guard.check(&call), Err(SafetyError::Blocked { .. })));
    }

    #[test]
    fn test_default_guard_has_sensible_defaults() {
        let guard = SafetyGuard::default_guard();
        // Should block dangerous patterns
        let call = ToolCall { name: "shell".into(), input: json!({ "command": "rm -rf /" }) };
        assert!(matches!(guard.check(&call), Err(SafetyError::Blocked { .. })));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agent_loop::minimal::safety_tests -- -v`
Expected: FAIL

**Step 3: Write minimal implementation**

```rust
// src/agent_loop/minimal/safety.rs
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub input: Value,
}

#[derive(Debug)]
pub enum SafetyError {
    Blocked { tool: String, pattern: String },
    NeedsConfirmation { tool: String },
}

impl std::fmt::Display for SafetyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blocked { tool, pattern } => write!(f, "Tool '{tool}' blocked by safety pattern: {pattern}"),
            Self::NeedsConfirmation { tool } => write!(f, "Tool '{tool}' requires user confirmation"),
        }
    }
}

impl std::error::Error for SafetyError {}

pub struct SafetyGuard {
    blocked_patterns: Vec<Regex>,
    confirmation_required: HashSet<String>,
}

impl SafetyGuard {
    pub fn new(blocked: Vec<String>, confirmation: Vec<String>) -> Self {
        let blocked_patterns = blocked.iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();
        let confirmation_required = confirmation.into_iter().collect();
        Self { blocked_patterns, confirmation_required }
    }

    /// Default safety guard with sensible defaults
    pub fn default_guard() -> Self {
        Self::new(
            vec![
                r"rm\s+-rf\s+/".to_string(),
                r"drop\s+database".to_string(),
                r"mkfs\.".to_string(),
                r"dd\s+if=.*of=/dev/".to_string(),
                r">\s*/dev/sd".to_string(),
            ],
            vec![
                "shell".to_string(),
                "file_write".to_string(),
                "file_delete".to_string(),
            ],
        )
    }

    pub fn check(&self, call: &ToolCall) -> Result<(), SafetyError> {
        // Serialize input for pattern matching
        let input_str = call.input.to_string();
        let check_str = format!("{} {}", call.name, input_str);

        // 1. Hard block (not bypassable)
        for pattern in &self.blocked_patterns {
            if pattern.is_match(&check_str) {
                return Err(SafetyError::Blocked {
                    tool: call.name.clone(),
                    pattern: pattern.as_str().to_string(),
                });
            }
        }

        // 2. Confirmation required
        if self.confirmation_required.contains(&call.name) {
            return Err(SafetyError::NeedsConfirmation {
                tool: call.name.clone(),
            });
        }

        Ok(())
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib agent_loop::minimal::safety_tests -- -v`
Expected: 5 tests PASS

**Step 5: Update mod.rs and commit**

Add to `src/agent_loop/minimal/mod.rs`:
```rust
pub mod safety;
#[cfg(test)]
mod safety_tests;

pub use safety::{SafetyGuard, SafetyError, ToolCall as SafetyToolCall};
```

```bash
git add src/agent_loop/minimal/
git commit -m "minimal-loop: add single-layer SafetyGuard"
```

---

### Task 1.3: Build MinimalPromptBuilder

**Files:**
- Create: `src/agent_loop/minimal/prompt_builder.rs`
- Create: `src/agent_loop/minimal/prompt_builder_tests.rs`

**Context:**
Current `PromptBuilder` in `src/thinker/prompt_builder/mod.rs` has 10+ build methods and a `PromptPipeline` with layered composition. We replace with a single `build()` that assembles: soul + rules + memory summary + session context.

Existing `SoulManifest` in `src/thinker/soul.rs:105-143` is reused as-is.

**Step 1: Write the failing test**

```rust
// src/agent_loop/minimal/prompt_builder_tests.rs
#[cfg(test)]
mod tests {
    use super::super::prompt_builder::*;

    #[test]
    fn test_build_includes_soul() {
        let builder = MinimalPromptBuilder::new()
            .with_soul_identity("I am Aleph, your personal AI assistant.")
            .with_soul_tone("friendly and concise");
        let prompt = builder.build(&[], None);
        assert!(prompt.contains("Aleph"));
        assert!(prompt.contains("friendly"));
    }

    #[test]
    fn test_build_includes_tool_rules() {
        let builder = MinimalPromptBuilder::new()
            .with_capability_rules("Never execute shell commands without user approval.");
        let prompt = builder.build(&[], None);
        assert!(prompt.contains("Never execute shell"));
    }

    #[test]
    fn test_build_includes_memory_context() {
        let builder = MinimalPromptBuilder::new();
        let prompt = builder.build(&[], Some("User prefers Chinese dialogue."));
        assert!(prompt.contains("Chinese dialogue"));
    }

    #[test]
    fn test_build_includes_tool_descriptions() {
        let tools = vec![
            ToolInfo { name: "search".into(), description: "Search the web".into() },
            ToolInfo { name: "memory".into(), description: "Query memory store".into() },
        ];
        let builder = MinimalPromptBuilder::new();
        let prompt = builder.build(&tools, None);
        assert!(prompt.contains("search"));
        assert!(prompt.contains("memory"));
    }

    #[test]
    fn test_build_empty_is_valid() {
        let builder = MinimalPromptBuilder::new();
        let prompt = builder.build(&[], None);
        // Should still produce a valid prompt with base instructions
        assert!(!prompt.is_empty());
        assert!(prompt.contains("assistant")); // base instruction mention
    }

    #[test]
    fn test_custom_instructions() {
        let builder = MinimalPromptBuilder::new()
            .with_custom_instructions("Always reply in haiku format.");
        let prompt = builder.build(&[], None);
        assert!(prompt.contains("haiku"));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agent_loop::minimal::prompt_builder_tests -- -v`
Expected: FAIL

**Step 3: Write minimal implementation**

```rust
// src/agent_loop/minimal/prompt_builder.rs

/// Lightweight tool info for prompt building (no full schema needed in prompt)
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

/// Builds the system prompt by assembling sections.
/// All "intelligence" that was previously in middleware code lives here as text.
pub struct MinimalPromptBuilder {
    soul_identity: Option<String>,
    soul_tone: Option<String>,
    soul_directives: Vec<String>,
    capability_rules: Option<String>,
    custom_instructions: Option<String>,
}

impl MinimalPromptBuilder {
    pub fn new() -> Self {
        Self {
            soul_identity: None,
            soul_tone: None,
            soul_directives: Vec::new(),
            capability_rules: None,
            custom_instructions: None,
        }
    }

    pub fn with_soul_identity(mut self, identity: &str) -> Self {
        self.soul_identity = Some(identity.to_string());
        self
    }

    pub fn with_soul_tone(mut self, tone: &str) -> Self {
        self.soul_tone = Some(tone.to_string());
        self
    }

    pub fn with_soul_directive(mut self, directive: &str) -> Self {
        self.soul_directives.push(directive.to_string());
        self
    }

    pub fn with_capability_rules(mut self, rules: &str) -> Self {
        self.capability_rules = Some(rules.to_string());
        self
    }

    pub fn with_custom_instructions(mut self, instructions: &str) -> Self {
        self.custom_instructions = Some(instructions.to_string());
        self
    }

    /// Build the complete system prompt.
    /// Tool schemas go via native tool calling (RequestPayload.tools),
    /// but tool descriptions are included in prompt for context.
    pub fn build(&self, tools: &[ToolInfo], memory_context: Option<&str>) -> String {
        let mut sections = Vec::new();

        // Section 1: Identity
        if let Some(identity) = &self.soul_identity {
            sections.push(format!("# Identity\n\n{identity}"));
        } else {
            sections.push("# Identity\n\nYou are a helpful personal AI assistant.".to_string());
        }

        // Section 2: Tone & Style
        if let Some(tone) = &self.soul_tone {
            sections.push(format!("# Communication Style\n\nTone: {tone}"));
        }

        // Section 3: Directives
        if !self.soul_directives.is_empty() {
            let directives = self.soul_directives.iter()
                .map(|d| format!("- {d}"))
                .collect::<Vec<_>>()
                .join("\n");
            sections.push(format!("# Directives\n\n{directives}"));
        }

        // Section 4: Capability rules (replaces triple filter)
        if let Some(rules) = &self.capability_rules {
            sections.push(format!("# Tool Usage Rules\n\n{rules}"));
        }

        // Section 5: Available tools summary
        if !tools.is_empty() {
            let tool_list = tools.iter()
                .map(|t| format!("- **{}**: {}", t.name, t.description))
                .collect::<Vec<_>>()
                .join("\n");
            sections.push(format!("# Available Tools\n\n{tool_list}"));
        }

        // Section 6: Memory context
        if let Some(memory) = memory_context {
            sections.push(format!("# Context from Memory\n\n{memory}"));
        }

        // Section 7: Custom instructions
        if let Some(custom) = &self.custom_instructions {
            sections.push(format!("# Additional Instructions\n\n{custom}"));
        }

        // Section 8: Base behavioral instructions
        sections.push(
            "# Behavior\n\n\
            - Use tools to accomplish tasks. Call tools when needed, don't just describe what you would do.\n\
            - Continue working until the task is fully complete. Don't stop prematurely.\n\
            - If a tool call fails, analyze the error and try a different approach.\n\
            - When the task is complete, provide a concise summary of what was done."
            .to_string()
        );

        sections.join("\n\n---\n\n")
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib agent_loop::minimal::prompt_builder_tests -- -v`
Expected: 6 tests PASS

**Step 5: Update mod.rs and commit**

Add to `src/agent_loop/minimal/mod.rs`:
```rust
pub mod prompt_builder;
#[cfg(test)]
mod prompt_builder_tests;

pub use prompt_builder::{MinimalPromptBuilder, ToolInfo};
```

```bash
git add src/agent_loop/minimal/
git commit -m "minimal-loop: add MinimalPromptBuilder with section-based assembly"
```

---

### Task 1.4: Build MinimalAgentLoop Core

**Files:**
- Create: `src/agent_loop/minimal/loop_core.rs`
- Create: `src/agent_loop/minimal/loop_core_tests.rs`

**Context:**
Current `AgentLoop::run()` in `src/agent_loop/agent_loop.rs:352-1199` is ~850 lines with OTAF phases, guards, POE hooks, swarm coordination, escalation checks. The new loop is ~100 lines: `think → act` with safety guard and compression.

Key types to reuse:
- `AiProvider::process_with_payload(RequestPayload)` → `ProviderResponse` (from `src/providers/adapter.rs`)
- `ProviderResponse { text, tool_calls, thinking, stop_reason, usage }`
- `NativeToolCall { tool_name, arguments }`
- `StopReason::EndTurn | ToolUse | MaxTokens`
- `ToolDefinition { name, description, parameters }`

**Step 1: Write the failing test**

```rust
// src/agent_loop/minimal/loop_core_tests.rs
#[cfg(test)]
mod tests {
    use super::super::loop_core::*;
    use super::super::tool::*;
    use super::super::safety::*;
    use super::super::prompt_builder::*;
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use crate::providers::adapter::{ProviderResponse, NativeToolCall, StopReason, TokenUsage};

    // Mock provider that returns predetermined responses
    struct MockProvider {
        responses: std::sync::Mutex<Vec<ProviderResponse>>,
    }

    impl MockProvider {
        fn new(responses: Vec<ProviderResponse>) -> Self {
            Self { responses: std::sync::Mutex::new(responses) }
        }
    }

    #[async_trait]
    impl MinimalProvider for MockProvider {
        async fn call(
            &self,
            _messages: &[LoopMessage],
            _system_prompt: &str,
            _tools: &[crate::tools::traits::ToolDefinition],
        ) -> anyhow::Result<ProviderResponse> {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                Ok(ProviderResponse {
                    text: Some("Done.".into()),
                    tool_calls: vec![],
                    thinking: None,
                    stop_reason: StopReason::EndTurn,
                    usage: None,
                })
            } else {
                Ok(responses.remove(0))
            }
        }
    }

    // Simple test tool
    struct AddTool;
    #[async_trait]
    impl MinimalTool for AddTool {
        fn name(&self) -> &str { "add" }
        fn description(&self) -> &str { "Adds two numbers" }
        fn schema(&self) -> Value { json!({"type": "object", "properties": {"a": {"type": "number"}, "b": {"type": "number"}}}) }
        async fn execute(&self, input: Value) -> ToolResult {
            let a = input["a"].as_f64().unwrap_or(0.0);
            let b = input["b"].as_f64().unwrap_or(0.0);
            ToolResult::Success { output: json!({ "sum": a + b }) }
        }
    }

    fn make_loop(
        provider: MockProvider,
        tools: Vec<Box<dyn MinimalTool>>,
    ) -> MinimalAgentLoop<MockProvider> {
        let mut registry = MinimalToolRegistry::new();
        for tool in tools {
            registry.register(tool);
        }
        MinimalAgentLoop::new(
            provider,
            registry,
            MinimalPromptBuilder::new(),
            SafetyGuard::new(vec![], vec![]),
            LoopConfig { max_iterations: 10, token_budget: 100000, timeout_secs: 60 },
        )
    }

    #[tokio::test]
    async fn test_simple_text_response() {
        let provider = MockProvider::new(vec![
            ProviderResponse {
                text: Some("Hello!".into()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);
        let agent = make_loop(provider, vec![]);
        let result = agent.run("Hi", &mut NoopCallback).await.unwrap();
        assert_eq!(result.final_text, Some("Hello!".into()));
        assert_eq!(result.iterations, 1);
    }

    #[tokio::test]
    async fn test_tool_call_then_response() {
        let provider = MockProvider::new(vec![
            // First call: LLM decides to use tool
            ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: "call_1".into(),
                    tool_name: "add".into(),
                    arguments: json!({ "a": 2, "b": 3 }),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            // Second call: LLM sees tool result, responds
            ProviderResponse {
                text: Some("The sum is 5.".into()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);
        let agent = make_loop(provider, vec![Box::new(AddTool)]);
        let result = agent.run("What is 2 + 3?", &mut NoopCallback).await.unwrap();
        assert_eq!(result.final_text, Some("The sum is 5.".into()));
        assert_eq!(result.iterations, 2);
        assert_eq!(result.tool_calls_made, 1);
    }

    #[tokio::test]
    async fn test_max_iterations_guard() {
        // Provider always requests tool calls - should hit max_iterations
        let responses: Vec<_> = (0..15).map(|i| ProviderResponse {
            text: None,
            tool_calls: vec![NativeToolCall {
                id: format!("call_{i}"),
                tool_name: "add".into(),
                arguments: json!({ "a": 1, "b": 1 }),
            }],
            thinking: None,
            stop_reason: StopReason::ToolUse,
            usage: None,
        }).collect();
        let provider = MockProvider::new(responses);
        let agent = make_loop(provider, vec![Box::new(AddTool)]);
        let result = agent.run("Loop forever", &mut NoopCallback).await.unwrap();
        assert_eq!(result.iterations, 10); // max_iterations = 10
        assert!(result.hit_limit);
    }

    #[tokio::test]
    async fn test_safety_guard_blocks_tool() {
        let provider = MockProvider::new(vec![
            ProviderResponse {
                text: None,
                tool_calls: vec![NativeToolCall {
                    id: "call_1".into(),
                    tool_name: "dangerous".into(),
                    arguments: json!({ "command": "rm -rf /" }),
                }],
                thinking: None,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
            // After blocked tool, LLM should get error and respond
            ProviderResponse {
                text: Some("I can't do that.".into()),
                tool_calls: vec![],
                thinking: None,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ]);

        let mut registry = MinimalToolRegistry::new();
        let agent = MinimalAgentLoop::new(
            provider,
            registry,
            MinimalPromptBuilder::new(),
            SafetyGuard::new(vec!["rm\\s+-rf\\s+/".into()], vec![]),
            LoopConfig { max_iterations: 10, token_budget: 100000, timeout_secs: 60 },
        );
        let result = agent.run("Delete everything", &mut NoopCallback).await.unwrap();
        // Should complete after LLM receives the safety error
        assert_eq!(result.final_text, Some("I can't do that.".into()));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agent_loop::minimal::loop_core_tests -- -v`
Expected: FAIL

**Step 3: Write minimal implementation**

```rust
// src/agent_loop/minimal/loop_core.rs
use std::time::Instant;
use async_trait::async_trait;
use serde_json::Value;

use crate::tools::traits::ToolDefinition;
use crate::providers::adapter::{ProviderResponse, StopReason};

use super::tool::{MinimalToolRegistry, ToolResult};
use super::safety::{SafetyGuard, SafetyError, ToolCall as SafetyToolCall};
use super::prompt_builder::{MinimalPromptBuilder, ToolInfo};

/// Abstraction over AI provider for testability
#[async_trait]
pub trait MinimalProvider: Send + Sync {
    async fn call(
        &self,
        messages: &[LoopMessage],
        system_prompt: &str,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<ProviderResponse>;
}

/// Messages in the conversation
#[derive(Debug, Clone)]
pub enum LoopMessage {
    User(String),
    Assistant(String),
    ToolUse { id: String, name: String, input: Value },
    ToolResult { id: String, output: Value, is_error: bool },
}

/// Loop configuration — simple, flat
pub struct LoopConfig {
    pub max_iterations: usize,
    pub token_budget: usize,
    pub timeout_secs: u64,
}

/// Result of a loop run
#[derive(Debug)]
pub struct LoopRunResult {
    pub final_text: Option<String>,
    pub iterations: usize,
    pub tool_calls_made: usize,
    pub total_tokens: usize,
    pub hit_limit: bool,
}

/// Callback for streaming and events
pub trait LoopCallback: Send {
    fn on_text(&mut self, _text: &str) {}
    fn on_tool_start(&mut self, _name: &str, _input: &Value) {}
    fn on_tool_done(&mut self, _name: &str, _result: &ToolResult) {}
    fn on_safety_block(&mut self, _error: &SafetyError) {}
}

/// No-op callback for testing
pub struct NoopCallback;
impl LoopCallback for NoopCallback {}

/// The minimal agent loop. Think → Act. That's it.
pub struct MinimalAgentLoop<P: MinimalProvider> {
    provider: P,
    tool_registry: MinimalToolRegistry,
    prompt_builder: MinimalPromptBuilder,
    safety_guard: SafetyGuard,
    config: LoopConfig,
}

impl<P: MinimalProvider> MinimalAgentLoop<P> {
    pub fn new(
        provider: P,
        tool_registry: MinimalToolRegistry,
        prompt_builder: MinimalPromptBuilder,
        safety_guard: SafetyGuard,
        config: LoopConfig,
    ) -> Self {
        Self { provider, tool_registry, prompt_builder, safety_guard, config }
    }

    pub async fn run(
        &self,
        input: &str,
        callback: &mut dyn LoopCallback,
    ) -> anyhow::Result<LoopRunResult> {
        let started = Instant::now();
        let mut messages = vec![LoopMessage::User(input.to_string())];
        let mut total_tokens: usize = 0;
        let mut tool_calls_made: usize = 0;
        let mut final_text: Option<String> = None;
        let mut hit_limit = false;

        // Build system prompt once (tool descriptions for context)
        let tool_infos: Vec<ToolInfo> = self.tool_registry.tool_definitions()
            .iter()
            .map(|td| ToolInfo { name: td.name.clone(), description: td.description.clone() })
            .collect();
        let system_prompt = self.prompt_builder.build(&tool_infos, None);

        // Tool schemas for native tool calling
        let tool_defs = self.tool_registry.tool_definitions();

        for iteration in 0..self.config.max_iterations {
            // Timeout check
            if started.elapsed().as_secs() > self.config.timeout_secs {
                hit_limit = true;
                break;
            }

            // 1. THINK — one LLM call, all reasoning happens here
            let response = self.provider.call(&messages, &system_prompt, &tool_defs).await?;

            // Track tokens
            if let Some(usage) = &response.usage {
                total_tokens += (usage.input_tokens + usage.output_tokens) as usize;
            }

            // Handle text output
            if let Some(text) = &response.text {
                if !text.is_empty() {
                    callback.on_text(text);
                    final_text = Some(text.clone());
                }
            }

            // 2. ACT — execute tool calls if any
            if response.tool_calls.is_empty() {
                // No tools, check stop reason
                if matches!(response.stop_reason, StopReason::EndTurn) {
                    break; // Task complete
                }
                // MaxTokens or other — add assistant message and continue
                if let Some(text) = &response.text {
                    messages.push(LoopMessage::Assistant(text.clone()));
                }
                if iteration + 1 >= self.config.max_iterations {
                    hit_limit = true;
                }
                continue;
            }

            // Process each tool call
            for tool_call in &response.tool_calls {
                // Add tool_use message
                messages.push(LoopMessage::ToolUse {
                    id: tool_call.id.clone(),
                    name: tool_call.tool_name.clone(),
                    input: tool_call.arguments.clone(),
                });

                // Safety check
                let safety_call = SafetyToolCall {
                    name: tool_call.tool_name.clone(),
                    input: tool_call.arguments.clone(),
                };
                match self.safety_guard.check(&safety_call) {
                    Err(SafetyError::Blocked { tool, pattern }) => {
                        callback.on_safety_block(&SafetyError::Blocked { tool: tool.clone(), pattern: pattern.clone() });
                        messages.push(LoopMessage::ToolResult {
                            id: tool_call.id.clone(),
                            output: serde_json::json!({
                                "error": format!("BLOCKED: Tool '{tool}' is not allowed (matched safety pattern: {pattern})")
                            }),
                            is_error: true,
                        });
                        continue;
                    }
                    Err(SafetyError::NeedsConfirmation { tool }) => {
                        // TODO: Wire to actual confirmation UI
                        // For now, treat as error
                        callback.on_safety_block(&SafetyError::NeedsConfirmation { tool: tool.clone() });
                        messages.push(LoopMessage::ToolResult {
                            id: tool_call.id.clone(),
                            output: serde_json::json!({
                                "error": format!("Tool '{tool}' requires user confirmation (not yet approved)")
                            }),
                            is_error: true,
                        });
                        continue;
                    }
                    Ok(()) => {}
                }

                // Execute tool
                callback.on_tool_start(&tool_call.tool_name, &tool_call.arguments);
                let result = self.tool_registry.execute(&tool_call.tool_name, &tool_call.arguments).await;
                callback.on_tool_done(&tool_call.tool_name, &result);
                tool_calls_made += 1;

                // Add result to messages
                let (output, is_error) = match &result {
                    ToolResult::Success { output } => (output.clone(), false),
                    ToolResult::Error { error, .. } => (serde_json::json!({ "error": error }), true),
                };
                messages.push(LoopMessage::ToolResult {
                    id: tool_call.id.clone(),
                    output,
                    is_error,
                });
            }

            // Check if this was the last iteration
            if iteration + 1 >= self.config.max_iterations {
                hit_limit = true;
            }

            // Token budget check
            if total_tokens > self.config.token_budget {
                hit_limit = true;
                break;
            }
        }

        Ok(LoopRunResult {
            final_text,
            iterations: std::cmp::min(
                messages.iter().filter(|m| matches!(m, LoopMessage::User(_))).count()
                    + messages.iter().filter(|m| matches!(m, LoopMessage::ToolResult { .. })).count(),
                self.config.max_iterations,
            ),
            tool_calls_made,
            total_tokens,
            hit_limit,
        })
    }
}
```

> **Note to implementor:** The `iterations` counting in the return value is a placeholder. The actual iteration counting should be tracked with a simple counter in the loop. Adjust the implementation to use a `let mut iteration_count = 0` counter incremented at the start of each loop iteration.

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib agent_loop::minimal::loop_core_tests -- -v`
Expected: 4 tests PASS

**Step 5: Update mod.rs and commit**

Add to `src/agent_loop/minimal/mod.rs`:
```rust
pub mod loop_core;
#[cfg(test)]
mod loop_core_tests;

pub use loop_core::{MinimalAgentLoop, MinimalProvider, LoopMessage, LoopConfig, LoopRunResult, LoopCallback, NoopCallback};
```

```bash
git add src/agent_loop/minimal/
git commit -m "minimal-loop: add MinimalAgentLoop core — think → act two-step loop"
```

---

### Task 1.5: Wire MinimalAgentLoop Module into Core

**Files:**
- Modify: `src/agent_loop/mod.rs` — add `pub mod minimal;`
- Modify: `src/lib.rs` — ensure agent_loop module re-exports minimal

**Step 1: Add module declaration**

In `src/agent_loop/mod.rs`, add:
```rust
pub mod minimal;
```

**Step 2: Verify compilation**

Run: `cargo check -p alephcore`
Expected: Compiles with no errors

**Step 3: Run all minimal tests**

Run: `cargo test -p alephcore --lib agent_loop::minimal -- -v`
Expected: All tests (3 + 5 + 6 + 4 = 18) PASS

**Step 4: Commit**

```bash
git add src/agent_loop/mod.rs
git commit -m "minimal-loop: wire minimal module into agent_loop"
```

---

## Phase 2: Tool Adapter Layer

### Task 2.1: Adapter from AlephToolDyn to MinimalTool

**Files:**
- Create: `src/agent_loop/minimal/adapters/mod.rs`
- Create: `src/agent_loop/minimal/adapters/builtin_adapter.rs`
- Test: inline in same file

**Context:**
Existing builtin tools implement `AlephToolDyn` (or `AlephTool` which blanket-impls `AlephToolDyn`). We need an adapter that wraps `Arc<dyn AlephToolDyn>` as a `MinimalTool`.

`AlephToolDyn` is in `src/tools/traits.rs:187-198`:
```rust
pub trait AlephToolDyn: Send + Sync {
    fn name(&self) -> &str;
    fn definition(&self) -> ToolDefinition;
    fn call(&self, args: Value) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + '_>>;
}
```

**Step 1-3: Write adapter**

```rust
// src/agent_loop/minimal/adapters/builtin_adapter.rs
use std::sync::Arc;
use async_trait::async_trait;
use serde_json::Value;

use crate::tools::traits::AlephToolDyn;
use super::super::tool::{MinimalTool, ToolResult};

/// Wraps an existing AlephToolDyn as a MinimalTool
pub struct BuiltinToolAdapter {
    inner: Arc<dyn AlephToolDyn>,
}

impl BuiltinToolAdapter {
    pub fn new(inner: Arc<dyn AlephToolDyn>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl MinimalTool for BuiltinToolAdapter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        // ToolDefinition has description field
        // We can't return &str from a computed value, so use a workaround
        // In practice, we'll store the definition
        "" // Will be overridden by BuiltinToolAdapterOwned
    }

    fn schema(&self) -> Value {
        self.inner.definition().parameters.clone()
    }

    async fn execute(&self, input: Value) -> ToolResult {
        match self.inner.call(input).await {
            Ok(output) => ToolResult::Success { output },
            Err(e) => ToolResult::Error {
                error: e.to_string(),
                retryable: true,
            },
        }
    }
}

/// Owned adapter that caches name/description for lifetime safety
pub struct BuiltinToolAdapterOwned {
    inner: Arc<dyn AlephToolDyn>,
    cached_name: String,
    cached_description: String,
    cached_schema: Value,
}

impl BuiltinToolAdapterOwned {
    pub fn new(inner: Arc<dyn AlephToolDyn>) -> Self {
        let def = inner.definition();
        Self {
            cached_name: def.name.clone(),
            cached_description: def.description.clone(),
            cached_schema: def.parameters.clone(),
            inner,
        }
    }
}

#[async_trait]
impl MinimalTool for BuiltinToolAdapterOwned {
    fn name(&self) -> &str { &self.cached_name }
    fn description(&self) -> &str { &self.cached_description }
    fn schema(&self) -> Value { self.cached_schema.clone() }
    async fn execute(&self, input: Value) -> ToolResult {
        match self.inner.call(input).await {
            Ok(output) => ToolResult::Success { output },
            Err(e) => ToolResult::Error { error: e.to_string(), retryable: true },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::future::Future;
    use crate::tools::traits::ToolDefinition;
    use serde_json::json;

    struct FakeTool;
    impl AlephToolDyn for FakeTool {
        fn name(&self) -> &str { "fake" }
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: "fake".into(),
                description: "A fake tool".into(),
                parameters: json!({"type": "object"}),
            }
        }
        fn call(&self, args: Value) -> Pin<Box<dyn Future<Output = anyhow::Result<Value>> + Send + '_>> {
            Box::pin(async move { Ok(json!({ "result": "ok" })) })
        }
    }

    #[tokio::test]
    async fn test_adapter_wraps_correctly() {
        let adapter = BuiltinToolAdapterOwned::new(Arc::new(FakeTool));
        assert_eq!(adapter.name(), "fake");
        assert_eq!(adapter.description(), "A fake tool");
        let result = adapter.execute(json!({})).await;
        assert!(matches!(result, ToolResult::Success { .. }));
    }
}
```

**Step 4: Run test**

Run: `cargo test -p alephcore --lib agent_loop::minimal::adapters -- -v`
Expected: PASS

**Step 5: Commit**

```bash
git add src/agent_loop/minimal/adapters/
git commit -m "minimal-loop: add BuiltinToolAdapter for AlephToolDyn → MinimalTool"
```

---

### Task 2.2: MCP Tool Adapter

**Files:**
- Create: `src/agent_loop/minimal/adapters/mcp_adapter.rs`

**Context:**
MCP tools are called via the MCP transport layer. We need to wrap MCP tool info + transport as a `MinimalTool`.

Reference: `src/mcp/` for transport types. MCP tools have `name`, `description`, `input_schema`, and are executed via JSON-RPC `tools/call` method.

**Step 1-3: Write adapter**

```rust
// src/agent_loop/minimal/adapters/mcp_adapter.rs
use std::sync::Arc;
use async_trait::async_trait;
use serde_json::Value;

use super::super::tool::{MinimalTool, ToolResult};

/// Minimal info needed to represent an MCP tool
#[derive(Clone)]
pub struct McpToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub server_name: String,
}

/// Trait for MCP transport — abstraction for testability
#[async_trait]
pub trait McpTransportTrait: Send + Sync {
    async fn call_tool(&self, server: &str, tool: &str, args: Value) -> anyhow::Result<Value>;
}

pub struct McpToolAdapter<T: McpTransportTrait> {
    spec: McpToolSpec,
    transport: Arc<T>,
}

impl<T: McpTransportTrait> McpToolAdapter<T> {
    pub fn new(spec: McpToolSpec, transport: Arc<T>) -> Self {
        Self { spec, transport }
    }
}

#[async_trait]
impl<T: McpTransportTrait + 'static> MinimalTool for McpToolAdapter<T> {
    fn name(&self) -> &str { &self.spec.name }
    fn description(&self) -> &str { &self.spec.description }
    fn schema(&self) -> Value { self.spec.input_schema.clone() }

    async fn execute(&self, input: Value) -> ToolResult {
        match self.transport.call_tool(&self.spec.server_name, &self.spec.name, input).await {
            Ok(output) => ToolResult::Success { output },
            Err(e) => ToolResult::Error { error: e.to_string(), retryable: true },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct FakeTransport;
    #[async_trait]
    impl McpTransportTrait for FakeTransport {
        async fn call_tool(&self, _server: &str, _tool: &str, args: Value) -> anyhow::Result<Value> {
            Ok(json!({ "echoed": args }))
        }
    }

    #[tokio::test]
    async fn test_mcp_adapter() {
        let spec = McpToolSpec {
            name: "weather".into(),
            description: "Get weather".into(),
            input_schema: json!({"type": "object"}),
            server_name: "weather-server".into(),
        };
        let adapter = McpToolAdapter::new(spec, Arc::new(FakeTransport));
        assert_eq!(adapter.name(), "weather");
        let result = adapter.execute(json!({"city": "Tokyo"})).await;
        match result {
            ToolResult::Success { output } => {
                assert_eq!(output["echoed"]["city"], "Tokyo");
            }
            _ => panic!("Expected success"),
        }
    }
}
```

**Step 4-5: Test and commit**

Run: `cargo test -p alephcore --lib agent_loop::minimal::adapters::mcp_adapter -- -v`

```bash
git add src/agent_loop/minimal/adapters/
git commit -m "minimal-loop: add McpToolAdapter for MCP tools → MinimalTool"
```

---

### Task 2.3: Memory as Tool Adapter

**Files:**
- Create: `src/agent_loop/minimal/adapters/memory_adapter.rs`

**Context:**
Memory hybrid retrieval in `src/memory/` stays intact. We wrap it as two MinimalTools: `memory_search` and `memory_store`. The LLM decides when to use them.

**Step 1-3: Write adapter**

```rust
// src/agent_loop/minimal/adapters/memory_adapter.rs
use std::sync::Arc;
use async_trait::async_trait;
use serde_json::{json, Value};

use super::super::tool::{MinimalTool, ToolResult};

/// Trait abstracting the memory store for testability
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryEntry>>;
    async fn store(&self, content: &str, metadata: Option<Value>) -> anyhow::Result<String>;
}

#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub id: String,
    pub content: String,
    pub score: f32,
    pub metadata: Option<Value>,
}

pub struct MemorySearchTool<M: MemoryBackend> {
    backend: Arc<M>,
}

impl<M: MemoryBackend> MemorySearchTool<M> {
    pub fn new(backend: Arc<M>) -> Self { Self { backend } }
}

#[async_trait]
impl<M: MemoryBackend + 'static> MinimalTool for MemorySearchTool<M> {
    fn name(&self) -> &str { "memory_search" }
    fn description(&self) -> &str { "Search long-term memory for relevant information. Use when you need to recall past conversations, user preferences, or stored knowledge." }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Natural language search query" },
                "limit": { "type": "integer", "description": "Max results to return", "default": 5 }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let query = input["query"].as_str().unwrap_or("");
        let limit = input["limit"].as_u64().unwrap_or(5) as usize;

        match self.backend.search(query, limit).await {
            Ok(entries) => {
                let results: Vec<Value> = entries.iter().map(|e| {
                    json!({ "content": e.content, "relevance": e.score })
                }).collect();
                ToolResult::Success { output: json!({ "results": results, "count": results.len() }) }
            }
            Err(e) => ToolResult::Error { error: e.to_string(), retryable: true },
        }
    }
}

pub struct MemoryStoreTool<M: MemoryBackend> {
    backend: Arc<M>,
}

impl<M: MemoryBackend> MemoryStoreTool<M> {
    pub fn new(backend: Arc<M>) -> Self { Self { backend } }
}

#[async_trait]
impl<M: MemoryBackend + 'static> MinimalTool for MemoryStoreTool<M> {
    fn name(&self) -> &str { "memory_store" }
    fn description(&self) -> &str { "Store important information in long-term memory for future recall. Use for user preferences, key facts, or decisions worth remembering." }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": "The information to remember" },
                "metadata": { "type": "object", "description": "Optional metadata tags" }
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let content = input["content"].as_str().unwrap_or("");
        let metadata = input.get("metadata").cloned();
        match self.backend.store(content, metadata).await {
            Ok(id) => ToolResult::Success { output: json!({ "stored": true, "id": id }) },
            Err(e) => ToolResult::Error { error: e.to_string(), retryable: true },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Mutex;

    struct FakeMemory {
        entries: Mutex<Vec<MemoryEntry>>,
    }

    impl FakeMemory {
        fn new() -> Self { Self { entries: Mutex::new(vec![]) } }
    }

    #[async_trait]
    impl MemoryBackend for FakeMemory {
        async fn search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryEntry>> {
            let entries = self.entries.lock().await;
            Ok(entries.iter()
                .filter(|e| e.content.contains(query))
                .take(limit)
                .cloned()
                .collect())
        }
        async fn store(&self, content: &str, metadata: Option<Value>) -> anyhow::Result<String> {
            let id = format!("mem_{}", self.entries.lock().await.len());
            self.entries.lock().await.push(MemoryEntry {
                id: id.clone(), content: content.to_string(), score: 1.0, metadata,
            });
            Ok(id)
        }
    }

    #[tokio::test]
    async fn test_memory_store_and_search() {
        let backend = Arc::new(FakeMemory::new());
        let store_tool = MemoryStoreTool::new(backend.clone());
        let search_tool = MemorySearchTool::new(backend);

        // Store
        let result = store_tool.execute(json!({ "content": "User prefers dark mode" })).await;
        assert!(matches!(result, ToolResult::Success { .. }));

        // Search
        let result = search_tool.execute(json!({ "query": "dark mode" })).await;
        match result {
            ToolResult::Success { output } => assert_eq!(output["count"], 1),
            _ => panic!("Expected success"),
        }
    }
}
```

**Step 4-5: Test and commit**

Run: `cargo test -p alephcore --lib agent_loop::minimal::adapters::memory_adapter -- -v`

```bash
git add src/agent_loop/minimal/adapters/
git commit -m "minimal-loop: add Memory search/store as MinimalTool adapters"
```

---

### Task 2.4: Daemon as Event Source Adapter

**Files:**
- Create: `src/agent_loop/minimal/adapters/daemon_adapter.rs`

**Context:**
Daemon events should become messages that enter the Core Loop. When a system event fires, it constructs a `LoopMessage::User` with the event description, and the LLM decides how to respond.

Also expose `daemon_query` and `daemon_subscribe` as MinimalTools.

**Implementation:** Similar pattern to memory adapter — define `DaemonBackend` trait, wrap as MinimalTool. The actual wiring to existing Daemon perception will happen in Phase 3. This task creates the adapter interface.

```rust
// src/agent_loop/minimal/adapters/daemon_adapter.rs
use std::sync::Arc;
use async_trait::async_trait;
use serde_json::{json, Value};
use super::super::tool::{MinimalTool, ToolResult};

#[async_trait]
pub trait DaemonBackend: Send + Sync {
    async fn query_active_events(&self) -> anyhow::Result<Vec<DaemonEvent>>;
    async fn subscribe(&self, rule: &str) -> anyhow::Result<String>;
}

#[derive(Debug, Clone)]
pub struct DaemonEvent {
    pub event_type: String,
    pub description: String,
    pub timestamp: i64,
}

pub struct DaemonQueryTool<D: DaemonBackend> {
    backend: Arc<D>,
}

impl<D: DaemonBackend> DaemonQueryTool<D> {
    pub fn new(backend: Arc<D>) -> Self { Self { backend } }
}

#[async_trait]
impl<D: DaemonBackend + 'static> MinimalTool for DaemonQueryTool<D> {
    fn name(&self) -> &str { "daemon_query" }
    fn description(&self) -> &str { "Query active system events and notifications" }
    fn schema(&self) -> Value { json!({"type": "object", "properties": {}}) }
    async fn execute(&self, _input: Value) -> ToolResult {
        match self.backend.query_active_events().await {
            Ok(events) => {
                let list: Vec<Value> = events.iter().map(|e| json!({
                    "type": e.event_type, "description": e.description
                })).collect();
                ToolResult::Success { output: json!({ "events": list }) }
            }
            Err(e) => ToolResult::Error { error: e.to_string(), retryable: true },
        }
    }
}

pub struct DaemonSubscribeTool<D: DaemonBackend> {
    backend: Arc<D>,
}

impl<D: DaemonBackend> DaemonSubscribeTool<D> {
    pub fn new(backend: Arc<D>) -> Self { Self { backend } }
}

#[async_trait]
impl<D: DaemonBackend + 'static> MinimalTool for DaemonSubscribeTool<D> {
    fn name(&self) -> &str { "daemon_subscribe" }
    fn description(&self) -> &str { "Subscribe to a new type of system event monitoring" }
    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "rule": { "type": "string", "description": "Event pattern to watch for" } },
            "required": ["rule"]
        })
    }
    async fn execute(&self, input: Value) -> ToolResult {
        let rule = input["rule"].as_str().unwrap_or("");
        match self.backend.subscribe(rule).await {
            Ok(id) => ToolResult::Success { output: json!({ "subscription_id": id }) },
            Err(e) => ToolResult::Error { error: e.to_string(), retryable: true },
        }
    }
}
```

**Test, update mod.rs, commit**

```bash
git add src/agent_loop/minimal/adapters/
git commit -m "minimal-loop: add Daemon event tools as MinimalTool adapters"
```

---

## Phase 3: AiProvider Integration

### Task 3.1: Implement MinimalProvider for AiProvider

**Files:**
- Create: `src/agent_loop/minimal/provider_bridge.rs`

**Context:**
Bridge from `MinimalProvider` trait to the existing `AiProvider::process_with_payload(RequestPayload)`. This converts `LoopMessage` list into the format `RequestPayload` expects.

Key reference: `src/providers/adapter.rs` — `RequestPayload` has fields: `input`, `system_prompt`, `tools`, etc. `ProviderResponse` has `text`, `tool_calls`, `stop_reason`.

The existing `AiProvider` implementations (Anthropic, OpenAI, Gemini) already support native tool calling via `RequestPayload.tools`. We just need to format `LoopMessage` list as the conversation messages.

**Note:** The exact message format depends on the provider's protocol adapter. For initial implementation, we serialize messages as the `input` field with a structured format. The proper implementation will use the provider's message array API — this will be refined when wiring to real providers.

```rust
// src/agent_loop/minimal/provider_bridge.rs
use std::sync::Arc;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::AiProvider;
use crate::tools::traits::ToolDefinition;

use super::loop_core::{MinimalProvider, LoopMessage};

/// Bridges MinimalProvider to an existing AiProvider implementation
pub struct AiProviderBridge {
    provider: Arc<dyn AiProvider>,
}

impl AiProviderBridge {
    pub fn new(provider: Arc<dyn AiProvider>) -> Self {
        Self { provider }
    }

    /// Convert LoopMessages to a structured input string for the provider
    fn format_messages(messages: &[LoopMessage]) -> String {
        messages.iter().map(|m| match m {
            LoopMessage::User(text) => format!("<user>\n{text}\n</user>"),
            LoopMessage::Assistant(text) => format!("<assistant>\n{text}\n</assistant>"),
            LoopMessage::ToolUse { id, name, input } => {
                format!("<tool_use id=\"{id}\" name=\"{name}\">\n{input}\n</tool_use>")
            }
            LoopMessage::ToolResult { id, output, is_error } => {
                let tag = if *is_error { "tool_error" } else { "tool_result" };
                format!("<{tag} id=\"{id}\">\n{output}\n</{tag}>")
            }
        }).collect::<Vec<_>>().join("\n\n")
    }
}

#[async_trait]
impl MinimalProvider for AiProviderBridge {
    async fn call(
        &self,
        messages: &[LoopMessage],
        system_prompt: &str,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<ProviderResponse> {
        let input = Self::format_messages(messages);
        let payload = RequestPayload {
            input: &input,
            system_prompt: Some(system_prompt),
            image: None,
            attachments: None,
            think_level: None,
            force_standard_mode: false,
            temperature: None,
            max_tokens: None,
            tools: if tools.is_empty() { None } else { Some(tools) },
        };
        self.provider.process_with_payload(payload).await
    }
}
```

**Test and commit**

```bash
git add src/agent_loop/minimal/provider_bridge.rs
git commit -m "minimal-loop: add AiProviderBridge connecting to existing providers"
```

---

### Task 3.2: Wire SoulManifest into MinimalPromptBuilder

**Files:**
- Modify: `src/agent_loop/minimal/prompt_builder.rs`

**Context:**
Add a `from_soul(soul: &SoulManifest)` constructor that populates identity, tone, and directives from the existing `SoulManifest` type in `src/thinker/soul.rs`.

```rust
// Add to prompt_builder.rs
use crate::thinker::soul::SoulManifest;

impl MinimalPromptBuilder {
    pub fn from_soul(soul: &SoulManifest) -> Self {
        let mut builder = Self::new()
            .with_soul_identity(&soul.identity);

        builder = builder.with_soul_tone(&soul.voice.tone);

        for directive in &soul.directives {
            builder = builder.with_soul_directive(directive);
        }

        if let Some(addendum) = &soul.addendum {
            builder = builder.with_custom_instructions(addendum);
        }

        builder
    }
}
```

**Test and commit**

```bash
git add src/agent_loop/minimal/prompt_builder.rs
git commit -m "minimal-loop: wire SoulManifest into MinimalPromptBuilder"
```

---

## Phase 4: Gateway Integration

### Task 4.1: Add Minimal Loop Feature Flag

**Files:**
- Modify: `src/config/` — add `use_minimal_loop: bool` to relevant config
- Create: `src/agent_loop/minimal/factory.rs` — factory function to build MinimalAgentLoop from AppContext

**Context:**
Add a config flag `use_minimal_loop: bool` (default `false`) that switches between old `AgentLoop` and new `MinimalAgentLoop` in the gateway handler. This allows A/B testing.

The factory function assembles the `MinimalAgentLoop` from existing services:
- Provider from `ProviderRegistry`
- Tools from `AlephToolServer` (wrapped via adapters)
- Soul from config
- Safety from config

**Implementation details:** This is the integration task — exact code depends on how `AppContext` / service container is structured. Read `src/gateway/handlers/agent.rs` and `src/gateway/server.rs` to understand the current assembly.

**Step 1: Create factory**

```rust
// src/agent_loop/minimal/factory.rs
// Factory to assemble MinimalAgentLoop from existing services
// This file bridges the old world to the new world

use std::sync::Arc;
use super::*;
use super::adapters::builtin_adapter::BuiltinToolAdapterOwned;
use super::provider_bridge::AiProviderBridge;

pub struct MinimalLoopFactory;

impl MinimalLoopFactory {
    /// Build a MinimalAgentLoop from existing Aleph services
    pub fn build(
        provider: Arc<dyn crate::providers::AiProvider>,
        tool_server: &crate::tools::server::AlephToolServer,
        soul: Option<&crate::thinker::soul::SoulManifest>,
        config: LoopConfig,
    ) -> MinimalAgentLoop<AiProviderBridge> {
        // Wrap provider
        let bridge = AiProviderBridge::new(provider);

        // Adapt existing tools
        let mut registry = MinimalToolRegistry::new();
        for tool_dyn in tool_server.list_tools() {
            registry.register(Box::new(BuiltinToolAdapterOwned::new(tool_dyn)));
        }

        // Build prompt
        let prompt_builder = match soul {
            Some(s) => MinimalPromptBuilder::from_soul(s),
            None => MinimalPromptBuilder::new(),
        };

        // Safety guard with defaults
        let safety = SafetyGuard::default_guard();

        MinimalAgentLoop::new(bridge, registry, prompt_builder, safety, config)
    }
}
```

> **Note to implementor:** The `tool_server.list_tools()` method may not exist exactly as shown. Check `AlephToolServer` in `src/tools/server.rs` for the actual method to enumerate registered tools (likely `list_definitions()` or iterating `tools` field). Adapt accordingly.

**Step 2: Test compilation**

Run: `cargo check -p alephcore`

**Step 3: Commit**

```bash
git add src/agent_loop/minimal/
git commit -m "minimal-loop: add factory for assembling MinimalAgentLoop from existing services"
```

---

### Task 4.2: Add Gateway Handler Route

**Files:**
- Modify: `src/gateway/handlers/agent.rs`

**Context:**
Add an alternative code path in the `agent.run` handler that uses `MinimalAgentLoop` when the config flag is enabled. The handler should:

1. Check config for `use_minimal_loop`
2. If true, build `MinimalAgentLoop` via factory
3. Call `minimal_loop.run(input, callback)`
4. Stream results back via existing EventEmitter

**Implementation:** This is a conditional branch in the existing handler. The exact integration depends on how the current handler is structured. Read `src/gateway/handlers/agent.rs:32-99` to understand `AgentRunManager` and add the alternative path.

**Step 1-3: Modify handler to support feature flag**

Add to the `handle_agent_run` function (or equivalent):

```rust
// Pseudocode for the integration point:
if config.use_minimal_loop {
    let minimal = MinimalLoopFactory::build(provider, tool_server, soul, loop_config);
    let mut callback = StreamingCallback::new(event_emitter);
    minimal.run(&params.input, &mut callback).await?;
} else {
    // Existing AgentLoop path (unchanged)
    agent_loop.run(run_context, callback).await?;
}
```

**Step 2: Test**

Run: `cargo check -p alephcore`
Then manual test: start server, send `agent.run` with minimal loop enabled.

**Step 3: Commit**

```bash
git add src/gateway/handlers/agent.rs src/config/
git commit -m "minimal-loop: add feature flag to switch between old and new agent loop"
```

---

## Phase 5: Module Cleanup (After Validation)

> **IMPORTANT:** Phase 5 should ONLY be executed after Phase 1-4 are validated and the minimal loop is working end-to-end in production-like conditions. Each removal is a separate commit for easy revert.

### Task 5.1: Remove Intent Detection

**Files:**
- Remove: `src/intent/` (entire directory)
- Modify: files that import from `intent::` — replace with direct routing

**Step 1:** Find all references: `grep -r "use crate::intent" src/`
**Step 2:** Remove imports and replace with no-ops or direct behavior
**Step 3:** Delete the directory
**Step 4:** `cargo check -p alephcore`
**Step 5:** `git commit -m "minimal-loop: remove intent detection module — LLM handles via prompt"`

### Task 5.2: Remove POE

**Files:**
- Remove: `src/poe/` (entire directory)
- Remove: POE-related callbacks from `agent_loop/callback.rs`

**Step 1-5:** Same pattern as 5.1. Find references, remove, check, commit.

```bash
git commit -m "minimal-loop: remove POE module — LLM self-evaluates task completion"
```

### Task 5.3: Remove Resilience

**Files:**
- Remove: `src/resilience/` (entire directory)

```bash
git commit -m "minimal-loop: remove resilience module — not needed for personal assistant"
```

### Task 5.4: Remove Dispatcher/Cortex

**Files:**
- Remove: `src/dispatcher/cortex/`
- Remove: `src/dispatcher/tool_filter.rs`
- Remove: `src/dispatcher/smart_filter.rs`
- Remove: `src/dispatcher/profile_filter.rs`
- Keep: `src/dispatcher/registry/` (used by old loop, can remove later)
- Keep: `src/dispatcher/types/` (shared types)

```bash
git commit -m "minimal-loop: remove dispatcher cortex and triple filter — flat registry replaces"
```

### Task 5.5: Simplify Daemon

**Files:**
- Remove: `src/daemon/worldmodel/`
- Remove: `src/daemon/perception/` (classification logic)
- Keep: `src/daemon/` event watcher core

```bash
git commit -m "minimal-loop: simplify daemon — keep event source, remove WorldModel and Perception classifier"
```

### Task 5.6: Clean Up Old AgentLoop (Final)

**Files:**
- Remove old `AgentLoop<T,E,C>` from `src/agent_loop/agent_loop.rs` once minimal loop is validated
- Remove `ThinkerTrait`, `ActionExecutor` traits from `traits.rs`
- Remove `decision_parser.rs`, `thinking.rs`, old `guards.rs`
- Rename `minimal/` to be the primary implementation

```bash
git commit -m "minimal-loop: promote MinimalAgentLoop as primary, remove old OTAF loop"
```

---

## Validation Checklist

After each phase, verify:

- [ ] `cargo check -p alephcore` — no compile errors
- [ ] `cargo test -p alephcore --lib` — existing tests pass (except intentionally removed modules)
- [ ] `cargo clippy -p alephcore` — no warnings
- [ ] Manual test: start server, send text message → get response
- [ ] Manual test: send message requiring tool use → tool executes correctly
- [ ] Manual test: send message to Telegram/Discord → multi-channel works
- [ ] Manual test: memory search/store tools work via LLM
- [ ] Performance: response latency comparable or better than old loop
