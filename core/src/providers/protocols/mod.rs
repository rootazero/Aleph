//! Protocol implementations for different AI APIs
//!
//! Each protocol handles the specific request/response format for an API family.

pub mod openai;
pub mod anthropic;
pub mod gemini;
pub mod definition;
pub mod registry;
pub mod configurable;
pub mod loader;
pub mod template;
mod jsonpath;

pub use openai::OpenAiProtocol;
pub use anthropic::AnthropicProtocol;
pub use gemini::GeminiProtocol;
pub use definition::ProtocolDefinition;
pub use registry::{ProtocolRegistry, PROTOCOL_REGISTRY};
pub use configurable::ConfigurableProtocol;
pub use loader::ProtocolLoader;
pub use template::{TemplateContext, TemplateRenderer};
pub use jsonpath::extract_value;
