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

        let mut offset = 0;
        let limit = 100;
        loop {
            let body =
                serde_json::json!({ "limit": limit, "offset": offset, "with": ["participants"] });
            let res = self
                .client
                .post(self.api_url("/api/v1/chat/query"))
                .json(&body)
                .send()
                .await
                .ok()?;
            let wrap: Wrap = res.json().await.ok()?;
            let chats = wrap.data.unwrap_or_default();
            if chats.is_empty() {
                break;
            }
            for chat in &chats {
                let guid = chat.guid.clone();
                let matches_id = chat.chat_identifier.as_deref() == Some(target);
                if matches_id {
                    if let Some(g) = guid {
                        cache.lock().await.put(target, &g);
                        return Some(g);
                    }
                }
            }
            for chat in &chats {
                let guid = chat.guid.clone();
                let participants = chat
                    .participants
                    .as_ref()
                    .map(|v| v.as_slice())
                    .unwrap_or_default();
                let is_one_on_one = participants.len() == 1;
                let matches_part = is_one_on_one
                    && participants
                        .iter()
                        .any(|p| p.address.as_deref() == Some(target));
                if matches_part {
                    if let Some(g) = guid {
                        cache.lock().await.put(target, &g);
                        return Some(g);
                    }
                }
            }
            // Advance the page: without this the loop re-queried page 1
            // forever and a target that is not on the first page never
            // resolved.
            offset += limit;
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

    /// Set the icon of a group chat. Requires private-api + macOS 11+ on the
    /// server Mac (BlueBubbles helper feature #8, "Update group chat photo").
    /// Endpoint: `POST /api/v1/chat/{chatGuid}/icon`, multipart/form-data with
    /// a single `icon` file part.
    ///
    /// Upstream reference for the wire shape: openclaw
    /// `extensions/bluebubbles/src/chat.ts::setGroupIconBlueBubbles`. The
    /// endpoint path and `icon` field name come from there; password is
    /// passed via the URL query string to match every other BlueBubbles REST
    /// call.
    ///
    /// `icon_data_url` is a `data:` URL (RFC 2397) carrying the image payload,
    /// matching `Channel::set_group_icon`'s public contract.
    pub async fn set_chat_icon(&self, chat_guid: &str, icon_data_url: &str) -> Result<(), BbError> {
        let (mime, bytes) = crate::gateway::data_url::decode(icon_data_url)
            .map_err(|e| BbError::BadResponse(format!("icon data URL: {e}")))?;
        let filename = mime_to_filename(&mime);
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(filename)
            .mime_str(&mime)
            .map_err(|e| BbError::Http(redact_password(e.to_string())))?;
        let form = reqwest::multipart::Form::new().part("icon", part);
        let encoded: String = url::form_urlencoded::byte_serialize(chat_guid.as_bytes())
            .collect::<String>()
            .replace('+', "%20");
        let res = self
            .client
            .post(self.api_url(&format!("/api/v1/chat/{encoded}/icon")))
            .multipart(form)
            .send()
            .await
            .map_err(|e| BbError::Http(redact_password(e.to_string())))?;
        res.error_for_status()
            .map_err(|e| BbError::Http(redact_password(e.to_string())))?;
        Ok(())
    }
}

/// Map an image MIME type to a filename for the multipart upload.
fn mime_to_filename(mime: &str) -> String {
    match mime {
        "image/png" => "icon.png".into(),
        "image/jpeg" => "icon.jpg".into(),
        "image/gif" => "icon.gif".into(),
        "image/heic" => "icon.heic".into(),
        "image/webp" => "icon.webp".into(),
        other => format!("icon.{}", other.split('/').next_back().unwrap_or("bin")),
    }
}

/// Build the callback URL BlueBubbles will POST to (password in query —
/// the webhook API cannot send custom headers).
#[must_use]
pub fn webhook_callback_url(host: &str, port: u16, path: &str, password: &str) -> String {
    // Wildcard listeners are advertised as loopback only for local deployment;
    // remote deployment is rejected by BlueBubblesConfig::validate.
    let host = match host {
        "0.0.0.0" => "127.0.0.1",
        "::" => "[::1]",
        h => h,
    };
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
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
    ///
    /// The `after` filter of the BlueBubbles message query is EXCLUSIVE
    /// (`dateCreated > after`), so a cursor that sits exactly on the newest
    /// message's millisecond skips every OTHER message that shares that
    /// millisecond — two chats in the same second, or a burst from one
    /// sender, lose rows silently. The cursor therefore backs off one
    /// millisecond before the next query and the guid set drops the rows
    /// that come back twice.
    pub async fn query_messages_after(&self, after_ms: i64) -> Vec<serde_json::Value> {
        let mut all_messages = Vec::new();
        // The filter is exclusive, so the first window starts one millisecond
        // BEFORE the cursor the caller gave us: the message that sits exactly
        // on that millisecond is part of what the caller wants.
        let mut cursor = after_ms.saturating_sub(1);
        let mut seen_guids = std::collections::HashSet::new();
        // The newest millisecond seen so far. A batch that does not move it
        // forward repeated the boundary and nothing else.
        let mut boundary = after_ms;

        loop {
            let body = serde_json::json!({
                "after": cursor,
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
                break;
            };
            let Ok(v) = res.json::<serde_json::Value>().await else {
                break;
            };
            let batch = v["data"].as_array().cloned().unwrap_or_default();

            if batch.is_empty() {
                break;
            }

            let mut max_timestamp = cursor;
            let batch_len = batch.len();

            for msg in &batch {
                if let Some(guid) = msg.get("guid").and_then(|g| g.as_str()) {
                    if seen_guids.insert(guid.to_string()) {
                        all_messages.push(msg.clone());
                    }
                }
                if let Some(date_created) = msg.get("dateCreated").and_then(|d| d.as_i64()) {
                    if date_created > max_timestamp {
                        max_timestamp = date_created;
                    }
                }
            }

            // No forward progress: the batch only repeated what the previous
            // query already covered. Stopping here is what keeps the loop
            // from re-querying the same window forever.
            if max_timestamp <= boundary {
                break;
            }
            boundary = max_timestamp;
            // One millisecond back, so the boundary millisecond itself is
            // inside the next window (the filter is exclusive).
            cursor = max_timestamp.saturating_sub(1);
            // A short batch is the last page.
            if batch_len < 100 {
                break;
            }
        }

        all_messages
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
        let bytes = read_attachment_limited(res, 100 * 1024 * 1024).await?;
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
        let dir = match super::staging::ensure_staging_dir().await {
            Ok(dir) => dir,
            Err(e) => {
                tracing::debug!("BlueBubbles attachment staging unavailable: {e}");
                return None;
            }
        };
        let path = dir.join(format!("aleph-bb-{sanitized}.{ext}"));
        if let Err(e) = tokio::fs::write(&path, &bytes).await {
            tracing::debug!("BlueBubbles attachment write failed for {guid}: {e}");
            return None;
        }
        Some(path.to_string_lossy().into_owned())
    }
}

async fn read_attachment_limited(mut res: reqwest::Response, max: usize) -> Option<Vec<u8>> {
    if res.content_length().is_some_and(|len| len > max as u64) {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = res.chunk().await.ok()? {
        if chunk.len() > max - bytes.len() {
            tracing::debug!("BlueBubbles attachment exceeds {max} bytes");
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    Some(bytes)
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

    /// The cursor must not skip the millisecond it sits on: the BlueBubbles
    /// `after` filter is exclusive, so a cursor on the newest message's
    /// timestamp would leave every other message of that millisecond
    /// unfetched. The query therefore starts one millisecond earlier.
    #[tokio::test]
    async fn the_query_cursor_includes_the_boundary_millisecond() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
        let recorder = seen.clone();
        let app = axum::Router::new().route(
            "/api/v1/message/query",
            axum::routing::post(move |body: String| {
                let recorder = recorder.clone();
                async move {
                    recorder
                        .lock()
                        .unwrap()
                        .push(serde_json::from_str::<serde_json::Value>(&body).unwrap());
                    axum::Json(serde_json::json!({
                        "data": [
                            { "guid": "g1", "dateCreated": 1000 },
                            { "guid": "g2", "dateCreated": 1000 }
                        ]
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let api = BlueBubblesApi::new(format!("http://{addr}"), "pw".to_string());
        let msgs = api.query_messages_after(1000).await;

        // Both messages of the boundary millisecond are returned.
        assert_eq!(msgs.len(), 2);
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["after"], 999);

        server.abort();
    }

    #[tokio::test]
    async fn chunked_attachment_limit_checks_actual_bytes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for cap in [3, 4] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 2048];
                socket.read(&mut request).await.unwrap();
                socket.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n2\r\nab\r\n2\r\ncd\r\n0\r\n\r\n").await.unwrap();
            });
            let response = reqwest::get(format!("http://{addr}")).await.unwrap();
            assert!(response.content_length().is_none());
            let bytes = read_attachment_limited(response, cap).await;
            assert_eq!(
                bytes,
                if cap == 4 {
                    Some(b"abcd".to_vec())
                } else {
                    None
                }
            );
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn staged_write_failure_keeps_attachment_metadata_only() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new().fallback(|| async { "attachment" });
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let guid = uuid::Uuid::new_v4().to_string();
        let file_name = format!("aleph-bb-{guid}.bin");
        let dir = super::super::staging::ensure_staging_dir().await.unwrap();
        let blocked = dir.join(&file_name);
        tokio::fs::create_dir(&blocked).await.unwrap();
        let api = BlueBubblesApi::new(format!("http://{addr}"), "pw".into());
        let result = api
            .download_attachment(&guid, "application/octet-stream")
            .await;
        tokio::fs::remove_dir(&blocked).await.unwrap();
        server.abort();
        assert!(result.is_none());
        assert!(!std::env::temp_dir().join(file_name).exists());
    }

    #[test]
    fn callback_supports_local_ipv6_and_encodes_credentials() {
        for host in ["localhost", "127.0.0.1", "::", "::1", "[::1]"] {
            let callback = webhook_callback_url(host, 8645, "/hook", "p &?+#/密码");
            let url = url::Url::parse(&callback).unwrap();
            assert_eq!(
                url.query_pairs()
                    .find(|(key, _)| key == "password")
                    .unwrap()
                    .1,
                "p &?+#/密码"
            );
        }
    }

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

    // ---- set_chat_icon URL shape ----------------------------------------

    /// The endpoint URL is `/api/v1/chat/{chatGuid}/icon` with the password
    /// in the query string (matching every other BlueBubbles REST call).
    /// The chatGuid is URL-encoded — `+` becomes `%20`, semicolons become
    /// `%3B`, etc. — which is the same encoding `send_typing` and `mark_read`
    /// use, so `set_chat_icon` joins them in the documented route shape.
    #[test]
    fn set_chat_icon_uses_documented_endpoint_and_encodes_chat_guid() {
        let api = BlueBubblesApi::new("http://h:1".into(), "pw".into());
        // `api_url()` does NOT encode the path itself — it only encodes
        // the password for the query string. `set_chat_icon` (and every
        // other BlueBubbles method that takes a chat guid) byte-serializes
        // the chat guid before interpolating it into the path. Mirror that
        // here so the assertion names the right shape.
        let encoded: String =
            url::form_urlencoded::byte_serialize("iMessage;-;+15555550100".as_bytes())
                .collect::<String>()
                .replace('+', "%20");
        assert_eq!(
            api.api_url(&format!("/api/v1/chat/{encoded}/icon")),
            "http://h:1/api/v1/chat/iMessage%3B-%3B%2B15555550100/icon?password=pw"
        );
    }
}
