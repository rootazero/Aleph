//! Cheap-summary and strategy-planner provider builders.

use crate::sync_primitives::Arc;

use crate::config::Config;
use crate::providers::{create_provider, AiProvider};

/// Build the cheap-tier summarization provider for [`ContextCompactor`].
///
/// Resolves the summary model in two tiers:
/// 1. **Explicit** — `[context_budget] summary_model` (Reasonix `summaryModel`
///    parity). Whatever the operator names is used verbatim.
/// 2. **Auto** — when `summary_model` is unset/blank, fall back to the *primary*
///    provider preset's declared cheap aux model (`default_aux_model`:
///    openai→`gpt-5.4-mini`, anthropic→`claude-haiku-4-5`,
///    gemini→`gemini-3-flash-preview`, deepseek→`deepseek-v4-flash`). This makes the
///    preset's long-dormant `default_aux_model` field a live routing consumer:
///    a vendor with a declared cheap tier gets cost-efficient summarization with
///    zero config. Only an **explicitly declared** aux model triggers this — a
///    preset whose `default_aux_model` is `None` (or a custom/unknown provider
///    key) keeps the legacy main-LLM path, never silently routing to a
///    same-tier `default_model`.
///
/// In both tiers the cheap provider reuses the primary provider's full config
/// (api key, base url, protocol, timeouts) with only `models` swapped, so it
/// targets the same vendor — just a cheaper sibling.
///
/// Returns `None` (→ compactor reuses the main LLM) when:
/// - section missing / `enabled = false`;
/// - the primary provider key has no `[providers.*]` entry to clone;
/// - `summary_model` unset AND the primary preset declares no cheap aux model;
/// - the resolved model equals the primary's configured default (a separate
///   provider would be byte-identical — pointless). Setting `summary_model` to
///   the primary's own model is therefore the explicit opt-out from auto aux
///   routing;
/// - `create_provider` fails (bad protocol/preset) — logged, then degraded.
///
/// Fail-soft by construction: a misconfigured model never aborts boot and never
/// blocks summarization (the compactor falls back to the main LLM).
/// Resolve the model history summarization will actually run on, plus the
/// tier that named it (`"summary_model"` for the explicit operator override,
/// `"preset aux_model"` for the auto tier).
///
/// This is the single source for the two-tier resolution
/// [`build_cheap_summary_provider`] implements: explicit `[context_budget]
/// summary_model` first, then the primary preset's declared
/// `default_aux_model`. Returns `None` under the same gates as that builder
/// (section missing/disabled, no provider entry to clone, neither tier names
/// a model) — plus when the resolved model IS the provider's configured
/// default, since summarization then runs on the main model and no distinct
/// cheap provider is built. In that case the returned model is the primary's
/// own default, so callers that want "which model summarizes" (rather than
/// "should a cheap provider be built") get the true answer either way.
pub(crate) fn effective_summary_model(
    config: &Config,
    primary_provider_key: &str,
) -> Option<(String, &'static str)> {
    let cb = config.context_budget.as_ref()?;
    if !cb.enabled {
        return None;
    }
    let base = config.providers.get(primary_provider_key)?;
    let explicit = cb
        .summary_model
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let (model, source): (String, &'static str) = match explicit {
        Some(model) => (model.to_string(), "summary_model"),
        None => (
            crate::providers::get_preset(&primary_provider_key.to_lowercase())
                .and_then(|p| p.default_aux_model)?
                .to_string(),
            "preset aux_model",
        ),
    };
    if base.models.first().is_some_and(|m| m.as_str() == model) {
        // Resolved model == primary default: summarization runs on the main
        // model (the byte-identical cheap provider is never built).
        return base.models.first().map(|m| (m.clone(), "primary default"));
    }
    Some((model, source))
}

#[must_use]
pub fn build_cheap_summary_provider(
    config: &Config,
    primary_provider_key: &str,
) -> Option<Arc<dyn AiProvider>> {
    let cb = config.context_budget.as_ref()?;
    if !cb.enabled {
        return None;
    }

    let base = config.providers.get(primary_provider_key)?;

    let (summary_model, source) = effective_summary_model(config, primary_provider_key)?;
    if source == "primary default" {
        return None;
    }

    // rust-doctor-disable-next-line excessive-clone
    let mut cheap_cfg = base.clone();
    cheap_cfg.models = vec![summary_model.clone()];
    match create_provider(primary_provider_key, cheap_cfg) {
        Ok(provider) => {
            tracing::info!(
                provider = %primary_provider_key,
                summary_model = %summary_model,
                source,
                "context budget: routing history summarization to cheap-tier model"
            );
            Some(provider)
        }
        Err(e) => {
            tracing::warn!(
                provider = %primary_provider_key,
                summary_model = %summary_model,
                error = %e,
                "context budget: cheap summary provider build failed — summarization will use the main LLM"
            );
            None
        }
    }
}

/// Build the cheap-tier provider for the **dream pipeline**'s LLM stages
/// (`[memory.dreaming] model`).
///
/// Resolves in the same two tiers as [`build_cheap_summary_provider`], and for
/// the same reason: every LLM stage in the dream pipeline is a small
/// classification or summarization task (`note_drift` asks for one word out of
/// `CONSISTENT | CONTRADICTORY | STALE`; `daily_digest` writes a paragraph).
/// Running that unattended, nightly, on the operator's main reasoning model is
/// pure waste — a single night once cost tens of thousands of main-model calls.
///
/// 1. **Explicit** — `[memory.dreaming] model`, used verbatim.
/// 2. **Auto** — unset/blank ⇒ the primary preset's declared `default_aux_model`.
///    A vendor with no declared cheap tier keeps the main LLM rather than
///    silently routing to a same-tier sibling.
///
/// Returns `None` (⇒ dreaming reuses the main provider) when dreaming is
/// disabled, the primary key has no `[providers.*]` entry to clone, no cheap
/// model resolves, the resolved model *is* the primary's own default (setting
/// `model` to the main model is therefore the explicit opt-out from auto
/// routing), or `create_provider` fails.
///
/// Fail-soft: a misconfigured model never aborts boot and never blocks dreaming.
#[must_use]
pub fn build_dream_provider(
    config: &Config,
    primary_provider_key: &str,
) -> Option<Arc<dyn AiProvider>> {
    let dreaming = &config.memory.dreaming;
    if !dreaming.enabled {
        return None;
    }

    let base = config.providers.get(primary_provider_key)?;

    let explicit = dreaming
        .model
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let dream_model: String = match explicit {
        Some(model) => model.to_string(),
        None => crate::providers::get_preset(&primary_provider_key.to_lowercase())
            .and_then(|p| p.default_aux_model)?
            .to_string(),
    };

    // No-op when the resolved model is the primary's own default — the rebuilt
    // provider would be byte-identical, so reuse the main LLM.
    if base
        .models
        .first()
        .is_some_and(|m| m.as_str() == dream_model)
    {
        return None;
    }

    let source = if explicit.is_some() {
        "dreaming.model"
    } else {
        "preset aux_model"
    };
    // rust-doctor-disable-next-line excessive-clone
    let mut dream_cfg = base.clone();
    dream_cfg.models = vec![dream_model.clone()];
    match create_provider(primary_provider_key, dream_cfg) {
        Ok(provider) => {
            tracing::info!(
                provider = %primary_provider_key,
                dream_model = %dream_model,
                source,
                "dreaming: routing dream-pipeline stages to cheap-tier model"
            );
            Some(provider)
        }
        Err(e) => {
            tracing::warn!(
                provider = %primary_provider_key,
                dream_model = %dream_model,
                error = %e,
                "dreaming: cheap provider build failed — dream stages will use the main LLM"
            );
            None
        }
    }
}

/// Build the optional dedicated provider for the strategic-planner node
/// (`[strategy] planner_model`). Mirrors [`build_cheap_summary_provider`]'s
/// vendor-cloning approach — clone the primary provider's config, swap only the
/// `models` vec — but with **two deliberate differences** (spec §4/§9):
///
/// 1. **No tier-2 `default_aux_model` fallback.** Planning is reasoning-heavy,
///    so an unset/blank `planner_model` must NOT silently downgrade the
///    strategist to a flash-tier sibling. Unset ⇒ `None` ⇒ the planner reuses
///    the executor's main provider.
/// 2. The section defaults to enabled, but is still honoured as an off-switch.
///
/// Returns `None` (planner reuses the executor provider) when:
/// - `[strategy]` is absent, or `enabled = false`;
/// - `planner_model` is unset or blank;
/// - the primary provider key has no `[providers.*]` entry to clone;
/// - the resolved model equals the primary's configured default (a separate
///   provider would be byte-identical — pointless);
/// - `create_provider` fails (bad protocol/preset) — logged, then degraded.
///
/// Fail-soft by construction: a misconfigured model never aborts boot. Total
/// failure ⇒ the planner runs on the main provider; if the planner call itself
/// fails, no Strategy is stored and the command proceeds with a byte-identical
/// prompt.
#[must_use]
pub fn build_strategy_planner_provider(
    config: &Config,
    primary_provider_key: &str,
) -> Option<Arc<dyn AiProvider>> {
    let strategy = config.strategy.as_ref()?;
    if !strategy.enabled {
        return None;
    }

    let base = config.providers.get(primary_provider_key)?;

    // Tier 1 only: explicit operator override. No `default_aux_model` tier-2 —
    // an unset/blank planner_model reuses the executor provider (return None).
    let planner_model: String = strategy
        .planner_model
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())?
        .to_string();

    // No-op when the resolved planner model is the primary's own default model —
    // the rebuilt provider would be byte-identical, so reuse the main LLM.
    if base
        .models
        .first()
        .is_some_and(|m| m.as_str() == planner_model)
    {
        return None;
    }

    // rust-doctor-disable-next-line excessive-clone
    let mut planner_cfg = base.clone();
    planner_cfg.models = vec![planner_model.clone()];
    match create_provider(primary_provider_key, planner_cfg) {
        Ok(provider) => {
            tracing::info!(
                provider = %primary_provider_key,
                planner_model = %planner_model,
                "strategy: routing strategic-planner node to a dedicated model"
            );
            Some(provider)
        }
        Err(e) => {
            tracing::warn!(
                provider = %primary_provider_key,
                planner_model = %planner_model,
                error = %e,
                "strategy: planner provider build failed — planner will use the main LLM"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::orchestrator::deps_builder::common::cfg_with_fallback;
    use crate::{ContextBudgetToml, ProviderConfig, StrategyToml};

    /// Config with `[context_budget]` enabled, a `summary_model`, and a single
    /// mock-protocol primary provider keyed `key` whose default model is
    /// `primary_model`. The `key` selects which preset (if any) the auto
    /// aux-model fallback resolves against; `"primary"` is a non-preset key.
    fn cfg_summary_keyed(key: &str, primary_model: &str, summary_model: Option<&str>) -> Config {
        let mut primary = ProviderConfig::test_config(primary_model);
        primary.protocol = Some("mock".to_string());
        let mut cfg = cfg_with_fallback(None, vec![(key, primary)]);
        cfg.context_budget = Some(ContextBudgetToml {
            enabled: true,
            summary_model: summary_model.map(str::to_string),
            ..ContextBudgetToml::default()
        });
        cfg
    }

    /// `cfg_summary_keyed` with the non-preset key `"primary"` (no aux fallback).
    fn cfg_summary(primary_model: &str, summary_model: Option<&str>) -> Config {
        cfg_summary_keyed("primary", primary_model, summary_model)
    }

    #[test]
    fn cheap_summary_none_when_section_missing() {
        // No `[context_budget]` at all → no cheap provider (legacy main-LLM path).
        let cfg = Config::default();
        assert!(build_cheap_summary_provider(&cfg, "primary").is_none());
    }

    #[test]
    fn cheap_summary_none_when_disabled() {
        let mut cfg = cfg_summary("main-model", Some("cheap-model"));
        cfg.context_budget.as_mut().unwrap().enabled = false;
        assert!(build_cheap_summary_provider(&cfg, "primary").is_none());
    }

    #[test]
    fn cheap_summary_none_when_unset_and_no_preset_aux() {
        // Key "primary" has no preset → no declared aux model → auto fallback
        // finds nothing → main LLM. Holds for both unset and blank.
        let cfg = cfg_summary("main-model", None);
        assert!(build_cheap_summary_provider(&cfg, "primary").is_none());
        let cfg = cfg_summary("main-model", Some("   "));
        assert!(build_cheap_summary_provider(&cfg, "primary").is_none());
    }

    #[test]
    fn cheap_summary_auto_falls_back_to_preset_aux_model() {
        // summary_model unset + a preset that declares a cheap aux model
        // (claude → claude-haiku-4-5) + a distinct configured default →
        // auto-route summarization to the aux model. This is the dead
        // `default_aux_model` field's live routing consumer.
        let cfg = cfg_summary_keyed("claude", "claude-opus-4-8", None);
        assert!(
            build_cheap_summary_provider(&cfg, "claude").is_some(),
            "unset summary_model must auto-fall back to the preset's declared aux model"
        );
    }

    #[test]
    fn cheap_summary_none_when_unset_and_preset_declares_no_aux() {
        // A real preset whose `default_aux_model` is None (cerebras) must NOT
        // route to its `default_model` — that would be a same-tier no-op. Auto
        // fallback fires only for an explicitly declared cheap aux model.
        let cfg = cfg_summary_keyed("cerebras", "some-model", None);
        assert!(build_cheap_summary_provider(&cfg, "cerebras").is_none());
    }

    #[test]
    fn cheap_summary_none_when_aux_equals_configured_default() {
        // Auto fallback resolves claude's aux (claude-haiku-4-5), but the
        // operator already runs that as the primary model → rebuilding would be
        // byte-identical → reuse the main LLM. (Also the explicit opt-out path:
        // setting summary_model to the primary's own model.)
        let cfg = cfg_summary_keyed("claude", "claude-haiku-4-5", None);
        assert!(build_cheap_summary_provider(&cfg, "claude").is_none());
    }

    #[test]
    fn cheap_summary_none_when_equals_primary_default() {
        // A summary model identical to the primary's default would rebuild a
        // byte-identical provider — pointless, so reuse the main LLM.
        let cfg = cfg_summary("same-model", Some("same-model"));
        assert!(build_cheap_summary_provider(&cfg, "primary").is_none());
    }

    #[test]
    fn cheap_summary_none_when_primary_key_absent() {
        // summary_model set but the named primary has no [providers.*] entry.
        let cfg = cfg_summary("main-model", Some("cheap-model"));
        assert!(build_cheap_summary_provider(&cfg, "does-not-exist").is_none());
    }

    #[test]
    fn cheap_summary_some_when_distinct_model_builds() {
        // Enabled + a distinct, buildable summary model → a cheap provider that
        // targets the primary vendor (mock protocol here) with the swapped model.
        let cfg = cfg_summary("main-model", Some("cheap-model"));
        assert!(
            build_cheap_summary_provider(&cfg, "primary").is_some(),
            "enabled + distinct buildable summary model must yield a cheap provider"
        );
    }

    // ── strategy planner provider wiring (tier-1 only, no aux fallback) ──

    /// Config with `[strategy]` enabled, an optional `planner_model`, and a
    /// single mock-protocol primary provider keyed `key` whose default model is
    /// `primary_model`. Mirrors `cfg_summary_keyed`, but the planner builder
    /// reads `config.strategy` and has NO `default_aux_model` tier-2 fallback —
    /// so the `key` preset is irrelevant to its resolution.
    fn cfg_strategy_keyed(key: &str, primary_model: &str, planner_model: Option<&str>) -> Config {
        let mut primary = ProviderConfig::test_config(primary_model);
        primary.protocol = Some("mock".to_string());
        let mut cfg = cfg_with_fallback(None, vec![(key, primary)]);
        cfg.strategy = Some(StrategyToml {
            enabled: true,
            planner_model: planner_model.map(str::to_string),
            ..StrategyToml::default()
        });
        cfg
    }

    /// `cfg_strategy_keyed` with the non-preset key `"primary"`.
    fn cfg_strategy(primary_model: &str, planner_model: Option<&str>) -> Config {
        cfg_strategy_keyed("primary", primary_model, planner_model)
    }

    #[test]
    fn strategy_planner_none_when_section_missing() {
        // No `[strategy]` at all → no separate planner provider (the planner
        // reuses the executor's main provider).
        let cfg = Config::default();
        assert!(build_strategy_planner_provider(&cfg, "primary").is_none());
    }

    #[test]
    fn strategy_planner_none_when_disabled() {
        let mut cfg = cfg_strategy("main-model", Some("planner-model"));
        cfg.strategy.as_mut().unwrap().enabled = false;
        assert!(build_strategy_planner_provider(&cfg, "primary").is_none());
    }

    #[test]
    fn strategy_planner_none_when_unset_or_blank() {
        // planner_model unset/blank → reuse the executor provider (None). Unlike
        // summary_model there is NO cheap-tier auto-fallback to downgrade to.
        let cfg = cfg_strategy("main-model", None);
        assert!(build_strategy_planner_provider(&cfg, "primary").is_none());
        let cfg = cfg_strategy("main-model", Some("   "));
        assert!(build_strategy_planner_provider(&cfg, "primary").is_none());
    }

    #[test]
    fn strategy_planner_none_even_with_preset_aux_model() {
        // A preset that declares a cheap aux model (claude → claude-haiku-4-5)
        // must NOT trigger an aux fallback here — the planner is reasoning-heavy,
        // so unset planner_model reuses the executor, never the flash sibling.
        let cfg = cfg_strategy_keyed("claude", "claude-opus-4-8", None);
        assert!(
            build_strategy_planner_provider(&cfg, "claude").is_none(),
            "unset planner_model must reuse the executor, never auto-downgrade to aux"
        );
    }

    #[test]
    fn strategy_planner_none_when_equals_primary_default() {
        // A planner model identical to the primary's default would rebuild a
        // byte-identical provider — pointless, so reuse the main LLM.
        let cfg = cfg_strategy("same-model", Some("same-model"));
        assert!(build_strategy_planner_provider(&cfg, "primary").is_none());
    }

    #[test]
    fn strategy_planner_none_when_primary_key_absent() {
        // planner_model set but the named primary has no [providers.*] entry.
        let cfg = cfg_strategy("main-model", Some("planner-model"));
        assert!(build_strategy_planner_provider(&cfg, "does-not-exist").is_none());
    }

    #[test]
    fn strategy_planner_some_when_distinct_model_builds() {
        // Enabled + a distinct, buildable planner model → a planner provider
        // that targets the primary vendor (mock protocol) with the swapped model.
        let cfg = cfg_strategy("main-model", Some("planner-model"));
        assert!(
            build_strategy_planner_provider(&cfg, "primary").is_some(),
            "enabled + distinct buildable planner model must yield a planner provider"
        );
    }
}
