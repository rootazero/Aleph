//! Context-budget derivation and building.

use crate::config::types::phase6_wiring::ModelThresholdToml;
use crate::config::types::ProviderConfig;
use crate::config::Config;
use crate::context::budget::ContextBudgetConfig;
use crate::providers::model_catalog::{capabilities_for, resolve_context_window_with_override};

/// Default model context-window estimate (tokens) used when neither the
/// primary provider nor the capability catalog reveals the active model's
/// window. Compaction thresholds are fractions of the *usable* budget derived
/// from it (window minus the reserved output margin).
const DEFAULT_CONTEXT_TOKEN_BUDGET: u64 = 200_000;

/// Output-token margin reserved out of the context window when neither the
/// provider's `max_tokens` nor the catalog's `max_output_tokens` is known. The
/// usable input budget is `window - reserve`, guaranteeing room for a reply
/// even at critical pressure.
const DEFAULT_OUTPUT_RESERVE: u64 = 8_192;

/// Floor for the derived usable budget. A mis-declared tiny window (or a
/// reserve ≥ window) must never collapse the budget to zero/near-zero, which
/// would force compaction or a final reply on the very first turn.
const MIN_USABLE_BUDGET: u64 = 16_384;

/// Fraction below which an auto-sized chain-minimum budget is considered to
/// *materially* undercut the primary's own window: a narrow fallback sibling
/// dragging the budget under 60% of the primary's usable budget means the
/// primary model compacts >40% earlier than its real window would require.
/// Purely an observability threshold — it changes no budget value.
const CHAIN_MIN_UNDERCUT_WARN_FRACTION: f64 = 0.60;

/// Whether an auto-sized chain-minimum budget materially undercuts the
/// primary's own usable window (see [`CHAIN_MIN_UNDERCUT_WARN_FRACTION`]).
///
/// The chain-min design is deliberately conservative — it sizes compaction for
/// the *smallest* window any in-request failover migration could land on — but
/// that makes a single narrow fallback sibling silently shrink the effective
/// context of a wide primary, the one genuinely surprising consequence of the
/// design. This predicate gates the one-line startup advisory that explains it.
/// Returns `false` when the primary budget is unknown (`0`), so an undeterminable
/// comparison never produces a spurious warning.
fn chain_min_materially_undercuts_primary(chain_min_usable: u64, primary_usable: u64) -> bool {
    primary_usable > 0
        && (chain_min_usable as f64) < (primary_usable as f64) * CHAIN_MIN_UNDERCUT_WARN_FRACTION
}

/// Historical flat default compaction thresholds, also the *caps* for the
/// window-aware auto-derivation below: a window wide enough to absorb a tool
/// spike at these fractions keeps them exactly, so wide models (and the
/// calibrated common case) stay byte-identical to the pre-wiring behaviour.
const DEFAULT_WARNING_THRESHOLD: f64 = 0.70;
const DEFAULT_CRITICAL_THRESHOLD: f64 = 0.85;

/// Absolute token headroom the auto-derived *warning* (compact) line keeps
/// below the critical line. This is the heart of model-aware compaction
/// *timing*: the same `0.70` fraction that leaves ~130k of runway on a 1M
/// window leaves only ~30k on a 200k window — not enough to absorb one large
/// tool result (a full file read / web fetch / search dump is easily 40k+
/// tokens) landing in a single turn, which would leap the whole warning→critical
/// band and overflow before compaction ever fires. Sizing the band by an
/// *absolute* token count instead of a flat fraction makes a narrow window
/// start compacting earlier so it keeps the same spike protection a wide
/// window gets for free.
const WARNING_SPIKE_HEADROOM_TOKENS: f64 = 48_000.0;

/// Lower bound for the auto-derived warning fraction, so a very small window
/// cannot drive the compaction trigger absurdly low (summarizing nearly every
/// turn). Operators with tiny windows can still set an explicit
/// `warning_threshold` to go lower.
const MIN_AUTO_WARNING_THRESHOLD: f64 = 0.40;

/// Window-aware *default* warning (compaction) fraction, used only when neither
/// a per-model nor a global `warning_threshold` is configured.
///
/// Keeps a model-independent *absolute* token band of
/// [`WARNING_SPIKE_HEADROOM_TOKENS`] below the effective `critical` line: a wide
/// 1M window resolves to the historical [`DEFAULT_WARNING_THRESHOLD`], while a
/// narrow 200k window automatically compacts earlier. Floored at
/// [`MIN_AUTO_WARNING_THRESHOLD`] and capped at [`DEFAULT_WARNING_THRESHOLD`],
/// so the result is always in a sane range; the single-arg `min`/`max` avoid the
/// `f64::clamp` min>max panic risk for a pathologically low configured critical
/// (which the downstream threshold-ordering validation rejects anyway).
#[allow(clippy::manual_clamp)] // intentional min/max chain; see doc above re: clamp panic risk
fn window_aware_warning_default(usable: u64, critical: f64) -> f64 {
    if usable == 0 {
        return DEFAULT_WARNING_THRESHOLD.min(critical);
    }
    let spike_fraction = WARNING_SPIKE_HEADROOM_TOKENS / usable as f64;
    (critical - spike_fraction)
        .min(DEFAULT_WARNING_THRESHOLD)
        .max(MIN_AUTO_WARNING_THRESHOLD)
}

/// Historical fixed count of recent messages compaction keeps verbatim (never
/// summarized). Also the *floor* of the window-aware derivation below: a
/// narrow window keeps exactly this many, so the legacy 200k default path is
/// byte-identical. Tuned against [`FRESH_TAIL_ANCHOR_BUDGET`].
const FRESH_TAIL_BASE_COUNT: usize = 6;

/// Usable budget the [`FRESH_TAIL_BASE_COUNT`] was tuned for. Windows at or
/// below this keep the base count; wider windows keep proportionally more.
/// Equals [`DEFAULT_CONTEXT_TOKEN_BUDGET`] so an un-tuned 200k model is exactly
/// the historical 6 (the `usable = 200k − reserve` is just under the anchor, so
/// it floors to the base — back-compatible).
const FRESH_TAIL_ANCHOR_BUDGET: u64 = DEFAULT_CONTEXT_TOKEN_BUDGET;

/// Usable-budget increment that buys one extra retained recent message above
/// [`FRESH_TAIL_BASE_COUNT`]. Sized so a 1M window keeps ~2× the base tail
/// (≈12) while a 200k window keeps the base 6 — the count-based analogue of the
/// references' token-budget `keepRecentTokens` (openclaw/pi 20k, hermes
/// `threshold × 0.2`), which all scale recent-context retention to the window.
const FRESH_TAIL_TOKENS_PER_EXTRA_MSG: u64 = 120_000;

/// Cap on the window-aware retained tail, so even a multi-million-token window
/// cannot let the protected tail dominate the compaction window (the compactor
/// summarizes only what sits *before* the tail).
const FRESH_TAIL_MAX_COUNT: usize = 16;

/// Window-aware count of recent messages compaction keeps verbatim.
///
/// The references all keep recent context as a *token budget* that scales with
/// the model window (openclaw/pi `keepRecentTokens`, hermes `tail_token_budget
/// = threshold × ratio`), so a wide-window model preserves more recent turns
/// untouched than a narrow one. Aleph's compactor is count-based, so this maps
/// that intent onto a message count: anchored at [`FRESH_TAIL_BASE_COUNT`] for
/// the [`FRESH_TAIL_ANCHOR_BUDGET`]-sized default window and growing one message
/// per [`FRESH_TAIL_TOKENS_PER_EXTRA_MSG`] of extra usable budget, capped at
/// [`FRESH_TAIL_MAX_COUNT`]. Wider windows thus summarize less aggressively
/// (better continuity, fewer lossy side-channel summaries) while a narrow
/// window keeps exactly the historical 6. Never drops below the base, so the
/// active-task tail is always protected regardless of window size.
fn window_aware_fresh_tail(usable: u64) -> usize {
    let extra = usable.saturating_sub(FRESH_TAIL_ANCHOR_BUDGET) / FRESH_TAIL_TOKENS_PER_EXTRA_MSG;
    FRESH_TAIL_BASE_COUNT
        .saturating_add(extra as usize)
        .min(FRESH_TAIL_MAX_COUNT)
}

/// Build the optional per-run context-budget config from `[context_budget]`.
///
/// Returns `None` when the section is absent or `enabled = false` — the
/// orchestrator then leaves `HarnessDeps.context_budget`/`context_compactor`
/// as `None`, so behavior is identical to before this wiring (no mid-run
/// compaction). When `Some`, `AgentHarnessRunner::run` constructs a *fresh*
/// `ContextBudget` per run (its circuit-breaker / diminishing-returns state
/// must not be shared across concurrent sessions).
///
/// `token_budget` and the two thresholds are user-tunable; the remaining
/// `ContextBudgetConfig` fields use validated internal defaults (KISS — not
/// every knob needs a toml surface).
/// A model-aware compaction budget derived from the primary model's real
/// context window. `usable` is what the pressure sensor consumes; the rest is
/// kept for the one observability line at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DerivedBudget {
    usable: u64,
    window: u64,
    reserve: u64,
    source: &'static str,
}

/// Derive the usable context-compaction budget for `model` running on
/// `primary`, in tokens.
///
/// Precedence (each independently for window and reserve):
///   - **window**:  `provider.context_window` ▸ `capabilities_for(model)` ▸
///     [`DEFAULT_CONTEXT_TOKEN_BUDGET`]
///   - **reserve**: `provider.max_tokens` ▸ `capabilities_for(model)` ▸
///     [`DEFAULT_OUTPUT_RESERVE`]
///
/// `usable = window.saturating_sub(reserve)`, floored at [`MIN_USABLE_BUDGET`]
/// so a mis-declared window can never collapse the budget. Static (no harness
/// involvement): a 200k-window model and a 1M-window model get proportionally
/// different absolute compaction trigger points once the warning/critical
/// fractions are applied.
fn derive_token_budget(primary: Option<&ProviderConfig>, model: Option<&str>) -> DerivedBudget {
    let caps = model.and_then(capabilities_for);
    let (window, source) = primary
        .and_then(|p| p.context_window)
        .map(|w| (u64::from(w), "config"))
        .or_else(|| caps.map(|c| (u64::from(c.context_window), "catalog")))
        .unwrap_or((DEFAULT_CONTEXT_TOKEN_BUDGET, "default"));
    let reserve = primary
        .and_then(|p| p.max_tokens)
        .map(u64::from)
        .or_else(|| caps.map(|c| u64::from(c.max_output_tokens)))
        .unwrap_or(DEFAULT_OUTPUT_RESERVE);
    let usable = window.saturating_sub(reserve).max(MIN_USABLE_BUDGET);
    DerivedBudget {
        usable,
        window,
        reserve,
        source,
    }
}

/// Outcome of [`derive_chain_min_budget`]: the smallest budget on the chain,
/// plus which provider/model set it (for the one startup observability line).
struct ChainMinBudget {
    budget: DerivedBudget,
    provider: String,
    model: Option<String>,
    chain_len: usize,
}

/// Provider keys participating in the failover chain, **primary first**.
///
/// Config-level twin of [`assemble_fallbacks`]'s selection, resolved from
/// `[providers]` + `[fallback_provider].chain` alone (no built provider Arcs):
/// explicit chain entries that exist and are enabled (primary excluded), or —
/// when that yields nothing — every *other* enabled provider (name-sorted).
/// Mirrors the set the live `FailoverProvider` can migrate into, so the
/// compaction budget can be sized for the smallest window any in-request model
/// migration could land on (see [`derive_chain_min_budget`]). A disabled or
/// undefined provider is dropped here exactly as it would be at chain assembly,
/// so it can never drag the budget down for a route that can't actually happen.
fn resolve_chain_provider_keys(config: &Config, primary_provider_key: &str) -> Vec<String> {
    let enabled = |name: &str| config.providers.get(name).is_some_and(|p| p.enabled);
    let mut fallbacks: Vec<String> = Vec::new();
    if let Some(fb) = config.fallback_provider.as_ref() {
        for name in fb.resolved_chain() {
            if name.eq_ignore_ascii_case(primary_provider_key) || !enabled(&name) {
                continue;
            }
            if !fallbacks.iter().any(|f| f.eq_ignore_ascii_case(&name)) {
                fallbacks.push(name);
            }
        }
    }
    if fallbacks.is_empty() {
        let mut names: Vec<String> = config
            .providers
            .iter()
            .filter(|(n, p)| p.enabled && !n.eq_ignore_ascii_case(primary_provider_key))
            .map(|(n, _)| n.clone())
            .collect();
        names.sort();
        fallbacks = names;
    }

    let mut keys = Vec::with_capacity(fallbacks.len() + 1);
    keys.push(primary_provider_key.to_string());
    keys.extend(fallbacks);
    keys
}

/// The chain-minimum compaction budget: the smallest [`derive_token_budget`]
/// `usable` across the primary and every provider failover can migrate into.
///
/// Build-time budgeting sizes the window from the *primary* model — already
/// conservatively safe when the primary is the largest. But an in-request
/// rate-limit migration (`Decision::RateLimited` → `continue 'model`) can land
/// on a sibling with a *smaller* window; a budget cut for a 1M primary would
/// then overflow a 200k sibling. Taking the minimum over the resolved chain
/// keeps the budget safe for whichever model the request ends up on — without
/// the per-turn `AiProvider`-boundary invasion a fully dynamic budget would
/// require. Returns the winning (smallest) provider/model for the startup log.
fn derive_chain_min_budget(config: &Config, primary_provider_key: &str) -> ChainMinBudget {
    let keys = resolve_chain_provider_keys(config, primary_provider_key);
    let mut best: Option<ChainMinBudget> = None;
    for key in &keys {
        let provider = config.providers.get(key);
        // The live FailoverProvider can migrate across EVERY model a provider
        // declares (model_catalog is seeded with the full list), so size the
        // budget from the smallest window any of them could land on — not just
        // `models.first()`, which would let a narrower sibling listed later
        // overflow the budget the chain-min design exists to keep safe. An empty
        // models list still contributes one evaluation (config window / default).
        let models: Vec<Option<&str>> = match provider {
            Some(p) if !p.models.is_empty() => p.models.iter().map(|m| Some(m.as_str())).collect(),
            _ => vec![None],
        };
        for model in models {
            let budget = derive_token_budget(provider, model);
            let is_smaller = best
                .as_ref()
                .is_none_or(|b| budget.usable < b.budget.usable);
            if is_smaller {
                best = Some(ChainMinBudget {
                    budget,
                    provider: key.clone(),
                    model: model.map(str::to_string),
                    chain_len: keys.len(),
                });
            }
        }
    }
    // `keys` always holds at least the primary, so `best` is always `Some`;
    // the fallback keeps the function total without an `unwrap`.
    best.unwrap_or_else(|| ChainMinBudget {
        budget: derive_token_budget(None, None),
        provider: primary_provider_key.to_string(),
        model: None,
        chain_len: 0,
    })
}

pub fn build_context_budget_config(
    config: &Config,
    primary_provider_key: &str,
) -> Option<ContextBudgetConfig> {
    let cb = config.context_budget.as_ref()?;
    if !cb.enabled {
        return None;
    }
    // Resolve the chain-minimum model once: its window sizes the budget (unless
    // overridden) AND its identity keys the per-model threshold override below,
    // so the trigger fractions always match the model the budget is sized for.
    let derived = derive_chain_min_budget(config, primary_provider_key);
    // An explicit `token_budget` is an operator override — honored verbatim
    // (back-compat). Otherwise use the model-aware budget sized for the
    // *smallest* window on the resolved failover chain, so an in-request model
    // migration to a narrower sibling can never overflow the compaction budget.
    let token_budget = match cb.token_budget {
        Some(explicit) => explicit,
        None => {
            tracing::info!(
                provider = %derived.provider,
                model = derived.model.as_deref().unwrap_or("<unknown>"),
                window = derived.budget.window,
                reserve = derived.budget.reserve,
                usable = derived.budget.usable,
                source = derived.budget.source,
                chain_len = derived.chain_len,
                "context budget derived from chain-minimum model context window"
            );
            // Advisory (P7/observability): when a *narrower* fallback sibling
            // wins the chain-minimum, the compaction budget is capped well
            // below the primary's own window, so the primary model compacts far
            // earlier than its real context would require. This is the single
            // most confusing consequence of the conservative chain-min design;
            // surface it with an actionable line. Log-only — the safe (smaller)
            // budget stands; the operator can reorder/trim the chain or pin an
            // explicit `token_budget` if the early compaction is unwanted.
            let primary = config.providers.get(primary_provider_key);
            let primary_model = primary.and_then(|p| p.models.first().map(String::as_str));
            let primary_usable = derive_token_budget(primary, primary_model).usable;
            if !derived.provider.eq_ignore_ascii_case(primary_provider_key)
                && chain_min_materially_undercuts_primary(derived.budget.usable, primary_usable)
            {
                tracing::warn!(
                    primary = %primary_provider_key,
                    primary_usable,
                    chain_min_provider = %derived.provider,
                    chain_min_usable = derived.budget.usable,
                    "context budget: a narrower fallback sibling caps the compaction budget well \
                     below the primary's window — the primary will compact early. Reorder/trim \
                     [fallback_provider].chain or set an explicit [context_budget] token_budget \
                     to override."
                );
            }
            derived.budget.usable
        }
    };
    // Per-model trigger-point override, keyed off the resolved chain-min model.
    // Each field falls back to the global threshold, then the built-in default
    // (flat critical, window-aware warning), so an absent or non-matching
    // override leaves the model-aware defaults intact.
    let model_override = cb.threshold_override_for(derived.model.as_deref(), &derived.provider);
    if let Some(o) = model_override {
        tracing::info!(
            matcher = %o.model,
            model = derived.model.as_deref().unwrap_or("<unknown>"),
            provider = %derived.provider,
            "context budget: applying per-model threshold override"
        );
    }
    // Critical (hard-stop) keeps the flat default: once `FinalReply` fires the
    // harness runs no further tools, so no tool result can be appended between
    // the critical check and the bounded final reply — the hard line is safe at
    // a fixed fraction regardless of window size.
    let critical_threshold = model_override
        .and_then(|o| o.critical_threshold)
        .or(cb.critical_threshold)
        .unwrap_or(DEFAULT_CRITICAL_THRESHOLD);
    // Warning (compaction trigger) is window-aware: absent explicit config, a
    // narrow window compacts earlier so one large tool result cannot leap the
    // whole band and overshoot critical before compaction fires (see
    // `window_aware_warning_default`). A configured threshold still wins.
    let auto_warning = window_aware_warning_default(token_budget, critical_threshold);
    let warning_threshold = model_override
        .and_then(|o| o.warning_threshold)
        .or(cb.warning_threshold)
        .unwrap_or(auto_warning);

    // Defensive validation (P7): a misconfigured budget silently cuts off
    // every run — e.g. inverted thresholds make `CompactAndContinue`
    // unreachable so every warning-pressure turn escalates to `FinalReply`.
    // Reject rather than degrade.
    if token_budget == 0 {
        tracing::warn!("context_budget: token_budget must be > 0; disabling");
        return None;
    }
    if !(warning_threshold > 0.0
        && warning_threshold < critical_threshold
        && critical_threshold <= 1.0)
    {
        tracing::warn!(
            warning_threshold,
            critical_threshold,
            "context_budget: require 0 < warning_threshold < critical_threshold <= 1.0; disabling"
        );
        return None;
    }

    Some(ContextBudgetConfig {
        token_budget,
        warning_threshold,
        critical_threshold,
        // Internal tuning — validated defaults, deliberately not exposed as
        // toml knobs (KISS: every run inherits the same compaction cadence).
        // The prose anchor reuses the estimator's own canonical default rather
        // than a duplicated literal (single source of truth); CJK/code content
        // is auto-densified by the content-aware estimator regardless.
        token_estimate_ratio: crate::context::budget::pressure::DEFAULT_PROSE_RATIO,
        // Model-aware retention: the recent tail kept verbatim scales with the
        // resolved budget (the same one pressure triggers against), mirroring the
        // references' window-scaled `keepRecentTokens`. A 200k window keeps the
        // historical 6; a 1M window keeps ~12 (see `window_aware_fresh_tail`).
        fresh_tail_count: window_aware_fresh_tail(token_budget),
        circuit_breaker_max: 3,
        max_splits: 3,
    })
}

// =============================================================================
// Per-run serving-model refinement
// =============================================================================

/// Boot-time companion to [`build_context_budget_config`] that re-keys the
/// (deliberately conservative, chain-minimum) budget onto the model ACTUALLY
/// serving each run.
///
/// The startup config is frozen to the smallest window on the failover chain —
/// the right floor for in-request migrations, but blind to per-run model
/// selection: a session `select_model` pick, an agent `model_hint`, or a
/// brain-level strict pin can all put a run on a model the chain-min
/// derivation never saw, while the context gauge already follows that serving
/// model (`runner_impl`'s `gauge_model`). A run pinned to a NARROWER model
/// than the chain minimum then compacts against a budget its real window
/// cannot honour (only the reactive rescue saves it), and a per-model
/// threshold override keys off the chain-min model rather than the model in
/// use. pi and opencode both evaluate compaction timing against the CURRENT
/// model every turn; this refiner closes that gap per run, without weakening
/// the failover floor: the refined budget is `min(chain_min, serving)` — it
/// can only compact EARLIER, never later than the startup-safe value.
#[derive(Debug, Clone)]
pub struct ContextBudgetRefiner {
    /// Operator-pinned `[context_budget] token_budget` — honored verbatim
    /// (same back-compat contract as the startup path).
    explicit_token_budget: Option<u64>,
    /// Raw global thresholds, pre-fold, so the serving model's override
    /// re-matches against the same fallback chain as the startup fold.
    global_warning: Option<f64>,
    global_critical: Option<f64>,
    /// Per-model override entries (`[[context_budget.model_thresholds]]`).
    model_thresholds: Vec<ModelThresholdToml>,
    /// Primary provider key + its declared `max_tokens` reserve override,
    /// applied only when the serving provider IS the primary (mirrors
    /// [`derive_token_budget`]'s reserve precedence for that provider).
    primary_provider_key: String,
    primary_max_tokens: Option<u32>,
}

/// Capture the refinement inputs from `[context_budget]`. Returns `None`
/// under exactly the same gate as [`build_context_budget_config`] (section
/// absent or disabled), so the two handles always come and go together.
#[must_use]
pub fn build_context_budget_refiner(
    config: &Config,
    primary_provider_key: &str,
) -> Option<ContextBudgetRefiner> {
    let cb = config.context_budget.as_ref()?;
    if !cb.enabled {
        return None;
    }
    Some(ContextBudgetRefiner {
        explicit_token_budget: cb.token_budget,
        global_warning: cb.warning_threshold,
        global_critical: cb.critical_threshold,
        model_thresholds: cb.model_thresholds.clone(),
        primary_provider_key: primary_provider_key.to_string(),
        primary_max_tokens: config
            .providers
            .get(primary_provider_key)
            .and_then(|p| p.max_tokens),
    })
}

impl ContextBudgetRefiner {
    /// Re-key `base` (the chain-minimum startup config) onto the model serving
    /// this run.
    ///
    /// - **budget**: `min(base, serving window − reserve)` unless the operator
    ///   pinned `token_budget` verbatim. The min keeps in-request failover
    ///   migrations safe: refinement can never relax the budget past the
    ///   chain-minimum floor.
    /// - **thresholds**: the per-model override re-matches against the serving
    ///   model/provider (first-match-wins, same semantics as the startup
    ///   fold); unset fields fall back to the global config, then the
    ///   window-aware / flat defaults — computed against the REFINED budget,
    ///   so the spike-headroom band tracks the numbers actually in force.
    /// - **fresh tail**: re-derived from the refined budget, so a narrower
    ///   serving model also protects a smaller recent tail.
    ///
    /// Byte-identical to `base` in the common case (the serving model is the
    /// primary's own model whose window already sits at or above the chain
    /// minimum), and whenever the serving model is unidentifiable (absent
    /// from the capability catalog AND no configured window override) — an
    /// unknown model id must never drag the budget down to the conservative
    /// catalog fallback on guesswork. Any refined combination that would fail
    /// the startup threshold-ordering gate also falls back to `base` (already
    /// validated) instead of disabling the budget mid-flight.
    #[must_use]
    pub fn refine_for_serving_model(
        &self,
        base: &ContextBudgetConfig,
        serving_model: &str,
        serving_provider: &str,
        window_override: Option<u32>,
    ) -> ContextBudgetConfig {
        let caps = capabilities_for(serving_model);
        if caps.is_none() && window_override.is_none_or(|w| w == 0) {
            return base.clone();
        }
        let window = u64::from(resolve_context_window_with_override(
            window_override,
            serving_model,
        ));
        let reserve = if serving_provider.eq_ignore_ascii_case(&self.primary_provider_key) {
            self.primary_max_tokens.map(u64::from)
        } else {
            None
        }
        .or_else(|| caps.map(|c| u64::from(c.max_output_tokens)))
        .unwrap_or(DEFAULT_OUTPUT_RESERVE);
        let serving_usable = window.saturating_sub(reserve).max(MIN_USABLE_BUDGET);

        let token_budget = match self.explicit_token_budget {
            Some(explicit) => explicit,
            None => base.token_budget.min(serving_usable),
        };
        if token_budget == 0 {
            return base.clone();
        }

        let model_override = ModelThresholdToml::first_match(
            &self.model_thresholds,
            Some(serving_model),
            serving_provider,
        );
        let critical_threshold = model_override
            .and_then(|o| o.critical_threshold)
            .or(self.global_critical)
            .unwrap_or(DEFAULT_CRITICAL_THRESHOLD);
        let auto_warning = window_aware_warning_default(token_budget, critical_threshold);
        let warning_threshold = model_override
            .and_then(|o| o.warning_threshold)
            .or(self.global_warning)
            .unwrap_or(auto_warning);
        // Same defensive gate as the startup fold — but degrading to the
        // (already validated) base rather than disabling the budget.
        if !(warning_threshold > 0.0
            && warning_threshold < critical_threshold
            && critical_threshold <= 1.0)
        {
            return base.clone();
        }

        let refined = ContextBudgetConfig {
            token_budget,
            warning_threshold,
            critical_threshold,
            fresh_tail_count: window_aware_fresh_tail(token_budget),
            ..base.clone()
        };
        if refined.token_budget != base.token_budget
            || refined.warning_threshold != base.warning_threshold
            || refined.critical_threshold != base.critical_threshold
        {
            tracing::info!(
                serving_model,
                serving_provider,
                base_budget = base.token_budget,
                refined_budget = refined.token_budget,
                warning = refined.warning_threshold,
                critical = refined.critical_threshold,
                "context budget refined for the run's serving model"
            );
        }
        refined
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::FallbackProviderToml;
    use crate::config::Config;
    use crate::orchestrator::deps_builder::common::cfg_with_fallback;
    use crate::{ContextBudgetToml, ModelThresholdToml, ProviderConfig};

    fn cfg_with_context_budget(cb: Option<ContextBudgetToml>) -> Config {
        Config {
            context_budget: cb,
            ..Config::default()
        }
    }

    #[test]
    fn context_budget_none_when_section_missing() {
        let cfg = Config::default();
        assert!(build_context_budget_config(&cfg, "primary").is_none());
    }

    #[test]
    fn context_budget_none_when_disabled() {
        let cfg = cfg_with_context_budget(Some(ContextBudgetToml {
            enabled: false,
            token_budget: Some(128_000),
            ..ContextBudgetToml::default()
        }));
        assert!(build_context_budget_config(&cfg, "primary").is_none());
    }

    #[test]
    fn context_budget_some_uses_defaults_when_fields_unset() {
        // No explicit token_budget and no known primary provider/model →
        // derived from the default window minus the default output reserve.
        let cfg = cfg_with_context_budget(Some(ContextBudgetToml {
            enabled: true,
            ..ContextBudgetToml::default()
        }));
        let bc = build_context_budget_config(&cfg, "primary").expect("enabled → Some");
        let usable = DEFAULT_CONTEXT_TOKEN_BUDGET - DEFAULT_OUTPUT_RESERVE;
        assert_eq!(bc.token_budget, usable);
        // Critical keeps the flat default; warning is window-aware. The default
        // 200k window is narrow enough that the 48k spike band pulls the warning
        // line below 0.70 (so one big tool result can't overshoot critical).
        assert_eq!(bc.critical_threshold, DEFAULT_CRITICAL_THRESHOLD);
        assert_eq!(
            bc.warning_threshold,
            window_aware_warning_default(usable, DEFAULT_CRITICAL_THRESHOLD)
        );
        assert!(
            bc.warning_threshold < DEFAULT_WARNING_THRESHOLD,
            "a 200k window must compact earlier than the flat 0.70 default, got {}",
            bc.warning_threshold
        );
    }

    #[test]
    fn context_budget_some_honours_explicit_values() {
        let cfg = cfg_with_context_budget(Some(ContextBudgetToml {
            enabled: true,
            token_budget: Some(64_000),
            warning_threshold: Some(0.6),
            critical_threshold: Some(0.9),
            ..ContextBudgetToml::default()
        }));
        let bc = build_context_budget_config(&cfg, "primary").expect("enabled → Some");
        assert_eq!(bc.token_budget, 64_000);
        assert_eq!(bc.warning_threshold, 0.6);
        assert_eq!(bc.critical_threshold, 0.9);
    }

    #[test]
    fn context_budget_none_when_thresholds_inverted() {
        // warning >= critical makes the CompactAndContinue branch unreachable;
        // every warning-pressure turn escalates straight to FinalReply,
        // silently cutting off every run. Reject rather than degrade.
        let cfg = cfg_with_context_budget(Some(ContextBudgetToml {
            enabled: true,
            warning_threshold: Some(0.9),
            critical_threshold: Some(0.7),
            ..ContextBudgetToml::default()
        }));
        assert!(build_context_budget_config(&cfg, "primary").is_none());
    }

    #[test]
    fn context_budget_none_when_token_budget_zero() {
        let cfg = cfg_with_context_budget(Some(ContextBudgetToml {
            enabled: true,
            token_budget: Some(0),
            ..ContextBudgetToml::default()
        }));
        assert!(build_context_budget_config(&cfg, "primary").is_none());
    }

    #[test]
    fn context_budget_none_when_threshold_out_of_range() {
        let cfg = cfg_with_context_budget(Some(ContextBudgetToml {
            enabled: true,
            warning_threshold: Some(1.5),
            ..ContextBudgetToml::default()
        }));
        assert!(build_context_budget_config(&cfg, "primary").is_none());
    }

    // ── model-aware budget derivation ────────────────────────────────────

    fn cfg_with_primary(key: &str, pc: ProviderConfig, cb: ContextBudgetToml) -> Config {
        let mut providers: std::collections::HashMap<String, ProviderConfig> =
            std::collections::HashMap::new();
        providers.insert(key.to_string(), pc);
        Config {
            context_budget: Some(cb),
            providers,
            ..Config::default()
        }
    }

    #[test]
    fn derive_prefers_provider_declared_window_and_reserve() {
        let mut pc = ProviderConfig::test_config("claude-sonnet-4-6");
        pc.context_window = Some(1_000_000);
        pc.max_tokens = Some(64_000);
        let d = derive_token_budget(Some(&pc), Some("claude-sonnet-4-6"));
        assert_eq!(d.window, 1_000_000);
        assert_eq!(d.reserve, 64_000);
        assert_eq!(d.usable, 936_000);
        assert_eq!(d.source, "config");
    }

    #[test]
    fn derive_falls_back_to_catalog_when_unset() {
        // Kimi K2 is in the catalog (256k window, 32_768 max output) but the
        // provider declares neither field.
        let pc = ProviderConfig::test_config("kimi-k2");
        let d = derive_token_budget(Some(&pc), Some("kimi-k2"));
        assert_eq!(d.window, 262_144);
        assert_eq!(d.reserve, 32_768);
        assert_eq!(d.usable, 262_144 - 32_768);
        assert_eq!(d.source, "catalog");
    }

    #[test]
    fn derive_defaults_when_model_unknown() {
        let pc = ProviderConfig::test_config("totally-unknown-model");
        let d = derive_token_budget(Some(&pc), Some("totally-unknown-model"));
        assert_eq!(d.window, DEFAULT_CONTEXT_TOKEN_BUDGET);
        assert_eq!(d.reserve, DEFAULT_OUTPUT_RESERVE);
        assert_eq!(d.source, "default");
    }

    #[test]
    fn derive_floors_tiny_or_inverted_window() {
        // window < reserve must not underflow/collapse the budget.
        let mut pc = ProviderConfig::test_config("x");
        pc.context_window = Some(4_000);
        pc.max_tokens = Some(8_000);
        let d = derive_token_budget(Some(&pc), Some("x"));
        assert_eq!(d.usable, MIN_USABLE_BUDGET);
    }

    #[test]
    fn build_derives_budget_from_catalog_model() {
        let cb = ContextBudgetToml {
            enabled: true,
            ..ContextBudgetToml::default()
        };
        let cfg = cfg_with_primary("kimi", ProviderConfig::test_config("kimi-k2"), cb);
        let bc = build_context_budget_config(&cfg, "kimi").expect("enabled → Some");
        assert_eq!(bc.token_budget, 262_144 - 32_768);
    }

    #[test]
    fn build_derives_budget_from_declared_window() {
        let mut pc = ProviderConfig::test_config("claude-sonnet-4-6");
        pc.context_window = Some(1_000_000);
        pc.max_tokens = Some(64_000);
        let cb = ContextBudgetToml {
            enabled: true,
            ..ContextBudgetToml::default()
        };
        let cfg = cfg_with_primary("c", pc, cb);
        let bc = build_context_budget_config(&cfg, "c").expect("enabled → Some");
        assert_eq!(bc.token_budget, 936_000);
    }

    #[test]
    fn build_explicit_token_budget_overrides_model_derivation() {
        // An explicit token_budget wins even when the model would derive a
        // much larger window — operator override, back-compat.
        let mut pc = ProviderConfig::test_config("claude-sonnet-4-6");
        pc.context_window = Some(1_000_000);
        let cb = ContextBudgetToml {
            enabled: true,
            token_budget: Some(50_000),
            ..ContextBudgetToml::default()
        };
        let cfg = cfg_with_primary("c", pc, cb);
        let bc = build_context_budget_config(&cfg, "c").expect("enabled → Some");
        assert_eq!(bc.token_budget, 50_000);
    }

    // ── per-model threshold overrides (G4) ───────────────────────────────

    #[test]
    fn build_applies_per_model_threshold_override() {
        // The resolved chain-min model id is "kimi-k2"; a "kimi" override must
        // tighten the trigger points away from the global 0.70/0.85 defaults.
        let cb = ContextBudgetToml {
            enabled: true,
            model_thresholds: vec![ModelThresholdToml {
                model: "kimi".to_string(),
                warning_threshold: Some(0.60),
                critical_threshold: Some(0.78),
            }],
            ..ContextBudgetToml::default()
        };
        let cfg = cfg_with_primary("moonshot", ProviderConfig::test_config("kimi-k2"), cb);
        let bc = build_context_budget_config(&cfg, "moonshot").expect("enabled → Some");
        assert_eq!(bc.warning_threshold, 0.60);
        assert_eq!(bc.critical_threshold, 0.78);
        // Budget itself still derives from the model window (override is thresholds-only).
        assert_eq!(bc.token_budget, 262_144 - 32_768);
    }

    #[test]
    fn build_per_model_override_falls_back_per_field_to_global() {
        // Override sets only `warning_threshold`; `critical_threshold` must fall
        // back to the top-level global (0.80 here), not the built-in 0.85.
        let cb = ContextBudgetToml {
            enabled: true,
            warning_threshold: Some(0.72),
            critical_threshold: Some(0.80),
            model_thresholds: vec![ModelThresholdToml {
                model: "kimi".to_string(),
                warning_threshold: Some(0.60),
                critical_threshold: None,
            }],
            ..ContextBudgetToml::default()
        };
        let cfg = cfg_with_primary("moonshot", ProviderConfig::test_config("kimi-k2"), cb);
        let bc = build_context_budget_config(&cfg, "moonshot").expect("enabled → Some");
        assert_eq!(
            bc.warning_threshold, 0.60,
            "override wins for the set field"
        );
        assert_eq!(
            bc.critical_threshold, 0.80,
            "unset override field inherits the global, not the built-in default"
        );
    }

    #[test]
    fn build_non_matching_override_is_byte_identical_to_global() {
        // A "claude" override must NOT touch a kimi run — behaviour stays the
        // global default, proving overrides are additive/back-compat.
        let cb = ContextBudgetToml {
            enabled: true,
            model_thresholds: vec![ModelThresholdToml {
                model: "claude".to_string(),
                warning_threshold: Some(0.50),
                critical_threshold: Some(0.60),
            }],
            ..ContextBudgetToml::default()
        };
        let cfg = cfg_with_primary("moonshot", ProviderConfig::test_config("kimi-k2"), cb);
        let bc = build_context_budget_config(&cfg, "moonshot").expect("enabled → Some");
        // No matching override → the *global default*, which for warning is now
        // window-aware. kimi-k2's usable window is narrow enough that the auto
        // warning sits below 0.70; critical still uses the flat default.
        let usable = 262_144 - 32_768;
        assert_eq!(
            bc.warning_threshold,
            window_aware_warning_default(usable, DEFAULT_CRITICAL_THRESHOLD)
        );
        assert_eq!(bc.critical_threshold, DEFAULT_CRITICAL_THRESHOLD);
        assert!(bc.warning_threshold < DEFAULT_WARNING_THRESHOLD);
    }

    #[test]
    fn build_per_model_override_applies_with_explicit_token_budget() {
        // Even when the operator pins token_budget, the model is still resolved
        // so a per-model threshold override still applies (matched on provider key).
        let cb = ContextBudgetToml {
            enabled: true,
            token_budget: Some(50_000),
            model_thresholds: vec![ModelThresholdToml {
                model: "moonshot".to_string(),
                warning_threshold: Some(0.55),
                ..ModelThresholdToml::default()
            }],
            ..ContextBudgetToml::default()
        };
        let cfg = cfg_with_primary("moonshot", ProviderConfig::test_config("kimi-k2"), cb);
        let bc = build_context_budget_config(&cfg, "moonshot").expect("enabled → Some");
        assert_eq!(bc.token_budget, 50_000);
        assert_eq!(bc.warning_threshold, 0.55);
    }

    #[test]
    fn build_rejects_override_that_inverts_thresholds() {
        // A per-model override that produces warning >= critical must trip the
        // same defensive gate as a bad global config: disable rather than degrade.
        let cb = ContextBudgetToml {
            enabled: true,
            model_thresholds: vec![ModelThresholdToml {
                model: "kimi".to_string(),
                warning_threshold: Some(0.90),
                critical_threshold: Some(0.70),
            }],
            ..ContextBudgetToml::default()
        };
        let cfg = cfg_with_primary("moonshot", ProviderConfig::test_config("kimi-k2"), cb);
        assert!(
            build_context_budget_config(&cfg, "moonshot").is_none(),
            "inverted per-model thresholds disable the budget (P7 defensive)"
        );
    }

    // ── window-aware default compaction timing (kimi 20w vs claude 100w) ─

    #[test]
    fn window_aware_warning_wide_window_keeps_flat_default() {
        // A 1M-class usable window absorbs a 48k spike at 0.70, so the auto
        // warning is exactly the historical flat default — wide models stay
        // byte-identical to the pre-wiring behaviour.
        let w = window_aware_warning_default(936_000, DEFAULT_CRITICAL_THRESHOLD);
        assert_eq!(w, DEFAULT_WARNING_THRESHOLD);
    }

    #[test]
    fn window_aware_warning_narrow_window_compacts_earlier() {
        // A 200k-class window cannot absorb a 48k spike at 0.70, so the auto
        // warning drops below it — keeping the absolute 48k band below critical.
        let usable = 262_144u64 - 32_768; // kimi-k2 usable
        let w = window_aware_warning_default(usable, DEFAULT_CRITICAL_THRESHOLD);
        assert!(w < DEFAULT_WARNING_THRESHOLD);
        let band_tokens = (DEFAULT_CRITICAL_THRESHOLD - w) * usable as f64;
        assert!(
            (band_tokens - WARNING_SPIKE_HEADROOM_TOKENS).abs() < 1.0,
            "warning→critical band must equal one spike (~48k), got {band_tokens}"
        );
    }

    #[test]
    fn window_aware_warning_floored_for_tiny_window() {
        // When the spike exceeds the whole band, the auto warning floors at the
        // minimum — never negative, never absurdly low.
        let w = window_aware_warning_default(MIN_USABLE_BUDGET, DEFAULT_CRITICAL_THRESHOLD);
        assert_eq!(w, MIN_AUTO_WARNING_THRESHOLD);
    }

    #[test]
    fn window_aware_warning_tracks_effective_critical() {
        // The band is measured below the *effective* critical, so a higher
        // configured critical lifts the auto warning with it.
        let usable = 262_144u64 - 32_768;
        assert!(
            window_aware_warning_default(usable, 0.90) > window_aware_warning_default(usable, 0.85)
        );
    }

    #[test]
    fn fresh_tail_anchors_at_base_for_default_window() {
        // Back-compat: the historical 200k default window (usable = 200k − reserve,
        // just under the anchor) keeps exactly the legacy base count of 6.
        let usable = DEFAULT_CONTEXT_TOKEN_BUDGET - DEFAULT_OUTPUT_RESERVE;
        assert_eq!(window_aware_fresh_tail(usable), FRESH_TAIL_BASE_COUNT);
        // The anchor itself (and anything below it) also floors to the base.
        assert_eq!(
            window_aware_fresh_tail(FRESH_TAIL_ANCHOR_BUDGET),
            FRESH_TAIL_BASE_COUNT
        );
        assert_eq!(
            window_aware_fresh_tail(MIN_USABLE_BUDGET),
            FRESH_TAIL_BASE_COUNT
        );
    }

    #[test]
    fn fresh_tail_grows_with_window() {
        // A 1M-class window keeps strictly more recent context verbatim than the
        // 200k default — the model-aware retention win. ~792k extra usable over
        // the anchor / 120k ≈ 6 extra → 12.
        let wide = window_aware_fresh_tail(1_000_000 - 8_192);
        assert!(
            wide > FRESH_TAIL_BASE_COUNT,
            "a 1M window must keep more than the base {FRESH_TAIL_BASE_COUNT}, got {wide}"
        );
        assert_eq!(wide, 12, "1M usable → base 6 + 6 extra");
    }

    #[test]
    fn fresh_tail_is_monotonic_and_capped() {
        // Retention never shrinks as the window grows, and never exceeds the cap
        // so the protected tail cannot dominate the compaction window.
        let a = window_aware_fresh_tail(300_000);
        let b = window_aware_fresh_tail(900_000);
        assert!(b >= a);
        assert_eq!(window_aware_fresh_tail(u64::MAX), FRESH_TAIL_MAX_COUNT);
    }

    #[test]
    fn narrow_model_compacts_earlier_than_wide_by_default() {
        // Headline property: with NO explicit thresholds, a narrow kimi window
        // starts compacting at a lower fraction than a wide claude window —
        // model-aware compaction timing without any per-model config.
        let cb = || ContextBudgetToml {
            enabled: true,
            ..ContextBudgetToml::default()
        };
        let mut claude = ProviderConfig::test_config("claude-sonnet-4-6");
        claude.context_window = Some(1_000_000);
        claude.max_tokens = Some(64_000);
        let claude_cfg = cfg_with_primary("claude", claude, cb());
        let claude_bc = build_context_budget_config(&claude_cfg, "claude").expect("some");

        let kimi_cfg = cfg_with_primary("moonshot", ProviderConfig::test_config("kimi-k2"), cb());
        let kimi_bc = build_context_budget_config(&kimi_cfg, "moonshot").expect("some");

        assert_eq!(
            claude_bc.warning_threshold, DEFAULT_WARNING_THRESHOLD,
            "a 1M window keeps the flat 0.70 default"
        );
        assert!(
            kimi_bc.warning_threshold < claude_bc.warning_threshold,
            "narrow kimi compacts earlier than wide claude: kimi {} < claude {}",
            kimi_bc.warning_threshold,
            claude_bc.warning_threshold
        );
        // Both keep the flat critical hard-stop — only the *trigger* is window-aware.
        assert_eq!(kimi_bc.critical_threshold, DEFAULT_CRITICAL_THRESHOLD);
        assert_eq!(claude_bc.critical_threshold, DEFAULT_CRITICAL_THRESHOLD);
    }

    // ── min-over-chain budgeting (failover-safe window) ──────────────────

    /// 1M-window primary, a declared `ContextBudgetToml`, and a fallback. Sets
    /// `context_budget` on top of the `cfg_with_fallback` skeleton.
    fn cfg_chain_budget(
        primary: (&str, ProviderConfig),
        fb: Option<FallbackProviderToml>,
        others: Vec<(&str, ProviderConfig)>,
    ) -> Config {
        let mut providers = vec![primary];
        providers.extend(others);
        let mut cfg = cfg_with_fallback(fb, providers);
        cfg.context_budget = Some(ContextBudgetToml {
            enabled: true,
            ..ContextBudgetToml::default()
        });
        cfg
    }

    fn big_primary() -> ProviderConfig {
        let mut pc = ProviderConfig::test_config("claude-sonnet-4-6");
        pc.context_window = Some(1_000_000);
        pc.max_tokens = Some(64_000);
        pc
    }

    #[test]
    fn chain_min_budget_picks_smallest_window_in_explicit_chain() {
        // A 1M primary that can migrate (rate-limit) to a 256k kimi must budget
        // for kimi's window, not its own — else the migrated turn overflows.
        let fb = FallbackProviderToml {
            chain: vec!["small".to_string()],
            provider: None,
            max_retries: None,
        };
        let cfg = cfg_chain_budget(
            ("big", big_primary()),
            Some(fb),
            vec![("small", ProviderConfig::test_config("kimi-k2"))],
        );
        let bc = build_context_budget_config(&cfg, "big").expect("enabled → Some");
        assert_eq!(bc.token_budget, 262_144 - 32_768);
    }

    #[test]
    fn chain_min_budget_auto_derive_spans_all_enabled_providers() {
        // No explicit chain → failover auto-derives from every enabled
        // provider, so the budget must span them and pick the smallest window.
        let cfg = cfg_chain_budget(
            ("big", big_primary()),
            None,
            vec![("small", ProviderConfig::test_config("kimi-k2"))],
        );
        let bc = build_context_budget_config(&cfg, "big").expect("enabled → Some");
        assert_eq!(bc.token_budget, 262_144 - 32_768);
    }

    #[test]
    fn chain_min_budget_single_provider_uses_primary_window() {
        // Only the primary exists → min-over-chain == primary budget (the
        // build-time-by-primary back-compat path is preserved).
        let cfg = cfg_chain_budget(("big", big_primary()), None, vec![]);
        let bc = build_context_budget_config(&cfg, "big").expect("enabled → Some");
        assert_eq!(bc.token_budget, 1_000_000 - 64_000);
    }

    #[test]
    fn chain_min_budget_ignores_disabled_fallback() {
        // A disabled sibling can never be migrated into, so it must not drag
        // the budget down to its (smaller) window.
        let mut disabled = ProviderConfig::test_config("kimi-k2");
        disabled.enabled = false;
        let fb = FallbackProviderToml {
            chain: vec!["small".to_string()],
            provider: None,
            max_retries: None,
        };
        let cfg = cfg_chain_budget(("big", big_primary()), Some(fb), vec![("small", disabled)]);
        let bc = build_context_budget_config(&cfg, "big").expect("enabled → Some");
        assert_eq!(bc.token_budget, 1_000_000 - 64_000);
    }

    // --- chain-min undercut advisory (observability predicate) ---

    #[test]
    fn undercut_fires_when_chain_min_well_below_primary() {
        // A 200k-usable fallback sibling against a 1M-usable primary is far
        // under the 60% line → the primary will compact early → advise.
        assert!(chain_min_materially_undercuts_primary(200_000, 1_000_000));
    }

    #[test]
    fn undercut_silent_when_chain_min_close_to_primary() {
        // A sibling within the 60% band is not surprising enough to warn about:
        // 700k of a 1M primary keeps most of the window.
        assert!(!chain_min_materially_undercuts_primary(700_000, 1_000_000));
    }

    #[test]
    fn undercut_silent_when_chain_min_equals_primary() {
        // Single-provider / uniform-window chain: chain-min IS the primary, so
        // there is nothing to advise.
        assert!(!chain_min_materially_undercuts_primary(
            1_000_000, 1_000_000
        ));
    }

    #[test]
    fn undercut_silent_when_primary_unknown() {
        // An undeterminable primary budget (0) must never produce a spurious
        // warning — the predicate guards the division-by-intent.
        assert!(!chain_min_materially_undercuts_primary(0, 0));
        assert!(!chain_min_materially_undercuts_primary(50_000, 0));
    }

    // ── per-run serving-model refinement (G1) ────────────────────────────

    /// Field-wise equality helper: `ContextBudgetConfig` does not derive
    /// `PartialEq`, and these tests care about every decision-relevant field.
    fn assert_cfg_eq(a: &ContextBudgetConfig, b: &ContextBudgetConfig) {
        assert_eq!(a.token_budget, b.token_budget, "token_budget");
        assert_eq!(a.warning_threshold, b.warning_threshold, "warning");
        assert_eq!(a.critical_threshold, b.critical_threshold, "critical");
        assert_eq!(a.token_estimate_ratio, b.token_estimate_ratio, "ratio");
        assert_eq!(a.fresh_tail_count, b.fresh_tail_count, "fresh_tail");
        assert_eq!(a.circuit_breaker_max, b.circuit_breaker_max, "breaker");
        assert_eq!(a.max_splits, b.max_splits, "max_splits");
    }

    #[test]
    fn refiner_none_when_section_missing_or_disabled() {
        let cfg = Config::default();
        assert!(build_context_budget_refiner(&cfg, "primary").is_none());
        let cfg = cfg_with_context_budget(Some(ContextBudgetToml {
            enabled: false,
            ..ContextBudgetToml::default()
        }));
        assert!(build_context_budget_refiner(&cfg, "primary").is_none());
    }

    #[test]
    fn refine_is_byte_identical_when_serving_model_is_the_budget_model() {
        // Common case: single provider, run served by the primary's own
        // (catalog-known) model — refinement must reproduce the startup
        // config exactly, field for field.
        let cb = ContextBudgetToml {
            enabled: true,
            ..ContextBudgetToml::default()
        };
        let cfg = cfg_with_primary("moonshot", ProviderConfig::test_config("kimi-k2"), cb);
        let base = build_context_budget_config(&cfg, "moonshot").expect("some");
        let refiner = build_context_budget_refiner(&cfg, "moonshot").expect("some");
        let refined = refiner.refine_for_serving_model(&base, "kimi-k2", "moonshot", None);
        assert_cfg_eq(&refined, &base);
    }

    #[test]
    fn refine_shrinks_budget_for_narrower_serving_model() {
        // A 1M-window primary whose run got pinned (select_model / hint) to a
        // 256k model: the budget must drop to the serving model's usable
        // window, with thresholds and fresh-tail re-derived — otherwise the
        // run compacts against a budget its real window cannot honour.
        let cb = ContextBudgetToml {
            enabled: true,
            ..ContextBudgetToml::default()
        };
        let cfg = cfg_with_primary("claude", big_primary(), cb);
        let base = build_context_budget_config(&cfg, "claude").expect("some");
        assert_eq!(base.token_budget, 936_000);
        let refiner = build_context_budget_refiner(&cfg, "claude").expect("some");
        let refined = refiner.refine_for_serving_model(&base, "kimi-k2", "moonshot", None);
        let kimi_usable = 262_144 - 32_768;
        assert_eq!(refined.token_budget, kimi_usable);
        assert_eq!(
            refined.warning_threshold,
            window_aware_warning_default(kimi_usable, DEFAULT_CRITICAL_THRESHOLD)
        );
        assert!(refined.warning_threshold < base.warning_threshold);
        assert_eq!(
            refined.fresh_tail_count,
            window_aware_fresh_tail(kimi_usable)
        );
        assert!(refined.fresh_tail_count < base.fresh_tail_count);
    }

    #[test]
    fn refine_never_relaxes_budget_above_chain_min() {
        // Serving model WIDER than the chain minimum: the budget stays at the
        // chain-minimum floor — refinement can only compact earlier, never
        // later, so in-request failover safety is preserved.
        let fb = FallbackProviderToml {
            chain: vec!["small".to_string()],
            provider: None,
            max_retries: None,
        };
        let cfg = cfg_chain_budget(
            ("big", big_primary()),
            Some(fb),
            vec![("small", ProviderConfig::test_config("kimi-k2"))],
        );
        let base = build_context_budget_config(&cfg, "big").expect("some");
        assert_eq!(base.token_budget, 262_144 - 32_768, "chain-min is kimi");
        let refiner = build_context_budget_refiner(&cfg, "big").expect("some");
        // Run served by the 1M primary model; the runner passes the
        // provider's configured window override, exactly as production does.
        let refined =
            refiner.refine_for_serving_model(&base, "claude-sonnet-4-6", "big", Some(1_000_000));
        assert_cfg_eq(&refined, &base);
    }

    #[test]
    fn refine_honours_explicit_token_budget_verbatim() {
        // Operator-pinned budget wins even against a much narrower serving
        // model — the same back-compat contract as the startup path.
        let cb = ContextBudgetToml {
            enabled: true,
            token_budget: Some(500_000),
            ..ContextBudgetToml::default()
        };
        let cfg = cfg_with_primary("claude", big_primary(), cb);
        let base = build_context_budget_config(&cfg, "claude").expect("some");
        let refiner = build_context_budget_refiner(&cfg, "claude").expect("some");
        let refined = refiner.refine_for_serving_model(&base, "kimi-k2", "moonshot", None);
        assert_eq!(refined.token_budget, 500_000);
    }

    #[test]
    fn refine_rekeys_threshold_override_to_serving_model() {
        // The startup fold keys overrides off the chain-min model; a run
        // actually served by a DIFFERENT model must match ITS override.
        let cb = ContextBudgetToml {
            enabled: true,
            model_thresholds: vec![ModelThresholdToml {
                model: "kimi".to_string(),
                warning_threshold: Some(0.60),
                critical_threshold: Some(0.78),
            }],
            ..ContextBudgetToml::default()
        };
        let cfg = cfg_with_primary("claude", big_primary(), cb);
        let base = build_context_budget_config(&cfg, "claude").expect("some");
        // "kimi" does not match claude-sonnet → base keeps the flat defaults.
        assert_eq!(base.critical_threshold, DEFAULT_CRITICAL_THRESHOLD);
        let refiner = build_context_budget_refiner(&cfg, "claude").expect("some");
        let refined = refiner.refine_for_serving_model(&base, "kimi-k2", "moonshot", None);
        assert_eq!(refined.warning_threshold, 0.60);
        assert_eq!(refined.critical_threshold, 0.78);
    }

    #[test]
    fn refine_unknown_serving_model_without_window_override_returns_base() {
        // An unidentifiable serving model (catalog miss, no configured window)
        // must NOT drag the budget down to the conservative catalog fallback —
        // refinement only acts on trustworthy window data.
        let cb = ContextBudgetToml {
            enabled: true,
            ..ContextBudgetToml::default()
        };
        let cfg = cfg_with_primary("claude", big_primary(), cb);
        let base = build_context_budget_config(&cfg, "claude").expect("some");
        let refiner = build_context_budget_refiner(&cfg, "claude").expect("some");
        let refined =
            refiner.refine_for_serving_model(&base, "totally-unknown-model", "claude", None);
        assert_cfg_eq(&refined, &base);
    }

    #[test]
    fn refine_uses_configured_window_override_for_unknown_serving_model() {
        // With `[providers.*] context_window` set, even a catalog-unknown
        // serving model refines against the declared window (mirrors
        // `derive_token_budget`'s config-first precedence).
        let mut pc = ProviderConfig::test_config("my-custom-model");
        pc.context_window = Some(64_000);
        let cb = ContextBudgetToml {
            enabled: true,
            ..ContextBudgetToml::default()
        };
        let cfg = cfg_with_primary("local", pc, cb);
        let base = build_context_budget_config(&cfg, "local").expect("some");
        let refiner = build_context_budget_refiner(&cfg, "local").expect("some");
        let refined =
            refiner.refine_for_serving_model(&base, "my-custom-model", "local", Some(64_000));
        assert_eq!(refined.token_budget, 64_000 - DEFAULT_OUTPUT_RESERVE);
        assert_cfg_eq(&refined, &base);
    }

    #[test]
    fn refine_inverted_serving_override_falls_back_to_base() {
        // A per-model override that inverts the thresholds (warning >=
        // critical) must not kill the run mid-flight: degrade to the
        // startup-validated base instead of disabling the budget.
        let cb = ContextBudgetToml {
            enabled: true,
            model_thresholds: vec![ModelThresholdToml {
                model: "kimi".to_string(),
                warning_threshold: Some(0.90),
                critical_threshold: Some(0.70),
            }],
            ..ContextBudgetToml::default()
        };
        let cfg = cfg_with_primary("claude", big_primary(), cb);
        let base = build_context_budget_config(&cfg, "claude").expect("some");
        let refiner = build_context_budget_refiner(&cfg, "claude").expect("some");
        let refined = refiner.refine_for_serving_model(&base, "kimi-k2", "moonshot", None);
        assert_cfg_eq(&refined, &base);
    }
}
