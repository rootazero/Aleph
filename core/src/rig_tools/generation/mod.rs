//! Media generation tools
//!
//! Tools for generating images, speech, and other media using AI providers.
//! These tools implement rig's Tool trait for AI agent integration.

mod image_generate;
mod speech_generate;

pub use image_generate::{ImageGenerateArgs, ImageGenerateOutput, ImageGenerateTool};
pub use speech_generate::{SpeechGenerateArgs, SpeechGenerateOutput, SpeechGenerateTool};

use crate::generation::GenerationError;
use crate::rig_tools::error::ToolError;

/// Convert GenerationError to ToolError for tool execution
impl From<GenerationError> for ToolError {
    fn from(err: GenerationError) -> Self {
        match &err {
            GenerationError::NetworkError { message } => ToolError::Network(message.clone()),
            GenerationError::InvalidParametersError { message, .. } => {
                ToolError::InvalidArgs(message.clone())
            }
            GenerationError::AuthenticationError { message, .. } => {
                ToolError::InvalidArgs(format!("Authentication failed: {}", message))
            }
            GenerationError::RateLimitError { message, .. } => {
                ToolError::Execution(format!("Rate limited: {}", message))
            }
            GenerationError::ContentFilteredError { message, .. } => {
                ToolError::Execution(format!("Content filtered: {}", message))
            }
            _ => ToolError::Execution(err.to_string()),
        }
    }
}
