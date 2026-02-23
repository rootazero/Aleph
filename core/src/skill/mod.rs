//! Skill System v2 — Domain-Driven Skill Management
//!
//! This module provides the runtime infrastructure for skill registration,
//! eligibility evaluation, SKILL.md parsing, and prompt injection.

pub mod eligibility;
pub mod manifest;
pub mod registry;

pub use eligibility::{EligibilityResult, EligibilityService, IneligibilityReason};
pub use manifest::{parse_skill_content, parse_skill_file, SkillParseError};
pub use registry::SkillRegistry;
