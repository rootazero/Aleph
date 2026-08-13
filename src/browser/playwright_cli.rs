//! `PlaywrightCliDriver` — manages per-session `playwright-cli` subprocesses.
//!
//! Each tool call spawns a fresh process with `-s=<session_key>`; the CLI
//! keeps browser state in memory across invocations under the same key.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::Mutex;

// Only the production `provision_binary` provisions a runtime; the sealed
// test twin must not so much as name the installer.
#[cfg(not(test))]
use crate::runtimes::{ensure_capability, ledger::CapabilityLedger};
use crate::security::secret_env::is_secret_env;
use crate::sync_primitives::RwLock;
use crate::utils::no_window::NoWindow;

use super::error::BrowserError;
use super::playwright_launch::{open_argv, write_launch_config, LaunchPolicy, SessionLaunch};
use super::profile::PlaywrightCliConfig;

/// How long bringing up a browser session may take.
///
/// Mirrors the existing-session driver's answer to the same question
/// (`chrome_mcp.rs::create_session`, `timeout_seconds: Some(60)`) — the two
/// drivers are twins and should not disagree about how slow a cold browser is.
const SESSION_START_TIMEOUT_SECS: u64 = 60;

/// Output of a single `playwright-cli` invocation.
#[derive(Debug, Clone)]
pub struct CliOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub page_meta: Option<PageMeta>,
}

/// Metadata extracted from the `### Page / URL / Title / Snapshot` header.
#[derive(Debug, Clone, Default)]
pub struct PageMeta {
    pub url: String,
    pub title: String,
    pub snapshot_file: Option<PathBuf>,
}

/// Lazily resolves + caches the `playwright-cli` binary path, then serializes
/// concurrent invocations per session key.
pub struct PlaywrightCliDriver {
    binary_path: RwLock<Option<PathBuf>>,
    config: PlaywrightCliConfig,
    per_session_locks: RwLock<HashMap<String, Arc<Mutex<()>>>>,
    binary_resolve_lock: tokio::sync::Mutex<()>,
}

impl PlaywrightCliDriver {
    #[must_use]
    pub fn new(config: PlaywrightCliConfig) -> Self {
        Self {
            binary_path: RwLock::new(None),
            config,
            per_session_locks: RwLock::new(HashMap::new()),
            binary_resolve_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Resolve (or re-resolve) the CLI binary path. Caches on success.
    pub async fn resolve_binary(&self) -> Result<PathBuf, BrowserError> {
        if let Some(p) = self
            .binary_path
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Ok(p);
        }

        let _guard = self.binary_resolve_lock.lock().await;

        if let Some(p) = self
            .binary_path
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Ok(p);
        }

        if let Some(explicit) = self.config.binary_path.as_deref() {
            let p = PathBuf::from(explicit);
            if !p.exists() {
                return Err(BrowserError::PlaywrightCliNotInstalled);
            }
            *self.binary_path.write().unwrap_or_else(|e| e.into_inner()) = Some(p.clone());
            return Ok(p);
        }
        self.provision_binary().await
    }

    /// The slow path of [`Self::resolve_binary`]: ensure the full chain
    /// (fnm → node → playwright-cli + chromium + skills), **installing it over
    /// the network** if it is not there, and cache the result.
    #[cfg(not(test))]
    async fn provision_binary(&self) -> Result<PathBuf, BrowserError> {
        let runtimes_dir = crate::runtimes::get_runtimes_dir()
            .map_err(|e| BrowserError::PlaywrightCliError(format!("runtimes dir: {e}")))?;
        let ledger_path = runtimes_dir.join("ledger.json");
        let ledger =
            tokio::task::spawn_blocking(move || CapabilityLedger::load_or_create(ledger_path))
                .await
                .map_err(|e| {
                    BrowserError::PlaywrightCliError(format!("load capability ledger: {e}"))
                })?;
        let ledger = Arc::new(tokio::sync::RwLock::new(ledger));

        let resolved = ensure_capability("playwright-cli", &ledger)
            .await
            .map_err(|e| BrowserError::PlaywrightCliError(format!("ensure playwright-cli: {e}")))?;

        *self.binary_path.write().unwrap_or_else(|e| e.into_inner()) = Some(resolved.clone());
        Ok(resolved)
    }

    /// Under test an unconfigured driver refuses instead of reaching outside
    /// the process — it neither installs a runtime nor launches a browser.
    ///
    /// The production twin above *installs things over the network*, and now
    /// that the driver can open a browser, a test that reached it would launch
    /// a real Chrome. One already did: a unit test asserting "degrades without
    /// a running browser" instead navigated a live browser to a public site.
    /// Those assertions were green because the machine happened to have no
    /// reachable browser — their green was a property of the environment, not
    /// of the code.
    ///
    /// Sealing here rather than in each test is deliberate: this is the one
    /// boundary where the process reaches out, so no future test can forget to
    /// seal itself. A test that genuinely wants a live browser opts in by
    /// setting `binary_path`, which [`Self::resolve_binary`] honors before
    /// calling this.
    ///
    /// ⚠️ `cfg(test)` covers `--lib` unit tests only; integration tests under
    /// `tests/` link the library built without it and are not sealed.
    #[cfg(test)]
    #[allow(clippy::unused_async)]
    async fn provision_binary(&self) -> Result<PathBuf, BrowserError> {
        Err(BrowserError::PlaywrightCliNotInstalled)
    }

    fn session_lock(&self, session_key: &str) -> Arc<Mutex<()>> {
        let mut map = self
            .per_session_locks
            .write()
            .unwrap_or_else(|e| e.into_inner());
        map.entry(session_key.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Run one `playwright-cli` subcommand for a session, launching the
    /// session's browser first if the CLI says there is none.
    ///
    /// Serializes concurrent calls within the same `session_key`, which is
    /// also what makes the lazy launch safe: the open-and-retry happens under
    /// the same per-session lock as every other call, so two callers racing
    /// into an unopened session cannot both open it.
    ///
    /// **Why lazily, off the CLI's own refusal, rather than eagerly at
    /// construction:** a second `open` on a live session is destructive —
    /// measured against `playwright-cli 0.1.8`, it relaunches the browser
    /// under a new pid and drops every existing tab. Opening only when the CLI
    /// itself reports the session is not open makes a redundant `open`
    /// structurally impossible, whereas an eager open would have to be right
    /// about every path that obtains a backend.
    ///
    /// `policy` comes from the caller for two reasons: the driver is shared
    /// across sessions and does not know which profile a session key belongs
    /// to, and only some calls are entitled to create a browser at all — see
    /// [`LaunchPolicy`].
    pub async fn run(
        &self,
        session_key: &str,
        policy: LaunchPolicy<'_>,
        args: &[&str],
        timeout: Duration,
    ) -> Result<CliOutput, BrowserError> {
        let bin = self.resolve_binary().await?;
        let lock = self.session_lock(session_key);
        let _guard = lock.lock().await;

        match self.spawn(&bin, session_key, args, timeout).await {
            Err(BrowserError::NoSession(key)) => {
                let Some(launch) = policy.launch() else {
                    return Err(BrowserError::NoSession(key));
                };
                self.open_session(&bin, session_key, launch).await?;
                // One retry only. If the session still reports "not open"
                // after a successful launch, that is a real failure and must
                // surface rather than loop.
                self.spawn(&bin, session_key, args, timeout).await
            }
            other => other,
        }
    }

    /// Launch the session's browser via `open`, the only subcommand that
    /// accepts `--config` / `--headed` / `--browser`.
    ///
    /// Calls [`Self::spawn`] directly rather than [`Self::run`]: the lock is
    /// already held by the caller, and going through `run` would make the
    /// open-on-`NoSession` path re-entrant.
    async fn open_session(
        &self,
        bin: &Path,
        session_key: &str,
        launch: &SessionLaunch,
    ) -> Result<(), BrowserError> {
        let config_path = write_launch_config(session_key, launch).await?;
        let argv = open_argv(launch, &config_path);
        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
        // Starting a browser is not a navigation and must not borrow the
        // navigation budget, whose default (30 s) is shorter than a cold
        // launch can take. The repo already answers "how long may bringing up
        // a browser session take" in the twin subsystem — `chrome_mcp.rs`'s
        // `create_session` allows 60 s — so take the larger of that and the
        // configured navigation timeout rather than inventing a third number.
        let timeout =
            Duration::from_secs(self.config.nav_timeout_secs.max(SESSION_START_TIMEOUT_SECS));
        self.spawn(bin, session_key, &args, timeout).await?;
        tracing::info!(session = %session_key, "playwright-cli session opened");
        Ok(())
    }

    /// Spawn one `playwright-cli -s=<session_key> <args>` process and capture
    /// its output. Assumes the per-session lock is already held.
    async fn spawn(
        &self,
        bin: &Path,
        session_key: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<CliOutput, BrowserError> {
        let session_flag = format!("-s={session_key}");
        let mut cmd = Command::new(bin);
        cmd.arg(&session_flag)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Kill the child if its owning future is dropped — e.g. on the
            // timeout branch below, where `wait_with_output` has consumed
            // `child` so it can no longer be killed explicitly.
            .kill_on_drop(true);
        // Strip secret-bearing env vars from the browser child process. The
        // browser never needs the parent's credentials; over-stripping is safe.
        for (name, _) in std::env::vars() {
            if is_secret_env(&name) {
                cmd.env_remove(&name);
            }
        }

        let child = cmd.no_window().spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => BrowserError::PlaywrightCliNotInstalled,
            _ => BrowserError::Io(e),
        })?;

        let output_fut = child.wait_with_output();
        let output = match tokio::time::timeout(timeout, output_fut).await {
            Ok(res) => res.map_err(BrowserError::Io)?,
            Err(_) => {
                // `output_fut` (owning `child`) is dropped here; `kill_on_drop`
                // set above terminates the process on timeout.
                return Err(BrowserError::Timeout(timeout.as_millis() as u64));
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);

        if !output.status.success() {
            return Err(classify_failure(
                &stdout,
                &stderr,
                exit_code,
                session_key,
                timeout.as_millis() as u64,
            ));
        }

        let page_meta = parse_page_meta(&stdout);
        Ok(CliOutput {
            stdout,
            stderr,
            exit_code,
            page_meta,
        })
    }

    pub const fn config(&self) -> &PlaywrightCliConfig {
        &self.config
    }
}

/// Classify a non-zero-exit invocation.
///
/// **Reads stdout as well as stderr.** `playwright-cli` writes its "the
/// browser is not open" refusal to *stdout* and leaves stderr empty (exit 1),
/// so a stderr-only classifier could never produce [`BrowserError::NoSession`]
/// — which is exactly what happened: the variant was constructed nowhere
/// reachable and had no consumers at all.
///
/// The not-open match is deliberately anchored on the CLI's full phrase rather
/// than a loose word: this runs on failure output that can include page text.
fn classify_failure(
    stdout: &str,
    stderr: &str,
    exit_code: i32,
    session_key: &str,
    timeout_ms: u64,
) -> BrowserError {
    let s = format!("{stdout}\n{stderr}").to_lowercase();
    if s.contains("please run open first")
        || s.contains("no session")
        || s.contains("browser not open")
    {
        BrowserError::NoSession(session_key.to_string())
    } else if s.contains("timeout") {
        BrowserError::Timeout(timeout_ms)
    } else if s.contains("element not found") || s.contains("no element") {
        BrowserError::ActionFailed(format!("element not found ({stderr})"))
    } else {
        BrowserError::PlaywrightCliError(format!("exit {exit_code}: {stderr}"))
    }
}

/// Parse stdout for `### Page / URL / Title / Snapshot [path]` header.
#[must_use]
pub fn parse_page_meta(stdout: &str) -> Option<PageMeta> {
    let mut meta = PageMeta::default();
    let mut in_page_section = false;
    let mut found_any = false;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed == "### Page" {
            in_page_section = true;
            continue;
        }
        if in_page_section {
            if let Some(rest) = trimmed.strip_prefix("- Page URL:") {
                meta.url = rest.trim().to_string();
                found_any = true;
            } else if let Some(rest) = trimmed.strip_prefix("- Page Title:") {
                meta.title = rest.trim().to_string();
                found_any = true;
            }
        }
        // "### Snapshot" is followed by: [Snapshot](<path>)
        if let Some(path) = trimmed
            .strip_prefix("[Snapshot](")
            .and_then(|s| s.strip_suffix(')'))
        {
            meta.snapshot_file = Some(PathBuf::from(path.trim()));
            found_any = true;
        }
    }
    if found_any {
        Some(meta)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_page_meta_full() {
        let stdout = "\
### Page
- Page URL: https://example.com/
- Page Title: Example Domain
### Snapshot
[Snapshot](.playwright-cli/page-2026-04-12T00-00-00Z.yml)
";
        let meta = parse_page_meta(stdout).unwrap();
        assert_eq!(meta.url, "https://example.com/");
        assert_eq!(meta.title, "Example Domain");
        assert_eq!(
            meta.snapshot_file.as_ref().unwrap().to_string_lossy(),
            ".playwright-cli/page-2026-04-12T00-00-00Z.yml"
        );
    }

    #[test]
    fn test_parse_page_meta_none_for_empty() {
        assert!(parse_page_meta("").is_none());
        assert!(parse_page_meta("just some unrelated output").is_none());
    }

    /// The `cfg(test)` seal is itself asserted, not assumed: an unconfigured
    /// driver must refuse to resolve a binary rather than probe the machine.
    /// If this ever regresses, every "degrades without a running browser" test
    /// silently becomes a live-browser test again.
    #[tokio::test]
    async fn an_unconfigured_driver_cannot_reach_the_machine_under_test() {
        let driver = PlaywrightCliDriver::new(PlaywrightCliConfig::default());
        assert!(
            matches!(
                driver.resolve_binary().await,
                Err(BrowserError::PlaywrightCliNotInstalled)
            ),
            "an unconfigured driver must not resolve a real binary in tests"
        );
    }

    /// …and the opt-in still works, so a deliberate live test can set a path.
    /// (Pointed at a path that does not exist, so this test stays hermetic
    /// too — what it proves is that `binary_path` is consulted before the
    /// seal, not that any particular binary is present.)
    #[tokio::test]
    async fn an_explicit_binary_path_is_still_consulted_first() {
        let driver = PlaywrightCliDriver::new(PlaywrightCliConfig {
            binary_path: Some("/nonexistent/aleph-playwright-cli".into()),
            ..PlaywrightCliConfig::default()
        });
        // Missing file → NotInstalled, same variant, so assert on the branch
        // by using a path that DOES exist instead.
        let exists = std::env::current_exe().expect("test binary path");
        let driver2 = PlaywrightCliDriver::new(PlaywrightCliConfig {
            binary_path: Some(exists.to_string_lossy().into_owned()),
            ..PlaywrightCliConfig::default()
        });
        assert!(driver.resolve_binary().await.is_err());
        assert_eq!(
            driver2.resolve_binary().await.ok(),
            Some(exists),
            "an explicit, existing binary_path must win over the test seal"
        );
    }

    #[test]
    fn test_classify_stderr_no_session() {
        let err = classify_failure("", "Error: no session found for -s=foo", 1, "foo", 5000);
        assert!(matches!(err, BrowserError::NoSession(_)));
    }

    /// The refusal `playwright-cli 0.1.8` actually emits, copied verbatim —
    /// and it arrives on **stdout** with stderr empty. A classifier that read
    /// only stderr called this a generic `PlaywrightCliError`, so the lazy
    /// launch could never trigger and the managed driver could never reach a
    /// browser at all.
    #[test]
    fn the_real_not_open_refusal_arrives_on_stdout_and_still_classifies() {
        let stdout = "The browser 'default' is not open, please run open first\n\n                        playwright-cli -s=default open [params]\n";
        let err = classify_failure(stdout, "", 1, "default", 5000);
        assert!(
            matches!(err, BrowserError::NoSession(_)),
            "stdout-only refusal must classify as NoSession, got {err:?}"
        );
    }

    #[test]
    fn test_classify_stderr_timeout() {
        let err = classify_failure("", "Error: action timeout 5000ms", 1, "foo", 5000);
        assert!(matches!(err, BrowserError::Timeout(5000)));
    }

    #[test]
    fn test_classify_stderr_element_not_found() {
        let err = classify_failure("", "element not found: #missing", 1, "foo", 5000);
        assert!(matches!(err, BrowserError::ActionFailed(_)));
    }

    #[test]
    fn test_classify_stderr_generic() {
        let err = classify_failure("", "something else", 2, "foo", 5000);
        assert!(matches!(err, BrowserError::PlaywrightCliError(_)));
    }

    #[test]
    fn test_is_secret_env_exact_matches() {
        assert!(is_secret_env("ANTHROPIC_API_KEY"));
        assert!(is_secret_env("ALEPH_VAULT_KEY"));
        assert!(is_secret_env("AWS_SECRET_ACCESS_KEY"));
        assert!(is_secret_env("DATABASE_URL"));
        // Case-insensitive
        assert!(is_secret_env("anthropic_api_key"));
    }

    #[test]
    fn test_is_secret_env_suffix_heuristic() {
        assert!(is_secret_env("ACME_API_KEY"));
        assert!(is_secret_env("SOME_SERVICE_TOKEN"));
        assert!(is_secret_env("MY_DB_PASSWORD"));
        assert!(is_secret_env("APP_PRIVATE_KEY"));
    }

    #[test]
    fn test_is_secret_env_allows_normal_vars() {
        assert!(!is_secret_env("PATH"));
        assert!(!is_secret_env("HOME"));
        assert!(!is_secret_env("LANG"));
        assert!(!is_secret_env("ALEPH_CHROME_PATH"));
    }
}
