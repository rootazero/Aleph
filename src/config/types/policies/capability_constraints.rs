//! Startup-applied capability restrictions. Runtime facts remain source-owned.

use crate::capability::constraints::{Constraint, ConstraintSet};
use crate::capability::descriptor::CapabilityId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

pub type CapabilityConstraintsConfig = BTreeMap<String, CapabilityConstraintRule>;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilityConstraintRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<TimeWindowConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<ResourceLimitConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub required_conditions: BTreeMap<String, bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimeWindowConfig {
    pub start_hour: u8,
    pub end_hour: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimitConfig {
    pub unit: ResourceUnit,
    pub max_active: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResourceUnit {
    ActiveCalls,
    PtySessions,
}

pub(crate) struct CompiledConstraintRule {
    pub constraints: ConstraintSet,
    pub resource_unit: Option<ResourceUnit>,
    pub required_conditions: Vec<String>,
}

/// The sole parsing/validation boundary, shared by config validation and startup.
/// Unknown, well-formed targets stay pending until their source attaches.
pub(crate) fn compile_constraints(
    config: &CapabilityConstraintsConfig,
) -> Result<HashMap<CapabilityId, CompiledConstraintRule>, &'static str> {
    config
        .iter()
        .map(|(key, rule)| {
            let (namespace, name) = key.rsplit_once('/').ok_or("invalid_capability_id")?;
            if namespace.split('/').any(str::is_empty)
                || name.is_empty()
                || key.chars().any(|c| c.is_whitespace() || c.is_control())
            {
                return Err("invalid_capability_id");
            }
            let mut constraints = Vec::new();
            if let Some(time) = rule.time {
                constraints.push(
                    Constraint::time_window(time.start_hour, time.end_hour)
                        .map_err(|_| "invalid_time_window")?,
                );
            }
            if let Some(resource) = rule.resource {
                if resource.max_active == 0 {
                    return Err("invalid_resource_limit");
                }
                constraints.push(Constraint::resource_limit(resource.max_active));
            }
            constraints.extend(
                rule.required_conditions
                    .iter()
                    .map(|(name, expected)| Constraint::conditional(name.clone(), *expected)),
            );
            Ok((
                CapabilityId {
                    namespace: namespace.into(),
                    name: name.into(),
                },
                CompiledConstraintRule {
                    constraints: ConstraintSet::new(constraints),
                    resource_unit: rule.resource.map(|resource| resource.unit),
                    required_conditions: rule.required_conditions.keys().cloned().collect(),
                },
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::ownership::OwnershipTree;
    use crate::capability::policy::PolicyEngine;
    use crate::config::types::policies::PoliciesConfig;
    use std::sync::Arc;

    #[test]
    fn typed_rules_round_trip_and_preserve_late_target() {
        let text = r#"
[capability_constraints."plugin/vendor/future".time]
start_hour = 22
end_hour = 6
[capability_constraints."plugin/vendor/future".resource]
unit = "active_calls"
max_active = 2
[capability_constraints."plugin/vendor/future".required_conditions]
provider_ready = true
"#;
        let config: PoliciesConfig = toml::from_str(text).unwrap();
        let encoded = toml::to_string(&config).unwrap();
        let decoded: PoliciesConfig = toml::from_str(&encoded).unwrap();
        assert_eq!(
            config.capability_constraints,
            decoded.capability_constraints
        );
        let policy = PolicyEngine::with_shared_tree(
            Arc::new(OwnershipTree::new()),
            decoded.capability_constraints,
        )
        .unwrap();
        let id = crate::capability::descriptor::CapabilityId {
            namespace: "plugin/vendor".into(),
            name: "future".into(),
        };
        assert!(policy.is_configured(&id));
    }

    #[test]
    fn invalid_id_time_and_zero_limit_fail_at_same_compile_boundary() {
        for text in [
            "[\"no_separator\"]",
            "[\"/empty_namespace\"]",
            "[\"aleph/tools/\"]",
            "[\"aleph//invalid\"]",
            "[\"aleph/tools/bad name\"]",
            "[\"aleph/tools/ui\".time]\nstart_hour=24\nend_hour=6",
            "[\"aleph/tools/ui\".time]\nstart_hour=9\nend_hour=9",
            "[\"aleph/tools/ui\".resource]\nunit='active_calls'\nmax_active=0",
        ] {
            let rules: CapabilityConstraintsConfig = toml::from_str(text).unwrap();
            assert!(compile_constraints(&rules).is_err(), "{text}");
            assert!(
                PolicyEngine::with_shared_tree(Arc::new(OwnershipTree::new()), rules.clone())
                    .is_err()
            );
            let mut config = crate::config::Config::default();
            config.policies.capability_constraints = rules;
            assert!(
                config.validate().is_err(),
                "Config::validate must wire the same boundary: {text}"
            );
        }
    }

    #[test]
    fn empty_defaults_do_not_change_serialized_policies() {
        let config: PoliciesConfig = toml::from_str("").unwrap();
        assert!(config.capability_constraints.is_empty());
        assert!(!toml::to_string(&config)
            .unwrap()
            .contains("capability_constraints"));
        assert!(PolicyEngine::with_shared_tree(
            Arc::new(OwnershipTree::new()),
            CapabilityConstraintsConfig::new()
        )
        .is_ok());
    }
}
