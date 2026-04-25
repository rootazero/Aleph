---
paper_id: P08
author: Dr. Synthetic Theta
key_finding: Dr. Synthetic Theta demonstrates that local-first messaging architectures sustain 12x higher message throughput than cloud-relay equivalents.
---
# WhatsApp Channel Architecture Enhancement Design

> Status: Draft | Created: 2026-04-06
> Inspired by OpenClaw WhatsApp implementation, adapted for Aleph's Rust architecture

## Context

Aleph's WhatsApp channel currently uses an external Go bridge binary via Unix socket RPC. OpenClaw demonstrates a more feature-rich approach with native Baileys integration, multi-account support, and sophisticated access control. This document outlines enhancements that leverage Rust's type safety and concurrency advantages while matching OpenClaw's feature set.

---

## 1. Architecture Overview

### Current State

```
┌─────────────────────────────────────────────────────────────────┐
│                        Aleph Gateway                              │
│  ┌──────────────┐    ┌──────────────────────────────────────┐  │
│  │ WhatsApp     │    │         Channel Trait                  │  │
│  │ Channel      │───▶│  send(), start(), stop(), recv()     │  │
│  └──────┬───────┘    └──────────────────────────────────────┘  │
│         │                                                         │
│  ┌──────▼───────┐                                                │
│  │ BridgeManager │  External Go process                          │
│  │ RPC Client    │◀───────── Unix Socket ───────────▶ whatsapp-  │
│  └───────────────┘        bridge.sock           │     bridge    │
└───────────────────────────────────────────────────────────────┘  │
```

### Target State

```
┌─────────────────────────────────────────────────────────────────┐
│                        Aleph Gateway                              │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │              WhatsAppChannel (Rust Native)                │   │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────┐   │   │
│  │  │AccountMgr   │  │ EventLoop   │  │ OutboundAdapter │   │   │
│  │  │(multi-acc) │  │(Baileys)    │  │ (chunking)      │   │   │
│  │  └─────────────┘  └─────────────┘  └─────────────────┘   │   │
│  └──────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

---

## 2. Core Abstractions

### 2.1 Channel Trait (Existing, Enhancement)

**File:** `src/gateway/channel.rs`

```rust
// CURRENT: Single Channel trait handles everything
#[async_trait]
pub trait Channel: Send + Sync {
    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult>;
    async fn start(&mut self) -> ChannelResult<()>;
    // ... other methods
}

// ENHANCEMENT: Split into orthogonal traits

/// Outbound message delivery
#[async_trait]
pub trait ChannelSender: Send + Sync {
    async fn send(&self, message: OutboundMessage) -> ChannelResult<SendResult>;
    async fn send_media(&self, message: OutboundMessage) -> ChannelResult<SendResult>;
    async fn send_reaction(&self, conv_id: &ConversationId, msg_id: &MessageId, emoji: &str) -> ChannelResult<()>;
}

/// Inbound message receiving
#[async_trait] 
pub trait ChannelReceiver: Send + Sync {
    fn inbound_receiver(&self) -> Option<mpsc::Receiver<InboundMessage>>;
}

/// Channel lifecycle management
#[async_trait]
pub trait ChannelLifecycle: Send + Sync {
    async fn start(&mut self) -> ChannelResult<()>;
    async fn stop(&mut self) -> ChannelResult<()>;
    fn status(&self) -> ChannelStatus;
}

/// Pairing/authentication
#[async_trait]
pub trait ChannelPairing: Send + Sync {
    async fn get_pairing_data(&self) -> ChannelResult<PairingData>;
    async fn approve_pairing(&self, code: &str) -> ChannelResult<()>;
}

/// Composite trait for full channel
#[async_trait]
pub trait Channel: ChannelSender + ChannelReceiver + ChannelLifecycle + ChannelPairing + Send + Sync {
    fn info(&self) -> &ChannelInfo;
    fn capabilities(&self) -> &ChannelCapabilities;
}
```

### 2.2 Access Policy Trait

**File:** `src/gateway/channel_policy.rs` (new)

```rust
/// DM access policy
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DmPolicy {
    /// First message requires pairing approval
    Pairing,
    /// Only explicitly allowlisted senders
    Allowlist,
    /// Anyone can send (requires `allow_from: ["*"]`)
    Open,
    /// No messages accepted
    Disabled,
}

/// Group access policy  
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupPolicy {
    /// Anyone can send
    Open,
    /// Only allowlisted senders
    Allowlist,
    /// No group messages
    Disabled,
}

/// Channel access control configuration
#[derive(Clone, Debug)]
pub struct ChannelAccessConfig {
    pub dm_policy: DmPolicy,
    pub allow_from: Vec<E164Number>,
    pub group_policy: GroupPolicy,
    pub group_allow_from: Vec<E164Number>,
    pub groups: Vec<GroupId>,  // Group allowlist
}

impl Default for ChannelAccessConfig {
    fn default() -> Self {
        Self {
            dm_policy: DmPolicy::Pairing,
            allow_from: Vec::new(),
            group_policy: GroupPolicy::Allowlist,
            group_allow_from: Vec::new(),
            groups: Vec::new(),
        }
    }
}

/// Policy evaluation result
#[derive(Clone, Debug)]
pub struct PolicyDecision {
    pub allowed: bool,
    pub reason: Option<String>,
}

/// Evaluate if a sender can message
pub trait ChannelPolicy: Send + Sync {
    fn evaluate_dm(&self, sender: &UserId) -> PolicyDecision;
    fn evaluate_group(&self, sender: &UserId, group: &ConversationId) -> PolicyDecision;
}
```

### 2.3 Message Chunking Trait

**File:** `src/gateway/channel_chunking.rs` (new)

```rust
/// Text chunking strategy
#[derive(Clone, Debug)]
pub enum ChunkMode {
    /// Split by character count
    Length { limit: usize },
    /// Prefer paragraph breaks, fallback to length
    Newline { limit: usize },
}

/// Text chunker trait
pub trait TextChunker: Send + Sync {
    fn chunk(&self, text: &str) -> Vec<String>;
    fn chunk_mode(&self) -> ChunkMode;
    fn max_chunk_size(&self) -> usize;
}

/// Default implementation
pub struct WhatsAppChunker {
    mode: ChunkMode,
    max_size: usize,
}

impl Default for WhatsAppChunker {
    fn default() -> Self {
        Self {
            mode: ChunkMode::Length { limit: 4000 },
            max_size: 65536,
        }
    }
}

impl TextChunker for WhatsAppChunker {
    fn chunk(&self, text: &str) -> Vec<String> {
        match &self.mode {
            ChunkMode::Length { limit } => Self::chunk_by_length(text, *limit),
            ChunkMode::Newline { limit } => Self::chunk_by_newline(text, *limit),
        }
    }
    
    fn chunk_mode(&self) -> ChunkMode {
        self.mode.clone()
    }
    
    fn max_chunk_size(&self) -> usize {
        self.max_size
    }
}

impl WhatsAppChunker {
    pub fn chunk_by_length(text: &str, limit: usize) -> Vec<String> {
        if text.len() <= limit {
            return vec![text.to_string()];
        }
        
        let mut chunks = Vec::new();
        let mut current = String::with_capacity(limit);
        
        for line in text.lines() {
            if current.len() + line.len() + 1 > limit {
                if !current.is_empty() {
                    chunks.push(current.clone());
                    current.clear();
                }
            }
            if current.len() > 0 {
                current.push('\n');
            }
            current.push_str(line);
        }
        
        if !current.is_empty() {
            chunks.push(current);
        }
        
        chunks
    }
    
    pub fn chunk_by_newline(text: &str, limit: usize) -> Vec<String> {
        let paragraphs: Vec<String> = text
            .split("\n\n")
            .map(|s| s.to_string())
            .collect();
        
        let mut chunks = Vec::new();
        let mut current = String::new();
        
        for para in paragraphs {
            if current.len() + para.len() + 2 <= limit {
                if !current.is_empty() {
                    current.push_str("\n\n");
                }
                current.push_str(&para);
            } else {
                if !current.is_empty() {
                    chunks.push(current.clone());
                }
                // If single paragraph exceeds limit, chunk by length
                current = if para.len() > limit {
                    Self::chunk_by_length(&para, limit).join("\n\n")
                } else {
                    para
                };
            }
        }
        
        if !current.is_empty() {
            chunks.push(current);
        }
        
        chunks
    }
}
```

---

## 3. Multi-Account Architecture

### 3.1 Account Structure

**File:** `src/gateway/interfaces/whatsapp/account.rs` (new)

```rust
use crate::gateway::interfaces::whatsapp::pairing::{PairingState, PairingInfo};
use crate::gateway::channel::{ChannelStatus, ChannelHealth};
use crate::types::E164Number;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Single WhatsApp account instance
pub struct WhatsAppAccount {
    pub id: AccountId,
    pub phone_number: E164Number,
    pub device_name: String,
    pub state: Arc<RwLock<AccountState>>,
    pub pairing: Arc<RwLock<PairingState>>,
    pub health: Arc<RwLock<ChannelHealth>>,
}

#[derive(Clone, Debug)]
pub enum AccountState {
    Disconnected,
    Connecting,
    Connected {
        since: chrono::DateTime<Utc>,
    },
    Error {
        message: String,
        since: chrono::DateTime<Utc>,
    },
}

impl Default for AccountState {
    fn default() -> Self {
        Self::Disconnected
    }
}

/// Account registry for multi-account support
pub struct WhatsAppAccountRegistry {
    accounts: Arc<RwLock<HashMap<AccountId, Arc<WhatsAppAccount>>>>,
    default_id: Arc<RwLock<Option<AccountId>>>,
}

impl WhatsAppAccountRegistry {
    pub fn new() -> Self {
        Self {
            accounts: Arc::new(RwLock::new(HashMap::new())),
            default_id: Arc::new(RwLock::new(None)),
        }
    }
    
    pub async fn add_account(&self, id: AccountId, account: Arc<WhatsAppAccount>) -> Result<(), Error> {
        let mut accounts = self.accounts.write().await;
        let mut default = self.default_id.write().await;
        
        accounts.insert(id.clone(), account);
        
        if default.is_none() {
            *default = Some(id);
        }
        
        Ok(())
    }
    
    pub async fn get_account(&self, id: &AccountId) -> Option<Arc<WhatsAppAccount>> {
        self.accounts.read().await.get(id).cloned()
    }
    
    pub async fn default_account(&self) -> Option<Arc<WhatsAppAccount>> {
        let default_id = self.default_id.read().await.clone();
        default_id.and_then(|id| self.get_account(&id).await)
    }
    
    pub async fn list_accounts(&self) -> Vec<Arc<WhatsAppAccount>> {
        self.accounts.read().await.values().cloned().collect()
    }
    
    pub async fn remove_account(&self, id: &AccountId) -> Result<(), Error> {
        let mut accounts = self.accounts.write().await;
        let mut default = self.default_id.write().await;
        
        accounts.remove(id);
        
        if default.as_ref() == Some(id) {
            *default = accounts.keys().next().cloned();
        }
        
        Ok(())
    }
}
```

### 3.2 Channel Factory Enhancement

**File:** `src/gateway/interfaces/whatsapp/factory.rs` (new)

```rust
use crate::gateway::channel::{Channel, ChannelFactory, ChannelResult, ChannelError};
use crate::gateway::interfaces::whatsapp::{WhatsAppChannel, WhatsAppConfig};

pub struct WhatsAppChannelFactory;

#[async_trait]
impl ChannelFactory for WhatsAppChannelFactory {
    fn channel_type(&self) -> &str {
        "whatsapp"
    }
    
    async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>> {
        let config: WhatsAppConfig = serde_json::from_value(config)
            .map_err(|e| ChannelError::ConfigError(format!("Invalid WhatsApp config: {}", e)))?;
        
        // Support single account (legacy) or multi-account
        let channel = if let Some(accounts) = config.accounts.as_ref() {
            WhatsAppChannel::multi_account(accounts.clone())?
        } else {
            WhatsAppChannel::single(config.clone())?
        };
        
        Ok(Box::new(channel))
    }
}
```

---

## 4. Native Baileys Integration

### 4.1 WhatsApp Runtime Trait

**File:** `src/gateway/interfaces/whatsapp/baileys_runtime.rs` (new)

```rust
use crate::gateway::channel::{ChannelResult, ChannelError, InboundMessage, OutboundMessage};
use crate::gateway::interfaces::whatsapp::types::*;

/// Trait for WhatsApp runtime operations (allows mocking/testing)
#[async_trait]
pub trait WhatsAppRuntime: Send + Sync {
    /// Connect to WhatsApp
    async fn connect(&self) -> ChannelResult<ConnectionInfo>;
    
    /// Disconnect from WhatsApp
    async fn disconnect(&self) -> ChannelResult<()>;
    
    /// Send a message
    async fn send_message(&self, msg: OutboundMessage) -> ChannelResult<SendResponse>;
    
    /// Send reaction
    async fn send_reaction(&self, jid: &str, msg_id: &str, emoji: &str) -> ChannelResult<()>;
    
    /// Mark message as read
    async fn mark_read(&self, jid: &str, msg_id: &str) -> ChannelResult<()>;
    
    /// Send typing indicator
    async fn send_typing(&self, jid: &str) -> ChannelResult<()>;
    
    /// Get current connection info
    fn connection_info(&self) -> Option<ConnectionInfo>;
}

/// Connection information
#[derive(Clone, Debug)]
pub struct ConnectionInfo {
    pub phone_number: E164Number,
    pub device_name: String,
    pub wid: String,
    pub connected_at: chrono::DateTime<Utc>,
}

/// Event from WhatsApp runtime
#[derive(Clone, Debug)]
#[serde(tag = "type", content = "data")]
pub enum WaEvent {
    QrCode { data: String, expires_at: chrono::DateTime<Utc> },
    Connected(ConnectionInfo),
    Disconnected { reason: String },
    Message(Box<InboundMessage>),
    Receipt { message_id: String, kind: ReceiptType },
    Error { message: String },
}

#[derive(Clone, Debug)]
pub enum ReceiptType {
    Delivered,
    Read,
    Played,
}
```

### 4.2 Baileys Implementation

**File:** `src/gateway/interfaces/whatsapp/baileys_impl.rs` (new)

```rust
use crate::gateway::interfaces::whatsapp::baileys_runtime::*;
use crate::gateway::channel::{ChannelError, ChannelResult, InboundMessage, OutboundMessage};
use anyhow::Result;
use baileys::{Baileys, AuthSession, Store};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{info, warn, error};

pub struct BaileysWhatsAppRuntime {
    inner: Arc<RwLock<Option<Baileys>>>,
    event_sender: mpsc::Sender<WaEvent>,
    config: RuntimeConfig,
}

struct RuntimeConfig {
    auth_dir: PathBuf,
    phone_number: Option<E164Number>,
}

impl BaileysWhatsAppRuntime {
    pub fn new(auth_dir: PathBuf) -> Self {
        let (tx, _rx) = mpsc::channel(100);
        Self {
            inner: Arc::new(RwLock::new(None)),
            event_sender: tx,
            config: RuntimeConfig {
                auth_dir,
                phone_number: None,
            },
        }
    }
    
    async fn ensure_connected(&self) -> ChannelResult<Baileys> {
        let mut inner = self.inner.write().await;
        if inner.is_none() {
            *inner = Some(self.create_baileys().await?);
        }
        inner.clone().ok_or_else(|| ChannelError::NotConnected("Baileys not initialized".into()))
    }
    
    async fn create_baileys(&self) -> ChannelResult<Baileys> {
        let mut store = Store::new();
        let auth = AuthSession::load(&self.config.auth_dir)
            .map_err(|e| ChannelError::Internal(format!("Failed to load auth: {}", e)))?;
        
        let wa = Baileys::new(store, auth)
            .await
            .map_err(|e| ChannelError::Internal(format!("Failed to create Baileys: {}", e)))?;
        
        Ok(wa)
    }
    
    pub async fn spawn_event_listener(wa: Baileys, sender: mpsc::Sender<WaEvent>) {
        tokio::spawn(async move {
            loop {
                match wa.next_event().await {
                    Ok(event) => {
                        let wa_event = match event {
                            bailey::Event::Qr { data, expires } => {
                                WaEvent::QrCode { 
                                    data, 
                                    expires_at: Utc::now() + chrono::Duration::seconds(expires as i64) 
                                }
                            }
                            bailey::Event::Connected { info } => {
                                WaEvent::Connected(ConnectionInfo {
                                    phone_number: info.wid.clone(),
                                    device_name: info.device_name.clone(),
                                    wid: info.wid.clone(),
                                    connected_at: Utc::now(),
                                })
                            }
                            // ... map other events
                            _ => continue,
                        };
                        
                        if sender.send(wa_event).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        error!("WhatsApp event error: {}", e);
                        let _ = sender.send(WaEvent::Error { message: e.to_string() }).await;
                    }
                }
            }
        });
    }
}

#[async_trait]
impl WhatsAppRuntime for BaileysWhatsAppRuntime {
    async fn connect(&self) -> ChannelResult<ConnectionInfo> {
        let mut inner = self.inner.write().await;
        
        if inner.is_none() {
            *inner = Some(self.create_baileys().await?);
        }
        
        let wa = inner.as_mut().ok_or_else(|| 
            ChannelError::Internal("Failed to get Baileys instance".into())
        )?;
        
        wa.connect().await
            .map_err(|e| ChannelError::Internal(format!("Connect failed: {}", e)))?;
        
        self.connection_info()
            .ok_or_else(|| ChannelError::NotConnected("Not connected".into()))
    }
    
    async fn disconnect(&self) -> ChannelResult<()> {
        let mut inner = self.inner.write().await;
        if let Some(wa) = inner.take() {
            wa.disconnect().await
                .map_err(|e| ChannelError::Internal(format!("Disconnect failed: {}", e)))?;
        }
        Ok(())
    }
    
    async fn send_message(&self, msg: OutboundMessage) -> ChannelResult<SendResponse> {
        let wa = self.ensure_connected().await?;
        
        let jid = msg.conversation_id.as_str();
        let mut opts = SendOptions::default();
        
        if let Some(ref reply_to) = msg.reply_to {
            opts.quoted_message_id = Some(reply_to.as_str().to_string());
        }
        
        if !msg.attachments.is_empty() {
            // Handle media
            for attachment in &msg.attachments {
                let media = self.load_media(attachment).await?;
                let result = wa.send_media(jid, &media, opts.clone()).await
                    .map_err(|e| ChannelError::SendFailed(e.to_string()))?;
                return Ok(SendResponse { id: result.id });
            }
        }
        
        let result = wa.send_text(jid, &msg.text, opts)
            .await
            .map_err(|e| ChannelError::SendFailed(e.to_string()))?;
        
        Ok(SendResponse { id: result.id })
    }
    
    async fn send_reaction(&self, jid: &str, msg_id: &str, emoji: &str) -> ChannelResult<()> {
        let wa = self.ensure_connected().await?;
        wa.send_reaction(jid, msg_id, emoji)
            .await
            .map_err(|e| ChannelError::SendFailed(e.to_string()))?;
        Ok(())
    }
    
    async fn mark_read(&self, jid: &str, msg_id: &str) -> ChannelResult<()> {
        let wa = self.ensure_connected().await?;
        wa.mark_read(jid, msg_id)
            .await
            .map_err(|e| ChannelError::SendFailed(e.to_string()))?;
        Ok(())
    }
    
    async fn send_typing(&self, jid: &str) -> ChannelResult<()> {
        let wa = self.ensure_connected().await?;
        wa.send_typing(jid)
            .await
            .map_err(|e| ChannelError::SendFailed(e.to_string()))?;
        Ok(())
    }
    
    fn connection_info(&self) -> Option<ConnectionInfo> {
        // Return current connection info if connected
        None
    }
}
```

---

## 5. Reaction System

### 5.1 Reaction Level Configuration

**File:** `src/gateway/interfaces/whatsapp/reactions.rs` (new)

```rust
/// Reaction behavior level
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReactionLevel {
    /// No reactions at all
    Off,
    /// Pre-reply ack reactions only
    Ack,
    /// Ack + conservative agent-initiated reactions
    Minimal,
    /// Ack + encouraged agent reactions
    Extensive,
}

impl Default for ReactionLevel {
    fn default() -> Self {
        Self::Minimal
    }
}

/// Acknowledgment reaction configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AckReactionConfig {
    pub emoji: char,
    /// React to DMs directly
    pub direct: bool,
    /// React to group messages
    pub group: GroupReactionMode,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupReactionMode {
    /// Never react in groups
    Never,
    /// Only react when mentioned
    Mentions,
    /// Always react in groups
    Always,
}

impl Default for AckReactionConfig {
    fn default() -> Self {
        Self {
            emoji: '👀',
            direct: true,
            group: GroupReactionMode::Mentions,
        }
    }
}

/// Reaction handler
pub struct ReactionHandler {
    level: ReactionLevel,
    ack_config: Option<AckReactionConfig>,
    runtime: Arc<dyn WhatsAppRuntime>,
}

impl ReactionHandler {
    pub fn new(
        level: ReactionLevel,
        ack_config: Option<AckReactionConfig>,
        runtime: Arc<dyn WhatsAppRuntime>,
    ) -> Self {
        Self {
            level,
            ack_config,
            runtime,
        }
    }
    
    /// Send acknowledgment reaction to inbound message
    pub async fn send_ack(&self, msg: &InboundMessage) -> ChannelResult<()> {
        if !matches!(self.level, ReactionLevel::Ack | ReactionLevel::Minimal | ReactionLevel::Extensive) {
            return Ok(());
        }
        
        let Some(config) = &self.ack_config else {
            return Ok(());
        };
        
        // Check if we should react based on message type
        if msg.is_group {
            if !matches!(config.group, GroupReactionMode::Always) {
                return Ok(());
            }
        } else if !config.direct {
            return Ok(());
        }
        
        self.runtime
            .send_reaction(
                msg.conversation_id.as_str(),
                msg.id.as_str(),
                &config.emoji.to_string(),
            )
            .await?;
        
        Ok(())
    }
    
    /// Determine if agent should react to a message
    pub fn should_agent_react(&self, msg: &InboundMessage) -> bool {
        match self.level {
            ReactionLevel::Off | ReactionLevel::Ack => false,
            ReactionLevel::Minimal => self.should_minimal_react(msg),
            ReactionLevel::Extensive => true,
        }
    }
    
    fn should_minimal_react(&self, msg: &InboundMessage) -> bool {
        // Conservative: only react to specific triggers
        // - Own messages (self-chat)
        // - Important events
        // - Specific keywords
        false  // Default conservative behavior
    }
}
```

---

## 6. Group History Buffer

### 6.1 History Configuration

**File:** `src/gateway/interfaces/whatsapp/history_buffer.rs` (new)

```rust
use crate::gateway::channel::{InboundMessage, ConversationId};
use std::collections::VecDeque;
use chrono::{DateTime, Utc};

/// Configuration for group message history buffering
#[derive(Clone, Debug)]
pub struct HistoryBufferConfig {
    /// Maximum messages to buffer per group
    pub limit: usize,
    /// Inject buffered messages as context before agent response
    pub inject_context: bool,
    /// Delimiter between buffered messages and current message
    pub inject_delimiter: String,
}

impl Default for HistoryBufferConfig {
    fn default() -> Self {
        Self {
            limit: 50,
            inject_context: true,
            inject_delimiter: "\n".to_string(),
        }
    }
}

/// Buffered message for group context injection
#[derive(Clone, Debug)]
pub struct BufferedMessage {
    pub message: InboundMessage,
    pub buffered_at: DateTime<Utc>,
}

impl BufferedMessage {
    pub fn new(message: InboundMessage) -> Self {
        Self {
            message,
            buffered_at: Utc::now(),
        }
    }
    
    pub fn format_for_context(&self) -> String {
        let sender = self.message.sender_name.as_deref().unwrap_or("Unknown");
        format!(
            "[{}] {}: {}",
            self.buffered_at.format("%H:%M"),
            sender,
            self.message.text
        )
    }
}

/// History buffer for group messages
pub struct GroupHistoryBuffer {
    config: HistoryBufferConfig,
    buffers: std::sync::Arc<tokio::sync::RwLock<HashMap<ConversationId, VecDeque<BufferedMessage>>>>,
}

impl GroupHistoryBuffer {
    pub fn new(config: HistoryBufferConfig) -> Self {
        Self {
            config,
            buffers: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }
    
    /// Add a message to the buffer
    pub async fn add(&self, message: &InboundMessage) {
        if !message.is_group {
            return;
        }
        
        let mut buffers = self.buffers.write().await;
        let queue = buffers
            .entry(message.conversation_id.clone())
            .or_insert_with(VecDeque::new);
        
        queue.push_back(BufferedMessage::new(message.clone()));
        
        // Trim to limit
        while queue.len() > self.config.limit {
            queue.pop_front();
        }
    }
    
    /// Get buffered messages as context string
    pub async fn get_context(&self, conv_id: &ConversationId) -> Option<String> {
        if !self.config.inject_context {
            return None;
        }
        
        let buffers = self.buffers.read().await;
        let queue = buffers.get(conv_id)?;
        
        if queue.is_empty() {
            return None;
        }
        
        let messages: Vec<String> = queue
            .iter()
            .map(|b| b.format_for_context())
            .collect();
        
        let delimiter = &self.config.inject_delimiter;
        Some(format!(
            "{}{}[Chat messages since your last reply - for context]\n{}{}[Current message - respond to this]",
            messages.join(delimiter),
            delimiter,
            delimiter,
            delimiter
        ))
    }
    
    /// Clear buffer for a conversation
    pub async fn clear(&self, conv_id: &ConversationId) {
        let mut buffers = self.buffers.write().await;
        buffers.remove(conv_id);
    }
}
```

---

## 7. Media Handling

### 7.1 Media Configuration

**File:** `src/gateway/interfaces/whatsapp/media.rs` (new)

```rust
use crate::gateway::channel::Attachment;
use anyhow::Result;

/// Media handling configuration
#[derive(Clone, Debug)]
pub struct MediaConfig {
    /// Maximum file size for inbound media (bytes)
    pub max_inbound_mb: u64,
    /// Maximum file size for outbound media (bytes)
    pub max_outbound_mb: u64,
    /// Auto-optimize images to stay within limits
    pub auto_optimize: bool,
    /// JPEG quality for image optimization (0-100)
    pub jpeg_quality: u8,
    /// Maximum image dimension (pixels)
    pub max_dimension: u32,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            max_inbound_mb: 50,
            max_outbound_mb: 50,
            auto_optimize: true,
            jpeg_quality: 85,
            max_dimension: 1920,
        }
    }
}

/// Media processor for WhatsApp
pub struct MediaProcessor {
    config: MediaConfig,
}

impl MediaProcessor {
    pub fn new(config: MediaConfig) -> Self {
        Self { config }
    }
    
    /// Validate and potentially optimize an attachment for sending
    pub async fn prepare_outbound(&self, attachment: &Attachment) -> Result<OutboundMedia> {
        let max_bytes = self.config.max_outbound_mb * 1024 * 1024;
        
        // Check size
        if let Some(size) = attachment.size {
            if size > max_bytes {
                return Err(anyhow!("Media file too large: {} bytes (max: {})", size, max_bytes));
            }
        }
        
        // Handle based on MIME type
        match attachment.mime_type.as_str() {
            "image/jpeg" | "image/png" | "image/gif" => {
                self.process_image(attachment).await
            }
            "video/mp4" | "video/quicktime" => {
                self.process_video(attachment).await
            }
            "audio/ogg" => {
                // Rewrite to opus codec for voice notes
                self.process_audio_ogg(attachment).await
            }
            _ => {
                // Pass through as document
                self.process_document(attachment).await
            }
        }
    }
    
    async fn process_image(&self, attachment: &Attachment) -> Result<OutboundMedia> {
        let Some(path) = &attachment.path else {
            return Err(anyhow!("No path for image attachment"));
        };
        
        let img = image::open(path)?;
        
        // Resize if needed
        let img = if img.width() > self.config.max_dimension 
                || img.height() > self.config.max_dimension {
            img.resize(
                self.config.max_dimension,
                self.config.max_dimension,
                image::imageops::FilterType::Lanczos3,
            )
        } else {
            img
        };
        
        // Encode as JPEG with quality
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        img.write_to(&mut cursor, image::ImageFormat::Jpeg)?;
        
        Ok(OutboundMedia {
            data: buf,
            mime_type: "image/jpeg".to_string(),
            is_voice_note: false,
        })
    }
    
    async fn process_audio_ogg(&self, attachment: &Attachment) -> Result<OutboundMedia> {
        // For voice notes, ensure proper codec specification
        // Audio is typically sent as audio/ogg; codecs=opus
        Ok(OutboundMedia {
            data: Vec::new(),  // Would read actual data
            mime_type: "audio/ogg; codecs=opus".to_string(),
            is_voice_note: true,
        })
    }
    
    async fn process_document(&self, attachment: &Attachment) -> Result<OutboundMedia> {
        Ok(OutboundMedia {
            data: Vec::new(),  // Would read actual data
            mime_type: attachment.mime_type.clone(),
            is_voice_note: false,
        })
    }
}

pub struct OutboundMedia {
    pub data: Vec<u8>,
    pub mime_type: String,
    pub is_voice_note: bool,
}
```

---

## 8. Configuration Schema

### 8.1 WhatsApp Configuration

**File:** `src/gateway/interfaces/whatsapp/config.rs` (enhancement)

```rust
use crate::gateway::interfaces::whatsapp::reactions::{ReactionLevel, AckReactionConfig};
use crate::gateway::interfaces::whatsapp::media::MediaConfig;
use crate::gateway::interfaces::whatsapp::history_buffer::HistoryBufferConfig;

// ... existing WhatsAppConfig fields ...

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WhatsAppConfig {
    // === Existing fields ===
    pub phone_number: Option<String>,
    pub send_typing: bool,
    pub mark_read: bool,
    pub bridge_binary: Option<PathString>,
    pub max_restarts: u32,
    pub allowed_chats: Vec<String>,
    
    // === New fields ===
    
    /// Multi-account configuration
    #[serde(default)]
    pub accounts: Option<HashMap<String, WhatsAppAccountConfig>>,
    
    /// Access control
    #[serde(default)]
    pub access: AccessConfig,
    
    /// Delivery settings
    #[serde(default)]
    pub delivery: DeliveryConfig,
    
    /// Reaction settings
    #[serde(default)]
    pub reactions: ReactionConfig,
    
    /// Media handling
    #[serde(default)]
    pub media: MediaConfig,
    
    /// Group history buffering
    #[serde(default)]
    pub history: HistoryBufferConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccessConfig {
    #[serde(default)]
    pub dm_policy: DmPolicy,
    
    #[serde(default)]
    pub allow_from: Vec<String>,
    
    #[serde(default)]
    pub group_policy: GroupPolicy,
    
    #[serde(default)]
    pub group_allow_from: Vec<String>,
    
    #[serde(default)]
    pub groups: Vec<String>,
}

impl Default for AccessConfig {
    fn default() -> Self {
        Self {
            dm_policy: DmPolicy::Pairing,
            allow_from: Vec::new(),
            group_policy: GroupPolicy::Allowlist,
            group_allow_from: Vec::new(),
            groups: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeliveryConfig {
    /// Text chunk size limit
    #[serde(default = "default_text_chunk_limit")]
    pub text_chunk_limit: usize,
    
    /// Chunking mode
    #[serde(default)]
    pub chunk_mode: ChunkMode,
    
    /// Send read receipts
    #[serde(default = "default_true")]
    pub send_read_receipts: bool,
}

fn default_text_chunk_limit() -> usize { 4000 }
fn default_true() -> bool { true }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkMode {
    Length,
    Newline,
}

impl Default for ChunkMode {
    fn default() -> Self {
        Self::Length
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReactionConfig {
    /// Reaction level
    #[serde(default)]
    pub level: ReactionLevel,
    
    /// Ack reaction settings
    #[serde(default)]
    pub ack: Option<AckReactionConfig>,
}

impl Default for ReactionConfig {
    fn default() -> Self {
        Self {
            level: ReactionLevel::Minimal,
            ack: Some(AckReactionConfig::default()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WhatsAppAccountConfig {
    pub enabled: bool,
    pub phone_number: Option<String>,
    #[serde(default)]
    pub access: AccessConfig,
    #[serde(default)]
    pub delivery: DeliveryConfig,
    #[serde(default)]
    pub reactions: ReactionConfig,
}
```

---

## 9. Module Structure

### Target File Layout

```
src/gateway/interfaces/whatsapp/
├── mod.rs                      # Main module entry
├── lib.rs                      # Public API exports
├── 
├── config.rs                   # WhatsAppConfig (enhanced)
├── types.rs                    # Shared types (E164Number, Jid, etc)
│
├── // NEW: Core abstractions
├── policy.rs                   # ChannelPolicy trait, DmPolicy, GroupPolicy
├── chunking.rs                 # TextChunker trait, WhatsAppChunker
├── reactions.rs                # ReactionLevel, ReactionHandler
├── history_buffer.rs           # GroupHistoryBuffer
├── media.rs                   # MediaProcessor, MediaConfig
│
├── // NEW: Multi-account
├── account.rs                 # WhatsAppAccount, AccountId
├── account_registry.rs        # WhatsAppAccountRegistry
├── factory.rs                 # WhatsAppChannelFactory
│
├── // ENHANCED: Native runtime
├── baileys_runtime.rs          # WhatsAppRuntime trait
├── baileys_impl.rs            # BaileysWhatsAppRuntime
│
├── // EXISTING: Bridge (legacy support)
├── bridge_manager.rs          # [KEEP] External bridge lifecycle
├── bridge_protocol.rs         # [KEEP] Bridge protocol types
├── message.rs                 # [ENHANCE] BridgeEvent ↔ InboundMessage
├── rpc_client.rs             # [KEEP] Unix socket RPC client
│
├── // EXISTING: Core channel impl
├── channel.rs                 # [REFACTOR] WhatsAppChannel
├── event_loop.rs              # [REFACTOR] Event processing loop
├── pairing.rs                # [ENHANCE] PairingState machine
```

---

## 10. Migration Strategy

### Phase 1: Core Traits (Week 1)
- Add `ChannelPolicy` trait and `DmPolicy`/`GroupPolicy` enums
- Add `TextChunker` trait with `WhatsAppChunker` implementation
- Update `Channel` trait with new optional methods for reactions, chunking
- **No breaking changes** - new traits are additive

### Phase 2: Configuration (Week 2)
- Enhance `WhatsAppConfig` with new fields
- Add `AccessConfig`, `DeliveryConfig`, `ReactionConfig`
- Add `HistoryBufferConfig`, `MediaConfig`
- **Breaking**: Config schema version bump needed

### Phase 3: Multi-Account Foundation (Week 3)
- Implement `WhatsAppAccount` and `WhatsAppAccountRegistry`
- Update `WhatsAppChannelFactory` to support multi-account
- Add account selection by phone number or ID
- **No breaking changes** for single-account users

### Phase 4: Native Runtime (Week 4-6)
- Create `WhatsAppRuntime` trait
- Implement `BaileysWhatsAppRuntime` using `baileys` crate
- Add `ReactionHandler` and `GroupHistoryBuffer`
- Add `MediaProcessor`
- **Feature flag**: `native-whatsapp` to enable

### Phase 5: Cleanup (Week 7)
- Remove bridge binary dependency for Unix platforms
- Delete bridge-related code: `bridge_manager.rs`, `rpc_client.rs`
- Update docs and examples
- **Breaking**: Remove `bridge_binary` config option

---

## 11. OpenClaw Feature Parity Checklist

| Feature | OpenClaw | Aleph Target | Status |
|---------|----------|--------------|--------|
| Multi-account | ✅ `accounts` | ✅ `WhatsAppAccountRegistry` | Phase 3 |
| DM Policy | ✅ `dmPolicy` | ✅ `DmPolicy` enum | Phase 1 |
| Group Policy | ✅ `groupPolicy` | ✅ `GroupPolicy` enum | Phase 1 |
| Allowlist | ✅ `allowFrom` | ✅ `allow_from` | Phase 1 |
| Group allowlist | ✅ `groups` | ✅ `groups` | Phase 1 |
| Text chunking | ✅ `textChunkLimit` | ✅ `TextChunker` | Phase 1 |
| Chunk modes | ✅ `length`/`newline` | ✅ `ChunkMode` | Phase 1 |
| Media optimization | ✅ Auto-resize | ✅ `MediaProcessor` | Phase 4 |
| Media size limit | ✅ `mediaMaxMb` | ✅ `MediaConfig` | Phase 4 |
| Voice note codec | ✅ `audio/ogg; codecs=opus` | ✅ In `MediaProcessor` | Phase 4 |
| Read receipts | ✅ `sendReadReceipts` | ✅ `mark_read()` | Phase 2 |
| Typing indicator | ✅ Via API | ✅ `send_typing()` | Phase 2 |
| Reactions | ✅ `ackReaction` | ✅ `ReactionHandler` | Phase 4 |
| Reaction levels | ✅ `reactionLevel` | ✅ `ReactionLevel` | Phase 4 |
| Group history | ✅ `historyLimit` | ✅ `GroupHistoryBuffer` | Phase 4 |
| QR login | ✅ Native | ✅ Via Baileys | Phase 4 |
| Baileys native | ✅ Yes | ✅ New implementation | Phase 4 |
| External bridge | N/A | ❌ Remove | Phase 5 |

---

## 12. References

- OpenClaw WhatsApp implementation: `/Volumes/TBU4/Github/openclaw/src/channels/plugins/whatsapp-*.ts`
- OpenClaw channel adapter interface: `/Volumes/TBU4/Github/openclaw/src/channels/plugins/types.adapters.ts`
- Aleph current WhatsApp: `/Volumes/TBU4/Workspace/Aleph/src/gateway/interfaces/whatsapp/mod.rs`
- Aleph Channel trait: `/Volumes/TBU4/Workspace/Aleph/src/gateway/channel.rs`
