//! Tool permission configuration for global and per-agent permission levels.
//!
//! Provides a two-layer permission system: Global (Policies) + Agent-level.
//! Each tool can be set to Allow, Ask (needs confirmation), or Deny.
//! When merging global and agent permissions, the most restrictive level wins.
//!
//! Override keys may be **exact tool names** or **glob patterns** (`*`, `?`),
//! reusing the same matcher as the action-type approval layer
//! ([`crate::approval::matches_glob`]). Exact names always win over patterns
//! (specific > general); when several patterns match one tool, the most
//! restrictive of them wins — consistent with the module-wide merge philosophy.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::extension::PermissionAction;

/// Tool permissions configuration.
///
/// Controls per-tool permission levels with a default fallback.
/// Used at both the global policy level and the per-agent level.
///
/// # Example TOML
/// ```toml
/// [policies.tool_permissions]
/// default = "allow"
///
/// [policies.tool_permissions.overrides]
/// shell = "ask"          # exact tool name
/// file_delete = "deny"   # exact tool name
/// "github__*" = "ask"    # glob: every tool of the `github` MCP server
///                        # (MCP tools are registered as `{server_id}__{tool}`)
/// "*_delete" = "deny"    # glob: any tool whose name ends in `_delete`
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolPermissionsConfig {
    /// Default permission for tools not listed in overrides
    #[serde(default = "default_allow")]
    pub default: PermissionAction,

    /// Per-tool permission overrides
    #[serde(default)]
    pub overrides: HashMap<String, PermissionAction>,
}

const fn default_allow() -> PermissionAction {
    PermissionAction::Allow
}

impl Default for ToolPermissionsConfig {
    fn default() -> Self {
        Self {
            default: PermissionAction::Allow,
            overrides: HashMap::new(),
        }
    }
}

/// Which override entry decided a tool's permission, and what it decided.
///
/// Returned by [`ToolPermissionsConfig::explain`]. `pattern` borrows the key as
/// written in the config — an exact tool name or a glob — so a surface can quote
/// it back verbatim ("`*` = deny") instead of paraphrasing a config the operator
/// wrote themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionMatch<'a> {
    /// The permission the entry states.
    pub action: PermissionAction,
    /// The override key that matched, exactly as written in the config.
    pub pattern: &'a str,
    /// `true` when `pattern` is the tool's exact name rather than a glob. The
    /// exec tier's argument-level filter stands down only for an exact entry
    /// (see `ScopedToolService::explicitly_named`), so the distinction is
    /// load-bearing, not cosmetic.
    pub exact: bool,
}

impl ToolPermissionsConfig {
    /// Resolve the effective permission for a tool.
    ///
    /// Precedence (most specific first):
    /// 1. **Exact** override for `tool_name` — the fast path.
    /// 2. **Glob** overrides (keys containing `*`/`?`) matched against
    ///    `tool_name`. When more than one pattern matches, the most restrictive
    ///    (`Deny > Ask > Allow`) wins, mirroring [`Self::merge`].
    /// 3. The configured `default`.
    ///
    /// Exact entries always take precedence over patterns, so a config can
    /// carve out specific allowances inside a broad pattern (e.g. `"github__*" =
    /// "ask"` plus `github__list_issues = "allow"`).
    pub fn resolve(&self, tool_name: &str) -> PermissionAction {
        self.resolve_explicit(tool_name).unwrap_or(self.default)
    }

    /// The permission an override entry *explicitly* states for `tool_name`
    /// (steps 1–2 of [`Self::resolve`]), or `None` when no entry names it.
    ///
    /// Callers that layer another rule between the overrides and the `default`
    /// — the exec tier at
    /// [`crate::tools::scoped::ScopedToolService::permission_for`] — need to
    /// tell "the operator decided Allow" apart from "nobody said anything and
    /// the default is Allow".
    ///
    /// Delegates to [`Self::explain`] so "what did the operator decide" and
    /// "which entry decided it" can never answer differently.
    pub fn resolve_explicit(&self, tool_name: &str) -> Option<PermissionAction> {
        self.explain(tool_name).map(|m| m.action)
    }

    /// [`Self::resolve_explicit`] plus the override key that produced the
    /// answer — the fact an approval card needs so the human can tell what to
    /// edit.
    ///
    /// A gate that says "this tool needs your confirmation" and stops there
    /// teaches the reader to guess, and the two guesses this repo actually sees
    /// are both wrong somewhere: "set it to allow and it will stop asking"
    /// (false whenever the tool declares its own gate) and "I never configured
    /// this" (false whenever a glob caught it). Naming the entry answers both.
    ///
    /// # Determinism
    ///
    /// `overrides` is a `HashMap`, so the glob scan visits patterns in an
    /// arbitrary order. The *action* was always deterministic
    /// ([`restrictive_min`] is commutative and associative) but the winning
    /// *pattern* is not, and a reason string that changes between runs is a
    /// reason string nobody trusts. Equally restrictive matches therefore break
    /// the tie on the lexicographically smallest key.
    pub fn explain(&self, tool_name: &str) -> Option<PermissionMatch<'_>> {
        // 1. Exact override — fast path and most specific.
        if let Some((pattern, action)) = self.overrides.get_key_value(tool_name) {
            return Some(PermissionMatch {
                action: *action,
                pattern,
                exact: true,
            });
        }
        // 2. Glob-pattern overrides; most restrictive match wins, ties on key.
        let mut matched: Option<PermissionMatch<'_>> = None;
        for (pattern, action) in &self.overrides {
            if !is_glob_pattern(pattern) || !crate::approval::matches_glob(tool_name, pattern) {
                continue;
            }
            let candidate = PermissionMatch {
                action: *action,
                pattern,
                exact: false,
            };
            matched = Some(match matched {
                // Strictly more restrictive wins outright; equally restrictive
                // breaks on the smallest key so the answer does not depend on
                // `HashMap` iteration order.
                Some(prev)
                    if restrictive_min(prev.action, candidate.action) != prev.action
                        || (prev.action == candidate.action
                            && candidate.pattern < prev.pattern) =>
                {
                    candidate
                }
                Some(prev) => prev,
                None => candidate,
            });
        }
        matched
    }

    /// Merge global and agent-level permissions into an effective config.
    ///
    /// For each tool, the most restrictive permission wins:
    /// Deny > Ask > Allow (i.e., `min(global, agent)`).
    ///
    /// The effective default is `min(global.default, agent.default)`.
    /// Overrides are merged: if both layers specify a tool, the most
    /// restrictive level is used.
    ///
    /// Every key named by either layer is kept in the merged overrides, even
    /// when its merged value equals the merged default. Dropping those entries
    /// (a former "compression") erased two things resolution depends on:
    /// an exact-name carve-out inside a same-layer glob family (the entry
    /// vanished, so the glob re-captured the tool), and the *explicitness* of
    /// an `allow` — [`resolve_explicit`](Self::resolve_explicit) could no
    /// longer tell "the operator decided Allow" from silence, so the exec tier
    /// tightened tools the operator had deliberately named.
    pub fn merge(global: &Self, agent: &Self) -> Self {
        let default = restrictive_min(global.default, agent.default);

        let mut overrides = HashMap::new();

        // Collect all tool names from both layers
        let all_keys: std::collections::HashSet<&String> = global
            .overrides
            .keys()
            .chain(agent.overrides.keys())
            .collect();

        for key in all_keys {
            let global_perm = global.resolve(key);
            let agent_perm = agent.resolve(key);
            let effective = restrictive_min(global_perm, agent_perm);
            overrides.insert(key.clone(), effective);
        }

        Self { default, overrides }
    }
}

/// Whether an override key is a glob pattern rather than an exact tool name.
///
/// Mirrors the metacharacters honored by [`crate::approval::matches_glob`]
/// (`*`, `?`). Real tool names never contain these, so any key carrying one is
/// unambiguously a pattern.
fn is_glob_pattern(key: &str) -> bool {
    key.contains('*') || key.contains('?')
}

/// Return the more restrictive of two permission actions.
///
/// Ordering: Deny (most restrictive) > Ask > Allow (least restrictive). The one
/// restrictiveness lattice in the codebase: [`ToolPermissionsConfig::merge`],
/// glob-override resolution, and the exec tier's tightening rule
/// ([`super::exec_tier::effective_permission`]) all order permissions with it,
/// so a layer can only ever tighten what a layer below it decided.
pub(super) const fn restrictive_min(a: PermissionAction, b: PermissionAction) -> PermissionAction {
    use PermissionAction::*;
    match (a, b) {
        (Deny, _) | (_, Deny) => Deny,
        (Ask, _) | (_, Ask) => Ask,
        _ => Allow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::PermissionAction;

    #[test]
    fn test_default_config() {
        let config = ToolPermissionsConfig::default();
        assert_eq!(config.default, PermissionAction::Allow);
        assert!(config.overrides.is_empty());
    }

    #[test]
    fn test_resolve_with_override() {
        let mut config = ToolPermissionsConfig::default();
        config
            .overrides
            .insert("shell".to_string(), PermissionAction::Ask);

        assert_eq!(config.resolve("shell"), PermissionAction::Ask);
        assert_eq!(config.resolve("read_file"), PermissionAction::Allow);
    }

    #[test]
    fn test_resolve_explicit_distinguishes_silence_from_allow() {
        let config = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("bash".to_string(), PermissionAction::Allow)]
                .into_iter()
                .collect(),
        };
        // Named by the operator → Allow is a decision.
        assert_eq!(
            config.resolve_explicit("bash"),
            Some(PermissionAction::Allow)
        );
        // Nobody named it → the tier gets to speak before the default does.
        assert_eq!(config.resolve_explicit("file_write"), None);
        assert_eq!(config.resolve("file_write"), PermissionAction::Allow);
    }

    #[test]
    fn test_resolve_uses_default() {
        let config = ToolPermissionsConfig {
            default: PermissionAction::Deny,
            overrides: HashMap::new(),
        };
        assert_eq!(config.resolve("anything"), PermissionAction::Deny);
    }

    #[test]
    fn test_glob_override_matches_family() {
        let config = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("github__*".to_string(), PermissionAction::Ask)]
                .into_iter()
                .collect(),
        };
        assert_eq!(
            config.resolve("github__create_issue"),
            PermissionAction::Ask
        );
        assert_eq!(config.resolve("github__delete_repo"), PermissionAction::Ask);
        // Non-matching tools — including another server's tools — fall back to
        // the default. The pattern is scoped to one server, as its key says.
        assert_eq!(
            config.resolve("slack__send_message"),
            PermissionAction::Allow
        );
        assert_eq!(config.resolve("read_file"), PermissionAction::Allow);
    }

    /// `explain` says WHICH entry decided, and marks whether it was the tool's
    /// own name. The `exact` bit is load-bearing: the exec tier's
    /// argument-level filter stands down only for an exact entry.
    #[test]
    fn explain_names_the_entry_and_whether_it_was_exact() {
        let config = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [
                ("github__*".to_string(), PermissionAction::Ask),
                ("file_ops".to_string(), PermissionAction::Ask),
            ]
            .into_iter()
            .collect(),
        };

        let exact = config.explain("file_ops").expect("named exactly");
        assert_eq!(exact.pattern, "file_ops");
        assert!(exact.exact);

        let glob = config.explain("github__delete_repo").expect("glob match");
        assert_eq!(glob.pattern, "github__*");
        assert!(!glob.exact);

        assert_eq!(config.explain("read_file"), None, "silence stays silence");
    }

    /// The winning PATTERN must be stable across runs. `overrides` is a
    /// `HashMap`, so the scan order is arbitrary; the action was always
    /// deterministic (`restrictive_min` is commutative) but the pattern quoted
    /// back to the user was not, and a card that names a different entry each
    /// time it renders is worse than one that names none.
    #[test]
    fn explain_breaks_ties_deterministically_across_iteration_orders() {
        let config = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [
                ("zzz_*".to_string(), PermissionAction::Ask),
                ("aaa*".to_string(), PermissionAction::Ask),
                ("a*".to_string(), PermissionAction::Ask),
            ]
            .into_iter()
            .collect(),
        };
        // Rebuild the map repeatedly: each `HashMap` gets its own random seed
        // for the SipHash keys, so this really does resample the scan order.
        for _ in 0..32 {
            let shuffled = ToolPermissionsConfig {
                default: config.default,
                overrides: config.overrides.clone().into_iter().collect(),
            };
            assert_eq!(
                shuffled.explain("aaa_tool").map(|m| m.pattern),
                Some("a*"),
                "equally restrictive matches must resolve to the same pattern"
            );
        }
    }

    /// A more restrictive pattern still wins outright — the tie-break only
    /// arbitrates between equals, it does not reorder the lattice.
    #[test]
    fn explain_prefers_restrictiveness_over_the_tie_break_key() {
        let config = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [
                ("a*".to_string(), PermissionAction::Ask),
                ("az*".to_string(), PermissionAction::Deny),
            ]
            .into_iter()
            .collect(),
        };
        // Both patterns match `az_tool`, and the denying one sorts LATER, so a
        // key-first rule would have picked the wrong (weaker) answer.
        let m = config.explain("az_tool").expect("both match");
        assert_eq!(m.action, PermissionAction::Deny);
        assert_eq!(m.pattern, "az*");
    }

    #[test]
    fn test_exact_override_beats_glob() {
        // Broad pattern asks, but a specific tool is explicitly allowed.
        let config = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [
                ("github__*".to_string(), PermissionAction::Ask),
                ("github__list_issues".to_string(), PermissionAction::Allow),
            ]
            .into_iter()
            .collect(),
        };
        assert_eq!(
            config.resolve("github__list_issues"),
            PermissionAction::Allow
        );
        assert_eq!(
            config.resolve("github__create_issue"),
            PermissionAction::Ask
        );
    }

    #[test]
    fn test_multiple_glob_matches_most_restrictive_wins() {
        // `file_delete` matches both patterns; Deny must win over Ask.
        let config = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [
                ("file_*".to_string(), PermissionAction::Ask),
                ("*_delete".to_string(), PermissionAction::Deny),
            ]
            .into_iter()
            .collect(),
        };
        assert_eq!(config.resolve("file_delete"), PermissionAction::Deny);
        // Only the Ask pattern matches here.
        assert_eq!(config.resolve("file_read"), PermissionAction::Ask);
    }

    #[test]
    fn test_star_glob_overrides_all() {
        let config = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("*".to_string(), PermissionAction::Deny)]
                .into_iter()
                .collect(),
        };
        assert_eq!(config.resolve("anything"), PermissionAction::Deny);
        assert_eq!(config.resolve("read_file"), PermissionAction::Deny);
    }

    #[test]
    fn test_merge_composes_with_glob() {
        // Global denies an MCP family via glob; agent tries to allow one member
        // exactly. Most-restrictive-wins must keep it denied after merge.
        let global = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("github__*".to_string(), PermissionAction::Deny)]
                .into_iter()
                .collect(),
        };
        let agent = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("github__create_issue".to_string(), PermissionAction::Allow)]
                .into_iter()
                .collect(),
        };
        let merged = ToolPermissionsConfig::merge(&global, &agent);
        assert_eq!(
            merged.resolve("github__create_issue"),
            PermissionAction::Deny
        );
        assert_eq!(
            merged.resolve("github__delete_repo"),
            PermissionAction::Deny
        );
        assert_eq!(merged.resolve("read_file"), PermissionAction::Allow);
    }

    #[test]
    fn test_merge_preserves_same_layer_exact_carveout() {
        // The documented carve-out shape — a broad glob `ask` with one exact
        // `allow` inside it — configured at ONE layer, merged with an
        // all-default other layer (which is what every turn does). The old
        // merge dropped the exact `allow` for equalling the merged default,
        // and the glob silently re-captured the tool.
        let global = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [
                ("github__*".to_string(), PermissionAction::Ask),
                ("github__list_issues".to_string(), PermissionAction::Allow),
            ]
            .into_iter()
            .collect(),
        };
        let merged = ToolPermissionsConfig::merge(&global, &ToolPermissionsConfig::default());
        assert_eq!(
            merged.resolve("github__list_issues"),
            PermissionAction::Allow,
            "exact carve-out must survive the merge"
        );
        assert_eq!(
            merged.resolve("github__create_issue"),
            PermissionAction::Ask,
            "the glob must still govern the rest of the family"
        );
    }

    #[test]
    fn test_merge_preserves_explicitness_of_allow() {
        // An explicit `bash = "allow"` equal to the default is still a
        // DECISION: `resolve_explicit` must keep returning `Some` after merge,
        // or the exec tier (which only speaks when nobody decided) would
        // tighten a tool the operator deliberately named.
        let global = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("bash".to_string(), PermissionAction::Allow)]
                .into_iter()
                .collect(),
        };
        let merged = ToolPermissionsConfig::merge(&global, &ToolPermissionsConfig::default());
        assert_eq!(
            merged.resolve_explicit("bash"),
            Some(PermissionAction::Allow),
            "merge must not erase the explicitness bit"
        );
        // Silence stays silence: an unnamed tool still has no explicit entry.
        assert_eq!(merged.resolve_explicit("file_write"), None);
    }

    #[test]
    fn test_merge_defaults_most_restrictive() {
        let global = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: HashMap::new(),
        };
        let agent = ToolPermissionsConfig {
            default: PermissionAction::Ask,
            overrides: HashMap::new(),
        };
        let merged = ToolPermissionsConfig::merge(&global, &agent);
        assert_eq!(merged.default, PermissionAction::Ask);
    }

    #[test]
    fn test_merge_deny_wins_over_allow() {
        let global = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("shell".to_string(), PermissionAction::Deny)]
                .into_iter()
                .collect(),
        };
        let agent = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("shell".to_string(), PermissionAction::Allow)]
                .into_iter()
                .collect(),
        };
        let merged = ToolPermissionsConfig::merge(&global, &agent);
        assert_eq!(merged.resolve("shell"), PermissionAction::Deny);
    }

    #[test]
    fn test_merge_agent_deny_overrides_global_allow() {
        let global = ToolPermissionsConfig::default();
        let agent = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("file_delete".to_string(), PermissionAction::Deny)]
                .into_iter()
                .collect(),
        };
        let merged = ToolPermissionsConfig::merge(&global, &agent);
        assert_eq!(merged.resolve("file_delete"), PermissionAction::Deny);
        assert_eq!(merged.resolve("read_file"), PermissionAction::Allow);
    }

    #[test]
    fn test_merge_both_overrides_combined() {
        let global = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("shell".to_string(), PermissionAction::Ask)]
                .into_iter()
                .collect(),
        };
        let agent = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("file_delete".to_string(), PermissionAction::Deny)]
                .into_iter()
                .collect(),
        };
        let merged = ToolPermissionsConfig::merge(&global, &agent);
        assert_eq!(merged.resolve("shell"), PermissionAction::Ask);
        assert_eq!(merged.resolve("file_delete"), PermissionAction::Deny);
        assert_eq!(merged.resolve("read_file"), PermissionAction::Allow);
    }

    #[test]
    fn test_merge_symmetric_deny() {
        // Deny from either side should produce Deny
        for (g, a) in [
            (PermissionAction::Deny, PermissionAction::Allow),
            (PermissionAction::Allow, PermissionAction::Deny),
            (PermissionAction::Ask, PermissionAction::Deny),
            (PermissionAction::Deny, PermissionAction::Ask),
        ] {
            let global = ToolPermissionsConfig {
                default: g,
                overrides: HashMap::new(),
            };
            let agent = ToolPermissionsConfig {
                default: a,
                overrides: HashMap::new(),
            };
            let merged = ToolPermissionsConfig::merge(&global, &agent);
            assert_eq!(
                merged.default,
                PermissionAction::Deny,
                "merge({:?}, {:?}) should be Deny",
                g,
                a
            );
        }
    }

    #[test]
    fn test_deserialize_empty_toml() {
        let config: ToolPermissionsConfig = toml::from_str("").unwrap();
        assert_eq!(config.default, PermissionAction::Allow);
        assert!(config.overrides.is_empty());
    }

    #[test]
    fn test_deserialize_with_overrides() {
        let toml_str = r#"
            default = "ask"

            [overrides]
            shell = "deny"
            read_file = "allow"
        "#;
        let config: ToolPermissionsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.default, PermissionAction::Ask);
        assert_eq!(config.overrides.get("shell"), Some(&PermissionAction::Deny));
        assert_eq!(
            config.overrides.get("read_file"),
            Some(&PermissionAction::Allow)
        );
    }

    #[test]
    fn test_serialization_roundtrip() {
        let config = ToolPermissionsConfig {
            default: PermissionAction::Ask,
            overrides: [
                ("shell".to_string(), PermissionAction::Deny),
                ("read_file".to_string(), PermissionAction::Allow),
            ]
            .into_iter()
            .collect(),
        };
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: ToolPermissionsConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.default, config.default);
        assert_eq!(parsed.overrides.len(), config.overrides.len());
    }
}
