//! Secret management command handlers.

use std::error::Error;
use std::io::Write;

use alephcore::secrets::types::EntryMetadata;
use alephcore::secrets::{resolve_master_key, SecretVault};

use crate::cli::SecretAction;

const SECRET_NAME_MAX_LEN: usize = 128;
const INIT_SENTINEL: &str = "__aleph_secret_init_probe__";

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

fn open_vault() -> Result<SecretVault, Box<dyn Error>> {
    let master_key = resolve_master_key()?;
    Ok(SecretVault::open(SecretVault::default_path(), &master_key)?)
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
    let path = SecretVault::default_path();
    let existed = path.exists();

    let mut vault = open_vault()?;
    if !existed {
        // Force the vault file to materialize on disk.
        vault.set(INIT_SENTINEL, "", EntryMetadata::default())?;
        let _ = vault.delete(INIT_SENTINEL)?;
    }

    println!("Secret vault ready at {}", path.display());
    Ok(())
}

fn handle_secret_set(name: String, value: Option<String>) -> Result<(), Box<dyn Error>> {
    let name = validate_secret_name(&name)?;
    let value = resolve_secret_value(value)?;

    let mut vault = open_vault()?;
    vault.set(&name, &value, EntryMetadata::default())?;

    println!("Stored secret '{}'", name);
    Ok(())
}

fn handle_secret_list() -> Result<(), Box<dyn Error>> {
    let vault = open_vault()?;
    let mut entries = vault.list();
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));

    if entries.is_empty() {
        println!("No secrets found");
        return Ok(());
    }

    println!("{:<40} {:<20}", "NAME", "PROVIDER");
    println!("{}", "-".repeat(62));
    for (name, metadata) in entries {
        let provider = metadata.provider.as_deref().unwrap_or("-");
        println!("{:<40} {:<20}", name, provider);
    }
    Ok(())
}

fn handle_secret_delete(name: String) -> Result<(), Box<dyn Error>> {
    let name = validate_secret_name(&name)?;
    let mut vault = open_vault()?;

    if vault.delete(&name)? {
        println!("Deleted secret '{}'", name);
    } else {
        eprintln!("Secret '{}' not found", name);
        std::process::exit(1);
    }
    Ok(())
}

fn handle_secret_verify(name: String) -> Result<(), Box<dyn Error>> {
    let name = validate_secret_name(&name)?;
    let vault = open_vault()?;

    let secret = vault.get(&name)?;
    println!(
        "Secret '{}' is available ({} bytes, value redacted)",
        name,
        secret.len()
    );
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
