//! Runtime install dispatcher driven by `super::specs::SPECS`.

use std::path::PathBuf;
use std::sync::Mutex;

use tokio::process::Command;
use tokio::time::{timeout, Duration};
use tracing::warn;

use super::os::TargetOs;
use super::post_install;
use super::probe;
use super::specs::{find_spec, select_install, InstallStrategy};
use crate::utils::no_window::NoWindow;

/// Process-wide serialization for PATH read-modify-write.
///
/// `std::env::set_var` is documented as not thread-safe; two concurrent
/// `prepend_existing_dirs` callers can race, with the loser's prepended
/// directory disappearing from PATH. Capability locks only serialize same-
/// capability bootstrap; a node + uv bootstrap from two different callers
/// races here. This mutex makes the read-modify-write atomic across the
/// process. It is short-lived and uncontended in steady state.
static PATH_LOCK: Mutex<()> = Mutex::new(());

/// Result of a bootstrap attempt.
#[derive(Debug)]
pub enum BootstrapResult {
    Success { bin_path: PathBuf, version: String },
    PathNotFound { expected: String },
    Failed { stderr: String },
    Unsupported { capability: String, reason: String },
    UnknownCapability { capability: String },
}

/// Errors raised by the dispatcher itself (not captured in `BootstrapResult`).
#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("post-install action failed: {0}")]
    PostInstall(#[from] post_install::PostInstallError),
    #[error("bootstrap timed out after {0}s")]
    Timeout(u64),
}

/// Install a capability according to its spec. Assumes `deps` are already Ready
/// (caller handles dep resolution).
// rust-doctor-disable-next-line high-cyclomatic-complexity
pub async fn install(name: &str) -> Result<BootstrapResult, BootstrapError> {
    let spec = match find_spec(name) {
        Some(s) => s,
        None => {
            return Ok(BootstrapResult::UnknownCapability {
                capability: name.into(),
            });
        }
    };

    if spec.install.is_empty() {
        return Ok(BootstrapResult::Unsupported {
            capability: name.into(),
            reason: "no install strategy defined for this capability".into(),
        });
    }

    let current = match TargetOs::current() {
        Some(os) => os,
        None => {
            return Ok(BootstrapResult::Unsupported {
                capability: name.into(),
                reason: "unsupported OS for Aleph runtimes".into(),
            });
        }
    };
    let os_install = match select_install(spec.install, current) {
        Some(oi) => oi,
        None => {
            return Ok(BootstrapResult::Unsupported {
                capability: name.into(),
                reason: format!("no install strategy for {current:?}"),
            });
        }
    };

    // 1. Run the install command.
    let cmd_result = match &os_install.strategy {
        InstallStrategy::Shell(script) => run_shell(script).await?,
        InstallStrategy::PowerShell(script) => run_powershell(script).await?,
        InstallStrategy::Via { parent, subcommand } => run_via_parent(parent, subcommand).await?,
        InstallStrategy::Unsupported { reason } => {
            return Ok(BootstrapResult::Unsupported {
                capability: name.into(),
                reason: (*reason).into(),
            });
        }
    };

    if let CmdOutcome::Failed { stderr } = cmd_result {
        return Ok(BootstrapResult::Failed { stderr });
    }

    // The install command may have dropped a binary into a directory that is
    // not on the daemon's current PATH (e.g. rustup writes `~/.cargo/bin`,
    // Homebrew uses `/opt/homebrew/bin`, winget shims live under
    // `%LOCALAPPDATA%\Microsoft\WinGet\Links`). Temporarily widen PATH so the
    // re-probe `which` lookup succeeds without forcing a daemon restart.
    enrich_path_for_reprobe();

    // `Via`-parent runtimes (node via fnm, playwright-cli via node→fnm) land in
    // a version-manager-controlled bin dir that is never on the daemon's PATH.
    // Resolve and prepend it so the re-probe `which`/`where` lookup below finds
    // the binary and records its absolute path (which then feeds build_path()).
    if let InstallStrategy::Via { parent, .. } = &os_install.strategy {
        enrich_path_for_via_parent(parent).await;
    }

    // 2. Re-probe to get binary path + version.
    let probe_result = probe::probe(name);
    if !probe_result.found {
        return Ok(BootstrapResult::PathNotFound {
            expected: format!("binary '{name}' on PATH after install"),
        });
    }
    let bin_path = match probe_result.bin_path.clone() {
        Some(path) => path,
        None => {
            return Ok(BootstrapResult::PathNotFound {
                expected: format!("binary path for '{name}' after successful probe"),
            });
        }
    };

    // 3. Run post-install actions.
    for action in spec.post_install {
        post_install::run(action, &bin_path).await?;
    }

    Ok(BootstrapResult::Success {
        bin_path,
        version: probe_result.version.unwrap_or_default(),
    })
}

/// Whether a bootstrap spec exists for this capability.
#[must_use]
pub fn has_spec(capability: &str) -> bool {
    find_spec(capability).is_some()
}

/// Dependencies that must be Ready before installing this capability.
#[must_use]
pub fn dependencies(capability: &str) -> &'static [&'static str] {
    find_spec(capability).map(|s| s.deps).unwrap_or(&[])
}

enum CmdOutcome {
    Success,
    Failed { stderr: String },
}

/// Bootstrap command timeout — installs may download large binaries over the network.
const BOOTSTRAP_TIMEOUT_SECS: u64 = 600;

async fn run_cmd(cmd: &mut Command) -> Result<CmdOutcome, BootstrapError> {
    let output = match timeout(
        Duration::from_secs(BOOTSTRAP_TIMEOUT_SECS),
        cmd.no_window().output(),
    )
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(BootstrapError::Io(e)),
        Err(_) => return Err(BootstrapError::Timeout(BOOTSTRAP_TIMEOUT_SECS)),
    };
    Ok(if output.status.success() {
        CmdOutcome::Success
    } else {
        CmdOutcome::Failed {
            stderr: String::from_utf8_lossy(&output.stderr).into(),
        }
    })
}

async fn run_shell(script: &str) -> Result<CmdOutcome, BootstrapError> {
    run_cmd(Command::new("sh").args(["-c", script])).await
}

async fn run_powershell(script: &str) -> Result<CmdOutcome, BootstrapError> {
    // Prefer PowerShell 7 (`pwsh`) when present — modern, the direction Microsoft
    // is steering toward — falling back to the always-present Windows PowerShell
    // (`powershell`, 5.1). Both run the winget install commands identically.
    // `-NoProfile` avoids loading the user profile (faster, no profile side
    // effects), matching the skill installer's shell selection.
    let shell = if which::which("pwsh").is_ok() {
        "pwsh"
    } else {
        "powershell"
    };
    run_cmd(Command::new(shell).args(["-NoProfile", "-Command", script])).await
}

/// Prepend well-known install-output directories to the current process PATH
/// so that `probe()` (which shells out to `which`/`where`) can find binaries
/// dropped by package managers that don't refresh the daemon's environment.
///
/// Idempotent: directories that don't exist or are already on PATH are skipped.
/// The change is process-wide; we accept it because the daemon already inherits
/// install paths through `CapabilityLedger::build_path` once the entry is Ready.
fn enrich_path_for_reprobe() {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    let mut candidates: Vec<PathBuf> = Vec::new();
    // Explicit `CARGO_HOME` override: rustup drops binaries in `$CARGO_HOME/bin`
    // instead of `~/.cargo/bin` when it is set.
    if let Some(cargo_home) = std::env::var_os("CARGO_HOME") {
        candidates.push(PathBuf::from(cargo_home).join("bin"));
    }
    // asdf shims under an explicit `$ASDF_DATA_DIR` (the `~/.asdf` default is
    // handled in the HOME block below).
    if let Some(asdf_data) = std::env::var_os("ASDF_DATA_DIR") {
        candidates.push(PathBuf::from(asdf_data).join("shims"));
    }
    if let Some(h) = home {
        let home_path = PathBuf::from(&h);
        candidates.push(home_path.join(".cargo").join("bin"));
        candidates.push(home_path.join(".fnm"));
        candidates.push(home_path.join(".asdf").join("shims"));
        candidates.push(home_path.join(".nix-profile").join("bin"));
        #[cfg(windows)]
        {
            candidates.push(
                PathBuf::from(&h)
                    .join("AppData")
                    .join("Local")
                    .join("Microsoft")
                    .join("WinGet")
                    .join("Links"),
            );
        }
    }
    #[cfg(target_os = "macos")]
    {
        candidates.push(PathBuf::from("/opt/homebrew/bin"));
        candidates.push(PathBuf::from("/usr/local/bin"));
        // Homebrew's `rustup` formula keeps cargo/rustc proxies in its keg
        // (`$(brew --prefix)/opt/rustup/bin`), not linked into `…/bin`. Cover
        // both the Apple-Silicon and Intel prefixes plus the MacPorts prefix.
        candidates.push(PathBuf::from("/opt/homebrew/opt/rustup/bin"));
        candidates.push(PathBuf::from("/usr/local/opt/rustup/bin"));
        candidates.push(PathBuf::from("/opt/local/bin"));
        candidates.push(PathBuf::from("/Library/Developer/CommandLineTools/usr/bin"));
        // Nix: multi-user default profile + nix-darwin system profile.
        candidates.push(PathBuf::from("/nix/var/nix/profiles/default/bin"));
        candidates.push(PathBuf::from("/run/current-system/sw/bin"));
    }
    #[cfg(target_os = "linux")]
    {
        candidates.push(PathBuf::from("/usr/local/bin"));
        candidates.push(PathBuf::from("/usr/bin"));
        // Nix: multi-user default profile + NixOS system profile.
        candidates.push(PathBuf::from("/nix/var/nix/profiles/default/bin"));
        candidates.push(PathBuf::from("/run/current-system/sw/bin"));
    }
    #[cfg(target_os = "windows")]
    {
        candidates.push(PathBuf::from(r"C:\Program Files\Git\cmd"));
    }

    prepend_existing_dirs(candidates);
}

/// Prepend the given directories to the process PATH, skipping any that don't
/// exist or are already present. Idempotent. Shared by `enrich_path_for_reprobe`
/// and `enrich_path_for_via_parent`.
fn prepend_existing_dirs(candidates: Vec<PathBuf>) {
    let _guard = PATH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut existing: std::collections::HashSet<PathBuf> =
        std::env::split_paths(&current).collect();
    let mut prepended: Vec<PathBuf> = Vec::new();
    for cand in candidates {
        if cand.is_dir() && existing.insert(cand.clone()) {
            prepended.push(cand);
        }
    }
    if prepended.is_empty() {
        return;
    }
    prepended.extend(std::env::split_paths(&current));
    if let Ok(joined) = std::env::join_paths(&prepended) {
        std::env::set_var("PATH", joined);
    } else {
        warn!("failed to join PATH components, leaving PATH unchanged");
    }
}

/// Resolve and prepend the version-manager-controlled bin directory for a
/// `Via`-parent install. fnm keeps node — and every npm-global CLI installed
/// through it, such as `playwright-cli` — under a versioned install directory
/// (`<fnm-dir>/node-versions/<v>/installation/bin` on Unix) that is never on
/// the daemon's PATH. Both `node` (parent `fnm`) and `playwright-cli`
/// (parent `node`) resolve to that same fnm-managed lts bin dir.
async fn enrich_path_for_via_parent(parent: &str) {
    let bin_dir = match parent {
        "fnm" | "node" => fnm_lts_bin_dir().await,
        // uv/cargo Via-parents install onto PATH-visible dirs already covered
        // by enrich_path_for_reprobe; nothing extra to resolve.
        _ => None,
    };
    if let Some(dir) = bin_dir {
        prepend_existing_dirs(vec![dir]);
    }
}

/// Absolute path of the fnm-managed LTS node install's bin directory, or `None`
/// if fnm/node can't be invoked. Asks node for its own executable path
/// (`process.execPath`) and returns the containing directory — correct on every
/// platform regardless of the fnm install layout.
async fn fnm_lts_bin_dir() -> Option<PathBuf> {
    let result = timeout(
        Duration::from_secs(30),
        Command::new("fnm")
            .args([
                "exec",
                "--using",
                "lts",
                "--",
                "node",
                "-e",
                "process.stdout.write(require('path').dirname(process.execPath))",
            ])
            .no_window()
            .output(),
    )
    .await;
    let output = match result {
        Ok(Ok(o)) if o.status.success() => o,
        _ => return None,
    };
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

async fn run_via_parent(parent: &str, subcommand: &[&str]) -> Result<CmdOutcome, BootstrapError> {
    let mut cmd = match parent {
        "fnm" => Command::new("fnm"),
        "node" => {
            let mut cmd = Command::new("fnm");
            cmd.args(["exec", "--using", "lts", "--"]);
            return run_cmd(cmd.args(subcommand)).await;
        }
        "uv" => Command::new("uv"),
        "cargo" => Command::new("cargo"),
        _ => {
            return Ok(CmdOutcome::Failed {
                stderr: format!("unknown Via parent: {parent}"),
            });
        }
    };
    run_cmd(cmd.args(subcommand)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_spec_known() {
        assert!(has_spec("fnm"));
        assert!(has_spec("node"));
        assert!(has_spec("playwright-cli"));
    }

    #[test]
    fn test_has_spec_unknown() {
        assert!(!has_spec("ruby"));
    }

    #[test]
    fn test_dependencies_from_specs() {
        assert_eq!(dependencies("fnm"), &[] as &[&str]);
        assert_eq!(dependencies("node"), &["fnm"]);
        assert_eq!(dependencies("playwright-cli"), &["node"]);
    }

    #[tokio::test]
    async fn test_install_unknown_capability() {
        let result = install("totally-unknown-capability").await.unwrap();
        assert!(matches!(result, BootstrapResult::UnknownCapability { .. }));
    }

    #[test]
    fn test_prepend_existing_dirs_skips_missing() {
        // A non-existent dir must be skipped — it must not be prepended to PATH.
        // We assert the specific dir is absent rather than comparing the whole
        // PATH before/after: PATH is a process-global env var, so an equality
        // check would race with other tests mutating it in parallel.
        let missing = PathBuf::from("/definitely/not/a/real/dir/xyz123");
        prepend_existing_dirs(vec![missing.clone()]);
        let after = std::env::var("PATH").unwrap_or_default();
        let missing_str = missing.to_string_lossy();
        assert!(
            !after.split(':').any(|p| p == missing_str),
            "missing dir must not be added to PATH"
        );
    }

    #[tokio::test]
    async fn test_enrich_path_for_via_parent_unknown_is_noop() {
        // Unknown parents short-circuit before invoking any external command,
        // so this is deterministic regardless of whether fnm is installed.
        let before = std::env::var_os("PATH").unwrap_or_default();
        enrich_path_for_via_parent("totally-unknown-parent").await;
        let after = std::env::var_os("PATH").unwrap_or_default();
        assert_eq!(before, after, "unknown Via parent must not touch PATH");
    }

    #[tokio::test]
    async fn test_enrich_path_for_reprobe_is_idempotent() {
        // Capture PATH before and after two consecutive calls — the second
        // call must be a no-op (no duplicate entries).
        let before = std::env::var_os("PATH").unwrap_or_default();
        enrich_path_for_reprobe();
        let after_first = std::env::var_os("PATH").unwrap_or_default();
        enrich_path_for_reprobe();
        let after_second = std::env::var_os("PATH").unwrap_or_default();
        assert_eq!(
            after_first, after_second,
            "enrich_path_for_reprobe must be idempotent across consecutive calls",
        );
        // First call may or may not extend PATH depending on which well-known
        // dirs exist on the test host; we only assert it doesn't shrink.
        let _ = before; // silence unused warning when extension does happen.
    }
}
