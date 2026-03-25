//! Credential types for auth profiles.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// API key credential (static key)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApiKeyCredential {
    pub provider: String,
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// Token credential (bearer-style, optionally expiring)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TokenCredential {
    pub provider: String,
    pub token: String,
    /// Optional expiry timestamp (ms since epoch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// OAuth credential (refreshable)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OAuthCredential {
    pub provider: String,
    pub access: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh: Option<String>,
    /// Expiry timestamp (ms since epoch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// OAuth client secret (stored in vault, not logged)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// Token endpoint URL for refresh (e.g., https://oauth2.googleapis.com/token)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// Auth profile credential (discriminated union)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthProfileCredential {
    ApiKey(ApiKeyCredential),
    Token(TokenCredential),
    OAuth(OAuthCredential),
}

impl AuthProfileCredential {
    /// Get the provider ID for this credential
    pub fn provider(&self) -> &str {
        match self {
            Self::ApiKey(c) => &c.provider,
            Self::Token(c) => &c.provider,
            Self::OAuth(c) => &c.provider,
        }
    }

    /// Get the credential type name
    pub fn credential_type(&self) -> &'static str {
        match self {
            Self::ApiKey(_) => "api_key",
            Self::Token(_) => "token",
            Self::OAuth(_) => "oauth",
        }
    }

    /// Check if the credential has valid/non-empty authentication data
    pub fn is_valid(&self) -> bool {
        match self {
            Self::ApiKey(c) => !c.key.trim().is_empty(),
            Self::Token(c) => !c.token.trim().is_empty(),
            Self::OAuth(c) => {
                !c.access.trim().is_empty()
                    || c.refresh.as_ref().is_some_and(|r| !r.trim().is_empty())
            }
        }
    }

    /// Check if the credential is expired (for token/oauth types)
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        match self {
            Self::ApiKey(_) => false, // API keys don't expire
            Self::Token(c) => c.expires.is_some_and(|exp| exp > 0 && now >= exp),
            Self::OAuth(c) => {
                // OAuth is expired only if no refresh token and access is expired
                c.refresh.is_none() && c.expires.is_some_and(|exp| exp > 0 && now >= exp)
            }
        }
    }

    /// Get the API key or token for use in requests
    pub fn resolve_key(&self) -> Option<&str> {
        match self {
            Self::ApiKey(c) => Some(&c.key),
            Self::Token(c) => Some(&c.token),
            Self::OAuth(c) => Some(&c.access),
        }
    }

    /// Type score for ordering (lower = higher priority)
    /// OAuth > Token > API Key
    pub fn type_score(&self) -> u8 {
        match self {
            Self::OAuth(_) => 0,
            Self::Token(_) => 1,
            Self::ApiKey(_) => 2,
        }
    }
}
