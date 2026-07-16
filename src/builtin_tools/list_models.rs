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
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;

/// Vault key convention for AI provider API keys (mirrors the providers handler
/// `helpers::vault_key`). Kept local so the tool depends only on the vault read
/// path, not on gateway handler internals.
fn vault_key(provider: &str) -> String {
    format!("ai:{provider}")
}

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
    /// Endpoint locality of the provider serving this model: `"local"`
    /// (on-machine / LAN) or `"cloud"` (public API). Lets the model prefer an
    /// on-machine option (privacy / offline / cost) when one exists. Always
    /// present — an absent/unparseable `base_url` classifies as `"cloud"`.
    pub endpoint: String,
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

    /// True when the provider has a usable credential: an `api_key` in
    /// `config.toml` or a secret under `ai:{provider}` in the vault. Mirrors
    /// `handle_catalog`'s `has_api_key` derivation.
    fn provider_configured(&self, name: &str, cfg_api_key: Option<&String>) -> bool {
        if cfg_api_key.is_some() {
            return true;
        }
        match &self.vault {
            Some(vault) => {
                crate::gateway::handlers::resolve_vault_secret(&vault_key(name), vault).is_some()
            }
            None => false,
        }
    }
}

/// Project a `(provider, model)` pair into a fully-enriched [`ModelEntry`].
/// Pure given the static capability/cost tables — no I/O.
fn enrich(
    provider: &str,
    model: &str,
    configured: bool,
    is_default: bool,
    display_name: Option<String>,
    base_url: Option<&str>,
) -> ModelEntry {
    let caps = crate::providers::capabilities_for(model);
    let rate = crate::pricing::rate_card(provider, model);
    ModelEntry {
        provider: provider.to_string(),
        model: model.to_string(),
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
        endpoint: crate::providers::endpoint_kind_for_base_url(base_url)
            .as_str()
            .to_string(),
    }
}

#[async_trait]
impl crate::tools::AlephTool for ListModelsTool {
    const NAME: &'static str = "list_models";
    const DESCRIPTION: &'static str = "List the LLM models you can switch to, each with its context \
        window, vision/tool/reasoning support, and price per million tokens. Use this to discover \
        options before calling `select_model` — e.g. to find a vision-capable model for an image, a \
        long-context model for a big document, a reasoning model for a hard problem, or a cheaper \
        model for simple chat. Returns only credentialed providers by default; pass `all: true` to \
        see every built-in preset.";

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
            let cfg = guard.providers.get(entry.name);
            let configured =
                self.provider_configured(entry.name, cfg.and_then(|c| c.api_key.as_ref()));
            if !args.all && !configured {
                continue;
            }
            if configured {
                configured_providers.insert(entry.name.to_string());
            }
            let display_name = preset.display_name.map(String::from);

            // Candidate ids: the preset default plus any explicitly configured
            // models, de-duplicated case-insensitively (default first).
            let mut seen = std::collections::HashSet::new();
            for model in std::iter::once(preset.default_model.to_string())
                .chain(cfg.map(|c| c.models.clone()).unwrap_or_default())
            {
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
                ));
            }
        }

        // Custom providers: user-added entries with no matching built-in preset
        // (e.g. an OpenAI-compatible relay). Same credential gate.
        for (name, cfg) in &guard.providers {
            if preset_ids.contains(name.as_str()) {
                continue;
            }
            let configured = self.provider_configured(name, cfg.api_key.as_ref());
            if !args.all && !configured {
                continue;
            }
            if configured {
                configured_providers.insert(name.clone());
            }
            for model in &cfg.models {
                if model.is_empty() {
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
                let mut names: Vec<&String> = cfg
                    .presets
                    .iter()
                    .filter(|(_, p)| p.enabled)
                    .map(|(n, _)| n)
                    .collect();
                names.sort();
                names
                    .into_iter()
                    .map(|name| {
                        let p = cfg
                            .presets
                            .get(name)
                            .expect("invariant: name came from presets iterator");
                        MoaPresetSummary {
                            name: name.clone(),
                            advisors: p
                                .advisors
                                .iter()
                                .map(|s| format!("{}:{}", s.provider, s.model))
                                .collect(),
                            aggregator: format!(
                                "{}:{}",
                                p.aggregator.provider, p.aggregator.model
                            ),
                            is_default: cfg.default_preset.as_deref() == Some(name.as_str()),
                        }
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

        let out = tool.call(ListModelsArgs { all: false }).await.unwrap();
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

        let configured_only = tool.call(ListModelsArgs { all: false }).await.unwrap();
        let everything = tool.call(ListModelsArgs { all: true }).await.unwrap();
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
        );
        assert_eq!(cloud.endpoint, "cloud");

        let local = enrich(
            "ollama",
            "llama-3.3-70b",
            true,
            false,
            None,
            Some("http://localhost:11434"),
        );
        assert_eq!(local.endpoint, "local");

        // Absent base_url falls back to the conservative cloud default.
        let unknown = enrich("custom", "mystery-model", false, false, None, None);
        assert_eq!(unknown.endpoint, "cloud");
    }
}
