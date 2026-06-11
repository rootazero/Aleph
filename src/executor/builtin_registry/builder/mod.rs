//! Builder / constructor submodules for `BuiltinToolRegistry`
//!
//! Split from the monolithic builder.rs to keep file sizes manageable.

// Re-export so submodules can access via `super::`
pub use crate::executor::builtin_registry::{BuiltinToolConfig, BuiltinToolRegistry};

mod constructor;
mod core_tools;
mod optional_tools;

#[cfg(test)]
mod tests;
