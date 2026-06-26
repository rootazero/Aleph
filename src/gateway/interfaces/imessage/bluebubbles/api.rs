//! BlueBubbles REST client. All requests carry `?password=` (BlueBubbles has no
//! header auth). Never log the password.

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
