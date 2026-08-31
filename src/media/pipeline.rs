//! Media pipeline orchestrator — routes media to providers with fallback.

use super::error::MediaError;
use super::policy::MediaPolicy;
use super::provider::MediaProvider;
use super::types::{MediaInput, MediaOutput, MediaType};

/// Orchestrates media understanding across multiple providers.
///
/// The pipeline:
/// 1. Detects media format (if not already known)
/// 2. Enforces size/duration policy
/// 3. Routes to providers sorted by priority
/// 4. Falls back to next provider on failure
pub struct MediaPipeline {
    providers: Vec<Box<dyn MediaProvider>>,
    policy: MediaPolicy,
}

impl MediaPipeline {
    /// Create pipeline with default policy.
    #[must_use]
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            policy: MediaPolicy::default(),
        }
    }

    /// Register a provider. Providers are sorted by priority on each call.
    pub fn add_provider(&mut self, provider: Box<dyn MediaProvider>) {
        self.providers.push(provider);
        self.providers.sort_by_key(|p| p.priority());
    }

    /// Process media input through the pipeline.
    pub async fn process(
        &self,
        input: &MediaInput,
        media_type: &MediaType,
        prompt: Option<&str>,
    ) -> Result<MediaOutput, MediaError> {
        if let MediaInput::Url { url } = input {
            crate::security::ssrf::validate_url_async(
                url,
                &crate::security::ssrf::SsrfPolicy::default(),
            )
            .await
            .map(|(_, _pinned)| ())
            .map_err(|e| MediaError::ProviderError {
                provider: "ssrf".to_string(),
                message: format!("media URL blocked: {e}"),
            })?;
        }

        // 1. Policy check (file size if path).
        //
        // SECURITY: `cache.rs::safe_fetch` enforces the size cap only on URL
        // inputs, so this is the ONLY size gate for `MediaInput::FilePath`.
        // The previous impl silently swallowed I/O errors (`unwrap_or(false)`
        // and `if let Ok(...)`), so a metadata failure (permission, sandbox
        // quirk, or TOCTOU) bypassed policy entirely and the provider opened
        // the file unrestricted — a DoS / OOM vector. Propagate errors: if
        // we cannot prove the file is small enough, refuse it.
        if let MediaInput::FilePath { path } = input {
            match tokio::fs::metadata(path).await {
                Ok(metadata) => {
                    self.policy.check_size(media_type, metadata.len())?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // File missing — let the provider surface the not-found.
                }
                Err(e) => {
                    return Err(MediaError::ProviderError {
                        provider: "policy".to_string(),
                        message: format!(
                            "cannot stat FilePath {path:?} for {} size policy: {e}",
                            media_type.category()
                        ),
                    });
                }
            }
        }

        // 1b. Same policy check for inline payloads. The cache layer's
        // `safe_fetch` enforces `MAX_FILE_SIZE = 50 MB` on URL inputs
        // (see `cache.rs::resolve_url`) and the data-URL pre-decode cap
        // catches inline payloads (see `cache.rs::decode_data_url`),
        // so this is defense-in-depth — a misrouted Base64 image
        // reaches the vision provider only after the policy module
        // has had a chance to refuse it. Base64 inflates by 4/3, so
        // estimate the decoded length and check against the same
        // policy cap the FilePath branch uses.
        if let MediaInput::Base64 { data, .. } = input {
            let estimated = data.len().saturating_mul(3) / 4;
            self.policy.check_size(media_type, estimated as u64)?;
        }

        // 2. Find providers that support this media type
        let eligible: Vec<_> = self
            .providers
            .iter()
            .filter(|p| p.supports(media_type))
            .collect();

        if eligible.is_empty() {
            return Err(MediaError::NoProvider {
                media_type: media_type.category().to_string(),
            });
        }

        // 3. Try providers in priority order with fallback. Track every
        // failure so the operator-visible error names all of them, not
        // just the last one (the most diagnostic is usually the first).
        let mut attempts: Vec<(String, String)> = Vec::with_capacity(eligible.len());
        let mut last_err = MediaError::NoProvider {
            media_type: media_type.category().to_string(),
        };

        for provider in &eligible {
            match provider.process(input, media_type, prompt).await {
                Ok(output) => return Ok(output),
                Err(e) => {
                    tracing::warn!(
                        provider = provider.name(),
                        error = %e,
                        "Media provider failed, trying next"
                    );
                    attempts.push((provider.name().to_string(), e.to_string()));
                    last_err = e;
                }
            }
        }

        if attempts.len() > 1 {
            // Surface every provider's failure in the final error so
            // operator UI / logging can name them all without reaching
            // back through the WARN-level trace above.
            let summary = attempts
                .iter()
                .map(|(n, e)| format!("{n}: {e}"))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(MediaError::ProviderError {
                provider: "all".to_string(),
                message: format!("all {} providers failed: {summary}", attempts.len()),
            });
        }

        Err(last_err)
    }
}

impl Default for MediaPipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::types::*;
    use async_trait::async_trait;

    struct SuccessProvider {
        name: &'static str,
        priority: u8,
        category: &'static str,
    }
    struct FailProvider {
        name: &'static str,
    }

    fn image_type() -> MediaType {
        MediaType::Image {
            format: MediaImageFormat::Png,
        }
    }
    fn audio_type() -> MediaType {
        MediaType::Audio {
            format: AudioFormat::Mp3,
            duration_secs: None,
        }
    }

    #[async_trait]
    impl MediaProvider for SuccessProvider {
        fn name(&self) -> &str {
            self.name
        }
        fn priority(&self) -> u8 {
            self.priority
        }
        fn supported_types(&self) -> Vec<MediaType> {
            match self.category {
                "image" => vec![image_type()],
                "audio" => vec![audio_type()],
                _ => vec![],
            }
        }
        async fn process(
            &self,
            _: &MediaInput,
            _: &MediaType,
            _: Option<&str>,
        ) -> Result<MediaOutput, MediaError> {
            Ok(MediaOutput::Description {
                text: format!("[{}] ok", self.name),
            })
        }
    }

    #[async_trait]
    impl MediaProvider for FailProvider {
        fn name(&self) -> &str {
            self.name
        }
        fn supported_types(&self) -> Vec<MediaType> {
            vec![image_type()]
        }
        async fn process(
            &self,
            _: &MediaInput,
            _: &MediaType,
            _: Option<&str>,
        ) -> Result<MediaOutput, MediaError> {
            Err(MediaError::ProviderError {
                provider: self.name.into(),
                message: "mock failure".into(),
            })
        }
    }

    fn sample_input() -> MediaInput {
        MediaInput::Url {
            url: "https://example.com/test.png".into(),
        }
    }

    fn install_example_public_ip() -> crate::security::ssrf::dns::test_hook::ResolverScope {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "example.com".to_string(),
            vec!["1.1.1.1".parse::<std::net::IpAddr>().unwrap()],
        );
        crate::security::ssrf::dns::test_hook::ResolverScope::install(map)
    }

    #[tokio::test]
    async fn empty_pipeline_returns_no_provider() {
        let _guard = install_example_public_ip();
        let pipeline = MediaPipeline::new();
        let err = pipeline
            .process(&sample_input(), &image_type(), None)
            .await
            .unwrap_err();
        assert!(matches!(err, MediaError::NoProvider { .. }));
    }

    #[tokio::test]
    async fn single_provider_success() {
        let _guard = install_example_public_ip();
        let mut pipeline = MediaPipeline::new();
        pipeline.add_provider(Box::new(SuccessProvider {
            name: "claude",
            priority: 10,
            category: "image",
        }));

        let result = pipeline
            .process(&sample_input(), &image_type(), Some("describe"))
            .await
            .unwrap();
        match result {
            MediaOutput::Description { text, .. } => assert!(text.contains("[claude]")),
            _ => panic!("Expected Description"),
        }
    }

    #[tokio::test]
    async fn fallback_on_failure() {
        let _guard = install_example_public_ip();
        let mut pipeline = MediaPipeline::new();
        pipeline.add_provider(Box::new(FailProvider { name: "primary" }));
        pipeline.add_provider(Box::new(SuccessProvider {
            name: "backup",
            priority: 50,
            category: "image",
        }));

        let result = pipeline
            .process(&sample_input(), &image_type(), None)
            .await
            .unwrap();
        match result {
            MediaOutput::Description { text, .. } => assert!(text.contains("[backup]")),
            _ => panic!("Expected Description from backup"),
        }
    }

    #[tokio::test]
    async fn skips_providers_without_matching_category() {
        let _guard = install_example_public_ip();
        let mut pipeline = MediaPipeline::new();
        pipeline.add_provider(Box::new(SuccessProvider {
            name: "audio-only",
            priority: 1,
            category: "audio",
        }));
        pipeline.add_provider(Box::new(SuccessProvider {
            name: "image-handler",
            priority: 10,
            category: "image",
        }));

        let result = pipeline
            .process(&sample_input(), &image_type(), None)
            .await
            .unwrap();
        match result {
            MediaOutput::Description { text, .. } => assert!(text.contains("[image-handler]")),
            _ => panic!("Expected image-handler"),
        }
    }

    #[tokio::test]
    async fn priority_ordering() {
        let _guard = install_example_public_ip();
        let mut pipeline = MediaPipeline::new();
        pipeline.add_provider(Box::new(SuccessProvider {
            name: "low",
            priority: 100,
            category: "image",
        }));
        pipeline.add_provider(Box::new(SuccessProvider {
            name: "high",
            priority: 1,
            category: "image",
        }));

        let result = pipeline
            .process(&sample_input(), &image_type(), None)
            .await
            .unwrap();
        match result {
            MediaOutput::Description { text, .. } => {
                assert!(
                    text.contains("[high]"),
                    "Expected high-priority, got: {}",
                    text
                )
            }
            _ => panic!("Expected Description"),
        }
    }
}
