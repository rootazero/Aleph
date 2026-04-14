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

/// Expand `$HOME` or `%USERPROFILE%` in a template path. On Windows also
/// rewrites Unix `/bin/python` → `\Scripts\python.exe` and converts forward
/// slashes to backslashes, so a single template string like
/// `"$HOME/.aleph/.venv/bin/python"` works cross-platform.
fn expand_home(template: &str) -> String {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    let s = template
        .replacen("$HOME", &home, 1)
        .replacen("%USERPROFILE%", &home, 1);

    #[cfg(target_os = "windows")]
    let s = s
        .replace("/bin/python", r"\Scripts\python.exe")
        .replace("/bin/", r"\Scripts\")
        .replace('/', r"\");

    s
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
    let expanded_repair: Vec<String> = repair.iter().map(|a| expand_home(a)).collect();
    let output = Command::new(bin_path)
        .args(&expanded_repair)
        .output()
        .await?;
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

    #[test]
    fn test_expand_home_multiple_placeholders() {
        std::env::set_var("HOME", "/tmp/fake-home");
        let out = expand_home("$HOME/a/$HOME/b");
        assert_eq!(out, "/tmp/fake-home/a/$HOME/b");
        // Only the first occurrence is replaced — caller should pass templates
        // with a single $HOME placeholder per arg. Document this contract.
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn test_verify_or_repair_expands_home_in_repair_args() {
        use std::os::unix::fs::PermissionsExt;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        std::env::set_var("HOME", dir.path());

        // Write a tiny shell script that creates a file at its first arg.
        let script_path = dir.path().join("touchit.sh");
        tokio::fs::write(
            &script_path,
            "#!/bin/sh\nmkdir -p \"$(dirname \"$1\")\" && : > \"$1\"\n",
        )
        .await
        .unwrap();
        let mut perms = tokio::fs::metadata(&script_path)
            .await
            .unwrap()
            .permissions();
        perms.set_mode(0o755);
        tokio::fs::set_permissions(&script_path, perms)
            .await
            .unwrap();

        // Probe a non-existent path so the repair fires.
        // repair[0] is the script to run (bin_path), repair[1] is the output
        // file path passed as $1 to touchit.sh.
        let action = PostInstallAction::AssetProbe {
            path: "$HOME/never/exists",
            repair: &["$HOME/expected_output_file"],
        };

        // Use touchit.sh as bin_path directly. run() will invoke it with the
        // $HOME-expanded repair args — so touchit.sh receives the expanded
        // output path as $1 and creates the file there.
        let bin = dir.path().join("touchit.sh");
        let result = run(&action, &bin).await;
        assert!(result.is_ok(), "repair must succeed: {result:?}");

        let expected_out = dir.path().join("expected_output_file");
        assert!(
            tokio::fs::try_exists(&expected_out).await.unwrap(),
            "expansion should have produced {}",
            expected_out.display()
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn test_verify_or_repair_skips_when_path_exists() {
        // Use /tmp which is guaranteed to exist on Unix — no env-var mutation
        // needed, so this test is free of HOME races with the other async test.
        let action = PostInstallAction::AssetProbe {
            path: "/tmp",
            repair: &["false"], // would fail if it ran
        };
        let sh = PathBuf::from("/bin/sh");
        let result = run(&action, &sh).await;
        assert!(result.is_ok());
    }
}
