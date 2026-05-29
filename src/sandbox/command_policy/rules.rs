//! Default catastrophic-command ruleset + action / enforcement types.
//!
//! Ported in spirit from clawshell's DLP `[[patterns]]` engine (regex +
//! `action = block|redact`), but specialised for *shell command* content
//! rather than HTTP payloads, and matched in a single pass via
//! `regex::RegexSet` instead of clawshell's sequential `Vec<Regex>` scan.
//!
//! Philosophy (CLAUDE.md R7 "安全硬过滤" — a sanctioned hard-filter, NOT an
//! LLM-replacing rule engine): this layer is defence-in-depth *in front of*
//! the OS sandbox. It does not decide intent; it refuses a small set of
//! patterns that are essentially never legitimate inside an agent workspace
//! and audits a slightly larger set of high-signal suspicious shapes. The
//! OS seatbelt/bwrap/job-object remains the real enforcer.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a matched rule asks the policy to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    /// Refuse execution outright (subject to the global [`EnforcementMode`]).
    Block,
    /// Allow execution but emit an audit log line.
    Warn,
}

impl Default for RuleAction {
    fn default() -> Self {
        // A custom rule with no explicit action defaults to the strongest
        // posture — opting in to a rule signals intent to stop something.
        RuleAction::Block
    }
}

/// Global override applied on top of per-rule [`RuleAction`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EnforcementMode {
    /// Honour per-rule actions: `Block` rules deny, `Warn` rules audit.
    #[default]
    Block,
    /// Observation mode: downgrade every `Block` to a `Warn`. Nothing is
    /// ever denied — useful for staged rollout / measuring false positives.
    Warn,
    /// Disable the policy entirely (the hook short-circuits to Allow).
    Off,
}

/// A single command-policy rule: a name, a human-readable description, the
/// action to take on match, and the regex source. `description` is surfaced
/// in the deny/warn message and audit log so an operator can tell *why* a
/// command was refused.
#[derive(Debug, Clone)]
pub struct PolicyRule {
    pub name: &'static str,
    pub description: &'static str,
    pub action: RuleAction,
    pub pattern: &'static str,
}

/// The curated default ruleset.
///
/// `Block` entries are patterns with essentially no legitimate use inside a
/// per-session agent workspace; `Warn` entries are high-signal shapes that
/// can occasionally be legitimate, so they are audited rather than refused.
/// All patterns are matched case-insensitively (see [`super::CommandPolicy`]).
pub fn default_rules() -> Vec<PolicyRule> {
    use RuleAction::{Block, Warn};
    vec![
        // ── Block: never legitimate in a sandboxed workspace ──────────────
        PolicyRule {
            name: "fork_bomb",
            description: "fork bomb — a self-piping backgrounded function that exhausts PIDs",
            action: Block,
            // Matches the structural shape `…(){ … | … & };` (e.g. the classic
            // `:(){ :|:& };:`). No backrefs in the regex crate, so this keys on
            // the function-body shape rather than name equality.
            pattern: r"\(\s*\)\s*\{[^}]*\|[^}]*&[^}]*\}\s*;",
        },
        PolicyRule {
            name: "rm_no_preserve_root",
            description: "rm --no-preserve-root — explicit request to delete the filesystem root",
            action: Block,
            pattern: r"\brm\b[^\n]*--no-preserve-root",
        },
        PolicyRule {
            name: "dd_to_block_device",
            description: "dd writing directly to a raw block device (disk-wipe / overwrite)",
            action: Block,
            pattern: r"\bdd\b[^\n]*\bof\s*=\s*/dev/(sd|nvme|disk|hd|vd|mmcblk|loop)",
        },
        PolicyRule {
            name: "mkfs_device",
            description: "mkfs formatting a device node (destroys an existing filesystem)",
            action: Block,
            pattern: r"\bmkfs(\.\w+)?\b[^\n]*\s/dev/",
        },
        PolicyRule {
            name: "redirect_to_block_device",
            description: "shell redirect overwriting a raw block device",
            action: Block,
            pattern: r">\s*/dev/(sd|nvme|disk|hd|vd|mmcblk)",
        },
        // ── Warn: high-signal but occasionally legitimate ─────────────────
        PolicyRule {
            name: "rm_rf_system_path",
            description: "recursive force-remove targeting an absolute root / home path",
            action: Warn,
            // Requires rm + a recursive-and-force flag combo (-rf / -fr / -Rf …)
            // and an absolute root / system / home target on the same line.
            pattern: r"\brm\s+(?:-{1,2}\S+\s+)*-{1,2}[a-z]*(?:rf|fr|r\S*f|f\S*r)[a-z]*\b[^\n]*\s(?:/|/\*|~|\$HOME|/etc|/usr|/var|/bin|/boot|/lib|/sys|/root|/sbin)(?:\s|/|\*|$|[\x22\x27])",
        },
        PolicyRule {
            name: "pipe_to_shell",
            description: "download piped straight into an interpreter (curl|wget … | sh/bash/python)",
            action: Warn,
            pattern: r"\b(?:curl|wget|fetch)\b[^\n|]*\|[^\n]*\b(?:sh|bash|zsh|ksh|python3?|perl|ruby|node)\b",
        },
        PolicyRule {
            name: "chmod_777_system",
            description: "world-writable chmod 777 on an absolute root / system path",
            action: Warn,
            pattern: r"\bchmod\s+(?:-{1,2}\S+\s+)*[0-7]*777[0-7]*\s+(?:/|/\*|/etc|/usr|/bin|/var)(?:\s|/|$)",
        },
        PolicyRule {
            name: "write_sensitive_etc",
            description: "writing to a sensitive system credential file (/etc/passwd, shadow, sudoers)",
            action: Warn,
            pattern: r"(?:>|>>|\btee\b[^\n]*)\s*/etc/(?:passwd|shadow|sudoers|gshadow)\b",
        },
        PolicyRule {
            name: "reverse_shell_devtcp",
            description: "bash /dev/tcp reverse-shell or raw TCP socket exfiltration",
            action: Warn,
            pattern: r"/dev/tcp/\d",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rules_are_nonempty_and_named_uniquely() {
        let rules = default_rules();
        assert!(rules.len() >= 8, "expected a meaningful default ruleset");
        let mut names: Vec<&str> = rules.iter().map(|r| r.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "rule names must be unique");
    }

    #[test]
    fn enforcement_and_action_defaults() {
        assert_eq!(EnforcementMode::default(), EnforcementMode::Block);
        assert_eq!(RuleAction::default(), RuleAction::Block);
    }

    #[test]
    fn action_serde_roundtrip_is_lowercase() {
        assert_eq!(
            serde_json::to_string(&RuleAction::Block).unwrap(),
            "\"block\""
        );
        assert_eq!(
            serde_json::from_str::<RuleAction>("\"warn\"").unwrap(),
            RuleAction::Warn
        );
    }
}
