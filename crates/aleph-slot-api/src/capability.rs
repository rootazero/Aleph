//! Pure capability identity and service-graph relations shared by every host.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A namespaced capability identifier, e.g. `aleph/tools/<name>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityId {
    pub namespace: String,
    pub name: String,
}

/// How one capability relates to another in the service graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServiceRelation {
    Requires,
    Provides,
    Conflicts,
}

/// A pure relation edge; it carries no payload and is never invocable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Service {
    pub relation: ServiceRelation,
    pub target: CapabilityId,
}
