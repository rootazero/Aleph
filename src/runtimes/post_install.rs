//! Post-install action runners for runtime specs.

use std::path::PathBuf;

use tokio::process::Command;

use super::specs::PostInstallAction;

/// Errors from post-install actions.
#[derive(Debug, thiserror::Error)]
pub enum PostInstallError {
    #[error("post-install subcommand failed: {stderr}")]
    SubcommandFailed { stderr: String },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not determine Node version for fnm alias")]
    NoNodeVersion,
    #[error("repair command failed for missing asset")]
    RepairFailed,
}

/// Expand `$HOME` in a template path.
fn expand_home(template: &str) -> String {
    if let Ok(home) = std::env::var("HOME") {
        template.replace("$HOME", &home)
    } else {
        template.to_string()
    }
}

/// Run a single post-install action. `bin_path` is the just-installed
/// capability binary (used for `RunSubcommand` and `AssetProbe`).
pub async fn run(action: &PostInstallAction, bin_path: &PathBuf) -> Result<(), PostInstallError> {
    match action {
        PostInstallAction::RunSubcommand { args, target_dir } => {
            run_subcommand(bin_path, args, *target_dir).await
        }
        PostInstallAction::FnmAlias { alias_name } => create_fnm_alias(alias_name).await,
        PostInstallAction::AssetProbe { path, repair } => {
            verify_or_repair(bin_path, path, repair).await
        }
    }
}

async fn run_subcommand(
    bin_path: &PathBuf,
    args: &[&str],
    target_dir: Option<&str>,
) -> Result<(), PostInstallError> {
    let mut cmd = Command::new(bin_path);
    cmd.args(args);
    if let Some(td) = target_dir {
        let expanded = expand_home(td);
        if let Some(parent) = PathBuf::from(&expanded).parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        cmd.arg(&expanded);
    }
    let output = cmd.output().await?;
    if !output.status.success() {
        return Err(PostInstallError::SubcommandFailed {
            stderr: String::from_utf8_lossy(&output.stderr).into(),
        });
    }
    Ok(())
}

async fn create_fnm_alias(alias_name: &str) -> Result<(), PostInstallError> {
    // Parse `fnm list` output to find the just-installed version token.
    let list = Command::new("fnm").args(["list"]).output().await?;
    let text = String::from_utf8_lossy(&list.stdout);
    let version = text
        .lines()
        .filter_map(|l| {
            l.split_whitespace()
                .find(|t| t.starts_with('v'))
                .map(String::from)
        })
        .last()
        .ok_or(PostInstallError::NoNodeVersion)?;
    // Best-effort: failure is not fatal; caller logs it.
    let _ = Command::new("fnm")
        .args(["alias", &version, alias_name])
        .output()
        .await;
    Ok(())
}

async fn verify_or_repair(
    bin_path: &PathBuf,
    path_template: &str,
    repair: &[&str],
) -> Result<(), PostInstallError> {
    let expanded = PathBuf::from(expand_home(path_template));
    if expanded.exists() {
        return Ok(());
    }
    let output = Command::new(bin_path).args(repair).output().await?;
    if !output.status.success() {
        return Err(PostInstallError::RepairFailed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_home_with_var() {
        std::env::set_var("HOME", "/tmp/fake-home");
        let out = expand_home("$HOME/.aleph/skills");
        assert_eq!(out, "/tmp/fake-home/.aleph/skills");
    }

    #[test]
    fn test_expand_home_no_placeholder() {
        let out = expand_home("/absolute/no/expansion");
        assert_eq!(out, "/absolute/no/expansion");
    }
}
