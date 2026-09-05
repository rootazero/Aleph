//! Markdown Skill System
//!
//! Runtime-loadable CLI tools defined in Markdown (SKILL.md format).
//! Compatible with `OpenClaw` ecosystem while adding Aleph-specific extensions.
//!
//! Phase 1 of skill data model unification deprecates `AlephSkillSpec` in
//! favor of `crate::domain::skill::SkillManifest`; the module itself remains
//! the only legitimate consumer until Phase 2 (≥2026-06-03) absorbs the
//! types. See docs/superpowers/specs/2026-05-20-skill-data-model-unification-design.md.
#![allow(deprecated)]

mod executor;
mod loader;
mod parser;
mod spec;
mod tool_adapter;
mod watcher;

pub use loader::{load_skills_from_dir, SkillLoadReport, SkillLoader};
pub use spec::{
    AlephExtensions, AlephSkillSpec, ConfirmationMode, DockerConfig, EvolutionMeta, InputHint,
    NetworkMode, RequiresSpec, SandboxMode, SecuritySpec, SkillMetadata,
};
pub use tool_adapter::{MarkdownCliTool, MarkdownToolOutput};
pub use watcher::{ReloadCallback, SkillEvent, SkillWatcher, SkillWatcherConfig};
