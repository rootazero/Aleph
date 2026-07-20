//! `PlaywrightCliDriver` — manages per-session `playwright-cli` subprocesses.
//!
//! Each tool call spawns a fresh process with `-s=<session_key>`; the CLI
//! keeps browser state in memory across invocations under the same key.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::Mutex;

use crate::runtimes::{ensure_capability, ledger::CapabilityLedger};
use crate::security::secret_env::is_secret_env;
use crate::sync_primitives::RwLock;
use crate::utils::no_window::NoWindow;

use super::error::BrowserError;
use super::profile::PlaywrightCliConfig;

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
        // Slow path: ensure the full chain (fnm → node → playwright-cli + chromium + skills).
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

    fn session_lock(&self, session_key: &str) -> Arc<Mutex<()>> {
        let mut map = self
            .per_session_locks
            .write()
            .unwrap_or_else(|e| e.into_inner());
        map.entry(session_key.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Spawn `playwright-cli -s=<session_key> <args>` and capture output.
    /// Serializes concurrent calls within the same `session_key`.
    pub async fn run(
        &self,
        session_key: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<CliOutput, BrowserError> {
        let bin = self.resolve_binary().await?;
        let lock = self.session_lock(session_key);
        let _guard = lock.lock().await;

        let session_flag = format!("-s={session_key}");
        let mut cmd = Command::new(&bin);
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
            return Err(classify_stderr(
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

fn classify_stderr(
    stderr: &str,
    exit_code: i32,
    session_key: &str,
    timeout_ms: u64,
) -> BrowserError {
    let s = stderr.to_lowercase();
    if s.contains("no session") || s.contains("browser not open") {
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

    #[test]
    fn test_classify_stderr_no_session() {
        let err = classify_stderr("Error: no session found for -s=foo", 1, "foo", 5000);
        assert!(matches!(err, BrowserError::NoSession(_)));
    }

    #[test]
    fn test_classify_stderr_timeout() {
        let err = classify_stderr("Error: action timeout 5000ms", 1, "foo", 5000);
        assert!(matches!(err, BrowserError::Timeout(5000)));
    }

    #[test]
    fn test_classify_stderr_element_not_found() {
        let err = classify_stderr("element not found: #missing", 1, "foo", 5000);
        assert!(matches!(err, BrowserError::ActionFailed(_)));
    }

    #[test]
    fn test_classify_stderr_generic() {
        let err = classify_stderr("something else", 2, "foo", 5000);
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
