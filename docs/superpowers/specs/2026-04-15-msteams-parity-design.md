# MS Teams Channel Parity Design

**Date:** 2026-04-15
**Status:** Approved
**Author:** Sisyphus
**Version:** 1.0

---

## 1. Executive Summary

This document defines the implementation plan to achieve complete feature parity between Aleph's MS Teams channel implementation and OpenClaw's MS Teams plugin, while leveraging Rust's type safety, async concurrency, and security advantages.

**Goal:** Aleph MS Teams = OpenClaw MS Teams + Rust advantages

---

## 2. Architecture Overview

### 2.1 Current State

Aleph's MS Teams channel is a built-in module using:
- Bot Framework v3 REST API
- Client secret authentication only
- Basic Graph API with token cache
- Health tracking without auto-recovery
- Simple `allowed_users` access control

### 2.2 Target State

Full parity with OpenClaw plus Rust advantages:
- **Federated Authentication** (certificate + managed identity)
- **RSC Permissions** (resource-specific consent)
- **Health Auto-Recovery** (ChannelHealthMonitor pattern)
- **Per-Team Policy Routing** (team-specific configuration)
- **SharePoint File Upload** (group chat file sharing)
- **Message History** (Graph API history fetching)
- **Enhanced DM Pairing** (auto-pair on first DM)

---

## 3. Design Decisions

### 3.1 Authentication Architecture

#### Current (Client Secret Only)
```rust
MsTeamsConfig {
    client_id: String,
    client_secret: String,
    tenant_id: String,
}
```

#### Target (Federated + Client Secret)
```rust
#[derive(Debug, Clone)]
pub enum AuthFlow {
    /// Static client secret (existing, deprecated)
    ClientSecret(String),
    /// Federated identity with certificate
    Federated(FederatedCredential),
}

#[derive(Debug, Clone)]
pub struct FederatedCredential {
    /// Certificate file path (.pem or .pfx)
    certificate_path: PathBuf,
    /// Optional password for .pfx files
    certificate_password: Option<String>,
    /// Azure Managed Identity client ID (optional)
    managed_identity_client_id: Option<String>,
    /// Authority URL template: https://login.microsoftonline.com/{tenant}
    authority_url: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct MsTeamsConfig {
    pub enabled: bool,
    pub client_id: String,
    pub tenant_id: String,

    /// Auth method 1: Client secret (deprecated, use federated instead)
    #[deprecated(since = "2026.06.01")]
    pub client_secret: Option<String>,

    /// Auth method 2: Federated identity (recommended)
    pub federated_identity: Option<FederatedCredential>,
}
```

#### Token Acquisition Strategy
```
┌──────────────────────────────────────────────────────────────────┐
│                     Token Acquisition Flow                         │
├──────────────────────────────────────────────────────────────────┤
│                                                                   │
│  1. If federated_identity configured:                            │
│     a. Load certificate (async, tokio::fs)                       │
│     b. If managed_identity_client_id set:                         │
│        - Acquire MI token from Azure IMDS endpoint               │
│        - Use MI token + cert to call Microsoft identity          │
│     c. Else:                                                     │
│        - Use certificate directly with Microsoft identity         │
│                                                                   │
│  2. Else if client_secret configured:                            │
│     a. Use existing client_secret flow (deprecated)               │
│     b. Log deprecation warning                                    │
│                                                                   │
│  3. Token Refresh:                                               │
│     a. Proactive refresh at 80% token lifetime                    │
│     b. Use tokio::time::Interval with jitter                     │
│     c. Background task, non-blocking                              │
│                                                                   │
└──────────────────────────────────────────────────────────────────┘
```

#### Config Schema (TOML)
```toml
[msteams]
enabled = true
client_id = "your-app-id"
tenant_id = "your-tenant-id"

# Authentication: federated OR client_secret (federated takes precedence)

[msteams.federated_identity]
# Certificate-based authentication (recommended)
certificate_path = "/path/to/cert.pem"
certificate_password = "optional-for-pfx"  # only for .pfx files

# OR Managed Identity (for Azure-hosted deployments)
# managed_identity_client_id = "your-mi-client-id"

# Authority (login endpoint)
authority_url = "https://login.microsoftonline.com/{{tenant}}"

# Deprecation: client_secret still works but logs warning
[msteams.client_secret]
secret = "your-secret"
```

#### Trait Extension
```rust
/// Extended MsTeamsChannel trait for auth introspection
pub trait MsTeamsChannelExt: MsTeamsChannel {
    /// Returns true if using federated authentication
    fn uses_federated_auth(&self) -> bool;

    /// Returns the auth method name for logging/telemetry
    fn auth_method(&self) -> &'static str {
        if self.uses_federated_auth() {
            "federated"
        } else {
            "client_secret"
        }
    }
}
```

---

### 3.2 RSC Permissions

#### What is RSC?
Resource-Specific Consent allows the app to access Teams data at the resource (team/channel) level without requiring each user to consent individually.

#### Data Structures
```rust
#[derive(Debug, Clone, Default)]
pub struct RscPermissions {
    /// Read channel messages
    pub channel_message_read: bool,
    /// Edit channel messages
    pub channel_message_edit: bool,
    /// Delete channel messages
    pub channel_message_delete: bool,
    /// Access team members
    pub member_access: bool,
    /// Access SharePoint files
    pub file_access: bool,
}

pub struct RscPermissionManager {
    graph_client: GraphClient,
    permissions: RscPermissions,
}

impl RscPermissionManager {
    /// Declare RSC permissions to Graph API
    /// POST /teams/{teamId}/appPermissions
    pub async fn declare_permissions(&self, team_id: &str) -> Result<()> {
        let payload = serde_json::json!({
            "requiredResourceSpecificPermissions": [
                {
                    "resource": "ChannelMessage.Read",
                    "delegated": ["ChannelMessage.Read.Group"],
                    "application": self.permissions.channel_message_read
                },
                {
                    "resource": "ChannelMessage.Read.Group",
                    "delegated": [],
                    "application": self.permissions.channel_message_read
                },
                // ... additional permissions ...
            ]
        });

        self.graph_client
            .post(&format!("/teams/{}/appPermissions", team_id), payload)
            .await?;

        Ok(())
    }
}
```

#### Config Extension
```toml
[msteams.rsc]
enabled = true
channel_message_read = true
channel_message_edit = true
channel_message_delete = false
member_access = true
file_access = true  # Required for SharePoint integration
```

---

### 3.3 Health Auto-Recovery

#### OpenClaw Pattern: ChannelHealthMonitor
OpenClaw implements automatic connection health monitoring with:
- Periodic health checks
- Stale connection detection
- Automatic restart on degradation

#### Aleph Implementation
```rust
/// Health status of a channel
#[derive(Debug, Clone)]
pub enum HealthStatus {
    /// Healthy: receiving messages normally
    Healthy,
    /// Degraded: missed health checks
    Degraded { missed_checks: u32 },
    /// Stale: no messages for threshold duration
    Stale { last_activity: Instant },
    /// Restarting: recovering from stale state
    Restarting,
}

/// Channel with health monitoring capability
pub trait HealthyChannel: Send + Sync {
    /// Check if channel is stale (no activity for threshold)
    async fn is_stale(&self) -> bool;

    /// Graceful restart of channel connection
    async fn restart(&mut self) -> Result<()>;

    /// Get current health status
    async health_status(&self) -> HealthStatus;
}

/// Health monitor with auto-recovery
pub struct ChannelHealthMonitor {
    /// How often to check channel health
    check_interval: Duration,
    /// Threshold after which channel is considered stale
    stale_threshold: Duration,
    /// Maximum restart attempts before giving up
    max_restart_attempts: u32,
    /// Current restart attempt count
    restart_attempts: AtomicU32,
}

impl ChannelHealthMonitor {
    /// Check all channels and recover stale ones
    pub async fn check_and_recover<C: HealthyChannel>(&self, channel: &mut C) -> Result<()> {
        if channel.is_stale().await {
            warn!("Channel stale, initiating recovery...");

            let attempts = self.restart_attempts.load(Ordering::SeqCst);
            if attempts >= self.max_restart_attempts {
                error!("Max restart attempts reached, giving up");
                return Err(anyhow!("Max restart attempts exceeded"));
            }

            self.restart_attempts.fetch_add(1, Ordering::SeqCst);
            channel.restart().await?;
            self.restart_attempts.store(0, Ordering::SeqCst);

            info!("Channel recovered successfully");
        }
        Ok(())
    }
}
```

---

### 3.4 Per-Team Policy Routing

#### OpenClaw Pattern
OpenClaw supports team-specific configuration:
- Per-team DM allow/deny
- Per-team user allowlists
- Per-team user blocklists
- Per-team rate limits

#### Aleph Implementation
```rust
/// Policy for a specific team
#[derive(Debug, Clone)]
pub struct TeamPolicy {
    /// Team ID from Microsoft Graph
    pub team_id: String,
    /// Allow direct messages to bot
    pub allow_dm: bool,
    /// Users explicitly allowed in this team
    pub allowed_users: Vec<UserId>,
    /// Users blocked in this team
    pub blocked_users: Vec<UserId>,
    /// Optional rate limiting for this team
    pub rate_limit: Option<RateLimitConfig>,
}

/// Global channel configuration
#[derive(Debug, Clone)]
pub struct ChannelConfig {
    /// Default policy for teams without specific config
    pub default_policy: TeamPolicy,
    /// Team-specific policy overrides
    pub team_policies: HashMap<String, TeamPolicy>,
}

impl ChannelConfig {
    /// Get effective policy for a team
    /// Team-specific > default
    pub fn effective_policy(&self, team_id: &str) -> &TeamPolicy {
        self.team_policies
            .get(team_id)
            .unwrap_or(&self.default_policy)
    }

    /// Check if user is allowed to interact
    pub fn is_user_allowed(&self, team_id: &str, user_id: &UserId) -> bool {
        let policy = self.effective_policy(team_id);

        // Check blocklist first (takes precedence)
        if policy.blocked_users.contains(user_id) {
            return false;
        }

        // If allowlist is non-empty, user must be in it
        if !policy.allowed_users.is_empty() && !policy.allowed_users.contains(user_id) {
            return false;
        }

        true
    }
}
```

#### Config Schema
```toml
[msteams.policies]
# Default policy for all teams
default = { allow_dm = true, allowed_users = [], blocked_users = [] }

# Per-team overrides
[msteams.policies.teams."team-id-123"]
allow_dm = false
allowed_users = ["user1@domain.com", "user2@domain.com"]
blocked_users = ["user3@domain.com"]

[msteams.policies.teams."team-id-456"]
allow_dm = true
allowed_users = ["admin@domain.com"]
blocked_users = []
```

---

### 3.5 SharePoint File Upload

#### OpenClaw Pattern
OpenClaw uploads files to SharePoint for group chats:
- Get channel's SharePoint drive ID
- Create upload session
- Chunked upload (up to 4MB chunks)
- Return share link

#### Aleph Implementation
```rust
/// SharePoint client for file operations
pub struct SharePointClient {
    graph_client: GraphClient,
}

/// Share link response
#[derive(Debug)]
pub struct ShareLink {
    pub web_url: Url,
    pub expiration_date_time: Option<DateTime<Utc>>,
}

impl SharePointClient {
    /// Get SharePoint drive ID for a channel
    async fn get_channel_drive(&self, channel_id: &str) -> Result<String> {
        let response: DriveResponse = self
            .graph_client
            .get(&format!("/teams/{}/channels/{}/drive", team_id, channel_id))
            .await?;

        Ok(response.id)
    }

    /// Upload file to SharePoint and return share link
    pub async fn upload_file(
        &self,
        team_id: &str,
        channel_id: &str,
        file_name: &str,
        content: Bytes,
    ) -> Result<ShareLink> {
        // 1. Get channel's SharePoint drive
        let drive_id = self.get_channel_drive(team_id, channel_id).await?;

        // 2. Create upload session (supports files > 4MB)
        let session: UploadSessionResponse = self
            .graph_client
            .put(&format!("/drives/{}/root:/{}/createUploadSession", drive_id, file_name), ())
            .await?;

        // 3. Chunked upload using async streams (Rust advantage: no memory buffer)
        self.upload_chunks(&session.upload_url, content).await?;

        // 4. Create share link
        Ok(self.create_share_link(&drive_id, file_name).await?)
    }

    /// Chunked upload using async streams
    /// Unlike OpenClaw's memory buffer, Rust streams chunks directly
    async fn upload_chunks(&self, upload_url: &str, content: Bytes) -> Result<()> {
        const CHUNK_SIZE: usize = 4 * 1024 * 1024; // 4MB

        for (index, chunk) in content.chunks(CHUNK_SIZE).enumerate() {
            let start = index * CHUNK_SIZE;
            let end = start + chunk.len() - 1;

            self.graph_client
                .put_raw(
                    upload_url,
                    chunk,
                    &[("Content-Range", &format!("bytes {}-{}/{}", start, end, content.len()))],
                )
                .await?;
        }

        Ok(())
    }

    /// Create sharing link
    async fn create_share_link(&self, drive_id: &str, file_name: &str) -> Result<ShareLink> {
        let response: PermissionResponse = self
            .graph_client
            .post(
                &format!("/drives/{}/items/{}:/createLink", drive_id, file_name),
                json!({
                    "type": "view",
                    "scope": "organization"
                }),
            )
            .await?;

        Ok(ShareLink {
            web_url: response.link.web_url,
            expiration_date_time: response.link.expiration_date_time,
        })
    }
}
```

---

### 3.6 Message History via Graph

#### OpenClaw Pattern
OpenClaw fetches channel message history via Graph API:
- `GET /teams/{teamId}/channels/{channelId}/messages`
- Supports pagination with `$top` and `$before`
- Returns full message objects with attachments

#### Aleph Implementation
```rust
/// Message from Microsoft Graph
#[derive(Debug, Deserialize)]
pub struct GraphMessage {
    pub id: String,
    #[serde(rename = "createdDateTime")]
    pub created_date_time: DateTime<Utc>,
    #[serde(rename = "from")]
    pub from: MessageFrom,
    pub body: MessageBody,
    pub attachments: Vec<Attachment>,
    pub mentions: Vec<Mention>,
}

#[derive(Debug, Deserialize)]
pub struct MessageFrom {
    pub user: Option<User>,
    pub application: Option<Application>,
}

#[derive(Debug, Deserialize)]
pub struct MessageBody {
    pub content_type: String,
    pub content: String,
}

/// Fetches channel message history
pub struct HistoryFetcher {
    graph_client: GraphClient,
    /// Maximum messages to fetch per request
    max_messages: usize,
}

impl HistoryFetcher {
    /// Fetch recent messages from channel
    pub async fn fetch_history(
        &self,
        team_id: &str,
        channel_id: &str,
        before_message_id: Option<&str>,
    ) -> Result<Vec<GraphMessage>> {
        let mut query = vec![
            ("$top", self.max_messages.to_string()),
            ("$orderby", "createdDateTime desc".to_string()),
        ];

        if let Some(before) = before_message_id {
            query.push(("$before", before.to_string()));
        }

        let endpoint = format!(
            "/teams/{}/channels/{}/messages",
            team_id, channel_id
        );

        let response: MessagesResponse = self
            .graph_client
            .get(&endpoint, &query)
            .await?;

        Ok(response.messages)
    }

    /// Fetch messages with full context (includes reactions, edits)
    pub async fn fetch_message_with_context(
        &self,
        team_id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> Result<GraphMessage> {
        let response: GraphMessage = self
            .graph_client
            .get(&format!(
                "/teams/{}/channels/{}/messages/{}",
                team_id, channel_id, message_id
            ))
            .await?;

        Ok(response)
    }
}

#[derive(Debug, Deserialize)]
pub struct MessagesResponse {
    pub messages: Vec<GraphMessage>,
    #[serde(rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}
```

#### Config Extension
```toml
[msteams.history]
enabled = true
max_messages = 50  # Per fetch request
retention_days = 7 # How far back to fetch
```

---

### 3.7 Enhanced DM Pairing

#### Current State (Aleph)
Aleph has `dm_handling` config but requires manual pairing.

#### OpenClaw Pattern
OpenClaw auto-pairs on first DM:
1. User sends first DM to bot
2. Bot creates direct line automatically
3. User is now paired without manual intervention

#### Aleph Enhancement
```rust
/// DM Pairing state
#[derive(Debug, Clone)]
pub enum PairingState {
    /// Not yet paired (waiting for first DM)
    Unpaired,
    /// Paired with specific user
    Paired(PairingInfo),
}

#[derive(Debug, Clone)]
pub struct PairingInfo {
    pub user_id: UserId,
    pub user_email: String,
    pub direct_line_token: String,
    pub created_at: Instant,
}

/// DM handling with auto-pairing
impl MsTeamsChannel {
    pub async fn handle_dm(&mut self, message: &Message) -> Result<Option<DirectLine>> {
        match self.pairing_state() {
            PairingState::Unpaired => {
                info!("First DM from user {}, creating pairing", message.from.id);

                // Create pairing automatically
                let pairing_info = self.create_pairing(&message.from).await?;
                self.set_pairing_state(PairingState::Paired(pairing_info.clone()));

                Ok(Some(pairing_info.into()))
            }
            PairingState::Paired(info) => {
                // Verify user matches
                if info.user_id == message.from.id {
                    Ok(Some(info.clone().into()))
                } else {
                    warn!("DM from {} but paired with {}", message.from.id, info.user_id);
                    Ok(None)
                }
            }
        }
    }

    /// Create new pairing for user
    async fn create_pairing(&self, user: &User) -> Result<PairingInfo> {
        // Call Graph API to create conversation
        let conversation = self
            .graph_client
            .post(
                "/me/chats",
                json!({
                    "chatType": "oneOnOne",
                    "members": [
                        {
                            "@odata.type": "#microsoft.graph.aadUserConversationMember",
                            "roles": ["user"],
                            "userId": user.id
                        }
                    ]
                }),
            )
            .await?;

        // Get token for this conversation
        let token = self.get_direct_line_token(&conversation.id).await?;

        Ok(PairingInfo {
            user_id: user.id.clone(),
            user_email: user.email.clone(),
            direct_line_token: token,
            created_at: Instant::now(),
        })
    }
}
```

---

## 4. Feature Matrix

| Feature | OpenClaw | Aleph (Target) | Priority | Phase |
|---------|----------|----------------|----------|-------|
| Client Secret Auth | ✅ | ✅ (deprecated) | Done | - |
| Federated Auth | ✅ | ✅ | Required | Phase 1 |
| RSC Permissions | ✅ | ✅ | Required | Phase 2A |
| Health Auto-Recovery | ✅ ChannelHealthMonitor | ✅ | Required | Phase 2B |
| Per-Team Policy | ✅ | ✅ (enhanced) | Required | Phase 2B |
| SharePoint Upload | ✅ | ✅ | Required | Phase 2B |
| Message History | ✅ | ✅ | Required | Phase 2B |
| DM Pairing | ✅ (auto) | ✅ (enhanced) | Required | Phase 2B |
| Polls | ✅ | ✅ | Done | - |

---

## 5. Implementation Phases

### Phase 1: Federated Authentication (Foundation)
**Estimated:** 3-4 days

1. Add `FederatedCredential` struct
2. Implement certificate loading (async)
3. Add Managed Identity support
4. Implement new token acquisition flow
5. Add deprecation warnings for client_secret
6. Update `MsTeamsChannel` trait

### Phase 2A: RSC Permissions
**Estimated:** 1-2 days

1. Add `RscPermissions` struct
2. Implement `RscPermissionManager`
3. Add Graph API declaration endpoint
4. Add config schema

### Phase 2B: Parallel Features
**Estimated:** 4-5 days (parallel tracks)

Track 1: Health Auto-Recovery
- Implement `HealthStatus` enum
- Add `HealthyChannel` trait
- Implement `ChannelHealthMonitor`

Track 2: Per-Team Policy Routing
- Add `TeamPolicy` struct
- Implement policy resolution
- Update access control checks

Track 3: SharePoint Upload
- Add `SharePointClient`
- Implement chunked upload
- Add share link creation

Track 4: Message History
- Add `HistoryFetcher`
- Implement pagination
- Add context fetching

Track 5: Enhanced DM Pairing
- Add `PairingState` enum
- Implement auto-pairing
- Add conversation management

---

## 6. Config Schema (Complete)

```toml
[msteams]
enabled = true
client_id = "your-app-id"
tenant_id = "your-tenant-id"

# Federated Authentication (recommended)
[msteams.federated_identity]
certificate_path = "/path/to/cert.pem"
certificate_password = "optional"
managed_identity_client_id = "optional"
authority_url = "https://login.microsoftonline.com/{{tenant}}"

# RSC Permissions
[msteams.rsc]
enabled = true
channel_message_read = true
channel_message_edit = true
channel_message_delete = false
member_access = true
file_access = true

# Health Monitoring
[msteams.health]
enabled = true
check_interval_seconds = 30
stale_threshold_seconds = 300
max_restart_attempts = 3

# Message History
[msteams.history]
enabled = true
max_messages = 50
retention_days = 7

# Policies
[msteams.policies]
default = { allow_dm = true }

[msteams.policies.teams."team-id"]
allow_dm = false
allowed_users = ["user@domain.com"]
blocked_users = []

# Legacy (deprecated, use federated_identity instead)
[msteams.client_secret]
secret = "your-secret"
```

---

## 7. Testing Strategy

### Unit Tests
- Token refresh logic
- Policy resolution
- Health status transitions
- Message parsing

### Integration Tests
- Graph API calls (mocked)
- Certificate loading
- SharePoint upload flow

### E2E Tests
- Full auth flow with test tenant
- Channel creation and messaging
- File upload in test channel

---

## 8. Open Questions & Decisions

| Question | Decision | Rationale |
|----------|----------|----------|
| Certificate format | PEM + PFX support | Flexibility for different setups |
| MI fallback | Fail fast if configured | MI is explicit opt-in |
| Config migration | Manual | Too risky to auto-migrate secrets |
| Deprecation timeline | 2026.06.01 | 6 weeks notice |
| SharePoint chunk size | 4MB | Matches Graph API limits |

---

## 9. References

- [OpenClaw MS Teams Plugin](file:///Volumes/TBU4/Github/openclaw/extensions/msteams)
- [Aleph MS Teams Module](file:///Volumes/TBU4/Workspace/Aleph/src/gateway/interfaces/msteams)
- [Microsoft Graph RSC Documentation](https://learn.microsoft.com/en-us/microsoftteams/platform/graph-api/rsc/)
- [Azure Managed Identity](https://learn.microsoft.com/en-us/azure/active-directory/managed-identities-azure-resources/)
