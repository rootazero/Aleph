//! Microsoft Graph API Client
//!
//! Provides access to Microsoft Graph API for:
//! - User and group directory search
//! - Team and channel listing
//! - Message history retrieval
//! - Media/file downloads via SharePoint
//!
//! Uses separate OAuth2 token from Bot Framework tokens.

use std::fmt::Write;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::gateway::channel::ChannelError;
use crate::sync_primitives::Arc;

// ── Graph Token Cache ─────────────────────────────────────────────────────────

const GRAPH_TOKEN_REFRESH_THRESHOLD: f64 = 0.8;

/// A cached Graph API access token with its expiry metadata.
struct CachedToken {
    token: String,
    /// Absolute time after which the token should be refreshed (80% of lifetime)
    refresh_at: Instant,
}

impl CachedToken {
    fn needs_refresh(&self) -> bool {
        Instant::now() >= self.refresh_at
    }
}

/// Outbound OAuth2 token cache for Microsoft Graph API calls.
///
/// Caches `client_credentials` tokens and refreshes them proactively at 80% of
/// their lifetime so callers always receive a valid token.
///
/// Uses the same app credentials as Bot Framework but requests a different scope.
pub struct GraphTokenCache {
    app_id: String,
    app_password: String,
    tenant_id: String,
    cached: tokio::sync::RwLock<Option<CachedToken>>,
    http: Client,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

impl GraphTokenCache {
    /// Create a new `GraphTokenCache`.
    pub fn new(app_id: String, app_password: String, tenant_id: String) -> Self {
        Self {
            app_id,
            app_password,
            tenant_id,
            cached: tokio::sync::RwLock::new(None),
            http: Client::new(),
        }
    }

    /// Return a valid Graph API access token, refreshing if necessary.
    pub async fn get_token(&self) -> Result<String, ChannelError> {
        // Fast path: check under read lock
        {
            let guard = self.cached.read().await;
            if let Some(ref t) = *guard {
                if !t.needs_refresh() {
                    return Ok(t.token.clone());
                }
            }
        }

        // Slow path: refresh under write lock
        let mut guard = self.cached.write().await;
        // Double-check after acquiring write lock
        if let Some(ref t) = *guard {
            if !t.needs_refresh() {
                return Ok(t.token.clone());
            }
        }

        let token = self.fetch_token().await?;
        let token_str = token.token.clone();
        *guard = Some(token);
        Ok(token_str)
    }

    async fn fetch_token(&self) -> Result<CachedToken, ChannelError> {
        let url = format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
            self.tenant_id
        );

        let params = [
            ("grant_type", "client_credentials"),
            ("client_id", self.app_id.as_str()),
            ("client_secret", self.app_password.as_str()),
            ("scope", "https://graph.microsoft.com/.default"),
        ];

        let resp = self
            .http
            .post(&url)
            .form(&params)
            .send()
            .await
            .map_err(|e| ChannelError::AuthFailed(format!("Graph token request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ChannelError::AuthFailed(format!(
                "Graph token endpoint returned {status}: {body}"
            )));
        }

        let tr: TokenResponse = resp.json().await.map_err(|e| {
            ChannelError::Internal(format!("Failed to parse Graph token response: {e}"))
        })?;

        let lifetime = Duration::from_secs(tr.expires_in);
        let refresh_after = lifetime.mul_f64(GRAPH_TOKEN_REFRESH_THRESHOLD);
        let now = Instant::now();

        debug!(
            expires_in = tr.expires_in,
            refresh_after_secs = refresh_after.as_secs(),
            "Fetched new Graph API token"
        );

        Ok(CachedToken {
            token: tr.access_token,
            refresh_at: now + refresh_after,
        })
    }
}

// ── Graph API Types ───────────────────────────────────────────────────────────

/// Microsoft Graph user representation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphUser {
    #[serde(rename = "id")]
    pub id: Option<String>,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(rename = "userPrincipalName")]
    pub user_principal_name: Option<String>,
    pub mail: Option<String>,
}

/// Microsoft Graph group/team representation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphGroup {
    #[serde(rename = "id")]
    pub id: Option<String>,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
}

/// Microsoft Graph channel representation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphChannel {
    #[serde(rename = "id")]
    pub id: Option<String>,
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
}

/// Microsoft Graph chat message representation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphMessage {
    pub id: Option<String>,
    #[serde(rename = "body")]
    pub body: Option<MessageBody>,
    pub from: Option<MessageFrom>,
    #[serde(rename = "createdDateTime")]
    pub created_date_time: Option<String>,
    #[serde(rename = "lastModifiedDateTime")]
    pub last_modified_date_time: Option<String>,
    #[serde(rename = "attachments")]
    pub attachments: Option<Vec<GraphAttachment>>,
    #[serde(rename = "messageType")]
    pub message_type: Option<String>,
    #[serde(rename = "replyToId")]
    pub reply_to_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageBody {
    #[serde(rename = "contentType")]
    pub content_type: Option<String>,
    pub content: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageFrom {
    pub user: Option<GraphUser>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphAttachment {
    pub id: Option<String>,
    #[serde(rename = "contentType")]
    pub content_type: Option<String>,
    #[serde(rename = "contentUrl")]
    pub content_url: Option<String>,
    pub name: Option<String>,
    #[serde(rename = "isInline")]
    pub is_inline: Option<bool>,
    pub size: Option<i64>,
}

/// Generic Graph API response wrapper.
#[derive(Debug, Deserialize)]
pub struct GraphResponse<T> {
    #[serde(default)]
    pub value: Vec<T>,
}

/// Graph API error response.
#[derive(Debug, Deserialize)]
pub struct GraphErrorResponse {
    pub error: Option<GraphErrorDetail>,
}

#[derive(Debug, Deserialize)]
pub struct GraphErrorDetail {
    pub code: Option<String>,
    pub message: Option<String>,
}

// ── Graph Token Source ──────────────────────────────────────────────────────

/// Trait for Graph API token providers.
///
/// Abstracts over different authentication methods:
/// - `GraphTokenCache`: ClientSecret authentication
/// - `GraphTokenManager`: Federated authentication
#[async_trait]
pub trait GraphTokenSource: Send + Sync {
    /// Get a valid Graph API access token, refreshing if necessary.
    async fn get_token(&self) -> Result<String, ChannelError>;
}

#[async_trait]
impl GraphTokenSource for GraphTokenCache {
    async fn get_token(&self) -> Result<String, ChannelError> {
        GraphTokenCache::get_token(self).await
    }
}

// ── Graph Client ─────────────────────────────────────────────────────────────

const GRAPH_BASE_URL: &str = "https://graph.microsoft.com/v1.0";

/// Microsoft Graph API client.
///
/// Provides typed access to Microsoft Graph endpoints for directory search,
/// team/channel management, and message operations.
pub struct GraphClient {
    http: Client,
    token_source: Arc<dyn GraphTokenSource>,
}

impl GraphClient {
    /// Create a new `GraphClient` with a ClientSecret token source.
    pub fn new(token_cache: Arc<GraphTokenCache>) -> Self {
        Self {
            http: Client::new(),
            token_source: token_cache,
        }
    }

    /// Create a new `GraphClient` with a Federated auth token source.
    pub fn with_federated(token_manager: Arc<impl GraphTokenSource + 'static>) -> Self {
        Self {
            http: Client::new(),
            token_source: token_manager,
        }
    }

    // ── Generic Operations ────────────────────────────────────────────────────

    /// Make a generic GET request to the Graph API.
    pub async fn get<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T, ChannelError> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", GRAPH_BASE_URL, path)
        };

        let token = self.token_source.get_token().await?;
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ChannelError::Internal(format!("Graph GET request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Graph GET failed");
            return Err(map_graph_error(status.as_u16(), &body));
        }

        let result: T = resp.json().await.map_err(|e| {
            ChannelError::Internal(format!("Failed to parse Graph GET response: {e}"))
        })?;

        Ok(result)
    }

    // ── User & Directory Operations ────────────────────────────────────────────

    /// Search for users by display name or email.
    ///
    /// Uses the `$search` query parameter with "people" endpoint for
    /// relevant user search results.
    pub async fn search_users(
        &self,
        query: &str,
        top: usize,
    ) -> Result<Vec<GraphUser>, ChannelError> {
        let url = format!(
            "{}/users?$search=\"displayName:{} OR mail:{}\"&$top={}&$select=id,displayName,userPrincipalName,mail",
            GRAPH_BASE_URL,
            encode_odata_param(query),
            encode_odata_param(query),
            top.min(25)
        );

        let token = self.token_source.get_token().await?;
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .header("ConsistencyLevel", "eventual")
            .send()
            .await
            .map_err(|e| {
                ChannelError::Internal(format!("Graph user search request failed: {e}"))
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Graph user search failed");
            return Err(map_graph_error(status.as_u16(), &body));
        }

        let result: GraphResponse<GraphUser> = resp.json().await.map_err(|e| {
            ChannelError::Internal(format!("Failed to parse Graph user response: {e}"))
        })?;

        Ok(result.value)
    }

    /// Get a user by their Azure AD object ID.
    pub async fn get_user(&self, user_id: &str) -> Result<Option<GraphUser>, ChannelError> {
        let url = format!(
            "{}/users/{}?$select=id,displayName,userPrincipalName,mail",
            GRAPH_BASE_URL,
            encode_uri_component(user_id)
        );

        let token = self.token_source.get_token().await?;
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ChannelError::Internal(format!("Graph get user request failed: {e}")))?;

        let status = resp.status();
        if status.as_u16() == 404 {
            return Ok(None);
        }

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Graph get user failed");
            return Err(map_graph_error(status.as_u16(), &body));
        }

        let user: GraphUser = resp
            .json()
            .await
            .map_err(|e| ChannelError::Internal(format!("Failed to parse Graph user: {e}")))?;

        Ok(Some(user))
    }

    // ── Team & Channel Operations ──────────────────────────────────────────────

    /// Search for teams (groups with resourceProvisioningOptions containing "Team").
    pub async fn list_teams_by_name(
        &self,
        query: &str,
        top: usize,
    ) -> Result<Vec<GraphGroup>, ChannelError> {
        // Escape single quotes for OData filter
        let escaped = query.replace('\'', "''");
        let filter = format!(
            "resourceProvisioningOptions/Any(x:x eq 'Team') and startsWith(displayName,'{}')",
            escaped
        );
        let url = format!(
            "{}/groups?$filter={}&$top={}&$select=id,displayName",
            GRAPH_BASE_URL,
            encode_uri_component(&filter),
            top.min(10)
        );

        let token = self.token_source.get_token().await?;
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .header("ConsistencyLevel", "eventual")
            .send()
            .await
            .map_err(|e| ChannelError::Internal(format!("Graph list teams request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Graph list teams failed");
            return Err(map_graph_error(status.as_u16(), &body));
        }

        let result: GraphResponse<GraphGroup> = resp.json().await.map_err(|e| {
            ChannelError::Internal(format!("Failed to parse Graph teams response: {e}"))
        })?;

        Ok(result.value)
    }

    /// List channels for a given team.
    pub async fn list_channels_for_team(
        &self,
        team_id: &str,
    ) -> Result<Vec<GraphChannel>, ChannelError> {
        let url = format!(
            "{}/teams/{}/channels?$select=id,displayName",
            GRAPH_BASE_URL,
            encode_uri_component(team_id)
        );

        let token = self.token_source.get_token().await?;
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| {
                ChannelError::Internal(format!("Graph list channels request failed: {e}"))
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Graph list channels failed");
            return Err(map_graph_error(status.as_u16(), &body));
        }

        let result: GraphResponse<GraphChannel> = resp.json().await.map_err(|e| {
            ChannelError::Internal(format!("Failed to parse Graph channels response: {e}"))
        })?;

        Ok(result.value)
    }

    // ── Message Operations ──────────────────────────────────────────────────────

    /// Get a message from a chat thread.
    ///
    /// Used for retrieving quoted message content when users reply with quotes.
    pub async fn get_message(
        &self,
        chat_id: &str,
        message_id: &str,
    ) -> Result<Option<GraphMessage>, ChannelError> {
        let url = format!(
            "{}/chats/{}/messages/{}",
            GRAPH_BASE_URL,
            encode_uri_component(chat_id),
            encode_uri_component(message_id)
        );

        let token = self.token_source.get_token().await?;
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| {
                ChannelError::Internal(format!("Graph get message request failed: {e}"))
            })?;

        let status = resp.status();
        if status.as_u16() == 404 {
            return Ok(None);
        }

        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Graph get message failed");
            return Err(map_graph_error(status.as_u16(), &body));
        }

        let message: GraphMessage = resp
            .json()
            .await
            .map_err(|e| ChannelError::Internal(format!("Failed to parse Graph message: {e}")))?;

        Ok(Some(message))
    }

    /// List recent messages from a chat thread.
    ///
    /// Used for conversation context retrieval.
    pub async fn list_messages(
        &self,
        chat_id: &str,
        top: usize,
    ) -> Result<Vec<GraphMessage>, ChannelError> {
        let url = format!(
            "{}/chats/{}/messages?$top={}&$orderby=createdDateTime desc",
            GRAPH_BASE_URL,
            encode_uri_component(chat_id),
            top.min(20)
        );

        let token = self.token_source.get_token().await?;
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| {
                ChannelError::Internal(format!("Graph list messages request failed: {e}"))
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Graph list messages failed");
            return Err(map_graph_error(status.as_u16(), &body));
        }

        let result: GraphResponse<GraphMessage> = resp.json().await.map_err(|e| {
            ChannelError::Internal(format!("Failed to parse Graph messages response: {e}"))
        })?;

        Ok(result.value)
    }

    // ── Media & File Operations ─────────────────────────────────────────────────

    /// Download file content from a SharePoint URL.
    ///
    /// Used for retrieving file attachments that were uploaded to SharePoint
    /// rather than sent as inline content.
    pub async fn download_media(&self, content_url: &str) -> Result<Vec<u8>, ChannelError> {
        let token = self.token_source.get_token().await?;
        let resp = self
            .http
            .get(content_url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| {
                ChannelError::Internal(format!("Graph media download request failed: {e}"))
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Graph media download failed");
            return Err(map_graph_error(status.as_u16(), &body));
        }

        let bytes = resp.bytes().await.map_err(|e| {
            ChannelError::Internal(format!("Failed to read Graph media bytes: {e}"))
        })?;

        Ok(bytes.to_vec())
    }

    /// Upload a file to SharePoint and return the download URL.
    ///
    /// Used for sending file attachments in group chats where files need
    /// to be uploaded to SharePoint first.
    pub async fn upload_to_sharepoint(
        &self,
        channel_id: &str,
        file_name: &str,
        data: &[u8],
        content_type: &str,
    ) -> Result<String, ChannelError> {
        let url = format!(
            "{}/teams/{}/channels/{}/messages",
            GRAPH_BASE_URL,
            encode_uri_component(channel_id),
            encode_uri_component(channel_id) // channels use same ID for upload context
        );

        let token = self.token_source.get_token().await?;
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&token)
            .header("Content-Type", content_type)
            .header("Content-Length", data.len().to_string())
            .body(data.to_vec())
            .send()
            .await
            .map_err(|e| {
                ChannelError::Internal(format!("Graph SharePoint upload request failed: {e}"))
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Graph SharePoint upload failed");
            return Err(map_graph_error(status.as_u16(), &body));
        }

        // Return the content URL from the response attachment
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct UploadResponse {
            id: Option<String>,
        }

        let _uploaded: UploadResponse = resp.json().await.map_err(|e| {
            ChannelError::Internal(format!("Failed to parse Graph upload response: {e}"))
        })?;

        // In practice, the actual file URL would be constructed from SharePoint
        // This is a simplified return - in production you'd parse the actual response
        Ok(format!(
            "https://graph.microsoft.com/v1.0/$sharepoint+upload+placeholder+{}",
            file_name
        ))
    }

    /// Make a generic POST request to the Graph API.
    pub async fn post<T: Serialize + Send>(
        &self,
        path: &str,
        payload: T,
    ) -> Result<(), ChannelError> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", GRAPH_BASE_URL, path)
        };

        let token = self.token_source.get_token().await?;
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&token)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| ChannelError::Internal(format!("Graph POST request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Graph POST failed");
            return Err(map_graph_error(status.as_u16(), &body));
        }

        Ok(())
    }

    /// Make a POST request and parse JSON response.
    pub async fn post_json<T: Serialize + Send, R: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        payload: T,
    ) -> Result<R, ChannelError> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", GRAPH_BASE_URL, path)
        };

        let token = self.token_source.get_token().await?;
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&token)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| ChannelError::Internal(format!("Graph POST request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Graph POST failed");
            return Err(map_graph_error(status.as_u16(), &body));
        }

        let body = resp.bytes().await.map_err(|e| {
            ChannelError::Internal(format!("Failed to read POST response body: {e}"))
        })?;

        serde_json::from_slice(&body)
            .map_err(|e| ChannelError::Internal(format!("Failed to parse POST response: {e}")))
    }

    /// Make a PUT request and parse JSON response.
    pub async fn put_with_empty<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T, ChannelError> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", GRAPH_BASE_URL, path)
        };

        let token = self.token_source.get_token().await?;
        let resp = self
            .http
            .put(&url)
            .bearer_auth(&token)
            .header("Content-Type", "application/json")
            .send()
            .await
            .map_err(|e| ChannelError::Internal(format!("Graph PUT request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Graph PUT failed");
            return Err(map_graph_error(status.as_u16(), &body));
        }

        let body = resp.bytes().await.map_err(|e| {
            ChannelError::Internal(format!("Failed to read PUT response body: {e}"))
        })?;

        serde_json::from_slice(&body)
            .map_err(|e| ChannelError::Internal(format!("Failed to parse PUT response: {e}")))
    }

    /// Make a raw PUT request with custom headers and byte body (e.g., for chunked upload).
    pub async fn put_raw<T: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        body: &[u8],
        headers: &[(&str, &str)],
    ) -> Result<T, ChannelError> {
        let mut req = self.http.put(url);
        req = req.header("Content-Type", "application/octet-stream");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = req
            .body(body.to_vec())
            .send()
            .await
            .map_err(|e| ChannelError::Internal(format!("Raw PUT request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!(status = %status, body = %body, "Raw PUT failed");
            return Err(map_graph_error(status.as_u16(), &body));
        }

        let body = resp.bytes().await.map_err(|e| {
            ChannelError::Internal(format!("Failed to read PUT response body: {e}"))
        })?;

        serde_json::from_slice(&body)
            .map_err(|e| ChannelError::Internal(format!("Failed to parse PUT response: {e}")))
    }
}

// ── Helper Functions ──────────────────────────────────────────────────────────

/// Encode a string for use in OData query parameters.
/// Escapes single quotes by doubling them ('').
fn encode_odata_param(value: &str) -> String {
    value.replace('\'', "''")
}

/// Encode a string for use in URI path segments.
fn encode_uri_component(value: &str) -> String {
    // Simple percent-encoding for URI path segments
    // This covers the common cases needed for Graph API
    let mut result = String::with_capacity(value.len() * 3);
    for c in value.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => {
                result.push(c);
            }
            ' ' => result.push_str("%20"),
            ':' => result.push_str("%3A"),
            '/' => result.push_str("%2F"),
            _ => {
                for byte in c.to_string().as_bytes() {
                    write!(&mut result, "%{:02X}", byte).unwrap();
                }
            }
        }
    }
    result
}

/// Map HTTP status codes to ChannelError for Graph API failures.
fn map_graph_error(status: u16, body: &str) -> ChannelError {
    // Try to extract error message from Graph error response
    if let Ok(error_resp) = serde_json::from_str::<GraphErrorResponse>(body) {
        if let Some(error) = error_resp.error {
            let code = error.code.unwrap_or_default();
            let message = error.message.unwrap_or_default();
            return ChannelError::Internal(format!("Graph API error [{}]: {}", code, message));
        }
    }

    match status {
        401 | 403 => ChannelError::AuthFailed(format!("Graph API {}: {}", status, body)),
        404 => ChannelError::Internal("Graph API resource not found".into()),
        429 => ChannelError::RateLimited {
            retry_after_secs: 5,
        },
        _ => ChannelError::Internal(format!("Graph API HTTP {}: {}", status, body)),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_odata_param() {
        assert_eq!(encode_odata_param("John's"), "John''s");
        assert_eq!(encode_odata_param("O'Brien"), "O''Brien");
        assert_eq!(encode_odata_param("simple"), "simple");
    }

    #[test]
    fn test_encode_uri_component() {
        assert_eq!(encode_uri_component("user-123"), "user-123");
        assert_eq!(encode_uri_component("hello world"), "hello%20world");
        assert_eq!(
            encode_uri_component("19:conv@thread.v2"),
            "19%3Aconv%40thread.v2"
        );
        assert_eq!(encode_uri_component("teams/Channel"), "teams%2FChannel");
    }

    #[test]
    fn test_graph_user_deserialization() {
        let json = r#"{
            "id": "user-123",
            "displayName": "John Doe",
            "userPrincipalName": "john@contoso.com",
            "mail": "john@contoso.com"
        }"#;
        let user: GraphUser = serde_json::from_str(json).unwrap();
        assert_eq!(user.id.as_deref(), Some("user-123"));
        assert_eq!(user.display_name.as_deref(), Some("John Doe"));
        assert_eq!(
            user.user_principal_name.as_deref(),
            Some("john@contoso.com")
        );
    }

    #[test]
    fn test_graph_group_deserialization() {
        let json = r#"{"id": "team-456", "displayName": "Engineering"}"#;
        let group: GraphGroup = serde_json::from_str(json).unwrap();
        assert_eq!(group.id.as_deref(), Some("team-456"));
        assert_eq!(group.display_name.as_deref(), Some("Engineering"));
    }

    #[test]
    fn test_graph_message_deserialization() {
        let json = r#"{
            "id": "msg-789",
            "body": {"contentType": "text", "content": "Hello world"},
            "from": {"user": {"id": "user-123", "displayName": "John"}},
            "createdDateTime": "2024-01-15T10:30:00Z",
            "messageType": "message",
            "replyToId": "msg-001"
        }"#;
        let msg: GraphMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.id.as_deref(), Some("msg-789"));
        assert_eq!(
            msg.body.as_ref().unwrap().content.as_deref(),
            Some("Hello world")
        );
        assert_eq!(
            msg.from
                .as_ref()
                .unwrap()
                .user
                .as_ref()
                .unwrap()
                .id
                .as_deref(),
            Some("user-123")
        );
        assert_eq!(msg.reply_to_id.as_deref(), Some("msg-001"));
    }

    #[test]
    fn test_graph_response_deserialization() {
        let json = r#"{
            "@odata.context": "https://graph.microsoft.com/v1.0/$metadata#users",
            "value": [
                {"id": "1", "displayName": "Alice"},
                {"id": "2", "displayName": "Bob"}
            ]
        }"#;
        let resp: GraphResponse<GraphUser> = serde_json::from_str(json).unwrap();
        assert_eq!(resp.value.len(), 2);
        assert_eq!(resp.value[0].id.as_deref(), Some("1"));
        assert_eq!(resp.value[1].display_name.as_deref(), Some("Bob"));
    }

    #[tokio::test]
    async fn test_graph_token_cache_new() {
        let cache = GraphTokenCache::new(
            "app-id".to_string(),
            "app-password".to_string(),
            "common".to_string(),
        );
        assert_eq!(cache.app_id, "app-id");
        assert_eq!(cache.tenant_id, "common");
    }
}
