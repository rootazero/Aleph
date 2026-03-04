# Multi-Agent Group Chat Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement channel-agnostic multi-agent group chat with Coordinator-as-Agent pattern, enabling multiple AI personas to collaboratively discuss user questions via any channel.

**Architecture:** Core `GroupChatOrchestrator` receives channel-agnostic `GroupChatRequest`, uses a Coordinator LLM call to decide which personas respond and in what order, then serially spawns each persona as a Thinker call with cumulative context. Results stream as `GroupChatMessage` back to the channel adapter for rendering.

**Tech Stack:** Rust, Tokio, serde/schemars (config), rusqlite (persistence), existing Thinker/ProviderRegistry (LLM calls), existing Gateway JSON-RPC (handlers).

**Design Doc:** `docs/plans/2026-03-04-multi-agent-group-chat-design.md`

---

### Task 1: Protocol Types

Define the channel-agnostic data types that form the contract between Core and Channel layers.

**Files:**
- Create: `core/src/group_chat/mod.rs`
- Create: `core/src/group_chat/protocol.rs`
- Modify: `core/src/lib.rs` (add `pub mod group_chat;`)

**Step 1: Write the failing test**

In `core/src/group_chat/protocol.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_speaker_display() {
        let s = Speaker::Persona { id: "arch".into(), name: "架构师".into() };
        assert_eq!(s.name(), "架构师");

        assert_eq!(Speaker::Coordinator.name(), "Coordinator");
        assert_eq!(Speaker::System.name(), "System");
    }

    #[test]
    fn test_group_chat_message_is_final() {
        let msg = GroupChatMessage {
            session_id: "s1".into(),
            speaker: Speaker::System,
            content: "hello".into(),
            round: 1,
            sequence: 0,
            is_final: true,
        };
        assert!(msg.is_final);
        assert_eq!(msg.round, 1);
    }

    #[test]
    fn test_group_chat_status_display() {
        assert_eq!(GroupChatStatus::Active.as_str(), "active");
        assert_eq!(GroupChatStatus::Ended.as_str(), "ended");
        assert_eq!(GroupChatStatus::from_str("active"), GroupChatStatus::Active);
        assert_eq!(GroupChatStatus::from_str("unknown"), GroupChatStatus::Active);
    }

    #[test]
    fn test_group_chat_request_variants() {
        let req = GroupChatRequest::Start {
            personas: vec![PersonaSource::Preset("arch".into())],
            topic: Some("API设计".into()),
            initial_message: "这个API怎么样?".into(),
        };
        matches!(req, GroupChatRequest::Start { .. });

        let req = GroupChatRequest::Continue {
            session_id: "s1".into(),
            message: "继续".into(),
        };
        matches!(req, GroupChatRequest::Continue { .. });
    }

    #[test]
    fn test_rendered_content_creation() {
        let rc = RenderedContent::markdown("**bold**");
        assert_eq!(rc.format, ContentFormat::Markdown);
        assert!(rc.metadata.is_none());

        let rc = RenderedContent::plain("text");
        assert_eq!(rc.format, ContentFormat::Plain);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib group_chat::protocol::tests -- --no-run 2>&1 | head -20`
Expected: FAIL — module not found

**Step 3: Write the implementation**

`core/src/group_chat/mod.rs`:
```rust
//! Multi-Agent Group Chat
//!
//! Channel-agnostic orchestration for multi-persona collaborative discussions.
//! See design doc: docs/plans/2026-03-04-multi-agent-group-chat-design.md

pub mod protocol;

pub use protocol::*;
```

`core/src/group_chat/protocol.rs`:
```rust
//! Protocol types for group chat — the contract between Core and Channel layers.

use serde::{Deserialize, Serialize};
use schemars::JsonSchema;

// =============================================================================
// Speaker
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub enum Speaker {
    Coordinator,
    Persona { id: String, name: String },
    System,
}

impl Speaker {
    pub fn name(&self) -> &str {
        match self {
            Self::Coordinator => "Coordinator",
            Self::Persona { name, .. } => name,
            Self::System => "System",
        }
    }
}

// =============================================================================
// Persona
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Persona {
    pub id: String,
    pub name: String,
    pub system_prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
}

// =============================================================================
// PersonaSource
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub enum PersonaSource {
    Preset(String),
    Inline(Persona),
}

// =============================================================================
// GroupChatRequest (Channel → Core)
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub enum GroupChatRequest {
    Start {
        personas: Vec<PersonaSource>,
        topic: Option<String>,
        initial_message: String,
    },
    Continue {
        session_id: String,
        message: String,
    },
    Mention {
        session_id: String,
        message: String,
        targets: Vec<String>,
    },
    End {
        session_id: String,
    },
}

// =============================================================================
// GroupChatMessage (Core → Channel)
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GroupChatMessage {
    pub session_id: String,
    pub speaker: Speaker,
    pub content: String,
    pub round: u32,
    pub sequence: u32,
    pub is_final: bool,
}

// =============================================================================
// GroupChatStatus
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum GroupChatStatus {
    Active,
    Paused,
    Ended,
}

impl GroupChatStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Ended => "ended",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "active" => Self::Active,
            "paused" => Self::Paused,
            "ended" => Self::Ended,
            _ => Self::Active,
        }
    }
}

// =============================================================================
// RenderedContent (Core → Channel rendering output)
// =============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ContentFormat {
    Markdown,
    Html,
    Plain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderedContent {
    pub text: String,
    pub format: ContentFormat,
    pub metadata: Option<serde_json::Value>,
}

impl RenderedContent {
    pub fn markdown(text: impl Into<String>) -> Self {
        Self { text: text.into(), format: ContentFormat::Markdown, metadata: None }
    }

    pub fn plain(text: impl Into<String>) -> Self {
        Self { text: text.into(), format: ContentFormat::Plain, metadata: None }
    }

    pub fn html(text: impl Into<String>) -> Self {
        Self { text: text.into(), format: ContentFormat::Html, metadata: None }
    }
}

// =============================================================================
// CoordinatorPlan (internal: Coordinator LLM output)
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinatorPlan {
    pub respondents: Vec<RespondentPlan>,
    #[serde(default)]
    pub need_summary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RespondentPlan {
    pub persona_id: String,
    pub order: u32,
    pub guidance: String,
}

// =============================================================================
// GroupChatError
// =============================================================================

#[derive(Debug, thiserror::Error)]
pub enum GroupChatError {
    #[error("Persona not found: {0}")]
    PersonaNotFound(String),
    #[error("Too many personas: requested {requested}, max {max}")]
    TooManyPersonas { max: usize, requested: usize },
    #[error("Max rounds reached for session {session_id}: {max}")]
    MaxRoundsReached { session_id: String, max: usize },
    #[error("Session not found: {0}")]
    SessionNotFound(String),
    #[error("Coordinator plan parse error: {0}")]
    CoordinatorPlanParseError(String),
    #[error("Persona invocation failed for {persona_id}: {source}")]
    PersonaInvocationFailed { persona_id: String, source: String },
    #[error("Provider unavailable: {provider}: {source}")]
    ProviderUnavailable { provider: String, source: String },
}
```

Add to `core/src/lib.rs` (after `pub mod gateway;` line):
```rust
pub mod group_chat;
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib group_chat::protocol::tests -v`
Expected: All 5 tests PASS

**Step 5: Commit**

```bash
git add core/src/group_chat/ core/src/lib.rs
git commit -m "group_chat: add protocol types (Speaker, Persona, GroupChatRequest/Message)"
```

---

### Task 2: GroupChat Config Types

Add `GroupChatConfig` and `PersonaConfig` to the config system.

**Files:**
- Create: `core/src/config/types/group_chat.rs`
- Modify: `core/src/config/types/mod.rs` (add module)
- Modify: `core/src/config/structs.rs` (add field to Config)

**Step 1: Write the failing test**

In `core/src/config/types/group_chat.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = GroupChatConfig::default();
        assert_eq!(config.max_personas_per_session, 6);
        assert_eq!(config.max_rounds, 10);
        assert!(!config.coordinator_visible);
        assert!(config.default_coordinator_model.is_none());
    }

    #[test]
    fn test_config_validation_valid() {
        let config = GroupChatConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_invalid_zero_personas() {
        let config = GroupChatConfig {
            max_personas_per_session: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validation_invalid_zero_rounds() {
        let config = GroupChatConfig {
            max_rounds: 0,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_persona_config_validation_valid() {
        let p = PersonaConfig {
            id: "arch".into(),
            name: "架构师".into(),
            system_prompt: "You are an architect".into(),
            provider: None,
            model: None,
            thinking_level: None,
        };
        assert!(p.validate().is_ok());
    }

    #[test]
    fn test_persona_config_validation_empty_id() {
        let p = PersonaConfig {
            id: "".into(),
            name: "架构师".into(),
            system_prompt: "prompt".into(),
            ..Default::default()
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn test_persona_config_validation_prompt_too_long() {
        let p = PersonaConfig {
            id: "test".into(),
            name: "Test".into(),
            system_prompt: "x".repeat(2001),
            ..Default::default()
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn test_config_serialization() {
        let config = GroupChatConfig::default();
        let serialized = toml::to_string(&config).unwrap();
        assert!(serialized.contains("max_personas_per_session"));
        assert!(serialized.contains("max_rounds"));
    }

    #[test]
    fn test_config_deserialization() {
        let toml_str = r#"
            max_personas_per_session = 4
            max_rounds = 5
            coordinator_visible = true
        "#;
        let config: GroupChatConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.max_personas_per_session, 4);
        assert_eq!(config.max_rounds, 5);
        assert!(config.coordinator_visible);
        assert!(config.default_coordinator_model.is_none());
    }

    #[test]
    fn test_persona_config_deserialization() {
        let toml_str = r#"
            id = "arch"
            name = "架构师"
            system_prompt = "You are an architect"
            provider = "claude"
            model = "claude-sonnet-4-20250514"
        "#;
        let p: PersonaConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(p.id, "arch");
        assert_eq!(p.provider, Some("claude".into()));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib config::types::group_chat::tests -- --no-run 2>&1 | head -10`
Expected: FAIL — module not found

**Step 3: Write the implementation**

`core/src/config/types/group_chat.rs`:
```rust
//! Group Chat Configuration
//!
//! Configuration for multi-agent group chat including persona presets
//! and orchestration settings.
//!
//! # Example Configuration (config.toml)
//!
//! ```toml
//! [group_chat]
//! max_personas_per_session = 6
//! max_rounds = 10
//! coordinator_visible = false
//!
//! [[personas]]
//! id = "architect"
//! name = "架构师"
//! system_prompt = "你是一位资深软件架构师..."
//! provider = "claude"
//! model = "claude-sonnet-4-20250514"
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Maximum allowed length for persona system prompts
const MAX_SYSTEM_PROMPT_LEN: usize = 2000;

// =============================================================================
// GroupChatConfig
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GroupChatConfig {
    #[serde(default = "default_max_personas_per_session")]
    pub max_personas_per_session: usize,

    #[serde(default = "default_max_rounds")]
    pub max_rounds: usize,

    #[serde(default)]
    pub coordinator_visible: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_coordinator_model: Option<String>,
}

fn default_max_personas_per_session() -> usize { 6 }
fn default_max_rounds() -> usize { 10 }

impl Default for GroupChatConfig {
    fn default() -> Self {
        Self {
            max_personas_per_session: default_max_personas_per_session(),
            max_rounds: default_max_rounds(),
            coordinator_visible: false,
            default_coordinator_model: None,
        }
    }
}

impl GroupChatConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_personas_per_session == 0 {
            return Err("max_personas_per_session must be greater than 0".into());
        }
        if self.max_rounds == 0 {
            return Err("max_rounds must be greater than 0".into());
        }
        Ok(())
    }
}

// =============================================================================
// PersonaConfig
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PersonaConfig {
    pub id: String,
    pub name: String,
    pub system_prompt: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
}

impl Default for PersonaConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            system_prompt: String::new(),
            provider: None,
            model: None,
            thinking_level: None,
        }
    }
}

impl PersonaConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("persona id must not be empty".into());
        }
        if self.name.trim().is_empty() {
            return Err("persona name must not be empty".into());
        }
        if self.system_prompt.trim().is_empty() {
            return Err("persona system_prompt must not be empty".into());
        }
        if self.system_prompt.len() > MAX_SYSTEM_PROMPT_LEN {
            return Err(format!(
                "persona system_prompt exceeds {} characters", MAX_SYSTEM_PROMPT_LEN
            ));
        }
        Ok(())
    }
}
```

Add to `core/src/config/types/mod.rs`:
```rust
pub mod group_chat;
```
and:
```rust
pub use group_chat::*;
```

Add to `core/src/config/structs.rs` Config struct (after `agents` field):
```rust
    /// Group chat configuration (multi-agent persona orchestration)
    #[serde(default)]
    pub group_chat: GroupChatConfig,
    /// Preset persona definitions for group chat
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub personas: Vec<PersonaConfig>,
```

Add to Config `Default::default()` (after `agents: AgentsConfig::default(),`):
```rust
            group_chat: GroupChatConfig::default(),
            personas: Vec::new(),
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib config::types::group_chat::tests -v`
Expected: All 9 tests PASS

**Step 5: Commit**

```bash
git add core/src/config/types/group_chat.rs core/src/config/types/mod.rs core/src/config/structs.rs
git commit -m "config: add GroupChatConfig and PersonaConfig types"
```

---

### Task 3: PersonaRegistry

Manages preset personas from config and resolves `PersonaSource` to concrete `Persona`.

**Files:**
- Create: `core/src/group_chat/persona.rs`
- Modify: `core/src/group_chat/mod.rs` (add module)

**Step 1: Write the failing test**

In `core/src/group_chat/persona.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::PersonaConfig;

    fn sample_persona_configs() -> Vec<PersonaConfig> {
        vec![
            PersonaConfig {
                id: "architect".into(),
                name: "架构师".into(),
                system_prompt: "You are an architect".into(),
                provider: Some("claude".into()),
                model: Some("claude-sonnet-4-20250514".into()),
                thinking_level: None,
            },
            PersonaConfig {
                id: "pm".into(),
                name: "产品经理".into(),
                system_prompt: "You are a product manager".into(),
                provider: None,
                model: None,
                thinking_level: None,
            },
        ]
    }

    #[test]
    fn test_load_from_configs() {
        let registry = PersonaRegistry::from_configs(&sample_persona_configs());
        assert_eq!(registry.len(), 2);
        assert!(registry.get("architect").is_some());
        assert!(registry.get("pm").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_resolve_preset() {
        let registry = PersonaRegistry::from_configs(&sample_persona_configs());
        let sources = vec![PersonaSource::Preset("architect".into())];
        let resolved = registry.resolve(&sources).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].id, "architect");
        assert_eq!(resolved[0].name, "架构师");
    }

    #[test]
    fn test_resolve_inline() {
        let registry = PersonaRegistry::from_configs(&[]);
        let inline = Persona {
            id: "custom".into(),
            name: "自定义".into(),
            system_prompt: "Custom prompt".into(),
            provider: None,
            model: None,
            thinking_level: None,
        };
        let sources = vec![PersonaSource::Inline(inline)];
        let resolved = registry.resolve(&sources).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].id, "custom");
    }

    #[test]
    fn test_resolve_preset_not_found() {
        let registry = PersonaRegistry::from_configs(&[]);
        let sources = vec![PersonaSource::Preset("nonexistent".into())];
        let result = registry.resolve(&sources);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_mixed() {
        let registry = PersonaRegistry::from_configs(&sample_persona_configs());
        let sources = vec![
            PersonaSource::Preset("architect".into()),
            PersonaSource::Inline(Persona {
                id: "security".into(),
                name: "安全专家".into(),
                system_prompt: "Security expert".into(),
                provider: None,
                model: None,
                thinking_level: None,
            }),
        ];
        let resolved = registry.resolve(&sources).unwrap();
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].id, "architect");
        assert_eq!(resolved[1].id, "security");
    }

    #[test]
    fn test_reload() {
        let mut registry = PersonaRegistry::from_configs(&sample_persona_configs());
        assert_eq!(registry.len(), 2);

        let new_configs = vec![PersonaConfig {
            id: "new".into(),
            name: "New".into(),
            system_prompt: "New prompt".into(),
            ..Default::default()
        }];
        registry.reload(&new_configs);
        assert_eq!(registry.len(), 1);
        assert!(registry.get("new").is_some());
        assert!(registry.get("architect").is_none());
    }

    #[test]
    fn test_list_presets() {
        let registry = PersonaRegistry::from_configs(&sample_persona_configs());
        let presets = registry.list_presets();
        assert_eq!(presets.len(), 2);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib group_chat::persona::tests -- --no-run 2>&1 | head -10`
Expected: FAIL — module not found

**Step 3: Write the implementation**

`core/src/group_chat/persona.rs`:
```rust
//! Persona registry — manages preset + runtime personas

use std::collections::HashMap;
use crate::config::types::PersonaConfig;
use super::protocol::{Persona, PersonaSource, GroupChatError};

pub struct PersonaRegistry {
    presets: HashMap<String, Persona>,
}

impl PersonaRegistry {
    pub fn from_configs(configs: &[PersonaConfig]) -> Self {
        let mut presets = HashMap::new();
        for cfg in configs {
            presets.insert(cfg.id.clone(), persona_from_config(cfg));
        }
        Self { presets }
    }

    pub fn get(&self, id: &str) -> Option<&Persona> {
        self.presets.get(id)
    }

    pub fn len(&self) -> usize {
        self.presets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.presets.is_empty()
    }

    pub fn resolve(&self, sources: &[PersonaSource]) -> Result<Vec<Persona>, GroupChatError> {
        sources.iter().map(|s| match s {
            PersonaSource::Preset(id) => {
                self.presets.get(id)
                    .cloned()
                    .ok_or_else(|| GroupChatError::PersonaNotFound(id.clone()))
            }
            PersonaSource::Inline(persona) => Ok(persona.clone()),
        }).collect()
    }

    pub fn reload(&mut self, configs: &[PersonaConfig]) {
        self.presets.clear();
        for cfg in configs {
            self.presets.insert(cfg.id.clone(), persona_from_config(cfg));
        }
    }

    pub fn list_presets(&self) -> Vec<&Persona> {
        self.presets.values().collect()
    }
}

fn persona_from_config(cfg: &PersonaConfig) -> Persona {
    Persona {
        id: cfg.id.clone(),
        name: cfg.name.clone(),
        system_prompt: cfg.system_prompt.clone(),
        provider: cfg.provider.clone(),
        model: cfg.model.clone(),
        thinking_level: cfg.thinking_level.clone(),
    }
}
```

Update `core/src/group_chat/mod.rs`:
```rust
//! Multi-Agent Group Chat
//!
//! Channel-agnostic orchestration for multi-persona collaborative discussions.
//! See design doc: docs/plans/2026-03-04-multi-agent-group-chat-design.md

pub mod protocol;
pub mod persona;

pub use protocol::*;
pub use persona::PersonaRegistry;
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib group_chat::persona::tests -v`
Expected: All 7 tests PASS

**Step 5: Commit**

```bash
git add core/src/group_chat/persona.rs core/src/group_chat/mod.rs
git commit -m "group_chat: add PersonaRegistry with preset/inline resolution"
```

---

### Task 4: GroupChatSession & SQLite Persistence

Session state management and database CRUD.

**Files:**
- Create: `core/src/group_chat/session.rs`
- Create: `core/src/resilience/database/group_chat.rs`
- Modify: `core/src/resilience/database/mod.rs` (add module)
- Modify: `core/src/resilience/database/state_database.rs` (add schema)
- Modify: `core/src/group_chat/mod.rs` (add module)

**Step 1: Write the failing test for session types**

In `core/src/group_chat/session.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::group_chat::protocol::{Speaker, Persona};

    #[test]
    fn test_session_creation() {
        let personas = vec![Persona {
            id: "arch".into(),
            name: "架构师".into(),
            system_prompt: "prompt".into(),
            provider: None,
            model: None,
            thinking_level: None,
        }];
        let session = GroupChatSession::new(
            "s1".into(),
            Some("API设计".into()),
            personas,
            "telegram".into(),
            "agent:main:telegram:dm:123".into(),
        );
        assert_eq!(session.id, "s1");
        assert_eq!(session.current_round, 0);
        assert_eq!(session.status, GroupChatStatus::Active);
        assert!(session.history.is_empty());
    }

    #[test]
    fn test_add_turn() {
        let mut session = GroupChatSession::new(
            "s1".into(), None, vec![], "cli".into(), "main".into(),
        );
        session.add_turn(1, Speaker::System, "hello".into());
        assert_eq!(session.history.len(), 1);
        assert_eq!(session.history[0].round, 1);
        assert_eq!(session.history[0].content, "hello");
    }

    #[test]
    fn test_build_history_text() {
        let mut session = GroupChatSession::new(
            "s1".into(), None, vec![], "cli".into(), "main".into(),
        );
        session.add_turn(1, Speaker::Persona { id: "arch".into(), name: "架构师".into() }, "建议用gRPC".into());
        session.add_turn(1, Speaker::Persona { id: "sec".into(), name: "安全专家".into() }, "需要mTLS".into());

        let text = session.build_history_text();
        assert!(text.contains("[架构师]"));
        assert!(text.contains("建议用gRPC"));
        assert!(text.contains("[安全专家]"));
    }

    #[test]
    fn test_end_session() {
        let mut session = GroupChatSession::new(
            "s1".into(), None, vec![], "cli".into(), "main".into(),
        );
        assert_eq!(session.status, GroupChatStatus::Active);
        session.end();
        assert_eq!(session.status, GroupChatStatus::Ended);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib group_chat::session::tests -- --no-run 2>&1 | head -10`
Expected: FAIL — module not found

**Step 3: Write the session implementation**

`core/src/group_chat/session.rs`:
```rust
//! Group chat session state management

use super::protocol::{GroupChatStatus, Persona, Speaker};

#[derive(Debug, Clone)]
pub struct GroupChatTurn {
    pub round: u32,
    pub speaker: Speaker,
    pub content: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone)]
pub struct GroupChatSession {
    pub id: String,
    pub topic: Option<String>,
    pub participants: Vec<Persona>,
    pub history: Vec<GroupChatTurn>,
    pub current_round: u32,
    pub status: GroupChatStatus,
    pub created_at: i64,
    pub source_channel: String,
    pub source_session_key: String,
}

impl GroupChatSession {
    pub fn new(
        id: String,
        topic: Option<String>,
        participants: Vec<Persona>,
        source_channel: String,
        source_session_key: String,
    ) -> Self {
        Self {
            id,
            topic,
            participants,
            history: Vec::new(),
            current_round: 0,
            status: GroupChatStatus::Active,
            created_at: chrono::Utc::now().timestamp(),
            source_channel,
            source_session_key,
        }
    }

    pub fn add_turn(&mut self, round: u32, speaker: Speaker, content: String) {
        self.history.push(GroupChatTurn {
            round,
            speaker,
            content,
            timestamp: chrono::Utc::now().timestamp(),
        });
        if round > self.current_round {
            self.current_round = round;
        }
    }

    pub fn build_history_text(&self) -> String {
        let mut text = String::new();
        for turn in &self.history {
            text.push_str(&format!("[{}]: {}\n\n", turn.speaker.name(), turn.content));
        }
        text
    }

    pub fn end(&mut self) {
        self.status = GroupChatStatus::Ended;
    }
}
```

**Step 4: Write SQLite CRUD**

`core/src/resilience/database/group_chat.rs`:
```rust
//! CRUD operations for group_chat_sessions and group_chat_turns tables

use crate::error::AlephError;
use super::StateDatabase;
use rusqlite::params;

impl StateDatabase {
    pub fn insert_group_chat_session(
        &self,
        id: &str,
        topic: Option<&str>,
        source_channel: &str,
        source_session_key: &str,
    ) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO group_chat_sessions (id, topic, status, source_channel, source_session_key, created_at, updated_at)
             VALUES (?1, ?2, 'active', ?3, ?4, ?5, ?5)",
            params![id, topic, source_channel, source_session_key, now],
        ).map_err(|e| AlephError::config(format!("Failed to insert group chat session: {}", e)))?;
        Ok(())
    }

    pub fn update_group_chat_session_status(
        &self,
        session_id: &str,
        status: &str,
    ) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "UPDATE group_chat_sessions SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status, now, session_id],
        ).map_err(|e| AlephError::config(format!("Failed to update session status: {}", e)))?;
        Ok(())
    }

    pub fn insert_group_chat_turn(
        &self,
        session_id: &str,
        round: u32,
        sequence: u32,
        speaker_type: &str,
        speaker_id: Option<&str>,
        speaker_name: &str,
        content: &str,
    ) -> Result<(), AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO group_chat_turns (session_id, round, sequence, speaker_type, speaker_id, speaker_name, content, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![session_id, round, sequence, speaker_type, speaker_id, speaker_name, content, now],
        ).map_err(|e| AlephError::config(format!("Failed to insert group chat turn: {}", e)))?;
        Ok(())
    }

    pub fn get_group_chat_turns(
        &self,
        session_id: &str,
    ) -> Result<Vec<(u32, u32, String, Option<String>, String, String, i64)>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT round, sequence, speaker_type, speaker_id, speaker_name, content, timestamp
             FROM group_chat_turns WHERE session_id = ?1 ORDER BY round, sequence"
        ).map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let rows = stmt.query_map(params![session_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        }).map_err(|e| AlephError::config(format!("Failed to query turns: {}", e)))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to collect turns: {}", e)))
    }

    pub fn list_active_group_chats(
        &self,
    ) -> Result<Vec<(String, Option<String>, String, i64)>, AlephError> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT id, topic, source_channel, created_at
             FROM group_chat_sessions WHERE status = 'active' ORDER BY created_at DESC"
        ).map_err(|e| AlephError::config(format!("Failed to prepare query: {}", e)))?;

        let rows = stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        }).map_err(|e| AlephError::config(format!("Failed to query sessions: {}", e)))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| AlephError::config(format!("Failed to collect sessions: {}", e)))
    }
}
```

Add group chat tables to `core/src/resilience/database/state_database.rs` `schema_sql()` — append before the closing `"#`:

```sql
            -- ================================================================
            -- Group Chat Tables
            -- ================================================================

            CREATE TABLE IF NOT EXISTS group_chat_sessions (
                id TEXT PRIMARY KEY,
                topic TEXT,
                status TEXT NOT NULL DEFAULT 'active',
                source_channel TEXT NOT NULL,
                source_session_key TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS group_chat_turns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES group_chat_sessions(id),
                round INTEGER NOT NULL,
                sequence INTEGER NOT NULL,
                speaker_type TEXT NOT NULL,
                speaker_id TEXT,
                speaker_name TEXT NOT NULL,
                content TEXT NOT NULL,
                timestamp INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_gc_turns_session ON group_chat_turns(session_id);
```

Add to `core/src/resilience/database/mod.rs`:
```rust
mod group_chat;
```

Update `core/src/group_chat/mod.rs`:
```rust
pub mod session;
pub use session::GroupChatSession;
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib group_chat::session::tests -v`
Expected: All 4 tests PASS

Then run: `cargo check -p alephcore` to verify no compilation errors.

**Step 5: Commit**

```bash
git add core/src/group_chat/session.rs core/src/group_chat/mod.rs \
        core/src/resilience/database/group_chat.rs core/src/resilience/database/mod.rs \
        core/src/resilience/database/state_database.rs
git commit -m "group_chat: add session state management and SQLite persistence"
```

---

### Task 5: Coordinator — LLM-Based Orchestration Planner

The Coordinator analyzes user messages and decides which personas should respond.

**Files:**
- Create: `core/src/group_chat/coordinator.rs`
- Modify: `core/src/group_chat/mod.rs` (add module)

**Step 1: Write the failing test**

In `core/src/group_chat/coordinator.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_coordinator_prompt() {
        let personas = vec![
            Persona {
                id: "arch".into(),
                name: "架构师".into(),
                system_prompt: "Software architect".into(),
                provider: None, model: None, thinking_level: None,
            },
            Persona {
                id: "pm".into(),
                name: "产品经理".into(),
                system_prompt: "Product manager".into(),
                provider: None, model: None, thinking_level: None,
            },
        ];
        let prompt = build_coordinator_prompt(
            &personas,
            "这个API设计怎么样?",
            "",
            &None,
        );
        assert!(prompt.contains("架构师"));
        assert!(prompt.contains("产品经理"));
        assert!(prompt.contains("这个API设计怎么样?"));
        assert!(prompt.contains("JSON"));
    }

    #[test]
    fn test_parse_coordinator_plan_valid() {
        let json = r#"{"respondents":[{"persona_id":"arch","order":1,"guidance":"Focus on tech"},{"persona_id":"pm","order":2,"guidance":"Focus on UX"}],"need_summary":true}"#;
        let plan = parse_coordinator_plan(json).unwrap();
        assert_eq!(plan.respondents.len(), 2);
        assert_eq!(plan.respondents[0].persona_id, "arch");
        assert!(plan.need_summary);
    }

    #[test]
    fn test_parse_coordinator_plan_with_markdown_wrapper() {
        let json = "```json\n{\"respondents\":[{\"persona_id\":\"arch\",\"order\":1,\"guidance\":\"tech\"}],\"need_summary\":false}\n```";
        let plan = parse_coordinator_plan(json).unwrap();
        assert_eq!(plan.respondents.len(), 1);
    }

    #[test]
    fn test_parse_coordinator_plan_invalid() {
        let result = parse_coordinator_plan("not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn test_build_fallback_plan() {
        let personas = vec![
            Persona { id: "a".into(), name: "A".into(), system_prompt: "".into(), provider: None, model: None, thinking_level: None },
            Persona { id: "b".into(), name: "B".into(), system_prompt: "".into(), provider: None, model: None, thinking_level: None },
        ];
        let plan = build_fallback_plan(&personas);
        assert_eq!(plan.respondents.len(), 2);
        assert_eq!(plan.respondents[0].persona_id, "a");
        assert_eq!(plan.respondents[0].order, 0);
        assert_eq!(plan.respondents[1].order, 1);
        assert!(!plan.need_summary);
    }

    #[test]
    fn test_build_persona_prompt() {
        let persona = Persona {
            id: "arch".into(),
            name: "架构师".into(),
            system_prompt: "You are a senior architect".into(),
            provider: None, model: None, thinking_level: None,
        };
        let prompt = build_persona_prompt(
            &persona,
            "API设计怎么样?",
            "之前的讨论内容",
            "关注技术可行性",
        );
        assert!(prompt.contains("架构师"));
        assert!(prompt.contains("You are a senior architect"));
        assert!(prompt.contains("API设计怎么样?"));
        assert!(prompt.contains("之前的讨论内容"));
        assert!(prompt.contains("关注技术可行性"));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib group_chat::coordinator::tests -- --no-run 2>&1 | head -10`
Expected: FAIL — module not found

**Step 3: Write the implementation**

`core/src/group_chat/coordinator.rs`:
```rust
//! Coordinator — LLM-based orchestration planner
//!
//! Builds prompts for the Coordinator LLM call that decides which personas
//! respond and in what order, and builds per-persona prompts with cumulative context.

use super::protocol::{CoordinatorPlan, GroupChatError, Persona, RespondentPlan};

/// Build the system+user prompt for the Coordinator LLM call
pub fn build_coordinator_prompt(
    personas: &[Persona],
    user_message: &str,
    history: &str,
    topic: &Option<String>,
) -> String {
    let mut persona_list = String::new();
    for p in personas {
        persona_list.push_str(&format!("- {} (id: {}): {}\n", p.name, p.id,
            p.system_prompt.get(..80).unwrap_or(&p.system_prompt)));
    }

    let topic_line = topic.as_ref()
        .map(|t| format!("讨论主题: {}\n", t))
        .unwrap_or_default();

    let history_section = if history.is_empty() {
        String::new()
    } else {
        format!("之前的讨论历史:\n{}\n", history)
    };

    format!(
r#"你是一个群聊主持人。当前群聊有以下角色：
{persona_list}
{topic_line}{history_section}
用户说: "{user_message}"

请分析：
1. 哪些角色应该对此发言？（不必每次所有人都说话）
2. 发言顺序是什么？（通常专业对口的先说，综合角色后说）
3. 每个角色应该关注什么方面？（简短提示）
4. 是否需要最后的综合总结？

以 JSON 格式回复，不要添加任何其他文本：
{{"respondents":[{{"persona_id":"<id>","order":<number>,"guidance":"<brief focus>"}}],"need_summary":<bool>}}"#
    )
}

/// Parse the Coordinator LLM output into a structured plan
pub fn parse_coordinator_plan(raw: &str) -> Result<CoordinatorPlan, GroupChatError> {
    // Strip markdown code fence if present
    let cleaned = raw.trim();
    let json_str = if cleaned.starts_with("```") {
        cleaned
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim()
    } else {
        cleaned
    };

    serde_json::from_str::<CoordinatorPlan>(json_str)
        .map_err(|e| GroupChatError::CoordinatorPlanParseError(
            format!("Failed to parse coordinator plan: {}. Raw: {}", e, &raw[..raw.len().min(200)])
        ))
}

/// Build a fallback plan when the Coordinator fails — all personas respond in config order
pub fn build_fallback_plan(personas: &[Persona]) -> CoordinatorPlan {
    CoordinatorPlan {
        respondents: personas.iter().enumerate().map(|(i, p)| RespondentPlan {
            persona_id: p.id.clone(),
            order: i as u32,
            guidance: String::new(),
        }).collect(),
        need_summary: false,
    }
}

/// Build the prompt for a persona's LLM call with cumulative context
pub fn build_persona_prompt(
    persona: &Persona,
    user_message: &str,
    prior_discussion: &str,
    guidance: &str,
) -> String {
    let guidance_line = if guidance.is_empty() {
        String::new()
    } else {
        format!("\n主持人提示你关注: {}\n", guidance)
    };

    let prior_section = if prior_discussion.is_empty() {
        String::new()
    } else {
        format!("\n其他角色已发表的观点:\n{}\n", prior_discussion)
    };

    format!(
r#"你是「{name}」。{system_prompt}

你正在参与一个多角色群聊讨论。{guidance_line}{prior_section}
用户的问题: {user_message}

请以「{name}」的身份和专业视角回复。如果其他角色已发表观点，你可以引用、补充或提出不同意见。回复要简洁有深度。"#,
        name = persona.name,
        system_prompt = persona.system_prompt,
        guidance_line = guidance_line,
        prior_section = prior_section,
        user_message = user_message,
    )
}
```

Update `core/src/group_chat/mod.rs` to add:
```rust
pub mod coordinator;
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib group_chat::coordinator::tests -v`
Expected: All 6 tests PASS

**Step 5: Commit**

```bash
git add core/src/group_chat/coordinator.rs core/src/group_chat/mod.rs
git commit -m "group_chat: add Coordinator prompt builder and plan parser"
```

---

### Task 6: GroupChatOrchestrator

The core orchestrator that ties everything together.

**Files:**
- Create: `core/src/group_chat/orchestrator.rs`
- Modify: `core/src/group_chat/mod.rs` (add module + exports)

**Step 1: Write the failing test**

In `core/src/group_chat/orchestrator.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{GroupChatConfig, PersonaConfig};

    fn test_config() -> GroupChatConfig {
        GroupChatConfig {
            max_personas_per_session: 4,
            max_rounds: 3,
            ..Default::default()
        }
    }

    fn test_personas() -> Vec<PersonaConfig> {
        vec![
            PersonaConfig {
                id: "arch".into(),
                name: "架构师".into(),
                system_prompt: "You are an architect".into(),
                ..Default::default()
            },
            PersonaConfig {
                id: "pm".into(),
                name: "产品经理".into(),
                system_prompt: "You are a PM".into(),
                ..Default::default()
            },
        ]
    }

    #[test]
    fn test_orchestrator_creation() {
        let orch = GroupChatOrchestrator::new(test_config(), &test_personas());
        assert_eq!(orch.active_session_count(), 0);
    }

    #[test]
    fn test_create_session() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());
        let sources = vec![
            PersonaSource::Preset("arch".into()),
            PersonaSource::Preset("pm".into()),
        ];
        let result = orch.create_session(
            sources,
            Some("test topic".into()),
            "telegram".into(),
            "session:key".into(),
        );
        assert!(result.is_ok());
        let session_id = result.unwrap();
        assert_eq!(orch.active_session_count(), 1);
        assert!(orch.get_session(&session_id).is_some());
    }

    #[test]
    fn test_create_session_preset_not_found() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());
        let sources = vec![PersonaSource::Preset("nonexistent".into())];
        let result = orch.create_session(sources, None, "cli".into(), "main".into());
        assert!(result.is_err());
    }

    #[test]
    fn test_create_session_too_many_personas() {
        let config = GroupChatConfig {
            max_personas_per_session: 1,
            ..Default::default()
        };
        let mut orch = GroupChatOrchestrator::new(config, &test_personas());
        let sources = vec![
            PersonaSource::Preset("arch".into()),
            PersonaSource::Preset("pm".into()),
        ];
        let result = orch.create_session(sources, None, "cli".into(), "main".into());
        assert!(result.is_err());
    }

    #[test]
    fn test_end_session() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());
        let sources = vec![PersonaSource::Preset("arch".into())];
        let session_id = orch.create_session(sources, None, "cli".into(), "main".into()).unwrap();

        assert!(orch.end_session(&session_id).is_ok());
        let session = orch.get_session(&session_id).unwrap();
        assert_eq!(session.status, GroupChatStatus::Ended);
    }

    #[test]
    fn test_end_session_not_found() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());
        assert!(orch.end_session("nonexistent").is_err());
    }

    #[test]
    fn test_list_active_sessions() {
        let mut orch = GroupChatOrchestrator::new(test_config(), &test_personas());
        let sources = vec![PersonaSource::Preset("arch".into())];
        let s1 = orch.create_session(sources.clone(), Some("t1".into()), "tg".into(), "k1".into()).unwrap();
        let _s2 = orch.create_session(sources, Some("t2".into()), "tg".into(), "k2".into()).unwrap();
        orch.end_session(&s1).unwrap();

        let active = orch.list_active_sessions();
        assert_eq!(active.len(), 1);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib group_chat::orchestrator::tests -- --no-run 2>&1 | head -10`
Expected: FAIL — module not found

**Step 3: Write the implementation**

`core/src/group_chat/orchestrator.rs`:
```rust
//! GroupChat Orchestrator — ties together persona registry, sessions, and coordination

use std::collections::HashMap;
use crate::config::types::{GroupChatConfig, PersonaConfig};
use super::persona::PersonaRegistry;
use super::protocol::{GroupChatError, GroupChatStatus, PersonaSource};
use super::session::GroupChatSession;

pub struct GroupChatOrchestrator {
    config: GroupChatConfig,
    persona_registry: PersonaRegistry,
    sessions: HashMap<String, GroupChatSession>,
}

impl GroupChatOrchestrator {
    pub fn new(config: GroupChatConfig, persona_configs: &[PersonaConfig]) -> Self {
        Self {
            config,
            persona_registry: PersonaRegistry::from_configs(persona_configs),
            sessions: HashMap::new(),
        }
    }

    pub fn config(&self) -> &GroupChatConfig {
        &self.config
    }

    pub fn persona_registry(&self) -> &PersonaRegistry {
        &self.persona_registry
    }

    pub fn create_session(
        &mut self,
        sources: Vec<PersonaSource>,
        topic: Option<String>,
        source_channel: String,
        source_session_key: String,
    ) -> Result<String, GroupChatError> {
        // Validate persona count
        if sources.len() > self.config.max_personas_per_session {
            return Err(GroupChatError::TooManyPersonas {
                max: self.config.max_personas_per_session,
                requested: sources.len(),
            });
        }

        // Resolve personas
        let personas = self.persona_registry.resolve(&sources)?;

        // Create session
        let session_id = uuid::Uuid::new_v4().to_string();
        let session = GroupChatSession::new(
            session_id.clone(),
            topic,
            personas,
            source_channel,
            source_session_key,
        );
        self.sessions.insert(session_id.clone(), session);
        Ok(session_id)
    }

    pub fn get_session(&self, session_id: &str) -> Option<&GroupChatSession> {
        self.sessions.get(session_id)
    }

    pub fn get_session_mut(&mut self, session_id: &str) -> Option<&mut GroupChatSession> {
        self.sessions.get_mut(session_id)
    }

    pub fn end_session(&mut self, session_id: &str) -> Result<(), GroupChatError> {
        let session = self.sessions.get_mut(session_id)
            .ok_or_else(|| GroupChatError::SessionNotFound(session_id.into()))?;
        session.end();
        Ok(())
    }

    pub fn active_session_count(&self) -> usize {
        self.sessions.values()
            .filter(|s| s.status == GroupChatStatus::Active)
            .count()
    }

    pub fn list_active_sessions(&self) -> Vec<&GroupChatSession> {
        self.sessions.values()
            .filter(|s| s.status == GroupChatStatus::Active)
            .collect()
    }

    pub fn check_round_limit(&self, session_id: &str) -> Result<(), GroupChatError> {
        if let Some(session) = self.sessions.get(session_id) {
            if session.current_round as usize >= self.config.max_rounds {
                return Err(GroupChatError::MaxRoundsReached {
                    session_id: session_id.into(),
                    max: self.config.max_rounds,
                });
            }
        }
        Ok(())
    }

    pub fn reload_config(&mut self, config: GroupChatConfig, persona_configs: &[PersonaConfig]) {
        self.config = config;
        self.persona_registry.reload(persona_configs);
    }
}
```

Update `core/src/group_chat/mod.rs`:
```rust
//! Multi-Agent Group Chat
//!
//! Channel-agnostic orchestration for multi-persona collaborative discussions.
//! See design doc: docs/plans/2026-03-04-multi-agent-group-chat-design.md

pub mod protocol;
pub mod persona;
pub mod session;
pub mod coordinator;
pub mod orchestrator;

pub use protocol::*;
pub use persona::PersonaRegistry;
pub use session::GroupChatSession;
pub use orchestrator::GroupChatOrchestrator;
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib group_chat::orchestrator::tests -v`
Expected: All 7 tests PASS

**Step 5: Commit**

```bash
git add core/src/group_chat/orchestrator.rs core/src/group_chat/mod.rs
git commit -m "group_chat: add GroupChatOrchestrator with session lifecycle management"
```

---

### Task 7: Gateway RPC Handlers

Register group_chat.* RPC methods in the gateway handler registry.

**Files:**
- Create: `core/src/gateway/handlers/group_chat.rs`
- Modify: `core/src/gateway/handlers/mod.rs` (add module + register handlers)

**Step 1: Write the failing test**

In `core/src/gateway/handlers/group_chat.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use crate::gateway::protocol::JsonRpcRequest;

    #[test]
    fn test_group_chat_handlers_registered() {
        let registry = crate::gateway::handlers::HandlerRegistry::new();
        assert!(registry.has_method("group_chat.start"));
        assert!(registry.has_method("group_chat.continue"));
        assert!(registry.has_method("group_chat.end"));
        assert!(registry.has_method("group_chat.list"));
    }

    #[tokio::test]
    async fn test_start_placeholder_returns_error() {
        let registry = crate::gateway::handlers::HandlerRegistry::new();
        let req = JsonRpcRequest::with_id("group_chat.start", Some(json!({"personas": [], "initial_message": "hi"})), json!(1));
        let resp = registry.handle(&req).await;
        assert!(resp.is_error());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::handlers::group_chat::tests -- --no-run 2>&1 | head -10`
Expected: FAIL — module not found

**Step 3: Write the implementation**

`core/src/gateway/handlers/group_chat.rs`:
```rust
//! Group Chat RPC handlers
//!
//! JSON-RPC method handlers for multi-agent group chat operations.
//! These are placeholder handlers — actual handlers are wired with
//! GroupChatOrchestrator runtime in Gateway::new().

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};

const RUNTIME_REQUIRED: &str = "requires GroupChatOrchestrator runtime - wire Gateway first";

pub async fn handle_start_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("group_chat.start {}", RUNTIME_REQUIRED))
}

pub async fn handle_continue_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("group_chat.continue {}", RUNTIME_REQUIRED))
}

pub async fn handle_mention_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("group_chat.mention {}", RUNTIME_REQUIRED))
}

pub async fn handle_end_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("group_chat.end {}", RUNTIME_REQUIRED))
}

pub async fn handle_list_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("group_chat.list {}", RUNTIME_REQUIRED))
}

pub async fn handle_history_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(req.id, INTERNAL_ERROR, format!("group_chat.history {}", RUNTIME_REQUIRED))
}
```

Add to `core/src/gateway/handlers/mod.rs` (after `pub mod discord_panel;`):
```rust
pub mod group_chat;
```

Register placeholders in `HandlerRegistry::new()` (after workspace handlers, before `registry` return):
```rust
        // Group Chat handlers (placeholders - actual handlers wired with GroupChatOrchestrator)
        registry.register("group_chat.start", group_chat::handle_start_placeholder);
        registry.register("group_chat.continue", group_chat::handle_continue_placeholder);
        registry.register("group_chat.mention", group_chat::handle_mention_placeholder);
        registry.register("group_chat.end", group_chat::handle_end_placeholder);
        registry.register("group_chat.list", group_chat::handle_list_placeholder);
        registry.register("group_chat.history", group_chat::handle_history_placeholder);
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib gateway::handlers::group_chat::tests -v`
Expected: All 2 tests PASS

Also run: `cargo test -p alephcore --lib gateway::handlers::tests -v`
Expected: Existing handler registry tests still pass (verify group_chat methods are in methods list)

**Step 5: Commit**

```bash
git add core/src/gateway/handlers/group_chat.rs core/src/gateway/handlers/mod.rs
git commit -m "gateway: register group_chat.* RPC handler placeholders"
```

---

### Task 8: Channel Adapter Traits

Define the `GroupChatRenderer` and `GroupChatCommandParser` traits in core.

**Files:**
- Create: `core/src/group_chat/channel.rs`
- Modify: `core/src/group_chat/mod.rs` (add module)

**Step 1: Write the failing test**

In `core/src/group_chat/channel.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::group_chat::protocol::*;

    /// Test implementation of GroupChatRenderer for verification
    struct TestRenderer;

    impl GroupChatRenderer for TestRenderer {
        fn render_message(&self, msg: &GroupChatMessage) -> RenderedContent {
            RenderedContent::plain(format!("[{}]: {}", msg.speaker.name(), msg.content))
        }

        fn render_session_start(&self, participants: &[Persona], topic: Option<&str>) -> RenderedContent {
            let names: Vec<&str> = participants.iter().map(|p| p.name.as_str()).collect();
            let topic_str = topic.unwrap_or("(no topic)");
            RenderedContent::plain(format!("Group chat started: {} - {}", names.join(", "), topic_str))
        }

        fn render_session_end(&self, _session_id: &str) -> RenderedContent {
            RenderedContent::plain("Group chat ended".into())
        }

        fn render_typing(&self, persona: &Persona) -> Option<RenderedContent> {
            Some(RenderedContent::plain(format!("{} is thinking...", persona.name)))
        }
    }

    #[test]
    fn test_render_message() {
        let renderer = TestRenderer;
        let msg = GroupChatMessage {
            session_id: "s1".into(),
            speaker: Speaker::Persona { id: "arch".into(), name: "架构师".into() },
            content: "建议用gRPC".into(),
            round: 1, sequence: 0, is_final: false,
        };
        let rendered = renderer.render_message(&msg);
        assert_eq!(rendered.text, "[架构师]: 建议用gRPC");
    }

    #[test]
    fn test_render_typing() {
        let renderer = TestRenderer;
        let persona = Persona {
            id: "arch".into(), name: "架构师".into(), system_prompt: "".into(),
            provider: None, model: None, thinking_level: None,
        };
        let rendered = renderer.render_typing(&persona).unwrap();
        assert!(rendered.text.contains("架构师"));
    }

    /// Test implementation of GroupChatCommandParser
    struct TestParser;

    impl GroupChatCommandParser for TestParser {
        fn parse_group_chat_command(&self, raw: &str) -> Option<GroupChatRequest> {
            if raw.starts_with("/groupchat start") {
                Some(GroupChatRequest::Start {
                    personas: vec![],
                    topic: None,
                    initial_message: raw.into(),
                })
            } else if raw.starts_with("/groupchat end") {
                Some(GroupChatRequest::End { session_id: "test".into() })
            } else {
                None
            }
        }
    }

    #[test]
    fn test_parse_start_command() {
        let parser = TestParser;
        let result = parser.parse_group_chat_command("/groupchat start --topic test");
        assert!(matches!(result, Some(GroupChatRequest::Start { .. })));
    }

    #[test]
    fn test_parse_non_command() {
        let parser = TestParser;
        let result = parser.parse_group_chat_command("hello world");
        assert!(result.is_none());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib group_chat::channel::tests -- --no-run 2>&1 | head -10`
Expected: FAIL — module not found

**Step 3: Write the implementation**

`core/src/group_chat/channel.rs`:
```rust
//! Channel adapter traits for group chat
//!
//! These traits define the contract between Core and Channel layers.
//! Each channel (Telegram, Discord, CLI) implements these traits
//! to provide channel-specific rendering and command parsing.

use super::protocol::{GroupChatMessage, GroupChatRequest, Persona, RenderedContent};

/// Renders group chat messages in channel-specific format
pub trait GroupChatRenderer: Send + Sync {
    /// Render a single persona message
    fn render_message(&self, msg: &GroupChatMessage) -> RenderedContent;

    /// Render session start announcement
    fn render_session_start(&self, participants: &[Persona], topic: Option<&str>) -> RenderedContent;

    /// Render session end announcement
    fn render_session_end(&self, session_id: &str) -> RenderedContent;

    /// Render typing indicator for a persona (None = channel doesn't support it)
    fn render_typing(&self, persona: &Persona) -> Option<RenderedContent>;
}

/// Parses channel-specific commands into GroupChatRequest
pub trait GroupChatCommandParser: Send + Sync {
    /// Check if a raw message is a group chat command and parse it
    fn parse_group_chat_command(&self, raw_message: &str) -> Option<GroupChatRequest>;
}
```

Update `core/src/group_chat/mod.rs` to add:
```rust
pub mod channel;
pub use channel::{GroupChatRenderer, GroupChatCommandParser};
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib group_chat::channel::tests -v`
Expected: All 4 tests PASS

**Step 5: Commit**

```bash
git add core/src/group_chat/channel.rs core/src/group_chat/mod.rs
git commit -m "group_chat: add GroupChatRenderer and GroupChatCommandParser traits"
```

---

### Task 9: Telegram Channel Adapter

Implement `GroupChatRenderer` and `GroupChatCommandParser` for Telegram.

**Files:**
- Create: `core/src/gateway/interfaces/telegram/group_chat.rs`
- Modify: `core/src/gateway/interfaces/telegram/mod.rs` (add module)

**Step 1: Write the failing test**

In `core/src/gateway/interfaces/telegram/group_chat.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::group_chat::protocol::*;

    #[test]
    fn test_render_persona_message() {
        let renderer = TelegramGroupChatRenderer;
        let msg = GroupChatMessage {
            session_id: "s1".into(),
            speaker: Speaker::Persona { id: "arch".into(), name: "架构师".into() },
            content: "建议用gRPC".into(),
            round: 1, sequence: 0, is_final: false,
        };
        let rendered = renderer.render_message(&msg);
        assert_eq!(rendered.format, ContentFormat::Markdown);
        assert!(rendered.text.contains("**[架构师]**"));
        assert!(rendered.text.contains("建议用gRPC"));
    }

    #[test]
    fn test_render_system_message() {
        let renderer = TelegramGroupChatRenderer;
        let msg = GroupChatMessage {
            session_id: "s1".into(),
            speaker: Speaker::System,
            content: "群聊已结束".into(),
            round: 0, sequence: 0, is_final: true,
        };
        let rendered = renderer.render_message(&msg);
        assert!(rendered.text.contains("群聊已结束"));
    }

    #[test]
    fn test_render_session_start() {
        let renderer = TelegramGroupChatRenderer;
        let personas = vec![
            Persona { id: "arch".into(), name: "架构师".into(), system_prompt: "".into(), provider: None, model: None, thinking_level: None },
            Persona { id: "pm".into(), name: "产品经理".into(), system_prompt: "".into(), provider: None, model: None, thinking_level: None },
        ];
        let rendered = renderer.render_session_start(&personas, Some("API设计"));
        assert!(rendered.text.contains("架构师"));
        assert!(rendered.text.contains("产品经理"));
        assert!(rendered.text.contains("API设计"));
    }

    #[test]
    fn test_render_typing() {
        let renderer = TelegramGroupChatRenderer;
        let persona = Persona { id: "arch".into(), name: "架构师".into(), system_prompt: "".into(), provider: None, model: None, thinking_level: None };
        let rendered = renderer.render_typing(&persona).unwrap();
        assert!(rendered.text.contains("架构师"));
    }

    #[test]
    fn test_parse_start_command() {
        let parser = TelegramGroupChatCommandParser;
        let result = parser.parse_group_chat_command("/groupchat start --preset architect,pm --topic API设计 这个API怎么样?");
        assert!(result.is_some());
        if let Some(GroupChatRequest::Start { personas, topic, initial_message }) = result {
            assert_eq!(personas.len(), 2);
            assert_eq!(topic, Some("API设计".into()));
            assert_eq!(initial_message, "这个API怎么样?");
        }
    }

    #[test]
    fn test_parse_start_with_inline_role() {
        let parser = TelegramGroupChatCommandParser;
        let result = parser.parse_group_chat_command(r#"/groupchat start --role "安全专家: 严谨的安全审计员" 检查这个PR"#);
        assert!(result.is_some());
        if let Some(GroupChatRequest::Start { personas, .. }) = result {
            assert_eq!(personas.len(), 1);
            matches!(&personas[0], PersonaSource::Inline(_));
        }
    }

    #[test]
    fn test_parse_end_command() {
        let parser = TelegramGroupChatCommandParser;
        let result = parser.parse_group_chat_command("/groupchat end abc123");
        assert!(matches!(result, Some(GroupChatRequest::End { session_id }) if session_id == "abc123"));
    }

    #[test]
    fn test_parse_non_groupchat_command() {
        let parser = TelegramGroupChatCommandParser;
        assert!(parser.parse_group_chat_command("hello world").is_none());
        assert!(parser.parse_group_chat_command("/other command").is_none());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib gateway::interfaces::telegram::group_chat::tests -- --no-run 2>&1 | head -10`
Expected: FAIL — module not found

**Step 3: Write the implementation**

`core/src/gateway/interfaces/telegram/group_chat.rs`:
```rust
//! Telegram-specific group chat rendering and command parsing

use crate::group_chat::channel::{GroupChatCommandParser, GroupChatRenderer};
use crate::group_chat::protocol::*;

// =============================================================================
// Renderer
// =============================================================================

pub struct TelegramGroupChatRenderer;

impl GroupChatRenderer for TelegramGroupChatRenderer {
    fn render_message(&self, msg: &GroupChatMessage) -> RenderedContent {
        let text = match &msg.speaker {
            Speaker::Persona { name, .. } => format!("**[{}]**: {}", name, msg.content),
            Speaker::Coordinator => format!("**[主持人]**: {}", msg.content),
            Speaker::System => format!("_{}_", msg.content),
        };
        RenderedContent::markdown(text)
    }

    fn render_session_start(&self, participants: &[Persona], topic: Option<&str>) -> RenderedContent {
        let names: Vec<&str> = participants.iter().map(|p| p.name.as_str()).collect();
        let topic_line = topic
            .map(|t| format!("\n**主题**: {}", t))
            .unwrap_or_default();
        let text = format!(
            "🎭 **群聊模式已开启**\n**参与者**: {}{}\n\n_发送消息即可开始讨论，发送 /groupchat end 结束_",
            names.join(", "),
            topic_line,
        );
        RenderedContent::markdown(text)
    }

    fn render_session_end(&self, _session_id: &str) -> RenderedContent {
        RenderedContent::markdown("🎭 **群聊模式已结束**".into())
    }

    fn render_typing(&self, persona: &Persona) -> Option<RenderedContent> {
        Some(RenderedContent::plain(format!("💭 {} 正在思考...", persona.name)))
    }
}

// =============================================================================
// Command Parser
// =============================================================================

pub struct TelegramGroupChatCommandParser;

impl GroupChatCommandParser for TelegramGroupChatCommandParser {
    fn parse_group_chat_command(&self, raw: &str) -> Option<GroupChatRequest> {
        let trimmed = raw.trim();
        if !trimmed.starts_with("/groupchat") {
            return None;
        }

        let after_prefix = trimmed.strip_prefix("/groupchat")?.trim();

        if after_prefix.starts_with("start") {
            parse_start_command(after_prefix.strip_prefix("start")?.trim())
        } else if after_prefix.starts_with("end") {
            let session_id = after_prefix.strip_prefix("end")?.trim();
            Some(GroupChatRequest::End {
                session_id: session_id.to_string(),
            })
        } else {
            None
        }
    }
}

fn parse_start_command(args: &str) -> Option<GroupChatRequest> {
    let mut personas = Vec::new();
    let mut topic = None;
    let mut remaining = args;
    let mut message_parts = Vec::new();

    while !remaining.is_empty() {
        remaining = remaining.trim();

        if remaining.starts_with("--preset") {
            remaining = remaining.strip_prefix("--preset")?.trim();
            let (preset_str, rest) = split_next_arg(remaining);
            for id in preset_str.split(',') {
                let id = id.trim();
                if !id.is_empty() {
                    personas.push(PersonaSource::Preset(id.to_string()));
                }
            }
            remaining = rest;
        } else if remaining.starts_with("--topic") {
            remaining = remaining.strip_prefix("--topic")?.trim();
            let (topic_str, rest) = split_next_arg(remaining);
            topic = Some(topic_str.to_string());
            remaining = rest;
        } else if remaining.starts_with("--role") {
            remaining = remaining.strip_prefix("--role")?.trim();
            let (role_str, rest) = split_quoted_arg(remaining);
            if let Some((name, prompt)) = role_str.split_once(':') {
                personas.push(PersonaSource::Inline(Persona {
                    id: name.trim().to_lowercase().replace(' ', "_"),
                    name: name.trim().to_string(),
                    system_prompt: prompt.trim().to_string(),
                    provider: None,
                    model: None,
                    thinking_level: None,
                }));
            }
            remaining = rest;
        } else {
            // Everything else is the initial message
            message_parts.push(remaining);
            break;
        }
    }

    let initial_message = message_parts.join(" ");
    if initial_message.is_empty() && personas.is_empty() {
        return None;
    }

    Some(GroupChatRequest::Start {
        personas,
        topic,
        initial_message,
    })
}

/// Split at next whitespace, respecting that the value might be unquoted
fn split_next_arg(s: &str) -> (&str, &str) {
    if s.starts_with('"') {
        // Quoted arg
        if let Some(end) = s[1..].find('"') {
            let value = &s[1..end + 1];
            let rest = s[end + 2..].trim_start();
            return (value, rest);
        }
    }
    match s.find(|c: char| c.is_whitespace() && c != '\u{00a0}') {
        Some(i) => (&s[..i], &s[i..].trim_start()),
        None => (s, ""),
    }
}

/// Split a quoted argument (expects opening quote)
fn split_quoted_arg(s: &str) -> (&str, &str) {
    if s.starts_with('"') {
        if let Some(end) = s[1..].find('"') {
            let value = &s[1..end + 1];
            let rest = s[end + 2..].trim_start();
            return (value, rest);
        }
    }
    split_next_arg(s)
}
```

Add to `core/src/gateway/interfaces/telegram/mod.rs`:
```rust
pub mod group_chat;
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib gateway::interfaces::telegram::group_chat::tests -v`
Expected: All 8 tests PASS

**Step 5: Commit**

```bash
git add core/src/gateway/interfaces/telegram/group_chat.rs core/src/gateway/interfaces/telegram/mod.rs
git commit -m "telegram: implement GroupChatRenderer and GroupChatCommandParser"
```

---

### Task 10: Integration Wiring & Compilation Check

Wire everything together: add `group_chat` module to lib.rs exports, ensure full compilation passes.

**Files:**
- Modify: `core/src/lib.rs` (add group_chat exports)
- Modify: `core/Cargo.toml` (add `thiserror` if not present, verify `uuid` has `v4` feature)

**Step 1: Check dependencies**

Run: `grep -E "thiserror|uuid|chrono" core/Cargo.toml`

If `thiserror` is missing, add it. If `uuid` lacks `v4` feature, add it.

**Step 2: Add public exports to lib.rs**

In `core/src/lib.rs`, after `pub mod group_chat;`, add in the exports section:
```rust
// =============================================================================
// Group Chat Exports
// =============================================================================

pub use crate::group_chat::{
    GroupChatOrchestrator, GroupChatSession, PersonaRegistry,
    GroupChatCommandParser, GroupChatRenderer,
    GroupChatError, GroupChatMessage, GroupChatRequest, GroupChatStatus,
    Persona, PersonaSource, Speaker, RenderedContent, ContentFormat,
    CoordinatorPlan, RespondentPlan,
};
```

**Step 3: Full compilation check**

Run: `cargo check -p alephcore`
Expected: No errors

**Step 4: Run all group_chat tests**

Run: `cargo test -p alephcore --lib group_chat -v`
Expected: All tests PASS (protocol: 5, persona: 7, session: 4, coordinator: 6, orchestrator: 7, channel: 4 = 33 tests)

**Step 5: Run full test suite to check for regressions**

Run: `cargo test -p alephcore --lib -- --skip tools::markdown_skill::loader::tests`
Expected: No regressions (skip known pre-existing failures)

**Step 6: Commit**

```bash
git add core/src/lib.rs core/Cargo.toml
git commit -m "group_chat: wire module exports and verify full compilation"
```

---

## Summary

| Task | Description | Tests | Files |
|------|-------------|-------|-------|
| 1 | Protocol types | 5 | 3 (create 2, modify 1) |
| 2 | Config types | 9 | 3 (create 1, modify 2) |
| 3 | PersonaRegistry | 7 | 2 (create 1, modify 1) |
| 4 | Session + SQLite | 4 | 5 (create 2, modify 3) |
| 5 | Coordinator | 6 | 2 (create 1, modify 1) |
| 6 | Orchestrator | 7 | 2 (create 1, modify 1) |
| 7 | Gateway handlers | 2 | 2 (create 1, modify 1) |
| 8 | Channel traits | 4 | 2 (create 1, modify 1) |
| 9 | Telegram adapter | 8 | 2 (create 1, modify 1) |
| 10 | Wiring + check | 0 | 2 (modify 2) |
| **Total** | | **52** | **10 new, 8 modified** |

Estimated: 10 commits, ~1200 lines of new code, ~52 unit tests.
