//! Security Context for Channel Capability Awareness
//!
//! This module defines policy-driven security types that describe what is ALLOWED
//! by security policy, orthogonal to `InteractionManifest` which describes what is
//! technically possible.
//!
//! # Prompt text, not a gate
//!
//! Everything here exists to *describe* the posture to the model — the bullets
//! [`SecurityContext::security_notes`] and
//! [`SecurityContext::elevated_policy_note`] produce. It does not decide whether
//! a call runs: that is declared tool metadata × exec tier × the sandbox floor,
//! enforced in `src/tools/scoped/`.
//!
//! A `check_tool` / `ToolPermission` pair used to live here and answer "may this
//! tool run?" by matching hardcoded tool-name substrings. Its only caller was
//! `ContextAggregator`'s two-phase filter, which production fed an empty slice;
//! both were removed 2026-07-27 rather than wired up, because a name-matched
//! verdict printed beside the enforced tier is a second voice on a question that
//! must have exactly one — see [`SecurityContext::elevated_policy_note`].
//!
//! # Examples
//!
//! ```
//! use std::path::PathBuf;
//! use alephcore::thinker::security_context::SecurityContext;
//!
//! // A permissive context imposes nothing worth telling the model.
//! let ctx = SecurityContext::permissive();
//! assert!(ctx.elevated_policy_note().is_none());
//!
//! // A strict context spells out its restrictions.
//! let ctx = SecurityContext::strict_readonly(PathBuf::from("/workspace"));
//! assert!(ctx.security_notes().iter().any(|n| n.contains("Network Access: Disabled")));
//! ```

use std::collections::HashSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::interaction::InteractionParadigm;

/// Sandbox isolation level for tool execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxLevel {
    /// No sandboxing - full system access
    #[default]
    None,
    /// Standard sandboxing - workspace-scoped filesystem, network allowed
    Standard,
    /// Strict sandboxing - limited tools, no dangerous operations
    Strict,
    /// Untrusted mode - minimal permissions, heavy restrictions
    Untrusted,
}

impl SandboxLevel {
    /// Returns a human-readable description for use in prompts
    #[must_use]
    pub const fn description(&self) -> &'static str {
        match self {
            Self::None => "Full system access with no sandboxing restrictions",
            Self::Standard => "Standard sandbox with workspace-scoped filesystem access",
            Self::Strict => "Strict sandbox with limited tool access and no dangerous operations",
            Self::Untrusted => "Untrusted mode with minimal permissions and heavy restrictions",
        }
    }
}

/// Policy for elevated/privileged operations (exec, bash, etc.)
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElevatedPolicy {
    /// Elevated operations are completely disabled
    #[default]
    Off,
    /// Ask for user approval on each elevated operation
    Ask,
    /// Allow only commands in the allowlist
    AllowList(Vec<String>),
    /// Full elevated access without restrictions
    Full,
}

/// Security context defining what operations are allowed by policy
///
/// This is orthogonal to `InteractionManifest` which describes technical
/// capabilities. `SecurityContext` describes what is ALLOWED, not what is
/// technically possible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityContext {
    /// The sandbox isolation level
    pub sandbox_level: SandboxLevel,
    /// Tool names the prompt should report as denied (a `Denied Tools:` note).
    /// Descriptive only — the enforcement allow/deny list is `tool_permissions`.
    pub denied_tools: HashSet<String>,
    /// Filesystem scope restriction (None means no restriction)
    pub filesystem_scope: Option<PathBuf>,
    /// Whether network operations are allowed
    pub network_allowed: bool,
    /// Policy for elevated operations (exec, bash)
    pub elevated_policy: ElevatedPolicy,
}

impl Default for SecurityContext {
    fn default() -> Self {
        Self::permissive()
    }
}

impl SecurityContext {
    /// Create a permissive context with full access
    ///
    /// Use this for trusted environments where the user has full control.
    #[must_use]
    pub fn permissive() -> Self {
        Self {
            sandbox_level: SandboxLevel::None,
            denied_tools: HashSet::new(),
            filesystem_scope: None,
            network_allowed: true,
            elevated_policy: ElevatedPolicy::Full,
        }
    }

    /// Create a standard sandbox context scoped to a workspace
    ///
    /// This provides reasonable security for most use cases:
    /// - Filesystem access scoped to workspace
    /// - Network allowed
    /// - Elevated operations require approval
    #[must_use]
    pub fn standard_sandbox(workspace: PathBuf) -> Self {
        Self {
            sandbox_level: SandboxLevel::Standard,
            denied_tools: HashSet::new(),
            filesystem_scope: Some(workspace),
            network_allowed: true,
            elevated_policy: ElevatedPolicy::Ask,
        }
    }

    /// Channel-aware default policy mapped from the interaction paradigm.
    ///
    /// This is the prompt-level security posture surfaced to the LLM via
    /// [`SecurityContext::security_notes`]. Tool enforcement at runtime
    /// happens in the tool execution layer; this constructor only determines
    /// *what the LLM is told* about its current envelope.
    ///
    /// - `CLI` / `WebRich` / `Background` / `Embedded` → [`permissive`] —
    ///   the trusted-self-host posture (user controls the device).
    /// - `Messaging` → Standard sandbox with elevated operations requiring
    ///   approval. Public-channel bots can be addressed by parties other
    ///   than the operator, so the LLM is told to be more cautious with
    ///   exec/bash. No filesystem-scope restriction is set because the
    ///   gateway has no canonical workspace path to attach.
    ///
    /// [`permissive`]: SecurityContext::permissive
    #[must_use]
    pub fn for_paradigm(paradigm: InteractionParadigm) -> Self {
        match paradigm {
            InteractionParadigm::CLI
            | InteractionParadigm::WebRich
            | InteractionParadigm::Background
            | InteractionParadigm::Embedded => Self::permissive(),
            InteractionParadigm::Messaging => Self {
                sandbox_level: SandboxLevel::Standard,
                denied_tools: HashSet::new(),
                filesystem_scope: None,
                network_allowed: true,
                elevated_policy: ElevatedPolicy::Ask,
            },
        }
    }

    /// Create a strict read-only context
    ///
    /// Use this for untrusted inputs or when maximum safety is needed:
    /// - Filesystem access scoped to workspace
    /// - No network access
    /// - No elevated operations (exec, bash)
    /// - File operations tool is denied
    #[must_use]
    pub fn strict_readonly(workspace: PathBuf) -> Self {
        let mut denied_tools = HashSet::new();
        denied_tools.insert("file_ops".to_string());
        denied_tools.insert("exec".to_string());
        denied_tools.insert("bash".to_string());
        denied_tools.insert("bash_exec".to_string());
        denied_tools.insert("code_exec".to_string());

        Self {
            sandbox_level: SandboxLevel::Strict,
            denied_tools,
            filesystem_scope: Some(workspace),
            network_allowed: false,
            elevated_policy: ElevatedPolicy::Off,
        }
    }

    /// Generate security notes for prompt injection
    ///
    /// Returns a list of security-related notes that should be included
    /// in the system prompt to inform the LLM of current restrictions.
    #[must_use]
    pub fn security_notes(&self) -> Vec<String> {
        let mut notes = Vec::new();

        // Sandbox level note
        notes.push(format!(
            "Security Level: {} - {}",
            match self.sandbox_level {
                SandboxLevel::None => "None",
                SandboxLevel::Standard => "Standard",
                SandboxLevel::Strict => "Strict",
                SandboxLevel::Untrusted => "Untrusted",
            },
            self.sandbox_level.description()
        ));

        // Filesystem scope note
        if let Some(ref scope) = self.filesystem_scope {
            notes.push(format!(
                "Filesystem Access: Restricted to {}",
                scope.display()
            ));
        }

        // Network note
        if !self.network_allowed {
            notes.push("Network Access: Disabled".to_string());
        }

        // NOTE: the elevated-policy note is deliberately NOT here — see
        // `elevated_policy_note`. It describes the same thing as the resolved
        // `ExecTier` (does a mutating call pause for the human?) and only one of
        // the two may speak per prompt.

        // Denied tools note (sorted for deterministic output)
        if !self.denied_tools.is_empty() {
            let mut tools: Vec<_> = self.denied_tools.iter().cloned().collect();
            tools.sort();
            notes.push(format!("Denied Tools: {}", tools.join(", ")));
        }

        notes
    }

    /// The paradigm-derived approval posture, as a prompt bullet — or `None` when
    /// the policy imposes nothing worth saying (`Full`).
    ///
    /// Split out of [`Self::security_notes`] because it is a **second voice on the
    /// same question** as the resolved [`ExecTier`](crate::config::types::policies::ExecTier)
    /// that `SecurityLayer` renders as `Approval mode:`. They disagreed in the
    /// default install: a Messaging paradigm derives `ElevatedPolicy::Ask`, so a
    /// Telegram turn at `exec_tier = auto` was told both "Approval mode: auto —
    /// routine tool calls run without interruption" *and* "Elevated Operations:
    /// Require user approval before execution", in one bullet list, three lines
    /// apart. Only the tier is enforced (`src/tools/scoped/`), and the unenforced
    /// line came second — winning on recency.
    ///
    /// `SecurityLayer` therefore renders this only when no tier was resolved, so
    /// the prompt always carries exactly one approval regime, and it is the
    /// enforced one.
    #[must_use]
    pub fn elevated_policy_note(&self) -> Option<String> {
        match &self.elevated_policy {
            ElevatedPolicy::Off => {
                Some("Elevated Operations: Disabled (exec, bash not available)".to_string())
            }
            ElevatedPolicy::Ask => {
                Some("Elevated Operations: Require user approval before execution".to_string())
            }
            ElevatedPolicy::AllowList(list) => Some(format!(
                "Elevated Operations: Limited to allowlist ({} entries)",
                list.len()
            )),
            ElevatedPolicy::Full => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_security_notes() {
        let ctx = SecurityContext::strict_readonly(PathBuf::from("/workspace"));
        let notes = ctx.security_notes();

        // Should have multiple notes
        assert!(!notes.is_empty());

        // Should mention strict level
        assert!(notes.iter().any(|n| n.contains("Strict")));

        // Should mention filesystem restriction
        assert!(notes.iter().any(|n| n.contains("/workspace")));

        // Should mention network disabled
        assert!(notes.iter().any(|n| n.contains("Network Access: Disabled")));

        // The elevated-policy posture is NOT in `security_notes`: it answers the
        // same question as the enforced `ExecTier`, so it lives in its own
        // accessor and `SecurityLayer` renders it only when no tier was resolved.
        assert!(!notes.iter().any(|n| n.contains("Elevated Operations")));
        assert!(ctx
            .elevated_policy_note()
            .is_some_and(|n| n.contains("Elevated Operations: Disabled")));

        // Should mention denied tools
        assert!(notes.iter().any(|n| n.contains("Denied Tools")));
    }

    #[test]
    fn test_sandbox_level_descriptions() {
        assert!(SandboxLevel::None
            .description()
            .to_lowercase()
            .contains("no"));
        assert!(SandboxLevel::Standard.description().contains("Standard"));
        assert!(SandboxLevel::Strict.description().contains("Strict"));
        assert!(SandboxLevel::Untrusted.description().contains("Untrusted"));
    }

    /// `AllowList` is the one `ElevatedPolicy` arm no constructor produces, so
    /// its note is only reachable from a hand-built context — keep it pinned.
    #[test]
    fn elevated_allowlist_note_reports_its_size() {
        let ctx = SecurityContext {
            sandbox_level: SandboxLevel::Standard,
            denied_tools: HashSet::new(),
            filesystem_scope: None,
            network_allowed: true,
            elevated_policy: ElevatedPolicy::AllowList(vec!["bash".to_string()]),
        };

        assert!(ctx
            .elevated_policy_note()
            .is_some_and(|n| n.contains("Limited to allowlist (1 entries)")));
    }

    #[test]
    fn for_paradigm_cli_is_permissive() {
        let ctx = SecurityContext::for_paradigm(InteractionParadigm::CLI);
        assert_eq!(ctx.sandbox_level, SandboxLevel::None);
        assert!(ctx.network_allowed);
        assert!(matches!(ctx.elevated_policy, ElevatedPolicy::Full));
        // `Full` imposes nothing worth telling the model.
        assert!(ctx.elevated_policy_note().is_none());
    }

    #[test]
    fn for_paradigm_webrich_background_embedded_are_permissive() {
        for paradigm in [
            InteractionParadigm::WebRich,
            InteractionParadigm::Background,
            InteractionParadigm::Embedded,
        ] {
            let ctx = SecurityContext::for_paradigm(paradigm);
            assert_eq!(ctx.sandbox_level, SandboxLevel::None, "{:?}", paradigm);
            assert!(matches!(ctx.elevated_policy, ElevatedPolicy::Full));
        }
    }

    #[test]
    fn for_paradigm_messaging_requires_approval_for_exec() {
        let ctx = SecurityContext::for_paradigm(InteractionParadigm::Messaging);
        assert_eq!(ctx.sandbox_level, SandboxLevel::Standard);
        assert!(ctx.network_allowed); // messaging bots typically need network
        assert!(matches!(ctx.elevated_policy, ElevatedPolicy::Ask));
        assert!(ctx.filesystem_scope.is_none());

        // The cautious posture must reach the model — that is the entire point
        // of deriving a policy from the channel. (Whether a call is actually
        // held for approval is the exec tier's job, not this type's.)
        assert!(ctx
            .elevated_policy_note()
            .is_some_and(|n| n.contains("Require user approval")));
    }

    #[test]
    fn for_paradigm_messaging_security_notes_announce_approval_policy() {
        let ctx = SecurityContext::for_paradigm(InteractionParadigm::Messaging);
        let notes = ctx.security_notes();
        assert!(
            ctx.elevated_policy_note()
                .is_some_and(|n| n.contains("Elevated Operations: Require user approval")),
            "messaging paradigm must surface approval-required posture"
        );
        // …but not inside `security_notes`, where it would render unconditionally
        // and contradict the enforced `Approval mode:` line.
        assert!(
            !notes.iter().any(|n| n.contains("Elevated Operations")),
            "elevated posture must not ride security_notes, got: {notes:?}"
        );
        assert!(
            notes.iter().any(|n| n.contains("Standard")),
            "messaging paradigm must surface Standard sandbox level"
        );
    }

    #[test]
    fn for_paradigm_cli_security_notes_stay_minimal() {
        let ctx = SecurityContext::for_paradigm(InteractionParadigm::CLI);
        let notes = ctx.security_notes();
        // Permissive ⇒ only the baseline "Security Level: None" line.
        assert_eq!(notes.len(), 1, "permissive should emit only the baseline");
        assert!(notes[0].contains("Security Level: None"));
    }

    #[test]
    fn test_default_context() {
        let ctx = SecurityContext::default();
        // Default should be permissive
        assert_eq!(ctx.sandbox_level, SandboxLevel::None);
        assert!(ctx.network_allowed);
        assert!(matches!(ctx.elevated_policy, ElevatedPolicy::Full));
    }
}
