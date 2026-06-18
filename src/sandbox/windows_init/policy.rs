//! Cross-platform policy surface for the `sandbox-init-windows`
//! subcommand: the `WindowsInitPolicy` JSON struct, the
//! `NetworkPolicy` → AppContainer capability translation, the DACL
//! constants, and the protected-metadata classifier.
//!
//! None of this is `cfg`-gated, so it compiles + unit-tests on macOS /
//! Linux dev boxes. Only the Windows [`super::imp`] consumer turns the
//! constants and classifier into Win32 calls.

use serde::{Deserialize, Serialize};

/// Policy passed from `WindowsSandboxDriver::run` to `sandbox-init-windows`
/// via JSON on argv. Bounded by capability count.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WindowsInitPolicy {
    /// When `true`, the init exits non-zero if `CreateProcessAsUserW`
    /// fails with `ERROR_PRIVILEGE_NOT_HELD` (host lacks
    /// `SE_INCREASE_QUOTA`). Default `false` → soft-degrade to
    /// `CreateProcessW` with the host token (cycle 1 behavior).
    /// `JobObject` containment continues to apply either way.
    #[serde(default)]
    pub require_restricted_token: bool,

    /// SP-6: try `AppContainer` first (strongest sandbox primitive).
    /// Soft-degrades to restricted-token (SP-3a) on failure.
    #[serde(default)]
    pub use_app_container: bool,

    /// SP-6: when `true`, refuse to spawn if `AppContainer` setup fails.
    /// Default `false` → soft-degrade to SP-3a's path.
    #[serde(default)]
    pub require_app_container: bool,

    /// SP-6: capability names (lowercase Win32 form like
    /// `internetClient`) to grant inside the `AppContainer`. Empty list
    /// = "no capabilities". Translated to SIDs via
    /// `DeriveCapabilitySidsFromName` at init time.
    #[serde(default)]
    pub app_container_capabilities: Vec<String>,

    /// SP-6: absolute path to the session workspace dir. SP-6 adds an
    /// Allow-Modify ACE for the per-execution `AppContainer` SID on this
    /// directory before spawn so the target can read/write its
    /// workspace. `None` → no DACL grant (target may fail on writes).
    #[serde(default)]
    pub workspace_path: Option<String>,

    /// Cycle 7: git-style globs (e.g. `**/.env`, `**/*.pem`, `**/.ssh`)
    /// identifying secret paths the sandboxed target must NOT be able to
    /// read, even though they live inside the otherwise-readable
    /// workspace. The init resolves each glob against `workspace_path`
    /// and stamps a `DENY_ACCESS` read ACE for the per-execution
    /// `AppContainer` SID on every match — the Windows analogue of the
    /// macOS seatbelt `deny_read_globs` floor (and codex's
    /// `deny_read_acl`). Empty list → no deny-read pass → byte-identical
    /// to the pre-Cycle-7 behaviour.
    ///
    /// Enforced only on the `AppContainer` path (the default): the
    /// restricted-token path shares the host user SID, so a per-SID deny
    /// would also lock out the parent. With `use_app_container = true`
    /// (the default) the common path is covered.
    #[serde(default)]
    pub deny_read_globs: Vec<String>,
}

/// Translate `NetworkPolicy` → `AppContainer` capability names. Lives at
/// crate top so it's testable cross-platform.
#[must_use]
pub fn capability_names_for_network(
    net: &crate::sandbox::capabilities::NetworkPolicy,
) -> Vec<String> {
    use crate::sandbox::capabilities::NetworkPolicy;
    match net {
        NetworkPolicy::None => Vec::new(),
        NetworkPolicy::AllowAll => vec![
            "internetClient".to_string(),
            "privateNetworkClientServer".to_string(),
        ],
        // AllowHosts is rejected at WindowsSandboxDriver::profile_for time
        // (cycle 1 / SP-3b). If we ever get here we're conservative.
        NetworkPolicy::AllowHosts { .. } => Vec::new(),
    }
}

/// SP-6 v2: DACL inheritance flags applied to `AppContainer` workspace
/// grants. `CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE` so the ACE
/// propagates to existing children whose default DACL inheritance is
/// enabled (the NTFS default) plus all future children.
///
/// MSDN documents `OBJECT_INHERIT_ACE = 0x1`,
/// `CONTAINER_INHERIT_ACE = 0x2`. Hard-coded so this constant — and
/// the regression test for it — work on the macOS / Linux dev boxes
/// without dragging in Win32 headers.
// Intentionally kept non-cfg-gated so the regression test below
// compiles on all platforms.  The Win32 consumer is Windows-only, so
// this triggers dead_code on non-Windows dev boxes.
#[allow(dead_code)]
pub(crate) const DACL_INHERIT_FLAGS_FOR_APPCONTAINER: u32 = 0x2 | 0x1;

/// Cycle 8: name of the OS mutex that serializes the workspace DACL
/// read-modify-write across concurrent `sandbox-init-windows` processes.
///
/// Aleph is a multi-agent system, so several inits can run at once against
/// the *same* workspace — sharing the `.git` / `.aleph` metadata subpaths
/// and any deny-read targets. `set_workspace_dacl_entry` mutates a path's
/// DACL with a non-atomic `GetNamedSecurityInfoW → SetEntriesInAclW →
/// SetNamedSecurityInfoW` sequence; without serialization two inits race on
/// the shared path's DACL (init B reads the DACL before init A writes its
/// ACE, so A's ACE is lost when B writes back). Dropping a per-execution
/// *deny* ACE is the dangerous case: a `.git` that should be read-only for
/// init A's `AppContainer` SID silently becomes writable.
///
/// We close the window exactly as codex does with its
/// `Local\CodexSandboxReadAcl` named mutex. `Local\` scope is correct here:
/// every aleph sandbox-init is a child of the one aleph-server daemon in a
/// single logon session, and `Global\` would demand the
/// `SeCreateGlobalPrivilege` that a standard user lacks. Kept at crate top
/// (non-gated) so the regression test compiles on macOS / Linux dev boxes
/// alongside the rest of this module's cross-platform surface.
#[allow(dead_code)]
pub(crate) const DACL_SERIALIZATION_MUTEX_NAME: &str = "Local\\Aleph.Sandbox.WorkspaceDacl";

/// Cycle 5: one protected-metadata subpath under a workspace root,
/// tagged with whether it was absent on disk at classification time.
#[allow(dead_code)]
pub(crate) struct MetadataTarget {
    /// Absolute path of the protected subpath (`<ws>/.git`, …).
    pub path: std::path::PathBuf,
    /// `true` when the path did not exist. The Windows ACE stamper
    /// pre-creates an empty stub directory for every absent path before
    /// applying its deny ACE — otherwise the sandboxed process could
    /// `mkdir` the directory itself and inherit the workspace root's
    /// `GENERIC_ALL` grant.
    pub absent: bool,
}

/// Cycle 3 + Cycle 5: resolve the four protected-metadata subpaths
/// under `workspace_root`, each tagged with on-disk existence. Cross-
/// platform on purpose so the partition logic unit-tests on macOS /
/// Linux dev boxes; only the Windows ACE/stub stamper consumes it.
///
/// Cycle 3 only protected children that already existed, because
/// `SetNamedSecurityInfoW` fails with `ERROR_FILE_NOT_FOUND` on a
/// missing target. Cycle 5 keeps the absent ones too so the Windows
/// stamper can pre-create an empty stub directory for each — closing
/// the gap where a sandboxed process `mkdir`s `.git` and inherits the
/// workspace root's inherited `GENERIC_ALL`.
#[allow(dead_code)]
pub(crate) fn classify_protected_metadata(workspace_root: &std::path::Path) -> Vec<MetadataTarget> {
    crate::sandbox::protected_paths::PROTECTED_METADATA_SUBPATHS
        .iter()
        .map(|sub| {
            let path = workspace_root.join(sub);
            let absent = !path.exists();
            MetadataTarget { path, absent }
        })
        .collect()
}
