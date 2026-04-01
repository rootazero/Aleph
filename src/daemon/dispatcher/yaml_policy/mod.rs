//! YAML-based Policy System

pub mod loader;
pub mod schema;
#[allow(clippy::module_inception)]
pub mod yaml_policy;

pub use loader::load_yaml_policies;
pub use schema::{Action, Condition, RiskLevel, Trigger, YamlRule};
pub use yaml_policy::YamlPolicy;
