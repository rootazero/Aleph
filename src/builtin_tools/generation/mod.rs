//! Media generation tools
//!
//! Tools for generating images, speech, and other media using AI providers.
//! These tools implement rig's Tool trait for AI agent integration.

mod audio_generate;
mod image_generate;
mod speech_generate;
mod video_generate;

pub use audio_generate::{AudioGenerateArgs, AudioGenerateOutput, AudioGenerateTool};
pub use image_generate::{ImageGenerateArgs, ImageGenerateOutput, ImageGenerateTool};
pub use speech_generate::{SpeechGenerateArgs, SpeechGenerateOutput, SpeechGenerateTool};
pub use video_generate::{VideoGenerateArgs, VideoGenerateOutput, VideoGenerateTool};

use crate::builtin_tools::error::ToolError;
use crate::generation::GenerationError;

/// Convert GenerationError to ToolError for tool execution
impl From<GenerationError> for ToolError {
    fn from(err: GenerationError) -> Self {
        match &err {
            GenerationError::NetworkError { message } => Self::Network(message.clone()),
            GenerationError::InvalidParametersError { message, .. } => {
                Self::InvalidArgs(message.clone())
            }
            GenerationError::AuthenticationError { message, .. } => {
                Self::InvalidArgs(format!("Authentication failed: {message}"))
            }
            GenerationError::RateLimitError { message, .. } => {
                Self::Execution(format!("Rate limited: {message}"))
            }
            GenerationError::ContentFilteredError { message, .. } => {
                Self::Execution(format!("Content filtered: {message}"))
            }
            _ => Self::Execution(err.to_string()),
        }
    }
}
