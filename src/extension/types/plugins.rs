//! Plugin types
//!
//! Core data structures for plugin management, discovery, and lifecycle.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// =============================================================================
// Plugin Types
// =============================================================================

/// Plugin info for display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub enabled: bool,
    pub path: String,
    pub skills_count: usize,
    pub commands_count: usize,
    pub agents_count: usize,
    pub hooks_count: usize,
    pub mcp_servers_count: usize,
    /// Tools this plugin registers. Load-bearing beyond display: zero means
    /// tool-call accounting has nothing to observe for this plugin, so its
    /// usage must render as `—` rather than `0`.
    #[serde(default)]
    pub tools_count: usize,
    /// Runtime kind: "static" | "mcp" | "wasm". Every client that lists
    /// plugins wanted this column; the server never sent it, so the column
    /// rendered a dash on every row.
    #[serde(default)]
    pub kind: String,
    /// Runtime status label — see
    /// [`aleph_protocol::plugins::PluginRuntimeStatus`], which is the wire
    /// vocabulary this string must stay inside.
    #[serde(default)]
    pub status: String,
    /// Error message when the plugin failed to load (status == "error").
    #[serde(default)]
    pub error: Option<String>,
}

/// Plugin origin - where the plugin was discovered from
///
/// Higher priority origins override lower priority ones when plugins
/// have the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginOrigin {
    /// From explicit config (highest priority)
    Config,
    /// From workspace .aleph/ directory
    Workspace,
    /// From global ~/.aleph/ directory
    Global,
    /// Bundled with core (lowest priority)
    Bundled,
}

impl PluginOrigin {
    /// Get the priority of this origin (higher = takes precedence)
    #[must_use]
    pub const fn priority(&self) -> u8 {
        match self {
            Self::Config => 4,
            Self::Workspace => 3,
            Self::Global => 2,
            Self::Bundled => 1,
        }
    }

    /// Classify a plugin by where it was found on disk.
    ///
    /// # Why this exists
    ///
    /// Every manifest adapter hardcoded `PluginOrigin::Global`. All six of
    /// them, at their construction sites — so `Bundled` and `Config` had no
    /// producer and the *only* value this enum ever took was `Global`.
    ///
    /// That was invisible while the enum's only consumer was `priority()`,
    /// where a constant answer looks like a working shadowing rule, and it
    /// stopped being invisible the moment `OwnerTrustPolicy` gained a
    /// producer: an origin nothing produces cannot be exempted, and
    /// `PluginInfo.origin` reached every client as a constant.
    ///
    /// One derivation, from the scanner's own classification -- which
    /// `collect_plugin_dirs` used to drop on the floor before anything could
    /// read it.
    ///
    /// # `Bundled` and `Config` still have no producer, deliberately
    ///
    /// [`crate::extension::plugin_trust::OwnerTrustPolicy::allows`] exempts
    /// both, so it is worth being exact about why neither is returned here.
    /// Aleph's shipped plugins are extracted into the official *marketplace
    /// cache* (`<aleph_home>/plugins/cache/aleph-official/<id>`), which the
    /// scanner never reaches: it descends one level below a plugin parent and
    /// that path is two. They become loadable only by being installed, which
    /// copies them into `plugins/installed/` -- genuinely `Global`, and
    /// genuinely something an operator chose to install.
    ///
    /// Classifying that cache path as `Bundled` would therefore be a branch no
    /// discovered plugin can take (R10). Enforcement means what it says with
    /// no asterisk: every plugin needs an explicit vouch, and refused ones are
    /// listed by id so there is something to vouch for.
    #[must_use]
    pub const fn classify(source: crate::discovery::DiscoverySource) -> Self {
        match source {
            // A project-local `.aleph/plugins` tree. NOT exempt from the trust
            // policy -- a checked-out repository can write that directory,
            // which is precisely the case the policy exists for. The
            // distinction earns its keep in the refusal message, which can
            // then say where the plugin came from.
            crate::discovery::DiscoverySource::Project => Self::Workspace,
            crate::discovery::DiscoverySource::AlephGlobal
            | crate::discovery::DiscoverySource::ClaudeGlobal => Self::Global,
        }
    }
}

/// Plugin kind - the type/format of the plugin
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginKind {
    /// WebAssembly plugin (.wasm)
    Wasm,
    /// MCP server plugin (.mcp.json) — uses Aleph's MCP client system
    Mcp,
    /// Static content plugin (markdown files)
    Static,
}

impl PluginKind {
    /// Every runtime this host can load, in wire spelling.
    ///
    /// Ordered to match [`aleph_protocol::plugins::PLUGIN_RUNTIMES`]; the test
    /// below holds them equal. The vocabulary used to live in three places
    /// that disagreed — see that constant's doc for what it cost.
    pub const ALL: [Self; 3] = [Self::Wasm, Self::Mcp, Self::Static];

    /// Stable lowercase wire label, matching the serde representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wasm => "wasm",
            Self::Mcp => "mcp",
            Self::Static => "static",
        }
    }

    /// Detect plugin kind from a file path
    ///
    /// Returns `Some(kind)` if the path indicates a known plugin type,
    /// `None` otherwise.
    #[must_use]
    pub fn detect_from_path(path: &Path) -> Option<Self> {
        let filename = path.file_name()?.to_str()?;
        let ext = path.extension().and_then(|e| e.to_str());

        match (filename, ext) {
            (_, Some("wasm")) => Some(Self::Wasm),
            (".mcp.json", _) => Some(Self::Mcp),
            ("aleph.plugin.json", _) => Some(Self::Wasm),
            ("SKILL.md" | "COMMAND.md" | "AGENT.md", _) => Some(Self::Static),
            (_, Some("md")) => Some(Self::Static),
            _ => None,
        }
    }
}

/// Plugin status - the runtime state of a plugin
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginStatus {
    /// Plugin is loaded and active
    Loaded,
    /// Plugin is disabled by user
    Disabled,
    /// Plugin failed to load with an error
    Error(String),
    /// The owner trust policy refused this plugin's origin.
    ///
    /// Distinct from [`Self::Disabled`] on purpose: the remedy is an allowlist
    /// entry, not the per-plugin toggle. Collapsing the two would point the
    /// operator at a switch that cannot change the outcome.
    Blocked(String),
    /// Mounted, but a declared dependency has not reached its terminal state:
    /// the MCP manager is not attached yet, a declared MCP server has not
    /// answered `initialize`, a required runtime is not provisioned.
    ///
    /// `waiting_on` is derived from the plugin's declared MCP servers and
    /// their start reports (`extension::readiness::derive_readiness`), never
    /// hand-written, and is never empty. It changes only when a dependency
    /// reports — there is no timer that turns this into [`Self::Error`]
    /// (判据 §8: "not ready" is not "failed"). The boot activation gate and the
    /// `extension/plugins-activated` doctor check list every plugin still
    /// here after boot, with this field as the reason.
    Pending { waiting_on: Vec<String> },
}

impl PluginStatus {
    /// Whether the plugin's registered capabilities are live.
    ///
    /// `Pending` is active: the capabilities that do not depend on the
    /// outstanding dependency (skills, agents, hooks, commands) are already
    /// registered and usable; the ones that do (the pending server's tools)
    /// are simply not there yet. Making `Pending` inactive would unregister
    /// and re-register everything else around a slow `initialize`.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        matches!(self, Self::Loaded | Self::Pending { .. })
    }

    /// Whether this is a state the plugin can stay in: everything except
    /// [`Self::Pending`]. Written as an exhaustive `match` with no wildcard
    /// so that a new variant is a compile error HERE, where a human decides
    /// whether it is terminal — not a silent "true". The activation gate and
    /// the doctor check derive their "still waiting" set from this.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        match self {
            Self::Loaded | Self::Disabled | Self::Error(_) | Self::Blocked(_) => true,
            Self::Pending { .. } => false,
        }
    }

    /// Stable lowercase label for client display / serialization — the wire
    /// vocabulary of [`aleph_protocol::plugins::PluginRuntimeStatus`].
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Loaded => "loaded",
            Self::Disabled => "disabled",
            Self::Error(_) => "error",
            Self::Blocked(_) => "blocked",
            Self::Pending { .. } => "pending",
        }
    }
}

// =============================================================================
// Load Summary
// =============================================================================

/// Summary of extension loading returned by `ExtensionManager::load_all()`
#[derive(Debug, Default)]
pub struct LoadSummary {
    /// Number of skills loaded
    pub skills_loaded: usize,
    /// Number of commands loaded
    pub commands_loaded: usize,
    /// Number of agents loaded
    pub agents_loaded: usize,
    /// Number of plugins loaded
    pub plugins_loaded: usize,
    /// Number of hooks loaded
    pub hooks_loaded: usize,
    /// Number of plugins the owner trust policy refused to load.
    ///
    /// The doc here used to claim this was "surfaced in `extensions.stat`".
    /// It was not surfaced anywhere — the field had zero consumers, so a
    /// policy refusal was invisible on every face. The refusals now carry
    /// their own registry rows (`PluginStatus::Blocked`, listed by
    /// `plugins.list` with the allowlist hint in `status_detail`); this
    /// counter is the aggregate for the boot log, not the operator's only
    /// window onto it.
    pub skipped_by_trust: usize,
    /// Number of plugin directories that lost a same-id shadow contest to a
    /// higher-priority scope.
    pub shadowed: usize,
    /// Number of plugins registered but held inactive because the operator
    /// disabled them (`<data_dir>/plugins.toml`). Distinct from
    /// [`Self::skipped_by_trust`]: that is a policy refusal, this is a
    /// deliberate per-plugin toggle, and the two need different remedies.
    pub disabled_by_operator: usize,
    /// Errors encountered during loading
    pub errors: Vec<String>,
}

impl LoadSummary {
    /// Check if loading was successful (no errors)
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.errors.is_empty()
    }

    /// Total components loaded
    #[must_use]
    pub const fn total_loaded(&self) -> usize {
        self.skills_loaded + self.commands_loaded + self.agents_loaded + self.plugins_loaded
    }
}

// =============================================================================
// Plugin Record
// =============================================================================

/// Installation scope for plugins.
///
/// Determines where a plugin is installed and its resolution priority
/// when multiple scopes define the same plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginScope {
    /// Installed for the current user (~/.aleph/plugins/)
    User,
    /// Installed for the current project (.aleph/plugins/)
    Project,
    /// Installed locally for development (symlinked or path-based)
    Local,
}

impl PluginScope {
    /// Get the resolution priority of this scope (higher = takes precedence).
    #[must_use]
    pub const fn priority(&self) -> u8 {
        match self {
            Self::Local => 3,
            Self::Project => 2,
            Self::User => 1,
        }
    }
}

impl std::fmt::Display for PluginScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User => write!(f, "user"),
            Self::Project => write!(f, "project"),
            Self::Local => write!(f, "local"),
        }
    }
}

/// Plugin record - comprehensive plugin information for registry tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRecord {
    /// Unique plugin identifier
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Version string (semver)
    pub version: Option<String>,
    /// Plugin description
    pub description: Option<String>,
    /// Plugin type/format
    pub kind: PluginKind,
    /// Discovery origin
    pub origin: PluginOrigin,
    /// Current status
    pub status: PluginStatus,
    /// Human-readable detail for a non-`Loaded` status: the parse error, the
    /// path that shadowed this plugin, or the policy that refused it.
    ///
    /// A status without a detail names a problem and no remedy, which is the
    /// half the operator actually needs.
    pub error: Option<String>,
    /// Root directory of the plugin
    pub root_dir: PathBuf,
    /// Who may see this plugin (`extension::visibility`). Stamped by
    /// `lifecycle.rs::build_record` from the discovery that found the
    /// directory — the adapters cannot know it, exactly as they cannot know
    /// `origin` (the `record.origin = origin` line beside it). `PluginRecord::new`
    /// / `from_adapter_output` start it at `Global` for the same reason they
    /// start `origin` at the adapter's placeholder; `load_all` overwrites it
    /// on every row it registers, including error rows.
    pub scope_key: crate::extension::visibility::ScopeKey,
    // Registration tracking
    /// Tool names registered by this plugin
    pub tool_names: Vec<String>,
    /// Number of hooks registered
    pub hook_count: usize,
    /// Service IDs registered by this plugin
    pub service_ids: Vec<String>,
    /// Number of skills declared by this plugin
    #[serde(default)]
    pub skill_count: usize,
    /// Number of in-chat commands declared by this plugin
    #[serde(default)]
    pub command_count: usize,
    /// Number of agents declared by this plugin
    #[serde(default)]
    pub agent_count: usize,
    /// Number of MCP servers declared by this plugin
    #[serde(default)]
    pub mcp_server_count: usize,
}

impl PluginRecord {
    /// Create a new plugin record with default values
    #[must_use]
    pub const fn new(id: String, name: String, kind: PluginKind, origin: PluginOrigin) -> Self {
        Self {
            id,
            name,
            version: None,
            description: None,
            kind,
            origin,
            status: PluginStatus::Loaded,
            error: None,
            root_dir: PathBuf::new(),
            scope_key: crate::extension::visibility::ScopeKey::Global,
            tool_names: Vec::new(),
            hook_count: 0,
            service_ids: Vec::new(),
            skill_count: 0,
            command_count: 0,
            agent_count: 0,
            mcp_server_count: 0,
        }
    }

    /// Create a `PluginRecord` from an `AdapterOutput`.
    ///
    /// Populates metadata from the adapter output and derives tool/hook counts
    /// from the declared capabilities.
    #[must_use]
    pub fn from_adapter_output(
        output: &crate::extension::manifest::adapter::AdapterOutput,
        root_dir: PathBuf,
    ) -> Self {
        use crate::extension::capability::CapabilityDeclaration;

        let mut tool_names = Vec::new();
        let mut hook_count = 0;
        let mut service_ids = Vec::new();
        let mut skill_count = 0;
        let mut command_count = 0;
        let mut agent_count = 0;
        let mut mcp_server_count = 0;

        for cap in &output.capabilities {
            match cap {
                CapabilityDeclaration::Tool(t) => tool_names.push(t.name.clone()),
                CapabilityDeclaration::Hook(_) => hook_count += 1,
                CapabilityDeclaration::Service(s) => service_ids.push(s.id.clone()),
                // Skills and `commands/`-derived commands share the Skill
                // variant, split by `skill_type`.
                CapabilityDeclaration::Skill(s) => {
                    if s.skill_type == crate::extension::types::SkillType::Command {
                        command_count += 1;
                    } else {
                        skill_count += 1;
                    }
                }
                CapabilityDeclaration::Agent(_) => agent_count += 1,
                CapabilityDeclaration::McpServer(_) => mcp_server_count += 1,
            }
        }

        Self {
            id: output.plugin_id.clone(),
            name: output
                .name
                .clone()
                .unwrap_or_else(|| output.plugin_id.clone()),
            version: output.version.clone(),
            description: output.description.clone(),
            kind: PluginKind::Static, // default; caller can override
            origin: output.source.origin,
            status: PluginStatus::Loaded,
            error: None,
            root_dir,
            scope_key: crate::extension::visibility::ScopeKey::Global,
            tool_names,
            hook_count,
            service_ids,
            skill_count,
            command_count,
            agent_count,
            mcp_server_count,
        }
    }

    /// Set an error status with message
    #[must_use]
    pub fn with_error(mut self, error: String) -> Self {
        self.status = PluginStatus::Error(error.clone());
        self.error = Some(error);
        self
    }

    /// Mark this record inactive with a stated reason.
    ///
    /// The reason lands in both the status (machine-readable) and
    /// [`Self::error`] (human-readable), because every one of these outcomes
    /// used to be expressed by *dropping the plugin from the registry* — which
    /// renders identically to "never installed".
    #[must_use]
    pub fn inactive(mut self, status: PluginStatus, detail: String) -> Self {
        debug_assert!(
            !status.is_active(),
            "`inactive` is for non-Loaded statuses only"
        );
        self.status = status;
        self.error = Some(detail);
        self
    }

    /// Mark this record pending on the named dependencies (sorted, deduped).
    ///
    /// Not `inactive`: a pending plugin's other capabilities stay live (see
    /// [`PluginStatus::is_active`]). The detail is the operator's window onto
    /// WHAT is being waited for, so it lists every entry.
    ///
    /// The empty-list check below is `assert!`, not `inactive`'s
    /// `debug_assert!`: an empty `waiting_on` is not "an unusual status to
    /// pass" the way a caller-supplied `PluginStatus` might be, it is a
    /// contradiction in terms ("pending on nothing" — 判据 §2, a predicate
    /// that can never go red). No runtime-data path calls this with a list it
    /// did not itself derive as non-empty; if one ever does, that is a
    /// programmer error this build should catch, release or not.
    #[must_use]
    pub fn with_pending(mut self, mut waiting_on: Vec<String>) -> Self {
        assert!(
            !waiting_on.is_empty(),
            "waiting_on must name at least one dependency"
        );
        waiting_on.sort();
        waiting_on.dedup();
        self.error = Some(format!("waiting on {}", waiting_on.join(", ")));
        self.status = PluginStatus::Pending { waiting_on };
        self
    }

    /// Set the root directory
    #[must_use]
    pub fn with_root_dir(mut self, path: PathBuf) -> Self {
        self.root_dir = path;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{PluginKind, PluginOrigin, PluginRecord, PluginStatus};

    #[test]
    fn pending_is_active_and_labelled_and_carries_its_dependencies() {
        let s = PluginStatus::Pending {
            waiting_on: vec!["mcp:plugin:x/srv".into()],
        };
        assert!(
            s.is_active(),
            "a pending plugin's other capabilities are live"
        );
        assert_eq!(s.label(), "pending");
        let rec = PluginRecord::new(
            "x".into(),
            "X".into(),
            PluginKind::Mcp,
            PluginOrigin::Global,
        )
        .with_pending(vec!["mcp:plugin:x/srv".into(), "mcp:plugin:x/aux".into()]);
        assert!(matches!(rec.status, PluginStatus::Pending { .. }));
        assert_eq!(
            rec.error.as_deref(),
            Some("waiting on mcp:plugin:x/aux, mcp:plugin:x/srv"),
            "the operator-facing detail names every dependency"
        );
    }

    /// `with_pending` refuses an empty list: "pending on nothing" is not a
    /// state, it is a predicate that can never go red (判据 §2).
    #[test]
    #[should_panic(expected = "waiting_on must name at least one dependency")]
    fn pending_on_nothing_is_rejected() {
        let _ = PluginRecord::new(
            "x".into(),
            "X".into(),
            PluginKind::Mcp,
            PluginOrigin::Global,
        )
        .with_pending(Vec::new());
    }
}

#[cfg(test)]
mod runtime_vocabulary_tests {
    use super::PluginKind;

    /// The server's serde vocabulary and the wire contract must be the same
    /// list. When they were not, `aleph plugin init --type nodejs` produced a
    /// manifest the server rejected with `unknown variant`.
    #[test]
    fn plugin_kinds_match_the_wire_contract() {
        let ours: Vec<&str> = PluginKind::ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(
            ours,
            aleph_protocol::plugins::PLUGIN_RUNTIMES.to_vec(),
            "PluginKind::ALL and PLUGIN_RUNTIMES are the same vocabulary"
        );
    }

    /// `as_str` must round-trip through serde, or the wire label and the
    /// stored value diverge silently.
    #[test]
    fn every_kind_round_trips_through_its_wire_label() {
        for kind in PluginKind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            let back: PluginKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, kind);
        }
    }
}
