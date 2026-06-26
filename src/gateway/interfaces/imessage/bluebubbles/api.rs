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
        Self { client: reqwest::Client::new(), server_url, password }
    }

    /// Build a fully-qualified URL with the password query appended.
    #[must_use]
    pub fn api_url(&self, path: &str) -> String {
        let sep = if path.contains('?') { '&' } else { '?' };
        // form_urlencoded encodes spaces as '+'; replace with '%20' to match RFC 3986
        // query encoding. Literal '+' in passwords is encoded as '%2B' by the serializer,
        // so this replacement is safe.
        let encoded: String =
            url::form_urlencoded::byte_serialize(self.password.as_bytes())
                .collect::<String>()
                .replace('+', "%20");
        format!("{}{}{}password={}", self.server_url, path, sep, encoded)
    }

    // Called in Task 7 (connection probe on channel start).
    pub async fn ping(&self) -> Result<(), BbError> {
        let res = self
            .client
            .get(self.api_url("/api/v1/ping"))
            .send()
            .await
            .map_err(|e| BbError::Http(e.to_string()))?;
        res.error_for_status().map_err(|e| BbError::Http(e.to_string()))?;
        Ok(())
    }

    /// Probe `/server/info` for private-api + helper availability. Failure
    /// degrades to all-false (rich features simply stay off).
    // Called in Task 7 (capabilities negotiation on channel start).
    pub async fn server_caps(&self) -> ServerCaps {
        #[derive(Deserialize)]
        struct Wrap { data: Option<Info> }
        #[derive(Deserialize)]
        struct Info { private_api: Option<bool>, helper_connected: Option<bool> }
        let Ok(res) = self.client.get(self.api_url("/api/v1/server/info")).send().await else {
            return ServerCaps::default();
        };
        match res.json::<Wrap>().await {
            Ok(w) => {
                let info = w.data.unwrap_or(Info { private_api: None, helper_connected: None });
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
        Self { order: VecDeque::new(), map: std::collections::HashMap::new(), cap }
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

        let body =
            serde_json::json!({ "limit": 100, "offset": 0, "with": ["participants"] });
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
            if (matches_id || matches_part) && guid.is_some() {
                let g = guid.unwrap();
                cache.lock().await.put(target, &g);
                return Some(g);
            }
        }
        None
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
            .map_err(|e| BbError::Http(e.to_string()))?;
        let res =
            res.error_for_status().map_err(|e| BbError::Http(e.to_string()))?;
        let v: serde_json::Value =
            res.json().await.map_err(|e| BbError::BadResponse(e.to_string()))?;
        Ok(v["data"]["guid"].as_str().unwrap_or("ok").to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_url_appends_password_query() {
        let api = BlueBubblesApi::new("http://h:1/".into(), "p w".into());
        // trailing slash trimmed; password url-encoded; ? vs & chosen correctly
        assert_eq!(api.api_url("/api/v1/ping"), "http://h:1/api/v1/ping?password=p%20w");
        assert_eq!(
            api.api_url("/api/v1/chat/x?with=participants"),
            "http://h:1/api/v1/chat/x?with=participants&password=p%20w"
        );
    }
}
