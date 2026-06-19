//! `update` subcommand for the standalone `aleph-server` binary.
//!
//! The desktop app self-updates through Tauri's bundled updater; the
//! standalone server (installed via `curl | bash` / `install.ps1`) has no such
//! host, so this subcommand is its equivalent: check GitHub for a newer
//! release and — unless `--check` — re-run the official installer to download
//! and install the correct binary for this platform.
//!
//! Reusing the published installer (rather than re-implementing
//! download-and-replace per OS) keeps the core minimal (R3) and leans on the
//! same battle-tested path the user originally installed with. The installer
//! places the new binary; the running daemon must be restarted to pick it up.

use std::error::Error;
use std::time::Duration;

/// Human-facing releases page (shown on lookup failure).
const RELEASES_PAGE: &str = "https://github.com/rootazero/Aleph/releases/latest";
/// GitHub API endpoint for the latest published release.
const LATEST_API: &str = "https://api.github.com/repos/rootazero/Aleph/releases/latest";
/// Official installer assets, always attached to the latest release. Each is
/// only referenced on its own platform, so gate them to avoid dead-code warns.
#[cfg(not(windows))]
const INSTALL_SH: &str = "https://github.com/rootazero/Aleph/releases/latest/download/install.sh";
#[cfg(windows)]
const INSTALL_PS1: &str =
    "https://github.com/rootazero/Aleph/releases/latest/download/install.ps1";

/// Handle `aleph-server update [--check]`.
pub fn handle_update(check_only: bool) -> Result<(), Box<dyn Error>> {
    let current = env!("ALEPH_VERSION");
    println!("Current version: {current}");

    let latest = fetch_latest_version()?;
    println!("Latest version:  {latest}");

    if !is_newer(&latest, current) {
        println!("Aleph is up to date.");
        return Ok(());
    }

    println!("A newer version is available: {latest}");
    if check_only {
        println!("Run `aleph-server update` to download and install it.");
        return Ok(());
    }

    run_installer()?;
    println!(
        "\nUpdate installed. Restart the server to run the new version:\n  \
         aleph-server stop && aleph-server start"
    );
    Ok(())
}

/// Query GitHub for the latest published release tag (without the leading `v`).
fn fetch_latest_version() -> Result<String, Box<dyn Error>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("aleph-server/", env!("ALEPH_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()?;
    let resp = client
        .get(LATEST_API)
        .header("Accept", "application/vnd.github+json")
        .send()?
        .error_for_status()
        .map_err(|e| format!("GitHub API request failed: {e} (see {RELEASES_PAGE})"))?;
    let json: serde_json::Value = resp.json()?;
    let tag = json
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .ok_or("GitHub API response had no tag_name")?;
    Ok(tag.trim_start_matches('v').to_string())
}

/// True when `latest` is strictly newer than `current`. Both are CalVer
/// (`YY.M.D`), which is valid SemVer; fall back to string inequality if either
/// fails to parse so a malformed tag never silently blocks a real update.
fn is_newer(latest: &str, current: &str) -> bool {
    match (semver::Version::parse(latest), semver::Version::parse(current)) {
        (Ok(l), Ok(c)) => l > c,
        _ => latest != current,
    }
}

/// Re-run the official installer for this platform (download + install). The
/// command is printed before it runs so the action is transparent.
fn run_installer() -> Result<(), Box<dyn Error>> {
    use std::process::Command;

    #[cfg(windows)]
    let mut cmd = {
        println!("Running the Windows installer:\n  iwr -useb {INSTALL_PS1} | iex");
        let mut c = Command::new("powershell");
        c.args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &format!("iwr -useb {INSTALL_PS1} | iex"),
        ]);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        println!("Running the installer:\n  curl -fsSL {INSTALL_SH} | bash");
        let mut c = Command::new("bash");
        c.arg("-c").arg(format!("curl -fsSL {INSTALL_SH} | bash"));
        c
    };

    let status = cmd
        .status()
        .map_err(|e| format!("failed to launch the installer: {e}"))?;
    if !status.success() {
        return Err(format!("installer exited with status {status}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[test]
    fn detects_newer_calver() {
        assert!(is_newer("26.6.20", "26.6.14"));
        assert!(is_newer("26.7.1", "26.6.30"));
        assert!(is_newer("27.1.1", "26.12.31"));
    }

    #[test]
    fn equal_or_older_is_not_newer() {
        assert!(!is_newer("26.6.14", "26.6.14"));
        assert!(!is_newer("26.6.10", "26.6.14"));
        assert!(!is_newer("25.1.1", "26.6.14"));
    }

    #[test]
    fn malformed_tags_fall_back_to_inequality() {
        assert!(is_newer("weird-tag", "26.6.14"));
        assert!(!is_newer("same", "same"));
    }
}
