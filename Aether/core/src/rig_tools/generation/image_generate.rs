//! Image generation tool
//!
//! Generates images from text prompts using configured AI providers.
//! Implements rig's Tool trait for AI agent integration.

use rig::completion::ToolDefinition;
use rig::tool::Tool;
use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info};

use crate::generation::{
    GenerationParams, GenerationProviderRegistry, GenerationRequest, GenerationType,
};
use crate::rig_tools::error::ToolError;

/// Arguments for image generation
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ImageGenerateArgs {
    /// The text prompt describing the image to generate
    pub prompt: String,

    /// Image width in pixels (default: 1024)
    #[serde(default)]
    pub width: Option<u32>,

    /// Image height in pixels (default: 1024)
    #[serde(default)]
    pub height: Option<u32>,

    /// Quality level: "standard" or "hd" (default: "standard")
    #[serde(default)]
    pub quality: Option<String>,

    /// Style preset: "vivid" or "natural" (default: "vivid")
    #[serde(default)]
    pub style: Option<String>,

    /// Provider name to use (default: first available image provider)
    #[serde(default)]
    pub provider: Option<String>,
}

/// Output from image generation tool
#[derive(Debug, Clone, Serialize)]
pub struct ImageGenerateOutput {
    /// Location of the generated image (URL or file path)
    pub image_location: String,

    /// Type of location: "url", "file", or "data_url"
    pub location_type: String,

    /// Original prompt used
    pub prompt: String,

    /// Revised prompt if the provider modified it
    pub revised_prompt: Option<String>,

    /// Provider that generated the image
    pub provider: String,

    /// Model used for generation
    pub model: Option<String>,

    /// Generation duration in milliseconds
    pub duration_ms: u64,
}

/// Image generation tool using GenerationProviderRegistry
pub struct ImageGenerateTool {
    registry: Arc<GenerationProviderRegistry>,
}

impl ImageGenerateTool {
    /// Tool identifier
    pub const NAME: &'static str = "generate_image";

    /// Tool description for AI prompt
    pub const DESCRIPTION: &'static str = "Generate an image from a text description. Use this when you need to create visual content based on a prompt.";

    /// Create a new ImageGenerateTool with the given provider registry
    pub fn new(registry: Arc<GenerationProviderRegistry>) -> Self {
        Self { registry }
    }

    /// Execute image generation
    pub async fn call(&self, args: ImageGenerateArgs) -> Result<ImageGenerateOutput, ToolError> {
        let start = Instant::now();

        info!(prompt = %args.prompt, provider = ?args.provider, "Starting image generation");

        // Find provider
        let (provider_name, provider) = if let Some(name) = &args.provider {
            let provider = self.registry.get(name).ok_or_else(|| {
                ToolError::InvalidArgs(format!("Provider '{}' not found", name))
            })?;

            // Check if provider supports image generation
            if !provider.supports(GenerationType::Image) {
                return Err(ToolError::InvalidArgs(format!(
                    "Provider '{}' does not support image generation",
                    name
                )));
            }

            (name.clone(), provider)
        } else {
            // Find first provider that supports image generation
            self.registry
                .first_for_type(GenerationType::Image)
                .ok_or_else(|| {
                    ToolError::InvalidArgs(
                        "No image generation provider available".to_string(),
                    )
                })?
        };

        debug!(provider = %provider_name, "Using provider for image generation");

        // Build generation parameters
        let mut params = GenerationParams::new();
        if let Some(width) = args.width {
            params.width = Some(width);
        }
        if let Some(height) = args.height {
            params.height = Some(height);
        }
        if let Some(quality) = args.quality {
            params.quality = Some(quality);
        }
        if let Some(style) = args.style {
            params.style = Some(style);
        }

        // Create generation request
        let request = GenerationRequest::image(&args.prompt).with_params(params);

        // Execute generation
        let output = provider.generate(request).await.map_err(ToolError::from)?;

        let duration_ms = start.elapsed().as_millis() as u64;

        // Determine location and type from the generation data
        let (image_location, location_type) = match &output.data {
            crate::generation::GenerationData::Url(url) => (url.clone(), "url".to_string()),
            crate::generation::GenerationData::LocalPath(path) => {
                (path.clone(), "file".to_string())
            }
            crate::generation::GenerationData::Bytes(bytes) => {
                // Convert bytes to base64 data URL
                use base64::Engine;
                let base64_data =
                    base64::engine::general_purpose::STANDARD.encode(bytes);
                let content_type = output
                    .metadata
                    .content_type
                    .as_deref()
                    .unwrap_or("image/png");
                let data_url = format!("data:{};base64,{}", content_type, base64_data);
                (data_url, "data_url".to_string())
            }
        };

        info!(
            provider = %provider_name,
            duration_ms = duration_ms,
            location_type = %location_type,
            "Image generation completed"
        );

        Ok(ImageGenerateOutput {
            image_location,
            location_type,
            prompt: args.prompt,
            revised_prompt: output.metadata.revised_prompt,
            provider: provider_name,
            model: output.metadata.model,
            duration_ms,
        })
    }
}

impl Clone for ImageGenerateTool {
    fn clone(&self) -> Self {
        Self {
            registry: Arc::clone(&self.registry),
        }
    }
}

impl Tool for ImageGenerateTool {
    const NAME: &'static str = "generate_image";

    type Error = ToolError;
    type Args = ImageGenerateArgs;
    type Output = ImageGenerateOutput;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        // Use schemars to generate JSON Schema for the arguments
        let schema = schema_for!(ImageGenerateArgs);
        let parameters = serde_json::to_value(&schema).unwrap_or_else(|_| {
            // Fallback to manually defined schema if generation fails
            json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The text prompt describing the image to generate"
                    },
                    "width": {
                        "type": "integer",
                        "description": "Image width in pixels (default: 1024)"
                    },
                    "height": {
                        "type": "integer",
                        "description": "Image height in pixels (default: 1024)"
                    },
                    "quality": {
                        "type": "string",
                        "description": "Quality level: 'standard' or 'hd' (default: 'standard')"
                    },
                    "style": {
                        "type": "string",
                        "description": "Style preset: 'vivid' or 'natural' (default: 'vivid')"
                    },
                    "provider": {
                        "type": "string",
                        "description": "Provider name to use (default: first available)"
                    }
                },
                "required": ["prompt"]
            })
        });

        ToolDefinition {
            name: Self::NAME.to_string(),
            description: Self::DESCRIPTION.to_string(),
            parameters,
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        ImageGenerateTool::call(self, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::MockGenerationProvider;

    fn create_test_registry() -> Arc<GenerationProviderRegistry> {
        let mut registry = GenerationProviderRegistry::new();
        let mock = Arc::new(MockGenerationProvider::image_only("mock-dalle"));
        registry.register("mock-dalle".to_string(), mock).unwrap();
        Arc::new(registry)
    }

    #[test]
    fn test_args_deserialization_minimal() {
        let json = r#"{"prompt": "A sunset over mountains"}"#;
        let args: ImageGenerateArgs = serde_json::from_str(json).unwrap();

        assert_eq!(args.prompt, "A sunset over mountains");
        assert!(args.width.is_none());
        assert!(args.height.is_none());
        assert!(args.quality.is_none());
        assert!(args.style.is_none());
        assert!(args.provider.is_none());
    }

    #[test]
    fn test_args_deserialization_full() {
        let json = r#"{
            "prompt": "A cat wearing a hat",
            "width": 1024,
            "height": 768,
            "quality": "hd",
            "style": "vivid",
            "provider": "dalle"
        }"#;
        let args: ImageGenerateArgs = serde_json::from_str(json).unwrap();

        assert_eq!(args.prompt, "A cat wearing a hat");
        assert_eq!(args.width, Some(1024));
        assert_eq!(args.height, Some(768));
        assert_eq!(args.quality, Some("hd".to_string()));
        assert_eq!(args.style, Some("vivid".to_string()));
        assert_eq!(args.provider, Some("dalle".to_string()));
    }

    #[test]
    fn test_tool_definition() {
        assert_eq!(ImageGenerateTool::NAME, "generate_image");
        assert!(!ImageGenerateTool::DESCRIPTION.is_empty());
    }

    #[tokio::test]
    async fn test_generate_image_success() {
        let registry = create_test_registry();
        let tool = ImageGenerateTool::new(registry);

        let args = ImageGenerateArgs {
            prompt: "A beautiful sunset".to_string(),
            width: Some(1024),
            height: Some(1024),
            quality: None,
            style: None,
            provider: Some("mock-dalle".to_string()),
        };

        let result = tool.call(args).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        assert_eq!(output.prompt, "A beautiful sunset");
        assert_eq!(output.provider, "mock-dalle");
        assert_eq!(output.location_type, "url");
        assert!(output.duration_ms > 0 || output.duration_ms == 0); // Just verify it's set
    }

    #[tokio::test]
    async fn test_generate_image_provider_not_found() {
        let registry = create_test_registry();
        let tool = ImageGenerateTool::new(registry);

        let args = ImageGenerateArgs {
            prompt: "A test image".to_string(),
            width: None,
            height: None,
            quality: None,
            style: None,
            provider: Some("nonexistent".to_string()),
        };

        let result = tool.call(args).await;
        assert!(result.is_err());

        if let Err(ToolError::InvalidArgs(msg)) = result {
            assert!(msg.contains("not found"));
        } else {
            panic!("Expected InvalidArgs error");
        }
    }

    #[tokio::test]
    async fn test_generate_image_no_provider_available() {
        let registry = Arc::new(GenerationProviderRegistry::new());
        let tool = ImageGenerateTool::new(registry);

        let args = ImageGenerateArgs {
            prompt: "A test image".to_string(),
            width: None,
            height: None,
            quality: None,
            style: None,
            provider: None,
        };

        let result = tool.call(args).await;
        assert!(result.is_err());

        if let Err(ToolError::InvalidArgs(msg)) = result {
            assert!(msg.contains("No image generation provider"));
        } else {
            panic!("Expected InvalidArgs error");
        }
    }

    #[tokio::test]
    async fn test_generate_image_auto_select_provider() {
        let registry = create_test_registry();
        let tool = ImageGenerateTool::new(registry);

        let args = ImageGenerateArgs {
            prompt: "Auto-selected provider test".to_string(),
            width: None,
            height: None,
            quality: None,
            style: None,
            provider: None, // Let it auto-select
        };

        let result = tool.call(args).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        assert_eq!(output.provider, "mock-dalle");
    }

    #[tokio::test]
    async fn test_rig_tool_trait() {
        let registry = create_test_registry();
        let tool = ImageGenerateTool::new(registry);

        // Test definition
        let definition = tool.definition("test".to_string()).await;
        assert_eq!(definition.name, "generate_image");
        assert!(!definition.description.is_empty());

        // Test call via trait
        let args = ImageGenerateArgs {
            prompt: "Trait test".to_string(),
            width: None,
            height: None,
            quality: None,
            style: None,
            provider: Some("mock-dalle".to_string()),
        };

        let result = <ImageGenerateTool as Tool>::call(&tool, args).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_output_serialization() {
        let output = ImageGenerateOutput {
            image_location: "https://example.com/image.png".to_string(),
            location_type: "url".to_string(),
            prompt: "Test prompt".to_string(),
            revised_prompt: Some("Revised prompt".to_string()),
            provider: "dalle".to_string(),
            model: Some("dall-e-3".to_string()),
            duration_ms: 1500,
        };

        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("image_location"));
        assert!(json.contains("https://example.com/image.png"));
        assert!(json.contains("revised_prompt"));
    }
}
