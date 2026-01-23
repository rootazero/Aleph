//! Response parsing types for generation requests
//!
//! This module contains types for parsing AI responses that contain
//! generation request tags.

/// FFI-safe parsed generation request from AI response
///
/// When AI recognizes a generation model mention in conversation,
/// it outputs a `[GENERATE:type:provider:model:prompt]` tag that
/// gets parsed into this structure.
#[derive(Debug, Clone)]
pub struct ParsedGenerationRequestFFI {
    /// Generation type (image, video, audio, speech)
    pub gen_type: String,
    /// Provider name (e.g., "midjourney", "dalle")
    pub provider: String,
    /// Model name or alias (e.g., "nanobanana" -> "nano-banana-2")
    pub model: String,
    /// Generation prompt
    pub prompt: String,
    /// Original matched text (for replacement in response)
    pub original_text: String,
}

/// FFI-safe parse result containing requests and cleaned response
#[derive(Debug, Clone)]
pub struct ParseResultFFI {
    /// Extracted generation requests
    pub requests: Vec<ParsedGenerationRequestFFI>,
    /// Response text with generation tags replaced by user-friendly messages
    pub cleaned_response: String,
}
