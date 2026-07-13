//! Execution permission tier — the single user-facing dial over tool permissions.
//!
//! Three tiers (codex-style): **Ask** / **Auto** / **Full**. A tier is *not* a
//! second policy axis: it projects onto the existing
//! [`ToolPermissionsConfig`] (`default` + per-tool `overrides`), so there is
//! exactly one mechanism enforcing tool permissions. Explicit
//! `[policies.tool_permissions]` entries are merged over the tier preset by the
//! run loop, which is what makes a Panel "advanced overrides" section coherent.
//!
//! The tier axis is orthogonal to `[sandbox.command_policy]`. No tier — not
//! even `Full` — can lower the command-policy hardline floor (fork bombs,
//! `rm -rf /`, device wipes); see the unit test at the bottom of this file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::tool_permissions::ToolPermissionsConfig;
use crate::extension::PermissionAction;

/// Identity-metadata custom key under which a session's per-session tier
/// override is persisted (written through the existing `sessions.patch` RPC,
/// read per turn by the execution engine). Same carrier pattern as
/// `custom["project_root"]`.
pub const EXEC_TIER_SESSION_KEY: &str = "exec_tier";

/// Execution permission tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecTier {
    /// Every mutating / side-effecting tool needs human confirmation.
    /// Read-only tools stay allowed so the model can still investigate.
    Ask,
    /// Today's de-facto behavior plus a real guard on the destructive tail
    /// (deletions, credential writes, team destruction). The default: an
    /// unconfigured install behaves as it did before tiers existed, except
    /// irreversible operations now stop for a human.
    #[default]
    Auto,
    /// Nothing is asked. The command-policy hardline floor still applies.
    Full,
}

impl ExecTier {
    /// Parse a tier from its serialized id (`"ask"` / `"auto"` / `"full"`).
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "ask" => Some(Self::Ask),
            "auto" => Some(Self::Auto),
            "full" => Some(Self::Full),
            _ => None,
        }
    }

    /// Serialized id of this tier.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Auto => "auto",
            Self::Full => "full",
        }
    }

    /// Project the tier onto the tool-permission axis.
    ///
    /// Override keys are globs wherever a family exists (`*_delete`,
    /// `mcp__*`, …) — [`ToolPermissionsConfig::resolve`] matches them, so new
    /// tools joining a family are covered without touching this table.
    #[must_use]
    pub fn expand(self) -> ToolPermissionsConfig {
        let ask = |keys: &[&str]| ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: keys
                .iter()
                .map(|k| ((*k).to_string(), PermissionAction::Ask))
                .collect(),
        };
        match self {
            Self::Ask => ask(&[
                // Code / command execution.
                "bash",
                "code_exec",
                "apply_patch",
                "automation",
                "desktop",
                "desktop_browser_operator",
                // Filesystem mutation.
                "file_write",
                "file_edit",
                "file_ops",
                // Irreversible.
                "*_delete",
                "team_disband",
                // Credentials.
                "vault_*",
                // Control plane / mutating management.
                "*_manage",
                "*_create",
                "*_install",
                "self_config",
                // Remote execution on cluster nodes.
                "node_invoke",
                "node_invoke_many",
                "node_file",
                // Outbound side effects.
                "*_send",
                // External servers — arbitrary, unaudited side effects.
                "mcp__*",
            ]),
            Self::Auto => ask(&["*_delete", "team_disband", "vault_*"]),
            Self::Full => ToolPermissionsConfig::default(),
        }
    }

    /// Effective global permission layer: this tier's preset with the explicit
    /// `[policies.tool_permissions]` block layered on top.
    ///
    /// Explicit entries REPLACE the preset's (they are not merged
    /// most-restrictive-wins): an operator who spells out a tool by name has
    /// made a deliberate decision, and that is what makes a Panel "advanced
    /// overrides" section coherent — otherwise an override could only ever
    /// tighten what the tier already decided.
    #[must_use]
    pub fn overlay(self, explicit: &ToolPermissionsConfig) -> ToolPermissionsConfig {
        let mut cfg = self.expand();
        cfg.default = explicit.default;
        cfg.overrides.extend(
            explicit
                .overrides
                .iter()
                .map(|(name, action)| (name.clone(), *action)),
        );
        cfg
    }
}

/// A tier as presented to a user surface (Panel / CLI / bot). Rendering the
/// same three everywhere keeps R6 (one core, many channels) honest.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct TierPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

/// The three built-in tiers, ordered least → most permissive.
#[must_use]
pub const fn builtin_tiers() -> &'static [TierPreset] {
    &[
        TierPreset {
            id: "ask",
            label: "Ask 请求",
            description: "每个会改动系统的工具调用（执行命令、写文件、删除、凭据、MCP）都先征求同意；只读工具照常运行。\nEvery mutating tool call asks first; read-only tools still run freely.",
        },
        TierPreset {
            id: "auto",
            label: "Auto 自动",
            description: "工具自动运行，只有不可逆的操作（删除、凭据写入、解散团队）才征求同意。默认档位。\nTools run automatically; only irreversible operations ask first. The default.",
        },
        TierPreset {
            id: "full",
            label: "Full 完全",
            description: "从不询问。灾难性命令的硬底线（fork bomb / rm -rf / 抹盘）依然生效，任何档位都无法关闭。\nNever asks. The catastrophic command floor still holds — no tier can disable it.",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::command_policy::{CommandPolicy, EnforcementMode};

    #[test]
    fn full_tier_leaves_the_command_policy_hardline_floor_intact() {
        // THE invariant of the tier system: a tier is a projection onto the
        // TOOL permission axis only. `Full` widens that axis to the maximum
        // (default Allow, zero overrides) and still cannot reach the sandbox
        // command-policy hardline — fork bombs, `rm -rf /`, device wipes stay
        // blocked under every tier and every enforcement mode.
        let full = ExecTier::Full.expand();
        assert_eq!(full.default, PermissionAction::Allow);
        assert!(full.overrides.is_empty());

        for enforcement in [
            EnforcementMode::Block,
            EnforcementMode::Warn,
            EnforcementMode::Off,
        ] {
            let policy = CommandPolicy::defaults(enforcement);
            for cmd in ["rm -rf /", "rm -rf / --no-preserve-root", ":(){ :|:& };:"] {
                assert!(
                    !policy.evaluate(cmd).blocked.is_empty(),
                    "hardline floor must block `{cmd}` (enforcement {enforcement:?}) regardless of exec tier"
                );
            }
        }
    }

    #[test]
    fn default_tier_is_auto() {
        assert_eq!(ExecTier::default(), ExecTier::Auto);
    }

    #[test]
    fn ask_tier_asks_for_mutators_and_allows_readers() {
        let cfg = ExecTier::Ask.expand();
        assert_eq!(cfg.resolve("bash"), PermissionAction::Ask);
        assert_eq!(cfg.resolve("file_write"), PermissionAction::Ask);
        assert_eq!(cfg.resolve("agent_delete"), PermissionAction::Ask);
        assert_eq!(cfg.resolve("vault_store"), PermissionAction::Ask);
        assert_eq!(cfg.resolve("mcp__github_write"), PermissionAction::Ask);
        // Read-only surface stays open so the model can still investigate.
        assert_eq!(cfg.resolve("file_read"), PermissionAction::Allow);
        assert_eq!(cfg.resolve("search"), PermissionAction::Allow);
        assert_eq!(cfg.resolve("memory_search"), PermissionAction::Allow);
    }

    #[test]
    fn auto_tier_only_guards_the_destructive_tail() {
        let cfg = ExecTier::Auto.expand();
        assert_eq!(cfg.resolve("bash"), PermissionAction::Allow);
        assert_eq!(cfg.resolve("file_write"), PermissionAction::Allow);
        assert_eq!(cfg.resolve("agent_delete"), PermissionAction::Ask);
        assert_eq!(cfg.resolve("team_disband"), PermissionAction::Ask);
        assert_eq!(cfg.resolve("vault_store"), PermissionAction::Ask);
    }

    #[test]
    fn explicit_overrides_win_over_the_tier_preset() {
        let explicit = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [
                ("bash".to_string(), PermissionAction::Allow),
                ("search".to_string(), PermissionAction::Deny),
            ]
            .into_iter()
            .collect(),
        };
        let cfg = ExecTier::Ask.overlay(&explicit);
        // Explicit allow beats the tier's Ask...
        assert_eq!(cfg.resolve("bash"), PermissionAction::Allow);
        // ...and an explicit deny lands on a tool the tier left alone.
        assert_eq!(cfg.resolve("search"), PermissionAction::Deny);
        // Untouched preset entries survive.
        assert_eq!(cfg.resolve("file_write"), PermissionAction::Ask);
    }

    #[test]
    fn tier_id_roundtrip() {
        for tier in [ExecTier::Ask, ExecTier::Auto, ExecTier::Full] {
            assert_eq!(ExecTier::from_id(tier.id()), Some(tier));
        }
        assert_eq!(ExecTier::from_id("nonsense"), None);
    }

    #[test]
    fn builtin_tiers_cover_every_variant() {
        let ids: Vec<&str> = builtin_tiers().iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["ask", "auto", "full"]);
        assert!(builtin_tiers()
            .iter()
            .all(|p| ExecTier::from_id(p.id).is_some()));
    }

    #[test]
    fn deserializes_from_policies_toml() {
        let toml_str = r#"exec_tier = "ask""#;
        let cfg: super::super::PoliciesConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.exec_tier, ExecTier::Ask);
        // Unconfigured installs stay on today's behavior.
        let empty: super::super::PoliciesConfig = toml::from_str("").unwrap();
        assert_eq!(empty.exec_tier, ExecTier::Auto);
    }
}
