//! Concrete ACP harness adapters for supported CLI tools.

mod claude_code;
mod codex;
mod gemini;

pub use claude_code::ClaudeCodeHarness;
pub use codex::CodexHarness;
pub use gemini::GeminiHarness;
