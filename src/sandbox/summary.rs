//! Sandbox prompt summary — codex-inspired surfacing of active sandbox
//! posture into the LLM system prompt (R9: intelligence in the prompt).
//!
//! The model receives a stable `<sandbox_summary>`-style section so it can
//! reason about what it's allowed to do without trial-and-error against a
//! silent enforcer. This is informational; runtime enforcement still happens
//! in the driver.
//!
//! Mirrors codex's `permission_profile_sandbox_tag` / `policy_tag` /
//! `summarize_sandbox_policy` trio in a single struct.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::sandbox::capabilities::{NetworkPolicy, SandboxCapabilities};

/// The sandbox filesystem/permission posture as an **ordered** type, so code
/// can reason about "at least as permissive as" instead of matching magic
/// strings. Maps `OpenSquilla`'s ordered `SecurityLevel` `IntEnum` onto idiomatic
/// Rust: the derived `Ord` ranks tiers from least to most permissive, so a
/// guard like `summary.tier() >= PolicyTier::DangerFullAccess` is meaningful
/// and total.
///
/// This enum is the single source of truth for the tier tag strings —
/// [`SandboxSummary::policy_tier`] is always set from [`PolicyTier::as_str`],
/// so the previously-scattered string literals now live in exactly one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyTier {
    /// No write access — observe-only.
    ReadOnly,
    /// Writes confined to explicit workspace roots.
    WorkspaceWrite,
    /// Workspace-tree isolation (a separate git worktree) WITHOUT an OS-level
    /// process sandbox — a distinct mechanism, ranked just above plain
    /// workspace-write because it pairs tree writes with open network and no
    /// seatbelt/bwrap confinement.
    Isolated,
    /// Unrestricted: root writable + all-hosts network. The danger posture.
    DangerFullAccess,
}

impl PolicyTier {
    /// Stable one-word tag (kebab-case) used in the prompt summary and shared
    /// docs. Matches codex's tier names so prompts read consistently.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::Isolated => "isolated",
            Self::DangerFullAccess => "danger-full-access",
        }
    }

    /// Recover the tier from its tag. Total: the field is always written from
    /// [`as_str`](Self::as_str), so every real value round-trips. An unknown
    /// tag is treated as [`DangerFullAccess`](Self::DangerFullAccess) — the
    /// fail-cautious choice that makes the model assume the riskiest posture.
    #[must_use]
    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "read-only" => Self::ReadOnly,
            "workspace-write" => Self::WorkspaceWrite,
            "isolated" => Self::Isolated,
            _ => Self::DangerFullAccess,
        }
    }

    /// True for the danger posture (root-writable + open network), where the
    /// model should be maximally cautious about destructive / exfiltration-prone
    /// actions.
    #[must_use]
    pub const fn is_danger(self) -> bool {
        matches!(self, Self::DangerFullAccess)
    }
}

/// Snapshot of the active sandbox posture suitable for prompt injection.
///
/// Construction is platform-agnostic — implementors of the `Sandbox` trait
/// return `Option<SandboxSummary>` describing their own backend + baseline.
/// The `Sandbox` trait's default impl returns `None`, so mock / Noop
/// sandboxes render nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxSummary {
    /// Backend tag in `os/mechanism` form (e.g. `"macos/seatbelt"`,
    /// `"linux/bwrap"`, `"windows/token"`, `"none/noop"`,
    /// `"git/worktree"`). Stable identifiers — the LLM can key on these.
    pub backend: &'static str,
    /// One-word policy tier: `"danger-full-access"`, `"read-only"`,
    /// `"workspace-write"`, or `"isolated"`. Matches codex's tier names so
    /// shared docs / prompts read consistently across both ecosystems.
    pub policy_tier: &'static str,
    /// Roots the agent may write to. Sorted, deduplicated. Empty for
    /// `"read-only"`.
    pub writable_roots: Vec<PathBuf>,
    /// Whether outbound network is permitted, and to where.
    pub network: NetworkState,
    /// Hard memory ceiling on each command, if configured.
    pub max_memory_mb: Option<u64>,
}

/// Coarse network state for prompt rendering. Mirrors `NetworkPolicy` but
/// collapses `AllowHosts` into a string list at construction time so the
/// summary stays simple to serialize.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NetworkState {
    Denied,
    AllowAll,
    AllowHosts { hosts: Vec<String> },
}

impl NetworkState {
    #[must_use]
    pub fn from_policy(policy: &NetworkPolicy) -> Self {
        match policy {
            NetworkPolicy::None => Self::Denied,
            NetworkPolicy::AllowAll => Self::AllowAll,
            NetworkPolicy::AllowHosts { hosts } => Self::AllowHosts {
                hosts: hosts.clone(),
            },
        }
    }

    /// True when egress is fully blocked. Used to gate the
    /// `ALEPH_SANDBOX_NETWORK_DISABLED=1` env var on spawned children.
    #[must_use]
    pub const fn is_denied(&self) -> bool {
        matches!(self, Self::Denied)
    }
}

impl SandboxSummary {
    /// Derive a summary from a backend tag + baseline capabilities. The
    /// `policy_tier` is inferred from the capability shape:
    ///
    /// - empty `fs_write` + empty `fs_read`        → `"read-only"`
    /// - non-empty `fs_write`                      → `"workspace-write"`
    /// - `network == AllowAll && fs_write covers /` → `"danger-full-access"`
    #[must_use]
    pub fn from_baseline(backend: &'static str, caps: &SandboxCapabilities) -> Self {
        let tier = if covers_root(&caps.fs_write) && matches!(caps.network, NetworkPolicy::AllowAll)
        {
            PolicyTier::DangerFullAccess
        } else if !caps.fs_write.is_empty() {
            PolicyTier::WorkspaceWrite
        } else {
            PolicyTier::ReadOnly
        };

        let mut writable_roots: Vec<PathBuf> = caps.fs_write.to_vec();
        writable_roots.sort();
        writable_roots.dedup();

        Self {
            backend,
            policy_tier: tier.as_str(),
            writable_roots,
            network: NetworkState::from_policy(&caps.network),
            max_memory_mb: caps.max_memory_mb,
        }
    }

    /// "Isolated" tier — used by `WorktreeSandbox` which performs
    /// workspace-tree isolation (separate git worktree) without an
    /// OS-level process sandbox. The LLM should know it is NOT seatbelted.
    #[must_use]
    pub fn isolated_worktree(worktree_path: PathBuf) -> Self {
        Self {
            backend: "git/worktree",
            policy_tier: PolicyTier::Isolated.as_str(),
            writable_roots: vec![worktree_path],
            network: NetworkState::AllowAll,
            max_memory_mb: None,
        }
    }

    /// The active posture as the ordered [`PolicyTier`] enum (parsed from the
    /// tag the summary was built with). Lets callers compare postures by risk
    /// rather than string-matching.
    #[must_use]
    pub fn tier(&self) -> PolicyTier {
        PolicyTier::from_tag(self.policy_tier)
    }

    /// Render bullet lines for the system prompt. Returns one line per
    /// fact — the caller wraps them in whatever section header it prefers.
    /// Format mirrors codex's `summarize_sandbox_policy()` output.
    ///
    /// **Prompt layers must not call this.** It mixes two things with different
    /// cache lifetimes: the session-stable *posture* and the per-run *identity*
    /// (`writable_roots`, which for [`Self::isolated_worktree`] carries a fresh
    /// UUID on every isolated run). Layers take [`Self::posture_lines`] and
    /// [`Self::writable_roots_line`] separately so each lands in the zone it
    /// belongs to. This full-picture form is for operator-facing surfaces
    /// (`aleph-server sandbox-debug`) that have no cache to break.
    #[must_use]
    pub fn to_prompt_lines(&self) -> Vec<String> {
        self.lines(true)
    }

    /// The session-stable half: which enforcer, which tier, which network, which
    /// memory ceiling. Everything here is fixed for the life of the process, so
    /// it belongs in the cacheable prefix (`SecurityLayer` @600, Stable).
    #[must_use]
    pub fn posture_lines(&self) -> Vec<String> {
        self.lines(false)
    }

    /// The per-run half: *where* the agent may write.
    ///
    /// Split out of the posture because [`Self::isolated_worktree`] mints a new
    /// worktree path — with a fresh UUID — for every isolated run. Rendered from
    /// a Stable layer that path welded a run-unique byte into the cacheable
    /// prefix, so no two isolated runs (a team fan-out of N sub-agents, say)
    /// could ever share it: each one paid `cache_creation` for the whole prefix
    /// instead of `cache_read`. It now rides the dynamic tail
    /// (`OperatingEnvelopeLayer` @1758), which is the same move
    /// `ExecTier`/`SessionMode` already made out of `SecurityLayer`.
    ///
    /// `None` when the tier grants no write access (`read-only`).
    #[must_use]
    pub fn writable_roots_line(&self) -> Option<String> {
        if self.writable_roots.is_empty() {
            return None;
        }
        let paths: Vec<String> = self
            .writable_roots
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        Some(format!("Writable roots: {}", paths.join(", ")))
    }

    /// Single source for both renderings, so the operator view and the two
    /// prompt halves can never drift in wording or order.
    fn lines(&self, include_writable_roots: bool) -> Vec<String> {
        let mut lines = Vec::with_capacity(5);
        lines.push(format!("Sandbox: {} ({})", self.backend, self.policy_tier));

        // Surface the danger posture explicitly so the model is maximally
        // cautious (R9: intelligence in the prompt). Driven by the ordered
        // tier enum, not a string compare.
        if self.tier().is_danger() {
            lines.push(
                "⚠ Danger posture: full filesystem write + open network — \
                 double-check destructive or exfiltration-prone actions before running them."
                    .to_string(),
            );
        }

        if include_writable_roots {
            lines.extend(self.writable_roots_line());
        }

        let net_line = match &self.network {
            NetworkState::Denied => "Network: denied".to_string(),
            NetworkState::AllowAll => "Network: allowed (all hosts)".to_string(),
            NetworkState::AllowHosts { hosts } => {
                if hosts.is_empty() {
                    "Network: allowed (no hosts configured)".to_string()
                } else {
                    format!("Network: allowed (hosts: {})", hosts.join(", "))
                }
            }
        };
        lines.push(net_line);

        if let Some(mb) = self.max_memory_mb {
            lines.push(format!("Per-command memory ceiling: {mb} MiB"));
        }

        lines
    }
}

fn covers_root(paths: &[PathBuf]) -> bool {
    paths.iter().any(|p| p.as_os_str() == "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_tier_when_no_writable_roots() {
        let caps = SandboxCapabilities::strict();
        let summary = SandboxSummary::from_baseline("macos/seatbelt", &caps);
        assert_eq!(summary.policy_tier, "read-only");
        assert!(summary.writable_roots.is_empty());
        assert_eq!(summary.network, NetworkState::Denied);
    }

    #[test]
    fn workspace_write_tier_when_fs_write_present() {
        let caps = SandboxCapabilities {
            fs_write: vec![PathBuf::from("/workspace")],
            network: NetworkPolicy::AllowAll,
            ..Default::default()
        };
        let summary = SandboxSummary::from_baseline("linux/bwrap", &caps);
        assert_eq!(summary.policy_tier, "workspace-write");
        assert_eq!(summary.writable_roots, vec![PathBuf::from("/workspace")]);
    }

    #[test]
    fn danger_full_access_tier_when_root_writable_and_allow_all() {
        let caps = SandboxCapabilities {
            fs_write: vec![PathBuf::from("/")],
            network: NetworkPolicy::AllowAll,
            ..Default::default()
        };
        let summary = SandboxSummary::from_baseline("macos/seatbelt", &caps);
        assert_eq!(summary.policy_tier, "danger-full-access");
    }

    #[test]
    fn writable_roots_are_sorted_and_deduped() {
        let caps = SandboxCapabilities {
            fs_write: vec![
                PathBuf::from("/b"),
                PathBuf::from("/a"),
                PathBuf::from("/a"),
            ],
            ..Default::default()
        };
        let summary = SandboxSummary::from_baseline("linux/bwrap", &caps);
        assert_eq!(
            summary.writable_roots,
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
    }

    #[test]
    fn prompt_lines_contain_backend_and_tier() {
        let caps = SandboxCapabilities {
            fs_write: vec![PathBuf::from("/ws")],
            network: NetworkPolicy::AllowHosts {
                hosts: vec!["github.com".into()],
            },
            max_memory_mb: Some(512),
            ..Default::default()
        };
        let lines = SandboxSummary::from_baseline("macos/seatbelt", &caps).to_prompt_lines();
        assert!(lines[0].contains("macos/seatbelt"));
        assert!(lines[0].contains("workspace-write"));
        assert!(lines.iter().any(|l| l.contains("/ws")));
        assert!(lines.iter().any(|l| l.contains("github.com")));
        assert!(lines.iter().any(|l| l.contains("512 MiB")));
    }

    #[test]
    fn isolated_worktree_marks_no_process_sandbox() {
        let summary = SandboxSummary::isolated_worktree(PathBuf::from("/wt/a"));
        assert_eq!(summary.backend, "git/worktree");
        assert_eq!(summary.policy_tier, "isolated");
        let lines = summary.to_prompt_lines();
        assert!(lines[0].contains("git/worktree"));
        assert!(lines[0].contains("isolated"));
    }

    /// The posture half must carry no path. A worktree root is minted per
    /// isolated run, so a single path leaking into `posture_lines` puts a
    /// run-unique byte back into the Stable prefix — the exact regression this
    /// split exists to prevent, and one that shows up only on a bill.
    #[test]
    fn posture_lines_never_carry_the_per_run_writable_root() {
        let summary = SandboxSummary::isolated_worktree(PathBuf::from(
            "/wt/aleph-6f1c2e9a-4b77-4d51-9a0e-2c8b5f3d17ab",
        ));

        let posture = summary.posture_lines();
        assert!(
            !posture.iter().any(|l| l.contains("6f1c2e9a")),
            "the per-run worktree identity reached the stable posture half: {posture:?}"
        );
        assert!(posture.iter().any(|l| l.contains("git/worktree")));

        let roots = summary
            .writable_roots_line()
            .expect("an isolated worktree is writable");
        assert!(roots.contains("6f1c2e9a"));

        // The operator view stays whole, and the two halves reconstruct it in
        // the original order — so the split cannot silently drop or reorder a
        // fact behind `sandbox-debug`'s back.
        let full = summary.to_prompt_lines();
        assert_eq!(full.len(), posture.len() + 1);
        assert!(full.contains(&roots));
        assert_eq!(
            full.iter().filter(|l| **l != roots).count(),
            posture.len(),
            "full view = posture ∪ writable roots, nothing else: {full:?}"
        );
    }

    /// Read-only tiers have no writable root, so the dynamic half renders
    /// nothing at all rather than an empty bullet.
    #[test]
    fn writable_roots_line_is_none_when_read_only() {
        let summary =
            SandboxSummary::from_baseline("macos/seatbelt", &SandboxCapabilities::strict());
        assert_eq!(summary.policy_tier, "read-only");
        assert!(summary.writable_roots_line().is_none());
        assert_eq!(summary.to_prompt_lines(), summary.posture_lines());
    }

    #[test]
    fn network_denied_renders_explicitly() {
        let caps = SandboxCapabilities {
            fs_write: vec![PathBuf::from("/ws")],
            network: NetworkPolicy::None,
            ..Default::default()
        };
        let lines = SandboxSummary::from_baseline("linux/bwrap", &caps).to_prompt_lines();
        assert!(lines.iter().any(|l| l == "Network: denied"));
    }

    #[test]
    fn network_state_is_denied_helper() {
        assert!(NetworkState::Denied.is_denied());
        assert!(!NetworkState::AllowAll.is_denied());
        assert!(!NetworkState::AllowHosts { hosts: vec![] }.is_denied());
    }

    #[test]
    fn policy_tier_is_ordered_by_permissiveness() {
        assert!(PolicyTier::ReadOnly < PolicyTier::WorkspaceWrite);
        assert!(PolicyTier::WorkspaceWrite < PolicyTier::DangerFullAccess);
        assert!(PolicyTier::Isolated < PolicyTier::DangerFullAccess);
        assert!(PolicyTier::DangerFullAccess.is_danger());
        assert!(!PolicyTier::ReadOnly.is_danger());
    }

    #[test]
    fn policy_tier_tag_round_trips_and_is_single_source() {
        for tier in [
            PolicyTier::ReadOnly,
            PolicyTier::WorkspaceWrite,
            PolicyTier::Isolated,
            PolicyTier::DangerFullAccess,
        ] {
            assert_eq!(PolicyTier::from_tag(tier.as_str()), tier);
        }
        // Unknown tag fails cautious → danger (model assumes the riskiest env).
        assert_eq!(PolicyTier::from_tag("???"), PolicyTier::DangerFullAccess);
    }

    #[test]
    fn summary_tier_accessor_matches_field() {
        let caps = SandboxCapabilities {
            fs_write: vec![PathBuf::from("/ws")],
            ..Default::default()
        };
        let summary = SandboxSummary::from_baseline("linux/bwrap", &caps);
        assert_eq!(summary.tier(), PolicyTier::WorkspaceWrite);
        assert_eq!(summary.tier().as_str(), summary.policy_tier);
    }

    #[test]
    fn danger_tier_emits_caution_line() {
        let caps = SandboxCapabilities {
            fs_write: vec![PathBuf::from("/")],
            network: NetworkPolicy::AllowAll,
            ..Default::default()
        };
        let lines = SandboxSummary::from_baseline("macos/seatbelt", &caps).to_prompt_lines();
        assert!(
            lines.iter().any(|l| l.contains("Danger posture")),
            "danger tier must warn the model, got {lines:?}"
        );
        // A non-danger posture must NOT emit the caution line.
        let safe = SandboxSummary::from_baseline("linux/bwrap", &SandboxCapabilities::strict());
        assert!(!safe
            .to_prompt_lines()
            .iter()
            .any(|l| l.contains("Danger posture")));
    }
}
