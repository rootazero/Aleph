//! Section renderers for PromptBuilder.
//!
//! Each submodule exports a `render()` function that returns a `PromptSection`.

pub mod identity;
pub mod tone;
pub mod directives;
pub mod model_behavior;
pub mod system_rules;
pub mod doing_tasks;
pub mod actions;
pub mod tool_usage;
pub mod tone_and_style;
pub mod output_efficiency;
pub mod tools;
pub mod skills;
pub mod memory_protocol;
pub mod custom_instructions;
pub mod environment;
pub mod session_guidance;
pub mod memory;
pub mod discovered_skills;
