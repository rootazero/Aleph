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
    /// Optional stable, audit-friendly identifier of the active permission
    /// profile. `None` for legacy / mock sandboxes (anywhere the profile-id
    /// book-keeping hasn't been wired in yet) — the bullet stays out of
    /// the prompt, and the byte stream stays byte-identical for the common
    /// path. `Some(id)` enables the `- Permission profile: <id>` line in
    /// `OperatingEnvelopeLayer` so the model (and the audit log) can tag a
    /// tool call against the exact policy it ran under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_profile_id: Option<String>,
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
            permission_profile_id: None,
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
            permission_profile_id: None,
        }
    }

    /// Builder helper: attach a permission-profile id post-construction.
    /// Keeps `from_baseline` and `isolated_worktree` free of an extra
    /// argument while still letting dispatchers opt into the
    /// `## Operating Envelope` bullet line when they wire one up.
    #[must_use]
    pub fn with_permission_profile_id(mut self, id: impl Into<String>) -> Self {
        self.permission_profile_id = Some(id.into());
        self
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

    /// The network posture, as the **only** place that sentence is produced.
    ///
    /// It used to be produced twice: [`Self::lines`] pushed it into the stable
    /// `SecurityLayer` @600 half *and* this method rendered it again into the
    /// dynamic `OperatingEnvelopeLayer` @1758 half. The duplication was
    /// defended as "the session rule and the rule this run actually applies",
    /// but both halves read the same `SandboxSummary` in the same turn, from
    /// the same field — there is no state in which they can differ, so the
    /// second copy was ~30 bytes of the same sentence on every request and a
    /// direct violation of §2.3's own rule ③ (one question, one voice).
    ///
    /// It belongs to the **dynamic** half for a second, independent reason:
    /// the posture is per-run. [`Self::isolated_worktree`] hardcodes
    /// [`NetworkState::AllowAll`] while [`Self::from_baseline`] reads config,
    /// so a session that mixes an isolated run with a normal one changes this
    /// value mid-conversation — and while it lived in the cacheable prefix,
    /// that change re-keyed the whole conversation's provider cache.
    ///
    /// Always `Some`: every [`NetworkState`] variant has a descriptor. The
    /// `Option` is kept so callers compose it with the other per-run bullets
    /// ([`Self::writable_roots_line`], [`Self::permission_profile_prompt_line`])
    /// without a special case.
    #[must_use]
    pub fn network_prompt_line(&self) -> Option<String> {
        let line = match &self.network {
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
        Some(line)
    }

    /// Every distinctive *value* this summary states, paired with the name of
    /// the fact it answers.
    ///
    /// Exists so `thinker::prompt_contract::no_environment_fact_is_stated_twice`
    /// can be driven by the type instead of by a hand-written list. The old
    /// list enumerated six `RuntimeContext` facts and no sandbox facts at all,
    /// which is exactly why the duplicated network sentence above survived four
    /// rounds of that guard being green: a guard that names its members by hand
    /// only ever covers the world as it stood on the day it was written.
    ///
    /// The exhaustive destructure is the mechanism — adding a field to
    /// [`SandboxSummary`] fails to compile here until its owner has said
    /// whether the new fact is model-visible.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn fact_census(&self) -> Vec<(&'static str, String)> {
        let Self {
            backend,
            policy_tier,
            writable_roots,
            network,
            max_memory_mb,
            permission_profile_id,
        } = self;
        let mut facts: Vec<(&'static str, String)> = vec![
            ("sandbox.backend", (*backend).to_string()),
            ("sandbox.policy_tier", (*policy_tier).to_string()),
        ];
        for root in writable_roots {
            facts.push(("sandbox.writable_root", root.display().to_string()));
        }
        facts.push((
            "sandbox.network",
            match network {
                NetworkState::Denied => "denied".to_string(),
                NetworkState::AllowAll => "allowed (all hosts)".to_string(),
                NetworkState::AllowHosts { hosts } => hosts.join(", "),
            },
        ));
        if let Some(mb) = max_memory_mb {
            facts.push(("sandbox.max_memory_mb", format!("{mb} MiB")));
        }
        if let Some(id) = permission_profile_id {
            facts.push(("sandbox.permission_profile_id", id.clone()));
        }
        facts
    }

    /// Stable permission-profile id for the active sandbox posture, when one
    /// is configured. Lets the model (and the audit log) tag a tool call
    /// against the exact policy it ran under — useful for postmortem and
    /// for cross-session comparisons when several profiles share a tier
    /// tag. `None` until a dispatcher hands a profile id in (legacy /
    /// mock sandboxes don't), so the bullet stays absent for the common
    /// path.
    #[must_use]
    pub fn permission_profile_prompt_line(&self) -> Option<String> {
        let profile_id = self.permission_profile_id.as_deref()?;
        Some(format!("Permission profile: {profile_id}"))
    }

    /// Single source for both renderings, so the operator view and the two
    /// prompt halves can never drift in wording or order.
    fn lines(&self, include_per_run: bool) -> Vec<String> {
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

        // `include_per_run` gates BOTH per-run facts. `writable_roots` carries a
        // fresh UUID on every isolated run and `network` flips with the sandbox
        // flavour, so neither may reach the cacheable prefix — and the operator
        // surface, which has no cache to break, wants both.
        if include_per_run {
            lines.extend(self.writable_roots_line());
            lines.extend(self.network_prompt_line());
        }

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

        // The network posture is per-run too — `isolated_worktree` hardcodes
        // AllowAll while `from_baseline` reads config — so it left the stable
        // half alongside the roots. Both are asserted absent from `posture`
        // and present in the operator view.
        let network = summary
            .network_prompt_line()
            .expect("every network state has a descriptor");
        assert!(
            !posture.iter().any(|l| l.starts_with("Network:")),
            "the per-run network posture reached the stable half: {posture:?}"
        );

        // The operator view stays whole, and the two halves reconstruct it in
        // the original order — so the split cannot silently drop or reorder a
        // fact behind `sandbox-debug`'s back.
        let full = summary.to_prompt_lines();
        assert_eq!(full.len(), posture.len() + 2);
        assert!(full.contains(&roots));
        assert!(full.contains(&network));
        assert_eq!(
            full.iter()
                .filter(|l| **l != roots && **l != network)
                .count(),
            posture.len(),
            "full view = posture ∪ per-run half, nothing else: {full:?}"
        );
    }

    /// The network sentence has exactly one producer.
    ///
    /// It used to have two: `lines()` pushed it into the stable
    /// `SecurityLayer` half and `network_prompt_line()` rendered it again for
    /// the dynamic `OperatingEnvelopeLayer` half — the same field, read from
    /// the same struct, in the same turn. The duplication was defended as
    /// "session rule vs the rule this run applies", which no state can make
    /// true. Asserted on the source of both halves rather than on an assembled
    /// prompt so it fails here, next to the code, rather than three layers
    /// away.
    #[test]
    fn the_network_posture_is_produced_exactly_once() {
        for summary in [
            SandboxSummary::from_baseline("macos/seatbelt", &SandboxCapabilities::strict()),
            SandboxSummary::isolated_worktree(PathBuf::from("/wt/aleph-1")),
        ] {
            let stable = summary.posture_lines();
            assert!(
                !stable.iter().any(|l| l.starts_with("Network:")),
                "the stable half restated the network posture: {stable:?}"
            );
            let full = summary.to_prompt_lines();
            assert_eq!(
                full.iter().filter(|l| l.starts_with("Network:")).count(),
                1,
                "the operator view must state the network posture exactly once: {full:?}"
            );
        }
    }

    /// Read-only tiers have no writable root, so the dynamic half renders
    /// nothing at all rather than an empty bullet.
    #[test]
    fn writable_roots_line_is_none_when_read_only() {
        let summary =
            SandboxSummary::from_baseline("macos/seatbelt", &SandboxCapabilities::strict());
        assert_eq!(summary.policy_tier, "read-only");
        assert!(summary.writable_roots_line().is_none());
        // The per-run half is then just the network posture: the full view is
        // the stable half plus that one line, and nothing else.
        let network = summary
            .network_prompt_line()
            .expect("a strict baseline still states `Network: denied`");
        let mut expected = summary.posture_lines();
        expected.push(network);
        assert_eq!(summary.to_prompt_lines(), expected);
    }

    #[test]
    fn network_prompt_line_renders_all_three_variants() {
        // Three variants, three wordings — single source so the
        // `OperatingEnvelopeLayer` and `sandbox-debug` cannot drift.
        let denied = SandboxSummary {
            backend: "b",
            policy_tier: "read-only",
            writable_roots: vec![],
            network: NetworkState::Denied,
            max_memory_mb: None,
            permission_profile_id: None,
        };
        assert_eq!(
            denied.network_prompt_line().as_deref(),
            Some("Network: denied")
        );

        let allow_all = SandboxSummary {
            backend: "b",
            policy_tier: "danger-full-access",
            writable_roots: vec![],
            network: NetworkState::AllowAll,
            max_memory_mb: None,
            permission_profile_id: None,
        };
        assert_eq!(
            allow_all.network_prompt_line().as_deref(),
            Some("Network: allowed (all hosts)")
        );

        let empty_hosts = SandboxSummary {
            backend: "b",
            policy_tier: "workspace-write",
            writable_roots: vec![],
            network: NetworkState::AllowHosts { hosts: vec![] },
            max_memory_mb: None,
            permission_profile_id: None,
        };
        assert_eq!(
            empty_hosts.network_prompt_line().as_deref(),
            Some("Network: allowed (no hosts configured)")
        );
    }

    #[test]
    fn permission_profile_prompt_line_is_none_then_some_after_with() {
        let summary = SandboxSummary::from_baseline("b", &SandboxCapabilities::strict());
        assert!(summary.permission_profile_prompt_line().is_none());
        let summary = summary.with_permission_profile_id("drafts-v1");
        assert_eq!(
            summary.permission_profile_prompt_line().as_deref(),
            Some("Permission profile: drafts-v1")
        );
    }

    #[test]
    fn with_permission_profile_id_round_trips_on_worktree_path() {
        // The `isolated_worktree` constructor leaves `permission_profile_id`
        // `None`; the builder helper is the only way to set it. Pin both
        // halves of the round-trip so a refactor cannot silently drop one.
        let summary = SandboxSummary::isolated_worktree(PathBuf::from("/wt/aleph-foo"))
            .with_permission_profile_id("iso-v2");
        assert_eq!(summary.permission_profile_id.as_deref(), Some("iso-v2"));
        assert_eq!(
            summary.permission_profile_prompt_line().as_deref(),
            Some("Permission profile: iso-v2")
        );
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
