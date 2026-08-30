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

use alephcore::utils::no_window::NoWindow;
use std::error::Error;
use std::time::Duration;

/// PR-2 / BIN-R4-07: SHA256SUMS endpoint convention. Each release
/// ships `install.sh` (Unix) and `install.ps1` (Windows); the matching
/// `SHA256SUMS.txt` carries one `<sha256>  <filename>` line per asset.
/// The file is fetched alongside the installer and the installer's
/// hash is matched against the line for its filename before execution.
/// When the SHA256SUMS manifest is missing (404 / network error), the
/// installer is not executed unless the operator explicitly opts in via
/// the `ALEPH_UPDATE_SOFT_FAIL=1` environment variable.
///
/// Rationale: a soft-fail default means a single DNS hijack / MITM against
/// GitHub's release endpoint can replace the binary with arbitrary code
/// that the user immediately runs with their privileges. The original
/// `true` default existed only because the release process did not yet
/// publish `SHA256SUMS.txt` for every release; that process is now
/// stable enough to require explicit opt-in for the unsafe path.
const SHA256SUMS_URL: &str =
    "https://github.com/rootazero/Aleph/releases/latest/download/SHA256SUMS.txt";
const SOFT_FAIL_ON_MISSING_CHECKSUMS: bool = false;

fn soft_fail_enabled() -> bool {
    if SOFT_FAIL_ON_MISSING_CHECKSUMS {
        return true;
    }
    matches!(
        std::env::var("ALEPH_UPDATE_SOFT_FAIL").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

/// Human-facing releases page (shown on lookup failure).
const RELEASES_PAGE: &str = "https://github.com/rootazero/Aleph/releases/latest";
/// GitHub API endpoint for the latest published release.
const LATEST_API: &str = "https://api.github.com/repos/rootazero/Aleph/releases/latest";
/// Official installer assets, always attached to the latest release. Each is
/// only referenced on its own platform, so gate them to avoid dead-code warns.
#[cfg(not(windows))]
const INSTALL_SH: &str = "https://github.com/rootazero/Aleph/releases/latest/download/install.sh";
#[cfg(windows)]
const INSTALL_PS1: &str = "https://github.com/rootazero/Aleph/releases/latest/download/install.ps1";

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
    match (
        semver::Version::parse(latest),
        semver::Version::parse(current),
    ) {
        (Ok(l), Ok(c)) => l > c,
        _ => latest != current,
    }
}

/// Re-run the official installer for this platform (download + install).
///
/// PR-2 / BIN-R4-07: the previous `curl | bash` (or `iwr | iex`) flow
/// downloaded and executed the installer in a single shell pipe, with
/// no integrity check. A compromised DNS resolver or MITM could swap
/// the installer's bytes for arbitrary code that the shell would then
/// execute as root. The new shape:
///
///   1. Download the installer to memory via reqwest.
///   2. Try to download SHA256SUMS.txt alongside it.
///   3. If SHA256SUMS is available, compute the installer's SHA-256
///      and match it against the line for the installer's filename.
///      Mismatch is a hard failure (no execute).
///   4. If SHA256SUMS is not yet published (404), behaviour is governed
///      by [`soft_fail_enabled`]: today we warn + proceed
///      for backward compatibility with pre-checksums releases; flip
///      to false once the release process always publishes SHA256SUMS.
///   5. Write the verified bytes to a temp file and execute that file
///      (not a fresh shell-piped download).
///
/// Reusing the published installer (rather than re-implementing
/// download-and-replace per OS) keeps the core minimal (R3) and leans on
/// the same battle-tested path the user originally installed with. The
/// installer is what the user opted into; we are only adding a gate.
fn run_installer() -> Result<(), Box<dyn Error>> {
    let (installer_url, installer_name) = installer_target();

    println!("Downloading the installer for verification...");
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("aleph-server/", env!("ALEPH_VERSION")))
        .timeout(Duration::from_secs(120))
        .build()?;
    let installer_bytes = client
        .get(&installer_url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("failed to download {installer_url}: {e}"))?
        .bytes()
        .map_err(|e| format!("failed to read installer body: {e}"))?;

    // Attempt to fetch SHA256SUMS.txt. A 404 is not fatal under the
    // soft-fail flag so existing releases keep working; a mismatch on
    // a present SHA256SUMS is a hard failure.
    let checksums = match client.get(SHA256SUMS_URL).send() {
        Ok(r) if r.status().is_success() => Some(
            r.text()
                .map_err(|e| format!("failed to read SHA256SUMS body: {e}"))?,
        ),
        Ok(r) if r.status().as_u16() == 404 => None,
        Ok(r) => {
            return Err(format!(
                "SHA256SUMS fetch returned unexpected status {}; refusing to install \
                 without integrity check",
                r.status()
            )
            .into());
        }
        Err(e) => {
            // Network error on the checksums fetch is treated like a
            // soft-fail for now; the installer itself still went over
            // HTTPS. To enforce hard-fail, set `SOFT_FAIL_ON_MISSING_CHECKSUMS=false`
            // and do NOT export `ALEPH_UPDATE_SOFT_FAIL`.
            if !soft_fail_enabled() {
                return Err(format!(
                    "failed to fetch SHA256SUMS and soft-fail disabled: {e}"
                )
                .into());
            }
            eprintln!(
                "Warning: could not fetch SHA256SUMS.txt ({e}); proceeding without verification."
            );
            None
        }
    };

    if let Some(text) = checksums {
        let actual = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(&installer_bytes);
            format!("{:x}", hasher.finalize())
        };
        let expected = parse_sha256sums_line(&text, installer_name).ok_or_else(|| {
            format!(
                "SHA256SUMS.txt was fetched but contained no entry for '{installer_name}'; \
                 refusing to install without integrity check"
            )
        })?;
        if !expected.eq_ignore_ascii_case(&actual) {
            return Err(format!(
                "SHA256 mismatch for '{installer_name}':\n  expected: {expected}\n  actual:   {actual}\n\
                 Refusing to install. The downloaded bytes do not match the release manifest."
            )
            .into());
        }
        println!(
            "SHA256 verified: {} ({} bytes)",
            &actual[..16],
            installer_bytes.len()
        );
    } else if !soft_fail_enabled() {
        return Err(
            "SHA256SUMS.txt not published for this release and soft-fail disabled; \
             refusing to install without integrity check (set ALEPH_UPDATE_SOFT_FAIL=1 to override)"
                .into(),
        );
    } else {
        println!(
            "SHA256SUMS.txt not published; verification SKIPPED (soft-fail mode). \
             The HTTPS channel alone protects the install."
        );
    }

    // Write the verified bytes to a temp file and execute that file
    // (not a fresh shell-piped download). The temp file outlives this
    // function so the child process can read it; cleanup is best-effort.
    let temp_path = std::env::temp_dir().join(format!(
        "aleph-installer-{}-{}",
        installer_name,
        std::process::id()
    ));
    std::fs::write(&temp_path, &installer_bytes).map_err(|e| {
        format!(
            "failed to write temp installer {}: {e}",
            temp_path.display()
        )
    })?;

    let status = {
        use std::process::Command;
        #[cfg(windows)]
        {
            println!(
                "Running the Windows installer:\n  powershell -File {}",
                temp_path.display()
            );
            let mut c = Command::new("powershell");
            c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
            c.arg(&temp_path);
            c
        }
        #[cfg(not(windows))]
        {
            println!("Running the installer:\n  bash {}", temp_path.display());
            let mut c = Command::new("bash");
            c.arg(&temp_path);
            c
        }
    }
    .no_window()
    .status()
    .map_err(|e| format!("failed to launch the installer: {e}"))?;

    // Best-effort temp cleanup. The installer may still be reading it;
    // a follow-up reboot / tmpfs reap will get it.
    let _ = std::fs::remove_file(&temp_path);

    if !status.success() {
        return Err(format!("installer exited with status {status}").into());
    }
    Ok(())
}

/// Returns `(url, filename)` of the platform's installer asset.
fn installer_target() -> (String, &'static str) {
    #[cfg(windows)]
    {
        (INSTALL_PS1.to_string(), "install.ps1")
    }
    #[cfg(not(windows))]
    {
        (INSTALL_SH.to_string(), "install.sh")
    }
}

/// Look up `<sha256>  <filename>` in a `sha256sums`-format manifest.
/// Returns `None` if the filename is absent or the file is malformed.
fn parse_sha256sums_line(manifest: &str, filename: &str) -> Option<String> {
    for line in manifest.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // sha256sums format: `<hex>  <filename>` (two spaces) or
        // `<hex> *<filename>` (binary mode marker). The filename may
        // appear with a leading `./`. Treat any mix of whitespace and the
        // binary-mode `*` as one separator class so the filename lands
        // cleanly on its own (and does not keep a stray leading `*`).
        let mut parts = line.splitn(2, |c: char| c.is_whitespace() || c == '*');
        let hex = parts.next()?.trim();
        let name = parts
            .next()?
            .trim()
            .trim_start_matches(|c: char| c.is_whitespace() || c == '*')
            .trim_start_matches("./");
        if name == filename && hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(hex.to_ascii_lowercase());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{is_newer, parse_sha256sums_line};

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

    /// PR-2 / BIN-R4-07: the SHA256SUMS manifest parser covers the
    /// common `sha256sums` formats (two-space separator, binary-mode
    /// `*` marker, leading `./` in filename) and rejects missing /
    /// malformed entries.
    #[test]
    fn parses_canonical_two_space_separator() {
        let manifest = "\
            1111111111111111111111111111111111111111111111111111111111111111  install.sh\n\
            2222222222222222222222222222222222222222222222222222222222222222  install.ps1\n";
        assert_eq!(
            parse_sha256sums_line(manifest, "install.sh").as_deref(),
            Some("1111111111111111111111111111111111111111111111111111111111111111")
        );
    }

    #[test]
    fn parses_binary_mode_star_marker() {
        let manifest = "\
            abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd *install.sh\n";
        assert!(parse_sha256sums_line(manifest, "install.sh").is_some());
    }

    #[test]
    fn parses_leading_dot_slash_filename() {
        let manifest =
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef  ./install.sh\n";
        assert!(parse_sha256sums_line(manifest, "install.sh").is_some());
    }

    #[test]
    fn ignores_comments_and_blanks() {
        let manifest = "# generated by release.sh\n\n\
            cafebabecafebabecafebabecafebabecafebabecafebabecafebabecafebabe  install.sh\n";
        assert!(parse_sha256sums_line(manifest, "install.sh").is_some());
    }

    #[test]
    fn returns_none_for_missing_file() {
        let manifest =
            "1111111111111111111111111111111111111111111111111111111111111111  install.sh\n";
        assert!(parse_sha256sums_line(manifest, "install.ps1").is_none());
    }

    #[test]
    fn returns_none_for_malformed_hash_length() {
        let manifest = "short  install.sh\n";
        assert!(parse_sha256sums_line(manifest, "install.sh").is_none());
    }

    #[test]
    fn returns_none_for_non_hex_hash() {
        let manifest =
            "ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ  install.sh\n";
        assert!(parse_sha256sums_line(manifest, "install.sh").is_none());
    }
}
