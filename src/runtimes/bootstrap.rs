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

    // 2. Re-probe to get binary path + version.
    let probe_result = probe::probe(name);
    if !probe_result.found {
        return Ok(BootstrapResult::PathNotFound {
            expected: format!("binary '{}' on PATH after install", name),
        });
    }
    let bin_path = probe_result.bin_path.clone().unwrap();

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

async fn run_shell(script: &str) -> Result<CmdOutcome, BootstrapError> {
    let output = Command::new("sh").args(["-c", script]).output().await?;
    if output.status.success() {
        Ok(CmdOutcome::Success)
    } else {
        Ok(CmdOutcome::Failed {
            stderr: String::from_utf8_lossy(&output.stderr).into(),
        })
    }
}

async fn run_powershell(script: &str) -> Result<CmdOutcome, BootstrapError> {
    let output = Command::new("powershell")
        .args(["-Command", script])
        .output()
        .await?;
    if output.status.success() {
        Ok(CmdOutcome::Success)
    } else {
        Ok(CmdOutcome::Failed {
            stderr: String::from_utf8_lossy(&output.stderr).into(),
        })
    }
}

async fn run_via_parent(
    parent: &str,
    subcommand: &[&str],
) -> Result<CmdOutcome, BootstrapError> {
    let output = match parent {
        "fnm" => {
            Command::new("fnm")
                .args(subcommand)
                .output()
                .await?
        }
        "node" => {
            // Wrap in `fnm exec --using lts --` to get a Node shell with PATH.
            let mut args: Vec<&str> = vec!["exec", "--using", "lts", "--"];
            args.extend(subcommand.iter().copied());
            Command::new("fnm").args(&args).output().await?
        }
        "uv" => Command::new("uv").args(subcommand).output().await?,
        "cargo" => Command::new("cargo").args(subcommand).output().await?,
        _ => {
            return Ok(CmdOutcome::Failed {
                stderr: format!("unknown Via parent: {}", parent),
            });
        }
    };
    if output.status.success() {
        Ok(CmdOutcome::Success)
    } else {
        Ok(CmdOutcome::Failed {
            stderr: String::from_utf8_lossy(&output.stderr).into(),
        })
    }
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
    async fn test_install_empty_install_array_returns_unsupported() {
        let result = install("cargo").await.unwrap();
        assert!(matches!(result, BootstrapResult::Unsupported { .. }));
    }
}
