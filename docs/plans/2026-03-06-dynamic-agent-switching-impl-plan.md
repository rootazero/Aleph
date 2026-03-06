# Dynamic Agent Switching Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable natural language agent switching with dynamic agent creation at the InboundMessageRouter layer.

**Architecture:** Add an intent detection layer to InboundMessageRouter (keyword regex + LLM fallback) that intercepts switch requests, dynamically creates agents with LLM-generated SOUL.md, and switches the active agent — all before the message reaches the agent loop.

**Tech Stack:** Rust, regex, tokio, serde_json, existing LLM provider infrastructure

---

## Dependency Graph

```
T1 (intent_detector module) → T2 (keyword matching) → T3 (LLM fallback)
                                                            ↓
T4 (dynamic agent creation) → T5 (wire into router) → T6 (cleanup switch_agent tool)
```

T1-T3 can be done sequentially. T4 depends on T1. T5 depends on T2-T4. T6 is independent cleanup.

---

### Task 1: Create `intent_detector` module with types

**Files:**
- Create: `core/src/gateway/intent_detector.rs`
- Modify: `core/src/gateway/mod.rs`

**Context:** This module defines the intent detection types and the public API. The `InboundMessageRouter` will call `IntentDetector::detect()` on each incoming DM. If the result is `SwitchAgent`, the router intercepts instead of forwarding to the agent loop.

**Step 1: Create the module with types and keyword detection stub**

```rust
// core/src/gateway/intent_detector.rs

//! Intent Detection for inbound messages
//!
//! Hybrid approach: keyword regex (zero latency) → LLM fallback.

use regex::Regex;
use once_cell::sync::Lazy;
use tracing::{debug, info};

/// Result of intent detection
#[derive(Debug, Clone, PartialEq)]
pub enum DetectedIntent {
    /// User wants to switch to a different agent
    SwitchAgent {
        /// English snake_case ID for filesystem (e.g., "trading")
        id: String,
        /// Display name in user's language (e.g., "交易助手")
        name: String,
    },
    /// Normal message — route to agent as usual
    Normal,
}

/// Async function type for LLM-based intent classification
pub type IntentClassifyFn = std::sync::Arc<
    dyn Fn(&str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<DetectedIntent>> + Send>>
        + Send
        + Sync,
>;

/// Intent detector with hybrid keyword + LLM approach
pub struct IntentDetector {
    llm_classify_fn: Option<IntentClassifyFn>,
}

impl IntentDetector {
    /// Create a new detector (keyword-only, no LLM fallback)
    pub fn new() -> Self {
        Self {
            llm_classify_fn: None,
        }
    }

    /// Add LLM fallback for intent classification
    pub fn with_llm_classify(mut self, f: IntentClassifyFn) -> Self {
        self.llm_classify_fn = Some(f);
        self
    }

    /// Detect intent from a message
    pub async fn detect(&self, text: &str) -> DetectedIntent {
        // 1. Try keyword match first (zero latency)
        if let Some(intent) = self.keyword_match(text) {
            info!("[IntentDetector] Keyword match: {:?}", intent);
            return intent;
        }

        // 2. LLM fallback
        if let Some(ref classify) = self.llm_classify_fn {
            if let Some(intent) = (classify)(text).await {
                info!("[IntentDetector] LLM classified: {:?}", intent);
                return intent;
            }
        }

        DetectedIntent::Normal
    }

    /// Fast keyword-based detection
    fn keyword_match(&self, text: &str) -> Option<DetectedIntent> {
        // Chinese patterns
        static RE_CN_SWITCH: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"^(?:切换到|换成|切换为|使用)(.+?)(?:模式|助手|agent)?$").unwrap());
        static RE_CN_CHAT: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"^我想(?:和|跟|找)(.+?)(?:聊|说|谈|咨询)").unwrap());

        // English patterns
        static RE_EN_SWITCH: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"(?i)^(?:switch to|change to|use) (.+?)(?:\s+(?:mode|agent|assistant))?$").unwrap());

        let text = text.trim();

        // Try Chinese patterns
        if let Some(caps) = RE_CN_SWITCH.captures(text) {
            let name = caps[1].trim().to_string();
            if !name.is_empty() {
                return Some(DetectedIntent::SwitchAgent {
                    id: String::new(), // Will be resolved by LLM later
                    name,
                });
            }
        }
        if let Some(caps) = RE_CN_CHAT.captures(text) {
            let name = caps[1].trim().to_string();
            if !name.is_empty() {
                return Some(DetectedIntent::SwitchAgent {
                    id: String::new(),
                    name,
                });
            }
        }

        // Try English patterns
        if let Some(caps) = RE_EN_SWITCH.captures(text) {
            let name = caps[1].trim().to_string();
            if !name.is_empty() {
                let id = name.to_lowercase().replace(' ', "_");
                return Some(DetectedIntent::SwitchAgent { id, name });
            }
        }

        None
    }
}

impl Default for IntentDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keyword_cn_switch() {
        let detector = IntentDetector::new();
        assert_eq!(
            detector.keyword_match("切换到交易助手"),
            Some(DetectedIntent::SwitchAgent {
                id: String::new(),
                name: "交易".to_string(),
            })
        );
    }

    #[test]
    fn test_keyword_cn_switch_mode() {
        let detector = IntentDetector::new();
        assert_eq!(
            detector.keyword_match("切换到健康模式"),
            Some(DetectedIntent::SwitchAgent {
                id: String::new(),
                name: "健康".to_string(),
            })
        );
    }

    #[test]
    fn test_keyword_cn_chat() {
        let detector = IntentDetector::new();
        let result = detector.keyword_match("我想和交易专家聊");
        assert!(matches!(result, Some(DetectedIntent::SwitchAgent { .. })));
    }

    #[test]
    fn test_keyword_en_switch() {
        let detector = IntentDetector::new();
        assert_eq!(
            detector.keyword_match("switch to trading"),
            Some(DetectedIntent::SwitchAgent {
                id: "trading".to_string(),
                name: "trading".to_string(),
            })
        );
    }

    #[test]
    fn test_keyword_en_switch_assistant() {
        let detector = IntentDetector::new();
        assert_eq!(
            detector.keyword_match("switch to health assistant"),
            Some(DetectedIntent::SwitchAgent {
                id: "health".to_string(),
                name: "health".to_string(),
            })
        );
    }

    #[test]
    fn test_keyword_normal_message() {
        let detector = IntentDetector::new();
        assert_eq!(detector.keyword_match("今天比特币价格多少"), None);
        assert_eq!(detector.keyword_match("hello world"), None);
    }

    #[tokio::test]
    async fn test_detect_no_llm_fallback() {
        let detector = IntentDetector::new();
        assert_eq!(
            detector.detect("今天天气怎么样").await,
            DetectedIntent::Normal
        );
    }

    #[tokio::test]
    async fn test_detect_keyword_hit() {
        let detector = IntentDetector::new();
        let result = detector.detect("切换到交易助手").await;
        assert!(matches!(result, DetectedIntent::SwitchAgent { .. }));
    }
}
```

**Step 2: Register module in gateway/mod.rs**

Add to `core/src/gateway/mod.rs`:
```rust
pub mod intent_detector;
pub use intent_detector::{IntentDetector, DetectedIntent, IntentClassifyFn};
```

**Step 3: Run tests**

```bash
cargo test -p alephcore --lib gateway::intent_detector -- -v
```

Expected: All tests pass.

**Step 4: Commit**

```bash
git add core/src/gateway/intent_detector.rs core/src/gateway/mod.rs
git commit -m "gateway: add intent_detector module with keyword matching"
```

---

### Task 2: Add LLM intent classification and id resolution

**Files:**
- Modify: `core/src/gateway/intent_detector.rs`

**Context:** When keyword matching returns a `SwitchAgent` with empty `id` (Chinese input), or when keyword matching fails entirely, we need LLM to either resolve the id or classify the intent. This task adds two LLM prompt builders used by the router when wiring the classify function.

**Step 1: Add LLM prompt builders and JSON parser**

Add these functions to `intent_detector.rs`:

```rust
/// Build prompt for intent classification (keyword miss fallback)
pub fn build_intent_classify_prompt(message: &str) -> String {
    format!(
        r#"Classify this message. If the user wants to switch to a different AI agent/persona, return JSON:
{{"intent":"switch","id":"english_snake_case","name":"display name"}}
Otherwise return:
{{"intent":"normal"}}

Rules for id: lowercase English, use underscores, short (1-2 words). Examples:
- "交易助手" -> id: "trading"
- "健康顾问" -> id: "health"
- "coding expert" -> id: "coding"

Message: {}"#,
        message
    )
}

/// Build prompt to resolve an English id from a display name
pub fn build_id_resolve_prompt(name: &str) -> String {
    format!(
        r#"Given this AI agent name, return ONLY a short English snake_case id (no quotes, no explanation).
Examples: "交易助手" -> trading, "健康顾问" -> health, "Code Expert" -> coding, "主助手" -> main
Name: {}"#,
        name
    )
}

/// Build prompt to generate SOUL.md content for a new agent
pub fn build_soul_generation_prompt(id: &str, name: &str) -> String {
    format!(
        r#"Generate a concise AI persona description for an agent named "{name}" (id: {id}).
Write 3-5 sentences describing this agent's expertise, communication style, and personality.
Write in the same language as the name. Be specific to the domain.
Output ONLY the persona description, no headers or markdown formatting."#
    )
}

/// Parse LLM response for intent classification
pub fn parse_intent_response(response: &str) -> Option<DetectedIntent> {
    // Try to extract JSON from response (may have surrounding text)
    let text = response.trim();

    // Find JSON object
    let start = text.find('{')?;
    let end = text.rfind('}')? + 1;
    let json_str = &text[start..end];

    let value: serde_json::Value = serde_json::from_str(json_str).ok()?;

    match value.get("intent")?.as_str()? {
        "switch" => {
            let id = value.get("id")?.as_str()?.to_string();
            let name = value.get("name")?.as_str()?.to_string();
            if id.is_empty() || name.is_empty() {
                return None;
            }
            Some(DetectedIntent::SwitchAgent { id, name })
        }
        "normal" => Some(DetectedIntent::Normal),
        _ => None,
    }
}
```

**Step 2: Add tests for prompt builders and parser**

```rust
#[test]
fn test_parse_intent_switch() {
    let resp = r#"{"intent":"switch","id":"trading","name":"交易助手"}"#;
    assert_eq!(
        parse_intent_response(resp),
        Some(DetectedIntent::SwitchAgent {
            id: "trading".to_string(),
            name: "交易助手".to_string(),
        })
    );
}

#[test]
fn test_parse_intent_normal() {
    let resp = r#"{"intent":"normal"}"#;
    assert_eq!(parse_intent_response(resp), Some(DetectedIntent::Normal));
}

#[test]
fn test_parse_intent_with_surrounding_text() {
    let resp = r#"Based on the message, here is my classification: {"intent":"switch","id":"health","name":"健康顾问"} That's my answer."#;
    let result = parse_intent_response(resp);
    assert!(matches!(result, Some(DetectedIntent::SwitchAgent { .. })));
}

#[test]
fn test_parse_intent_invalid() {
    assert_eq!(parse_intent_response("not json"), None);
    assert_eq!(parse_intent_response(r#"{"intent":"unknown"}"#), None);
}

#[test]
fn test_build_prompts_not_empty() {
    assert!(!build_intent_classify_prompt("hello").is_empty());
    assert!(!build_id_resolve_prompt("交易助手").is_empty());
    assert!(!build_soul_generation_prompt("trading", "交易助手").is_empty());
}
```

**Step 3: Run tests**

```bash
cargo test -p alephcore --lib gateway::intent_detector -- -v
```

**Step 4: Commit**

```bash
git add core/src/gateway/intent_detector.rs
git commit -m "gateway: add LLM prompt builders and intent parser"
```

---

### Task 3: Add `create_dynamic` to AgentRegistry

**Files:**
- Modify: `core/src/gateway/agent_instance.rs`

**Context:** When a user requests an agent that doesn't exist, the router needs to create it dynamically. This adds a `create_dynamic()` method to `AgentRegistry` that creates the workspace directory, writes SOUL.md, creates the `AgentInstance`, and registers it.

**Step 1: Add `create_dynamic` method**

Add to `impl AgentRegistry` in `core/src/gateway/agent_instance.rs`:

```rust
    /// Dynamically create and register a new agent
    ///
    /// Creates workspace directory at `~/.aleph/workspaces/{id}/`,
    /// writes SOUL.md with provided content, and registers the agent.
    pub async fn create_dynamic(
        &self,
        id: &str,
        soul_content: &str,
        session_manager: Option<Arc<super::session_manager::SessionManager>>,
    ) -> Result<Arc<AgentInstance>, AgentInstanceError> {
        // Check if already exists
        if self.get(id).await.is_some() {
            return Err(AgentInstanceError::InitFailed(
                format!("Agent '{}' already exists", id),
            ));
        }

        let workspace_root = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".aleph/workspaces");
        let workspace_path = workspace_root.join(id);

        // Create workspace directory
        std::fs::create_dir_all(&workspace_path).map_err(|e| {
            AgentInstanceError::InitFailed(format!(
                "Failed to create workspace for '{}': {}", id, e
            ))
        })?;

        // Write SOUL.md
        let soul_path = workspace_path.join("SOUL.md");
        if !soul_path.exists() {
            std::fs::write(&soul_path, soul_content).map_err(|e| {
                AgentInstanceError::InitFailed(format!(
                    "Failed to write SOUL.md for '{}': {}", id, e
                ))
            })?;
        }

        // Create agent config
        let config = AgentInstanceConfig {
            agent_id: id.to_string(),
            workspace: workspace_path,
            ..Default::default()
        };

        // Create and register
        let instance = if let Some(sm) = session_manager {
            AgentInstance::with_session_manager(config, sm)?
        } else {
            AgentInstance::new(config)?
        };

        self.register(instance).await;
        let agent = self.get(id).await.unwrap();
        info!("Dynamically created agent: {}", id);
        Ok(agent)
    }
```

**Step 2: Add test**

```rust
#[tokio::test]
async fn test_create_dynamic_agent() {
    let temp = tempdir().unwrap();
    // Override home for test
    let registry = AgentRegistry::new();

    let config = AgentInstanceConfig {
        agent_id: "main".to_string(),
        workspace: temp.path().join("main"),
        ..Default::default()
    };
    let instance = AgentInstance::new(config).unwrap();
    registry.register(instance).await;

    // Dynamic creation
    let result = registry.create_dynamic(
        "trading",
        "You are a trading assistant.",
        None,
    ).await;
    assert!(result.is_ok());

    let agent = registry.get("trading").await;
    assert!(agent.is_some());
    assert_eq!(agent.unwrap().id(), "trading");

    let agents = registry.list().await;
    assert_eq!(agents.len(), 2);
}

#[tokio::test]
async fn test_create_dynamic_already_exists() {
    let temp = tempdir().unwrap();
    let registry = AgentRegistry::new();
    let config = AgentInstanceConfig {
        agent_id: "main".to_string(),
        workspace: temp.path().join("main"),
        ..Default::default()
    };
    registry.register(AgentInstance::new(config).unwrap()).await;

    let result = registry.create_dynamic("main", "soul", None).await;
    assert!(result.is_err());
}
```

**Step 3: Run tests**

```bash
cargo test -p alephcore --lib gateway::agent_instance -- -v
```

**Step 4: Commit**

```bash
git add core/src/gateway/agent_instance.rs
git commit -m "gateway: add create_dynamic to AgentRegistry for runtime agent creation"
```

---

### Task 4: Wire intent detection into InboundMessageRouter

**Files:**
- Modify: `core/src/gateway/inbound_router.rs`

**Context:** This is the main integration task. Replace the `/switch` command-only approach with the full intent detection pipeline: keyword match → LLM fallback → dynamic create → switch. The router already has `workspace_manager` and `agent_registry`.

**Step 1: Add IntentDetector field and builder**

Add to `InboundMessageRouter` struct:
```rust
    /// Intent detector for natural language agent switching
    intent_detector: Option<IntentDetector>,
    /// Session manager for dynamic agent creation
    session_manager: Option<Arc<super::session_manager::SessionManager>>,
    /// LLM provider for intent classification and soul generation
    llm_provider: Option<Arc<dyn crate::providers::traits::ProviderTrait>>,
```

Add builder methods:
```rust
    pub fn with_intent_detector(mut self, detector: IntentDetector) -> Self {
        self.intent_detector = Some(detector);
        self
    }

    pub fn with_session_manager(mut self, sm: Arc<super::session_manager::SessionManager>) -> Self {
        self.session_manager = Some(sm);
        self
    }

    pub fn with_llm_provider(mut self, provider: Arc<dyn crate::providers::traits::ProviderTrait>) -> Self {
        self.llm_provider = Some(provider);
        self
    }
```

Initialize all new fields to `None` in constructors.

**Step 2: Add intent handling method**

Add a new method `try_handle_switch_intent` to `InboundMessageRouter`:

```rust
    /// Try to handle a switch intent from the message.
    /// Returns Some(Ok(())) if handled (message consumed), None if not a switch intent.
    async fn try_handle_switch_intent(
        &self,
        msg: &InboundMessage,
    ) -> Option<Result<(), RoutingError>> {
        let detector = self.intent_detector.as_ref()?;
        let manager = self.workspace_manager.as_ref()?;
        let registry = self.agent_registry.as_ref()?;

        let mut intent = detector.detect(&msg.text).await;

        // If keyword matched but id is empty, resolve via LLM
        if let DetectedIntent::SwitchAgent { ref id, ref name } = intent {
            if id.is_empty() {
                if let Some(ref provider) = self.llm_provider {
                    let prompt = super::intent_detector::build_id_resolve_prompt(name);
                    match provider.process(&prompt, None).await {
                        Ok(response) => {
                            let resolved_id = response.trim().to_lowercase().replace(' ', "_");
                            if !resolved_id.is_empty() {
                                intent = DetectedIntent::SwitchAgent {
                                    id: resolved_id,
                                    name: name.clone(),
                                };
                            }
                        }
                        Err(e) => {
                            warn!("[Router] Failed to resolve agent id: {}", e);
                        }
                    }
                }
            }
        }

        match intent {
            DetectedIntent::SwitchAgent { id, name } if !id.is_empty() => {
                let channel_id = msg.channel_id.as_str();
                let sender_id = msg.sender_id.as_str();

                // Create agent dynamically if it doesn't exist
                if registry.get(&id).await.is_none() {
                    info!("[Router] Agent '{}' not found, creating dynamically", id);

                    // Generate SOUL.md via LLM
                    let soul_content = if let Some(ref provider) = self.llm_provider {
                        let prompt = super::intent_detector::build_soul_generation_prompt(&id, &name);
                        match provider.process(&prompt, None).await {
                            Ok(content) => content,
                            Err(e) => {
                                warn!("[Router] Failed to generate soul: {}, using default", e);
                                format!("You are {}, an AI assistant specializing in {} related tasks.", name, name)
                            }
                        }
                    } else {
                        format!("You are {}, an AI assistant specializing in {} related tasks.", name, name)
                    };

                    if let Err(e) = registry.create_dynamic(
                        &id, &soul_content, self.session_manager.clone(),
                    ).await {
                        let reply = OutboundMessage::text(
                            msg.conversation_id.as_str(),
                            format!("Failed to create agent '{}': {}", id, e),
                        );
                        let _ = self.channel_registry.send(&msg.channel_id, reply).await;
                        return Some(Ok(()));
                    }
                }

                // Switch active agent
                let reply_text = match manager.set_active_agent(channel_id, sender_id, &id) {
                    Ok(()) => {
                        info!("[Router] Switched agent for {}:{} -> {} ({})", channel_id, sender_id, id, name);
                        format!("Switched to {} ({}). Send any message to start.", name, id)
                    }
                    Err(e) => {
                        error!("[Router] Failed to switch agent: {}", e);
                        format!("Failed to switch agent: {}", e)
                    }
                };

                let reply = OutboundMessage::text(msg.conversation_id.as_str(), reply_text);
                if let Err(e) = self.channel_registry.send(&msg.channel_id, reply).await {
                    error!("[Router] Failed to send switch reply: {}", e);
                }

                Some(Ok(()))
            }
            _ => None, // Not a switch intent
        }
    }
```

**Step 3: Call intent handler in `handle_dm`**

In `handle_dm`, **after** the `/switch` command block and **before** the group chat block, add:

```rust
        // Natural language switch intent detection
        if let Some(result) = self.try_handle_switch_intent(&msg).await {
            return result;
        }
```

**Step 4: Run check**

```bash
cargo check -p alephcore
```

**Step 5: Commit**

```bash
git add core/src/gateway/inbound_router.rs
git commit -m "gateway: wire intent detection into InboundMessageRouter"
```

---

### Task 5: Wire at startup (start/mod.rs)

**Files:**
- Modify: `core/src/bin/aleph/commands/start/mod.rs`

**Context:** The InboundMessageRouter needs its new dependencies wired at startup: IntentDetector, session_manager, and llm_provider. The LLM provider is already available from the provider registry. The IntentDetector gets an LLM classify function that uses the same provider.

**Step 1: Wire intent detector and LLM provider**

In `initialize_inbound_router()`, after the existing `workspace_manager` wiring, add:

```rust
    // Wire intent detector for natural language agent switching
    if let (Some(ref wm), Some(ref _ar)) = (&workspace_manager, &agent_registry) {
        use alephcore::gateway::intent_detector::{
            IntentDetector, IntentClassifyFn,
            build_intent_classify_prompt, parse_intent_response,
        };

        let detector = IntentDetector::new();
        // LLM fallback will be wired if provider is available
        inbound_router = inbound_router.with_intent_detector(detector);
    }
```

Pass `session_manager` and `llm_provider` (the default provider from the provider registry) into the router via new builder methods.

Find where the provider registry's default provider is available (it's created in `register_agent_handlers`), and pass it through `AgentHandlersResult`.

**Step 2: Run build**

```bash
cargo check --bin aleph
```

**Step 3: Commit**

```bash
git add core/src/bin/aleph/commands/start/mod.rs
git commit -m "gateway: wire intent detector and LLM provider at startup"
```

---

### Task 6: Remove switch_agent built-in tool

**Files:**
- Delete: `core/src/builtin_tools/switch_agent.rs`
- Modify: `core/src/builtin_tools/mod.rs` (remove module + re-export)
- Modify: `core/src/executor/builtin_registry/registry.rs` (remove tool field, handle, execute branch)

**Context:** The `switch_agent` tool is no longer needed since the router handles everything. Clean it up.

**Step 1: Remove module reference from mod.rs**

Remove from `core/src/builtin_tools/mod.rs`:
```rust
pub mod switch_agent;
pub use switch_agent::{SwitchAgentArgs, SwitchAgentOutput, SwitchAgentTool};
```

**Step 2: Remove from registry.rs**

In `core/src/executor/builtin_registry/registry.rs`:
- Remove `SwitchAgentTool` from imports
- Remove `switch_agent_tool: Arc<RwLock<Option<SwitchAgentTool>>>` field
- Remove `switch_agent_handle()` method
- Remove `"switch_agent"` from `tools.insert(...)` in `with_config()`
- Remove `"switch_agent" =>` match arm from `execute_tool()`

**Step 3: Remove from start/mod.rs**

In `core/src/bin/aleph/commands/start/mod.rs`:
- Remove `switch_agent_handle` field from `AgentHandlersResult`
- Remove the late-binding block that writes to `switch_agent_handle`
- Remove `switch_handle` variable

**Step 4: Delete the file**

```bash
rm core/src/builtin_tools/switch_agent.rs
```

**Step 5: Run tests**

```bash
cargo check --bin aleph
cargo test -p alephcore --lib builtin_tools -- -v 2>&1 | tail -5
```

**Step 6: Commit**

```bash
git add -A
git commit -m "gateway: remove switch_agent built-in tool (replaced by router intent detection)"
```

---

### Task 7: Integration test — full build and manual verification

**Step 1: Full build**

```bash
just build
```

**Step 2: Start server and verify**

```bash
target/release/aleph start
```

Check output for:
- `switch_agent tool bound` should NOT appear
- `Intent detector initialized` or similar should appear
- Telegram channel connects

**Step 3: Test via Telegram**

1. Send: `切换到交易助手` → Should create agent and switch
2. Send: `今天比特币多少` → Should be handled by trading agent
3. Send: `切换到主助手` → Should switch back to main
4. Send: `switch to health` → Should create health agent
5. Check: `ls ~/.aleph/workspaces/` → Should show `main/`, `trading/`, `health/`
6. Check: `cat ~/.aleph/workspaces/trading/SOUL.md` → Should have generated content

**Step 4: Commit any fixes**

```bash
git add -A
git commit -m "gateway: integration fixes for dynamic agent switching"
```
