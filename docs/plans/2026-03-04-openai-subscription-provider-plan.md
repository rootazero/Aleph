# OpenAI Subscription Provider Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a ChatGPT subscription-based AI provider that authenticates via OAuth browser login and calls the ChatGPT backend API (`chatgpt.com/backend-api/conversation`), enabling users to use their Plus/Pro subscription quota instead of API keys.

**Architecture:** New `ChatGptProtocol` implementing the existing `ProtocolAdapter` trait, with dedicated auth (`ChatGptAuth`) and security (`ChatGptSecurity`) modules. Registers as a built-in protocol alongside OpenAI/Anthropic/Gemini. OAuth flow uses a temporary localhost HTTP server + system browser redirect.

**Tech Stack:** Rust, reqwest, tokio, axum (localhost OAuth callback), serde_json, uuid, sha2 (proof-of-work)

**Design Doc:** `docs/plans/2026-03-04-openai-subscription-provider-design.md`

---

### Task 1: ChatGPT Request/Response Types

**Files:**
- Create: `core/src/providers/chatgpt/mod.rs`
- Create: `core/src/providers/chatgpt/types.rs`
- Modify: `core/src/providers/mod.rs:54-70` (add `pub mod chatgpt;`)

**Step 1: Create the types module**

Create `core/src/providers/chatgpt/types.rs`:

```rust
//! ChatGPT backend-api request/response types

use serde::{Deserialize, Serialize};

/// ChatGPT backend-api conversation request
#[derive(Debug, Serialize)]
pub struct ChatGptRequest {
    pub action: String,
    pub messages: Vec<ChatGptMessage>,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    pub parent_message_id: String,
    pub timezone_offset_min: i32,
    pub conversation_mode: ConversationMode,
}

/// A message in the ChatGPT conversation
#[derive(Debug, Serialize)]
pub struct ChatGptMessage {
    pub id: String,
    pub author: Author,
    pub content: ChatGptContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Message author
#[derive(Debug, Serialize)]
pub struct Author {
    pub role: String,
}

/// Message content
#[derive(Debug, Serialize)]
pub struct ChatGptContent {
    pub content_type: String,
    pub parts: Vec<serde_json::Value>,
}

/// Conversation mode controls built-in tools
#[derive(Debug, Serialize)]
pub struct ConversationMode {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_ids: Option<Vec<String>>,
}

/// ChatGPT SSE response message wrapper
#[derive(Debug, Deserialize)]
pub struct ChatGptStreamResponse {
    pub message: Option<ChatGptResponseMessage>,
    pub conversation_id: Option<String>,
    pub error: Option<serde_json::Value>,
}

/// Response message from ChatGPT
#[derive(Debug, Deserialize)]
pub struct ChatGptResponseMessage {
    pub id: String,
    pub author: ResponseAuthor,
    pub content: ResponseContent,
    #[serde(default)]
    pub status: String,
}

/// Response author
#[derive(Debug, Deserialize)]
pub struct ResponseAuthor {
    pub role: String,
}

/// Response content
#[derive(Debug, Deserialize)]
pub struct ResponseContent {
    pub content_type: String,
    #[serde(default)]
    pub parts: Vec<serde_json::Value>,
}

/// ChatGPT available models response
#[derive(Debug, Deserialize)]
pub struct ModelsResponse {
    pub models: Vec<ModelInfo>,
}

/// Model information
#[derive(Debug, Deserialize)]
pub struct ModelInfo {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Chat requirements response (security tokens)
#[derive(Debug, Deserialize)]
pub struct ChatRequirements {
    pub token: String,
    #[serde(default)]
    pub proofofwork: Option<ProofOfWork>,
}

/// Proof-of-work challenge
#[derive(Debug, Deserialize)]
pub struct ProofOfWork {
    pub required: bool,
    pub seed: Option<String>,
    pub difficulty: Option<String>,
}
```

**Step 2: Create the module file**

Create `core/src/providers/chatgpt/mod.rs`:

```rust
//! ChatGPT subscription provider types
//!
//! Types for interacting with the ChatGPT backend API (chatgpt.com/backend-api).

pub mod types;

pub use types::*;
```

**Step 3: Register the module**

Modify `core/src/providers/mod.rs` — add `pub mod chatgpt;` after the existing module declarations (around line 55-70 where other `pub mod` declarations are).

**Step 4: Run `cargo check -p alephcore` to verify compilation**

Expected: compiles successfully (types are defined but not yet used)

**Step 5: Commit**

```bash
git add core/src/providers/chatgpt/
git add core/src/providers/mod.rs
git commit -m "chatgpt: add request/response types for backend-api"
```

---

### Task 2: Security Layer (CSRF + Requirements + Proof-of-Work)

**Files:**
- Create: `core/src/providers/chatgpt/security.rs`
- Modify: `core/src/providers/chatgpt/mod.rs` (add `pub mod security;`)

**Step 1: Write the failing test**

Add at the bottom of `core/src/providers/chatgpt/security.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_solve_proof_of_work_finds_valid_hash() {
        // Known seed and difficulty — PoW should find a hash with required prefix
        let result = ChatGptSecurity::solve_proof_of_work("test_seed_123", "0000");
        assert!(result.is_ok());
        let answer = result.unwrap();
        assert!(!answer.is_empty());
    }

    #[test]
    fn test_solve_proof_of_work_empty_difficulty_returns_empty() {
        let result = ChatGptSecurity::solve_proof_of_work("seed", "");
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib chatgpt::security -- --nocapture`
Expected: FAIL — module doesn't exist yet

**Step 3: Write the implementation**

Create `core/src/providers/chatgpt/security.rs`:

```rust
//! ChatGPT security layer
//!
//! Handles CSRF tokens, chat-requirements, and proof-of-work challenges
//! required by the ChatGPT backend API.

use crate::error::{AlephError, Result};
use reqwest::Client;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

use super::types::{ChatRequirements, ProofOfWork};

const CSRF_URL: &str = "https://chatgpt.com/api/auth/csrf";
const REQUIREMENTS_URL: &str = "https://chatgpt.com/backend-api/sentinel/chat-requirements";

/// ChatGPT security token manager
pub struct ChatGptSecurity;

impl ChatGptSecurity {
    /// Fetch CSRF token from the auth endpoint
    pub async fn fetch_csrf(client: &Client) -> Result<String> {
        let response = client
            .get(CSRF_URL)
            .send()
            .await
            .map_err(|e| AlephError::network(format!("Failed to fetch CSRF token: {}", e)))?;

        if !response.status().is_success() {
            return Err(AlephError::provider(format!(
                "CSRF fetch failed with status: {}",
                response.status()
            )));
        }

        let json: Value = response
            .json()
            .await
            .map_err(|e| AlephError::provider(format!("Failed to parse CSRF response: {}", e)))?;

        json["csrfToken"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| AlephError::provider("CSRF token not found in response"))
    }

    /// Fetch chat-requirements (security tokens + proof-of-work params)
    pub async fn fetch_requirements(
        client: &Client,
        access_token: &str,
    ) -> Result<ChatRequirements> {
        let response = client
            .post(REQUIREMENTS_URL)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| {
                AlephError::network(format!("Failed to fetch chat requirements: {}", e))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!(
                "Chat requirements failed ({}): {}",
                status, body
            )));
        }

        let requirements: ChatRequirements = response.json().await.map_err(|e| {
            AlephError::provider(format!("Failed to parse chat requirements: {}", e))
        })?;

        debug!(
            has_pow = requirements.proofofwork.is_some(),
            "Fetched chat requirements"
        );

        Ok(requirements)
    }

    /// Solve proof-of-work challenge
    ///
    /// Finds a nonce such that SHA-256(seed + nonce) starts with the required
    /// difficulty prefix (hex zeros).
    pub fn solve_proof_of_work(seed: &str, difficulty: &str) -> Result<String> {
        if difficulty.is_empty() {
            return Ok(String::new());
        }

        let max_iterations: u64 = 10_000_000;

        for nonce in 0..max_iterations {
            let input = format!("{}{}", seed, nonce);
            let hash = Sha256::digest(input.as_bytes());
            let hex = format!("{:x}", hash);

            if hex.starts_with(difficulty) {
                debug!(nonce, "Proof-of-work solved");
                return Ok(format!("gAAAAAB{}", nonce));
            }
        }

        warn!(seed, difficulty, "Proof-of-work exhausted max iterations");
        Err(AlephError::provider(
            "Failed to solve proof-of-work within iteration limit",
        ))
    }

    /// Build all security headers needed for a conversation request.
    /// Returns (requirements_token, pow_token_option).
    pub async fn prepare_security_tokens(
        client: &Client,
        access_token: &str,
    ) -> Result<(String, Option<String>)> {
        let requirements = Self::fetch_requirements(client, access_token).await?;

        let pow_token = if let Some(ProofOfWork {
            required: true,
            seed: Some(ref seed),
            difficulty: Some(ref diff),
        }) = requirements.proofofwork
        {
            Some(Self::solve_proof_of_work(seed, diff)?)
        } else {
            None
        };

        Ok((requirements.token, pow_token))
    }
}
```

**Step 4: Update mod.rs**

Add to `core/src/providers/chatgpt/mod.rs`:

```rust
pub mod security;
pub use security::ChatGptSecurity;
```

**Step 5: Add `sha2` dependency**

Run: `cargo add sha2 -p alephcore`

If `sha2` is already in the workspace, just add it to `core/Cargo.toml` dependencies.

**Step 6: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib chatgpt::security -- --nocapture`
Expected: 2 tests PASS

**Step 7: Commit**

```bash
git add core/src/providers/chatgpt/security.rs core/src/providers/chatgpt/mod.rs core/Cargo.toml
git commit -m "chatgpt: add security layer (CSRF, requirements, proof-of-work)"
```

---

### Task 3: OAuth Browser Authentication

**Files:**
- Create: `core/src/providers/chatgpt/auth.rs`
- Modify: `core/src/providers/chatgpt/mod.rs` (add `pub mod auth;`)

**Step 1: Write the failing test**

Add at the bottom of `core/src/providers/chatgpt/auth.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_state_token_expired() {
        let auth = ChatGptAuth {
            access_token: "test_token".to_string(),
            refresh_token: None,
            expires_at: SystemTime::UNIX_EPOCH, // expired
            session_id: "test_session".to_string(),
        };
        assert!(auth.is_expired());
    }

    #[test]
    fn test_auth_state_token_valid() {
        let auth = ChatGptAuth {
            access_token: "test_token".to_string(),
            refresh_token: None,
            expires_at: SystemTime::now() + Duration::from_secs(3600),
            session_id: "test_session".to_string(),
        };
        assert!(!auth.is_expired());
    }

    #[test]
    fn test_build_authorize_url() {
        let url = ChatGptAuth::build_authorize_url(12345, "random_state");
        assert!(url.contains("auth0.openai.com") || url.contains("auth.openai.com"));
        assert!(url.contains("12345"));
        assert!(url.contains("random_state"));
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib chatgpt::auth -- --nocapture`
Expected: FAIL — module doesn't exist yet

**Step 3: Write the implementation**

Create `core/src/providers/chatgpt/auth.rs`:

```rust
//! ChatGPT OAuth authentication
//!
//! Implements the browser-based OAuth flow for ChatGPT subscription accounts.
//! Opens system browser for user login, captures callback via localhost server.

use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use reqwest::Client;
use serde::Deserialize;
use std::time::{Duration, SystemTime};
use tokio::sync::oneshot;
use tracing::{debug, error, info};

/// OpenAI OAuth client ID (web client)
const OPENAI_CLIENT_ID: &str = "DRivsnm2Mu42T3KOpqdtwB3NYviHYzwD";

/// OpenAI auth endpoints
const AUTHORIZE_URL: &str = "https://auth0.openai.com/authorize";
const TOKEN_URL: &str = "https://auth0.openai.com/oauth/token";
const SESSION_URL: &str = "https://chatgpt.com/api/auth/session";

/// ChatGPT authentication state
#[derive(Debug, Clone)]
pub struct ChatGptAuth {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: SystemTime,
    pub session_id: String,
}

/// OAuth token response
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    #[allow(dead_code)]
    token_type: Option<String>,
}

/// Session response from ChatGPT
#[derive(Debug, Deserialize)]
struct SessionResponse {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[allow(dead_code)]
    user: Option<serde_json::Value>,
}

impl ChatGptAuth {
    /// Check if the access token is expired
    pub fn is_expired(&self) -> bool {
        SystemTime::now() >= self.expires_at
    }

    /// Get the access token, or error if expired
    pub fn access_token(&self) -> Result<&str> {
        if self.is_expired() {
            return Err(AlephError::provider("ChatGPT access token expired"));
        }
        Ok(&self.access_token)
    }

    /// Build the OAuth authorization URL
    pub fn build_authorize_url(port: u16, state: &str) -> String {
        format!(
            "{}?client_id={}&redirect_uri=http://localhost:{}/callback&response_type=code&scope=openid%20profile%20email&state={}&audience=https://api.openai.com/v1",
            AUTHORIZE_URL, OPENAI_CLIENT_ID, port, state
        )
    }

    /// Start the OAuth browser flow
    ///
    /// 1. Start a temporary localhost HTTP server
    /// 2. Open system browser to OpenAI login
    /// 3. Wait for callback with authorization code
    /// 4. Exchange code for access token
    pub async fn authorize_via_browser() -> Result<Self> {
        // Find an available port
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| AlephError::provider(format!("Failed to bind callback server: {}", e)))?;
        let port = listener.local_addr()
            .map_err(|e| AlephError::provider(format!("Failed to get server port: {}", e)))?
            .port();

        let state = uuid::Uuid::new_v4().to_string();
        let authorize_url = Self::build_authorize_url(port, &state);

        info!(port, "Starting OAuth callback server");

        // Channel to receive the auth code
        let (tx, rx) = oneshot::channel::<String>();
        let tx = Arc::new(tokio::sync::Mutex::new(Some(tx)));
        let expected_state = state.clone();

        // Start callback server
        let tx_clone = tx.clone();
        let server_handle = tokio::spawn(async move {
            use axum::{extract::Query, response::Html, routing::get, Router};

            #[derive(Deserialize)]
            struct CallbackParams {
                code: Option<String>,
                state: Option<String>,
                #[allow(dead_code)]
                error: Option<String>,
            }

            let tx_inner = tx_clone;
            let state_check = expected_state;

            let app = Router::new().route(
                "/callback",
                get(move |Query(params): Query<CallbackParams>| {
                    let tx = tx_inner.clone();
                    let expected = state_check.clone();
                    async move {
                        if let (Some(code), Some(state)) = (params.code, params.state) {
                            if state == expected {
                                if let Some(sender) = tx.lock().await.take() {
                                    let _ = sender.send(code);
                                }
                                Html("<html><body><h1>Authorization successful!</h1><p>You can close this window and return to Aleph.</p></body></html>".to_string())
                            } else {
                                Html("<html><body><h1>Error: State mismatch</h1></body></html>".to_string())
                            }
                        } else {
                            Html("<html><body><h1>Error: Missing authorization code</h1></body></html>".to_string())
                        }
                    }
                }),
            );

            axum::serve(listener, app)
                .await
                .unwrap_or_else(|e| error!("Callback server error: {}", e));
        });

        // Open browser
        debug!(url = %authorize_url, "Opening browser for OAuth login");
        if let Err(e) = open::that(&authorize_url) {
            error!(error = %e, "Failed to open browser — user must navigate manually");
            info!(url = %authorize_url, "Please open this URL in your browser");
        }

        // Wait for callback (with timeout)
        let code = tokio::time::timeout(Duration::from_secs(300), rx)
            .await
            .map_err(|_| AlephError::provider("OAuth login timed out (5 minutes)"))?
            .map_err(|_| AlephError::provider("OAuth callback channel closed"))?;

        // Abort the server
        server_handle.abort();

        debug!("Received authorization code, exchanging for token");

        // Exchange code for token
        Self::exchange_code_for_token(&code, port).await
    }

    /// Exchange authorization code for access token
    async fn exchange_code_for_token(code: &str, port: u16) -> Result<Self> {
        let client = Client::new();

        let response = client
            .post(TOKEN_URL)
            .json(&serde_json::json!({
                "grant_type": "authorization_code",
                "client_id": OPENAI_CLIENT_ID,
                "code": code,
                "redirect_uri": format!("http://localhost:{}/callback", port),
            }))
            .send()
            .await
            .map_err(|e| AlephError::network(format!("Token exchange failed: {}", e)))?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!("Token exchange error: {}", body)));
        }

        let token: TokenResponse = response.json().await.map_err(|e| {
            AlephError::provider(format!("Failed to parse token response: {}", e))
        })?;

        let expires_in = token.expires_in.unwrap_or(3600);

        info!("Successfully obtained ChatGPT access token");

        Ok(Self {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_at: SystemTime::now() + Duration::from_secs(expires_in),
            session_id: uuid::Uuid::new_v4().to_string(),
        })
    }

    /// Refresh the access token using the session endpoint
    pub async fn refresh(&mut self) -> Result<()> {
        let client = Client::new();

        let response = client
            .get(SESSION_URL)
            .header("Cookie", format!("__Secure-next-auth.session-token={}", self.access_token))
            .send()
            .await
            .map_err(|e| AlephError::network(format!("Session refresh failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(AlephError::provider("Session refresh failed — re-authorization required"));
        }

        let session: SessionResponse = response.json().await.map_err(|e| {
            AlephError::provider(format!("Failed to parse session response: {}", e))
        })?;

        if let Some(token) = session.access_token {
            self.access_token = token;
            self.expires_at = SystemTime::now() + Duration::from_secs(3600);
            debug!("Refreshed ChatGPT access token");
            Ok(())
        } else {
            Err(AlephError::provider("No access token in session response"))
        }
    }

    /// Ensure the token is valid, refreshing if needed
    pub async fn ensure_valid(&mut self) -> Result<&str> {
        if self.is_expired() {
            self.refresh().await?;
        }
        Ok(&self.access_token)
    }
}
```

**Step 4: Update mod.rs**

Add to `core/src/providers/chatgpt/mod.rs`:

```rust
pub mod auth;
pub use auth::ChatGptAuth;
```

**Step 5: Add dependencies if needed**

Check if `uuid`, `open`, `axum` are already in Cargo.toml. Add any that are missing:

```bash
# Check existing deps first
grep -E "uuid|open|axum" core/Cargo.toml
# Add missing ones
```

**Step 6: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib chatgpt::auth -- --nocapture`
Expected: 3 tests PASS

**Step 7: Commit**

```bash
git add core/src/providers/chatgpt/auth.rs core/src/providers/chatgpt/mod.rs
git commit -m "chatgpt: add OAuth browser authentication flow"
```

---

### Task 4: ChatGPT Protocol Adapter

**Files:**
- Create: `core/src/providers/protocols/chatgpt.rs`
- Modify: `core/src/providers/protocols/mod.rs:5-24` (add module and re-export)

**Step 1: Write the failing test**

Add at the bottom of `core/src/providers/protocols/chatgpt.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sse_line_text_content() {
        let line = r#"data: {"message":{"id":"abc","author":{"role":"assistant"},"content":{"content_type":"text","parts":["Hello world"]},"status":"in_progress"},"conversation_id":"conv123","error":null}"#;
        let result = ChatGptProtocol::parse_sse_line(line);
        assert_eq!(result, Some("Hello world".to_string()));
    }

    #[test]
    fn test_parse_sse_line_done() {
        let result = ChatGptProtocol::parse_sse_line("data: [DONE]");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_sse_line_non_text_content() {
        let line = r#"data: {"message":{"id":"abc","author":{"role":"assistant"},"content":{"content_type":"code","parts":["print('hi')"]},"status":"in_progress"},"conversation_id":"conv123","error":null}"#;
        let result = ChatGptProtocol::parse_sse_line(line);
        // Code content should still be extracted as text
        assert_eq!(result, Some("print('hi')".to_string()));
    }

    #[test]
    fn test_parse_sse_line_empty_parts() {
        let line = r#"data: {"message":{"id":"abc","author":{"role":"assistant"},"content":{"content_type":"text","parts":[]},"status":"in_progress"},"conversation_id":"conv123","error":null}"#;
        let result = ChatGptProtocol::parse_sse_line(line);
        assert_eq!(result, None);
    }

    #[test]
    fn test_build_conversation_request() {
        let request = ChatGptProtocol::build_conversation_request(
            "Hello",
            None,
            "gpt-4o",
            None,
            None,
        );
        assert_eq!(request.action, "next");
        assert_eq!(request.model, "gpt-4o");
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].author.role, "user");
        assert!(request.conversation_id.is_none());
    }

    #[test]
    fn test_build_conversation_request_with_system_prompt() {
        let request = ChatGptProtocol::build_conversation_request(
            "Hello",
            Some("You are helpful"),
            "gpt-4o",
            None,
            None,
        );
        // System prompt should be prepended to user message
        let parts = &request.messages[0].content.parts;
        let text = parts[0].as_str().unwrap();
        assert!(text.contains("You are helpful"));
        assert!(text.contains("Hello"));
    }

    #[test]
    fn test_adapter_name() {
        let adapter = ChatGptProtocol::new(Client::new());
        assert_eq!(adapter.name(), "chatgpt");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib protocols::chatgpt -- --nocapture`
Expected: FAIL — module doesn't exist yet

**Step 3: Write the implementation**

Create `core/src/providers/protocols/chatgpt.rs`:

```rust
//! ChatGPT backend-api protocol adapter
//!
//! Handles the ChatGPT subscription API format (chatgpt.com/backend-api).
//! This is NOT the official OpenAI API — it uses the ChatGPT web backend
//! with OAuth authentication instead of API keys.

use crate::config::ProviderConfig;
use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
use crate::providers::chatgpt::types::{
    Author, ChatGptContent, ChatGptMessage, ChatGptRequest, ChatGptStreamResponse,
    ConversationMode,
};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::TryStreamExt;
use reqwest::Client;
use tracing::{debug, error};

const CONVERSATION_ENDPOINT: &str = "/backend-api/conversation";

/// ChatGPT backend-api protocol adapter
pub struct ChatGptProtocol {
    client: Client,
}

impl ChatGptProtocol {
    /// Create a new ChatGPT protocol adapter
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Build the conversation endpoint URL
    fn build_endpoint(config: &ProviderConfig) -> String {
        let base_url = config
            .base_url
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_end_matches('/').to_string())
            .unwrap_or_else(|| "https://chatgpt.com".to_string());

        format!("{}{}", base_url, CONVERSATION_ENDPOINT)
    }

    /// Build a ChatGPT conversation request
    pub fn build_conversation_request(
        input: &str,
        system_prompt: Option<&str>,
        model: &str,
        conversation_id: Option<&str>,
        parent_message_id: Option<&str>,
    ) -> ChatGptRequest {
        // Combine system prompt with user input (prepend mode)
        let full_input = match system_prompt {
            Some(sp) => format!("{}\n\n{}", sp, input),
            None => input.to_string(),
        };

        let message_id = uuid::Uuid::new_v4().to_string();
        let parent_id = parent_message_id
            .map(|s| s.to_string())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        ChatGptRequest {
            action: "next".to_string(),
            messages: vec![ChatGptMessage {
                id: message_id,
                author: Author {
                    role: "user".to_string(),
                },
                content: ChatGptContent {
                    content_type: "text".to_string(),
                    parts: vec![serde_json::Value::String(full_input)],
                },
                metadata: None,
            }],
            model: model.to_string(),
            conversation_id: conversation_id.map(|s| s.to_string()),
            parent_message_id: parent_id,
            timezone_offset_min: 0,
            conversation_mode: ConversationMode {
                kind: "primary_assistant".to_string(),
                plugin_ids: None,
            },
        }
    }

    /// Parse a single SSE line from the ChatGPT stream
    ///
    /// ChatGPT streams responses as:
    /// ```text
    /// data: {"message":{"content":{"parts":["text"]},...},...}
    /// data: [DONE]
    /// ```
    fn parse_sse_line(line: &str) -> Option<String> {
        if !line.starts_with("data: ") {
            return None;
        }
        let data = &line[6..];
        if data == "[DONE]" {
            return None;
        }

        let parsed: ChatGptStreamResponse = serde_json::from_str(data).ok()?;
        let message = parsed.message?;

        // Extract text from parts
        let parts = &message.content.parts;
        if parts.is_empty() {
            return None;
        }

        // Get the last part's text content
        parts.last().and_then(|p| p.as_str()).map(|s| s.to_string())
    }
}

#[async_trait]
impl ProtocolAdapter for ChatGptProtocol {
    fn build_request(
        &self,
        payload: &RequestPayload,
        config: &ProviderConfig,
        is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder> {
        let endpoint = Self::build_endpoint(config);

        let request = Self::build_conversation_request(
            payload.input,
            payload.system_prompt,
            &config.model,
            None, // conversation_id managed externally
            None, // parent_message_id managed externally
        );

        // For ChatGPT backend-api, the access_token is stored in api_key field
        // after OAuth flow completes
        let access_token = config
            .api_key
            .as_ref()
            .ok_or_else(|| {
                AlephError::invalid_config(
                    "ChatGPT access token not set — run OAuth login first",
                )
            })?;

        debug!(
            endpoint = %endpoint,
            model = %config.model,
            streaming = is_streaming,
            "Building ChatGPT request"
        );

        let mut builder = self
            .client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");

        // Note: CSRF and requirements tokens should be set by the caller
        // via config or middleware before this point. For now we build
        // the base request; security headers are added at the HttpProvider level.

        builder = builder.json(&request);

        Ok(builder)
    }

    async fn parse_response(&self, response: reqwest::Response) -> Result<String> {
        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            error!(status = %status, error = %error_text, "ChatGPT API error");

            if status.as_u16() == 401 {
                return Err(AlephError::provider(
                    "ChatGPT authentication expired — please re-login",
                ));
            }
            if status.as_u16() == 429 {
                return Err(AlephError::provider(
                    "ChatGPT subscription rate limit reached — please try again later",
                ));
            }

            return Err(AlephError::provider(format!(
                "ChatGPT API error ({}): {}",
                status, error_text
            )));
        }

        // ChatGPT always returns SSE, even for "non-streaming" requests
        // We collect all events and return the final text
        let text = response.text().await.map_err(|e| {
            AlephError::provider(format!("Failed to read ChatGPT response: {}", e))
        })?;

        let mut result = String::new();
        for line in text.lines() {
            if let Some(content) = Self::parse_sse_line(line) {
                result = content; // Keep the last (most complete) text
            }
        }

        if result.is_empty() {
            Err(AlephError::provider("Empty response from ChatGPT"))
        } else {
            Ok(result)
        }
    }

    async fn parse_stream(
        &self,
        response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<String>>> {
        let status = response.status();

        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!(
                "ChatGPT API error ({}): {}",
                status, error_text
            )));
        }

        // Track the previous text to compute incremental deltas
        // ChatGPT sends full text in each event, not deltas
        let prev_text = std::sync::Arc::new(std::sync::Mutex::new(String::new()));

        let stream = response
            .bytes_stream()
            .map_err(|e| AlephError::network(format!("Stream error: {}", e)))
            .try_filter_map(move |chunk| {
                let prev = prev_text.clone();
                async move {
                    let text = std::str::from_utf8(&chunk)
                        .map_err(|e| AlephError::provider(format!("UTF-8 error: {}", e)))?;

                    let mut delta = String::new();
                    for line in text.lines() {
                        if let Some(full_text) = Self::parse_sse_line(line) {
                            let mut prev_guard =
                                prev.lock().unwrap_or_else(|e| e.into_inner());

                            // Compute incremental delta
                            if full_text.len() > prev_guard.len() {
                                let new_part = &full_text[prev_guard.len()..];
                                delta.push_str(new_part);
                            } else if full_text != *prev_guard {
                                // Text changed entirely (e.g., tool output)
                                delta.push_str(&full_text);
                            }
                            *prev_guard = full_text;
                        }
                    }

                    if delta.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(delta))
                    }
                }
            });

        Ok(Box::pin(stream))
    }

    fn name(&self) -> &'static str {
        "chatgpt"
    }
}
```

**Step 4: Register in protocols/mod.rs**

Add to `core/src/providers/protocols/mod.rs`:

```rust
pub mod chatgpt;
pub use chatgpt::ChatGptProtocol;
```

**Step 5: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib protocols::chatgpt -- --nocapture`
Expected: 7 tests PASS

**Step 6: Commit**

```bash
git add core/src/providers/protocols/chatgpt.rs core/src/providers/protocols/mod.rs
git commit -m "chatgpt: add protocol adapter for backend-api"
```

---

### Task 5: Register ChatGPT in Provider System

**Files:**
- Modify: `core/src/providers/presets.rs:22-293` (add preset entry)
- Modify: `core/src/providers/protocols/registry.rs:41-61` (add to built-in)
- Modify: `core/src/providers/mod.rs:120-127` (add to doc comment)

**Step 1: Write the failing test**

Add to `core/src/providers/protocols/registry.rs` tests section:

```rust
#[test]
fn test_chatgpt_protocol_registered() {
    let registry = ProtocolRegistry::new();
    registry.register_builtin();
    assert!(registry.get("chatgpt").is_some(), "chatgpt protocol should be registered");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib protocols::registry::tests::test_chatgpt_protocol_registered -- --nocapture`
Expected: FAIL — chatgpt not registered

**Step 3: Add preset entry**

Add to `core/src/providers/presets.rs`, inside the `PRESETS` HashMap initialization (after the OpenAI entry around line 34):

```rust
    // ChatGPT subscription (via backend-api, OAuth login)
    m.insert(
        "chatgpt",
        ProviderPreset {
            base_url: "https://chatgpt.com",
            protocol: "chatgpt",
            color: "#10a37f",
            default_model: "gpt-4o",
        },
    );
```

**Step 4: Register in protocol registry**

Add to `core/src/providers/protocols/registry.rs` in `register_builtin()` method (after the gemini insert around line 60):

```rust
        builtin.insert(
            "chatgpt".to_string(),
            (|client| Arc::new(ChatGptProtocol::new(client)) as Arc<dyn ProtocolAdapter>)
                as ProtocolFactory,
        );
```

Also add the import at the top of `registry.rs`:

```rust
use crate::providers::protocols::{AnthropicProtocol, ChatGptProtocol, GeminiProtocol, OpenAiProtocol};
```

**Step 5: Update doc comment in mod.rs**

In `core/src/providers/mod.rs`, around line 124-127, add to the supported protocols list:

```rust
/// - `"chatgpt"` - ChatGPT subscription backend API (via OAuth)
```

**Step 6: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib protocols::registry -- --nocapture`
Expected: 3 tests PASS (including the new one)

**Step 7: Run full check**

Run: `cargo check -p alephcore`
Expected: compiles successfully

**Step 8: Commit**

```bash
git add core/src/providers/presets.rs core/src/providers/protocols/registry.rs core/src/providers/mod.rs
git commit -m "chatgpt: register protocol in provider system (preset + registry)"
```

---

### Task 6: Integration Smoke Test

**Files:**
- Modify: `core/src/providers/protocols/chatgpt.rs` (add integration test section)

**Step 1: Write integration test**

Add to the test section at the bottom of `core/src/providers/protocols/chatgpt.rs`:

```rust
    #[test]
    fn test_create_chatgpt_provider_via_factory() {
        use crate::config::ProviderConfig;
        use crate::providers::create_provider;

        let config = ProviderConfig {
            protocol: Some("chatgpt".to_string()),
            model: "gpt-4o".to_string(),
            api_key: Some("test_token".to_string()),
            base_url: Some("https://chatgpt.com".to_string()),
            enabled: true,
            ..ProviderConfig::test_config()
        };

        let provider = create_provider("chatgpt-sub", config);
        assert!(provider.is_ok(), "Should create chatgpt provider: {:?}", provider.err());

        let p = provider.unwrap();
        assert_eq!(p.name(), "chatgpt-sub");
        assert!(p.supports_vision());
    }

    #[test]
    fn test_chatgpt_preset_applied() {
        use crate::providers::presets::get_preset;

        let preset = get_preset("chatgpt");
        assert!(preset.is_some(), "chatgpt preset should exist");

        let p = preset.unwrap();
        assert_eq!(p.protocol, "chatgpt");
        assert_eq!(p.base_url, "https://chatgpt.com");
        assert_eq!(p.default_model, "gpt-4o");
    }
```

**Step 2: Run tests**

Run: `cargo test -p alephcore --lib protocols::chatgpt -- --nocapture`
Expected: All tests PASS

**Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib`
Expected: All existing tests still pass (no regressions)

**Step 4: Commit**

```bash
git add core/src/providers/protocols/chatgpt.rs
git commit -m "chatgpt: add integration smoke tests"
```

---

### Task 7: Configuration Example & Documentation Update

**Files:**
- Modify: `core/config.search.example.toml` (add chatgpt example)
- Modify: `docs/reference/ARCHITECTURE.md` (mention chatgpt provider if providers are listed)

**Step 1: Add configuration example**

Add to `core/config.search.example.toml` (after existing provider examples):

```toml
# ChatGPT Subscription (uses OAuth login instead of API key)
# Run `aleph auth chatgpt` to connect your subscription account
[providers.chatgpt-sub]
protocol = "chatgpt"
model = "gpt-4o"
color = "#10a37f"
timeout_seconds = 120
enabled = false
```

**Step 2: Commit**

```bash
git add core/config.search.example.toml
git commit -m "chatgpt: add configuration example for subscription provider"
```

---

### Summary

| Task | Description | Est. Tests |
|------|-------------|------------|
| 1 | Request/response types | 0 (compile check) |
| 2 | Security layer (CSRF, PoW) | 2 |
| 3 | OAuth browser authentication | 3 |
| 4 | ChatGPT protocol adapter | 7 |
| 5 | Register in provider system | 1 |
| 6 | Integration smoke test | 2 |
| 7 | Config example & docs | 0 |

**Total new tests:** 15
**Total new files:** 5 (`types.rs`, `mod.rs`, `security.rs`, `auth.rs`, `chatgpt.rs` protocol)
**Modified files:** 4 (`providers/mod.rs`, `protocols/mod.rs`, `registry.rs`, `presets.rs`)
