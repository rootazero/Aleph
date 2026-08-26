//! 1Password CLI (`op`) secret provider.
//!
//! Implements `SecretProvider` by shelling out to the `op` CLI tool.
//! Supports both interactive sessions and service account tokens.

use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use tracing::debug;

use super::{ProviderStatus, SecretProvider};
use crate::secrets::types::SecretError;
use crate::utils::no_window::NoWindow;

/// Secret provider backed by the 1Password CLI (`op`).
///
/// Requires the `op` binary to be installed and available on `$PATH`.
/// Authentication can be via interactive `op signin` or a service account token.
pub struct OnePasswordProvider {
    account: Option<String>,
    service_account_token: Option<SecretString>,
}

/// Default wall-clock budget for a single `op` invocation.
///
/// `op` is interactive: it blocks on Touch ID / biometric / TTY prompts when
/// the user's session has expired. Without a timeout a hung child wedges
/// `health_check` (and the caller) indefinitely, and a dropped task leaves a
/// zombie child connected to the daemon's stdio. 5 s is comfortably above
/// any healthy `op` round-trip and short enough to surface a stuck prompt
/// before the next caller blocks on the same lock.
const OP_INVOCATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl OnePasswordProvider {
    /// Create a new 1Password provider.
    ///
    /// - `account`: Optional 1Password account shorthand (passed as `--account`).
    /// - `service_account_token`: Optional service account token (set as `OP_SERVICE_ACCOUNT_TOKEN`).
    #[must_use]
    pub const fn new(account: Option<String>, service_account_token: Option<SecretString>) -> Self {
        Self {
            account,
            service_account_token,
        }
    }

    /// Build a base `op` command with account and token pre-configured.
    ///
    /// Always sets `kill_on_drop(true)` so any task drop (timeout, cancellation
    /// token, panic unwind) reaps the child instead of leaving a zombie whose
    /// stdio is still connected to the daemon.
    fn base_command(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("op");
        if let Some(ref account) = self.account {
            cmd.arg("--account").arg(account);
        }
        if let Some(ref token) = self.service_account_token {
            cmd.env("OP_SERVICE_ACCOUNT_TOKEN", token.expose_secret());
        }
        cmd.kill_on_drop(true);
        cmd.no_window();
        cmd
    }

    /// Classify stderr output into a typed `SecretError`.
    ///
    /// Returns sanitized user-facing messages without raw stderr content
    /// (which may contain vault names, account emails, or internal debug output).
    fn classify_error(stderr: &str) -> SecretError {
        let lower = stderr.to_lowercase();
        debug!("1Password stderr: {}", stderr.trim());
        if lower.contains("not signed in")
            || lower.contains("session expired")
            || lower.contains("authorization prompt")
            || lower.contains("sign in")
        {
            SecretError::ProviderAuthRequired {
                provider: "1password".into(),
                message: "1Password session expired or not signed in. Run `op signin`.".into(),
            }
        } else if lower.contains("not found")
            || lower.contains("doesn't exist")
            || lower.contains("no item")
        {
            SecretError::NotFound("Item not found in 1Password vault".into())
        } else {
            SecretError::ProviderError {
                provider: "1password".into(),
                message: "1Password CLI returned an error. Check logs for details.".into(),
            }
        }
    }
}

#[async_trait]
impl SecretProvider for OnePasswordProvider {
    fn provider_type(&self) -> &str {
        "1password"
    }

    async fn health_check(&self) -> Result<ProviderStatus, SecretError> {
        let mut cmd = self.base_command();
        cmd.arg("whoami");

        // Bound the wait: a stuck interactive prompt or a hung CLI process
        // must not block `health_check` indefinitely. `kill_on_drop(true)`
        // (set in `base_command`) reaps the child if the timeout future is
        // dropped before `output()` resolves.
        let output = match tokio::time::timeout(OP_INVOCATION_TIMEOUT, cmd.output()).await {
            Ok(res) => res.map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    SecretError::ProviderError {
                        provider: "1password".into(),
                        message: "1Password CLI (`op`) not found".into(),
                    }
                } else {
                    SecretError::ProviderError {
                        provider: "1password".into(),
                        message: format!("Failed to execute `op whoami`: {e}"),
                    }
                }
            })?,
            Err(_elapsed) => {
                return Ok(ProviderStatus::Unavailable {
                    reason: format!(
                        "`op whoami` timed out after {}s — \
                         the 1Password CLI may be waiting for an interactive prompt",
                        OP_INVOCATION_TIMEOUT.as_secs()
                    ),
                });
            }
        };

        if output.status.success() {
            Ok(ProviderStatus::Ready)
        } else {
            // Don't leak raw stderr (may contain account emails, vault names).
            // classify_error logs the raw stderr via tracing::debug.
            let err = Self::classify_error(&String::from_utf8_lossy(&output.stderr));
            match &err {
                SecretError::ProviderAuthRequired { .. } => Ok(ProviderStatus::NeedsAuth {
                    message: "Run `op signin` to authenticate with 1Password.".into(),
                }),
                _ => Ok(ProviderStatus::Unavailable {
                    reason: format!("1Password CLI error: {err}"),
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_error_auth() {
        let err = OnePasswordProvider::classify_error("You are not signed in");
        assert!(matches!(err, SecretError::ProviderAuthRequired { .. }));
    }

    #[test]
    fn test_classify_error_not_found() {
        let err = OnePasswordProvider::classify_error("item not found in vault");
        assert!(matches!(err, SecretError::NotFound(_)));
    }

    #[test]
    fn test_classify_error_generic() {
        let err = OnePasswordProvider::classify_error("some random error");
        assert!(matches!(err, SecretError::ProviderError { .. }));
    }

    #[test]
    fn test_provider_type() {
        let provider = OnePasswordProvider::new(None, None);
        assert_eq!(provider.provider_type(), "1password");
    }

    #[tokio::test]
    #[ignore]
    async fn test_health_check_live() {
        let provider = OnePasswordProvider::new(None, None);
        let status = provider.health_check().await.unwrap();
        println!("1Password status: {:?}", status);
    }
}
