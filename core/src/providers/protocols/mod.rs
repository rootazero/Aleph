//! Protocol implementations for different AI APIs
//!
//! Each protocol handles the specific request/response format for an API family.

pub mod openai;

pub use openai::OpenAiProtocol;
