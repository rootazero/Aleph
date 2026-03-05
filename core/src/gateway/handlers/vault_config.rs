//! Vault Configuration Handlers
//!
//! Stateless RPC handlers for vault master key management via OS keychain.
//!
//! | RPC Method       | Handler            | Description                        |
//! |------------------|--------------------|------------------------------------|
//! | vault.status     | handle_status      | Return VaultStatus (no key values) |
//! | vault.storeKey   | handle_store_key   | Validate & store key in keychain   |
//! | vault.deleteKey  | handle_delete_key  | Remove key from keychain           |
//! | vault.verify     | handle_verify      | Verify key can open the vault      |

use serde::Deserialize;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::secrets::{
    vault_status, store_master_key_to_keyring, delete_master_key_from_keyring,
    resolve_master_key, SecretVault,
};
use crate::secrets::migration::{
    needs_migration, migrate_api_keys, save_migrated_config,
    needs_reverse_migration, reverse_migrate_api_keys,
};

/// Handle vault.status — returns current vault status without exposing secrets.
pub async fn handle_status(request: JsonRpcRequest) -> JsonRpcResponse {
    let status = vault_status();
    let result = serde_json::to_value(&status)
        .unwrap_or_else(|_| serde_json::json!({}));
    JsonRpcResponse::success(request.id, result)
}

#[derive(Deserialize)]
struct StoreKeyParams {
    master_key: String,
}

/// Handle vault.storeKey — validate the key against existing vault (if any), then store in keychain.
pub async fn handle_store_key(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: StoreKeyParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    if params.master_key.is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Master key cannot be empty");
    }

    // If vault file exists, verify the key can open it before storing
    let vault_path = SecretVault::default_path();
    if vault_path.exists() {
        match SecretVault::open(&vault_path, &params.master_key) {
            Ok(vault) => {
                // If vault has entries, try decrypting one to verify the key is correct
                let names: Vec<String> = vault.list().into_iter().map(|(n, _)| n).collect();
                if let Some(first) = names.first() {
                    if let Err(_) = vault.get(first) {
                        return JsonRpcResponse::error(
                            request.id,
                            INVALID_PARAMS,
                            "Key does not match existing vault — decryption failed",
                        );
                    }
                }
            }
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Cannot open existing vault with this key: {}", e),
                );
            }
        }
    }

    match store_master_key_to_keyring(&params.master_key) {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            serde_json::json!({ "success": true }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to store key in keychain: {}", e),
        ),
    }
}

/// Handle vault.deleteKey — remove the master key from the OS keychain.
pub async fn handle_delete_key(request: JsonRpcRequest) -> JsonRpcResponse {
    match delete_master_key_from_keyring() {
        Ok(deleted) => JsonRpcResponse::success(
            request.id,
            serde_json::json!({ "success": true, "was_present": deleted }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to delete key from keychain: {}", e),
        ),
    }
}

/// Handle vault.verify — try opening the vault with the currently resolved key.
pub async fn handle_verify(request: JsonRpcRequest) -> JsonRpcResponse {
    let key = match resolve_master_key() {
        Ok(k) => k,
        Err(_) => {
            return JsonRpcResponse::success(
                request.id,
                serde_json::json!({
                    "ok": false,
                    "message": "No master key configured"
                }),
            );
        }
    };

    let vault_path = SecretVault::default_path();
    if !vault_path.exists() {
        return JsonRpcResponse::success(
            request.id,
            serde_json::json!({
                "ok": true,
                "message": "Key is configured. No vault file yet — it will be created on first secret storage."
            }),
        );
    }

    match SecretVault::open(&vault_path, &key) {
        Ok(vault) => {
            // Try decrypting an entry if available
            let names: Vec<String> = vault.list().into_iter().map(|(n, _)| n).collect();
            if let Some(first) = names.first() {
                match vault.get(first) {
                    Ok(_) => JsonRpcResponse::success(
                        request.id,
                        serde_json::json!({
                            "ok": true,
                            "message": format!("Vault unlocked successfully ({} entries)", vault.len())
                        }),
                    ),
                    Err(_) => JsonRpcResponse::success(
                        request.id,
                        serde_json::json!({
                            "ok": false,
                            "message": "Key does not match vault — decryption failed"
                        }),
                    ),
                }
            } else {
                JsonRpcResponse::success(
                    request.id,
                    serde_json::json!({
                        "ok": true,
                        "message": "Vault opened successfully (empty)"
                    }),
                )
            }
        }
        Err(e) => JsonRpcResponse::success(
            request.id,
            serde_json::json!({
                "ok": false,
                "message": format!("Failed to open vault: {}", e)
            }),
        ),
    }
}

#[derive(Deserialize)]
struct MigrateKeysParams {
    master_key: String,
}

/// Handle vault.migrateKeys — migrate all plaintext API keys to the vault.
///
/// Accepts `{ master_key }` from the caller (UI provides the key directly).
/// On success, best-effort stores the key in the OS keychain.
pub async fn handle_migrate_keys(request: JsonRpcRequest) -> JsonRpcResponse {
    // 1. Parse master key from params
    let params: MigrateKeysParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if params.master_key.is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Master key cannot be empty");
    }

    // 2. Load config
    let mut config = match crate::config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to load config: {}", e),
            );
        }
    };

    // 3. Check if migration is needed
    if !needs_migration(&config) {
        return JsonRpcResponse::success(
            request.id,
            serde_json::json!({
                "migrated_count": 0,
                "migrated_providers": []
            }),
        );
    }

    // 4. Open vault
    let vault_path = SecretVault::default_path();
    let mut vault = match SecretVault::open(&vault_path, &params.master_key) {
        Ok(v) => v,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to open vault: {}", e),
            );
        }
    };

    // 5. Migrate
    let result = match migrate_api_keys(&mut config, &mut vault) {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Migration failed: {}", e),
            );
        }
    };

    // 6. Save config (removes plaintext keys from file)
    if let Err(e) = save_migrated_config(&config) {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Keys stored in vault but failed to update config file: {}", e),
        );
    }

    // 7. Best-effort store key in keychain
    let _ = store_master_key_to_keyring(&params.master_key);

    // 8. Return result
    JsonRpcResponse::success(
        request.id,
        serde_json::json!({
            "migrated_count": result.migrated_count,
            "migrated_providers": result.migrated_providers,
        }),
    )
}

#[derive(Deserialize)]
struct DisableVaultParams {
    master_key: String,
    #[serde(default)]
    remove_from_keychain: bool,
}

/// Handle vault.disableVault — reverse-migrate all encrypted keys back to plaintext.
///
/// Accepts `{ master_key, remove_from_keychain? }`.
/// Decrypts each vault entry back into the config's api_key field.
pub async fn handle_disable_vault(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: DisableVaultParams = match super::parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if params.master_key.is_empty() {
        return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Master key cannot be empty");
    }

    // Load config
    let mut config = match crate::config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to load config: {}", e),
            );
        }
    };

    if !needs_reverse_migration(&config) {
        return JsonRpcResponse::success(
            request.id,
            serde_json::json!({
                "restored_count": 0,
                "restored_providers": [],
                "keychain_removed": false,
            }),
        );
    }

    // Open vault with the provided master key
    let vault_path = SecretVault::default_path();
    let mut vault = match SecretVault::open(&vault_path, &params.master_key) {
        Ok(v) => v,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to open vault (wrong key?): {}", e),
            );
        }
    };

    // Reverse migrate
    let result = match reverse_migrate_api_keys(&mut config, &mut vault) {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Reverse migration failed: {}", e),
            );
        }
    };

    // Save config (restores plaintext keys to file)
    if let Err(e) = save_migrated_config(&config) {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Keys decrypted but failed to update config file: {}", e),
        );
    }

    // Optionally remove master key from keychain
    let keychain_removed = if params.remove_from_keychain {
        delete_master_key_from_keyring().unwrap_or(false)
    } else {
        false
    };

    JsonRpcResponse::success(
        request.id,
        serde_json::json!({
            "restored_count": result.restored_count,
            "restored_providers": result.restored_providers,
            "keychain_removed": keychain_removed,
        }),
    )
}
