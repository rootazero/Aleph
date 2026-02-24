//! 1Password CLI (`op`) secret provider.
//!
//! Implements `SecretProvider` by shelling out to the `op` CLI tool.
//! Supports both interactive sessions and service account tokens.

use async_trait::async_trait;
use chrono::Utc;
use tracing::debug;

use super::{ProviderStatus, SecretMetadata, SecretProvider};
use crate::secrets::types::{DecryptedSecret, SecretError};

/// Secret provider backed by the 1Password CLI (`op`).
///
/// Requires the `op` binary to be installed and available on `$PATH`.
/// Authentication can be via interactive `op signin` or a service account token.
pub struct OnePasswordProvider {
    account: Option<String>,
    service_account_token: Option<String>,
}

impl OnePasswordProvider {
    /// Create a new 1Password provider.
    ///
    /// - `account`: Optional 1Password account shorthand (passed as `--account`).
    /// - `service_account_token`: Optional service account token (set as `OP_SERVICE_ACCOUNT_TOKEN`).
    pub fn new(account: Option<String>, service_account_token: Option<String>) -> Self {
        Self {
            account,
            service_account_token,
        }
    }

    /// Build a base `op` command with account and token pre-configured.
    fn base_command(&self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("op");
        if let Some(ref account) = self.account {
            cmd.arg("--account").arg(account);
        }
        if let Some(ref token) = self.service_account_token {
            cmd.env("OP_SERVICE_ACCOUNT_TOKEN", token);
        }
        cmd
    }

    /// Classify stderr output into a typed `SecretError`.
    fn classify_error(stderr: &str) -> SecretError {
        let lower = stderr.to_lowercase();
        if lower.contains("not signed in")
            || lower.contains("session expired")
            || lower.contains("authorization prompt")
            || lower.contains("sign in")
        {
            SecretError::ProviderAuthRequired {
                provider: "1password".into(),
                message: format!(
                    "1Password session expired or not signed in. Run `op signin`. Details: {}",
                    stderr.trim()
                ),
            }
        } else if lower.contains("not found")
            || lower.contains("doesn't exist")
            || lower.contains("no item")
        {
            SecretError::NotFound(stderr.trim().to_string())
        } else {
            SecretError::ProviderError {
                provider: "1password".into(),
                message: stderr.trim().to_string(),
            }
        }
    }
}

#[async_trait]
impl SecretProvider for OnePasswordProvider {
    fn provider_type(&self) -> &str {
        "1password"
    }

    async fn get(&self, reference: &str) -> Result<DecryptedSecret, SecretError> {
        let mut cmd = self.base_command();
        cmd.arg("read").arg(reference).arg("--no-newline");

        debug!(reference = reference, "Fetching secret from 1Password");

        let output = cmd.output().await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SecretError::ProviderError {
                    provider: "1password".into(),
                    message: "1Password CLI (`op`) not found. Install from https://1password.com/downloads/command-line/".into(),
                }
            } else {
                SecretError::ProviderError {
                    provider: "1password".into(),
                    message: format!("Failed to execute `op`: {}", e),
                }
            }
        })?;

        if output.status.success() {
            Ok(DecryptedSecret::new(
                String::from_utf8_lossy(&output.stdout).into_owned(),
            ))
        } else {
            Err(Self::classify_error(&String::from_utf8_lossy(
                &output.stderr,
            )))
        }
    }

    async fn health_check(&self) -> Result<ProviderStatus, SecretError> {
        let mut cmd = self.base_command();
        cmd.arg("whoami");

        let output = cmd.output().await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SecretError::ProviderError {
                    provider: "1password".into(),
                    message: "1Password CLI (`op`) not found".into(),
                }
            } else {
                SecretError::ProviderError {
                    provider: "1password".into(),
                    message: format!("Failed to execute `op whoami`: {}", e),
                }
            }
        })?;

        if output.status.success() {
            Ok(ProviderStatus::Ready)
        } else {
            Ok(ProviderStatus::NeedsAuth {
                message: format!(
                    "Run `op signin` to authenticate. Error: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            })
        }
    }

    async fn list(&self) -> Result<Vec<SecretMetadata>, SecretError> {
        let mut cmd = self.base_command();
        cmd.arg("item").arg("list").arg("--format=json");

        let output = cmd.output().await.map_err(|e| SecretError::ProviderError {
            provider: "1password".into(),
            message: format!("Failed to execute `op item list`: {}", e),
        })?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let items: Vec<serde_json::Value> =
                serde_json::from_str(&stdout).unwrap_or_default();
            Ok(items
                .iter()
                .filter_map(|item| {
                    let name = item.get("title")?.as_str()?.to_string();
                    let updated_at = item
                        .get("updated_at")
                        .and_then(|v| v.as_str())
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(Utc::now);
                    Some(SecretMetadata {
                        name,
                        provider: "1password".into(),
                        updated_at,
                    })
                })
                .collect())
        } else {
            Err(Self::classify_error(&String::from_utf8_lossy(
                &output.stderr,
            )))
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
