//! Skill System v2 — Domain-Driven Skill Management
//!
//! This module provides the runtime infrastructure for skill registration,
//! eligibility evaluation, SKILL.md parsing, and prompt injection.

pub mod commands;
pub mod eligibility;
pub mod installer;
pub mod manifest;
pub mod prompt;
pub mod registry;
pub mod snapshot;
pub mod status;

pub use commands::{list_available_commands, resolve_command, SkillCommandSpec};
pub use eligibility::{EligibilityResult, EligibilityService, IneligibilityReason};
pub use installer::{build_install_command, filter_install_specs_for_current_os};
pub use manifest::{parse_skill_content, parse_skill_file, SkillParseError};
pub use prompt::build_skills_prompt_xml;
pub use registry::SkillRegistry;
pub use snapshot::SkillSnapshot;
pub use status::SkillStatusReport;
