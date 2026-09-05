//! Secret management command handlers.
//!
//! All secret operations go through `SharedTokenManager`, which uses the
//! auth token as master key for AES-256-GCM encryption.
//!
//! Spec C policy:
//! - `secret init` / `secret list` / `secret set`: **`LockOrIpc`** — when
//!   the server holds the singleton lock, forward to
//!   `/v1/admin/secrets` (handlers shipped in Task 16). When no server
//!   is running, take the lock locally and operate directly on the
//!   vault.
//! - `secret delete` / `secret verify`: **`LockOnly`** — these target a
//!   key-by-name path (e.g. `/v1/admin/secrets/{key}`). The current
//!   `CommandPolicy::LockOrIpc { route: &'static str }` does not
//!   support dynamic path segments, so we refuse cleanly when the
//!   server is up and ask the operator to stop the server first. A
//!   future spec can promote these to `LockOrIpc` once the policy
//!   supports owned route strings.
//! - `secret providers`: **`NoLock`** — prints the built-in local vault;
//!   never touches the vault.

use std::error::Error;
use std::io::Write;

use alephcore::gateway::security::{store::SecurityStore, SharedTokenManager};
use alephcore::secrets::validate_secret_name;
use alephcore::utils::paths;
use std::sync::Arc;

use crate::cli::SecretAction;

/// Build a `SharedTokenManager` and load the existing token from DB.
/// Returns an error if no token exists (server must be started at least once).
fn open_token_manager() -> Result<SharedTokenManager, Box<dyn Error>> {
    let security_store_path = paths::get_security_db_path()
        .map_err(|e| format!("Failed to resolve security DB path: {e}"))?;
    let security_store = Arc::new(
        SecurityStore::open(&security_store_path)
            .map_err(|e| format!("Failed to open security store: {e}"))?,
    );

    // Resolve via the authoritative resolver (honours ALEPH_HOME / $HOME) so
    // the CLI `secret` command and the running daemon agree on the SAME vault.
    let data_dir = alephcore::utils::paths::get_data_dir()
        .map_err(|e| format!("Cannot determine data directory: {e}"))?;
    let vault_path = data_dir.join("secrets.vault");

    let manager = SharedTokenManager::new(security_store, vault_path);

    // Load existing token from DB
    if manager.try_load_token_from_db().is_none() {
        return Err("No valid token found. Start the server at least once to generate one.".into());
    }

    Ok(manager)
}

fn resolve_secret_value(value: Option<String>) -> Result<String, Box<dyn Error>> {
    if let Some(value) = value {
        return Ok(value);
    }

    eprint!("Secret value (input hidden): ");
    std::io::stderr().flush()?;
    let value = rpassword::read_password()?;
    if value.is_empty() {
        return Err("Secret value cannot be empty".into());
    }
    Ok(value)
}

/// Local idempotent initialization: open the vault manager without requiring
/// an existing token; if a token (and hence the vault) is already present
/// the call is a no-op, otherwise we generate a token so subsequent
/// `secret set`/`get` calls have a master key. The token itself is never
/// returned — only the ready/no-op status is reported.
fn init_locked(data_dir: &std::path::Path) -> Result<bool, Box<dyn Error>> {
    use alephcore::gateway::security::store::SecurityStore;
    use alephcore::gateway::security::SharedTokenManager;

    let security_store_path = data_dir.join("security.db");
    let store = std::sync::Arc::new(
        SecurityStore::open(&security_store_path)
            .map_err(|e| format!("Failed to open security store: {e}"))?,
    );
    let vault_path = data_dir.join("secrets.vault");
    let manager = SharedTokenManager::new(store, vault_path);

    if manager.try_load_token_from_db().is_some() || manager.has_stored_token() {
        return Ok(false);
    }
    manager
        .generate_token()
        .map_err(|e| format!("Failed to initialize vault: {e}"))?;
    Ok(true)
}

/// Local fast-path: list secrets while holding the singleton lock.
fn list_locked(
) -> Result<Vec<alephcore::gateway::admin_api::secrets::SecretSummary>, Box<dyn Error>> {
    use alephcore::gateway::admin_api::secrets::SecretSummary;
    let manager = open_token_manager()?;
    let names = manager
        .list_secret_names()
        .map_err(|e| format!("Failed to list secrets: {e}"))?;
    Ok(names.into_iter().map(|key| SecretSummary { key }).collect())
}

/// Local fast-path: store a secret while holding the singleton lock.
fn set_locked(
    name: &str,
    value: &str,
) -> Result<alephcore::gateway::admin_api::secrets::SecretSummary, Box<dyn Error>> {
    use alephcore::gateway::admin_api::secrets::SecretSummary;
    let manager = open_token_manager()?;
    manager
        .store_secret(name, value)
        .map_err(|e| format!("Failed to store secret: {e}"))?;
    Ok(SecretSummary {
        key: name.to_string(),
    })
}

/// Local: delete a secret while holding the singleton lock.
fn delete_locked(name: &str) -> Result<(), Box<dyn Error>> {
    let manager = open_token_manager()?;
    let deleted = manager
        .delete_secret(name)
        .map_err(|e| format!("Failed to delete secret: {e}"))?;
    if !deleted {
        return Err(format!("Secret '{name}' not found").into());
    }
    Ok(())
}

/// Local: verify a secret while holding the singleton lock. Returns
/// the byte length of the redacted value on success.
fn verify_locked(name: &str) -> Result<usize, Box<dyn Error>> {
    let manager = open_token_manager()?;
    match manager
        .get_secret(name)
        .map_err(|e| format!("Failed to verify secret: {e}"))?
    {
        Some(secret) => Ok(secret.expose().len()),
        None => Err(format!("Secret '{name}' not found").into()),
    }
}

fn handle_secret_providers() -> Result<(), Box<dyn Error>> {
    // The external-provider plugin point (the `SecretProvider` trait, the
    // 1Password stub, the `[secret_providers]` config table) was removed in
    // the 2026-09-05 audit pass (secrets I-3): the trait never grew a
    // `get_secret`, so a configured external provider could never resolve a
    // single secret — the listing advertised a capability that did not
    // exist. What remains is the truth: the built-in local vault.
    println!("{:<15} {:<15} STATUS", "KEY", "TYPE");
    println!("{}", "-".repeat(55));
    println!("{:<15} {:<15} Ready (built-in)", "local", "local_vault");
    Ok(())
}

/// Handle secret subcommands. Each action declares its Spec C policy
/// and dispatches through `with_policy` (`LockOrIpc` / `LockOnly`) or
/// `run_no_lock` (Providers) so the server-vs-CLI race never reaches
/// `VaultIo`'s exclusive fcntl lock.
pub fn handle_secret_command(action: SecretAction) -> Result<(), Box<dyn Error>> {
    use alephcore::cli::policy::{run_no_lock, with_policy, CommandPolicy, HttpMethod};
    use alephcore::gateway::admin_api::secrets::SecretSummary;

    let data_dir = paths::get_data_dir().map_err(|e| format!("data dir: {e}"))?;

    match action {
        SecretAction::Init => {
            let summaries: Vec<SecretSummary> = with_policy(
                CommandPolicy::LockOrIpc {
                    route: "/v1/admin/secrets",
                    method: HttpMethod::Get,
                },
                &data_dir,
                |_lock| -> anyhow::Result<Vec<SecretSummary>> {
                    init_locked(&data_dir).map_err(|e| anyhow::anyhow!("{e}"))?;
                    list_locked().map_err(|e| anyhow::anyhow!("{e}"))
                },
                serde_json::Value::Null,
            )
            .map_err(|e| -> Box<dyn Error> { format!("{e:#}").into() })?;
            println!("Secret vault ready ({} entries)", summaries.len());
            Ok(())
        }
        SecretAction::Set { name, value } => {
            let name = validate_secret_name(&name)?;
            let value = resolve_secret_value(value)?;
            let body = serde_json::json!({ "key": &name, "value": &value });
            let name_local = name;
            let value_local = value;
            let summary: SecretSummary = with_policy(
                CommandPolicy::LockOrIpc {
                    route: "/v1/admin/secrets",
                    method: HttpMethod::Post,
                },
                &data_dir,
                move |_lock| {
                    set_locked(&name_local, &value_local).map_err(|e| anyhow::anyhow!("{e}"))
                },
                body,
            )
            .map_err(|e| -> Box<dyn Error> { format!("{e:#}").into() })?;
            println!("Stored secret '{}'", summary.key);
            Ok(())
        }
        SecretAction::List => {
            let summaries: Vec<SecretSummary> = with_policy(
                CommandPolicy::LockOrIpc {
                    route: "/v1/admin/secrets",
                    method: HttpMethod::Get,
                },
                &data_dir,
                |_lock| list_locked().map_err(|e| anyhow::anyhow!("{e}")),
                serde_json::Value::Null,
            )
            .map_err(|e| -> Box<dyn Error> { format!("{e:#}").into() })?;

            if summaries.is_empty() {
                println!("No secrets found");
                return Ok(());
            }

            let mut names: Vec<String> = summaries.into_iter().map(|s| s.key).collect();
            names.sort();
            println!("{:<40}", "NAME");
            println!("{}", "-".repeat(40));
            for name in names {
                println!("{name:<40}");
            }
            Ok(())
        }
        SecretAction::Delete { name } => {
            let name = validate_secret_name(&name)?;
            let name_local = name.clone();
            with_policy::<_, ()>(
                CommandPolicy::LockOnly,
                &data_dir,
                move |_lock| delete_locked(&name_local).map_err(|e| anyhow::anyhow!("{e}")),
                serde_json::Value::Null,
            )
            .map_err(|e| -> Box<dyn Error> { format!("{e:#}").into() })?;
            println!("Deleted secret '{name}'");
            Ok(())
        }
        SecretAction::Verify { name } => {
            let name = validate_secret_name(&name)?;
            let name_local = name.clone();
            let len: usize = with_policy(
                CommandPolicy::LockOnly,
                &data_dir,
                move |_lock| verify_locked(&name_local).map_err(|e| anyhow::anyhow!("{e}")),
                serde_json::Value::Null,
            )
            .map_err(|e| -> Box<dyn Error> { format!("{e:#}").into() })?;
            println!("Secret '{name}' is available ({len} bytes, value redacted)");
            Ok(())
        }
        SecretAction::Providers => {
            // NoLock: doesn't touch the vault.
            run_no_lock(|| Ok::<(), anyhow::Error>(()))
                .map_err(|e| -> Box<dyn Error> { format!("{e:#}").into() })?;
            handle_secret_providers()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::validate_secret_name;

    #[test]
    fn test_secret_name_rejects_empty() {
        assert!(validate_secret_name("").is_err());
        assert!(validate_secret_name("   ").is_err());
    }

    #[test]
    fn test_secret_name_rejects_invalid_chars() {
        assert!(validate_secret_name("bad name").is_err());
        assert!(validate_secret_name("bad/name").is_err());
        assert!(validate_secret_name("bad$name").is_err());
    }

    #[test]
    fn test_secret_name_trims_whitespace() {
        assert_eq!(
            validate_secret_name("  openai.main-key_1  ").unwrap(),
            "openai.main-key_1"
        );
    }
}
