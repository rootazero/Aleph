//! Embedding provider abstraction
//!
//! All embeddings go through remote OpenAI-compatible APIs.

use crate::config::types::memory::{EmbeddingPreset, EmbeddingProviderConfig};
use crate::error::AlephError;
use crate::sync_primitives::Arc;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

/// Abstract embedding provider
#[async_trait::async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Generate embedding for a single text
    async fn embed(&self, text: &str) -> Result<Vec<f32>, AlephError>;

    /// Generate embeddings for multiple texts (batch)
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError>;

    /// Get the output dimension of this provider
    fn dimensions(&self) -> usize;

    /// Get the model name (e.g., "BAAI/bge-m3")
    fn model_name(&self) -> &str;

    /// Get the provider id (e.g., "siliconflow")
    fn provider_id(&self) -> &str;
}

/// Validate `api_base` against SSRF and cleartext-HTTPS policy before any
/// outbound traffic is constructed.
///
/// Rules:
/// - `EmbeddingPreset::Ollama` is the only preset that allows `http://` and
///   loopback addresses. The model runs on the operator's machine.
/// - Every other preset (OpenAI / SiliconFlow / Jina / Mistral / Custom) must
///   use `https://`. Bearer credentials and embedding bodies transit in
///   cleartext otherwise.
/// - IP literals, RFC1918 ranges, link-local (169.254/16, fe80::/10), and
///   loopback non-`localhost` names are rejected for every preset. Without
///   this guard, a corrupted config row can repoint the client at e.g.
///   `http://169.254.169.254/` and exfiltrate the `Authorization` header.
fn validate_api_base(api_base: &str, preset: &EmbeddingPreset) -> Result<(), AlephError> {
    let parsed = url::Url::parse(api_base).map_err(|e| {
        AlephError::config(format!(
            "Embedding api_base {api_base:?} is not a valid URL: {e}"
        ))
    })?;

    let scheme = parsed.scheme();
    let host = parsed
        .host_str()
        .ok_or_else(|| AlephError::config(format!("Embedding api_base {api_base:?} has no host")))?;

    let is_loopback_name = host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case("ip6-localhost")
        || host.eq_ignore_ascii_case("ip6-loopback");

    let is_private_ip = match parsed.host() {
        Some(url::Host::Ipv4(addr)) => is_private_v4(addr),
        Some(url::Host::Ipv6(addr)) => is_private_v6(&addr),
        Some(url::Host::Domain(_)) => false,
        None => true,
    };

    match preset {
        EmbeddingPreset::Ollama => {
            // Ollama: allow http only against loopback; require https otherwise.
            if scheme == "http" && !(is_loopback_name || matches!(parsed.host(), Some(url::Host::Ipv4(a)) if a.is_loopback())) {
                return Err(AlephError::config(format!(
                    "Embedding Ollama preset requires api_base to be https:// or http://localhost; got {api_base:?}"
                )));
            }
            if scheme != "http" && scheme != "https" {
                return Err(AlephError::config(format!(
                    "Embedding api_base scheme must be http or https; got {scheme:?}"
                )));
            }
        }
        _ => {
            if scheme != "https" {
                return Err(AlephError::config(format!(
                    "Embedding preset {preset} requires https:// api_base; got {scheme:?}"
                )));
            }
            if is_loopback_name || is_private_ip {
                return Err(AlephError::config(format!(
                    "Embedding preset {preset} api_base {api_base:?} targets a loopback or private address"
                )));
            }
        }
    }

    Ok(())
}

fn is_private_v4(addr: Ipv4Addr) -> bool {
    addr.is_loopback()
        || addr.is_link_local()
        || addr.is_private()
        || addr.is_unspecified()
        || addr.is_multicast()
        // 169.254.0.0/16 (link-local) is covered by `is_link_local()`; cloud
        // metadata 169.254.169.254 is therefore already rejected.
        // Carrier-grade NAT 100.64.0.0/10
        || matches!(addr.octets(), [100, b, _, _] if (64..=127).contains(&b))
}

fn is_private_v6(addr: &Ipv6Addr) -> bool {
    addr.is_loopback()
        || addr.is_unspecified()
        || addr.is_multicast()
        // Unique-local fc00::/7
        || (addr.segments()[0] & 0xfe00) == 0xfc00
        // fe80::/10 link-local
        || (addr.segments()[0] & 0xffc0) == 0xfe80
}
///
/// Used when a remote model returns vectors larger than the configured
/// storage dimension. Borrowed from `OpenViking`'s design.
///
/// If `embedding.len() <= target_dim`, returns the embedding unchanged.
#[must_use]
pub fn truncate_and_normalize(embedding: Vec<f32>, target_dim: usize) -> Vec<f32> {
    if embedding.len() <= target_dim {
        return embedding;
    }
    let truncated = &embedding[..target_dim];
    let norm: f32 = truncated.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 && norm.is_finite() {
        truncated.iter().map(|x| x / norm).collect()
    } else {
        truncated.to_vec()
    }
}

/// Remote embedding provider using OpenAI-compatible API
///
/// Works with `SiliconFlow`, `OpenAI`, Ollama, and any service
/// that implements the `/v1/embeddings` endpoint.
pub struct RemoteEmbeddingProvider {
    client: reqwest::Client,
    api_base: String,
    api_key: String,
    model: String,
    dimension: usize,
    batch_size: usize,
    /// UTF-8-safe per-input character ceiling. Texts exceeding this are
    /// truncated at a char boundary before the API call to prevent
    /// 8192-token-limit failures on long compound-ingest inputs.
    max_input_chars: usize,
    /// Whether to emit the OpenAI-style `dimensions` request field. Gated per
    /// preset (`EmbeddingPreset::sends_dimensions_param`) so providers that
    /// reject unknown fields (Mistral) aren't sent it.
    send_dimensions: bool,
    provider_id: String,
}

impl RemoteEmbeddingProvider {
    /// Create from `EmbeddingProviderConfig`
    pub fn from_config(config: &EmbeddingProviderConfig) -> Result<Self, AlephError> {
        // API key is populated from vault at runtime
        // rust-doctor-disable-next-line excessive-clone
        let api_key = config.api_key.clone().unwrap_or_default();

        validate_api_base(&config.api_base, &config.preset)?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .map_err(|e| AlephError::config(format!("Failed to create HTTP client: {e}")))?;

        Ok(Self {
            client,
            // rust-doctor-disable-next-line excessive-clone
            api_base: config.api_base.clone(),
            api_key,
            model: config.default_model().to_string(),
            dimension: config.dimensions as usize,
            batch_size: config.batch_size as usize,
            max_input_chars: config.max_input_chars,
            send_dimensions: config.preset.sends_dimensions_param(),
            // rust-doctor-disable-next-line excessive-clone
            provider_id: config.id.clone(),
        })
    }

    /// UTF-8-safe truncation to at most `max_input_chars` chars.
    /// Returns a `Cow::Borrowed` for in-bounds inputs (zero-alloc fast path).
    fn truncate_input<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
        if text.chars().count() <= self.max_input_chars {
            return std::borrow::Cow::Borrowed(text);
        }
        let truncated: String = text.chars().take(self.max_input_chars).collect();
        tracing::debug!(
            target: "aleph::embedding",
            provider = %self.provider_id,
            original_chars = text.chars().count(),
            truncated_chars = truncated.chars().count(),
            "embedding input truncated to fit provider input limit"
        );
        std::borrow::Cow::Owned(truncated)
    }

    /// Test connectivity by embedding a short text
    pub async fn test_connection(&self) -> Result<(), AlephError> {
        let result = self.embed("test").await?;
        if result.len() != self.dimension {
            return Err(AlephError::config(format!(
                "Dimension mismatch: expected {}, got {}",
                self.dimension,
                result.len()
            )));
        }
        Ok(())
    }

    /// Call the embeddings API for a batch of texts
    async fn call_api(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        // Normalize: ensure /v1 is present before appending /embeddings
        let base = self.api_base.trim_end_matches('/');
        let normalized = if base.ends_with("/v1") {
            base.to_string()
        } else {
            format!("{base}/v1")
        };
        let url = format!("{normalized}/embeddings");

        // Truncate each input UTF-8-safely to stay under the provider's
        // token cap (8192 for T8Star/OpenAI text-embedding-3-*). Without
        // this, long compound-ingest concatenations fail the whole batch
        // and surface as `compound ingest failed` WARNs.
        let truncated: Vec<std::borrow::Cow<'_, str>> =
            texts.iter().map(|t| self.truncate_input(t)).collect();
        let payload: Vec<&str> = truncated.iter().map(|c| c.as_ref()).collect();

        let mut body = serde_json::json!({
            "input": payload,
            "model": self.model,
        });

        if self.send_dimensions && self.dimension > 0 {
            body["dimensions"] = serde_json::json!(self.dimension);
        }

        let mut request = self.client.post(&url).json(&body);

        if !self.api_key.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", self.api_key));
        }

        let response = request
            .send()
            .await
            .map_err(|e| AlephError::config(format!("Embedding API request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AlephError::config(format!(
                "Embedding API returned {status}: {body}"
            )));
        }

        // Check content-type before parsing — catch misconfigured URLs early
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        if !content_type.contains("json") {
            let body_snippet: String = response
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            return Err(AlephError::config(format!(
                "Embedding API returned non-JSON response (content-type: {content_type}). \
                 Check that api_base points to a valid OpenAI-compatible endpoint. Body: {body_snippet}"
            )));
        }

        let raw_body = response.text().await.map_err(|e| {
            AlephError::config(format!("Failed to read embedding response body: {e}"))
        })?;

        let resp: serde_json::Value = serde_json::from_str(&raw_body).map_err(|e| {
            let snippet: String = raw_body.chars().take(200).collect();
            AlephError::config(format!(
                "Failed to parse embedding response: {e}. Body: {snippet}"
            ))
        })?;

        let data = resp["data"]
            .as_array()
            .ok_or_else(|| AlephError::config("Missing 'data' array in response".to_string()))?;

        let mut embeddings: Vec<(usize, Vec<f32>)> = Vec::with_capacity(data.len());

        for item in data {
            let index = item["index"].as_u64().unwrap_or(0) as usize;
            let embedding: Vec<f32> = item["embedding"]
                .as_array()
                .ok_or_else(|| AlephError::config("Missing 'embedding' array".to_string()))?
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();

            embeddings.push((index, embedding));
        }

        embeddings.sort_by_key(|(idx, _)| *idx);

        let results: Vec<Vec<f32>> = embeddings
            .into_iter()
            .map(|(_, emb)| truncate_and_normalize(emb, self.dimension))
            .collect();

        Ok(results)
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for RemoteEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, AlephError> {
        let results = self.call_api(&[text]).await?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| AlephError::config("No embedding returned from API".to_string()))
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // Bounded-concurrency batch dispatch in input order: split the texts
        // into `batch_size` chunks, then drive at most `MAX_CONCURRENT_BATCHES`
        // chunks per wave via `try_join_all`. Supersedes the previous serial
        // `for chunk` loop (latency = sum of all chunks) with `ceil(chunks / n)`
        // waves instead. The futures are built in an explicit `for` loop rather
        // than `.map(|chunk| self.call_api(chunk))`: under `#[async_trait]`'s
        // boxed future, the mapping closure must satisfy a higher-ranked
        // lifetime bound for `call_api` futures borrowing both `self` and the
        // nested `&[&str]` chunk slice, which fails with "FnOnce is not general
        // enough". An explicit loop polls concrete futures without that bound
        // while preserving order within each wave.
        const MAX_CONCURRENT_BATCHES: usize = 4;
        // `chunks(0)` would panic — clamp defensively (config default is 32).
        let batch_size = self.batch_size.max(1);
        let chunks: Vec<&[&str]> = texts.chunks(batch_size).collect();

        let mut all_embeddings = Vec::with_capacity(texts.len());
        for wave in chunks.chunks(MAX_CONCURRENT_BATCHES) {
            let mut wave_futures = Vec::with_capacity(wave.len());
            for chunk in wave {
                wave_futures.push(self.call_api(chunk));
            }
            let wave_results = futures::future::try_join_all(wave_futures).await?;
            for chunk_embeddings in wave_results {
                all_embeddings.extend(chunk_embeddings);
            }
        }

        Ok(all_embeddings)
    }

    fn dimensions(&self) -> usize {
        self.dimension
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn provider_id(&self) -> &str {
        &self.provider_id
    }
}

/// Create an `EmbeddingProvider` from a provider config
pub fn create_provider(
    config: &EmbeddingProviderConfig,
) -> Result<Arc<dyn EmbeddingProvider>, AlephError> {
    let provider = RemoteEmbeddingProvider::from_config(config)?;
    Ok(Arc::new(provider))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn test_truncate_and_normalize_no_op_when_smaller() {
        let embedding = vec![0.6, 0.8];
        let result = truncate_and_normalize(embedding.clone(), 5);
        assert_eq!(result, embedding);
    }

    #[test]
    fn test_truncate_and_normalize_equal_dim() {
        let embedding = vec![0.6, 0.8];
        let result = truncate_and_normalize(embedding.clone(), 2);
        assert_eq!(result, embedding);
    }

    #[test]
    fn test_truncate_and_normalize_truncates_and_normalizes() {
        let embedding = vec![3.0, 4.0, 99.0, 99.0];
        let result = truncate_and_normalize(embedding, 2);
        assert_eq!(result.len(), 2);
        assert!((result[0] - 0.6).abs() < 1e-6);
        assert!((result[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_truncate_and_normalize_zero_vector() {
        let embedding = vec![0.0, 0.0, 0.0, 0.0];
        let result = truncate_and_normalize(embedding, 2);
        assert_eq!(result, vec![0.0, 0.0]);
    }

    fn make_provider_for_truncation(max_input_chars: usize) -> RemoteEmbeddingProvider {
        RemoteEmbeddingProvider {
            client: reqwest::Client::new(),
            api_base: "http://localhost/v1".to_string(),
            api_key: String::new(),
            model: "test".to_string(),
            dimension: 8,
            batch_size: 1,
            max_input_chars,
            send_dimensions: true,
            provider_id: "test".to_string(),
        }
    }

    #[test]
    fn truncate_input_passthrough_when_under_limit() {
        let provider = make_provider_for_truncation(100);
        let result = provider.truncate_input("hello world");
        // Borrowed = zero-alloc fast path. Owned would mean we cloned.
        assert!(matches!(result, std::borrow::Cow::Borrowed(_)));
        assert_eq!(result.as_ref(), "hello world");
    }

    #[test]
    fn truncate_input_cuts_at_char_boundary_for_oversize() {
        let provider = make_provider_for_truncation(5);
        // Mixed ASCII + CJK to exercise multi-byte char boundary safety.
        let result = provider.truncate_input("abc你好世界xyz");
        assert!(matches!(result, std::borrow::Cow::Owned(_)));
        assert_eq!(result.chars().count(), 5);
        // Must be valid UTF-8 — the test would panic on byte-boundary slicing.
        assert_eq!(result.as_ref(), "abc你好");
    }

    /// Mock embedding provider for tests
    pub(crate) struct MockEmbeddingProvider {
        dim: usize,
        model: String,
    }

    impl MockEmbeddingProvider {
        pub(crate) fn new(dim: usize, model: &str) -> Self {
            Self {
                dim,
                model: model.to_string(),
            }
        }
    }

    #[async_trait::async_trait]
    impl EmbeddingProvider for MockEmbeddingProvider {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, AlephError> {
            Ok(vec![0.1; self.dim])
        }

        async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, AlephError> {
            Ok(texts.iter().map(|_| vec![0.1; self.dim]).collect())
        }

        fn dimensions(&self) -> usize {
            self.dim
        }

        fn model_name(&self) -> &str {
            &self.model
        }

        fn provider_id(&self) -> &str {
            "mock"
        }
    }
}
