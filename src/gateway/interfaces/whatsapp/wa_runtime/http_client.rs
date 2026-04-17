//! Reqwest-based HTTP client for whatsapp-rust

use async_trait::async_trait;


/// HTTP client implementation using reqwest for whatsapp-rust Bot
#[derive(Debug, Clone)]
pub struct ReqwestHttpClient {
    client: reqwest::Client,
}

impl ReqwestHttpClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }
}

impl Default for ReqwestHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl whatsapp_rust::http::HttpClient for ReqwestHttpClient {
    async fn execute(
        &self,
        request: whatsapp_rust::http::HttpRequest,
    ) -> anyhow::Result<whatsapp_rust::http::HttpResponse> {
        let method = match request.method.as_str() {
            "GET" => reqwest::Method::GET,
            "POST" => reqwest::Method::POST,
            m => reqwest::Method::from_bytes(m.as_bytes())
                .map_err(|e| anyhow::anyhow!("Invalid HTTP method: {}", e))?,
        };

        let mut builder = self.client.request(method, &request.url);

        for (key, value) in &request.headers {
            builder = builder.header(key, value);
        }

        if let Some(body) = request.body {
            builder = builder.body(body);
        }

        let response = builder
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("HTTP request failed: {}", e))?;

        let status_code = response.status().as_u16();
        let body = response
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read response body: {}", e))?
            .to_vec();

        Ok(whatsapp_rust::http::HttpResponse { status_code, body })
    }
}
