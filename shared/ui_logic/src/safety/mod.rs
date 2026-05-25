//! Client-side safety heuristics shared by all UI surfaces (panel, TUI, mobile).
//!
//! These are *soft* checks — final authority lives in the server-side
//! security pipeline. They exist so a panel can warn the user (or refuse
//! to send) before a round-trip wastes a model call.

pub mod prompt_injection;

pub use prompt_injection::{
    check_prompt_injection, prompt_guard_message, PromptInjectionCheck, PromptInjectionReason,
    PromptInjectionVerdict,
};
