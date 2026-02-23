//! YAML-based Policy System

pub mod schema;
#[allow(clippy::module_inception)]
pub mod yaml_policy;
pub mod loader;

pub use schema::{YamlRule, Trigger, Condition, Action, RiskLevel};
pub use yaml_policy::YamlPolicy;
pub use loader::load_yaml_policies;
