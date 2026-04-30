//! Configuration-driven approval policy.
//!
//! Loads approval rules from `~/.aleph/approval-policy.json` and evaluates
//! action requests against blocklists, allowlists, and per-action-type defaults.

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::policy::ApprovalPolicy;
use super::types::{ActionRequest, ActionType, ApprovalDecision, DefaultDecision};

// ---------------------------------------------------------------------------
// JSON config schema
// ---------------------------------------------------------------------------

/// Top-level policy configuration, deserialized from JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfig {
    /// Schema version (currently 1).
    pub version: u32,
    /// Per-action-type default decisions.
    pub defaults: HashMap<ActionType, DefaultDecision>,
    /// Rules that unconditionally allow matching actions.
    #[serde(default)]
    pub allowlist: Vec<PolicyRule>,
    /// Rules that unconditionally deny matching actions.
    #[serde(default)]
    pub blocklist: Vec<PolicyRule>,
}

/// A single allowlist or blocklist entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    /// The action type this rule applies to.
    #[serde(rename = "type")]
    pub action_type: ActionType,
    /// Glob pattern matched against the action target.
    pub pattern: String,
}

// ---------------------------------------------------------------------------
// Glob matching
// ---------------------------------------------------------------------------

/// Convert a glob pattern to a regex string.
///
/// Pattern rules:
/// - `*`  matches any characters except `/`
/// - `**` matches any characters including `/`
/// - `?`  matches a single character (except `/`)
///
/// This intentionally mirrors the logic in `exec/approval/binding.rs`.
fn glob_to_regex_str(pattern: &str) -> String {
    let mut regex_str = String::with_capacity(pattern.len() * 2);
    regex_str.push('^');

    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    // ** matches everything including /
                    // If followed by /, make the slash optional so **/x matches x
                    if chars.peek() == Some(&'/') {
                        chars.next();
                        regex_str.push_str("(.*/)?");
                    } else {
                        regex_str.push_str(".*");
                    }
                } else {
                    // * matches everything except /
                    regex_str.push_str("[^/]*");
                }
            }
            '?' => regex_str.push_str("[^/]"),
            '.' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '+' | '\\' => {
                regex_str.push('\\');
                regex_str.push(ch);
            }
            _ => regex_str.push(ch),
        }
    }

    regex_str.push('$');
    regex_str
}

/// Match a value against a glob pattern.
///
/// Public for use in tests. The hot path in [`ConfigApprovalPolicy::check`]
/// uses pre-compiled regexes instead.
pub fn matches_glob(value: &str, pattern: &str) -> bool {
    regex::Regex::new(&glob_to_regex_str(pattern))
        .map(|re| re.is_match(value))
        .unwrap_or(false)
}

/// A compiled policy rule, pairing the original glob pattern with its regex.
#[derive(Debug, Clone)]
struct CompiledRule {
    /// Original glob pattern (used for logging and audit messages).
    pattern: String,
    /// Compiled regex for matching.
    regex: regex::Regex,
}

/// Pre-compile a list of [`PolicyRule`]s into a map keyed by [`ActionType`].
///
/// Rules whose patterns fail to compile are skipped with a warning.
fn compile_rules_grouped(rules: &[PolicyRule]) -> HashMap<ActionType, Vec<CompiledRule>> {
    let mut grouped: HashMap<ActionType, Vec<CompiledRule>> = HashMap::new();
    for rule in rules {
        let regex_str = glob_to_regex_str(&rule.pattern);
        match regex::Regex::new(&regex_str) {
            Ok(regex) => {
                grouped
                    .entry(rule.action_type.clone())
                    .or_default()
                    .push(CompiledRule {
                        pattern: rule.pattern.clone(),
                        regex,
                    });
            }
            Err(e) => {
                tracing::warn!(
                    pattern = %rule.pattern,
                    error = %e,
                    "Failed to compile glob pattern; skipping rule"
                );
            }
        }
    }
    grouped
}

// ---------------------------------------------------------------------------
// ConfigApprovalPolicy
// ---------------------------------------------------------------------------

/// An [`ApprovalPolicy`] backed by a JSON configuration file.
///
/// Decision logic (evaluated in order):
/// 1. If the target matches any **blocklist** entry for the action type → `Deny`
/// 2. If the target matches any **allowlist** entry for the action type → `Allow`
/// 3. Fall back to the **defaults** map for the action type
/// 4. If no default is configured → `Ask`
pub struct ConfigApprovalPolicy {
    config: PolicyConfig,
    blocklist_by_type: HashMap<ActionType, Vec<CompiledRule>>,
    allowlist_by_type: HashMap<ActionType, Vec<CompiledRule>>,
}

impl ConfigApprovalPolicy {
    /// Create a new policy from an explicit [`PolicyConfig`].
    pub fn new(config: PolicyConfig) -> Self {
        let blocklist_by_type = compile_rules_grouped(&config.blocklist);
        let allowlist_by_type = compile_rules_grouped(&config.allowlist);
        Self {
            config,
            blocklist_by_type,
            allowlist_by_type,
        }
    }

    /// Load the policy from `~/.aleph/approval-policy.json`.
    ///
    /// If the file does not exist or cannot be parsed, a sensible default
    /// policy is returned instead.
    pub fn load() -> Self {
        Self::load_from(Self::config_path())
    }

    /// Load the policy from the given path.
    ///
    /// If the file does not exist or cannot be parsed, a sensible default
    /// policy is returned instead.
    pub fn load_from(path: PathBuf) -> Self {
        match std::fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str::<PolicyConfig>(&contents) {
                Ok(config) => {
                    debug!("Loaded approval policy from {}", path.display());
                    Self::new(config)
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to parse approval policy at {}: {}. Using defaults.",
                        path.display(),
                        e
                    );
                    Self::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!(
                    "Approval policy file not found at {}. Using defaults.",
                    path.display()
                );
                Self::default()
            }
            Err(e) => {
                tracing::error!(
                    "Failed to read approval policy at {}: {}. Using defaults.",
                    path.display(),
                    e
                );
                Self::default()
            }
        }
    }

    /// Return the expected path for the configuration file.
    fn config_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| {
                warn!("Cannot determine home directory; approval policy will use temp dir fallback");
                std::env::temp_dir()
            })
            .join(".aleph")
            .join("approval-policy.json")
    }
}

impl Default for ConfigApprovalPolicy {
    /// Sensible defaults:
    /// - Browser navigate/click/type → Allow
    /// - Browser evaluate → Ask
    /// - Desktop actions → Ask
    /// - Shell exec → Deny
    fn default() -> Self {
        let mut defaults = HashMap::new();
        defaults.insert(ActionType::BrowserNavigate, DefaultDecision::Allow);
        defaults.insert(ActionType::BrowserClick, DefaultDecision::Allow);
        defaults.insert(ActionType::BrowserType, DefaultDecision::Allow);
        defaults.insert(ActionType::BrowserFill, DefaultDecision::Allow);
        defaults.insert(ActionType::BrowserEvaluate, DefaultDecision::Ask);
        defaults.insert(ActionType::DesktopClick, DefaultDecision::Ask);
        defaults.insert(ActionType::DesktopType, DefaultDecision::Ask);
        defaults.insert(ActionType::DesktopKeyCombo, DefaultDecision::Ask);
        defaults.insert(ActionType::DesktopLaunchApp, DefaultDecision::Ask);
        defaults.insert(ActionType::ShellExec, DefaultDecision::Deny);

        Self::new(PolicyConfig {
            version: 1,
            defaults,
            allowlist: vec![],
            blocklist: vec![],
        })
    }
}

#[async_trait]
impl ApprovalPolicy for ConfigApprovalPolicy {
    async fn check(&self, request: &ActionRequest) -> ApprovalDecision {
        let action = &request.action_type;
        let target = &request.target;

        // 1. Blocklist takes priority (pre-compiled regexes, grouped by ActionType)
        if let Some(rules) = self.blocklist_by_type.get(action) {
            for rule in rules {
                if rule.regex.is_match(target) {
                    debug!(
                        action = ?action,
                        target = %target,
                        pattern = %rule.pattern,
                        "Blocked by blocklist rule"
                    );
                    return ApprovalDecision::Deny {
                        reason: format!("Blocked by policy rule: {}", rule.pattern),
                    };
                }
            }
        }

        // 2. Allowlist overrides defaults (pre-compiled regexes, grouped by ActionType)
        if let Some(rules) = self.allowlist_by_type.get(action) {
            for rule in rules {
                if rule.regex.is_match(target) {
                    debug!(
                        action = ?action,
                        target = %target,
                        pattern = %rule.pattern,
                        "Allowed by allowlist rule"
                    );
                    return ApprovalDecision::Allow;
                }
            }
        }

        // 3. Fall back to defaults
        if let Some(default_decision) = self.config.defaults.get(action) {
            return match default_decision {
                DefaultDecision::Allow => ApprovalDecision::Allow,
                DefaultDecision::Deny => ApprovalDecision::Deny {
                    reason: format!("Denied by default policy for {:?}", action),
                },
                DefaultDecision::Ask => ApprovalDecision::Ask {
                    prompt: format!(
                        "Action {:?} on target '{}' requires approval",
                        action, target
                    ),
                },
            };
        }

        // 4. No default → Ask
        ApprovalDecision::Ask {
            prompt: format!(
                "No policy configured for {:?} on '{}'. Please approve or deny.",
                action, target
            ),
        }
    }

    async fn record(&self, request: &ActionRequest, decision: &ApprovalDecision) {
        info!(
            action = ?request.action_type,
            target = %request.target,
            agent = %request.agent_id,
            decision = ?decision,
            "Approval decision recorded"
        );
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_single_star() {
        // * does not cross path boundaries
        assert!(matches_glob("file.txt", "*.txt"));
        assert!(!matches_glob("dir/file.txt", "*.txt"));
    }

    #[test]
    fn test_glob_double_star() {
        // ** crosses path boundaries
        assert!(matches_glob("a/b/c.txt", "**/*.txt"));
        assert!(matches_glob("c.txt", "**/*.txt"));
    }

    #[test]
    fn test_glob_question_mark() {
        assert!(matches_glob("file.txt", "fil?.txt"));
        assert!(!matches_glob("fill.txt", "fil?.tx"));
    }

    #[test]
    fn test_glob_url_pattern() {
        // Single * does not cross /
        assert!(matches_glob(
            "https://docs.github.com/actions",
            "https://*.github.com/*"
        ));
        assert!(!matches_glob(
            "https://docs.github.com/en/actions",
            "https://*.github.com/*"
        ));
        // ** matches across path separators
        assert!(matches_glob(
            "https://docs.github.com/en/actions",
            "https://*.github.com/**"
        ));
        assert!(matches_glob(
            "https://docs.github.com/en/actions/sub",
            "https://*.github.com/**"
        ));
    }

    #[test]
    fn test_glob_bundle_id() {
        assert!(matches_glob("com.apple.Safari", "com.apple.*"));
        assert!(!matches_glob("com.google.Chrome", "com.apple.*"));
    }

    #[test]
    fn test_glob_special_chars() {
        // Dots and parens are escaped properly
        assert!(matches_glob("a.b.c", "a.b.c"));
        assert!(!matches_glob("axbxc", "a.b.c"));
    }
}
