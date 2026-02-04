//! Protocol implementations for different AI APIs
//!
//! Each protocol handles the specific request/response format for an API family.

pub mod openai;
pub mod anthropic;
pub mod gemini;

pub use openai::OpenAiProtocol;
pub use anthropic::AnthropicProtocol;
pub use gemini::GeminiProtocol;
