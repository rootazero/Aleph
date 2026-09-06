//! Post-install action runners for runtime specs.

use std::path::PathBuf;

use tokio::process::Command;
use tokio::time::{timeout, Duration};
use tracing::warn;

use super::specs::PostInstallAction;
use crate::utils::no_window::NoWindow;

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
    #[error("HOME or USERPROFILE environment variable not set")]
    HomeNotSet,
    #[error("post-install timed out after {0}s")]
    Timeout(u64),
}

/// Expand `$HOME` or `%USERPROFILE%` in a template path. On Windows also
/// rewrites Unix `/bin/python` → `\Scripts\python.exe` and converts forward
/// slashes to backslashes, so a single template string like
/// `"$HOME/.aleph/.venv/bin/python"` works cross-platform.
fn expand_home(template: &str) -> Result<String, PostInstallError> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| PostInstallError::HomeNotSet)?;
    let s = template
        .replacen("$HOME", &home, 1)
        .replacen("%USERPROFILE%", &home, 1);

    #[cfg(target_os = "windows")]
    let s = s
        .replace("/bin/python", r"\Scripts\python.exe")
        .replace("/bin/", r"\Scripts\")
        .replace('/', r"\");

    Ok(s)
}

/// Post-install command timeout — prevents hung subcommands from blocking indefinitely.
const POST_INSTALL_TIMEOUT_SECS: u64 = 300;

async fn run_cmd_with_timeout(cmd: &mut Command) -> Result<std::process::Output, PostInstallError> {
    match timeout(
        Duration::from_secs(POST_INSTALL_TIMEOUT_SECS),
        cmd.no_window().output(),
    )
    .await
    {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(PostInstallError::Io(e)),
        Err(_) => Err(PostInstallError::Timeout(POST_INSTALL_TIMEOUT_SECS)),
    }
}

/// Run a single post-install action. `bin_path` is the just-installed
/// capability binary (used for `RunSubcommand` and `AssetProbe`).
pub async fn run(action: &PostInstallAction, bin_path: &PathBuf) -> Result<(), PostInstallError> {
    match action {
        PostInstallAction::RunSubcommand {
            args,
            target_dir,
            env,
        } => run_subcommand(bin_path, args, *target_dir, env).await,
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
    env: &[super::specs::EnvFromConfig],
) -> Result<(), PostInstallError> {
    let mut cmd = Command::new(bin_path);
    cmd.args(args);
    for (key, value) in config_env(env) {
        cmd.env(key, value);
    }
    if let Some(td) = target_dir {
        let expanded = expand_home(td)?;
        if let Some(parent) = PathBuf::from(&expanded).parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                warn!(
                    "Failed to create target directory {}: {}",
                    parent.display(),
                    e
                );
            }
        }
        cmd.arg(&expanded);
    }
    let output = run_cmd_with_timeout(&mut cmd).await?;
    if !output.status.success() {
        return Err(PostInstallError::SubcommandFailed {
            stderr: String::from_utf8_lossy(&output.stderr).into(),
        });
    }
    Ok(())
}

/// The environment `vars` ask for, read out of the running config.
///
/// Split from [`config_env_from`] so the mapping is testable without a config
/// file: this half performs the global read, that half is a total function of
/// its inputs.
pub fn config_env(vars: &[super::specs::EnvFromConfig]) -> Vec<(&'static str, String)> {
    if vars.is_empty() {
        return Vec::new();
    }
    match crate::config::Config::load() {
        Ok(cfg) => config_env_from(vars, &cfg.general.browser.runtime),
        Err(e) => {
            // "I could not read the config" is not "the operator set no
            // mirror": say so, then proceed with the default host, which is the
            // only thing left to do.
            warn!("cannot read config for post-install environment: {e}");
            Vec::new()
        }
    }
}

/// [`config_env`]'s pure half.
#[must_use]
pub fn config_env_from(
    vars: &[super::specs::EnvFromConfig],
    runtime: &crate::browser::profile::BrowserRuntimeConfig,
) -> Vec<(&'static str, String)> {
    vars.iter()
        .filter_map(|v| match v {
            super::specs::EnvFromConfig::PlaywrightDownloadHost => runtime
                .download_host()
                .map(|h| ("PLAYWRIGHT_DOWNLOAD_HOST", h.to_string())),
        })
        .collect()
}

async fn create_fnm_alias(alias_name: &str) -> Result<(), PostInstallError> {
    let list = run_cmd_with_timeout(Command::new("fnm").args(["list"])).await?;
    if !list.status.success() {
        return Err(PostInstallError::SubcommandFailed {
            stderr: String::from_utf8_lossy(&list.stderr).into(),
        });
    }
    let text = String::from_utf8_lossy(&list.stdout);
    let version = text
        .lines()
        .filter_map(|l| {
            // Look for the default version marked with '*'
            if l.trim().starts_with('*') {
                l.split_whitespace()
                    .find(|t| t.starts_with('v'))
                    .map(String::from)
            } else {
                None
            }
        })
        .next()
        .ok_or(PostInstallError::NoNodeVersion)?;
    let output =
        run_cmd_with_timeout(Command::new("fnm").args(["alias", &version, alias_name])).await?;
    if !output.status.success() {
        return Err(PostInstallError::SubcommandFailed {
            stderr: String::from_utf8_lossy(&output.stderr).into(),
        });
    }
    Ok(())
}

async fn verify_or_repair(
    bin_path: &PathBuf,
    path_template: &str,
    repair: &[&str],
) -> Result<(), PostInstallError> {
    let expanded = PathBuf::from(expand_home(path_template)?);
    if expanded.exists() {
        return Ok(());
    }
    let expanded_repair: Vec<String> = repair
        .iter()
        .map(|a| expand_home(a))
        .collect::<Result<Vec<_>, _>>()?;
    let output = run_cmd_with_timeout(Command::new(bin_path).args(&expanded_repair)).await?;
    if !output.status.success() {
        return Err(PostInstallError::RepairFailed);
    }
    // A successful exit code does not guarantee the asset was created (e.g. no-op script)
    if !expanded.exists() {
        return Err(PostInstallError::RepairFailed);
    }
    Ok(())
}

// Tests that modify HOME must run serially to avoid race conditions.
#[cfg(test)]
pub(crate) static HOME_LOCK: crate::sync_primitives::Mutex<()> =
    crate::sync_primitives::Mutex::new(());

/// RAII guard for tests that read or mutate the process-global `$HOME`.
///
/// Acquiring it locks [`HOME_LOCK`] (serializing against all other HOME users)
/// and snapshots the current `$HOME`. On drop it restores the snapshot *before*
/// releasing the lock — a struct's `Drop::drop` runs ahead of its fields — so a
/// waiting test always observes the restored value, never a leaked one. This
/// closes the leak where a test set `$HOME` to a temp dir and left it dangling,
/// which made unrelated tests (e.g. the sandbox UDS bridge, whose socket path
/// is derived from `$HOME`) fail with `SUN_LEN` overflow.
#[cfg(test)]
pub(crate) struct HomeEnvGuard {
    _lock: crate::sync_primitives::MutexGuard<'static, ()>,
    prev: Option<std::ffi::OsString>,
}

#[cfg(test)]
impl HomeEnvGuard {
    /// Lock the HOME mutex and snapshot `$HOME`. Use for read-only tests that
    /// transitively depend on `$HOME` and must not race a mutator.
    pub(crate) fn acquire() -> Self {
        let lock = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("HOME");
        Self { _lock: lock, prev }
    }

    /// Lock, snapshot, then set `$HOME` to `value` for the guard's lifetime.
    pub(crate) fn acquire_and_set(value: impl AsRef<std::ffi::OsStr>) -> Self {
        let guard = Self::acquire();
        std::env::set_var("HOME", value);
        guard
    }

    /// Lock, snapshot, then REMOVE `$HOME` for the guard's lifetime.
    pub(crate) fn acquire_and_clear() -> Self {
        let guard = Self::acquire();
        std::env::remove_var("HOME");
        guard
    }
}

#[cfg(test)]
impl Drop for HomeEnvGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}

/// The **only** way for a test to hold both `$ALEPH_HOME` and `$HOME`.
///
/// [`HOME_LOCK`] and [`crate::utils::paths::ALEPH_HOME_TEST_GUARD`] are two
/// separate mutexes over two separate env vars, so taking them by hand admits
/// two orders — and two orders is an ABBA deadlock. That is not a theoretical
/// risk: one test taking them in the opposite order to its three siblings hung
/// the entire `--lib` run **forever** (measured: 15 hangs in 25 runs of
/// `cargo test --lib skill::tests -- --test-threads=32`), with every other
/// `ALEPH_HOME` test piled up behind it. A hang, not a failure — nothing goes
/// red, the run just never ends.
///
/// This type takes them in one fixed order so no call site gets to choose.
/// `nothing_acquires_the_two_env_locks_separately` (below) fails the build if a
/// new site goes back to acquiring them individually, because the mistake it
/// guards against cannot announce itself any other way.
#[cfg(test)]
pub(crate) struct HomeEnvGuards {
    // Declaration order is drop order. Release order does not affect deadlock
    // freedom, but mirroring acquisition keeps the nesting obvious.
    _home: HomeEnvGuard,
    _aleph_home: crate::utils::paths::AlephHomeEnvGuard,
}

#[cfg(test)]
impl HomeEnvGuards {
    /// Lock both, then point `$ALEPH_HOME` and `$HOME` at the given paths for
    /// the guard's lifetime.
    pub(crate) fn acquire_and_set(
        aleph_home: impl AsRef<std::ffi::OsStr>,
        home: impl AsRef<std::ffi::OsStr>,
    ) -> Self {
        // ALEPH_HOME first — the one and only order.
        let aleph_home = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(aleph_home);
        let home = HomeEnvGuard::acquire_and_set(home);
        Self {
            _home: home,
            _aleph_home: aleph_home,
        }
    }

    /// Lock both, then REMOVE `$ALEPH_HOME` and `$HOME` for the guard's
    /// lifetime — for a test that needs `get_config_dir()` to fail outright
    /// (no fallback of any kind), which only happens when neither resolves.
    ///
    /// Taking `HomeEnvGuard` alone for this — as this codebase's own
    /// `runtime_manage` ledger-failure test once did — races: `HOME_LOCK` and
    /// `ALEPH_HOME_TEST_GUARD` are separate mutexes, so a test elsewhere in
    /// the same binary that legitimately sets `$ALEPH_HOME` under its OWN
    /// guard can supply the very fallback the `$HOME`-only test is trying to
    /// rule out, whenever `cargo test`'s default parallel execution overlaps
    /// them. Measured: green in isolation, red running the whole module.
    pub(crate) fn acquire_and_clear() -> Self {
        // Same fixed order as `acquire_and_set` — ALEPH_HOME first.
        let aleph_home = crate::utils::paths::AlephHomeEnvGuard::acquire_and_clear();
        let home = HomeEnvGuard::acquire_and_clear();
        Self {
            _home: home,
            _aleph_home: aleph_home,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::profile::BrowserRuntimeConfig;
    use crate::runtimes::specs::EnvFromConfig;

    /// Collect every `.rs` file under `dir`, recursively.
    fn rust_sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// No file may take the `$ALEPH_HOME` and `$HOME` locks individually —
    /// [`HomeEnvGuards`] exists so the order is not a call site's to pick.
    ///
    /// This is a source scan rather than a runtime assertion because the bug it
    /// catches produces a **hang**, not a failure: a suite that deadlocks never
    /// reaches any assertion, so the only place to catch it is before it runs.
    /// Same shape as the `src/harness/tests/budget.rs` ratchet.
    #[test]
    fn nothing_acquires_the_two_env_locks_separately() {
        let src_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_sources(&src_root, &mut files);
        assert!(!files.is_empty(), "found no sources under {src_root:?}");

        let mut offenders = Vec::new();
        for path in files {
            // This file *defines* `HomeEnvGuards`, so it is the one place that
            // legitimately names both guards.
            if path.ends_with("runtimes/post_install.rs") {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            // `AlephHomeEnvGuard::acquire` ends with `HomeEnvGuard::acquire`, so
            // blank the longer name out before looking for the shorter one.
            // (`HomeEnvGuards::acquire` — the combined guard — does not match
            // either, thanks to the plural.)
            let takes_home = src
                .replace("AlephHomeEnvGuard", "\u{0}")
                .contains("HomeEnvGuard::acquire");
            let takes_aleph_home = src.contains("AlephHomeEnvGuard::acquire")
                || src.contains("IsolatedAlephHome::new")
                || src.contains("ALEPH_HOME_TEST_GUARD");
            if takes_home && takes_aleph_home {
                offenders.push(
                    path.strip_prefix(&src_root)
                        .unwrap_or(&path)
                        .display()
                        .to_string(),
                );
            }
        }

        assert!(
            offenders.is_empty(),
            "these take the $ALEPH_HOME and $HOME locks separately — an ABBA \
             deadlock that hangs the whole --lib run instead of failing it. Use \
             `runtimes::post_install::HomeEnvGuards::acquire_and_set` instead: {offenders:?}"
        );
    }

    #[test]
    fn test_expand_home_with_var() {
        let _home = HomeEnvGuard::acquire_and_set("/tmp/fake-home");
        let out = expand_home("$HOME/.aleph/skills").unwrap();
        // expand_home rewrites '/' to '\' on Windows, so the expected separator
        // is platform-dependent.
        #[cfg(not(target_os = "windows"))]
        assert_eq!(out, "/tmp/fake-home/.aleph/skills");
        #[cfg(target_os = "windows")]
        assert_eq!(out, r"\tmp\fake-home\.aleph\skills");
    }

    #[test]
    fn test_expand_home_no_placeholder() {
        let out = expand_home("/absolute/no/expansion").unwrap();
        #[cfg(not(target_os = "windows"))]
        assert_eq!(out, "/absolute/no/expansion");
        #[cfg(target_os = "windows")]
        assert_eq!(out, r"\absolute\no\expansion");
    }

    #[test]
    fn test_expand_home_multiple_placeholders() {
        let _home = HomeEnvGuard::acquire_and_set("/tmp/fake-home");
        let out = expand_home("$HOME/a/$HOME/b").unwrap();
        // Only the first occurrence is replaced — caller should pass templates
        // with a single $HOME placeholder per arg. Document this contract.
        #[cfg(not(target_os = "windows"))]
        assert_eq!(out, "/tmp/fake-home/a/$HOME/b");
        #[cfg(target_os = "windows")]
        assert_eq!(out, r"\tmp\fake-home\a\$HOME\b");
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn test_verify_or_repair_expands_home_in_repair_args() {
        use std::os::unix::fs::PermissionsExt;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let _home = HomeEnvGuard::acquire_and_set(dir.path());

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
        // The repair command (touchit.sh) must create the probed path.
        let action = PostInstallAction::AssetProbe {
            path: "$HOME/expected_output_file",
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

    /// The mirror is a config key rather than "go export a variable" because
    /// the installer runs inside the daemon — there is no shell for an operator
    /// to export into. Playwright's CDN is blocked on the same networks that
    /// block GitHub release assets, so without this the install just hangs and
    /// then fails with a network error naming a host nobody chose.
    #[test]
    fn the_download_host_reaches_the_installer_as_playwright_download_host() {
        let runtime = BrowserRuntimeConfig {
            download_host: Some("https://npmmirror.com/mirrors/playwright".into()),
            ..BrowserRuntimeConfig::default()
        };
        assert_eq!(
            config_env_from(&[EnvFromConfig::PlaywrightDownloadHost], &runtime),
            vec![(
                "PLAYWRIGHT_DOWNLOAD_HOST",
                "https://npmmirror.com/mirrors/playwright".to_string()
            )]
        );
    }

    /// An unset or blank mirror must produce NO variable, not an empty one.
    /// `PLAYWRIGHT_DOWNLOAD_HOST=` is not "use the default host"; it is "the
    /// host is the empty string", and every download then fails on a malformed
    /// URL. This is the fail-closed reading of a blank field (判据 §8).
    #[test]
    fn a_blank_download_host_sets_no_variable_at_all() {
        for host in [None, Some(String::new()), Some("   ".to_string())] {
            let runtime = BrowserRuntimeConfig {
                download_host: host.clone(),
                ..BrowserRuntimeConfig::default()
            };
            assert!(
                config_env_from(&[EnvFromConfig::PlaywrightDownloadHost], &runtime).is_empty(),
                "blank host {host:?} produced a variable"
            );
        }
    }

    /// An action that declares no env gets none — the passthrough must not
    /// leak onto every other post-install subcommand just because it is cheap.
    #[test]
    fn an_action_that_declares_no_env_gets_none() {
        let runtime = BrowserRuntimeConfig {
            download_host: Some("https://mirror.example".into()),
            ..BrowserRuntimeConfig::default()
        };
        assert!(config_env_from(&[], &runtime).is_empty());
    }
}
