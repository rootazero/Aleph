//! BlueBubbles REST client. All requests carry `?password=` (BlueBubbles has no
//! header auth). Never log the password.

use std::collections::VecDeque;

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum BbError {
    #[error("http error: {0}")]
    Http(String),
    #[error("bad response: {0}")]
    BadResponse(String),
}

/// Detected server capabilities (gate private-api features).
#[derive(Debug, Clone, Copy, Default)]
pub struct ServerCaps {
    pub private_api: bool,
    pub helper_connected: bool,
}

#[derive(Clone)]
pub struct BlueBubblesApi {
    client: reqwest::Client,
    server_url: String,
    password: String,
}

impl BlueBubblesApi {
    #[must_use]
    pub fn new(server_url: String, password: String) -> Self {
        let server_url = server_url.trim_end_matches('/').to_string();
        Self {
            client: reqwest::Client::new(),
            server_url,
            password,
        }
    }

    /// Build a fully-qualified URL with the password query appended.
    #[must_use]
    pub fn api_url(&self, path: &str) -> String {
        let sep = if path.contains('?') { '&' } else { '?' };
        // form_urlencoded encodes spaces as '+'; replace with '%20' to match RFC 3986
        // query encoding. Literal '+' in passwords is encoded as '%2B' by the serializer,
        // so this replacement is safe.
        let encoded: String = url::form_urlencoded::byte_serialize(self.password.as_bytes())
            .collect::<String>()
            .replace('+', "%20");
        format!("{}{}{}password={}", self.server_url, path, sep, encoded)
    }

    pub async fn ping(&self) -> Result<(), BbError> {
        let res = self
            .client
            .get(self.api_url("/api/v1/ping"))
            .send()
            .await
            .map_err(|e| BbError::Http(redact_password(e.to_string())))?;
        res.error_for_status()
            .map_err(|e| BbError::Http(redact_password(e.to_string())))?;
        Ok(())
    }

    /// Probe `/server/info` for private-api + helper availability. Failure
    /// degrades to all-false (rich features simply stay off).
    pub async fn server_caps(&self) -> ServerCaps {
        #[derive(Deserialize)]
        struct Wrap {
            data: Option<Info>,
        }
        #[derive(Deserialize)]
        struct Info {
            private_api: Option<bool>,
            helper_connected: Option<bool>,
        }
        let Ok(res) = self
            .client
            .get(self.api_url("/api/v1/server/info"))
            .send()
            .await
        else {
            return ServerCaps::default();
        };
        match res.json::<Wrap>().await {
            Ok(w) => {
                let info = w.data.unwrap_or(Info {
                    private_api: None,
                    helper_connected: None,
                });
                ServerCaps {
                    private_api: info.private_api.unwrap_or(false),
                    helper_connected: info.helper_connected.unwrap_or(false),
                }
            }
            Err(_) => ServerCaps::default(),
        }
    }
}

/// Tiny LRU for chat-GUID lookups (bounded; BlueBubbles chat lists are large).
pub struct LruGuidCache {
    order: VecDeque<String>,
    map: std::collections::HashMap<String, String>,
    cap: usize,
}

impl LruGuidCache {
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            order: VecDeque::new(),
            map: std::collections::HashMap::new(),
            cap,
        }
    }

    pub fn get(&mut self, k: &str) -> Option<String> {
        let v = self.map.get(k).cloned();
        if v.is_some() {
            self.order.retain(|x| x != k);
            self.order.push_back(k.to_string());
        }
        v
    }

    pub fn put(&mut self, k: &str, v: &str) {
        if !self.map.contains_key(k) {
            while self.order.len() >= self.cap {
                if let Some(old) = self.order.pop_front() {
                    self.map.remove(&old);
                }
            }
        }
        self.order.retain(|x| x != k);
        self.order.push_back(k.to_string());
        self.map.insert(k.to_string(), v.to_string());
    }
}

impl BlueBubblesApi {
    /// Resolve email/phone/identifier to a chat GUID. Raw GUIDs (containing `;`)
    /// pass through. Uses the supplied cache.
    pub async fn resolve_chat_guid(
        &self,
        target: &str,
        cache: &tokio::sync::Mutex<LruGuidCache>,
    ) -> Option<String> {
        let target = target.trim();
        if target.is_empty() {
            return None;
        }
        if target.contains(';') {
            return Some(target.to_string());
        }
        if let Some(g) = cache.lock().await.get(target) {
            return Some(g);
        }

        #[derive(Deserialize)]
        struct Wrap {
            data: Option<Vec<Chat>>,
        }
        #[derive(Deserialize)]
        struct Chat {
            guid: Option<String>,
            #[serde(rename = "chatIdentifier")]
            chat_identifier: Option<String>,
            participants: Option<Vec<Participant>>,
        }
        #[derive(Deserialize)]
        struct Participant {
            address: Option<String>,
        }

        let body = serde_json::json!({ "limit": 100, "offset": 0, "with": ["participants"] });
        let res = self
            .client
            .post(self.api_url("/api/v1/chat/query"))
            .json(&body)
            .send()
            .await
            .ok()?;
        let wrap: Wrap = res.json().await.ok()?;
        for chat in wrap.data.unwrap_or_default() {
            let guid = chat.guid.clone();
            let matches_id = chat.chat_identifier.as_deref() == Some(target);
            let matches_part = chat
                .participants
                .unwrap_or_default()
                .iter()
                .any(|p| p.address.as_deref() == Some(target));
            if matches_id || matches_part {
                if let Some(g) = guid {
                    cache.lock().await.put(target, &g);
                    return Some(g);
                }
            }
        }
        None
    }

    /// Upload a file attachment via multipart. Returns the new message GUID.
    pub async fn send_attachment(
        &self,
        chat_guid: &str,
        path: &std::path::Path,
        is_audio: bool,
    ) -> Result<String, BbError> {
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| BbError::Http(redact_password(e.to_string())))?;
        let name = crate::gateway::interfaces::imessage::bluebubbles::outbound::attachment::attachment_form_name(path);
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(name.clone())
            .mime_str("application/octet-stream")
            .map_err(|e| BbError::Http(redact_password(e.to_string())))?;
        let mut form = reqwest::multipart::Form::new()
            .text("chatGuid", chat_guid.to_string())
            .text("name", name)
            .text("tempGuid", format!("aleph-{}", uuid::Uuid::new_v4()))
            .part("attachment", part);
        if is_audio {
            form = form.text("isAudioMessage", "true");
        }
        let res = self
            .client
            .post(self.api_url("/api/v1/message/attachment"))
            .multipart(form)
            .send()
            .await
            .map_err(|e| BbError::Http(redact_password(e.to_string())))?;
        let res = res
            .error_for_status()
            .map_err(|e| BbError::Http(redact_password(e.to_string())))?;
        let v: serde_json::Value = res
            .json()
            .await
            .map_err(|e| BbError::BadResponse(redact_password(e.to_string())))?;
        Ok(v["data"]["guid"].as_str().unwrap_or("ok").to_string())
    }

    /// POST a single text bubble. Returns the new message GUID.
    pub async fn send_text_chunk(
        &self,
        chat_guid: &str,
        text: &str,
        reply_to: Option<&str>,
        private_api: bool,
    ) -> Result<String, BbError> {
        let mut payload = serde_json::json!({
            "chatGuid": chat_guid,
            "tempGuid": format!("aleph-{}", uuid::Uuid::new_v4()),
            "message": text,
        });
        if let (Some(r), true) = (reply_to, private_api) {
            payload["method"] = serde_json::json!("private-api");
            payload["selectedMessageGuid"] = serde_json::json!(r);
            payload["partIndex"] = serde_json::json!(0);
        }
        let res = self
            .client
            .post(self.api_url("/api/v1/message/text"))
            .json(&payload)
            .send()
            .await
            .map_err(|e| BbError::Http(redact_password(e.to_string())))?;
        let res = res
            .error_for_status()
            .map_err(|e| BbError::Http(redact_password(e.to_string())))?;
        let v: serde_json::Value = res
            .json()
            .await
            .map_err(|e| BbError::BadResponse(redact_password(e.to_string())))?;
        Ok(v["data"]["guid"].as_str().unwrap_or("ok").to_string())
    }
}

impl BlueBubblesApi {
    /// Send a tapback reaction on a specific message (requires private-api; callers gate).
    pub async fn send_reaction(
        &self,
        chat_guid: &str,
        selected_msg_guid: &str,
        reaction: &str,
    ) -> Result<(), BbError> {
        let payload = serde_json::json!({
            "chatGuid": chat_guid,
            "selectedMessageGuid": selected_msg_guid,
            "reaction": reaction,
        });
        let res = self
            .client
            .post(self.api_url("/api/v1/message/react"))
            .json(&payload)
            .send()
            .await
            .map_err(|e| BbError::Http(redact_password(e.to_string())))?;
        res.error_for_status()
            .map_err(|e| BbError::Http(redact_password(e.to_string())))?;
        Ok(())
    }

    /// Send a typing indicator for a chat (requires private-api; callers gate).
    pub async fn send_typing(&self, chat_guid: &str) -> Result<(), BbError> {
        let encoded: String = url::form_urlencoded::byte_serialize(chat_guid.as_bytes())
            .collect::<String>()
            .replace('+', "%20");
        let res = self
            .client
            .post(self.api_url(&format!("/api/v1/chat/{encoded}/typing")))
            .send()
            .await
            .map_err(|e| BbError::Http(redact_password(e.to_string())))?;
        res.error_for_status()
            .map_err(|e| BbError::Http(redact_password(e.to_string())))?;
        Ok(())
    }

    /// Mark a chat as read. Best-effort — failure is logged and ignored by callers.
    pub async fn mark_read(&self, chat_guid: &str) -> Result<(), BbError> {
        let encoded: String = url::form_urlencoded::byte_serialize(chat_guid.as_bytes())
            .collect::<String>()
            .replace('+', "%20");
        let res = self
            .client
            .post(self.api_url(&format!("/api/v1/chat/{encoded}/read")))
            .send()
            .await
            .map_err(|e| BbError::Http(redact_password(e.to_string())))?;
        res.error_for_status()
            .map_err(|e| BbError::Http(redact_password(e.to_string())))?;
        Ok(())
    }
}

/// Build the callback URL BlueBubbles will POST to (password in query —
/// the webhook API cannot send custom headers).
#[must_use]
pub fn webhook_callback_url(host: &str, port: u16, path: &str, password: &str) -> String {
    let host = match host {
        "0.0.0.0" | "127.0.0.1" | "::" | "localhost" => "localhost",
        h => h,
    };
    let enc: String = url::form_urlencoded::byte_serialize(password.as_bytes()).collect();
    format!("http://{host}:{port}{path}?password={enc}")
}

impl BlueBubblesApi {
    pub async fn list_webhooks(&self) -> Vec<(i64, String)> {
        let Ok(res) = self
            .client
            .get(self.api_url("/api/v1/webhook"))
            .send()
            .await
        else {
            return vec![];
        };
        let Ok(v) = res.json::<serde_json::Value>().await else {
            return vec![];
        };
        v["data"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|w| Some((w["id"].as_i64()?, w["url"].as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Register `url` if not already present (idempotent — survives crashes).
    pub async fn register_webhook(&self, url: &str) -> bool {
        if self.list_webhooks().await.iter().any(|(_, u)| u == url) {
            return true;
        }
        // Subscribe to `new-message` only. `updated-message` (edits / delivery
        // status) was previously subscribed but is inert: the inbound handler
        // dedups by message GUID, and an edit carries the same GUID as the
        // original, so it is always dropped as a duplicate. Subscribing to it
        // only added webhook traffic with no effect — Aleph has no edit-apply
        // path. Drop the subscription until edits are handled as a first-class
        // event.
        let payload = serde_json::json!({ "url": url, "events": ["new-message"] });
        self.client
            .post(self.api_url("/api/v1/webhook"))
            .json(&payload)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    pub async fn unregister_matching(&self, url: &str) {
        for (id, u) in self.list_webhooks().await {
            if u == url {
                let _ = self
                    .client
                    .delete(self.api_url(&format!("/api/v1/webhook/{id}")))
                    .send()
                    .await;
            }
        }
    }

    /// Fetch messages created after `after_ms` (epoch milliseconds).
    /// Returns the `data` array; empty on any failure.
    pub async fn query_messages_after(&self, after_ms: i64) -> Vec<serde_json::Value> {
        let body = serde_json::json!({
            "after": after_ms,
            "limit": 100,
            "sort": "ASC",
            "with": ["handle", "attachment", "chats"]
        });
        let Ok(res) = self
            .client
            .post(self.api_url("/api/v1/message/query"))
            .json(&body)
            .send()
            .await
        else {
            return vec![];
        };
        let Ok(v) = res.json::<serde_json::Value>().await else {
            return vec![];
        };
        v["data"].as_array().cloned().unwrap_or_default()
    }

    /// Download an attachment by GUID, write to a temp file, return the path.
    /// `None` on any failure (network, disk, etc.). Logged at debug level.
    pub async fn download_attachment(&self, guid: &str, mime: &str) -> Option<String> {
        let encoded_guid: String = url::form_urlencoded::byte_serialize(guid.as_bytes())
            .collect::<String>()
            .replace('+', "%20");
        let url = self.api_url(&format!("/api/v1/attachment/{encoded_guid}/download"));
        let res = self.client.get(&url).send().await.ok()?;
        let res = res.error_for_status().ok()?;
        let bytes = res.bytes().await.ok()?;
        let ext = mime_to_ext(mime);
        let sanitized: String = guid
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        // Land in the dedicated staging subdir so the periodic sweep can bound
        // disk use; fall back to the bare temp dir if the subdir can't be made.
        let dir = super::staging::ensure_staging_dir().unwrap_or_else(|_| std::env::temp_dir());
        let path = dir.join(format!("aleph-bb-{sanitized}.{ext}"));
        if let Err(e) = tokio::fs::write(&path, &bytes).await {
            tracing::debug!("BlueBubbles attachment write failed for {guid}: {e}");
            return None;
        }
        Some(path.to_string_lossy().into_owned())
    }
}

/// Map a MIME type to a short extension for temp-file naming.
fn mime_to_ext(mime: &str) -> &'static str {
    if mime.contains("jpeg") || mime.contains("jpg") {
        "jpg"
    } else if mime.contains("png") {
        "png"
    } else if mime.contains("heic") {
        "heic"
    } else if mime.contains("gif") {
        "gif"
    } else if mime.contains("mp3") {
        "mp3"
    } else if mime.starts_with("audio/") {
        "m4a"
    } else if mime.contains("quicktime") {
        "mov"
    } else if mime.starts_with("video/") {
        "mp4"
    } else {
        "bin"
    }
}

/// Strip any `password=<value>` query value from an error/URL string so the
/// BlueBubbles server password never reaches logs. The value runs to the next
/// `&`, `)`, whitespace, or end-of-string.
fn redact_password(s: String) -> String {
    let Some(start) = s.find("password=") else {
        return s;
    };
    let val_start = start + "password=".len();
    let end = s[val_start..]
        .find(|c: char| c == '&' || c == ')' || c.is_whitespace())
        .map_or(s.len(), |i| val_start + i);
    let mut out = String::with_capacity(s.len());
    out.push_str(&s[..val_start]);
    out.push_str("***");
    out.push_str(&s[end..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_url_appends_password_query() {
        let api = BlueBubblesApi::new("http://h:1/".into(), "p w".into());
        // trailing slash trimmed; password url-encoded; ? vs & chosen correctly
        assert_eq!(
            api.api_url("/api/v1/ping"),
            "http://h:1/api/v1/ping?password=p%20w"
        );
        assert_eq!(
            api.api_url("/api/v1/chat/x?with=participants"),
            "http://h:1/api/v1/chat/x?with=participants&password=p%20w"
        );
    }

    #[test]
    fn webhook_register_url_embeds_password() {
        let _api = BlueBubblesApi::new("http://h:1".into(), "pw".into());
        let url = super::webhook_callback_url("localhost", 8645, "/bluebubbles-webhook", "pw");
        assert_eq!(url, "http://localhost:8645/bluebubbles-webhook?password=pw");
    }

    #[test]
    fn redacts_password_in_error_url() {
        let s =
            "error sending request for url (http://h:1/api/v1/ping?password=SECRET)".to_string();
        let r = redact_password(s);
        assert!(!r.contains("SECRET"), "password leaked: {r}");
        assert!(r.contains("password=***"));
        // value followed by & is also redacted, and non-password strings pass through
        assert_eq!(
            redact_password("a=1&password=PW&b=2".into()),
            "a=1&password=***&b=2"
        );
        assert_eq!(redact_password("no secret here".into()), "no secret here");
    }
}
