//! Startup secret migration — detects plaintext secrets in config.toml and
//! moves them to the encrypted vault.
//!
//! This acts as a safety net: even if a user manually edits config or an LLM
//! writes an `api_key` directly into the TOML file, the next server start will
//! automatically migrate it to the vault and strip it from config.
//!
//! Covers all secret-bearing config sections:
//! - AI providers (`providers.*`)           → vault prefix `ai:`
//! - Generation providers (`generation.*`)  → vault prefix `gen:`
//! - Embedding providers (`memory.embedding.providers[]`) → vault prefix `embed:`
//! - Rerank config (`memory.rerank`)        → vault prefix `rerank:`
//! - Search backends (`search.backends.*`)  → vault prefix `search:`
//! - Channels (`channels.*`)               → vault prefix `channel:`
//! - Voice streaming (`voice.streaming`)    → vault prefix `voice_streaming:`

use crate::gateway::security::SharedTokenManager;
use crate::Config;

/// Migrate all plaintext secrets from config to vault at startup.
///
/// Operates directly on a mutable Config — caller is responsible for any
/// surrounding lock. Saves config only if any migrations occurred
/// (incremental save per affected section).
///
/// Returns the total number of secrets migrated.
pub fn migrate_all_secrets_to_vault(cfg: &mut Config, vault: &SharedTokenManager) -> usize {
    let mut sections_to_save: Vec<&str> = Vec::new();

    // 1. AI providers: providers.{name}.api_key → ai:{name}
    let ai_migrated = migrate_ai_providers(cfg, vault);
    if ai_migrated > 0 {
        sections_to_save.push("providers");
    }

    // 2. Generation providers: generation.{type}_providers.{name}.api_key → gen:{name}
    let gen_migrated = migrate_generation_providers(cfg, vault);
    if gen_migrated > 0 {
        sections_to_save.push("generation");
    }

    // 3. Embedding providers: memory.embedding.providers[].api_key → embed:{id}
    let embed_migrated = migrate_embedding_providers(cfg, vault);

    // 4. Rerank: memory.rerank.api_key → rerank:{provider}
    let rerank_migrated = migrate_rerank(cfg, vault);

    if embed_migrated + rerank_migrated > 0 {
        sections_to_save.push("memory");
    }

    // 5. Search backends: search.backends.{name}.api_key → search:{name}
    let search_migrated = migrate_search_backends(cfg, vault);
    if search_migrated > 0 {
        sections_to_save.push("search");
    }

    // 6. Channels: channels.{id}.bot_token etc → channel:{id}:{field}
    let channel_migrated = migrate_channels(cfg, vault);
    if channel_migrated > 0 {
        sections_to_save.push("channels");
    }

    // 7. Voice streaming: voice.streaming.api_key → voice_streaming:api_key
    let voice_migrated = migrate_voice_streaming(cfg, vault);
    if voice_migrated > 0 {
        sections_to_save.push("voice_local");
    }

    let total = ai_migrated
        + gen_migrated
        + embed_migrated
        + rerank_migrated
        + search_migrated
        + channel_migrated
        + voice_migrated;

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

    total
}

// ── AI providers ────────────────────────────────────────────────────────────

fn migrate_ai_providers(cfg: &mut Config, vault: &SharedTokenManager) -> usize {
    let mut count = 0;
    let names: Vec<String> = cfg.providers.keys().cloned().collect();
    for name in names {
        if let Some(provider) = cfg.providers.get_mut(&name) {
            if let Some(ref key) = provider.api_key {
                if !key.is_empty() {
                    let vault_key = format!("ai:{name}");
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

    // Every provider map, enumerated from one list so the migration cannot
    // drift from the maps the runtime reads: the legacy flat map plus the
    // five typed ones. `generation_providers/helpers.rs` resolves a provider
    // by falling back to the legacy map and reads its key from the vault
    // under `gen:{name}`; a map missing here keeps its plaintext key on disk
    // until the first `Config::save_*` of the section drops it, because
    // `GenerationProviderConfig::api_key` is `skip_serializing`.
    let categories = [
        &mut cfg.generation.providers,
        &mut cfg.generation.image_providers,
        &mut cfg.generation.video_providers,
        &mut cfg.generation.speech_providers,
        &mut cfg.generation.audio_providers,
        &mut cfg.generation.transcription_providers,
    ];

    // Collect all provider names with their api_keys across categories to detect conflicts
    let mut name_to_keys: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for providers in &categories {
        for (name, provider) in providers.iter() {
            if let Some(ref key) = provider.api_key {
                if !key.is_empty() {
                    name_to_keys
                        .entry(name.clone())
                        .or_insert_with(Vec::new)
                        .push(key.clone());
                }
            }
        }
    }

    // Check for conflicts: same name with different non-empty api_keys
    let mut has_conflict = false;
    for (name, keys) in &name_to_keys {
        if keys.len() > 1 {
            let first = &keys[0];
            if keys.iter().any(|k| k != first) {
                tracing::error!(provider = %name, "Conflicting api_keys for same provider name across generation categories; skipping generation migration");
                has_conflict = true;
            }
        }
    }

    if has_conflict {
        return 0;
    }

    // We need to collect names first due to borrow checker
    for providers in categories {
        let names: Vec<String> = providers.keys().cloned().collect();
        for name in names {
            if let Some(provider) = providers.get_mut(&name) {
                let vault_key = format!("gen:{name}");
                let plaintext = provider.api_key.clone();
                if let Some(ref key) = plaintext {
                    if !key.is_empty() {
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
                // `GenerationProviderConfig::api_key` is `skip_serializing`, so
                // the vault is the only durable home for the key. Restore the
                // runtime copy from the vault — right after a migration, and
                // on every later start where config.toml no longer carries
                // the key at all. Without this the migration would leave the
                // provider with no key for the generation factory to read.
                if provider.api_key.is_none() {
                    if let Some(secret) = super::resolve_vault_secret(&vault_key, vault) {
                        provider.api_key = Some(secret);
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
    let vault_key = format!("rerank:{provider_name}");
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
                    let vault_key = format!("search:{name}");
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

// ── Voice streaming ─────────────────────────────────────────────────────────

/// Vault key holding the streaming ASR bearer token.
///
/// The name is `voice_streaming:api_key`, not `voice.streaming:api_key`: every
/// other key in this module is `<prefix>:<field>`, and `voice.streaming` is a
/// TOML path in `config.toml`, not a key name. The config doc comment in
/// `config/types/voice_local.rs` quotes this same name.
const VOICE_STREAMING_API_KEY: &str = "voice_streaming:api_key";

fn migrate_voice_streaming(cfg: &mut Config, vault: &SharedTokenManager) -> usize {
    let plaintext = cfg.voice_local.streaming.api_key.clone();
    let mut count = 0;
    if !plaintext.is_empty() {
        match vault.store_secret(VOICE_STREAMING_API_KEY, &plaintext) {
            Ok(_) => {
                tracing::info!("Migrated voice.streaming.api_key to vault");
                count += 1;
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to store voice streaming secret");
                return 0;
            }
        }
    }
    // `StreamingConfig::api_key` is `skip_serializing`, so the vault is the
    // only durable home for the token — config.toml never carries it again
    // once the migration has run. Rehydrate the in-memory config from the
    // vault so the streaming ASR bearer token reaches the adapter on this
    // start *and* on every later start, where config.toml has no key to
    // migrate. Without this read the migration wrote a vault key nothing
    // looked at and streaming ASR silently ran unauthenticated.
    if let Some(secret) = super::resolve_vault_secret(VOICE_STREAMING_API_KEY, vault) {
        cfg.voice_local.streaming.api_key = secret;
    }
    count
}
