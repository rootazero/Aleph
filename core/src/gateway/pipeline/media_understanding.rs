//! Media understanding stage — uses an LLM provider to generate
//! descriptions of downloaded media items.

use std::path::Path;

use crate::sync_primitives::Arc;

use super::types::{LocalMedia, MediaCategory, MediaUnderstanding, UnderstandingType};

// ---------------------------------------------------------------------------
// UnderstandingProvider trait
// ---------------------------------------------------------------------------

/// Abstraction over the LLM call that produces a textual understanding
/// of a media file. Implementations may call OpenAI, Anthropic, etc.
#[async_trait::async_trait]
pub trait UnderstandingProvider: Send + Sync {
    /// Analyse the file at `local_path` and return a human-readable
    /// description together with the number of tokens consumed.
    async fn understand(
        &self,
        local_path: &Path,
        category: &MediaCategory,
        model: &str,
    ) -> Result<(String, u64), String>;
}

// ---------------------------------------------------------------------------
// Prompt templates
// ---------------------------------------------------------------------------

/// Prompt templates used when calling the understanding provider.
pub mod prompts {
    use super::MediaCategory;

    /// Return the system prompt appropriate for the given media category.
    pub fn for_category(category: &MediaCategory) -> &'static str {
        match category {
            MediaCategory::Image => {
                "Describe this image concisely in the user's language. \
                 Focus on the key visual elements and context."
            }
            MediaCategory::Link => {
                "Summarize this webpage content in 2-3 sentences. \
                 Capture the main topic and key points."
            }
            MediaCategory::Document => {
                "Summarize this document concisely. \
                 Highlight the main points and conclusions."
            }
            // Audio / Video / Unknown should never reach here in normal flow,
            // but we return a sensible fallback just in case.
            _ => "Describe this content briefly.",
        }
    }
}

// ---------------------------------------------------------------------------
// MediaUnderstander
// ---------------------------------------------------------------------------

/// Orchestrates media understanding by delegating to an [`UnderstandingProvider`].
pub struct MediaUnderstander {
    provider: Arc<dyn UnderstandingProvider>,
    default_model: String,
}

impl MediaUnderstander {
    /// Create a new understander with the given provider and default model name.
    pub fn new(provider: Arc<dyn UnderstandingProvider>, default_model: String) -> Self {
        Self {
            provider,
            default_model,
        }
    }

    /// Process a single media item, returning its understanding.
    async fn understand_one(
        &self,
        media: &LocalMedia,
        model: &str,
    ) -> (MediaUnderstanding, u64) {
        let (understanding_type, description, tokens) = match &media.media_category {
            MediaCategory::Image => match self
                .provider
                .understand(&media.local_path, &media.media_category, model)
                .await
            {
                Ok((desc, tok)) => (UnderstandingType::ImageDescription, desc, tok),
                Err(e) => (UnderstandingType::Skipped(e.clone()), e, 0),
            },
            MediaCategory::Link => match self
                .provider
                .understand(&media.local_path, &media.media_category, model)
                .await
            {
                Ok((desc, tok)) => (UnderstandingType::LinkSummary, desc, tok),
                Err(e) => (UnderstandingType::Skipped(e.clone()), e, 0),
            },
            MediaCategory::Document => match self
                .provider
                .understand(&media.local_path, &media.media_category, model)
                .await
            {
                Ok((desc, tok)) => (UnderstandingType::DocumentSummary, desc, tok),
                Err(e) => (UnderstandingType::Skipped(e.clone()), e, 0),
            },
            MediaCategory::Audio | MediaCategory::Video => {
                let reason = "not yet supported".to_string();
                (
                    UnderstandingType::Skipped(reason.clone()),
                    reason,
                    0,
                )
            }
            MediaCategory::Unknown => {
                let reason = "unknown media type".to_string();
                (
                    UnderstandingType::Skipped(reason.clone()),
                    reason,
                    0,
                )
            }
        };

        let understanding = MediaUnderstanding {
            media: media.clone(),
            description,
            understanding_type,
        };
        (understanding, tokens)
    }

    /// Process all media items concurrently, returning the list of
    /// understandings and the total token count.
    pub async fn understand_all(
        &self,
        media: &[LocalMedia],
        agent_model_override: Option<&str>,
    ) -> (Vec<MediaUnderstanding>, u64) {
        let model = agent_model_override.unwrap_or(&self.default_model);

        let futures: Vec<_> = media
            .iter()
            .map(|m| self.understand_one(m, model))
            .collect();

        let results = futures::future::join_all(futures).await;

        let mut understandings = Vec::with_capacity(results.len());
        let mut total_tokens = 0u64;
        for (understanding, tokens) in results {
            total_tokens += tokens;
            understandings.push(understanding);
        }

        (understandings, total_tokens)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::gateway::channel::Attachment;

    /// Mock provider that returns pre-configured results in order.
    struct MockProvider {
        results: tokio::sync::Mutex<Vec<Result<(String, u64), String>>>,
    }

    impl MockProvider {
        fn new(results: Vec<Result<(String, u64), String>>) -> Self {
            Self {
                results: tokio::sync::Mutex::new(results),
            }
        }
    }

    #[async_trait::async_trait]
    impl UnderstandingProvider for MockProvider {
        async fn understand(
            &self,
            _local_path: &Path,
            _category: &MediaCategory,
            _model: &str,
        ) -> Result<(String, u64), String> {
            let mut results = self.results.lock().await;
            if results.is_empty() {
                Err("no more mock results".to_string())
            } else {
                results.remove(0)
            }
        }
    }

    /// Mock that records the model it was called with.
    struct ModelCapturingProvider {
        model_used: tokio::sync::Mutex<Vec<String>>,
    }

    impl ModelCapturingProvider {
        fn new() -> Self {
            Self {
                model_used: tokio::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl UnderstandingProvider for ModelCapturingProvider {
        async fn understand(
            &self,
            _local_path: &Path,
            _category: &MediaCategory,
            model: &str,
        ) -> Result<(String, u64), String> {
            self.model_used.lock().await.push(model.to_string());
            Ok(("ok".to_string(), 10))
        }
    }

    fn make_local_media(category: MediaCategory) -> LocalMedia {
        LocalMedia {
            original: Attachment {
                id: "a1".to_string(),
                mime_type: "test/test".to_string(),
                filename: Some("test.bin".to_string()),
                size: None,
                url: None,
                path: None,
                data: None,
            },
            local_path: PathBuf::from("/tmp/test.bin"),
            media_category: category,
        }
    }

    #[tokio::test]
    async fn test_understand_image() {
        let provider = Arc::new(MockProvider::new(vec![Ok((
            "A cat on a desk".to_string(),
            42,
        ))]));
        let understander = MediaUnderstander::new(provider, "gpt-4o".to_string());
        let media = vec![make_local_media(MediaCategory::Image)];

        let (results, tokens) = understander.understand_all(&media, None).await;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].description, "A cat on a desk");
        assert_eq!(results[0].understanding_type, UnderstandingType::ImageDescription);
        assert_eq!(tokens, 42);
    }

    #[tokio::test]
    async fn test_understand_skips_audio() {
        let provider = Arc::new(MockProvider::new(vec![]));
        let understander = MediaUnderstander::new(provider, "gpt-4o".to_string());
        let media = vec![make_local_media(MediaCategory::Audio)];

        let (results, tokens) = understander.understand_all(&media, None).await;

        assert_eq!(results.len(), 1);
        assert!(matches!(
            &results[0].understanding_type,
            UnderstandingType::Skipped(reason) if reason == "not yet supported"
        ));
        assert_eq!(tokens, 0);
    }

    #[tokio::test]
    async fn test_understand_failure_becomes_skipped() {
        let provider = Arc::new(MockProvider::new(vec![Err(
            "provider timeout".to_string(),
        )]));
        let understander = MediaUnderstander::new(provider, "gpt-4o".to_string());
        let media = vec![make_local_media(MediaCategory::Image)];

        let (results, tokens) = understander.understand_all(&media, None).await;

        assert_eq!(results.len(), 1);
        assert!(matches!(
            &results[0].understanding_type,
            UnderstandingType::Skipped(reason) if reason == "provider timeout"
        ));
        assert_eq!(tokens, 0);
    }

    #[tokio::test]
    async fn test_understand_concurrent_multiple() {
        let provider = Arc::new(MockProvider::new(vec![
            Ok(("desc 1".to_string(), 30)),
            Ok(("desc 2".to_string(), 20)),
        ]));
        let understander = MediaUnderstander::new(provider, "gpt-4o".to_string());
        let media = vec![
            make_local_media(MediaCategory::Image),
            make_local_media(MediaCategory::Document),
        ];

        let (results, tokens) = understander.understand_all(&media, None).await;

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].understanding_type, UnderstandingType::ImageDescription);
        assert_eq!(results[1].understanding_type, UnderstandingType::DocumentSummary);
        assert_eq!(tokens, 50);
    }

    #[tokio::test]
    async fn test_model_override() {
        let provider = Arc::new(ModelCapturingProvider::new());
        let provider_ref = Arc::clone(&provider);
        let understander = MediaUnderstander::new(provider, "default-model".to_string());
        let media = vec![make_local_media(MediaCategory::Image)];

        // With override
        understander
            .understand_all(&media, Some("override-model"))
            .await;

        let models = provider_ref.model_used.lock().await;
        assert_eq!(models[0], "override-model");
    }

    #[test]
    fn test_prompts() {
        let image_prompt = prompts::for_category(&MediaCategory::Image);
        assert!(image_prompt.contains("image"));
        assert!(image_prompt.contains("Describe"));

        let link_prompt = prompts::for_category(&MediaCategory::Link);
        assert!(link_prompt.contains("Summarize"));
        assert!(link_prompt.contains("webpage"));

        let doc_prompt = prompts::for_category(&MediaCategory::Document);
        assert!(doc_prompt.contains("Summarize"));
        assert!(doc_prompt.contains("document"));
    }
}
