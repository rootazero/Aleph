use async_trait::async_trait;

use super::error::VisionError;
use super::types::{ImageInput, OcrResult, VisionCapabilities, VisionResult};

/// Trait for pluggable vision backends.
///
/// Implementations may delegate to:
/// - A multimodal LLM (e.g. Claude, GPT-4V) for image understanding
/// - A platform-native OCR engine (e.g. macOS Vision framework via Desktop Bridge)
/// - A local or remote object-detection model
///
/// The [`VisionPipeline`](super::VisionPipeline) orchestrates multiple providers,
/// trying them in registration order until one succeeds.
#[async_trait]
pub trait VisionProvider: Send + Sync {
    /// Describe / answer a question about the given image.
    async fn understand_image(
        &self,
        image: &ImageInput,
        prompt: &str,
    ) -> Result<VisionResult, VisionError>;

    /// Extract text from the given image via OCR.
    async fn ocr(&self, image: &ImageInput) -> Result<OcrResult, VisionError>;

    /// Report which capabilities this provider supports.
    fn capabilities(&self) -> VisionCapabilities;

    /// Human-readable name of this provider (used for logging / diagnostics).
    fn name(&self) -> &str;
}
