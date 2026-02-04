# Agent Core Design: Session Key, Agent-to-Agent, Tool Security, Streaming

**Date**: 2025-01-28
**Status**: Draft
**Reference**: Moltbot (`/Users/zouguojun/Workspace/moltbot/`)

---

## Overview

This document outlines the implementation plan for four core agent capabilities:

1. **C. Session Key Enhancement** - Route context encoding
2. **B. Agent-to-Agent Communication** - Inter-session messaging
3. **A. Tool Security Execution** - Policy layering + approval protocol
4. **D. Streaming Thinking** - `<think>` block streaming

Implementation order: C → B → A → D (serial, depth-first)

---

## Part C: Session Key Enhancement

### C.1 Core Data Structures

```rust
// core/src/routing/session_key.rs

use serde::{Deserialize, Serialize};

/// DM session isolation strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DmScope {
    /// All DMs share main session
    Main,
    /// Per-user isolation (cross-channel)
    PerPeer,
    /// Per-channel per-user isolation
    PerChannelPeer,
}

impl Default for DmScope {
    fn default() -> Self {
        Self::PerPeer
    }
}

/// Peer type for group sessions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PeerKind {
    Group,
    Channel,
    Thread,
}

/// Enhanced session key with full context encoding
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionKey {
    /// Main session (cross-channel shared)
    Main {
        agent_id: String,
        #[serde(default = "default_main_key")]
        main_key: String,
    },

    /// Direct message session with scope strategy
    DirectMessage {
        agent_id: String,
        channel: String,
        peer_id: String,
        #[serde(default)]
        dm_scope: DmScope,
    },

    /// Group/channel session
    Group {
        agent_id: String,
        channel: String,
        peer_kind: PeerKind,
        peer_id: String,
        /// Optional thread ID for nested conversations
        thread_id: Option<String>,
    },

    /// Task session (cron, webhook, scheduled)
    Task {
        agent_id: String,
        task_type: String,
        task_id: String,
    },

    /// Subagent session (nested under parent)
    Subagent {
        parent_key: Box<SessionKey>,
        subagent_id: String,
    },

    /// Ephemeral session (no persistence)
    Ephemeral {
        agent_id: String,
        ephemeral_id: String,
    },
}

fn default_main_key() -> String {
    "main".to_string()
}
```

### C.2 Session Key String Format

```
Format specification:
agent:<agent_id>:<rest>

Examples:
agent:main:main                              # Main session
agent:main:dm:user123                        # DM (per-peer scope)
agent:main:telegram:dm:user123               # DM (per-channel-peer scope)
agent:work:discord:group:guild456            # Discord guild
agent:main:slack:channel:C123456             # Slack channel
agent:main:telegram:group:chat789:thread:t1  # Thread in group
agent:main:subagent:coding-agent:task123     # Subagent session
agent:main:cron:daily-summary                # Cron task
agent:main:ephemeral:uuid-xxx                # Ephemeral
```

**Parsing rules**:

```rust
impl SessionKey {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_lowercase();
        let parts: Vec<&str> = s.split(':').collect();

        if parts.len() < 3 || parts[0] != "agent" {
            return None;
        }

        let agent_id = normalize_agent_id(parts[1]);
        let rest = &parts[2..];

        match rest {
            // agent:id:main
            ["main"] | [] => Some(Self::Main {
                agent_id,
                main_key: "main".to_string(),
            }),

            // agent:id:dm:peer (per-peer scope)
            ["dm", peer_id] => Some(Self::DirectMessage {
                agent_id,
                channel: String::new(),
                peer_id: peer_id.to_string(),
                dm_scope: DmScope::PerPeer,
            }),

            // agent:id:channel:dm:peer (per-channel-peer scope)
            [channel, "dm", peer_id] => Some(Self::DirectMessage {
                agent_id,
                channel: channel.to_string(),
                peer_id: peer_id.to_string(),
                dm_scope: DmScope::PerChannelPeer,
            }),

            // agent:id:channel:group:peer
            [channel, "group", peer_id] => Some(Self::Group {
                agent_id,
                channel: channel.to_string(),
                peer_kind: PeerKind::Group,
                peer_id: peer_id.to_string(),
                thread_id: None,
            }),

            // agent:id:channel:channel:peer
            [channel, "channel", peer_id] => Some(Self::Group {
                agent_id,
                channel: channel.to_string(),
                peer_kind: PeerKind::Channel,
                peer_id: peer_id.to_string(),
                thread_id: None,
            }),

            // agent:id:channel:group:peer:thread:tid
            [channel, "group", peer_id, "thread", thread_id] => Some(Self::Group {
                agent_id,
                channel: channel.to_string(),
                peer_kind: PeerKind::Group,
                peer_id: peer_id.to_string(),
                thread_id: Some(thread_id.to_string()),
            }),

            // agent:id:cron|webhook|scheduled:task_id
            [task_type @ ("cron" | "webhook" | "scheduled"), task_id] => Some(Self::Task {
                agent_id,
                task_type: task_type.to_string(),
                task_id: task_id.to_string(),
            }),

            // agent:id:subagent:name:rest...
            ["subagent", subagent_id, rest @ ..] => {
                let parent_key = Self::Main {
                    agent_id,
                    main_key: "main".to_string(),
                };
                Some(Self::Subagent {
                    parent_key: Box::new(parent_key),
                    subagent_id: subagent_id.to_string(),
                })
            }

            // agent:id:ephemeral:uuid
            ["ephemeral", ephemeral_id] => Some(Self::Ephemeral {
                agent_id,
                ephemeral_id: ephemeral_id.to_string(),
            }),

            // Fallback: treat as main_key
            [main_key] => Some(Self::Main {
                agent_id,
                main_key: main_key.to_string(),
            }),

            _ => None,
        }
    }

    pub fn to_key_string(&self) -> String {
        match self {
            Self::Main { agent_id, main_key } => {
                format!("agent:{}:{}", agent_id, main_key)
            }
            Self::DirectMessage { agent_id, channel, peer_id, dm_scope } => {
                match dm_scope {
                    DmScope::Main => format!("agent:{}:main", agent_id),
                    DmScope::PerPeer => format!("agent:{}:dm:{}", agent_id, peer_id),
                    DmScope::PerChannelPeer => {
                        format!("agent:{}:{}:dm:{}", agent_id, channel, peer_id)
                    }
                }
            }
            Self::Group { agent_id, channel, peer_kind, peer_id, thread_id } => {
                let kind = match peer_kind {
                    PeerKind::Group => "group",
                    PeerKind::Channel => "channel",
                    PeerKind::Thread => "thread",
                };
                match thread_id {
                    Some(tid) => format!(
                        "agent:{}:{}:{}:{}:thread:{}",
                        agent_id, channel, kind, peer_id, tid
                    ),
                    None => format!("agent:{}:{}:{}:{}", agent_id, channel, kind, peer_id),
                }
            }
            Self::Task { agent_id, task_type, task_id } => {
                format!("agent:{}:{}:{}", agent_id, task_type, task_id)
            }
            Self::Subagent { parent_key, subagent_id } => {
                format!("{}:subagent:{}", parent_key.to_key_string(), subagent_id)
            }
            Self::Ephemeral { agent_id, ephemeral_id } => {
                format!("agent:{}:ephemeral:{}", agent_id, ephemeral_id)
            }
        }
    }

    pub fn agent_id(&self) -> &str {
        match self {
            Self::Main { agent_id, .. } => agent_id,
            Self::DirectMessage { agent_id, .. } => agent_id,
            Self::Group { agent_id, .. } => agent_id,
            Self::Task { agent_id, .. } => agent_id,
            Self::Subagent { parent_key, .. } => parent_key.agent_id(),
            Self::Ephemeral { agent_id, .. } => agent_id,
        }
    }

    /// Get the main session key for this agent
    pub fn main_session_key(&self) -> SessionKey {
        Self::Main {
            agent_id: self.agent_id().to_string(),
            main_key: "main".to_string(),
        }
    }
}

/// Normalize agent ID (lowercase, alphanumeric + dash/underscore)
fn normalize_agent_id(id: &str) -> String {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return "main".to_string();
    }

    let normalized: String = trimmed
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();

    // Remove leading/trailing dashes
    normalized
        .trim_start_matches('-')
        .trim_end_matches('-')
        .to_string()
}
```

### C.3 Route Resolution

```rust
// core/src/routing/resolve_route.rs

use super::session_key::{SessionKey, DmScope, PeerKind};
use crate::config::{BindingsConfig, SessionConfig};

/// Input for route resolution
#[derive(Debug, Clone)]
pub struct RouteInput {
    pub channel: String,
    pub account_id: Option<String>,
    pub peer: Option<RoutePeer>,
    pub guild_id: Option<String>,  // Discord
    pub team_id: Option<String>,   // Slack
}

#[derive(Debug, Clone)]
pub struct RoutePeer {
    pub kind: RoutePeerKind,
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutePeerKind {
    Dm,
    Group,
    Channel,
}

/// Resolved route result
#[derive(Debug, Clone)]
pub struct ResolvedRoute {
    pub agent_id: String,
    pub channel: String,
    pub account_id: String,
    pub session_key: SessionKey,
    pub main_session_key: SessionKey,
    pub matched_by: MatchedBy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchedBy {
    Peer,
    Guild,
    Team,
    Account,
    Channel,
    Default,
}

/// Resolve agent route from input
pub fn resolve_route(
    bindings: &[RouteBinding],
    session_cfg: &SessionConfig,
    default_agent: &str,
    input: &RouteInput,
) -> ResolvedRoute {
    let channel = input.channel.trim().to_lowercase();
    let account_id = input.account_id.clone()
        .unwrap_or_else(|| "default".to_string());

    // Filter bindings by channel and account
    let candidates: Vec<_> = bindings.iter()
        .filter(|b| matches_channel(&b.match_rule, &channel))
        .filter(|b| matches_account(&b.match_rule, &account_id))
        .collect();

    let dm_scope = session_cfg.dm_scope;
    let identity_links = &session_cfg.identity_links;

    let build = |agent_id: &str, matched_by: MatchedBy| {
        let agent_id = agent_id.to_string();
        let session_key = build_session_key(
            &agent_id,
            &channel,
            input.peer.as_ref(),
            dm_scope,
            identity_links,
        );
        let main_session_key = SessionKey::Main {
            agent_id: agent_id.clone(),
            main_key: "main".to_string(),
        };
        ResolvedRoute {
            agent_id,
            channel: channel.clone(),
            account_id: account_id.clone(),
            session_key,
            main_session_key,
            matched_by,
        }
    };

    // 1. Peer match
    if let Some(peer) = &input.peer {
        if let Some(b) = candidates.iter().find(|b| matches_peer(&b.match_rule, peer)) {
            return build(&b.agent_id, MatchedBy::Peer);
        }
    }

    // 2. Guild match (Discord)
    if let Some(guild_id) = &input.guild_id {
        if let Some(b) = candidates.iter().find(|b| matches_guild(&b.match_rule, guild_id)) {
            return build(&b.agent_id, MatchedBy::Guild);
        }
    }

    // 3. Team match (Slack)
    if let Some(team_id) = &input.team_id {
        if let Some(b) = candidates.iter().find(|b| matches_team(&b.match_rule, team_id)) {
            return build(&b.agent_id, MatchedBy::Team);
        }
    }

    // 4. Account match (specific, not wildcard)
    if let Some(b) = candidates.iter().find(|b| {
        b.match_rule.account_id.as_ref().map(|a| a != "*").unwrap_or(false)
            && b.match_rule.peer.is_none()
            && b.match_rule.guild_id.is_none()
            && b.match_rule.team_id.is_none()
    }) {
        return build(&b.agent_id, MatchedBy::Account);
    }

    // 5. Channel match (wildcard account)
    if let Some(b) = candidates.iter().find(|b| {
        b.match_rule.account_id.as_ref().map(|a| a == "*").unwrap_or(false)
            && b.match_rule.peer.is_none()
            && b.match_rule.guild_id.is_none()
            && b.match_rule.team_id.is_none()
    }) {
        return build(&b.agent_id, MatchedBy::Channel);
    }

    // 6. Default
    build(default_agent, MatchedBy::Default)
}

fn build_session_key(
    agent_id: &str,
    channel: &str,
    peer: Option<&RoutePeer>,
    dm_scope: DmScope,
    identity_links: &std::collections::HashMap<String, Vec<String>>,
) -> SessionKey {
    let Some(peer) = peer else {
        return SessionKey::Main {
            agent_id: agent_id.to_string(),
            main_key: "main".to_string(),
        };
    };

    match peer.kind {
        RoutePeerKind::Dm => {
            let peer_id = resolve_linked_peer_id(identity_links, channel, &peer.id)
                .unwrap_or_else(|| peer.id.clone());

            match dm_scope {
                DmScope::Main => SessionKey::Main {
                    agent_id: agent_id.to_string(),
                    main_key: "main".to_string(),
                },
                DmScope::PerPeer => SessionKey::DirectMessage {
                    agent_id: agent_id.to_string(),
                    channel: String::new(),
                    peer_id,
                    dm_scope: DmScope::PerPeer,
                },
                DmScope::PerChannelPeer => SessionKey::DirectMessage {
                    agent_id: agent_id.to_string(),
                    channel: channel.to_string(),
                    peer_id,
                    dm_scope: DmScope::PerChannelPeer,
                },
            }
        }
        RoutePeerKind::Group | RoutePeerKind::Channel => {
            let peer_kind = match peer.kind {
                RoutePeerKind::Group => PeerKind::Group,
                RoutePeerKind::Channel => PeerKind::Channel,
                _ => PeerKind::Group,
            };
            SessionKey::Group {
                agent_id: agent_id.to_string(),
                channel: channel.to_string(),
                peer_kind,
                peer_id: peer.id.clone(),
                thread_id: None,
            }
        }
    }
}

/// Resolve linked peer ID from identity links
fn resolve_linked_peer_id(
    identity_links: &std::collections::HashMap<String, Vec<String>>,
    channel: &str,
    peer_id: &str,
) -> Option<String> {
    let peer_lower = peer_id.trim().to_lowercase();
    let scoped = format!("{}:{}", channel.to_lowercase(), peer_lower);

    for (canonical, ids) in identity_links {
        for id in ids {
            let id_lower = id.trim().to_lowercase();
            if id_lower == peer_lower || id_lower == scoped {
                return Some(canonical.clone());
            }
        }
    }
    None
}

// Match helper functions
fn matches_channel(rule: &MatchRule, channel: &str) -> bool {
    rule.channel.as_ref().map(|c| c.to_lowercase() == channel).unwrap_or(false)
}

fn matches_account(rule: &MatchRule, account_id: &str) -> bool {
    match &rule.account_id {
        None => account_id == "default",
        Some(a) if a == "*" => true,
        Some(a) => a == account_id,
    }
}

fn matches_peer(rule: &MatchRule, peer: &RoutePeer) -> bool {
    rule.peer.as_ref().map(|p| {
        p.kind == peer.kind && p.id.to_lowercase() == peer.id.to_lowercase()
    }).unwrap_or(false)
}

fn matches_guild(rule: &MatchRule, guild_id: &str) -> bool {
    rule.guild_id.as_ref().map(|g| g == guild_id).unwrap_or(false)
}

fn matches_team(rule: &MatchRule, team_id: &str) -> bool {
    rule.team_id.as_ref().map(|t| t == team_id).unwrap_or(false)
}
```

### C.4 Configuration Structures

```rust
// core/src/config/session.rs

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::routing::session_key::DmScope;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    #[serde(default)]
    pub dm_scope: DmScope,

    #[serde(default)]
    pub identity_links: HashMap<String, Vec<String>>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            dm_scope: DmScope::PerPeer,
            identity_links: HashMap::new(),
        }
    }
}

// core/src/config/bindings.rs

use serde::{Deserialize, Serialize};
use crate::routing::resolve_route::RoutePeerKind;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteBinding {
    pub agent_id: String,
    #[serde(rename = "match")]
    pub match_rule: MatchRule,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MatchRule {
    pub channel: Option<String>,
    pub account_id: Option<String>,
    pub peer: Option<PeerMatch>,
    pub guild_id: Option<String>,
    pub team_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerMatch {
    pub kind: RoutePeerKind,
    pub id: String,
}

// core/src/config/agents.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsConfig {
    #[serde(default = "default_agent")]
    pub default: String,

    #[serde(default)]
    pub list: Vec<AgentDefinition>,
}

fn default_agent() -> String {
    "main".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub id: String,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub tools: Option<AgentToolsConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolsConfig {
    pub allow: Option<Vec<String>>,
    pub deny: Option<Vec<String>>,
    pub profile: Option<String>,
}
```

### C.5 Implementation Plan

| Step | Task | Files | Depends |
|------|------|-------|---------|
| 1 | Create routing module | `routing/mod.rs` | - |
| 2 | Implement SessionKey | `routing/session_key.rs` | Step 1 |
| 3 | Implement identity links | `routing/identity_links.rs` | Step 2 |
| 4 | Add SessionConfig | `config/session.rs` | - |
| 5 | Add BindingsConfig | `config/bindings.rs` | - |
| 6 | Implement route matching | `routing/bindings.rs` | Step 4, 5 |
| 7 | Implement resolve_route | `routing/resolve_route.rs` | Step 2, 3, 6 |
| 8 | Refactor gateway/router.rs | `gateway/router.rs` | Step 7 |
| 9 | Adapt session_manager | `gateway/session_manager.rs` | Step 8 |
| 10 | Unit tests | `routing/tests/` | Step 1-9 |
| 11 | Integration tests | `tests/routing_integration.rs` | Step 10 |

### C.6 Backward Compatibility

```rust
impl SessionKey {
    /// Parse legacy session key format
    pub fn from_legacy(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split(':').collect();

        if parts.len() < 3 || parts[0] != "agent" {
            return None;
        }

        let agent_id = parts[1].to_string();

        match parts.get(2..) {
            Some(["main"]) => Some(Self::Main {
                agent_id,
                main_key: "main".to_string(),
            }),
            Some(["peer", peer_id]) => Some(Self::DirectMessage {
                agent_id,
                channel: String::new(),
                peer_id: peer_id.to_string(),
                dm_scope: DmScope::PerPeer,
            }),
            Some([task_type @ ("cron" | "webhook" | "scheduled"), task_id]) => Some(Self::Task {
                agent_id,
                task_type: task_type.to_string(),
                task_id: task_id.to_string(),
            }),
            Some(["ephemeral", ephemeral_id]) => Some(Self::Ephemeral {
                agent_id,
                ephemeral_id: ephemeral_id.to_string(),
            }),
            Some([main_key]) => Some(Self::Main {
                agent_id,
                main_key: main_key.to_string(),
            }),
            _ => None,
        }
    }
}
```

---

## Part B: Agent-to-Agent Communication

### B.1 Core Tools Definition

Three core tools for inter-session communication:

| Tool | Purpose |
|------|---------|
| `sessions_list` | List visible sessions with filters |
| `sessions_send` | Send message to another session |
| `sessions_spawn` | Create sub-agent session for task execution |

```rust
// core/src/tools/sessions/list.rs

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionsListParams {
    /// Filter by session kinds: main, group, cron, hook, other
    #[serde(default)]
    pub kinds: Option<Vec<String>>,

    /// Maximum sessions to return
    #[serde(default)]
    pub limit: Option<u32>,

    /// Only sessions active within N minutes
    #[serde(default)]
    pub active_minutes: Option<u32>,

    /// Include last N messages (0-20)
    #[serde(default)]
    pub message_limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionsListResult {
    pub count: usize,
    pub sessions: Vec<SessionListRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListRow {
    pub key: String,
    pub kind: String,
    pub channel: Option<String>,
    pub label: Option<String>,
    pub updated_at: Option<i64>,
    pub model: Option<String>,
    pub messages: Option<Vec<SessionMessage>>,
}

// core/src/tools/sessions/send.rs

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionsSendParams {
    /// Target session key (mutually exclusive with label)
    #[serde(default)]
    pub session_key: Option<String>,

    /// Target session label (mutually exclusive with session_key)
    #[serde(default)]
    pub label: Option<String>,

    /// Agent ID for label lookup (optional)
    #[serde(default)]
    pub agent_id: Option<String>,

    /// Message to send
    pub message: String,

    /// Timeout in seconds (0 = fire-and-forget)
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
}

fn default_timeout() -> u32 { 30 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionsSendResult {
    pub status: SendStatus,
    pub run_id: Option<String>,
    pub session_key: Option<String>,
    pub reply: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SendStatus {
    Ok,
    Accepted,
    Timeout,
    Forbidden,
    Error,
}

// core/src/tools/sessions/spawn.rs

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionsSpawnParams {
    /// Task description for the sub-agent
    pub task: String,

    /// Optional label for the child session
    #[serde(default)]
    pub label: Option<String>,

    /// Target agent ID (defaults to current agent)
    #[serde(default)]
    pub agent_id: Option<String>,

    /// Model override for child session
    #[serde(default)]
    pub model: Option<String>,

    /// Thinking level override
    #[serde(default)]
    pub thinking: Option<String>,

    /// Run timeout in seconds (0 = no timeout)
    #[serde(default)]
    pub run_timeout_seconds: Option<u32>,

    /// Cleanup policy: "keep" or "delete"
    #[serde(default = "default_cleanup")]
    pub cleanup: String,
}

fn default_cleanup() -> String { "keep".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionsSpawnResult {
    pub status: SpawnStatus,
    pub run_id: Option<String>,
    pub child_session_key: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpawnStatus {
    Accepted,
    Forbidden,
    Error,
}
```

### B.2 Agent-to-Agent Policy

Cross-agent communication requires permission control:

```toml
# ~/.aleph/config.toml

[tools.agent_to_agent]
# Enable cross-agent communication
enabled = true

# Allow rules (format: "from_agent -> to_agent" or "*")
allow = [
  "main -> *",           # main can send to any agent
  "work -> main",        # work can only send to main
  "* -> monitor",        # all agents can send to monitor
]

# Sub-agent spawn permissions
[agents.list.main.subagents]
allow_agents = ["*"]     # main can spawn any agent

[agents.list.work.subagents]
allow_agents = ["work", "coding"]  # work can only spawn work or coding
model = "claude-sonnet-4"          # default model for sub-agents
```

```rust
// core/src/tools/sessions/policy.rs

use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct AgentToAgentPolicy {
    pub enabled: bool,
    rules: Vec<A2ARule>,
}

#[derive(Debug, Clone)]
struct A2ARule {
    from: RuleMatcher,
    to: RuleMatcher,
}

#[derive(Debug, Clone)]
enum RuleMatcher {
    Any,                    // "*"
    Specific(String),       // "agent_id"
}

impl AgentToAgentPolicy {
    pub fn from_config(cfg: &ToolsConfig) -> Self {
        let a2a = cfg.agent_to_agent.as_ref();
        let enabled = a2a.map(|c| c.enabled).unwrap_or(false);

        let rules = a2a
            .and_then(|c| c.allow.as_ref())
            .map(|allows| {
                allows.iter().filter_map(|s| parse_rule(s)).collect()
            })
            .unwrap_or_default();

        Self { enabled, rules }
    }

    /// Check if from_agent can send to to_agent
    pub fn is_allowed(&self, from_agent: &str, to_agent: &str) -> bool {
        if !self.enabled {
            return false;
        }

        // Same agent always allowed
        if from_agent == to_agent {
            return true;
        }

        self.rules.iter().any(|rule| {
            rule.from.matches(from_agent) && rule.to.matches(to_agent)
        })
    }
}

impl RuleMatcher {
    fn matches(&self, agent_id: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Specific(id) => id.eq_ignore_ascii_case(agent_id),
        }
    }
}

fn parse_rule(s: &str) -> Option<A2ARule> {
    let parts: Vec<&str> = s.split("->").map(|p| p.trim()).collect();
    match parts.as_slice() {
        ["*"] => Some(A2ARule {
            from: RuleMatcher::Any,
            to: RuleMatcher::Any,
        }),
        [from, to] => Some(A2ARule {
            from: parse_matcher(from),
            to: parse_matcher(to),
        }),
        _ => None,
    }
}

fn parse_matcher(s: &str) -> RuleMatcher {
    if s.trim() == "*" {
        RuleMatcher::Any
    } else {
        RuleMatcher::Specific(s.trim().to_lowercase())
    }
}
```

### B.3 Sandbox Visibility Control

Sandboxed sessions have limited visibility to prevent unauthorized access:

| Policy | Description |
|--------|-------------|
| `all` | Sandbox can see all sessions |
| `spawned` | Sandbox can only see self-spawned sessions (default) |
| `none` | Sandbox cannot see any other sessions |

```toml
[agents.defaults.sandbox]
session_tools_visibility = "spawned"
```

```rust
// core/src/tools/sessions/visibility.rs

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionToolsVisibility {
    All,
    #[default]
    Spawned,
    None,
}

/// Context for session visibility checks
pub struct VisibilityContext {
    /// Current session's internal key
    pub requester_key: String,
    /// Is this a sandboxed session?
    pub sandboxed: bool,
    /// Visibility policy
    pub visibility: SessionToolsVisibility,
}

impl VisibilityContext {
    /// Check if requester can see target session
    pub fn can_see(&self, target_key: &str, spawned_by: Option<&str>) -> bool {
        if !self.sandboxed {
            return true;
        }

        match self.visibility {
            SessionToolsVisibility::All => true,
            SessionToolsVisibility::None => false,
            SessionToolsVisibility::Spawned => {
                // Can see self
                if target_key == self.requester_key {
                    return true;
                }
                // Can see sessions spawned by self
                spawned_by.map(|s| s == self.requester_key).unwrap_or(false)
            }
        }
    }

    /// Check if requester can send to target session
    pub fn can_send(&self, target_key: &str, spawned_by: Option<&str>) -> bool {
        self.can_see(target_key, spawned_by)
    }

    /// Check if requester can spawn sub-agents
    pub fn can_spawn(&self) -> bool {
        if !self.sandboxed {
            return true;
        }
        // Subagent sessions cannot spawn further subagents
        !is_subagent_session_key(&self.requester_key)
    }
}

fn is_subagent_session_key(key: &str) -> bool {
    key.contains(":subagent:")
}
```

### B.4 Execution Flow

**sessions_send flow**:

```rust
// core/src/tools/sessions/send.rs

pub async fn execute_sessions_send(
    ctx: &ExecutionContext,
    params: SessionsSendParams,
    gateway: &GatewayClient,
) -> Result<SessionsSendResult> {
    // 1. Validate params (session_key XOR label)
    if params.session_key.is_some() && params.label.is_some() {
        return Err(ToolError::InvalidParams(
            "Provide either session_key or label, not both".into()
        ));
    }

    // 2. Resolve target session
    let target_key = match (&params.session_key, &params.label) {
        (Some(key), _) => resolve_session_key(key, &ctx.config)?,
        (_, Some(label)) => {
            gateway.resolve_session_by_label(label, params.agent_id.as_deref()).await?
        }
        _ => return Err(ToolError::InvalidParams("session_key or label required".into())),
    };

    // 3. Check A2A policy
    let requester_agent = ctx.session_key.agent_id();
    let target_agent = SessionKey::parse(&target_key)?.agent_id().to_string();

    if requester_agent != target_agent {
        let policy = AgentToAgentPolicy::from_config(&ctx.config.tools);
        if !policy.is_allowed(requester_agent, &target_agent) {
            return Ok(SessionsSendResult {
                status: SendStatus::Forbidden,
                error: Some("Agent-to-agent messaging denied".into()),
                ..Default::default()
            });
        }
    }

    // 4. Check visibility
    if !ctx.visibility.can_send(&target_key, None) {
        return Ok(SessionsSendResult {
            status: SendStatus::Forbidden,
            error: Some("Session not visible from sandboxed context".into()),
            ..Default::default()
        });
    }

    // 5. Send message
    let run_id = uuid::Uuid::new_v4().to_string();

    if params.timeout_seconds == 0 {
        // Fire-and-forget mode
        gateway.send_message(&target_key, &params.message, &run_id).await?;
        return Ok(SessionsSendResult {
            status: SendStatus::Accepted,
            run_id: Some(run_id),
            session_key: Some(target_key),
            ..Default::default()
        });
    }

    // 6. Wait for response
    let timeout = Duration::from_secs(params.timeout_seconds as u64);
    let response = gateway.send_and_wait(&target_key, &params.message, &run_id, timeout).await?;

    Ok(SessionsSendResult {
        status: SendStatus::Ok,
        run_id: Some(run_id),
        session_key: Some(target_key),
        reply: response.reply,
        ..Default::default()
    })
}
```

**sessions_spawn flow**:

```rust
// core/src/tools/sessions/spawn.rs

pub async fn execute_sessions_spawn(
    ctx: &ExecutionContext,
    params: SessionsSpawnParams,
    gateway: &GatewayClient,
) -> Result<SessionsSpawnResult> {
    // 1. Check spawn permission
    if !ctx.visibility.can_spawn() {
        return Ok(SessionsSpawnResult {
            status: SpawnStatus::Forbidden,
            error: Some("Sub-agents cannot spawn further sub-agents".into()),
            ..Default::default()
        });
    }

    // 2. Resolve target agent
    let requester_agent = ctx.session_key.agent_id();
    let target_agent = params.agent_id
        .as_deref()
        .unwrap_or(requester_agent);

    // 3. Check cross-agent spawn permission
    if target_agent != requester_agent {
        let allowed = ctx.config.get_agent(requester_agent)
            .and_then(|a| a.subagents.as_ref())
            .map(|s| s.is_agent_allowed(target_agent))
            .unwrap_or(false);

        if !allowed {
            return Ok(SessionsSpawnResult {
                status: SpawnStatus::Forbidden,
                error: Some(format!("Agent {} not allowed for spawn", target_agent)),
                ..Default::default()
            });
        }
    }

    // 4. Create child session key
    let child_session_key = format!(
        "agent:{}:subagent:{}",
        target_agent,
        uuid::Uuid::new_v4()
    );

    // 5. Apply model override if specified
    if let Some(model) = &params.model {
        gateway.patch_session(&child_session_key, SessionPatch {
            model: Some(model.clone()),
            ..Default::default()
        }).await?;
    }

    // 6. Build system prompt with context
    let system_prompt = build_subagent_system_prompt(
        &ctx.session_key.to_key_string(),
        &child_session_key,
        params.label.as_deref(),
        &params.task,
    );

    // 7. Start child agent run
    let run_id = gateway.start_agent_run(AgentRunParams {
        session_key: child_session_key.clone(),
        message: params.task.clone(),
        system_prompt: Some(system_prompt),
        thinking: params.thinking.clone(),
        timeout_seconds: params.run_timeout_seconds,
        spawned_by: Some(ctx.session_key.to_key_string()),
        label: params.label.clone(),
    }).await?;

    // 8. Register for announce callback
    register_subagent_run(SubagentRun {
        run_id: run_id.clone(),
        child_session_key: child_session_key.clone(),
        requester_session_key: ctx.session_key.to_key_string(),
        task: params.task,
        cleanup: params.cleanup.clone(),
    });

    Ok(SessionsSpawnResult {
        status: SpawnStatus::Accepted,
        run_id: Some(run_id),
        child_session_key: Some(child_session_key),
        ..Default::default()
    })
}
```

### B.5 Implementation Plan

**File structure**:

```
core/src/
├── tools/
│   └── sessions/                    # New module
│       ├── mod.rs                   # Module exports
│       ├── list.rs                  # sessions_list tool
│       ├── send.rs                  # sessions_send tool
│       ├── spawn.rs                 # sessions_spawn tool
│       ├── policy.rs                # A2A policy
│       ├── visibility.rs            # Sandbox visibility
│       ├── helpers.rs               # Shared helpers
│       └── registry.rs              # Sub-agent registry
├── config/
│   └── tools.rs                     # Add AgentToAgentConfig
├── gateway/
│   └── handlers/
│       └── sessions.rs              # Add resolve, patch methods
└── lib.rs                           # Exports
```

**Implementation steps**:

| Step | Task | Depends |
|------|------|---------|
| 1 | Create `tools/sessions/mod.rs` | Part C |
| 2 | Implement `policy.rs` (A2A policy) | Step 1 |
| 3 | Implement `visibility.rs` (sandbox control) | Step 1 |
| 4 | Add `config/tools.rs` config structures | - |
| 5 | Implement `helpers.rs` (shared functions) | Step 2, 3 |
| 6 | Implement `list.rs` (sessions_list) | Step 5 |
| 7 | Implement `send.rs` (sessions_send) | Step 5 |
| 8 | Implement `registry.rs` (sub-agent registry) | Step 5 |
| 9 | Implement `spawn.rs` (sessions_spawn) | Step 8 |
| 10 | Gateway handlers enhancement | Step 6-9 |
| 11 | Register to ToolRegistry | Step 10 |
| 12 | Unit tests | Step 11 |

**Gateway protocol extension**:

```rust
enum GatewayMethod {
    // Existing...

    // New
    SessionsList { params: SessionsListParams },
    SessionsResolve { label: String, agent_id: Option<String> },
    SessionsPatch { key: String, patch: SessionPatch },
    AgentRun { params: AgentRunParams },
    AgentWait { run_id: String, timeout_ms: u64 },
}
```

---

## Part A: Tool Security Execution

### A.1 Security Model and Configuration

**Three-level security model** (reference: Moltbot):

| Level | Description | Use Case |
|-------|-------------|----------|
| `deny` | Reject all command execution | Most secure, test env |
| `allowlist` | Only allow whitelisted commands | Production recommended |
| `full` | Allow all commands | Full trust |

**Ask policy**:

| Policy | Description |
|--------|-------------|
| `off` | Never ask user |
| `on-miss` | Ask when not in allowlist (default) |
| `always` | Ask every execution |

```rust
// core/src/exec/approvals.rs

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ExecSecurity {
    #[default]
    Deny,
    Allowlist,
    Full,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ExecAsk {
    Off,
    #[default]
    OnMiss,
    Always,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecApprovalsFile {
    pub version: u8,  // = 1
    pub socket: Option<SocketConfig>,
    pub defaults: Option<ExecDefaults>,
    pub agents: HashMap<String, AgentExecConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocketConfig {
    pub path: Option<String>,   // ~/.aleph/exec-approvals.sock
    pub token: Option<String>,  // Random auth token
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecDefaults {
    pub security: Option<ExecSecurity>,
    pub ask: Option<ExecAsk>,
    pub ask_fallback: Option<ExecSecurity>,
    pub auto_allow_skills: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentExecConfig {
    #[serde(flatten)]
    pub defaults: ExecDefaults,
    pub allowlist: Option<Vec<AllowlistEntry>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowlistEntry {
    pub id: Option<String>,           // UUID
    pub pattern: String,              // /usr/bin/git, ~/bin/*
    pub last_used_at: Option<i64>,
    pub last_used_command: Option<String>,
    pub last_resolved_path: Option<String>,
}
```

### A.2 Shell Command Analysis

Quote-aware parser supporting pipes, chain operators, escapes:

```rust
// core/src/exec/analysis.rs

#[derive(Debug, Clone)]
pub struct CommandAnalysis {
    pub ok: bool,
    pub reason: Option<String>,
    pub segments: Vec<CommandSegment>,
    pub chains: Option<Vec<Vec<CommandSegment>>>,
}

#[derive(Debug, Clone)]
pub struct CommandSegment {
    pub raw: String,
    pub argv: Vec<String>,
    pub resolution: Option<CommandResolution>,
}

#[derive(Debug, Clone)]
pub struct CommandResolution {
    pub raw_executable: String,
    pub resolved_path: Option<PathBuf>,
    pub executable_name: String,
}

// core/src/exec/parser.rs

const DISALLOWED_TOKENS: &[char] = &['>', '<', '`', '\n', '\r', '(', ')'];

pub fn analyze_shell_command(
    command: &str,
    cwd: Option<&Path>,
    env: Option<&HashMap<String, String>>,
) -> CommandAnalysis {
    // 1. Split by chain operators (&&, ||, ;)
    let chain_parts = match split_command_chain(command) {
        Ok(parts) => parts,
        Err(reason) => return CommandAnalysis::error(reason),
    };

    let mut all_segments = Vec::new();
    let mut chains = Vec::new();

    for part in chain_parts {
        // 2. Split by pipe |
        let pipeline_parts = match split_pipeline(&part) {
            Ok(parts) => parts,
            Err(reason) => return CommandAnalysis::error(reason),
        };

        // 3. Tokenize and resolve each segment
        let mut chain_segments = Vec::new();
        for raw in pipeline_parts {
            let argv = match tokenize_segment(&raw) {
                Some(tokens) => tokens,
                None => return CommandAnalysis::error("unable to parse segment"),
            };

            let resolution = resolve_executable(&argv, cwd, env);
            chain_segments.push(CommandSegment { raw, argv, resolution });
        }

        all_segments.extend(chain_segments.clone());
        chains.push(chain_segments);
    }

    CommandAnalysis {
        ok: true,
        reason: None,
        segments: all_segments,
        chains: Some(chains),
    }
}

fn tokenize_segment(segment: &str) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut buf = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;

    for ch in segment.chars() {
        if escaped {
            buf.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' if !in_single => { escaped = true; }
            '\'' if !in_double => { in_single = !in_single; }
            '"' if !in_single => { in_double = !in_double; }
            c if c.is_whitespace() && !in_single && !in_double => {
                if !buf.is_empty() {
                    tokens.push(std::mem::take(&mut buf));
                }
            }
            c => { buf.push(c); }
        }
    }

    if escaped || in_single || in_double {
        return None;
    }

    if !buf.is_empty() {
        tokens.push(buf);
    }

    Some(tokens)
}
```

### A.3 Approval Decision Logic

```rust
// core/src/exec/decision.rs

pub const DEFAULT_SAFE_BINS: &[&str] = &[
    "jq", "grep", "cut", "sort", "uniq", "head", "tail", "tr", "wc",
    "cat", "echo", "pwd", "ls", "which", "env", "date", "true", "false",
];

#[derive(Debug, Clone)]
pub enum ApprovalDecision {
    Allow,
    Deny { reason: String },
    NeedApproval { request: ApprovalRequest },
}

#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub id: String,
    pub command: String,
    pub cwd: Option<String>,
    pub analysis: CommandAnalysis,
    pub agent_id: String,
    pub session_key: String,
}

pub fn decide_exec_approval(
    config: &ExecApprovalsResolved,
    analysis: &CommandAnalysis,
    context: &ExecContext,
) -> ApprovalDecision {
    // 1. Analysis must be OK
    if !analysis.ok {
        return ApprovalDecision::Deny {
            reason: analysis.reason.clone().unwrap_or_else(|| "parse error".into()),
        };
    }

    // 2. Check security level
    match config.agent.security {
        ExecSecurity::Deny => {
            return ApprovalDecision::Deny {
                reason: "execution denied by security policy".into(),
            };
        }
        ExecSecurity::Full => {
            return ApprovalDecision::Allow;
        }
        ExecSecurity::Allowlist => { /* continue */ }
    }

    // 3. Check all segments
    for segment in &analysis.segments {
        let decision = check_segment(config, segment);

        if let SegmentDecision::NeedApproval = decision {
            if config.agent.ask == ExecAsk::Off {
                return apply_fallback(config);
            }
            return ApprovalDecision::NeedApproval {
                request: build_approval_request(analysis, context),
            };
        }

        if let SegmentDecision::Deny(reason) = decision {
            return ApprovalDecision::Deny { reason };
        }
    }

    ApprovalDecision::Allow
}

fn check_segment(config: &ExecApprovalsResolved, segment: &CommandSegment) -> SegmentDecision {
    let Some(resolution) = &segment.resolution else {
        return SegmentDecision::NeedApproval;
    };

    // Check safe bins
    if is_safe_bin_usage(&resolution.executable_name, &segment.argv, &config.safe_bins) {
        return SegmentDecision::Allow;
    }

    // Check allowlist
    if match_allowlist(&config.allowlist, resolution).is_some() {
        return SegmentDecision::Allow;
    }

    SegmentDecision::NeedApproval
}

fn is_safe_bin_usage(executable: &str, argv: &[String], safe_bins: &[String]) -> bool {
    if !safe_bins.iter().any(|b| b.eq_ignore_ascii_case(executable)) {
        return false;
    }

    // Arguments must not contain file paths
    for arg in argv.iter().skip(1) {
        if arg.contains('/') || arg.contains('\\') {
            return false;
        }
    }

    true
}
```

### A.4 Approval Socket Protocol

UI communicates with Core via Unix Domain Socket for approvals:

**Socket path**: `~/.aleph/exec-approvals.sock`

```rust
// core/src/exec/socket.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SocketMessage {
    Request {
        token: String,
        id: String,
        request: ApprovalRequestPayload,
    },
    Decision {
        id: String,
        decision: ApprovalDecisionType,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequestPayload {
    pub command: String,
    pub cwd: Option<String>,
    pub agent_id: String,
    pub session_key: String,
    pub executable: String,
    pub resolved_path: Option<String>,
    pub segments: Vec<SegmentInfo>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalDecisionType {
    AllowOnce,
    AllowAlways,
    Deny,
}

// core/src/exec/socket_server.rs

pub struct ApprovalSocketServer {
    path: PathBuf,
    token: String,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<ApprovalDecisionType>>>>,
}

impl ApprovalSocketServer {
    pub async fn start(config: &SocketConfig) -> Result<Self> {
        let path = expand_home(&config.path.clone()
            .unwrap_or_else(|| "~/.aleph/exec-approvals.sock".into()));

        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;

        #[cfg(unix)]
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;

        let server = Self {
            path,
            token: config.token.clone().unwrap_or_else(generate_token),
            pending: Arc::new(Mutex::new(HashMap::new())),
        };

        // Spawn accept loop
        let pending = server.pending.clone();
        let token = server.token.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let pending = pending.clone();
                let token = token.clone();
                tokio::spawn(handle_connection(stream, pending, token));
            }
        });

        Ok(server)
    }

    pub async fn request_approval(
        &self,
        request: ApprovalRequestPayload,
        timeout: Duration,
    ) -> Option<ApprovalDecisionType> {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();

        self.pending.lock().await.insert(id.clone(), tx);

        self.broadcast(SocketMessage::Request {
            token: self.token.clone(),
            id: id.clone(),
            request,
        }).await;

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(decision)) => Some(decision),
            _ => {
                self.pending.lock().await.remove(&id);
                None
            }
        }
    }
}
```

### A.5 Implementation Plan

**File structure**:

```
core/src/
├── exec/                            # New module
│   ├── mod.rs                       # Module exports
│   ├── approvals.rs                 # Config structures and loading
│   ├── analysis.rs                  # Command analysis structures
│   ├── parser.rs                    # Shell parser
│   ├── decision.rs                  # Approval decision logic
│   ├── allowlist.rs                 # Allowlist matching
│   ├── socket.rs                    # Socket protocol definition
│   ├── socket_server.rs             # Socket server
│   └── executor.rs                  # Command executor
├── tools/
│   └── bash.rs                      # Modify: integrate approval flow
└── lib.rs                           # Exports
```

**Implementation steps**:

| Step | Task | Depends |
|------|------|---------|
| 1 | Create `exec/mod.rs` | - |
| 2 | Implement `approvals.rs` (config loading) | Step 1 |
| 3 | Implement `analysis.rs` (structure definitions) | Step 1 |
| 4 | Implement `parser.rs` (shell parsing) | Step 3 |
| 5 | Implement `allowlist.rs` (matching logic) | Step 2 |
| 6 | Implement `decision.rs` (decision logic) | Step 4, 5 |
| 7 | Implement `socket.rs` (protocol definition) | Step 6 |
| 8 | Implement `socket_server.rs` (server) | Step 7 |
| 9 | Implement `executor.rs` (command execution) | Step 8 |
| 10 | Modify `tools/bash.rs` integrate approval | Step 9 |
| 11 | macOS UI approval dialog | Step 10 |
| 12 | Unit tests | Step 1-10 |

**Bash tool integration**:

```rust
// core/src/tools/bash.rs

pub async fn execute_bash(
    params: BashParams,
    ctx: &ExecutionContext,
) -> Result<BashResult> {
    // 1. Analyze command
    let analysis = analyze_shell_command(&params.command, ctx.cwd.as_deref(), None);

    // 2. Check approval
    let config = resolve_exec_approvals(ctx.agent_id(), None);
    let decision = decide_exec_approval(&config, &analysis, &ctx.exec_context());

    match decision {
        ApprovalDecision::Allow => { /* proceed */ }
        ApprovalDecision::Deny { reason } => {
            return Ok(BashResult::denied(reason));
        }
        ApprovalDecision::NeedApproval { request } => {
            let result = ctx.approval_server
                .request_approval(request.into(), Duration::from_secs(15))
                .await;

            match result {
                Some(ApprovalDecisionType::AllowOnce) => { /* proceed */ }
                Some(ApprovalDecisionType::AllowAlways) => {
                    add_to_allowlist(&config, &analysis).await?;
                }
                Some(ApprovalDecisionType::Deny) | None => {
                    return Ok(BashResult::denied("user denied or timeout"));
                }
            }
        }
    }

    // 3. Execute command
    execute_command(&params.command, ctx).await
}
```

---

## Part D: Streaming Thinking

### D.1 Thinking Levels and Configuration

**Thinking levels** (reference: Moltbot):

| Level | Description | Applicable Models |
|-------|-------------|-------------------|
| `off` | Disable thinking | All |
| `minimal` | Minimal thinking | All |
| `low` | Low-level thinking | All |
| `medium` | Medium-level thinking | All |
| `high` | High-level thinking | All |
| `xhigh` | Extra-high thinking | OpenAI o1/o3 |

**Reasoning display mode**:

| Mode | Description |
|------|-------------|
| `off` | Don't show reasoning |
| `on` | Show complete reasoning (after completion) |
| `stream` | Stream reasoning in real-time |

```rust
// core/src/agents/thinking.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThinkLevel {
    #[default]
    Off,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningMode {
    #[default]
    Off,
    On,
    Stream,
}

impl ThinkLevel {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "off" | "false" | "0" => Some(Self::Off),
            "on" | "enable" | "enabled" => Some(Self::Low),
            "min" | "minimal" => Some(Self::Minimal),
            "low" | "thinkhard" | "think-hard" => Some(Self::Low),
            "mid" | "med" | "medium" | "harder" => Some(Self::Medium),
            "high" | "ultra" | "max" => Some(Self::High),
            "xhigh" | "x-high" | "x_high" => Some(Self::XHigh),
            _ => None,
        }
    }

    pub fn available_levels(provider: Option<&str>, model: Option<&str>) -> Vec<Self> {
        let mut levels = vec![
            Self::Off, Self::Minimal, Self::Low, Self::Medium, Self::High
        ];

        if supports_xhigh(provider, model) {
            levels.push(Self::XHigh);
        }

        levels
    }
}

fn supports_xhigh(provider: Option<&str>, model: Option<&str>) -> bool {
    const XHIGH_MODELS: &[&str] = &["gpt-5.2", "gpt-5.2-codex", "gpt-5.1-codex"];
    model.map(|m| XHIGH_MODELS.iter().any(|x| m.contains(x))).unwrap_or(false)
}
```

### D.2 Streaming Thinking Block Processing

Detect multiple thinking tag formats:
- `<think>`, `</think>`
- `<thinking>`, `</thinking>`
- `<thought>`, `</thought>`
- `<antthinking>`, `</antthinking>` (Anthropic)

```rust
// core/src/agents/streaming/block_state.rs

use regex::Regex;
use lazy_static::lazy_static;

lazy_static! {
    static ref THINKING_TAG_RE: Regex =
        Regex::new(r"<\s*(/?)\s*(?:think(?:ing)?|thought|antthinking)\s*>").unwrap();
    static ref FINAL_TAG_RE: Regex =
        Regex::new(r"<\s*(/?)\s*final\s*>").unwrap();
}

#[derive(Debug, Clone, Default)]
pub struct BlockState {
    pub in_thinking: bool,
    pub in_final: bool,
    pub inline_code: InlineCodeState,
}

#[derive(Debug, Clone, Default)]
pub struct InlineCodeState {
    pub in_backtick: bool,
    pub backtick_count: usize,
}

#[derive(Debug, Clone)]
pub struct ProcessedDelta {
    pub thinking: Option<String>,
    pub final_answer: Option<String>,
    pub regular: Option<String>,
}

impl BlockState {
    pub fn process_delta(&mut self, delta: &str) -> ProcessedDelta {
        let mut thinking_content = String::new();
        let mut final_content = String::new();
        let mut regular_content = String::new();

        let mut remaining = delta;

        while !remaining.is_empty() {
            self.update_inline_code_state(remaining);

            if self.inline_code.in_backtick {
                regular_content.push_str(remaining);
                break;
            }

            if let Some(m) = THINKING_TAG_RE.find(remaining) {
                let before = &remaining[..m.start()];
                let tag = m.as_str();
                remaining = &remaining[m.end()..];

                if self.in_thinking {
                    thinking_content.push_str(before);
                } else if self.in_final {
                    final_content.push_str(before);
                } else {
                    regular_content.push_str(before);
                }

                let is_closing = tag.contains('/');
                self.in_thinking = !is_closing;
                continue;
            }

            if let Some(m) = FINAL_TAG_RE.find(remaining) {
                let before = &remaining[..m.start()];
                let tag = m.as_str();
                remaining = &remaining[m.end()..];

                if self.in_thinking {
                    thinking_content.push_str(before);
                } else if self.in_final {
                    final_content.push_str(before);
                } else {
                    regular_content.push_str(before);
                }

                let is_closing = tag.contains('/');
                self.in_final = !is_closing;
                continue;
            }

            if self.in_thinking {
                thinking_content.push_str(remaining);
            } else if self.in_final {
                final_content.push_str(remaining);
            } else {
                regular_content.push_str(remaining);
            }
            break;
        }

        ProcessedDelta {
            thinking: if thinking_content.is_empty() { None } else { Some(thinking_content) },
            final_answer: if final_content.is_empty() { None } else { Some(final_content) },
            regular: if regular_content.is_empty() { None } else { Some(regular_content) },
        }
    }
}
```

### D.3 Provider Adapters

| Provider | Thinking Support |
|----------|------------------|
| Anthropic | Native `thinking` param + `budget_tokens` |
| OpenAI | `<think>` tag parsing |
| Google | `thinking_config` param |
| DeepSeek | `<think>` tag parsing |

```rust
// core/src/agents/thinking_adapter.rs

pub trait ThinkingAdapter {
    fn apply_thinking(&self, level: ThinkLevel) -> Self;
    fn supports_native_thinking(&self) -> bool;
}

impl ThinkingAdapter for AnthropicRequest {
    fn apply_thinking(&self, level: ThinkLevel) -> Self {
        let mut req = self.clone();

        match level {
            ThinkLevel::Off => {
                req.thinking = None;
            }
            _ => {
                let budget = match level {
                    ThinkLevel::Minimal => 1024,
                    ThinkLevel::Low => 4096,
                    ThinkLevel::Medium => 8192,
                    ThinkLevel::High => 16384,
                    ThinkLevel::XHigh => 32768,
                    ThinkLevel::Off => 0,
                };

                req.thinking = Some(ThinkingConfig {
                    type_: "enabled".to_string(),
                    budget_tokens: Some(budget),
                });
            }
        }

        req
    }

    fn supports_native_thinking(&self) -> bool {
        true
    }
}

impl ThinkingAdapter for OpenAIRequest {
    fn apply_thinking(&self, level: ThinkLevel) -> Self {
        let mut req = self.clone();

        if level != ThinkLevel::Off && !self.is_reasoning_model() {
            let hint = match level {
                ThinkLevel::Minimal => "Think briefly before responding.",
                ThinkLevel::Low => "Think step by step before responding.",
                ThinkLevel::Medium => "Think carefully and thoroughly before responding.",
                ThinkLevel::High => "Think very carefully, considering multiple angles.",
                ThinkLevel::XHigh => "Engage in extensive reasoning and analysis.",
                ThinkLevel::Off => "",
            };

            if !hint.is_empty() {
                req.prepend_system_message(hint);
            }
        }

        req
    }

    fn supports_native_thinking(&self) -> bool {
        self.is_reasoning_model()
    }
}

impl ThinkingAdapter for GoogleRequest {
    fn apply_thinking(&self, level: ThinkLevel) -> Self {
        let mut req = self.clone();

        if level != ThinkLevel::Off {
            req.generation_config.thinking_config = Some(GoogleThinkingConfig {
                thinking_budget: match level {
                    ThinkLevel::Minimal => 1024,
                    ThinkLevel::Low => 4096,
                    ThinkLevel::Medium => 8192,
                    ThinkLevel::High => 16384,
                    ThinkLevel::XHigh => 32768,
                    ThinkLevel::Off => 0,
                },
            });
        }

        req
    }

    fn supports_native_thinking(&self) -> bool {
        true
    }
}
```

### D.4 Streaming Events and Callbacks

```rust
// core/src/agents/streaming/events.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    AssistantStart { message_index: u32 },
    TextDelta { delta: String, accumulated: String },
    ThinkingDelta { delta: String, accumulated: String },
    ThinkingComplete { content: String },
    ToolStart { tool_id: String, tool_name: String },
    ToolComplete { tool_id: String, result: ToolResult },
    BlockReply { text: String, is_final: bool },
    AssistantComplete {
        content: String,
        thinking: Option<String>,
        usage: Option<TokenUsage>,
    },
    Error { message: String, recoverable: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub thinking_tokens: Option<u32>,
    pub cache_read_tokens: Option<u32>,
    pub cache_write_tokens: Option<u32>,
}

// core/src/agents/streaming/subscriber.rs

pub struct StreamSubscriber {
    config: StreamConfig,
    state: StreamState,
    block_state: BlockState,
    callbacks: StreamCallbacks,
}

pub struct StreamConfig {
    pub reasoning_mode: ReasoningMode,
    pub block_reply_break: BlockReplyBreak,
    pub chunk_size: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum BlockReplyBreak {
    #[default]
    TextEnd,
    Paragraph,
    Sentence,
}

pub struct StreamCallbacks {
    pub on_text_delta: Option<Box<dyn Fn(&str) + Send + Sync>>,
    pub on_thinking_delta: Option<Box<dyn Fn(&str) + Send + Sync>>,
    pub on_thinking_complete: Option<Box<dyn Fn(&str) + Send + Sync>>,
    pub on_block_reply: Option<Box<dyn Fn(&str, bool) + Send + Sync>>,
    pub on_tool_start: Option<Box<dyn Fn(&str, &str) + Send + Sync>>,
    pub on_tool_complete: Option<Box<dyn Fn(&str, &ToolResult) + Send + Sync>>,
    pub on_complete: Option<Box<dyn Fn(&StreamResult) + Send + Sync>>,
    pub on_error: Option<Box<dyn Fn(&str, bool) + Send + Sync>>,
}

impl StreamSubscriber {
    pub fn new(config: StreamConfig, callbacks: StreamCallbacks) -> Self {
        Self {
            config,
            state: StreamState::default(),
            block_state: BlockState::default(),
            callbacks,
        }
    }

    pub fn process_event(&mut self, event: ProviderStreamEvent) {
        match event {
            ProviderStreamEvent::ContentDelta { delta } => {
                let processed = self.block_state.process_delta(&delta);

                if let Some(thinking) = processed.thinking {
                    self.state.thinking_buffer.push_str(&thinking);

                    if self.config.reasoning_mode == ReasoningMode::Stream {
                        if let Some(cb) = &self.callbacks.on_thinking_delta {
                            cb(&thinking);
                        }
                    }
                }

                if let Some(regular) = processed.regular {
                    self.state.text_buffer.push_str(&regular);

                    if let Some(cb) = &self.callbacks.on_text_delta {
                        cb(&regular);
                    }

                    self.maybe_emit_block_reply(false);
                }
            }

            ProviderStreamEvent::MessageEnd => {
                if !self.state.thinking_buffer.is_empty() {
                    if let Some(cb) = &self.callbacks.on_thinking_complete {
                        cb(&self.state.thinking_buffer);
                    }
                }

                self.maybe_emit_block_reply(true);

                if let Some(cb) = &self.callbacks.on_complete {
                    cb(&self.build_result());
                }
            }
        }
    }
}
```

### D.5 Implementation Plan

**File structure**:

```
core/src/
├── agents/
│   ├── thinking.rs                  # ThinkLevel, ReasoningMode
│   ├── thinking_adapter.rs          # Provider adapters
│   └── streaming/                   # New module
│       ├── mod.rs                   # Module exports
│       ├── events.rs                # StreamEvent definitions
│       ├── block_state.rs           # Thinking tag parser state machine
│       ├── subscriber.rs            # Stream subscription callbacks
│       ├── chunker.rs               # Block reply chunking
│       └── inline_code.rs           # Inline code state
├── gateway/
│   └── handlers/
│       └── agent.rs                 # Modify: integrate thinking params
└── lib.rs                           # Exports
```

**Implementation steps**:

| Step | Task | Depends |
|------|------|---------|
| 1 | Implement `thinking.rs` (level definitions) | - |
| 2 | Create `streaming/mod.rs` | Step 1 |
| 3 | Implement `streaming/events.rs` | Step 2 |
| 4 | Implement `streaming/inline_code.rs` | Step 2 |
| 5 | Implement `streaming/block_state.rs` | Step 4 |
| 6 | Implement `streaming/chunker.rs` | Step 5 |
| 7 | Implement `streaming/subscriber.rs` | Step 3, 6 |
| 8 | Implement `thinking_adapter.rs` | Step 1 |
| 9 | Modify Agent Loop integration | Step 7, 8 |
| 10 | Gateway handler thinking param support | Step 9 |
| 11 | Unit tests | Step 1-10 |

**Gateway protocol extension**:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequest {
    pub message: String,
    pub session_key: String,

    #[serde(default)]
    pub thinking: Option<String>,

    #[serde(default)]
    pub reasoning_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentStreamFrame {
    TextDelta { delta: String },
    ThinkingDelta { delta: String },
    ThinkingComplete { content: String },
    ToolStart { tool_id: String, tool_name: String },
    ToolComplete { tool_id: String, result: Value },
    Complete { usage: Option<TokenUsage> },
    Error { message: String },
}
```

---

## Summary

This design document covers four core agent capabilities:

| Part | Capability | Key Features |
|------|------------|--------------|
| C | Session Key Enhancement | Channel-aware routing, DM scope, identity links |
| B | Agent-to-Agent Communication | sessions_list/send/spawn, A2A policy, sandbox visibility |
| A | Tool Security Execution | 3-level security, shell parsing, socket approval protocol |
| D | Streaming Thinking | ThinkLevel, provider adapters, tag parsing, stream callbacks |

**Implementation order**: C → B → A → D (serial, depth-first)

**Estimated file changes**:
- New modules: ~20 files
- Modified files: ~10 files
- Total new code: ~4000-5000 lines

---

## References

- Moltbot routing: `/Users/zouguojun/Workspace/moltbot/src/routing/`
- Moltbot sessions: `/Users/zouguojun/Workspace/moltbot/src/sessions/`
- Moltbot tools: `/Users/zouguojun/Workspace/moltbot/src/agents/pi-tools.ts`
- Moltbot approvals: `/Users/zouguojun/Workspace/moltbot/src/infra/exec-approvals.ts`
