//! Extension system type definitions
//!
//! Core data structures for skills, commands, agents, and plugins.
//!
//! This module provides a unified namespace for all extension-related types.
//! The types are organized into submodules by domain, but re-exported here
//! to maintain backward compatibility with existing code.

// Submodule declarations
mod agents;
mod hooks;
mod plugins;
mod runtime;
mod skills;

// Flatten exports to maintain existing API surface
// This allows existing code to continue using:
//   use crate::extension::types::SkillMetadata;
//   use crate::extension::types::ExtensionAgent;
//   use crate::extension::types::PluginRecord;
// without needing to know about the internal module structure.

pub use agents::*;
pub use hooks::*;
pub use plugins::*;
pub use runtime::*;
pub use skills::*;
