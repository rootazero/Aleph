//! Startup secret migration — detects plaintext secrets in config.toml and
//! moves them to the encrypted vault.
//!
//! This acts as a safety net: even if a user manually edits config or an LLM
//! writes an api_key directly into the TOML file, the next server start will
//! automatically migrate it to the vault and strip it from config.
//!
//! Covers all secret-bearing config sections:
//! - AI providers (`providers.*`)           → vault prefix `ai:`
//! - Generation providers (`generation.*`)  → vault prefix `gen:`
//! - Embedding providers (`memory.embedding.providers[]`) → vault prefix `embed:`
//! - Rerank config (`memory.rerank`)        → vault prefix `rerank:`
//! - Search backends (`search.backends.*`)  → vault prefix `search:`
//! - Channels (`channels.*`)               → vault prefix `channel:`

use crate::gateway::security::SharedTokenManager;
use crate::sync_primitives::Arc;
use crate::Config;
use tokio::sync::RwLock;

/// Migrate all plaintext secrets from config to vault at startup.
///
/// Returns the total number of secrets migrated. Saves config only if
/// any migrations occurred (incremental save per affected section).
pub async fn migrate_all_secrets_to_vault(
    app_config: &Arc<RwLock<Config>>,
    vault: &SharedTokenManager,
) {
    let mut cfg = app_config.write().await;
    let mut sections_to_save: Vec<&str> = Vec::new();

    // 1. AI providers: providers.{name}.api_key → ai:{name}
    let ai_migrated = migrate_ai_providers(&mut cfg, vault);
    if ai_migrated > 0 {
        sections_to_save.push("providers");
    }

    // 2. Generation providers: generation.{type}_providers.{name}.api_key → gen:{name}
    let gen_migrated = migrate_generation_providers(&mut cfg, vault);
    if gen_migrated > 0 {
        sections_to_save.push("generation");
    }

    // 3. Embedding providers: memory.embedding.providers[].api_key → embed:{id}
    let embed_migrated = migrate_embedding_providers(&mut cfg, vault);

    // 4. Rerank: memory.rerank.api_key → rerank:{provider}
    let rerank_migrated = migrate_rerank(&mut cfg, vault);

    if embed_migrated + rerank_migrated > 0 {
        sections_to_save.push("memory");
    }

    // 5. Search backends: search.backends.{name}.api_key → search:{name}
    let search_migrated = migrate_search_backends(&mut cfg, vault);
    if search_migrated > 0 {
        sections_to_save.push("search");
    }

    // 6. Channels: channels.{id}.bot_token etc → channel:{id}:{field}
    //    (handled by channel::migrate_channel_secrets_to_vault, called separately)
    //    We include it here for unified logging.
    let channel_migrated = migrate_channels(&mut cfg, vault);
    if channel_migrated > 0 {
        sections_to_save.push("channels");
    }

    let total = ai_migrated
        + gen_migrated
        + embed_migrated
        + rerank_migrated
        + search_migrated
        + channel_migrated;

    if total > 0 {
        tracing::info!(
            "Secret migration: {} secret(s) moved to vault (sections: {:?})",
            total,
            sections_to_save
        );
        if let Err(e) = cfg.save_incremental(&sections_to_save) {
            tracing::error!(error = %e, "Failed to save config after secret migration");
        }
    }
}

// ── AI providers ────────────────────────────────────────────────────────────

fn migrate_ai_providers(cfg: &mut Config, vault: &SharedTokenManager) -> usize {
    let mut count = 0;
    let names: Vec<String> = cfg.providers.keys().cloned().collect();
    for name in names {
        if let Some(provider) = cfg.providers.get_mut(&name) {
            if let Some(ref key) = provider.api_key {
                if !key.is_empty() {
                    let vault_key = format!("ai:{}", name);
                    match vault.store_secret(&vault_key, key) {
                        Ok(_) => {
                            provider.api_key = None;
                            count += 1;
                            tracing::info!(provider = %name, "Migrated AI provider api_key to vault");
                        }
                        Err(e) => {
                            tracing::error!(provider = %name, error = %e, "Failed to store AI provider secret");
                        }
                    }
                }
            }
        }
    }
    count
}

// ── Generation providers ────────────────────────────────────────────────────

fn migrate_generation_providers(cfg: &mut Config, vault: &SharedTokenManager) -> usize {
    let mut count = 0;

    let categories = [
        &mut cfg.generation.image_providers,
        &mut cfg.generation.video_providers,
        &mut cfg.generation.speech_providers,
        &mut cfg.generation.audio_providers,
    ];

    // We need to collect names first due to borrow checker
    for providers in categories {
        let names: Vec<String> = providers.keys().cloned().collect();
        for name in names {
            if let Some(provider) = providers.get_mut(&name) {
                if let Some(ref key) = provider.api_key {
                    if !key.is_empty() {
                        let vault_key = format!("gen:{}", name);
                        match vault.store_secret(&vault_key, key) {
                            Ok(_) => {
                                provider.api_key = None;
                                count += 1;
                                tracing::info!(provider = %name, "Migrated generation provider api_key to vault");
                            }
                            Err(e) => {
                                tracing::error!(provider = %name, error = %e, "Failed to store generation provider secret");
                            }
                        }
                    }
                }
            }
        }
    }

    count
}

// ── Embedding providers ─────────────────────────────────────────────────────

fn migrate_embedding_providers(cfg: &mut Config, vault: &SharedTokenManager) -> usize {
    let mut count = 0;
    for provider in &mut cfg.memory.embedding.providers {
        if let Some(ref key) = provider.api_key {
            if !key.is_empty() {
                let vault_key = format!("embed:{}", provider.id);
                match vault.store_secret(&vault_key, key) {
                    Ok(_) => {
                        tracing::info!(provider = %provider.id, "Migrated embedding provider api_key to vault");
                        provider.api_key = None;
                        count += 1;
                    }
                    Err(e) => {
                        tracing::error!(provider = %provider.id, error = %e, "Failed to store embedding provider secret");
                    }
                }
            }
        }
    }
    count
}

// ── Rerank ──────────────────────────────────────────────────────────────────

fn migrate_rerank(cfg: &mut Config, vault: &SharedTokenManager) -> usize {
    let rerank = &mut cfg.memory.rerank;
    if rerank.api_key.is_empty() {
        return 0;
    }
    let provider_name = format!("{:?}", rerank.provider).to_lowercase();
    let vault_key = format!("rerank:{}", provider_name);
    match vault.store_secret(&vault_key, &rerank.api_key) {
        Ok(_) => {
            tracing::info!(provider = %provider_name, "Migrated rerank api_key to vault");
            rerank.api_key = String::new();
            1
        }
        Err(e) => {
            tracing::error!(provider = %provider_name, error = %e, "Failed to store rerank secret");
            0
        }
    }
}

// ── Search backends ─────────────────────────────────────────────────────────

fn migrate_search_backends(cfg: &mut Config, vault: &SharedTokenManager) -> usize {
    let search = match cfg.search.as_mut() {
        Some(s) => s,
        None => return 0,
    };
    let mut count = 0;
    let names: Vec<String> = search.backends.keys().cloned().collect();
    for name in names {
        if let Some(backend) = search.backends.get_mut(&name) {
            if let Some(ref key) = backend.api_key {
                if !key.is_empty() {
                    let vault_key = format!("search:{}", name);
                    match vault.store_secret(&vault_key, key) {
                        Ok(_) => {
                            backend.api_key = None;
                            count += 1;
                            tracing::info!(backend = %name, "Migrated search backend api_key to vault");
                        }
                        Err(e) => {
                            tracing::error!(backend = %name, error = %e, "Failed to store search backend secret");
                        }
                    }
                }
            }
        }
    }
    count
}

// ── Channels ────────────────────────────────────────────────────────────────

fn migrate_channels(cfg: &mut Config, vault: &SharedTokenManager) -> usize {
    use super::channel::store_and_strip_channel_secrets;

    let channel_ids: Vec<String> = cfg.channels.keys().cloned().collect();
    let mut count = 0;
    for id in &channel_ids {
        if let Some(channel_config) = cfg.channels.get_mut(id) {
            count += store_and_strip_channel_secrets(id, channel_config, vault);
        }
    }
    count
}
