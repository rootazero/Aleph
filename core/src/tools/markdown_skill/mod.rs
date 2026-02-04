//! Markdown Skill System
//!
//! Runtime-loadable CLI tools defined in Markdown (SKILL.md format).
//! Compatible with OpenClaw ecosystem while adding Aether-specific extensions.

mod auto_loader;
mod executor;
mod generator;
mod loader;
mod parser;
mod spec;
mod tool_adapter;
mod watcher;

pub use auto_loader::{BatchLoadResult, EvolutionAutoLoader};
pub use generator::{MarkdownSkillGenerator, MarkdownSkillGeneratorConfig};
pub use loader::{load_skills_from_dir, SkillLoader};
pub use spec::{
    AetherExtensions, AetherSkillSpec, ConfirmationMode, DockerConfig, EvolutionMeta, InputHint,
    NetworkMode, RequiresSpec, SandboxMode, SecuritySpec, SkillMetadata,
};
pub use tool_adapter::{MarkdownCliTool, MarkdownToolOutput};
pub use watcher::{ReloadCallback, SkillEvent, SkillWatcher, SkillWatcherConfig};
