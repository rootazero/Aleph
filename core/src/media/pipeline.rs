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
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            policy: MediaPolicy::default(),
        }
    }

    /// Create pipeline with custom policy.
    pub fn with_policy(policy: MediaPolicy) -> Self {
        Self {
            providers: Vec::new(),
            policy,
        }
    }

    /// Get the policy.
    pub fn policy(&self) -> &MediaPolicy {
        &self.policy
    }

    /// Register a provider. Providers are sorted by priority on each call.
    pub fn add_provider(&mut self, provider: Box<dyn MediaProvider>) {
        self.providers.push(provider);
        self.providers.sort_by_key(|p| p.priority());
    }

    /// Number of registered providers.
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    /// Process media input through the pipeline.
    pub async fn process(
        &self,
        input: &MediaInput,
        media_type: &MediaType,
        prompt: Option<&str>,
    ) -> Result<MediaOutput, MediaError> {
        // 1. Policy check (file size if path)
        if let MediaInput::FilePath { path } = input {
            if path.exists() {
                if let Ok(metadata) = std::fs::metadata(path) {
                    self.policy.check_size(media_type, metadata.len())?;
                }
            }
        }

        // 2. Find providers that support this media type
        let eligible: Vec<_> = self.providers.iter().filter(|p| p.supports(media_type)).collect();

        if eligible.is_empty() {
            return Err(MediaError::NoProvider {
                media_type: media_type.category().to_string(),
            });
        }

        // 3. Try providers in priority order with fallback
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
                    last_err = e;
                }
            }
        }

        Err(last_err)
    }

    /// List supported media categories across all providers.
    pub fn supported_categories(&self) -> Vec<String> {
        let mut categories: Vec<String> = self
            .providers
            .iter()
            .flat_map(|p| p.supported_types())
            .map(|t| t.category().to_string())
            .collect();
        categories.sort();
        categories.dedup();
        categories
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
            width: None,
            height: None,
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
                confidence: 0.9,
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

    #[tokio::test]
    async fn empty_pipeline_returns_no_provider() {
        let pipeline = MediaPipeline::new();
        let err = pipeline
            .process(&sample_input(), &image_type(), None)
            .await
            .unwrap_err();
        assert!(matches!(err, MediaError::NoProvider { .. }));
    }

    #[tokio::test]
    async fn single_provider_success() {
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
                assert!(text.contains("[high]"), "Expected high-priority, got: {}", text)
            }
            _ => panic!("Expected Description"),
        }
    }

    #[test]
    fn supported_categories() {
        let mut pipeline = MediaPipeline::new();
        pipeline.add_provider(Box::new(SuccessProvider {
            name: "a",
            priority: 10,
            category: "image",
        }));
        pipeline.add_provider(Box::new(SuccessProvider {
            name: "b",
            priority: 20,
            category: "audio",
        }));
        pipeline.add_provider(Box::new(SuccessProvider {
            name: "c",
            priority: 30,
            category: "image",
        }));

        let cats = pipeline.supported_categories();
        assert_eq!(cats, vec!["audio", "image"]);
    }

    #[test]
    fn provider_count() {
        let mut pipeline = MediaPipeline::new();
        assert_eq!(pipeline.provider_count(), 0);
        pipeline.add_provider(Box::new(SuccessProvider {
            name: "a",
            priority: 10,
            category: "image",
        }));
        assert_eq!(pipeline.provider_count(), 1);
    }
}
