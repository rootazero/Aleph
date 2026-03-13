//! Secret management command handlers.
//!
//! All secret operations go through SharedTokenManager, which uses the
//! auth token as master key for AES-256-GCM encryption.

use std::error::Error;
use std::io::Write;
use std::path::PathBuf;

use alephcore::gateway::security::{SharedTokenManager, store::SecurityStore};
use alephcore::utils::paths;
use std::sync::Arc;

use crate::cli::SecretAction;

const SECRET_NAME_MAX_LEN: usize = 128;

/// Validate and normalize a secret name.
pub fn validate_secret_name(name: &str) -> Result<String, String> {
    let normalized = name.trim();

    if normalized.is_empty() {
        return Err("Secret name cannot be empty".to_string());
    }

    if normalized.len() > SECRET_NAME_MAX_LEN {
        return Err(format!(
            "Secret name must be <= {} characters",
            SECRET_NAME_MAX_LEN
        ));
    }

    let valid = normalized
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'));
    if !valid {
        return Err(
            "Secret name can only contain ASCII letters, digits, '_', '-', and '.'".to_string(),
        );
    }

    Ok(normalized.to_string())
}

/// Build a SharedTokenManager and load the existing token from file.
/// Returns an error if no token exists (server must be started at least once).
fn open_token_manager() -> Result<(SharedTokenManager, PathBuf), Box<dyn Error>> {
    let security_store_path = paths::get_security_db_path()
        .unwrap_or_else(|_| PathBuf::from("/tmp/aleph_security.db"));
    let security_store = Arc::new(
        SecurityStore::open(&security_store_path)
            .unwrap_or_else(|e| {
                eprintln!("Warning: Failed to load security store from {:?}: {}. Using in-memory.", security_store_path, e);
                SecurityStore::in_memory().expect("Failed to create in-memory security store")
            })
    );

    let data_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".aleph/data");
    let vault_path = data_dir.join("secrets.vault");
    let token_file = data_dir.join(".shared_token");

    let manager = SharedTokenManager::new(security_store, vault_path);

    // Load existing token from file
    if manager.try_load_token_from_file(&token_file).is_none() {
        return Err("No valid token found. Start the server at least once to generate one.".into());
    }

    Ok((manager, token_file))
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

fn handle_secret_init() -> Result<(), Box<dyn Error>> {
    let (manager, _) = open_token_manager()?;
    let count = manager.list_secret_names()
        .map(|names| names.len())
        .unwrap_or(0);
    println!("Secret vault ready ({} entries)", count);
    Ok(())
}

fn handle_secret_set(name: String, value: Option<String>) -> Result<(), Box<dyn Error>> {
    let name = validate_secret_name(&name)?;
    let value = resolve_secret_value(value)?;

    let (manager, _) = open_token_manager()?;
    manager.store_secret(&name, &value)
        .map_err(|e| format!("Failed to store secret: {}", e))?;

    println!("Stored secret '{}'", name);
    Ok(())
}

fn handle_secret_list() -> Result<(), Box<dyn Error>> {
    let (manager, _) = open_token_manager()?;
    let mut names = manager.list_secret_names()
        .map_err(|e| format!("Failed to list secrets: {}", e))?;
    names.sort();

    if names.is_empty() {
        println!("No secrets found");
        return Ok(());
    }

    println!("{:<40}", "NAME");
    println!("{}", "-".repeat(40));
    for name in names {
        println!("{:<40}", name);
    }
    Ok(())
}

fn handle_secret_delete(name: String) -> Result<(), Box<dyn Error>> {
    let name = validate_secret_name(&name)?;
    let (manager, _) = open_token_manager()?;

    let deleted = manager.delete_secret(&name)
        .map_err(|e| format!("Failed to delete secret: {}", e))?;
    if deleted {
        println!("Deleted secret '{}'", name);
    } else {
        eprintln!("Secret '{}' not found", name);
        std::process::exit(1);
    }
    Ok(())
}

fn handle_secret_verify(name: String) -> Result<(), Box<dyn Error>> {
    let name = validate_secret_name(&name)?;
    let (manager, _) = open_token_manager()?;

    match manager.get_secret(&name) {
        Ok(Some(secret)) => {
            println!(
                "Secret '{}' is available ({} bytes, value redacted)",
                name,
                secret.expose().len()
            );
        }
        Ok(None) => {
            eprintln!("Secret '{}' not found", name);
            std::process::exit(1);
        }
        Err(e) => {
            return Err(format!("Failed to verify secret: {}", e).into());
        }
    }
    Ok(())
}

fn handle_secret_providers() -> Result<(), Box<dyn Error>> {
    use alephcore::secrets::provider::onepassword::OnePasswordProvider;
    use alephcore::secrets::provider::SecretProvider;
    use alephcore::secrets::ProviderStatus;

    let config = alephcore::Config::load().unwrap_or_default();

    println!("{:<15} {:<15} STATUS", "KEY", "TYPE");
    println!("{}", "-".repeat(55));

    // Always show built-in local vault
    println!("{:<15} {:<15} Ready (built-in)", "local", "local_vault");

    // Show configured external providers
    for (key, provider_config) in &config.secret_providers {
        match provider_config.provider_type.as_str() {
            "local_vault" => {
                println!("{:<15} {:<15} Ready (built-in)", key, "local_vault");
            }
            "1password" => {
                let token = provider_config
                    .service_account_token_env
                    .as_ref()
                    .and_then(|env_name| std::env::var(env_name).ok());
                let op = OnePasswordProvider::new(provider_config.account.clone(), token);

                let rt = tokio::runtime::Runtime::new()?;
                match rt.block_on(op.health_check()) {
                    Ok(ProviderStatus::Ready) => {
                        println!("{:<15} {:<15} Ready", key, "1password");
                    }
                    Ok(ProviderStatus::NeedsAuth { message }) => {
                        println!("{:<15} {:<15} Needs Auth: {}", key, "1password", message);
                    }
                    Ok(ProviderStatus::Unavailable { reason }) => {
                        println!("{:<15} {:<15} Unavailable: {}", key, "1password", reason);
                    }
                    Err(e) => {
                        println!("{:<15} {:<15} Error: {}", key, "1password", e);
                    }
                }
            }
            other => {
                println!("{:<15} {:<15} Unknown type", key, other);
            }
        }
    }

    Ok(())
}

/// Handle secret subcommands.
pub fn handle_secret_command(action: SecretAction) -> Result<(), Box<dyn Error>> {
    match action {
        SecretAction::Init => handle_secret_init(),
        SecretAction::Set { name, value } => handle_secret_set(name, value),
        SecretAction::List => handle_secret_list(),
        SecretAction::Delete { name } => handle_secret_delete(name),
        SecretAction::Verify { name } => handle_secret_verify(name),
        SecretAction::Providers => handle_secret_providers(),
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
