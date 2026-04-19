//! LayeredPermissionResolver — translates merged two-tier permissions
//! (global + per-agent, most-restrictive-wins) into SmartFilter::Classification.
//!
//! Wraps the existing `ToolPermissionsConfig` + `PermissionAction`
//! (`Allow` / `Ask` / `Deny`). The merged config is stored in an `ArcSwap`
//! so live-reload of permissions (from config RPC handlers) is possible
//! without swapping the whole filter.
//!
//! Phase 3 Task 2: Replaces the Phase 2 placeholder `SmartFilter` with a
//! concrete policy-backed implementation.

use std::sync::Arc;

use arc_swap::ArcSwap;
use serde_json::Value;

use crate::config::types::policies::tool_permissions::ToolPermissionsConfig;
use crate::extension::PermissionAction;

use super::{Classification, SmartFilter};

/// Tools whose approval is owned downstream by WorkspaceSandbox.
/// PermissionLayer must NOT call ApprovalGate for these — doing so
/// would produce a duplicate prompt for the same action.
const EXEC_CLASS_TOOLS: &[&str] = &["code_exec", "bash"];

/// Merges global + per-agent permissions; classifies each tool call.
///
/// Uses `ToolPermissionsConfig::merge` (with `restrictive_min`) so that
/// Deny > Ask > Allow. The final `resolve(tool_name)` consults overrides
/// first and falls back to the merged default.
pub struct LayeredPermissionResolver {
    merged: Arc<ArcSwap<ToolPermissionsConfig>>,
}

impl LayeredPermissionResolver {
    /// Build a resolver from an already-merged config.
    pub fn from_merged(merged: ToolPermissionsConfig) -> Self {
        Self {
            merged: Arc::new(ArcSwap::from_pointee(merged)),
        }
    }

    /// Build a resolver by merging global + per-agent layers at construction
    /// time. The result is equivalent to calling `ToolPermissionsConfig::merge`
    /// and handing that to [`Self::from_merged`].
    pub fn from_layers(global: &ToolPermissionsConfig, agent: &ToolPermissionsConfig) -> Self {
        Self::from_merged(ToolPermissionsConfig::merge(global, agent))
    }

    /// Atomically swap the merged config. Used by live-reload paths when
    /// the global or per-agent config changes at runtime.
    pub fn swap(&self, new_merged: ToolPermissionsConfig) {
        self.merged.store(Arc::new(new_merged));
    }

    /// Returns true if `tool_name` is an exec-class tool whose approval is
    /// owned by `WorkspaceSandbox`. `PermissionLayer` must not prompt for
    /// these tools — doing so would produce a duplicate approval dialog.
    pub fn is_exec_class(&self, tool_name: &str) -> bool {
        EXEC_CLASS_TOOLS.contains(&tool_name)
    }
}

impl SmartFilter for LayeredPermissionResolver {
    fn classify(&self, name: &str, _input: &Value) -> Classification {
        let snap = self.merged.load();
        match snap.resolve(name) {
            PermissionAction::Allow => Classification::Allow,
            PermissionAction::Ask => {
                // Exec-class tools (code_exec, bash) own their own approval gate
                // inside WorkspaceSandbox. Prompting here too would produce a
                // duplicate dialog for the same action — let them through.
                if self.is_exec_class(name) {
                    Classification::Allow
                } else {
                    Classification::Confirm {
                        reason: format!("Tool '{}' requires confirmation per policy", name),
                    }
                }
            }
            PermissionAction::Deny => Classification::Deny {
                reason: format!("Tool '{}' denied by policy", name),
            },
        }
    }
}

#[cfg(test)]
mod exec_class_tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn is_exec_class_recognizes_exec_tools() {
        let resolver = LayeredPermissionResolver::from_merged(Default::default());
        assert!(resolver.is_exec_class("code_exec"));
        assert!(resolver.is_exec_class("bash"));
    }

    #[test]
    fn is_exec_class_rejects_read_only_tools() {
        let resolver = LayeredPermissionResolver::from_merged(Default::default());
        assert!(!resolver.is_exec_class("read_file"));
        assert!(!resolver.is_exec_class("list_dir"));
    }

    /// Exec-class tools under Ask policy must classify as Allow, not Confirm.
    /// This prevents PermissionLayer from issuing a duplicate approval prompt
    /// for tools that already prompt via WorkspaceSandbox.
    #[test]
    fn exec_class_under_ask_policy_classifies_as_allow() {
        use serde_json::Value;

        let mut overrides = HashMap::new();
        overrides.insert("code_exec".to_string(), PermissionAction::Ask);
        overrides.insert("bash".to_string(), PermissionAction::Ask);
        let config = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides,
        };
        let resolver = LayeredPermissionResolver::from_merged(config);

        assert_eq!(
            resolver.classify("code_exec", &Value::Null),
            Classification::Allow,
            "code_exec under Ask must classify Allow (approval owned by WorkspaceSandbox)"
        );
        assert_eq!(
            resolver.classify("bash", &Value::Null),
            Classification::Allow,
            "bash under Ask must classify Allow (approval owned by WorkspaceSandbox)"
        );
    }

    /// Non-exec tools under Ask policy must still classify as Confirm.
    #[test]
    fn non_exec_under_ask_policy_classifies_as_confirm() {
        use serde_json::Value;

        let mut overrides = HashMap::new();
        overrides.insert("web_fetch".to_string(), PermissionAction::Ask);
        let config = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides,
        };
        let resolver = LayeredPermissionResolver::from_merged(config);

        assert_eq!(
            resolver.classify("web_fetch", &Value::Null),
            Classification::Confirm {
                reason: "Tool 'web_fetch' requires confirmation per policy".to_string()
            },
            "non-exec tool under Ask must still produce Confirm"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Build a single-override config. Default is `Allow` unless overridden.
    fn cfg(
        default: PermissionAction,
        tool: &str,
        action: PermissionAction,
    ) -> ToolPermissionsConfig {
        let mut overrides = HashMap::new();
        overrides.insert(tool.to_string(), action);
        ToolPermissionsConfig { default, overrides }
    }

    /// Convert a `PermissionAction` into its expected `Classification`
    /// variant for a given tool name. The reason strings must match the
    /// formatting used by `LayeredPermissionResolver::classify`.
    fn expected(action: PermissionAction, tool: &str) -> Classification {
        match action {
            PermissionAction::Allow => Classification::Allow,
            PermissionAction::Ask => Classification::Confirm {
                reason: format!("Tool '{}' requires confirmation per policy", tool),
            },
            PermissionAction::Deny => Classification::Deny {
                reason: format!("Tool '{}' denied by policy", tool),
            },
        }
    }

    /// The 3×3 effective-trust matrix. For every (global, agent) pair,
    /// the merged classification must equal `restrictive_min(global, agent)`.
    #[test]
    fn effective_trust_matrix_3x3() {
        use PermissionAction::*;
        let all = [Allow, Ask, Deny];

        for g in all {
            for a in all {
                let global = cfg(Allow, "t", g);
                let agent = cfg(Allow, "t", a);
                let resolver = LayeredPermissionResolver::from_layers(&global, &agent);

                let expected_action = match (g, a) {
                    (Deny, _) | (_, Deny) => Deny,
                    (Ask, _) | (_, Ask) => Ask,
                    _ => Allow,
                };
                assert_eq!(
                    resolver.classify("t", &Value::Null),
                    expected(expected_action, "t"),
                    "merge({g:?}, {a:?}) should classify as {expected_action:?}",
                );
            }
        }
    }

    /// When no override exists, classification falls back to the merged
    /// default (which itself is `restrictive_min(global.default, agent.default)`).
    #[test]
    fn default_fallback_uses_merged_default() {
        use PermissionAction::*;

        // Global default Allow, agent default Ask → merged default Ask.
        let global = ToolPermissionsConfig {
            default: Allow,
            overrides: HashMap::new(),
        };
        let agent = ToolPermissionsConfig {
            default: Ask,
            overrides: HashMap::new(),
        };
        let resolver = LayeredPermissionResolver::from_layers(&global, &agent);
        assert_eq!(
            resolver.classify("unknown_tool", &Value::Null),
            expected(Ask, "unknown_tool"),
        );

        // Global default Allow, agent default Deny → merged default Deny.
        let agent_deny = ToolPermissionsConfig {
            default: Deny,
            overrides: HashMap::new(),
        };
        let resolver2 = LayeredPermissionResolver::from_layers(&global, &agent_deny);
        assert_eq!(
            resolver2.classify("anything", &Value::Null),
            expected(Deny, "anything"),
        );
    }

    /// An override should win over the default on the same layer, and the
    /// merge step should still apply `restrictive_min` across layers.
    #[test]
    fn override_beats_default_then_merge_applies() {
        use PermissionAction::*;

        // Global default Allow + agent override Ask on tool "t" → Ask.
        let global = ToolPermissionsConfig {
            default: Allow,
            overrides: HashMap::new(),
        };
        let mut agent_overrides = HashMap::new();
        agent_overrides.insert("t".to_string(), Ask);
        let agent = ToolPermissionsConfig {
            default: Allow,
            overrides: agent_overrides,
        };
        let resolver = LayeredPermissionResolver::from_layers(&global, &agent);
        assert_eq!(resolver.classify("t", &Value::Null), expected(Ask, "t"));
        // Untouched tool still follows the merged default (Allow).
        assert_eq!(
            resolver.classify("other", &Value::Null),
            expected(Allow, "other"),
        );
    }

    /// `swap` must make new classifications reflect the replaced config.
    #[test]
    fn swap_replaces_merged_config() {
        use PermissionAction::*;

        let resolver = LayeredPermissionResolver::from_merged(ToolPermissionsConfig {
            default: Allow,
            overrides: HashMap::new(),
        });
        assert_eq!(resolver.classify("t", &Value::Null), expected(Allow, "t"));

        resolver.swap(ToolPermissionsConfig {
            default: Deny,
            overrides: HashMap::new(),
        });
        assert_eq!(resolver.classify("t", &Value::Null), expected(Deny, "t"));
    }
}
