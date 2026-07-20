# MS Teams Channel Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Achieve complete feature parity between Aleph's MS Teams channel and OpenClaw's MS Teams plugin by implementing: Federated Auth, RSC Permissions, Health Auto-Recovery, Per-Team Policy Routing, SharePoint File Upload, Message History, and Enhanced DM Pairing.

**Architecture:** Two-phase approach. Phase 1 implements Federated Authentication as the foundation for all Graph API features. Phase 2 adds remaining features in parallel tracks. All features integrate with existing Aleph channel abstraction via trait extensions.

**Tech Stack:** Rust (tokio, reqwest, serde, thiserror, tokio-rustls), alephcore workspace

---

## File Structure Overview

```
src/gateway/interfaces/msteams/
├── mod.rs                          # Module root (modify)
├── config.rs                       # MsTeamsConfig extension (create)
├── auth.rs                         # Federated credentials (create)
├── token.rs                        # TokenManager updates (create)
├── graph.rs                        # GraphClient updates (create)
├── rsc.rs                          # RSC PermissionManager (create)
├── health.rs                       # Health monitor (create)
├── policy.rs                       # Team policies (create)
├── sharepoint.rs                   # SharePoint client (create)
├── history.rs                      # History fetcher (create)
├── pairing.rs                      # DM pairing (create)
└── types.rs                        # Shared types (create/modify)

crates/alephcore/src/gateway/interfaces/mod.rs  # Interface registration
docs/superpowers/specs/2026-04-15-msteams-parity-design.md  # Reference
```

---

## Phase 1: Federated Authentication (Foundation)

### Task 1: Add FederatedCredential and AuthFlow Types

**Files:**
- Create: `src/gateway/interfaces/msteams/auth.rs`
- Modify: `src/gateway/interfaces/msteams/config.rs`
- Test: `src/gateway/interfaces/msteams/tests/auth_tests.rs`

- [ ] **Step 1: Create FederatedCredential struct**

```rust
// src/gateway/interfaces/msteams/auth.rs

use std::path::PathBuf;
use serde::{Deserialize, Serialize};

/// Federated identity configuration for certificate-based or MI auth
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedCredential {
    /// Path to certificate file (.pem or .pfx)
    pub certificate_path: PathBuf,
    /// Optional password for .pfx files
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate_password: Option<String>,
    /// Azure Managed Identity client ID (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub managed_identity_client_id: Option<String>,
    /// Authority URL: https://login.microsoftonline.com/{tenant}
    pub authority_url: String,
}

impl FederatedCredential {
    /// Validate the configuration
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.certificate_path.exists() {
            return Err("Certificate file does not exist");
        }
        if !self.authority_url.contains("{tenant}") {
            return Err("authority_url must contain {tenant} placeholder");
        }
        Ok(())
    }
}
```

- [ ] **Step 2: Add AuthFlow enum to config.rs**

```rust
// Add to src/gateway/interfaces/msteams/config.rs

/// Authentication flow type
#[derive(Debug, Clone)]
pub enum AuthFlow {
    /// Client secret (deprecated)
    ClientSecret(String),
    /// Federated identity (certificate + optional MI)
    Federated(FederatedCredential),
}

impl AuthFlow {
    /// Returns true if using federated auth
    pub fn is_federated(&self) -> bool {
        matches!(self, AuthFlow::Federated(_))
    }
}
```

- [ ] **Step 3: Create auth_tests.rs**

```rust
// src/gateway/interfaces/msteams/tests/auth_tests.rs

use super::*;

#[test]
fn test_federated_credential_validate_ok() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cert_path = temp_dir.path().join("cert.pem");
    std::fs::write(&cert_path, "dummy cert").unwrap();

    let cred = FederatedCredential {
        certificate_path: cert_path,
        certificate_password: None,
        managed_identity_client_id: None,
        authority_url: "https://login.microsoftonline.com/{tenant}".to_string(),
    };

    assert!(cred.validate().is_ok());
}

#[test]
fn test_federated_credential_validate_missing_file() {
    let cred = FederatedCredential {
        certificate_path: PathBuf::from("/nonexistent/cert.pem"),
        certificate_password: None,
        managed_identity_client_id: None,
        authority_url: "https://login.microsoftonline.com/{tenant}".to_string(),
    };

    assert!(cred.validate().is_err());
}

#[test]
fn test_auth_flow_is_federated() {
    let secret_flow = AuthFlow::ClientSecret("secret".to_string());
    assert!(!secret_flow.is_federated());

    let cert_path = PathBuf::from("/path/cert.pem");
    let fed_flow = AuthFlow::Federated(FederatedCredential {
        certificate_path: cert_path,
        certificate_password: None,
        managed_identity_client_id: None,
        authority_url: "https://login.microsoftonline.com/{tenant}".to_string(),
    });
    assert!(fed_flow.is_federated());
}
```

- [ ] **Step 4: Run tests to verify**

Run: `cargo test -p alephcore msteams::auth_tests`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/msteams/auth.rs src/gateway/interfaces/msteams/config.rs
git add src/gateway/interfaces/msteams/tests/auth_tests.rs
git commit -m "msteams: add FederatedCredential and AuthFlow types"
```

---

### Task 2: Implement Certificate Loading (Async)

**Files:**
- Modify: `src/gateway/interfaces/msteams/auth.rs`
- Create: `src/gateway/interfaces/msteams/tests/cert_tests.rs`

- [ ] **Step 1: Add async certificate loading**

```rust
// Add to auth.rs

use tokio::fs;
use rustls::{Certificate, PrivateKey, pems};

pub enum CertificateFormat {
    Pem,
    Pfx,
}

impl CertificateFormat {
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "pem" => Some(CertificateFormat::Pem),
            "pfx" | "p12" => Some(CertificateFormat::Pfx),
            _ => None,
        }
    }
}

/// Load certificate from file asynchronously
pub async fn load_certificate(path: &Path) -> Result<Certificate, Error> {
    let contents = fs::read(path).await?;
    let cert = Certificate(contents);
    Ok(cert)
}

/// Load private key from PEM file
pub async fn load_private_key_from_pem(path: &Path) -> Result<PrivateKey, Error> {
    let contents = fs::read(path).await?;
    let key = PrivateKey(contents);
    Ok(key)
}
```

- [ ] **Step 2: Create cert loading test**

```rust
// src/gateway/interfaces/msteams/tests/cert_tests.rs

use super::*;

#[tokio::test]
async fn test_load_certificate_from_pem() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cert_path = temp_dir.path().join("cert.pem");
    std::fs::write(&cert_path, "dummy cert content").unwrap();

    let result = load_certificate(&cert_path).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_certificate_format_detection() {
    let pem_path = Path::new("/path/to/cert.pem");
    let pfx_path = Path::new("/path/to/cert.pfx");

    assert_eq!(CertificateFormat::from_path(pem_path), Some(CertificateFormat::Pem));
    assert_eq!(CertificateFormat::from_path(pfx_path), Some(CertificateFormat::Pfx));
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore msteams::cert_tests`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/msteams/auth.rs src/gateway/interfaces/msteams/tests/cert_tests.rs
git commit -m "msteams: add async certificate loading"
```

---

### Task 3: Implement Managed Identity Token Acquisition

**Files:**
- Modify: `src/gateway/interfaces/msteams/auth.rs`
- Create: `src/gateway/interfaces/msteams/tests/mi_tests.rs`

- [ ] **Step 1: Add Managed Identity support**

```rust
// Add to auth.rs

/// Azure Instance Metadata Service endpoint
const IMDS_ENDPOINT: &str = "http://169.254.169.254/metadata/identity/oauth2/token";

/// Managed Identity token response
#[derive(Debug, Deserialize)]
struct ImdsTokenResponse {
    access_token: String,
    expires_in: String,
    resource: String,
}

/// Acquire token via Azure Managed Identity
pub async fn acquire_token_via_managed_identity(
    client_id: &str,
    resource: &str,
) -> Result<String, Error> {
    let url = format!(
        "{}?api-version=2021-02-01&resource={}",
        IMDS_ENDPOINT, resource
    );

    let mut req = reqwest::Client::new().get(&url);
    req = req.header("Metadata", "true");

    if !client_id.is_empty() {
        req = req.query(&[("client_id", client_id)]);
    }

    let response: ImdsTokenResponse = req.send().await?.json().await?;
    Ok(response.access_token)
}
```

- [ ] **Step 2: Create MI test with mocked endpoint**

```rust
// src/gateway/interfaces/msteams/tests/mi_tests.rs

#[tokio::test]
async fn test_mi_token_response_parsing() {
    let json = r#"{"access_token":"test-token","expires_in":"3600","resource":"https://graph.microsoft.com"}"#;
    let response: ImdsTokenResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.access_token, "test-token");
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore msteams::mi_tests`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/gateway/interfaces/msteams/auth.rs
git add src/gateway/interfaces/msteams/tests/mi_tests.rs
git commit -m "msteams: add Managed Identity token acquisition"
```

---

### Task 4: Implement TokenManager with Federated Auth

**Files:**
- Modify: `src/gateway/interfaces/msteams/token.rs` (create if not exists)
- Create: `src/gateway/interfaces/msteams/tests/token_tests.rs`

- [ ] **Step 1: Create TokenManager with federated support**

```rust
// src/gateway/interfaces/msteams/token.rs

use crate::gateway::error::Error;
use super::auth::{AuthFlow, FederatedCredential};
use std::sync::Arc;
use tokio::sync::RwLock;
use std::time::{Instant, Duration};

/// Token with expiration tracking
#[derive(Clone)]
pub struct Token {
    pub access_token: String,
    pub expires_at: Instant,
}

impl Token {
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }

    pub fn is_expiring_soon(&self) -> bool {
        // Refresh at 80% of lifetime
        let remaining = self.expires_at.saturating_duration_since(Instant::now());
        remaining < Duration::from_secs(300) // 5 minutes
    }
}

/// Token manager with multi-auth support
pub struct TokenManager {
    auth_flow: AuthFlow,
    graph_client: Arc<GraphClient>,
    cached_token: RwLock<Option<Token>>,
}

impl TokenManager {
    pub fn new(auth_flow: AuthFlow, graph_client: Arc<GraphClient>) -> Self {
        Self {
            auth_flow,
            graph_client,
            cached_token: RwLock::new(None),
        }
    }

    /// Get valid token (from cache or acquire new)
    pub async fn get_token(&self) -> Result<String, Error> {
        let token = self.cached_token.read().await;
        if let Some(ref t) = *token {
            if !t.is_expired() {
                return Ok(t.access_token.clone());
            }
        }
        drop(token);

        self.acquire_and_cache_token().await
    }

    /// Acquire new token based on auth flow
    async fn acquire_and_cache_token(&self) -> Result<String, Error> {
        let (access_token, expires_in) = match &self.auth_flow {
            AuthFlow::ClientSecret(secret) => {
                self.acquire_token_via_secret(secret).await?
            }
            AuthFlow::Federated(cred) => {
                self.acquire_token_via_federated(cred).await?
            }
        };

        let token = Token {
            access_token: access_token.clone(),
            expires_at: Instant::now() + Duration::from_secs(expires_in),
        };

        *self.cached_token.write().await = Some(token);
        Ok(access_token)
    }

    /// Acquire token using client secret
    async fn acquire_token_via_secret(&self, secret: &str) -> Result<(String, u64), Error> {
        // Existing implementation
        self.graph_client
            .acquire_token_with_secret(secret)
            .await
    }

    /// Acquire token using federated identity
    async fn acquire_token_via_federated(&self, cred: &FederatedCredential) -> Result<(String, u64), Error> {
        // 1. Load certificate
        let cert = load_certificate(&cred.certificate_path).await?;

        // 2. If MI configured, get MI token first
        let mi_token = if let Some(ref mi_client_id) = cred.managed_identity_client_id {
            Some(acquire_token_via_managed_identity(mi_client_id, "https://management.azure.com").await?)
        } else {
            None
        };

        // 3. Use certificate (+ MI token if available) to get Graph token
        self.graph_client
            .acquire_token_with_certificate(cert, mi_token, &cred.authority_url)
            .await
    }

    /// Force refresh token (for proactive refresh at 80% lifetime)
    pub async fn refresh_if_needed(&self) -> Result<(), Error> {
        let needs_refresh = {
            let token = self.cached_token.read().await;
            token.as_ref().map(|t| t.is_expiring_soon()).unwrap_or(true)
        };

        if needs_refresh {
            self.acquire_and_cache_token().await?;
        }
        Ok(())
    }
}
```

- [ ] **Step 2: Add GraphClient trait for token acquisition**

```rust
// Add to graph.rs (or token.rs)

#[async_trait::async_trait]
pub trait GraphTokenAcquisition {
    async fn acquire_token_with_secret(&self, secret: &str) -> Result<(String, u64), Error>;
    async fn acquire_token_with_certificate(
        &self,
        cert: Certificate,
        mi_token: Option<String>,
        authority: &str,
    ) -> Result<(String, u64), Error>;
}
```

- [ ] **Step 3: Create token tests**

```rust
// src/gateway/interfaces/msteams/tests/token_tests.rs

#[tokio::test]
async fn test_token_is_expired() {
    let token = Token {
        access_token: "test".to_string(),
        expires_at: Instant::now() - Duration::from_secs(1),
    };
    assert!(token.is_expired());
}

#[tokio::test]
async fn test_token_is_expiring_soon() {
    let token = Token {
        access_token: "test".to_string(),
        expires_at: Instant::now() + Duration::from_secs(60), // 1 minute
    };
    assert!(token.is_expiring_soon());
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore msteams::token_tests`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/msteams/token.rs
git add src/gateway/interfaces/msteams/graph.rs  # if modified
git add src/gateway/interfaces/msteams/tests/token_tests.rs
git commit -m "msteams: implement TokenManager with federated auth"
```

---

### Task 5: Update MsTeamsChannel Trait with Auth Introspection

**Files:**
- Modify: `src/gateway/interfaces/msteams/mod.rs`
- Create: `src/gateway/interfaces/msteams/tests/trait_tests.rs`

- [ ] **Step 1: Add trait extension**

```rust
// Add to mod.rs

/// Extension trait for auth introspection
pub trait MsTeamsAuthExt {
    /// Returns true if using federated authentication
    fn uses_federated_auth(&self) -> bool;

    /// Returns the auth method name for logging
    fn auth_method(&self) -> &'static str {
        if self.uses_federated_auth() {
            "federated"
        } else {
            "client_secret"
        }
    }
}

impl<T: MsTeamsChannel + ?Sized> MsTeamsAuthExt for T {
    fn uses_federated_auth(&self) -> bool {
        self.config().auth_flow.is_federated()
    }
}
```

- [ ] **Step 2: Run tests to verify compilation**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 3: Commit**

```bash
git add src/gateway/interfaces/msteams/mod.rs
git commit -m "msteams: add MsTeamsAuthExt trait for auth introspection"
```

---

## Phase 2A: RSC Permissions

### Task 6: Implement RscPermissionManager

**Files:**
- Create: `src/gateway/interfaces/msteams/rsc.rs`
- Modify: `src/gateway/interfaces/msteams/config.rs`
- Create: `src/gateway/interfaces/msteams/tests/rsc_tests.rs`

- [ ] **Step 1: Create RscPermissions struct**

```rust
// src/gateway/interfaces/msteams/rsc.rs

use serde::{Deserialize, Serialize};

/// RSC permissions for Teams
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RscPermissions {
    #[serde(rename = "channelMessage.Read")]
    pub channel_message_read: bool,
    #[serde(rename = "channelMessage.Edit")]
    pub channel_message_edit: bool,
    #[serde(rename = "channelMessage.Delete")]
    pub channel_message_delete: bool,
    #[serde(rename = "member.Read")]
    pub member_access: bool,
    #[serde(rename = "fileAccess")]
    pub file_access: bool,
}

/// RSC Permission Manager
pub struct RscPermissionManager {
    graph_client: Arc<GraphClient>,
    permissions: RscPermissions,
}

impl RscPermissionManager {
    pub fn new(graph_client: Arc<GraphClient>, permissions: RscPermissions) -> Self {
        Self {
            graph_client,
            permissions,
        }
    }

    /// Declare permissions to Graph API
    /// POST /teams/{teamId}/appPermissions
    pub async fn declare_permissions(&self, team_id: &str) -> Result<(), Error> {
        let payload = serde_json::json!({
            "requiredResourceSpecificPermissions": [
                {
                    "resource": "ChannelMessage.Read.Group",
                    "delegated": ["ChannelMessage.Read.Group"],
                    "application": self.permissions.channel_message_read
                },
                {
                    "resource": "ChannelMessage.Read",
                    "delegated": ["ChannelMessage.Read"],
                    "application": self.permissions.channel_message_read
                }
            ]
        });

        self.graph_client
            .post(&format!("/teams/{}/appPermissions", team_id), payload)
            .await?;

        Ok(())
    }
}
```

- [ ] **Step 2: Add RSC config**

```rust
// Add to config.rs

#[derive(Debug, Clone, Deserialize)]
pub struct RscConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub channel_message_read: bool,
    #[serde(default)]
    pub channel_message_edit: bool,
    #[serde(default)]
    pub channel_message_delete: bool,
    #[serde(default)]
    pub member_access: bool,
    #[serde(default)]
    pub file_access: bool,
}

impl Default for RscConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            channel_message_read: false,
            channel_message_edit: false,
            channel_message_delete: false,
            member_access: false,
            file_access: false,
        }
    }
}
```

- [ ] **Step 3: Create RSC tests**

```rust
// src/gateway/interfaces/msteams/tests/rsc_tests.rs

use super::*;

#[test]
fn test_rsc_permissions_default() {
    let perms = RscPermissions::default();
    assert!(!perms.channel_message_read);
    assert!(!perms.channel_message_edit);
}

#[test]
fn test_rsc_config_default() {
    let config = RscConfig::default();
    assert!(!config.enabled);
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore msteams::rsc_tests`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/msteams/rsc.rs src/gateway/interfaces/msteams/config.rs
git add src/gateway/interfaces/msteams/tests/rsc_tests.rs
git commit -m "msteams: implement RSC PermissionManager"
```

---

## Phase 2B: Parallel Feature Tracks

### Track 1: Health Auto-Recovery

### Task 7: Implement Health Monitoring and Auto-Recovery

**Files:**
- Create: `src/gateway/interfaces/msteams/health.rs`
- Create: `src/gateway/interfaces/msteams/tests/health_tests.rs`

- [ ] **Step 1: Create HealthStatus enum**

```rust
// src/gateway/interfaces/msteams/health.rs

use std::time::Instant;

/// Health status of a channel
#[derive(Debug, Clone)]
pub enum HealthStatus {
    /// Healthy: receiving messages normally
    Healthy,
    /// Degraded: missed some health checks
    Degraded { missed_checks: u32 },
    /// Stale: no messages for threshold duration
    Stale { last_activity: Instant },
    /// Restarting: recovering from stale state
    Restarting,
}

impl HealthStatus {
    pub fn is_healthy(&self) -> bool {
        matches!(self, HealthStatus::Healthy)
    }
}
```

- [ ] **Step 2: Create HealthyChannel trait**

```rust
/// Channel with health monitoring capability
pub trait HealthyChannel: Send + Sync {
    /// Check if channel is stale
    async fn is_stale(&self) -> bool;

    /// Graceful restart
    async fn restart(&mut self) -> Result<(), Error>;

    /// Get current health status
    async fn health_status(&self) -> HealthStatus;
}
```

- [ ] **Step 3: Create ChannelHealthMonitor**

```rust
/// Monitor with auto-recovery
pub struct ChannelHealthMonitor {
    check_interval: Duration,
    stale_threshold: Duration,
    max_restart_attempts: u32,
    restart_attempts: AtomicU32,
}

impl ChannelHealthMonitor {
    pub fn new(
        check_interval: Duration,
        stale_threshold: Duration,
        max_restart_attempts: u32,
    ) -> Self {
        Self {
            check_interval,
            stale_threshold,
            max_restart_attempts,
            restart_attempts: AtomicU32::new(0),
        }
    }

    pub async fn check_and_recover<C: HealthyChannel>(&self, channel: &mut C) -> Result<()> {
        if channel.is_stale().await {
            let attempts = self.restart_attempts.load(Ordering::SeqCst);
            if attempts >= self.max_restart_attempts {
                return Err(anyhow!("Max restart attempts exceeded"));
            }

            self.restart_attempts.fetch_add(1, Ordering::SeqCst);
            channel.restart().await?;
            self.restart_attempts.store(0, Ordering::SeqCst);
        }
        Ok(())
    }
}
```

- [ ] **Step 4: Create health tests**

```rust
// src/gateway/interfaces/msteams/tests/health_tests.rs

use super::*;

#[test]
fn test_health_status_is_healthy() {
    assert!(HealthStatus::Healthy.is_healthy());
    assert!(!HealthStatus::Stale { last_activity: Instant::now() }.is_healthy());
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p alephcore msteams::health_tests`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/gateway/interfaces/msteams/health.rs src/gateway/interfaces/msteams/tests/health_tests.rs
git commit -m "msteams: implement HealthMonitor with auto-recovery"
```

---

### Track 2: Per-Team Policy Routing

### Task 8: Implement Team Policy System

**Files:**
- Create: `src/gateway/interfaces/msteams/policy.rs`
- Modify: `src/gateway/interfaces/msteams/config.rs`
- Create: `src/gateway/interfaces/msteams/tests/policy_tests.rs`

- [ ] **Step 1: Create TeamPolicy struct**

```rust
// src/gateway/interfaces/msteams/policy.rs

use crate::domain::UserId;
use serde::{Deserialize, Serialize};

/// Policy for a specific team
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamPolicy {
    pub team_id: String,
    #[serde(default = "default_allow_dm")]
    pub allow_dm: bool,
    #[serde(default)]
    pub allowed_users: Vec<UserId>,
    #[serde(default)]
    pub blocked_users: Vec<UserId>,
}

fn default_allow_dm() -> bool {
    true
}

impl TeamPolicy {
    pub fn new(team_id: String) -> Self {
        Self {
            team_id,
            allow_dm: true,
            allowed_users: Vec::new(),
            blocked_users: Vec::new(),
        }
    }

    pub fn is_user_allowed(&self, user_id: &UserId) -> bool {
        if self.blocked_users.contains(user_id) {
            return false;
        }
        if !self.allowed_users.is_empty() && !self.allowed_users.contains(user_id) {
            return false;
        }
        true
    }
}

/// Channel config with policy support
#[derive(Debug, Clone)]
pub struct PolicyConfig {
    pub default: TeamPolicy,
    pub teams: HashMap<String, TeamPolicy>,
}

impl PolicyConfig {
    pub fn effective_policy(&self, team_id: &str) -> &TeamPolicy {
        self.teams.get(team_id).unwrap_or(&self.default)
    }
}
```

- [ ] **Step 2: Add PolicyConfig to MsTeamsConfig**

```rust
// Add to config.rs

use super::policy::PolicyConfig;

#[derive(Debug, Clone)]
pub struct MsTeamsConfig {
    // ... existing fields ...
    pub policies: PolicyConfig,
}
```

- [ ] **Step 3: Create policy tests**

```rust
// src/gateway/interfaces/msteams/tests/policy_tests.rs

use super::*;

#[test]
fn test_team_policy_blocked_user() {
    let policy = TeamPolicy {
        team_id: "team1".to_string(),
        allow_dm: true,
        allowed_users: vec![],
        blocked_users: vec![UserId::new("blocked")],
    };

    assert!(!policy.is_user_allowed(&UserId::new("blocked")));
    assert!(policy.is_user_allowed(&UserId::new("other")));
}

#[test]
fn test_effective_policy_fallback() {
    let config = PolicyConfig {
        default: TeamPolicy::new("default".to_string()),
        teams: HashMap::new(),
    };

    assert_eq!(config.effective_policy("unknown-team").team_id, "default");
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore msteams::policy_tests`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/msteams/policy.rs src/gateway/interfaces/msteams/config.rs
git add src/gateway/interfaces/msteams/tests/policy_tests.rs
git commit -m "msteams: implement per-team policy routing"
```

---

### Track 3: SharePoint File Upload

### Task 9: Implement SharePoint Client

**Files:**
- Create: `src/gateway/interfaces/msteams/sharepoint.rs`
- Create: `src/gateway/interfaces/msteams/tests/sharepoint_tests.rs`

- [ ] **Step 1: Create SharePointClient**

```rust
// src/gateway/interfaces/msteams/sharepoint.rs

use bytes::Bytes;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug)]
pub struct ShareLink {
    pub web_url: url::Url,
    pub expiration_date_time: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize)]
struct UploadSessionResponse {
    upload_url: String,
}

#[derive(Debug, Deserialize)]
struct PermissionResponse {
    link: ShareLinkResponse,
}

#[derive(Debug, Deserialize)]
struct ShareLinkResponse {
    web_url: String,
    #[serde(rename = "expirationDateTime")]
    expiration_date_time: Option<String>,
}

/// SharePoint client for file operations
pub struct SharePointClient {
    graph_client: Arc<GraphClient>,
}

impl SharePointClient {
    pub fn new(graph_client: Arc<GraphClient>) -> Self {
        Self { graph_client }
    }

    /// Get channel's SharePoint drive
    async fn get_channel_drive(&self, team_id: &str, channel_id: &str) -> Result<String, Error> {
        #[derive(Deserialize)]
        struct DriveResponse {
            id: String,
        }

        let response: DriveResponse = self
            .graph_client
            .get(&format!(
                "/teams/{}/channels/{}/drive",
                team_id, channel_id
            ))
            .await?;

        Ok(response.id)
    }

    /// Upload file to SharePoint
    pub async fn upload_file(
        &self,
        team_id: &str,
        channel_id: &str,
        file_name: &str,
        content: Bytes,
    ) -> Result<ShareLink, Error> {
        // 1. Get drive ID
        let drive_id = self.get_channel_drive(team_id, channel_id).await?;

        // 2. Create upload session
        let session: UploadSessionResponse = self
            .graph_client
            .put(
                &format!("/drives/{}/root:/{}/createUploadSession", drive_id, file_name),
                (),
            )
            .await?;

        // 3. Chunked upload
        self.upload_chunks(&session.upload_url, content).await?;

        // 4. Create share link
        self.create_share_link(&drive_id, file_name).await
    }

    /// Chunked upload using async streams
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
    async fn create_share_link(&self, drive_id: &str, file_name: &str) -> Result<ShareLink, Error> {
        let response: PermissionResponse = self
            .graph_client
            .post(
                &format!("/drives/{}/items/{}:/createLink", drive_id, file_name),
                serde_json::json!({
                    "type": "view",
                    "scope": "organization"
                }),
            )
            .await?;

        Ok(ShareLink {
            web_url: url::Url::parse(&response.link.web_url)?,
            expiration_date_time: response.link.expiration_date_time.and_then(|s| {
                chrono::DateTime::parse_from_rfc3339(&s).ok().map(|dt| dt.with_timezone(&chrono::Utc))
            }),
        })
    }
}
```

- [ ] **Step 2: Add put_raw method to GraphClient**

```rust
// Add to graph.rs

impl GraphClient {
    /// Raw PUT with custom headers
    pub async fn put_raw<T: Serialize + ?Sized>(
        &self,
        url: &str,
        body: &[u8],
        headers: &[(&str, &str)],
    ) -> Result<T, Error> {
        let mut req = self.client.put(&format!("{}{}", self.base_url, url));
        req = req.header("Content-Type", "application/octet-stream");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        req = req.body(body.to_vec());
        let response = req.send().await?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::from(status));
        }
        let body = response.bytes().await?;
        Ok(serde_json::from_slice(&body)?)
    }
}
```

- [ ] **Step 3: Create SharePoint tests**

```rust
// src/gateway/interfaces/msteams/tests/sharepoint_tests.rs

use super::*;

#[test]
fn test_share_link_response_parsing() {
    let json = r#"{
        "link": {
            "web_url": "https://tenant.sharepoint.com/sites/site/Shared%20Documents/file.txt",
            "expirationDateTime": "2026-12-31T23:59:59Z"
        }
    }"#;
    let response: PermissionResponse = serde_json::from_str(json).unwrap();
    assert!(response.link.web_url.contains("sharepoint.com"));
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore msteams::sharepoint_tests`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/msteams/sharepoint.rs src/gateway/interfaces/msteams/graph.rs
git add src/gateway/interfaces/msteams/tests/sharepoint_tests.rs
git commit -m "msteams: implement SharePoint file upload"
```

---

### Track 4: Message History

### Task 10: Implement HistoryFetcher

**Files:**
- Create: `src/gateway/interfaces/msteams/history.rs`
- Create: `src/gateway/interfaces/msteams/tests/history_tests.rs`

- [ ] **Step 1: Create GraphMessage and HistoryFetcher**

```rust
// src/gateway/interfaces/msteams/history.rs

use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct GraphMessage {
    pub id: String,
    #[serde(rename = "createdDateTime")]
    pub created_date_time: DateTime<Utc>,
    pub from: MessageFrom,
    pub body: MessageBody,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub mentions: Vec<Mention>,
}

#[derive(Debug, Deserialize)]
pub struct MessageFrom {
    pub user: Option<User>,
    #[serde(default)]
    pub application: Option<Application>,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub id: String,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Application {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct MessageBody {
    #[serde(rename = "contentType")]
    pub content_type: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct Attachment {
    pub id: String,
    #[serde(rename = "contentType")]
    pub content_type: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Mention {
    pub id: i32,
    pub mentioned: Mentioned,
}

#[derive(Debug, Deserialize)]
pub struct Mentioned {
    pub user: Option<User>,
}

#[derive(Debug, Deserialize)]
pub struct MessagesResponse {
    pub messages: Vec<GraphMessage>,
    #[serde(rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}

/// Fetches channel message history
pub struct HistoryFetcher {
    graph_client: Arc<GraphClient>,
    max_messages: usize,
}

impl HistoryFetcher {
    pub fn new(graph_client: Arc<GraphClient>, max_messages: usize) -> Self {
        Self {
            graph_client,
            max_messages,
        }
    }

    /// Fetch recent messages from channel
    pub async fn fetch_history(
        &self,
        team_id: &str,
        channel_id: &str,
        before_message_id: Option<&str>,
    ) -> Result<Vec<GraphMessage>, Error> {
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

    /// Fetch single message with full context
    pub async fn fetch_message(
        &self,
        team_id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> Result<GraphMessage, Error> {
        let endpoint = format!(
            "/teams/{}/channels/{}/messages/{}",
            team_id, channel_id, message_id
        );

        let message: GraphMessage = self.graph_client.get(&endpoint, &[]).await?;
        Ok(message)
    }
}
```

- [ ] **Step 2: Add HistoryConfig to config.rs**

```rust
// Add to config.rs

#[derive(Debug, Clone, Deserialize)]
pub struct HistoryConfig {
    #[serde(default = "default_history_enabled")]
    pub enabled: bool,
    #[serde(default = "default_max_messages")]
    pub max_messages: usize,
    #[serde(default = "default_retention_days")]
    pub retention_days: usize,
}

fn default_history_enabled() -> bool {
    true
}

fn default_max_messages() -> usize {
    50
}

fn default_retention_days() -> usize {
    7
}
```

- [ ] **Step 3: Create history tests**

```rust
// src/gateway/interfaces/msteams/tests/history_tests.rs

use super::*;

#[test]
fn test_graph_message_parsing() {
    let json = r#"{
        "id": "msg123",
        "createdDateTime": "2026-04-15T10:30:00Z",
        "from": {
            "user": {
                "id": "user123",
                "displayName": "Test User"
            }
        },
        "body": {
            "contentType": "html",
            "content": "<p>Hello</p>"
        }
    }"#;

    let msg: GraphMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.id, "msg123");
    assert_eq!(msg.from.user.as_ref().unwrap().id, "user123");
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore msteams::history_tests`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/msteams/history.rs src/gateway/interfaces/msteams/config.rs
git add src/gateway/interfaces/msteams/tests/history_tests.rs
git commit -m "msteams: implement HistoryFetcher for message retrieval"
```

---

### Track 5: Enhanced DM Pairing

### Task 11: Implement Auto-Pairing DM System

**Files:**
- Create: `src/gateway/interfaces/msteams/pairing.rs`
- Modify: `src/gateway/interfaces/msteams/mod.rs`
- Create: `src/gateway/interfaces/msteams/tests/pairing_tests.rs`

- [ ] **Step 1: Create PairingState and PairingInfo**

```rust
// src/gateway/interfaces/msteams/pairing.rs

use std::time::Instant;
use crate::domain::UserId;
use serde::{Deserialize, Serialize};

/// DM Pairing state
#[derive(Debug, Clone)]
pub enum PairingState {
    Unpaired,
    Paired(PairingInfo),
}

#[derive(Debug, Clone)]
pub struct PairingInfo {
    pub user_id: UserId,
    pub user_email: String,
    pub direct_line_token: String,
    pub created_at: Instant,
}

impl PairingState {
    pub fn is_paired(&self) -> bool {
        matches!(self, PairingState::Paired(_))
    }
}

/// DM Pairing manager
pub struct PairingManager {
    graph_client: Arc<GraphClient>,
    state: RwLock<PairingState>,
}

impl PairingManager {
    pub fn new(graph_client: Arc<GraphClient>) -> Self {
        Self {
            graph_client,
            state: RwLock::new(PairingState::Unpaired).into(),
        }
    }

    pub async fn handle_dm(&self, message: &crate::msteams::GraphMessage) -> Result<Option<PairingInfo>, Error> {
        let user = message.from.user.as_ref().ok_or_else(|| Error::msg("No user in message"))?;
        let user_id = UserId::new(user.id.clone());
        let user_email = user.display_name.clone().unwrap_or_default();

        let mut state = self.state.write().await;

        match &*state {
            PairingState::Unpaired => {
                // Auto-pair on first DM
                info!("Auto-pairing with user {}", user_id);
                let pairing_info = self.create_pairing(&user_id, &user_email).await?;
                *state = PairingState::Paired(pairing_info.clone());
                Ok(Some(pairing_info))
            }
            PairingState::Paired(info) => {
                if info.user_id == user_id {
                    Ok(Some(info.clone()))
                } else {
                    warn!("DM from {} but paired with {}", user_id, info.user_id);
                    Ok(None)
                }
            }
        }
    }

    async fn create_pairing(&self, user_id: &UserId, email: &str) -> Result<PairingInfo, Error> {
        // Create one-on-one chat
        #[derive(Serialize)]
        struct CreateChatRequest {
            #[serde(rename = "chatType")]
            chat_type: String,
            members: Vec<Member>,
        }

        #[derive(Serialize)]
        struct Member {
            #[serde(rename = "@odata.type")]
            odata_type: String,
            roles: Vec<String>,
            #[serde(rename = "userId")]
            user_id: String,
        }

        let request = CreateChatRequest {
            chat_type: "oneOnOne".to_string(),
            members: vec![Member {
                odata_type: "#microsoft.graph.aadUserConversationMember".to_string(),
                roles: vec!["user".to_string()],
                user_id: user_id.to_string(),
            }],
        };

        #[derive(Deserialize)]
        struct ChatResponse {
            id: String,
        }

        let chat: ChatResponse = self.graph_client.post("/me/chats", serde_json::to_value(&request)?).await?;

        // Get direct line token
        let token = self.get_direct_line_token(&chat.id).await?;

        Ok(PairingInfo {
            user_id: user_id.clone(),
            user_email: email.to_string(),
            direct_line_token: token,
            created_at: Instant::now(),
        })
    }

    async fn get_direct_line_token(&self, conversation_id: &str) -> Result<String, Error> {
        #[derive(Deserialize)]
        struct TokenResponse {
            token: String,
        }

        let response: TokenResponse = self.graph_client
            .post(
                &format!("/conversations/{}/messages", conversation_id),
                (),
            )
            .await?;

        Ok(response.token)
    }
}
```

- [ ] **Step 2: Update MsTeamsChannel trait**

```rust
// Add to mod.rs

use super::pairing::{PairingState, PairingManager};

pub trait MsTeamsChannel: Send + Sync {
    // ... existing methods ...

    /// Handle incoming DM with auto-pairing
    async fn handle_dm(&mut self, message: &GraphMessage) -> Result<Option<DirectLine>>;
}
```

- [ ] **Step 3: Create pairing tests**

```rust
// src/gateway/interfaces/msteams/tests/pairing_tests.rs

use super::*;

#[test]
fn test_pairing_state_is_paired() {
    assert!(!PairingState::Unpaired.is_paired());
    assert!(PairingState::Paired(PairingInfo {
        user_id: UserId::new("test"),
        user_email: "test@test.com".to_string(),
        direct_line_token: "token".to_string(),
        created_at: Instant::now(),
    }).is_paired());
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore msteams::pairing_tests`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/msteams/pairing.rs src/gateway/interfaces/msteams/mod.rs
git add src/gateway/interfaces/msteams/tests/pairing_tests.rs
git commit -m "msteams: implement auto-pairing DM system"
```

---

## Final Integration

### Task 12: Update Module Exports and Documentation

**Files:**
- Modify: `src/gateway/interfaces/msteams/mod.rs`
- Modify: `src/gateway/interfaces/msteams/config.rs`
- Create: `docs/reference/MS_TEAMS_CHANNEL.md`

- [ ] **Step 1: Update mod.rs exports**

```rust
// src/gateway/interfaces/msteams/mod.rs

pub mod auth;
pub mod config;
pub mod graph;
pub mod health;
pub mod history;
pub mod pairing;
pub mod policy;
pub mod rsc;
pub mod sharepoint;
pub mod token;

pub use auth::{AuthFlow, FederatedCredential};
pub use config::{MsTeamsConfig, RscConfig, HistoryConfig, PolicyConfig};
pub use graph::GraphClient;
pub use health::{ChannelHealthMonitor, HealthStatus, HealthyChannel};
pub use history::{GraphMessage, HistoryFetcher};
pub use pairing::{PairingManager, PairingState, PairingInfo};
pub use policy::{TeamPolicy, PolicyConfig};
pub use rsc::{RscPermissionManager, RscPermissions};
pub use sharepoint::{SharePointClient, ShareLink};
pub use token::TokenManager;
```

- [ ] **Step 2: Final compilation check**

Run: `cargo check -p alephcore`
Expected: No errors

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All tests PASS

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -p alephcore -- -D warnings`
Expected: No warnings

- [ ] **Step 5: Commit**

```bash
git add src/gateway/interfaces/msteams/mod.rs
git add docs/reference/MS_TEAMS_CHANNEL.md
git commit -m "msteams: final integration and module exports"
```

---

## Summary

| Task | Feature | Files | Status |
|------|---------|-------|--------|
| 1 | FederatedCredential types | auth.rs, config.rs | Pending |
| 2 | Certificate loading | auth.rs | Pending |
| 3 | Managed Identity | auth.rs | Pending |
| 4 | TokenManager | token.rs | Pending |
| 5 | MsTeamsChannel trait | mod.rs | Pending |
| 6 | RSC Permissions | rsc.rs, config.rs | Pending |
| 7 | Health Auto-Recovery | health.rs | Pending |
| 8 | Per-Team Policy | policy.rs, config.rs | Pending |
| 9 | SharePoint Upload | sharepoint.rs | Pending |
| 10 | Message History | history.rs | Pending |
| 11 | DM Pairing | pairing.rs, mod.rs | Pending |
| 12 | Integration | mod.rs | Pending |

---

## Dependencies

- `tokio` - async runtime
- `reqwest` - HTTP client
- `rustls` - TLS certificates
- `serde` / `serde_json` - serialization
- `thiserror` - error handling
- `chrono` - datetime handling
- `bytes` - binary data
- `url` - URL parsing
- `tokio-rustls` - async TLS

---

## References

- Design: `docs/superpowers/specs/2026-04-15-msteams-parity-design.md`
- OpenClaw MS Teams: `/Volumes/TBU4/Github/openclaw/extensions/msteams`
- Aleph MS Teams: `src/gateway/interfaces/msteams/`
