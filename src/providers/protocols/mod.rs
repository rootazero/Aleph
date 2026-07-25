//! Protocol implementations for different AI APIs
//!
//! Each protocol handles the specific request/response format for an API family.

pub mod anthropic;
pub mod configurable;
pub mod definition;
pub mod gemini;
pub(crate) mod http_client;
mod jsonpath;
pub mod loader;
pub mod openai_chat;
pub mod openai_common;
pub mod openai_responses;
pub mod registry;
pub(crate) mod stream_idle;
pub mod template;

pub use anthropic::AnthropicProtocol;
pub use configurable::ConfigurableProtocol;
pub use definition::ProtocolDefinition;
pub use gemini::GeminiProtocol;
pub use jsonpath::extract_value;
pub use loader::ProtocolLoader;
pub use openai_chat::OpenAiProtocol;
pub use openai_responses::OpenAiResponsesProtocol;
pub use registry::{ProtocolRegistry, PROTOCOL_REGISTRY};
pub use template::{TemplateContext, TemplateRenderer};
