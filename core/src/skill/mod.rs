//! Skill System v2 — Domain-Driven Skill Management
//!
//! This module provides the runtime infrastructure for skill registration,
//! eligibility evaluation, SKILL.md parsing, and prompt injection.

pub mod registry;

pub use registry::SkillRegistry;
