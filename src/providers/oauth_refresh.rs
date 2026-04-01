//! OAuth token auto-refresh.
//!
//! Checks if an OAuth credential is near expiry and refreshes it
//! using the refresh_token grant.

use anyhow::Result;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info};

use super::OAuthCredential;

/// Shared HTTP client for OAuth token refresh (reuses connection pool).
fn oauth_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default()
    })
}

/// Default refresh margin: refresh 5 minutes before expiry.
const REFRESH_MARGIN_SECS: u64 = 300;

/// Google OAuth2 token endpoint.
const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// Check if an OAuth credential needs refresh.
pub fn needs_refresh(cred: &OAuthCredential) -> bool {
    let Some(expires_ms) = cred.expires else {
        return false; // No expiry set — don't refresh
    };
    // No refresh token — can't refresh
    if cred.refresh.is_none() {
        return false;
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let margin_ms = REFRESH_MARGIN_SECS * 1000;
    now_ms + margin_ms >= expires_ms
}

/// Refresh an OAuth credential. Returns updated credential on success.
pub async fn refresh_token(cred: &OAuthCredential) -> Result<OAuthCredential> {
    let refresh_token = cred
        .refresh
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("No refresh token available"))?;

    let endpoint = cred
        .token_endpoint
        .as_deref()
        .or_else(|| {
            // Auto-detect Google
            if cred.provider.contains("google") || cred.provider.contains("vertex") {
                Some(GOOGLE_TOKEN_ENDPOINT)
            } else {
                None
            }
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No token_endpoint configured for provider '{}'",
                cred.provider
            )
        })?;

    let client = oauth_client();
    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];
    if let Some(ref client_id) = cred.client_id {
        form.push(("client_id", client_id.as_str()));
    }
    if let Some(ref client_secret) = cred.client_secret {
        form.push(("client_secret", client_secret.as_str()));
    }

    debug!(
        "Refreshing OAuth token for provider '{}' at {}",
        cred.provider, endpoint
    );

    let resp = client.post(endpoint).form(&form).send().await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        debug!("Token refresh error body: {}", body);
        // Truncate body to avoid leaking credentials (some providers echo back client_secret)
        let body_preview: String = body.chars().take(200).collect();
        return Err(anyhow::anyhow!(
            "Token refresh failed: {} — {}",
            status,
            body_preview
        ));
    }

    let body: serde_json::Value = resp.json().await?;
    let new_access = body["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No access_token in refresh response"))?;
    let expires_in = body["expires_in"].as_u64().unwrap_or(3600);
    let new_expires_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
        + expires_in * 1000;

    // Use new refresh token if provided, otherwise keep existing
    let new_refresh = body["refresh_token"]
        .as_str()
        .map(String::from)
        .or_else(|| cred.refresh.clone());

    info!(
        "OAuth token refreshed for provider '{}', expires in {}s",
        cred.provider, expires_in
    );

    Ok(OAuthCredential {
        provider: cred.provider.clone(),
        access: new_access.to_string(),
        refresh: new_refresh,
        expires: Some(new_expires_ms),
        client_id: cred.client_id.clone(),
        client_secret: cred.client_secret.clone(),
        token_endpoint: cred.token_endpoint.clone(),
        email: cred.email.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_oauth(expires_ms: Option<u64>, has_refresh: bool) -> OAuthCredential {
        OAuthCredential {
            provider: "test".to_string(),
            access: "access-token".to_string(),
            refresh: if has_refresh {
                Some("refresh-token".to_string())
            } else {
                None
            },
            expires: expires_ms,
            client_id: None,
            client_secret: None,
            token_endpoint: None,
            email: None,
        }
    }

    #[test]
    fn test_no_expiry_no_refresh() {
        let cred = make_oauth(None, true);
        assert!(!needs_refresh(&cred));
    }

    #[test]
    fn test_no_refresh_token() {
        let cred = make_oauth(Some(1000), false);
        assert!(!needs_refresh(&cred));
    }

    #[test]
    fn test_expired_needs_refresh() {
        // Expired 1 hour ago
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let cred = make_oauth(Some(now_ms - 3_600_000), true);
        assert!(needs_refresh(&cred));
    }

    #[test]
    fn test_near_expiry_needs_refresh() {
        // Expires in 2 minutes (within 5-minute margin)
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let cred = make_oauth(Some(now_ms + 120_000), true);
        assert!(needs_refresh(&cred));
    }

    #[test]
    fn test_far_future_no_refresh() {
        // Expires in 1 hour (well outside 5-minute margin)
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let cred = make_oauth(Some(now_ms + 3_600_000), true);
        assert!(!needs_refresh(&cred));
    }
}
