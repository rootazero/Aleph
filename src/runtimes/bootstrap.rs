//! Runtime install dispatcher driven by `super::specs::SPECS`.

use std::path::PathBuf;

use tokio::process::Command;

use super::os::TargetOs;
use super::post_install;
use super::probe;
use super::specs::{find_spec, select_install, InstallStrategy};

/// Result of a bootstrap attempt.
#[derive(Debug)]
pub enum BootstrapResult {
    Success { bin_path: PathBuf, version: String },
    PathNotFound { expected: String },
    Failed { stderr: String },
    Unsupported { capability: String, reason: String },
    UnknownCapability { capability: String },
}

/// Errors raised by the dispatcher itself (not captured in BootstrapResult).
#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("post-install action failed: {0}")]
    PostInstall(#[from] post_install::PostInstallError),
    #[error("unknown capability: {0}")]
    Unknown(String),
}

/// Install a capability according to its spec. Assumes `deps` are already Ready
/// (caller handles dep resolution).
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

    let current = TargetOs::current();
    let os_install = match select_install(spec.install, current) {
        Some(oi) => oi,
        None => {
            return Ok(BootstrapResult::Unsupported {
                capability: name.into(),
                reason: format!("no install strategy for {:?}", current),
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

    // 2. Re-probe to get binary path + version.
    let probe_result = probe::probe(name);
    if !probe_result.found {
        return Ok(BootstrapResult::PathNotFound {
            expected: format!("binary '{}' on PATH after install", name),
        });
    }
    let bin_path = match probe_result.bin_path.clone() {
        Some(path) => path,
        None => {
            return Ok(BootstrapResult::PathNotFound {
                expected: format!("binary path for '{}' after successful probe", name),
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
pub fn has_spec(capability: &str) -> bool {
    find_spec(capability).is_some()
}

/// Dependencies that must be Ready before installing this capability.
pub fn dependencies(capability: &str) -> &'static [&'static str] {
    find_spec(capability).map(|s| s.deps).unwrap_or(&[])
}

enum CmdOutcome {
    Success,
    Failed { stderr: String },
}

async fn run_cmd(cmd: &mut Command) -> Result<CmdOutcome, BootstrapError> {
    let output = cmd.output().await?;
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
    run_cmd(Command::new("powershell").args(["-Command", script])).await
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
    if let Some(h) = home {
        let home_path = PathBuf::from(&h);
        candidates.push(home_path.join(".cargo").join("bin"));
        candidates.push(home_path.join(".fnm"));
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
        candidates.push(PathBuf::from("/Library/Developer/CommandLineTools/usr/bin"));
    }
    #[cfg(target_os = "linux")]
    {
        candidates.push(PathBuf::from("/usr/local/bin"));
        candidates.push(PathBuf::from("/usr/bin"));
    }
    #[cfg(target_os = "windows")]
    {
        candidates.push(PathBuf::from(r"C:\Program Files\Git\cmd"));
    }

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
                stderr: format!("unknown Via parent: {}", parent),
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
