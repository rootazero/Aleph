//! iLink API HTTP Client
//!
//! Functions for making requests to the iLink Bot API.

use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::types::*;

fn random_wechat_uin() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let value = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u32;
    let bytes = value.to_be_bytes();
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for byte in bytes.iter().take(3) {
        result.push(CHARS[(byte >> 2) as usize] as char);
        result.push(CHARS[((byte & 0x03) << 4) as usize] as char);
    }
    result
}

/// Build HTTP headers for iLink API requests.
pub fn build_headers(token: Option<&str>, body: &str) -> HeaderMap {
    use reqwest::header::{HeaderName, HeaderValue, CONTENT_LENGTH, CONTENT_TYPE};

    let mut headers = HeaderMap::new();

    if let Ok(value) = HeaderValue::from_str("application/json") {
        headers.insert(CONTENT_TYPE, value);
    }
    let name = HeaderName::from_static("authorizationtype");
    if let Ok(value) = HeaderValue::from_str("ilink_bot_token") {
        headers.insert(name, value);
    }
    if let Ok(value) = HeaderValue::from_str(&body.len().to_string()) {
        headers.insert(CONTENT_LENGTH, value);
    }
    let name = HeaderName::from_static("x-wechat-uin");
    if let Ok(value) = HeaderValue::from_str(&random_wechat_uin()) {
        headers.insert(name, value);
    }
    let name = HeaderName::from_static("ilink-app-id");
    if let Ok(value) = HeaderValue::from_str(ILINK_APP_ID) {
        headers.insert(name, value);
    }
    let name = HeaderName::from_static("ilink-app-clientversion");
    if let Ok(value) = HeaderValue::from_str(&ILINK_APP_CLIENT_VERSION.to_string()) {
        headers.insert(name, value);
    }
    if let Some(t) = token {
        let name = HeaderName::from_static("authorization");
        if let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", t)) {
            headers.insert(name, value);
        }
    }
    headers
}

/// Base info payload for all requests.
pub fn base_info() -> HashMap<String, String> {
    HashMap::from([("channel_version".to_string(), CHANNEL_VERSION.to_string())])
}

/// API client for iLink endpoints.
#[derive(Clone)]
pub struct ILinkApi {
    http: Client,
    base_url: String,
}

impl ILinkApi {
    pub fn new(base_url: String) -> Self {
        Self {
            http: Client::new(),
            base_url,
        }
    }

    pub fn with_timeout(base_url: String, timeout_ms: u64) -> Self {
        Self {
            http: Client::builder()
                .timeout(std::time::Duration::from_millis(timeout_ms))
                .build()
                .unwrap_or_else(|_| Client::new()),
            base_url,
        }
    }

    /// GET updates via long polling.
    pub async fn get_updates(
        &self,
        token: &str,
        sync_buf: &str,
        timeout_ms: u64,
    ) -> Result<GetUpdatesResponse, String> {
        let url = format!("{}/{}", self.base_url.trim_end_matches('/'), EP_GET_UPDATES);
        let payload = serde_json::json!({
            "get_updates_buf": sync_buf,
            "base_info": base_info()
        });
        let body = payload.to_string();
        let headers = build_headers(Some(token), &body);

        let client = Client::builder()
            .timeout(std::time::Duration::from_millis(timeout_ms))
            .build()
            .map_err(|e| format!("Client error: {}", e))?;

        client
            .post(&url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?
            .json::<GetUpdatesResponse>()
            .await
            .map_err(|e| format!("Parse error: {}", e))
    }

    /// Send a message.
    pub async fn send_message(
        &self,
        token: &str,
        payload: SendMessagePayload,
    ) -> Result<(), String> {
        let url = format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            EP_SEND_MESSAGE
        );
        let body =
            serde_json::to_string(&payload).map_err(|e| format!("Serialize error: {}", e))?;
        let headers = build_headers(Some(token), &body);

        self.http
            .post(&url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?
            .error_for_status()
            .map_err(|e| format!("API error: {}", e))?;

        Ok(())
    }

    /// Send typing indicator.
    pub async fn send_typing(&self, token: &str, payload: SendTypingPayload) -> Result<(), String> {
        let url = format!("{}/{}", self.base_url.trim_end_matches('/'), EP_SEND_TYPING);
        let body =
            serde_json::to_string(&payload).map_err(|e| format!("Serialize error: {}", e))?;
        let headers = build_headers(Some(token), &body);

        self.http
            .post(&url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?
            .error_for_status()
            .map_err(|e| format!("API error: {}", e))?;

        Ok(())
    }

    /// Get config (typing ticket).
    pub async fn get_config(
        &self,
        token: &str,
        user_id: &str,
        context_token: Option<&str>,
    ) -> Result<ConfigResponse, String> {
        let url = format!("{}/{}", self.base_url.trim_end_matches('/'), EP_GET_CONFIG);
        let mut payload = serde_json::json!({
            "ilink_user_id": user_id,
            "base_info": base_info()
        });
        if let Some(ctx) = context_token {
            payload["context_token"] = serde_json::json!(ctx);
        }
        let body = payload.to_string();
        let headers = build_headers(Some(token), &body);

        self.http
            .post(&url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?
            .json::<ConfigResponse>()
            .await
            .map_err(|e| format!("Parse error: {}", e))
    }

    /// Get QR code for login.
    pub async fn get_bot_qr(&self, bot_type: &str) -> Result<QrCodeResponse, String> {
        use reqwest::header::HeaderName;

        let url = format!(
            "{}/{}?bot_type={}",
            self.base_url.trim_end_matches('/'),
            EP_GET_BOT_QR,
            bot_type
        );
        let mut headers = HeaderMap::new();
        let name = HeaderName::from_static("ilink-app-id");
        if let Ok(value) = HeaderValue::from_str(ILINK_APP_ID) {
            headers.insert(name, value);
        }
        let name = HeaderName::from_static("ilink-app-clientversion");
        if let Ok(value) = HeaderValue::from_str(&ILINK_APP_CLIENT_VERSION.to_string()) {
            headers.insert(name, value);
        }

        self.http
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?
            .json::<QrCodeResponse>()
            .await
            .map_err(|e| format!("Parse error: {}", e))
    }

    /// Get QR code status.
    pub async fn get_qr_status(&self, qrcode: &str) -> Result<QrStatusResponse, String> {
        use reqwest::header::HeaderName;

        let url = format!(
            "{}/{}?qrcode={}",
            self.base_url.trim_end_matches('/'),
            EP_GET_QR_STATUS,
            qrcode
        );
        let mut headers = HeaderMap::new();
        let name = HeaderName::from_static("ilink-app-id");
        if let Ok(value) = HeaderValue::from_str(ILINK_APP_ID) {
            headers.insert(name, value);
        }
        let name = HeaderName::from_static("ilink-app-clientversion");
        if let Ok(value) = HeaderValue::from_str(&ILINK_APP_CLIENT_VERSION.to_string()) {
            headers.insert(name, value);
        }

        self.http
            .get(&url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?
            .json::<QrStatusResponse>()
            .await
            .map_err(|e| format!("Parse error: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_headers_with_token() {
        let headers = build_headers(Some("test_token"), "body");
        assert_eq!(headers.get("Content-Type").unwrap(), "application/json");
        assert_eq!(headers.get("AuthorizationType").unwrap(), "ilink_bot_token");
        assert_eq!(headers.get("Authorization").unwrap(), "Bearer test_token");
    }

    #[test]
    fn test_build_headers_without_token() {
        let headers = build_headers(None, "body");
        assert!(headers.get("Authorization").is_none());
    }

    #[test]
    fn test_base_info() {
        let info = base_info();
        assert_eq!(info.get("channel_version").unwrap(), "2.2.0");
    }
}
