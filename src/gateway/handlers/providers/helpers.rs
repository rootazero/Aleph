//! Provider handler helper functions.

use crate::config::presets_override::PresetsOverride;
use crate::config::{Config, ProviderConfig};
use crate::gateway::security::SharedTokenManager;
use crate::providers::presets::get_merged_preset;

use aleph_protocol::providers::{
    CostTierJson, ModelCompatJson, ModelCostJson, ModelDefJson, ModelInputJson,
    ThinkingLevelMapJson,
};

use super::types::{ProviderConfigJson, ProviderInfo};

/// Build the wire row for one configured provider.
///
/// `providers.list` and `providers.get` return the same shape, and before this
/// helper each built it with its own field-by-field literal — including its own
/// copy of the `model = models[0]` projection. Two authors for one derived
/// field is how a projection drifts from the thing it projects, so the ladder
/// goes through [`ProviderInfo::with_models`] and the scalar is derived there.
pub(super) fn provider_info_row(
    name: &str,
    cfg: &ProviderConfig,
    default_provider: Option<&String>,
    has_api_key: bool,
) -> ProviderInfo {
    ProviderInfo {
        name: name.to_string(),
        enabled: cfg.enabled,
        models: Vec::new(),
        model: String::new(),
        provider_type: Some(cfg.protocol()),
        has_api_key,
        // Never serialised back to a client: the key lives in the vault and a
        // response is the one place it must not appear.
        api_key: None,
        base_url: cfg.base_url.clone(),
        color: cfg.color.clone(),
        timeout_seconds: cfg.timeout_seconds,
        max_tokens: cfg.max_tokens,
        context_window: cfg.context_window,
        temperature: cfg.temperature,
        is_default: default_provider.is_some_and(|d| d == name),
        verified: cfg.verified,
        // Per-model overrides — readback so the Panel can pre-fill accurately.
        model_defs: cfg.model_defs.iter().map(model_def_to_wire).collect(),
        thinking_level: cfg.thinking_level.clone(),
        effort: cfg.effort.clone(),
        cache_retention: cfg.cache_retention.as_ref().map(cache_retention_to_wire),
        service_tier: cfg.service_tier.clone(),
        stop_sequences: cfg.stop_sequences.clone(),
        media_resolution: cfg.media_resolution.clone(),
        system_prompt_mode: cfg.system_prompt_mode.clone(),
        model_behavior: cfg.model_behavior.clone(),
        metadata_user_id: cfg.metadata_user_id.clone(),
        response_format: cfg.response_format.as_ref().map(response_format_to_wire),
        stream_idle_timeout_secs: cfg.stream_idle_timeout_secs,
        frequency_penalty: cfg.frequency_penalty,
        presence_penalty: cfg.presence_penalty,
        repeat_penalty: cfg.repeat_penalty,
        seed: cfg.seed,
        logprobs: cfg.logprobs,
        top_logprobs: cfg.top_logprobs,
        parallel_tool_calls: cfg.parallel_tool_calls,
    }
    .with_models(cfg.models.clone())
}

/// Vault key for AI provider API keys (`ai:<name>`) — single definition
/// shared with the diagnostics connectivity check.
pub(super) fn vault_key(provider_name: &str) -> String {
    crate::providers::probe::provider_vault_key(provider_name)
}

/// Resolve the actual API key from vault (for internal use like test/create-provider, never serialized to responses)
pub(super) fn resolve_api_key(name: &str, vault: &SharedTokenManager) -> Option<String> {
    crate::gateway::handlers::resolve_vault_secret(&vault_key(name), vault)
}

pub(super) use crate::gateway::handlers::normalize_optional_string;

pub(super) fn save_config(cfg: &Config, sections: &[&str]) -> Result<(), String> {
    // api_key is #[serde(skip)] — never persisted to disk
    cfg.save_incremental(sections).map_err(|e| e.to_string())
}

/// Build the `ProviderConfig` to persist for a create/update RPC.
///
/// `prior` is the currently-stored entry on `providers.update`, and `None` on
/// `providers.create`. It exists because `ProviderConfigJson` is a lossy view
/// of `ProviderConfig`: roughly a dozen fields have no DTO representation at
/// all. Rebuilding the struct from the DTO alone therefore *erased* every one
/// of them whenever the user changed anything else about the provider — a
/// model id, a timeout, a colour — both in memory and, via `save_config`, on
/// disk.
///
/// `cache_retention` is the one that costs money: an operator sets
/// `cache_retention = "long"` for the 1-hour prompt-cache TTL, later edits an
/// unrelated field from the Panel, and silently drops back to the 5-minute
/// default. Long-idle agents (cron fires, subscription loops, overnight
/// sessions) that used to land inside the window then re-pay full-price
/// `cache_creation` on every wake. Nothing logs it and the watchdog stays
/// quiet, because 5-minute caching still reports activity — the only symptom
/// is the bill. `stop_sequences`, `thinking_level`, `service_tier`, `effort`,
/// `seed` and the rest were eaten the same way.
///
/// Starting from `prior` and overwriting only the DTO-expressible fields also
/// means a field added to `ProviderConfig` later is carried forward by
/// default, rather than silently reset until someone notices.
pub(super) fn build_provider_config_for_persistence(
    provider_name: &str,
    params: ProviderConfigJson,
    presets_override: &PresetsOverride,
    prior: Option<&ProviderConfig>,
) -> Result<ProviderConfig, String> {
    // api_key is runtime-only (#[serde(skip)]) — kept in memory, never persisted
    let api_key = normalize_optional_string(params.api_key);

    // Restore preset defaults for empty base_url / model (merged with user overrides)
    let preset = get_merged_preset(provider_name, presets_override);

    let base_url = match normalize_optional_string(params.base_url) {
        Some(url) => Some(url),
        None => preset.as_ref().map(|p| p.base_url.clone()),
    };

    let models = {
        // Filter empty models from the input
        let non_empty: Vec<String> = params
            .models
            .iter()
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .collect();
        if non_empty.is_empty() {
            // Fall back to preset default model
            preset
                .as_ref()
                .filter(|p| !p.default_model.is_empty())
                .map(|p| vec![p.default_model.clone()])
                .unwrap_or_default()
        } else {
            non_empty
        }
    };

    // Ensure protocol is always persisted (prevents "openai" default on reload)
    let protocol = params
        .protocol
        .or_else(|| preset.as_ref().map(|p| p.protocol.clone()));

    // Baseline: the stored entry when updating (so DTO-inexpressible fields
    // survive), otherwise a fresh all-unset config.
    let mut cfg = match prior {
        Some(existing) => existing.clone(),
        None => ProviderConfig {
            protocol: None,
            api_key: None,
            models: Vec::new(),
            base_url: None,
            color: String::new(),
            timeout_seconds: 300,
            enabled: true,
            max_tokens: None,
            context_window: None,
            temperature: None,
            top_p: None,
            top_k: None,
            frequency_penalty: None,
            presence_penalty: None,
            stop_sequences: None,
            thinking_level: None,
            media_resolution: None,
            repeat_penalty: None,
            system_prompt_mode: None,
            model_behavior: None,
            verified: false,
            service_tier: None,
            stream_idle_timeout_secs: None,
            cache_retention: None,
            response_format: None,
            parallel_tool_calls: None,
            seed: None,
            logprobs: None,
            top_logprobs: None,
            metadata_user_id: None,
            effort: None,
            server_context_editing: Default::default(),
            model_defs: vec![],
        },
    };

    // Everything the DTO can express is authoritative and overwrites the
    // baseline. Everything it cannot is left as the baseline carried it.
    cfg.protocol = protocol;
    cfg.api_key = api_key;
    cfg.models = models;
    cfg.base_url = base_url;
    cfg.color = params.color.unwrap_or_else(|| "#808080".to_string());
    cfg.timeout_seconds = params.timeout_seconds.unwrap_or(300);
    cfg.enabled = params.enabled;
    cfg.max_tokens = params.max_tokens;
    cfg.context_window = params.context_window;
    cfg.temperature = params.temperature;
    // `top_p` / `top_k` are the only pre-existing scalars absent from
    // `ProviderInfo` readback, so the Panel can never echo them back and would
    // otherwise wipe them on every save. Guard them like the new fields below:
    // an absent DTO value preserves the stored value.
    if let Some(v) = params.top_p {
        cfg.top_p = Some(v);
    }
    if let Some(v) = params.top_k {
        cfg.top_k = Some(v);
    }
    // New provider-level knobs (added for feat/per-model-params-core). Each is
    // guarded by `is_some()` so an absent DTO field preserves the prior value
    // — the same survival guarantee the rest of this helper upholds. The
    // resulting wire field can't carry "explicitly clear"; that is a known
    // gap (out of scope per design §5.1) and goes through the config file
    // when an operator needs it.
    if let Some(v) = params.thinking_level {
        cfg.thinking_level = Some(v);
    }
    if let Some(v) = params.effort {
        cfg.effort = Some(v);
    }
    if let Some(v) = params.cache_retention {
        cfg.cache_retention = Some(cache_retention_from_wire(&v)?);
    }
    if let Some(v) = params.service_tier {
        cfg.service_tier = Some(v);
    }
    if let Some(v) = params.stop_sequences {
        cfg.stop_sequences = Some(v);
    }
    if let Some(v) = params.media_resolution {
        cfg.media_resolution = Some(v);
    }
    if let Some(v) = params.system_prompt_mode {
        cfg.system_prompt_mode = Some(v);
    }
    if let Some(v) = params.model_behavior {
        cfg.model_behavior = Some(v);
    }
    if let Some(v) = params.metadata_user_id {
        cfg.metadata_user_id = Some(v);
    }
    if let Some(v) = params.response_format {
        cfg.response_format = Some(response_format_from_wire(&v)?);
    }
    if let Some(v) = params.stream_idle_timeout_secs {
        cfg.stream_idle_timeout_secs = Some(v);
    }
    if let Some(v) = params.frequency_penalty {
        cfg.frequency_penalty = Some(v);
    }
    if let Some(v) = params.presence_penalty {
        cfg.presence_penalty = Some(v);
    }
    if let Some(v) = params.repeat_penalty {
        cfg.repeat_penalty = Some(v);
    }
    if let Some(v) = params.seed {
        cfg.seed = Some(v);
    }
    if let Some(v) = params.logprobs {
        cfg.logprobs = Some(v);
    }
    if let Some(v) = params.top_logprobs {
        cfg.top_logprobs = Some(v);
    }
    if let Some(v) = params.parallel_tool_calls {
        cfg.parallel_tool_calls = Some(v);
    }
    // Per-model overrides: `None` keeps prior (default branch above already
    // left it alone); `Some(vec)` replaces, with `Some(vec![])` clearing.
    if let Some(defs) = params.model_defs {
        cfg.model_defs = dedup_model_defs(defs.into_iter().map(model_def_from_wire))?;
    }
    // Any edit invalidates a previous connectivity check.
    cfg.verified = false;

    Ok(cfg)
}

// ---- Wire <-> core conversions for the per-model fields added in this commit.
//
// These exist on the helper side (not in `aleph_protocol`) because the wire
// types live in the protocol crate which cannot depend on alephcore. Mapping
// keeps the wire shape stable: adding a field to the core type does not
// require a matching wire change, and adding a wire field does not require
// editing every consumer.

// ---------------------------------------------------------------------------
// cache_retention
// ---------------------------------------------------------------------------

fn cache_retention_to_wire(v: &CacheRetentionCore) -> String {
    match v {
        CacheRetentionCore::Off => "off".to_string(),
        CacheRetentionCore::Short => "short".to_string(),
        CacheRetentionCore::Long => "long".to_string(),
    }
}

fn cache_retention_from_wire(s: &str) -> Result<CacheRetentionCore, String> {
    match s {
        "off" => Ok(CacheRetentionCore::Off),
        "short" => Ok(CacheRetentionCore::Short),
        "long" => Ok(CacheRetentionCore::Long),
        other => Err(format!(
            "invalid cache_retention {other:?}: expected one of \"off\", \"short\", \"long\""
        )),
    }
}

// ---------------------------------------------------------------------------
// response_format — the wire keeps the editor simple as a JSON string; the
// core uses an enum. Round-trip is exact for the text/json_object forms.
// ---------------------------------------------------------------------------

fn response_format_to_wire(_v: &ResponseFormatCore) -> String {
    // Serialised JSON string from the core enum. The wire carries a plain
    // string today so the Panel editor does not have to render a tagged enum.
    serde_json::to_string(_v).unwrap_or_default()
}

fn response_format_from_wire(s: &str) -> Result<ResponseFormatCore, String> {
    serde_json::from_str::<ResponseFormatCore>(s)
        .map_err(|e| format!("invalid response_format {s:?}: {e}"))
}

// ---------------------------------------------------------------------------
// ModelDef <-> ModelDefJson
// ---------------------------------------------------------------------------

fn model_def_to_wire(d: &ModelDefCore) -> ModelDefJson {
    ModelDefJson {
        id: d.id.clone(),
        name: d.name.clone(),
        base_url: d.base_url.clone(),
        context_window: d.context_window,
        max_output_tokens: d.max_output_tokens,
        input: d.input.as_ref().map(|i| ModelInputJson {
            text: i.text,
            image: i.image,
        }),
        reasoning: d.reasoning,
        thinking_level_map: d.thinking_level_map.as_ref().map(|m| ThinkingLevelMapJson {
            off: m.off.clone(),
            minimal: m.minimal.clone(),
            low: m.low.clone(),
            medium: m.medium.clone(),
            high: m.high.clone(),
            xhigh: m.xhigh.clone(),
            max: m.max.clone(),
        }),
        cost: d.cost.as_ref().map(|c| ModelCostJson {
            input: c.input,
            output: c.output,
            cache_read: c.cache_read,
            cache_write: c.cache_write,
            reasoning: c.reasoning,
            tiers: c
                .tiers
                .iter()
                .map(|t| CostTierJson {
                    input_tokens_above: t.input_tokens_above,
                    input: t.input,
                    output: t.output,
                    cache_read: t.cache_read,
                    cache_write: t.cache_write,
                })
                .collect(),
        }),
        compat: d.compat.as_ref().map(|k| ModelCompatJson {
            uses_max_completion_tokens: k.uses_max_completion_tokens,
            forbids_sampling_params: k.forbids_sampling_params,
            temperature_policy: k.temperature_policy.clone(),
            supports_cache_control: k.supports_cache_control,
            supports_reasoning_effort: k.supports_reasoning_effort,
            requires_object_properties: k.requires_object_properties,
            requires_derefed_refs: k.requires_derefed_refs,
            supports_prompt_cache: k.supports_prompt_cache,
            thinking_format: k.thinking_format.clone(),
            vendor_behavior_profile: k.vendor_behavior_profile.clone(),
        }),
    }
}

fn model_def_from_wire(d: ModelDefJson) -> Result<ModelDefCore, String> {
    let id = d.id.trim().to_string();
    if id.is_empty() {
        return Err("model_defs entry has empty `id`".to_string());
    }
    if let Some(cw) = d.context_window {
        if cw == 0 {
            return Err(format!("model_defs[{id}].context_window must be > 0"));
        }
    }
    if let Some(mot) = d.max_output_tokens {
        if mot == 0 {
            return Err(format!("model_defs[{id}].max_output_tokens must be > 0"));
        }
    }
    Ok(ModelDefCore {
        id,
        name: d.name,
        base_url: d.base_url,
        context_window: d.context_window,
        max_output_tokens: d.max_output_tokens,
        input: d.input.map(|i| ModelInputCore {
            text: i.text,
            image: i.image,
        }),
        reasoning: d.reasoning,
        thinking_level_map: d.thinking_level_map.map(|m| ThinkingLevelMapCore {
            off: m.off,
            minimal: m.minimal,
            low: m.low,
            medium: m.medium,
            high: m.high,
            xhigh: m.xhigh,
            max: m.max,
        }),
        cost: d.cost.map(|c| ModelCostCore {
            input: c.input,
            output: c.output,
            cache_read: c.cache_read,
            cache_write: c.cache_write,
            reasoning: c.reasoning,
            tiers: c
                .tiers
                .into_iter()
                .map(|t| CostTierCore {
                    input_tokens_above: t.input_tokens_above,
                    input: t.input,
                    output: t.output,
                    cache_read: t.cache_read,
                    cache_write: t.cache_write,
                })
                .collect(),
        }),
        compat: d.compat.map(|k| ModelCompatCore {
            uses_max_completion_tokens: k.uses_max_completion_tokens,
            forbids_sampling_params: k.forbids_sampling_params,
            temperature_policy: k.temperature_policy,
            supports_cache_control: k.supports_cache_control,
            supports_reasoning_effort: k.supports_reasoning_effort,
            requires_object_properties: k.requires_object_properties,
            requires_derefed_refs: k.requires_derefed_refs,
            supports_prompt_cache: k.supports_prompt_cache,
            thinking_format: k.thinking_format,
            vendor_behavior_profile: k.vendor_behavior_profile,
        }),
    })
}

/// Drop entries that share an `id` with a previous one. The spec says
/// "per-id dedup"; we keep the FIRST occurrence so an operator who adds an
/// override in the Panel does not lose it because a stale read also carried
/// an entry. This is the helper side of the wire-roundtrip test that
/// confirms both copies survive the wire layer.
fn dedup_model_defs(
    defs: impl IntoIterator<Item = Result<ModelDefCore, String>>,
) -> Result<Vec<ModelDefCore>, String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for d in defs {
        let d = d?;
        if seen.insert(d.id.clone()) {
            out.push(d);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Local aliases for the core types so the conversions above stay readable.
// Keeping the imports at the top of the file would shadow the wire-side
// `ModelDefJson` / `ModelCostJson` etc. with the core-side names; this type
// alias block makes the wire-side names the canonical ones in this file.
// ---------------------------------------------------------------------------
use crate::config::types::provider::{
    CacheRetention as CacheRetentionCore, CostTier as CostTierCore, ModelCompat as ModelCompatCore,
    ModelCost as ModelCostCore, ModelDef as ModelDefCore, ModelInput as ModelInputCore,
    ResponseFormat as ResponseFormatCore, ThinkingLevelMap as ThinkingLevelMapCore,
};
