//! `ListModelsTool` — LLM-facing model discovery (R8 "everything is a tool").
//!
//! The discover half of the `discover → choose` pair completed by
//! [`SelectModelTool`](crate::builtin_tools::select_model). `select_model`
//! switches the active model but is metadata-blind: it takes a bare model id
//! and surfaces nothing for the model to reason over. The chat-window picker
//! (`providers.catalog` RPC) already joins the static capability + cost tables
//! onto each provider for the *UI* — its own comment calls this "the R7
//! 'enable the LLM' surface — capability + cost data the picker **and the
//! model** can reason over, not an auto-router." That surface never reached the
//! LLM. This tool closes that gap: it returns the same enrichment
//! ([`capabilities_for`](crate::providers::capabilities_for) +
//! [`rate_card`](crate::pricing::rate_card)) so the main-loop model can choose a
//! model on context-window / vision / tool / reasoning / price grounds, then
//! call `select_model` with an exact id it just saw.
//!
//! R7-aligned: this is *data for the model*, never an auto-router. Aleph never
//! picks a model by cost/capability on the model's behalf — it just lets the
//! model see what the picker sees.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::Result;
use crate::gateway::security::SharedTokenManager;
use crate::providers::metadata::Modality;
use crate::providers::model_catalog::{ModelRecord, ModelSource};
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

use crate::providers::probe::provider_vault_key;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListModelsArgs {
    /// When true, include every built-in provider preset, not just providers
    /// with a configured credential. Default false → only models you can
    /// actually switch to right now.
    #[serde(default)]
    #[schemars(
        description = "Include unconfigured presets too (default: only credentialed providers)."
    )]
    pub all: bool,

    /// When true, ask each credentialed provider for its live model list
    /// before answering.
    #[serde(default)]
    #[schemars(
        description = "Ask each configured provider for its current model list before answering \
            (one network call per provider, ~10s cap each). Use when the built-in roster looks \
            stale, when a model you expect is missing, or when the provider is an aggregator \
            whose catalog changes independently of Aleph releases."
    )]
    pub refresh: bool,
}

/// One selectable model with its static capability + cost metadata. Absent
/// fields (`None`) mean "not in the reference table" — unknown, not zero.
#[derive(Debug, Clone, Serialize)]
pub struct ModelEntry {
    /// Provider id to pass as `select_model.provider`.
    pub provider: String,
    /// Model id to pass as `select_model.model`.
    pub model: String,
    /// The provider has a usable credential (config `api_key` or vault entry).
    pub configured: bool,
    /// This is the current system default (provider + its default model).
    pub is_default: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Max total context window in tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    /// Max output tokens per response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    /// Accepts image input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_vision: Option<bool>,
    /// Supports native tool / function calling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_tools: Option<bool>,
    /// Has an extended-thinking / reasoning mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_reasoning: Option<bool>,
    /// USD per million input tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_per_mtok: Option<f64>,
    /// USD per million output tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_per_mtok: Option<f64>,
    /// `"direct"` when the rate is the serving provider's own published price,
    /// `"vendor_inferred"` when it was taken from the model's vendor because
    /// the provider is an aggregator / cloud reseller — treat the latter as a
    /// floor. Absent when the model has no rate at all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_basis: Option<String>,
    /// Endpoint locality of the provider serving this model: `"local"`
    /// (on-machine / LAN) or `"cloud"` (public API). Lets the model prefer an
    /// on-machine option (privacy / offline / cost) when one exists. Always
    /// present — an absent/unparseable `base_url` classifies as `"cloud"`.
    pub endpoint: String,
    /// `"active"` / `"preview"` / `"deprecated"`. A deprecated id will be
    /// refused by `select_model`; prefer `successor` instead.
    pub status: String,
    /// The model the vendor points at, when this one is deprecated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub successor: Option<String>,
    /// Caveat attached to a non-active status (retirement date, preview note).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_note: Option<String>,
    /// How Aleph knows about this id: `"preset_default"` (the provider's
    /// shipped default), `"preset_fallback"` (a curated alternative from the
    /// same vendor), `"preset_aux"` (the cheap tier), `"configured"` (listed
    /// by the operator) or `"discovered"` (returned by the provider's live
    /// `/models` endpoint just now).
    pub source: String,
}

#[derive(Debug, Serialize)]
pub struct ListModelsOutput {
    pub ok: bool,
    /// Selectable models, configured providers first then alphabetical.
    pub models: Vec<ModelEntry>,
    /// Human-readable summary (counts + current default + the `all` hint).
    pub message: String,
    /// MoA advisory presets selectable via `select_model` with model
    /// "moa:<name>" (enabled presets only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub moa_presets: Vec<MoaPresetSummary>,
}

/// A `[moa]` preset surfaced on the same discovery pass as models — the model
/// can choose "switch to MoA preset X" via `select_model { model: "moa:X" }`
/// exactly as it would choose a plain model id.
#[derive(Debug, Clone, Serialize)]
pub struct MoaPresetSummary {
    pub name: String,
    /// Advisor slots as "provider:model" labels.
    pub advisors: Vec<String>,
    /// Aggregator (acting model) as "provider:model".
    pub aggregator: String,
    pub is_default: bool,
}

/// LLM-facing model catalog. Holds optional handles injected at registry
/// construction (mirrors [`SelfConfigTool`](crate::builtin_tools::self_config)):
/// config for the provider/credential state, vault for keys stored outside
/// `config.toml`.
#[derive(Clone, Default)]
pub struct ListModelsTool {
    config: Option<Arc<RwLock<Config>>>,
    vault: Option<Arc<SharedTokenManager>>,
}

impl ListModelsTool {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(mut self, config: Arc<RwLock<Config>>) -> Self {
        self.config = Some(config);
        self
    }

    pub fn with_vault(mut self, vault: Arc<SharedTokenManager>) -> Self {
        self.vault = Some(vault);
        self
    }

    /// The provider's usable credential: an `api_key` in `config.toml` or a
    /// secret under `ai:{provider}` in the vault. Mirrors `handle_catalog`'s
    /// `has_api_key` derivation.
    fn resolve_api_key(&self, name: &str, cfg_api_key: Option<&String>) -> Option<String> {
        if let Some(key) = cfg_api_key {
            return Some(key.clone());
        }
        let vault = self.vault.as_ref()?;
        crate::gateway::handlers::resolve_vault_secret(&provider_vault_key(name), vault)
    }

    /// True when the provider has a usable credential.
    fn provider_configured(&self, name: &str, cfg_api_key: Option<&String>) -> bool {
        self.resolve_api_key(name, cfg_api_key).is_some()
    }

    /// Ask every credentialed provider for its live model list, concurrently.
    ///
    /// Returns `provider → [model id]` for the providers that answered.
    /// Failures are dropped, not surfaced as errors: discovery is an
    /// enrichment pass over a catalog that already works, so one unreachable
    /// vendor must not fail the tool call. The per-provider reasons are logged
    /// and the counts come back in the tool's `message`.
    ///
    /// The config lock is taken only to *build the target list* and released
    /// before any network call — a `list_models { refresh: true }` must not
    /// hold a read lock across ~10s of I/O while other turns want to write it.
    async fn discover_live_models(
        &self,
        config: &Arc<RwLock<Config>>,
    ) -> (std::collections::HashMap<String, Vec<String>>, usize) {
        struct Target {
            provider: String,
            base_url: String,
            protocol: String,
            api_key: String,
        }

        let targets: Vec<Target> = {
            let guard = config.read().await;
            guard
                .providers
                .iter()
                .filter(|(_, cfg)| cfg.enabled)
                // Six presets publish no `/models` endpoint. `refresh_models`
                // refuses them at the leaf, so this is not what stops the
                // network call — it stops them being *counted*: `attempted`
                // rides back to the model in the tool's own message, and a
                // number that includes providers nobody asked anything is a
                // small lie told to the one reader who cannot check it.
                // Same predicate as the leaf, so the two cannot disagree.
                .filter(|(name, _)| crate::providers::probe::supports_model_listing(name))
                .filter_map(|(name, cfg)| {
                    let preset = crate::providers::presets::get_preset(name);
                    let base_url = cfg
                        .base_url
                        .clone()
                        .or_else(|| preset.map(|p| p.base_url.to_string()))?;
                    let protocol = cfg
                        .protocol
                        .clone()
                        .or_else(|| preset.map(|p| p.protocol.to_string()))
                        .unwrap_or_else(|| "openai".to_string());
                    let api_key = self.resolve_api_key(name, cfg.api_key.as_ref())?;
                    Some(Target {
                        provider: name.clone(),
                        base_url,
                        protocol,
                        api_key,
                    })
                })
                .collect()
        };

        let attempted = targets.len();
        let mut out = std::collections::HashMap::new();
        let mut tasks = tokio::task::JoinSet::new();
        for t in targets {
            // A listing fetched moments ago is reused rather than refetched.
            // `list_models` is open to chat-tier callers, so an unconditional
            // fetch would make "call this repeatedly" a way to hammer every
            // configured vendor with the operator's keys. The operator RPC
            // (`providers.modelsRefresh`) still forces a real round trip —
            // that path is an explicit, authorised "go look now".
            if let Some(cached) =
                crate::providers::model_catalog::cached_models(&t.provider, &t.base_url)
            {
                if cached.is_fresh(crate::providers::model_catalog::discovery::CACHE_TTL) {
                    out.insert(
                        t.provider,
                        cached.models.into_iter().map(|m| m.id).collect::<Vec<_>>(),
                    );
                    continue;
                }
            }
            tasks.spawn(async move {
                let base_url = t.base_url.clone();
                let outcome = crate::providers::model_catalog::refresh_models(
                    &t.provider,
                    &t.base_url,
                    &t.protocol,
                    &t.api_key,
                )
                .await;
                (t.provider, base_url, outcome)
            });
        }

        while let Some(joined) = tasks.join_next().await {
            let Ok((provider, base_url, outcome)) = joined else {
                continue;
            };
            match outcome {
                Ok(listing) => {
                    out.insert(
                        provider,
                        listing.models.into_iter().map(|m| m.id).collect::<Vec<_>>(),
                    );
                }
                Err(e) => {
                    // Stale snapshot beats no snapshot (pi's recovery shape):
                    // a vendor that is unreachable *right now* still had an
                    // inventory the last time we looked, and every id in it
                    // is enriched by the curated tables anyway.
                    if let Some(stale) =
                        crate::providers::model_catalog::cached_models(&provider, &base_url)
                    {
                        tracing::debug!(
                            provider = %provider, error = %e,
                            "model discovery failed; serving stale cache"
                        );
                        out.insert(
                            provider,
                            stale.models.into_iter().map(|m| m.id).collect::<Vec<_>>(),
                        );
                    } else {
                        tracing::debug!(provider = %provider, error = %e, "model discovery skipped");
                    }
                }
            }
        }
        (out, attempted)
    }
}

/// Project a `(provider, model)` pair into a fully-enriched [`ModelEntry`].
///
/// The four-table join itself lives in
/// [`ModelRecord::resolve`](crate::providers::model_catalog::ModelRecord) —
/// this only flattens it into the tool's wire shape, so a fifth dimension
/// added to the catalog reaches the LLM without another hand-written join.
fn enrich(
    provider: &str,
    model: &str,
    configured: bool,
    is_default: bool,
    display_name: Option<String>,
    base_url: Option<&str>,
    source: ModelSource,
) -> ModelEntry {
    let record = ModelRecord::resolve(provider, model, base_url, source);
    let caps = record.capabilities;
    let rate = record.cost;
    ModelEntry {
        provider: record.provider,
        model: record.model,
        configured,
        is_default,
        display_name,
        context_window: caps.map(|c| c.context_window),
        max_output_tokens: caps.map(|c| c.max_output_tokens),
        supports_vision: caps.map(|c| c.supports_vision),
        supports_tools: caps.map(|c| c.supports_tools),
        supports_reasoning: caps.map(|c| c.supports_reasoning),
        input_per_mtok: rate.and_then(|r| r.input_per_mtok),
        output_per_mtok: rate.and_then(|r| r.output_per_mtok),
        price_basis: rate.map(|r| match r.basis {
            crate::pricing::RateBasis::Direct => "direct".to_string(),
            crate::pricing::RateBasis::VendorInferred => "vendor_inferred".to_string(),
        }),
        endpoint: record.endpoint.as_str().to_string(),
        status: record.lifecycle.status.as_str().to_string(),
        successor: record.lifecycle.successor.map(|s| s.into_owned()),
        status_note: record.lifecycle.note.map(|s| s.into_owned()),
        source: record.source.as_str().to_string(),
    }
}

#[async_trait]
impl crate::tools::AlephTool for ListModelsTool {
    const NAME: &'static str = "list_models";
    const DESCRIPTION: &'static str =
        "List the LLM models you can switch to, each with its context \
        window, vision/tool/reasoning support, price per million tokens, and lifecycle status. Use \
        this to discover options before calling `select_model` — e.g. to find a vision-capable \
        model for an image, a long-context model for a big document, a reasoning model for a hard \
        problem, or a cheaper model for simple chat. Each provider contributes its default model, \
        its curated alternatives and its cheap tier. Returns only credentialed providers by \
        default; pass `all: true` to see every built-in preset, or `refresh: true` to ask each \
        provider for its current live model list first.";

    type Args = ListModelsArgs;
    type Output = ListModelsOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let Some(config) = self.config.as_ref() else {
            return Ok(ListModelsOutput {
                ok: false,
                models: Vec::new(),
                message: "Model catalog unavailable: no config handle.".to_string(),
                moa_presets: Vec::new(),
            });
        };

        // Live discovery runs before the main read lock is taken (see
        // `discover_live_models`) so no network I/O happens under it.
        let (discovered, discovery_attempted) = if args.refresh {
            self.discover_live_models(config).await
        } else {
            (std::collections::HashMap::new(), 0)
        };

        let guard = config.read().await;
        let default_provider = guard.general.default_provider.clone();

        let mut models: Vec<ModelEntry> = Vec::new();
        let mut configured_providers = std::collections::HashSet::new();

        // Built-in chat presets, joined with per-provider credential state.
        let entries = crate::providers::catalog::presets_for_modality(Modality::Chat);
        let preset_ids: std::collections::HashSet<&str> = entries.iter().map(|e| e.name).collect();

        for entry in &entries {
            let Some(preset) = crate::providers::presets::get_preset(entry.name) else {
                continue;
            };
            // A provider configured under an *alias* (`kimi` for `moonshot`)
            // attaches to the canonical row. The matched config key matters
            // beyond the config itself: vault secrets (`ai:<name>`) and
            // discovery results are both keyed by the *config* name, not the
            // canonical one.
            let (cfg_name, cfg) = match guard.providers.get_key_value(entry.name).or_else(|| {
                preset
                    .aliases
                    .iter()
                    .find_map(|a| guard.providers.get_key_value(*a))
            }) {
                Some((n, c)) => (n.as_str(), Some(c)),
                None => (entry.name, None),
            };
            let configured =
                self.provider_configured(cfg_name, cfg.and_then(|c| c.api_key.as_ref()));
            if !args.all && !configured {
                continue;
            }
            if configured {
                configured_providers.insert(entry.name.to_string());
            }
            let display_name = preset.display_name.map(String::from);

            // Candidate ids, in provenance order. Until this round the roster
            // stopped at "default + whatever the operator typed into config",
            // so a model asking for a bigger sibling saw *one* id per vendor —
            // even though the preset already curates a fallback chain and a
            // cheap aux tier, and the Panel picker (`providers.catalog`) has
            // been showing both all along. Same data, same source of truth;
            // it just never reached the LLM.
            //
            // The full roster is only worth its tokens for providers the caller
            // can actually reach. Under `all: true` the question is "which
            // providers exist" — listing every uncredentialed preset's whole
            // chain would triple that answer's size to enumerate models nobody
            // can select, so uncredentialed presets contribute their default
            // only.
            let curated: Vec<(String, ModelSource)> = if configured {
                preset
                    .fallback_models
                    .iter()
                    .map(|m| ((*m).to_string(), ModelSource::PresetFallback))
                    .chain(
                        preset
                            .default_aux_model
                            .map(|m| (m.to_string(), ModelSource::PresetAux)),
                    )
                    .collect()
            } else {
                Vec::new()
            };
            let candidates: Vec<(String, ModelSource)> =
                std::iter::once((preset.default_model.to_string(), ModelSource::PresetDefault))
                    .chain(curated)
                    .chain(
                        cfg.map(|c| c.models.clone())
                            .unwrap_or_default()
                            .into_iter()
                            .map(|m| (m, ModelSource::Configured)),
                    )
                    .chain(
                        discovered
                            .get(cfg_name)
                            .into_iter()
                            .flatten()
                            .map(|m| (m.clone(), ModelSource::Discovered)),
                    )
                    .collect();

            // First mention wins, so an id keeps its most authoritative
            // provenance (default ▸ fallback ▸ aux ▸ configured ▸ discovered).
            let mut seen = std::collections::HashSet::new();
            for (model, source) in candidates {
                let key = model.to_ascii_lowercase();
                if model.is_empty() || !seen.insert(key) {
                    continue;
                }
                let is_default = default_provider.as_deref() == Some(entry.name)
                    && model.eq_ignore_ascii_case(preset.default_model);
                models.push(enrich(
                    entry.name,
                    &model,
                    configured,
                    is_default,
                    display_name.clone(),
                    Some(preset.base_url),
                    source,
                ));
            }
        }

        // Custom providers: user-added entries with no matching built-in preset
        // (e.g. an OpenAI-compatible relay). Same credential gate. A config
        // keyed by an *alias* (`kimi`) attached to its canonical row above,
        // so "no preset answers to this name" is the filter — not just "name
        // is not a canonical id".
        for (name, cfg) in &guard.providers {
            if preset_ids.contains(name.as_str())
                || crate::providers::presets::get_preset(name).is_some()
            {
                continue;
            }
            let configured = self.provider_configured(name, cfg.api_key.as_ref());
            if !args.all && !configured {
                continue;
            }
            if configured {
                configured_providers.insert(name.clone());
            }
            let candidates: Vec<(&String, ModelSource)> = cfg
                .models
                .iter()
                .map(|m| (m, ModelSource::Configured))
                .chain(
                    discovered
                        .get(name.as_str())
                        .into_iter()
                        .flatten()
                        .map(|m| (m, ModelSource::Discovered)),
                )
                .collect();
            let mut seen = std::collections::HashSet::new();
            for (model, source) in candidates {
                if model.is_empty() || !seen.insert(model.to_ascii_lowercase()) {
                    continue;
                }
                let is_default = default_provider.as_deref() == Some(name.as_str());
                models.push(enrich(
                    name,
                    model,
                    configured,
                    is_default,
                    None,
                    cfg.base_url.as_deref(),
                    source,
                ));
            }
        }

        // Configured first, then by provider name, then model — deterministic.
        models.sort_by(|a, b| {
            b.configured
                .cmp(&a.configured)
                .then_with(|| a.provider.cmp(&b.provider))
                .then_with(|| a.model.cmp(&b.model))
        });

        // MoA presets ride the same discovery surface (round-2 E3): the model
        // can offer "switch to MoA preset X" with select_model "moa:X".
        let moa_presets: Vec<MoaPresetSummary> = crate::providers::moa::get_moa_config()
            .map(|cfg| {
                // Iterate entries directly so the invariant that "name
                // came from the iterator" is held in the loop itself
                // rather than via an `expect` on a second `HashMap::get`
                // call. A rehash between the two lookups (e.g. if the
                // map ever became a `DashMap` or similar) would defeat
                // the type-system guarantee and the `expect` would be a
                // silent panicking spot.
                let mut entries: Vec<(&String, &crate::config::types::moa::MoaPreset)> =
                    cfg.presets.iter().filter(|(_, p)| p.enabled).collect();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                entries
                    .into_iter()
                    .map(|(name, p)| MoaPresetSummary {
                        name: name.clone(),
                        advisors: p
                            .advisors
                            .iter()
                            .map(|s| format!("{}:{}", s.provider, s.model))
                            .collect(),
                        aggregator: format!("{}:{}", p.aggregator.provider, p.aggregator.model),
                        is_default: cfg.default_preset.as_deref() == Some(name.as_str()),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let default_label = match &default_provider {
            Some(p) => format!("current default provider: '{p}'"),
            None => "no default provider set".to_string(),
        };
        let mut message = format!(
            "{} model(s) across {} configured provider(s); {}. {}",
            models.len(),
            configured_providers.len(),
            default_label,
            if args.all {
                "Showing all presets."
            } else {
                "Pass all=true to also see unconfigured presets."
            }
        );
        if !moa_presets.is_empty() {
            message.push_str(&format!(
                " {} MoA preset(s) available — select with model \"moa:<name>\".",
                moa_presets.len()
            ));
        }
        if args.refresh {
            // Say how many providers actually answered. Silence here would let
            // "I refreshed and this is everything" stand for "every provider I
            // asked timed out and you are reading the built-in roster".
            message.push_str(&format!(
                " Live refresh: {}/{} provider(s) answered.",
                discovered.len(),
                discovery_attempted
            ));
        }
        let deprecated = models.iter().filter(|m| m.status == "deprecated").count();
        if deprecated > 0 {
            message.push_str(&format!(
                " {deprecated} listed id(s) are deprecated — see `successor` before selecting."
            ));
        }

        Ok(ListModelsOutput {
            ok: true,
            models,
            message,
            moa_presets,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::AlephTool;
    use serde_json::json;

    fn config_with(
        provider: &str,
        api_key: Option<&str>,
        models: &[&str],
        default: bool,
    ) -> Arc<RwLock<Config>> {
        let mut cfg = Config::default();
        let pc: crate::config::types::provider::ProviderConfig = serde_json::from_value(json!({
            "api_key": api_key,
            "enabled": true,
            "models": models,
        }))
        .unwrap();
        cfg.providers.insert(provider.to_string(), pc);
        if default {
            cfg.general.default_provider = Some(provider.to_string());
        }
        Arc::new(RwLock::new(cfg))
    }

    #[tokio::test]
    async fn lists_only_configured_by_default() {
        // The Anthropic preset is keyed "claude" (protocol "anthropic").
        // Credential it; every other preset (no api_key) is excluded.
        // (ProviderConfig.models rejects an empty list; the default model is
        // de-duplicated against it case-insensitively.)
        let cfg = config_with(
            "claude",
            Some("sk-test"),
            &["claude-sonnet-4-5-20250514"],
            true,
        );
        let tool = ListModelsTool::new().with_config(cfg);

        let out = tool
            .call(ListModelsArgs {
                all: false,
                refresh: false,
            })
            .await
            .unwrap();
        assert!(out.ok);
        assert!(!out.models.is_empty(), "configured provider must appear");
        assert!(
            out.models.iter().all(|m| m.provider == "claude"),
            "only the credentialed provider is listed by default"
        );
        // The anthropic default model carries capability + cost metadata.
        let def = out
            .models
            .iter()
            .find(|m| m.is_default)
            .expect("a default model");
        assert!(def.configured);
        assert!(def.context_window.is_some(), "capability metadata surfaced");
        assert!(def.input_per_mtok.is_some(), "cost metadata surfaced");
    }

    #[tokio::test]
    async fn all_flag_includes_unconfigured_presets() {
        let cfg = config_with(
            "claude",
            Some("sk-test"),
            &["claude-sonnet-4-5-20250514"],
            true,
        );
        let tool = ListModelsTool::new().with_config(cfg);

        let configured_only = tool
            .call(ListModelsArgs {
                all: false,
                refresh: false,
            })
            .await
            .unwrap();
        let everything = tool
            .call(ListModelsArgs {
                all: true,
                refresh: false,
            })
            .await
            .unwrap();
        assert!(
            everything.models.len() > configured_only.models.len(),
            "all=true surfaces presets without a credential"
        );
        // Unconfigured presets are flagged as such.
        assert!(everything.models.iter().any(|m| !m.configured));
    }

    #[tokio::test]
    async fn no_config_handle_is_graceful() {
        let tool = ListModelsTool::new();
        let out = tool.call(ListModelsArgs::default()).await.unwrap();
        assert!(!out.ok);
        assert!(out.models.is_empty());
    }

    #[test]
    fn enrich_surfaces_endpoint_locality() {
        // A public vendor base_url classifies as "cloud"; a loopback Ollama
        // endpoint classifies as "local" — letting the model prefer
        // on-machine inference when one is configured.
        let cloud = enrich(
            "claude",
            "claude-sonnet-4-6",
            true,
            true,
            None,
            Some("https://api.anthropic.com"),
            ModelSource::PresetDefault,
        );
        assert_eq!(cloud.endpoint, "cloud");

        let local = enrich(
            "ollama",
            "llama-3.3-70b",
            true,
            false,
            None,
            Some("http://localhost:11434"),
            ModelSource::Configured,
        );
        assert_eq!(local.endpoint, "local");

        // Absent base_url falls back to the conservative cloud default.
        let unknown = enrich(
            "custom",
            "mystery-model",
            false,
            false,
            None,
            None,
            ModelSource::Configured,
        );
        assert_eq!(unknown.endpoint, "cloud");
    }

    /// The curated roster is the reason the LLM can now see more than one model
    /// per vendor. Before this round `list_models` returned only the default
    /// plus whatever the operator had typed into config, while the Panel picker
    /// showed the preset's whole fallback chain from the same data.
    #[tokio::test]
    async fn preset_fallbacks_and_aux_reach_the_model() {
        let cfg = config_with("claude", Some("sk-test"), &["claude-sonnet-5"], true);
        let tool = ListModelsTool::new().with_config(cfg);
        let out = tool
            .call(ListModelsArgs {
                all: false,
                refresh: false,
            })
            .await
            .unwrap();

        let ids: Vec<&str> = out.models.iter().map(|m| m.model.as_str()).collect();
        let preset = crate::providers::presets::get_preset("claude").unwrap();
        for expected in preset.fallback_models {
            assert!(
                ids.contains(expected),
                "fallback {expected} missing from {ids:?}"
            );
        }
        let aux = preset.default_aux_model.unwrap();
        assert!(ids.contains(&aux), "aux {aux} missing from {ids:?}");

        // Provenance is reported, so the model can tell a curated alternative
        // from something the operator pinned.
        let default_row = out.models.iter().find(|m| m.is_default).unwrap();
        assert_eq!(default_row.source, "preset_default");
        assert!(out.models.iter().any(|m| m.source == "preset_fallback"));
    }

    /// Lifecycle reaches the model: a retired id it might otherwise pick is
    /// labelled, with the successor to use instead.
    #[tokio::test]
    async fn retired_configured_model_is_labelled_with_its_successor() {
        let cfg = config_with("deepseek", Some("sk-test"), &["deepseek-chat"], true);
        let tool = ListModelsTool::new().with_config(cfg);
        let out = tool
            .call(ListModelsArgs {
                all: false,
                refresh: false,
            })
            .await
            .unwrap();

        let retired = out
            .models
            .iter()
            .find(|m| m.model == "deepseek-chat")
            .expect("configured model is listed");
        assert_eq!(retired.status, "deprecated");
        assert_eq!(retired.successor.as_deref(), Some("deepseek-v4-flash"));
        assert!(out.message.contains("deprecated"), "{}", out.message);

        // The preset's own default is current, and says so.
        let default_row = out.models.iter().find(|m| m.is_default).unwrap();
        assert_eq!(default_row.status, "active");
    }

    /// Aggregator-served models used to carry no price at all, which also made
    /// `cost_aware` routing rank them last. They now price through the vendor,
    /// flagged so the reseller margin is not passed off as a quote.
    #[test]
    fn aggregator_rows_carry_an_inferred_price_basis() {
        let row = enrich(
            "openrouter",
            "anthropic/claude-sonnet-5",
            true,
            false,
            None,
            Some("https://openrouter.ai/api"),
            ModelSource::Configured,
        );
        assert_eq!(row.price_basis.as_deref(), Some("vendor_inferred"));
        assert!(row.input_per_mtok.is_some());

        let direct = enrich(
            "claude",
            "claude-sonnet-5",
            true,
            true,
            None,
            Some("https://api.anthropic.com"),
            ModelSource::PresetDefault,
        );
        assert_eq!(direct.price_basis.as_deref(), Some("direct"));
    }
}
