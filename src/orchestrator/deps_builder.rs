//! Shared `HarnessDeps` builder functions.
//!
//! Used by both the main runner (`aleph-server` bin's `orchestrator_init.rs`)
//! and the subagent spawner (`agents::subagent_spawner`) to assemble
//! `HarnessDeps` fields consistently. Subagents inherit identical config; no
//! override params are accepted (per P1 zero-override decision).

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::time::Duration;

use crate::config::types::ProviderConfig;
use crate::config::Config;
use crate::context::budget::ContextBudgetConfig;
use crate::harness::StallConfig;
use crate::providers::model_catalog::{capabilities_for, endpoint_kind_for_base_url};
use crate::providers::route_observe::{ChainCandidate, RouteObservability};
use crate::providers::route_policy::EndpointTier;
use crate::providers::{
    create_provider, AiProvider, DefaultProviderHandle, FailoverConfig, FailoverHealth,
    FailoverNode, FailoverProvider, LoadStats, ModelCooldown, ProviderCooldown, StaticDefault,
};
use crate::sandbox::exec_approval::gate::ApprovalRequester;

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

/// Stability triple — three independent Optionals derived from `[stability]`.
///
/// Returned as a struct (not tuple) so consumers can name fields and future
/// additions don't break callers.
pub struct StabilityTriple {
    pub stall_config: Option<StallConfig>,
    pub consecutive_failure_cap: Option<usize>,
    pub turn_timeout: Option<Duration>,
}

/// Sentinel name for the fallback node that wraps the whole global chain
/// inside a per-`provider_hint` override. Cannot collide with a real provider
/// (names come from `[providers]` toml keys), so `FailoverProvider`'s
/// primary-vs-fallback dedup never drops it.
const GLOBAL_CHAIN_NODE: &str = "__global_chain__";

/// Endpoint tier for one configured provider, used by the route policy.
///
/// Special-cases `protocol == "ollama"` with no `base_url` → [`Local`]
/// ([`OllamaProvider`](crate::providers::OllamaProvider) defaults to
/// `http://localhost:11434`). Otherwise defers to
/// [`endpoint_kind_for_base_url`], which keys on the `base_url` host so
/// `ollama.com` (Cloud) and a local Ollama (Local) are distinguished.
///
/// [`Local`]: EndpointTier::Local
pub(crate) fn provider_tier(pc: &ProviderConfig) -> EndpointTier {
    let is_native_ollama = pc
        .protocol
        .as_deref()
        .is_some_and(|p| p.eq_ignore_ascii_case("ollama"));
    if is_native_ollama && pc.base_url.as_deref().is_none_or(str::is_empty) {
        return EndpointTier::Local;
    }
    endpoint_kind_for_base_url(pc.base_url.as_deref()).into()
}

/// Provider routing assembled once at boot.
///
/// `default` is the global failover chain — wired as `deps.llm` for the main
/// harness and used as the subagent provider for agents without a
/// `provider_hint`. `agent_overrides` maps a `provider_hint` to a
/// `FailoverProvider` that pins that provider, then falls through the entire
/// global chain (the "pin + fall-through" semantics).
#[derive(Clone)]
pub struct ProviderChain {
    pub default: Arc<dyn DefaultProviderHandle>,
    pub agent_overrides: HashMap<String, Arc<dyn AiProvider>>,
    /// Live handles onto the chain's shared runtime state (breaker, cooldowns,
    /// load) plus the boot-time chain composition. The production boot path
    /// registers a clone as the process-global `route_observe` bundle so the
    /// `self_config` `route_status` action can render live diagnostics.
    pub observability: RouteObservability,
}

/// Build the provider routing: the global failover chain plus the
/// per-`provider_hint` override registry.
///
/// The global chain wraps `default_provider` in a [`FailoverProvider`] that
/// walks `[primary, ...[fallback_provider].chain]`. The primary slot stays
/// *live*: the `FailoverProvider` reads `default_provider.current()` on every
/// call, so a UI `set_default` swap takes effect on the next turn (hot-reload
/// preserved). Every configured provider's `models` list feeds the catalog,
/// so the chain can also fail over *between models* of one provider.
///
/// Each `agent_overrides` entry pins one configured provider as the primary
/// and adds a single fallback node = the whole global chain. All chains share
/// one [`FailoverHealth`], so one provider's outage is visible everywhere.
/// The primary provider itself gets no override entry — hinting it is
/// equivalent to not hinting at all.
///
/// A provider that fails to construct, and a chain entry that matches the
/// primary or is absent from `[providers]`, are skipped with a warn log.
pub fn build_failover_chain(
    config: &Config,
    primary_provider_key: &str,
    default_provider: Arc<dyn DefaultProviderHandle>,
    escalation_approval: Option<Arc<dyn ApprovalRequester>>,
    route_handle: Option<Arc<crate::providers::route_handle::RouteHandle>>,
) -> ProviderChain {
    // Local/cloud route preference (`[route]`). `Auto` (default) is a no-op,
    // so an unconfigured deployment is byte-identical to pre-route failover.
    let route_mode = config.route.mode;
    let allow_escalation = config.route.allow_cloud_escalation;
    // Catalog of every configured provider's model list, keyed by toml name.
    // Lets the live primary — and each fallback — fail over across models.
    let model_catalog: HashMap<String, Vec<String>> = config
        .providers
        .iter()
        .map(|(name, pc)| (name.clone(), pc.all_models().to_vec()))
        .collect();

    // Shared circuit-breaker health: the global chain and every per-hint
    // override see the same provider-outage picture.
    let health = FailoverHealth::default();

    // Shared per-model rate-limit cooldown: a model-specific 429 sidelines that
    // one model (not the whole provider) across every chain built here.
    let model_cooldown = ModelCooldown::default();

    // Shared per-provider rate-limit cooldown gate: after a provider 429s, the
    // next turn paces itself (waits out the recorded window) before re-dialing
    // it, instead of eating a fresh 429 and bouncing to a fallback — keeps a
    // single paid primary (e.g. Kimi) in use. Scoped exactly like `health`.
    let provider_cooldown = ProviderCooldown::default();

    // Operator-tunable in-place retry budget (`[fallback_provider].max_retries`),
    // falling back to the built-in default. Useful for single-provider setups
    // with no sibling to fail over to.
    let failover_config = FailoverConfig {
        max_retries: config
            .fallback_provider
            .as_ref()
            .and_then(|fb| fb.max_retries)
            .unwrap_or_else(|| FailoverConfig::default().max_retries),
        ..FailoverConfig::default()
    };

    // Shared runtime load registry: in-flight counts and observed latencies are
    // visible to every chain that might dial a given endpoint, so the
    // load-balancing strategy sees one consistent picture (scoped exactly like
    // `health` — one registry per boot-time chain assembly).
    let load = Arc::new(LoadStats::new());

    // Build every non-primary provider once, reused by both the fallback
    // chain and the per-hint override registry (no double construction).
    let mut built: HashMap<String, Arc<dyn AiProvider>> = HashMap::new();
    for (name, pc) in &config.providers {
        if name.eq_ignore_ascii_case(primary_provider_key) {
            continue; // the primary is already built behind `default_provider`
        }
        match create_provider(name, pc.clone()) {
            Ok(provider) => {
                built.insert(name.clone(), provider);
            }
            Err(e) => tracing::warn!(
                provider = %name,
                error = %e,
                "provider build failed; skipping"
            ),
        }
    }

    // Ordered fallback chain. Primary source is `[fallback_provider].chain`;
    // when that resolves to nothing usable (missing section, typo'd entry, or a
    // provider that failed to build) the helper auto-derives a chain from every
    // other enabled provider — so a throttled primary always has somewhere to
    // migrate instead of hard-failing the run on a 429.
    let fallbacks = assemble_fallbacks(config, primary_provider_key, &built, &model_catalog);

    tracing::info!(
        primary = %primary_provider_key,
        fallback_count = fallbacks.len(),
        override_count = built.len(),
        "failover chain assembled"
    );

    // Capture the chain composition + shared-state handles for the read-only
    // observability bundle before `fallbacks` moves into the provider. Each
    // handle clone shares the same live `Arc` map the chains mutate, so a
    // later snapshot always reads the current breaker/cooldown/load picture.
    let observability = RouteObservability {
        primary: default_provider.clone(),
        fallbacks: fallbacks
            .iter()
            .map(|n| ChainCandidate {
                name: n.name.clone(),
                models: n.models.clone(),
                tier: n.tier,
            })
            .collect(),
        health: health.clone(),
        model_cooldown: model_cooldown.clone(),
        provider_cooldown: provider_cooldown.clone(),
        load: load.clone(),
        route: route_handle.clone(),
    };

    let global_provider = FailoverProvider::new(
        default_provider,
        fallbacks,
        model_catalog.clone(),
        health.clone(),
        failover_config.clone(),
    )
    .with_route(route_mode, allow_escalation, escalation_approval.clone())
    .with_load_stats(load.clone())
    .with_model_cooldown(model_cooldown.clone())
    .with_provider_cooldown(provider_cooldown.clone());
    // A live handle (production) makes mode switches hot-apply; its absence
    // (tests) keeps the boot snapshot above — byte-identical to before.
    let global_provider = match route_handle.clone() {
        Some(h) => global_provider.with_route_live(h),
        None => global_provider,
    };
    let global: Arc<dyn AiProvider> = Arc::new(global_provider);
    let default: Arc<dyn DefaultProviderHandle> = Arc::new(StaticDefault::new(global.clone()));

    // Per-`provider_hint` overrides: one FailoverProvider per non-primary
    // provider, pinning it as primary then falling through the global chain.
    let agent_overrides: HashMap<String, Arc<dyn AiProvider>> = built
        .into_iter()
        .map(|(name, provider)| {
            // The pinned provider's real tier, so a hard-guardrail route mode
            // (`AlwaysLocal`) routes an explicit cloud pin through the
            // borrow-cloud approval instead of silently allowing it. Unknown
            // configured tier defaults to Cloud (the conservative, gated side).
            let pin_tier = config
                .providers
                .get(&name)
                .map_or(EndpointTier::Cloud, provider_tier);
            let pinned = FailoverProvider::new(
                Arc::new(StaticDefault::new(provider)),
                vec![FailoverNode {
                    name: GLOBAL_CHAIN_NODE.to_string(),
                    models: Vec::new(),
                    provider: global.clone(),
                    // The global-chain wrapper is itself route-shaped; tag it
                    // Unknown so the pinned chain's own route policy never drops
                    // it (the global chain already enforced the tier policy).
                    tier: EndpointTier::Unknown,
                }],
                model_catalog.clone(),
                health.clone(),
                failover_config.clone(),
            )
            .with_route(route_mode, allow_escalation, escalation_approval.clone())
            .with_primary_tier(pin_tier)
            .with_load_stats(load.clone())
            .with_model_cooldown(model_cooldown.clone())
            .with_provider_cooldown(provider_cooldown.clone());
            let pinned = match route_handle.clone() {
                Some(h) => pinned.with_route_live(h),
                None => pinned,
            };
            (name, Arc::new(pinned) as Arc<dyn AiProvider>)
        })
        .collect();

    ProviderChain {
        default,
        agent_overrides,
        observability,
    }
}

/// Assemble the ordered fallback chain for the global failover provider.
///
/// Source of truth is `[fallback_provider].chain` (with the back-compat
/// `provider` field folded in by [`FallbackProviderToml::resolved_chain`]),
/// resolved against the already-built non-primary provider set. Two hardening
/// rules close the gap that let a 429 hard-fail despite the provider-layer
/// retry/cooldown machinery — the failure was never "retry too shallow", it
/// was "the chain to migrate into is empty":
///
/// 1. A chain entry naming a provider absent from `[providers]` (a typo, or one
///    that failed to build) is logged at ERROR — loud enough to actually notice
///    in the boot log — then skipped, instead of the old silent WARN.
/// 2. If the resolved chain ends up empty but other enabled providers exist,
///    derive the chain from all of them (deterministic, name-sorted). A missing
///    or fully-invalid `[fallback_provider]` therefore no longer strands the
///    primary with nowhere to fail over to: model fallback works by default.
///
/// An explicit, non-empty chain is always honored verbatim — auto-derivation
/// only fills a vacuum, it never overrides operator intent.
fn assemble_fallbacks(
    config: &Config,
    primary_provider_key: &str,
    built: &HashMap<String, Arc<dyn AiProvider>>,
    model_catalog: &HashMap<String, Vec<String>>,
) -> Vec<FailoverNode> {
    // Build one node from a name + its already-constructed provider. Tier comes
    // from the provider's base_url host so the route policy can order/gate this
    // fallback by local-vs-cloud.
    let node_for = |name: &str, provider: &Arc<dyn AiProvider>| FailoverNode {
        name: name.to_string(),
        models: model_catalog.get(name).cloned().unwrap_or_default(),
        tier: config
            .providers
            .get(name)
            .map_or(EndpointTier::Cloud, provider_tier),
        provider: provider.clone(),
    };

    // Effective protocol of a provider ("anthropic" / "openai" / ...), used to
    // prefer *same-protocol* fallbacks. Cross-protocol failover (e.g. an
    // Anthropic-protocol primary like Kimi migrating to an OpenAI-compatible
    // endpoint) has to convert the request shape, and some OpenAI-compat Claude
    // shims reject the conversion outright (the -10003 "参数错误" gap). A
    // homogeneous chain sends the same format the primary already proved valid.
    let protocol_of = |name: &str| -> Option<String> {
        config
            .providers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, pc)| pc.protocol())
    };
    let primary_protocol = protocol_of(primary_provider_key);

    let mut fallbacks: Vec<FailoverNode> = Vec::new();
    if let Some(fb) = config.fallback_provider.as_ref() {
        for name in fb.resolved_chain() {
            if name.eq_ignore_ascii_case(primary_provider_key) {
                tracing::warn!(provider = %name, "failover chain: entry matches primary; skipping");
                continue;
            }
            let Some(provider) = built.get(&name) else {
                tracing::error!(
                    provider = %name,
                    "failover chain: '{name}' is not defined under [providers] (or failed to \
                     build) — fix the [fallback_provider].chain entry; skipping it",
                );
                continue;
            };
            // Loud diagnostic for a cross-protocol fallback: it still runs (the
            // operator configured it on purpose), but a protocol mismatch is the
            // root of the request-conversion rejection class, so make it visible.
            if let (Some(prim), Some(fbp)) = (&primary_protocol, protocol_of(&name)) {
                if !fbp.eq_ignore_ascii_case(prim) {
                    tracing::warn!(
                        primary_protocol = %prim,
                        fallback = %name,
                        fallback_protocol = %fbp,
                        "failover chain: fallback protocol differs from primary — cross-protocol \
                         failover converts the request shape and some OpenAI-compatible endpoints \
                         reject it (e.g. -10003); prefer a same-protocol fallback",
                    );
                }
            }
            fallbacks.push(node_for(&name, provider));
        }
    }

    // Self-heal: a primary with no usable configured fallback still gets one,
    // derived from every other enabled provider. Same-protocol providers are
    // preferred (listed first) so an auto-derived chain stays homogeneous when
    // possible; name-sort breaks ties for determinism.
    if fallbacks.is_empty() && !built.is_empty() {
        let mut names: Vec<&String> = built.keys().collect();
        names.sort();
        if let Some(prim) = &primary_protocol {
            // Stable sort: within each protocol group the name order is kept,
            // and same-as-primary (`false`) sorts before different (`true`).
            names.sort_by_key(|n| !protocol_of(n).is_some_and(|p| p.eq_ignore_ascii_case(prim)));
        }
        for name in names {
            if let Some(provider) = built.get(name) {
                fallbacks.push(node_for(name, provider));
            }
        }
        tracing::warn!(
            derived_count = fallbacks.len(),
            "failover chain: no usable [fallback_provider].chain — auto-derived fallback from all \
             enabled providers (same-protocol first) so the primary can fail over on \
             rate-limit/outage",
        );
    }

    fallbacks
}

/// Build the P0 rescue triple from `[stability]`. Each field is independent.
pub fn build_stability_triple(config: &Config) -> StabilityTriple {
    let Some(s) = config.stability.as_ref() else {
        return StabilityTriple {
            stall_config: None,
            consecutive_failure_cap: None,
            turn_timeout: None,
        };
    };
    let stall_config = s.stall_timeout_secs.map(|secs| {
        let mut sc = StallConfig::default().with_timeout(Duration::from_secs(secs));
        if let Some(ci) = s.stall_check_interval_secs {
            sc = sc.with_check_interval(Duration::from_secs(ci));
        }
        sc
    });
    StabilityTriple {
        stall_config,
        consecutive_failure_cap: s.consecutive_failure_cap,
        turn_timeout: s.turn_timeout_secs.map(Duration::from_secs),
    }
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
        let model = provider.and_then(|p| p.models.first().map(String::as_str));
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
            derived.budget.usable
        }
    };
    // Per-model trigger-point override, keyed off the resolved chain-min model.
    // Each field falls back to the global threshold, then the built-in default,
    // so an absent or non-matching override is byte-identical to before.
    let model_override = cb.threshold_override_for(derived.model.as_deref(), &derived.provider);
    if let Some(o) = model_override {
        tracing::info!(
            matcher = %o.model,
            model = derived.model.as_deref().unwrap_or("<unknown>"),
            provider = %derived.provider,
            "context budget: applying per-model threshold override"
        );
    }
    let warning_threshold = model_override
        .and_then(|o| o.warning_threshold)
        .or(cb.warning_threshold)
        .unwrap_or(0.70);
    let critical_threshold = model_override
        .and_then(|o| o.critical_threshold)
        .or(cb.critical_threshold)
        .unwrap_or(0.85);

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
        token_estimate_ratio: 3.5,
        fresh_tail_count: 6,
        circuit_breaker_max: 3,
        diminishing_window: 4,
        diminishing_threshold: 500,
        max_splits: 3,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::{ContextBudgetToml, FallbackProviderToml, ProviderConfig, StabilityToml};

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
        assert_eq!(
            bc.token_budget,
            DEFAULT_CONTEXT_TOKEN_BUDGET - DEFAULT_OUTPUT_RESERVE
        );
        assert_eq!(bc.warning_threshold, 0.70);
        assert_eq!(bc.critical_threshold, 0.85);
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
        let mut providers: HashMap<String, ProviderConfig> = HashMap::new();
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
            model_thresholds: vec![crate::ModelThresholdToml {
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
            model_thresholds: vec![crate::ModelThresholdToml {
                model: "kimi".to_string(),
                warning_threshold: Some(0.60),
                critical_threshold: None,
            }],
            ..ContextBudgetToml::default()
        };
        let cfg = cfg_with_primary("moonshot", ProviderConfig::test_config("kimi-k2"), cb);
        let bc = build_context_budget_config(&cfg, "moonshot").expect("enabled → Some");
        assert_eq!(bc.warning_threshold, 0.60, "override wins for the set field");
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
            model_thresholds: vec![crate::ModelThresholdToml {
                model: "claude".to_string(),
                warning_threshold: Some(0.50),
                critical_threshold: Some(0.60),
            }],
            ..ContextBudgetToml::default()
        };
        let cfg = cfg_with_primary("moonshot", ProviderConfig::test_config("kimi-k2"), cb);
        let bc = build_context_budget_config(&cfg, "moonshot").expect("enabled → Some");
        assert_eq!(bc.warning_threshold, 0.70);
        assert_eq!(bc.critical_threshold, 0.85);
    }

    #[test]
    fn build_per_model_override_applies_with_explicit_token_budget() {
        // Even when the operator pins token_budget, the model is still resolved
        // so a per-model threshold override still applies (matched on provider key).
        let cb = ContextBudgetToml {
            enabled: true,
            token_budget: Some(50_000),
            model_thresholds: vec![crate::ModelThresholdToml {
                model: "moonshot".to_string(),
                warning_threshold: Some(0.55),
                ..crate::ModelThresholdToml::default()
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
            model_thresholds: vec![crate::ModelThresholdToml {
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

    fn cfg_with_fallback(
        fb: Option<FallbackProviderToml>,
        providers: Vec<(&str, ProviderConfig)>,
    ) -> Config {
        let mut providers_map: std::collections::HashMap<String, ProviderConfig> =
            std::collections::HashMap::new();
        for (k, v) in providers {
            providers_map.insert(k.to_string(), v);
        }
        Config {
            fallback_provider: fb,
            providers: providers_map,
            ..Config::default()
        }
    }

    fn mock_provider_config() -> ProviderConfig {
        let mut pc = ProviderConfig::test_config("mock-model");
        pc.protocol = Some("mock".to_string());
        pc
    }

    fn provider_config_with_protocol(proto: &str) -> ProviderConfig {
        let mut pc = ProviderConfig::test_config("mock-model");
        pc.protocol = Some(proto.to_string());
        pc
    }

    fn mock_handle(name: &str) -> Arc<dyn DefaultProviderHandle> {
        let provider = create_provider(name, mock_provider_config()).expect("mock provider");
        Arc::new(StaticDefault::new(provider))
    }

    /// The already-built non-primary provider set, exactly as
    /// `build_failover_chain` constructs it before assembling the chain.
    fn built_map(names: &[&str]) -> HashMap<String, Arc<dyn AiProvider>> {
        names
            .iter()
            .map(|n| {
                (
                    n.to_string(),
                    create_provider(n, mock_provider_config()).expect("mock provider"),
                )
            })
            .collect()
    }

    fn fallback_names(nodes: &[FailoverNode]) -> Vec<&str> {
        nodes.iter().map(|n| n.name.as_str()).collect()
    }

    #[test]
    fn assemble_fallbacks_auto_derives_when_no_chain_configured() {
        // No `[fallback_provider]` at all, but two other enabled providers
        // exist → they become the fallback chain (sorted) so the primary can
        // still fail over.
        let cfg = cfg_with_fallback(
            None,
            vec![
                ("primary", mock_provider_config()),
                ("aux2", mock_provider_config()),
                ("aux1", mock_provider_config()),
            ],
        );
        let built = built_map(&["aux1", "aux2"]);
        let nodes = assemble_fallbacks(&cfg, "primary", &built, &HashMap::new());
        assert_eq!(fallback_names(&nodes), vec!["aux1", "aux2"]);
    }

    #[test]
    fn assemble_fallbacks_auto_derives_when_chain_entries_all_invalid() {
        // The real-world bug: the chain names providers that were never defined
        // under `[providers]` (`minimax`/`chatgpt`), so every entry skips. The
        // one healthy alternative is auto-derived instead of leaving the chain
        // empty (which previously made a 429 hard-fail with nowhere to migrate).
        let cfg = cfg_with_fallback(
            Some(FallbackProviderToml {
                chain: vec!["minimax".to_string(), "chatgpt".to_string()],
                provider: None,
                max_retries: None,
            }),
            vec![
                ("kimi", mock_provider_config()),
                ("x302", mock_provider_config()),
            ],
        );
        // `kimi` is the primary, so only `x302` is in the built set.
        let built = built_map(&["x302"]);
        let nodes = assemble_fallbacks(&cfg, "kimi", &built, &HashMap::new());
        assert_eq!(fallback_names(&nodes), vec!["x302"]);
    }

    #[test]
    fn assemble_fallbacks_prefers_same_protocol_in_auto_derive() {
        // Primary is anthropic-protocol (e.g. Kimi). Auto-derive must list the
        // same-protocol fallback first even though its name sorts LATER, so the
        // derived chain stays homogeneous and avoids the cross-protocol request
        // conversion that an OpenAI-compat endpoint rejects (-10003).
        let cfg = cfg_with_fallback(
            None,
            vec![
                ("primary", provider_config_with_protocol("anthropic")),
                ("a_openai", provider_config_with_protocol("openai")),
                ("z_anthropic", provider_config_with_protocol("anthropic")),
            ],
        );
        let built = built_map(&["a_openai", "z_anthropic"]);
        let nodes = assemble_fallbacks(&cfg, "primary", &built, &HashMap::new());
        // Same-protocol (anthropic) first despite the later name, then the
        // cross-protocol one — protocol affinity beats the name-sort tiebreak.
        assert_eq!(fallback_names(&nodes), vec!["z_anthropic", "a_openai"]);
    }

    #[test]
    fn assemble_fallbacks_honors_explicit_chain_without_auto_deriving() {
        // A valid explicit chain is used verbatim; auto-derivation must NOT
        // append other enabled providers (`aux`) behind operator intent.
        let cfg = cfg_with_fallback(
            Some(FallbackProviderToml {
                chain: vec!["fb".to_string()],
                provider: None,
                max_retries: None,
            }),
            vec![
                ("primary", mock_provider_config()),
                ("fb", mock_provider_config()),
                ("aux", mock_provider_config()),
            ],
        );
        let built = built_map(&["fb", "aux"]);
        let nodes = assemble_fallbacks(&cfg, "primary", &built, &HashMap::new());
        assert_eq!(fallback_names(&nodes), vec!["fb"]);
    }

    #[test]
    fn failover_chain_wraps_primary_even_without_fallback_section() {
        // No `[fallback_provider]` — the primary is still wrapped so it gains
        // model-level fallback, transient retry, and a circuit breaker.
        let cfg = cfg_with_fallback(None, vec![("primary", mock_provider_config())]);
        let chain = build_failover_chain(&cfg, "primary", mock_handle("primary"), None, None);
        assert_eq!(chain.default.current().name(), "failover");
        // Only the primary is configured → no per-hint overrides.
        assert!(chain.agent_overrides.is_empty());
    }

    #[test]
    fn failover_chain_builds_with_configured_chain() {
        let cfg = cfg_with_fallback(
            Some(FallbackProviderToml {
                chain: vec!["fb".to_string()],
                provider: None,
                max_retries: None,
            }),
            vec![
                ("primary", mock_provider_config()),
                ("fb", mock_provider_config()),
            ],
        );
        let chain = build_failover_chain(&cfg, "primary", mock_handle("primary"), None, None);
        assert_eq!(chain.default.current().name(), "failover");
        // `fb` is a non-primary configured provider → it gets a pinned override;
        // the primary never gets one (hinting it == no hint).
        assert!(chain.agent_overrides.contains_key("fb"));
        assert!(!chain.agent_overrides.contains_key("primary"));
        assert_eq!(chain.agent_overrides["fb"].name(), "failover");
    }

    #[test]
    fn failover_chain_skips_self_reference_and_unknown_providers() {
        // `Primary` (self, case-insensitive) and `ghost` (absent from
        // `[providers]`) are both skipped with a warn log; the build still
        // succeeds and yields a failover handle.
        let cfg = cfg_with_fallback(
            Some(FallbackProviderToml {
                chain: vec!["Primary".to_string(), "ghost".to_string()],
                provider: None,
                max_retries: None,
            }),
            vec![("primary", mock_provider_config())],
        );
        let chain = build_failover_chain(&cfg, "primary", mock_handle("primary"), None, None);
        assert_eq!(chain.default.current().name(), "failover");
    }

    #[test]
    fn agent_overrides_cover_every_non_primary_provider() {
        // Three providers configured; `aux1` / `aux2` are not even in the
        // fallback chain, but per-hint overrides still cover them so any agent
        // can pin any configured provider.
        let cfg = cfg_with_fallback(
            Some(FallbackProviderToml {
                chain: vec!["aux1".to_string()],
                provider: None,
                max_retries: None,
            }),
            vec![
                ("primary", mock_provider_config()),
                ("aux1", mock_provider_config()),
                ("aux2", mock_provider_config()),
            ],
        );
        let chain = build_failover_chain(&cfg, "primary", mock_handle("primary"), None, None);
        assert_eq!(chain.agent_overrides.len(), 2);
        assert!(chain.agent_overrides.contains_key("aux1"));
        assert!(chain.agent_overrides.contains_key("aux2"));
        assert!(!chain.agent_overrides.contains_key("primary"));
    }

    #[test]
    fn stability_triple_independence_all_none() {
        let cfg = Config::default();
        let triple = build_stability_triple(&cfg);
        assert!(triple.stall_config.is_none());
        assert!(triple.consecutive_failure_cap.is_none());
        assert!(triple.turn_timeout.is_none());
    }

    #[test]
    fn stability_triple_only_turn_timeout_set() {
        let cfg = Config {
            stability: Some(StabilityToml {
                stall_timeout_secs: None,
                stall_check_interval_secs: None,
                consecutive_failure_cap: None,
                turn_timeout_secs: Some(60),
            }),
            ..Config::default()
        };
        let triple = build_stability_triple(&cfg);
        assert!(triple.stall_config.is_none());
        assert!(triple.consecutive_failure_cap.is_none());
        assert_eq!(triple.turn_timeout, Some(Duration::from_secs(60)));
    }
}
