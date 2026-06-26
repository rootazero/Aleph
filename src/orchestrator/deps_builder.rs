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
        diminishing_window: 4,
        diminishing_threshold: 500,
        max_splits: 3,
    })
}

/// Build the cheap-tier summarization provider for [`ContextCompactor`].
///
/// Resolves the summary model in two tiers:
/// 1. **Explicit** — `[context_budget] summary_model` (Reasonix `summaryModel`
///    parity). Whatever the operator names is used verbatim.
/// 2. **Auto** — when `summary_model` is unset/blank, fall back to the *primary*
///    provider preset's declared cheap aux model (`default_aux_model`:
///    openai→`gpt-5.4-mini`, anthropic→`claude-haiku-4-5`,
///    gemini→`gemini-2.5-flash-lite`, deepseek→`deepseek-chat`). This makes the
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

    // Tier 1: explicit operator override. Tier 2: the primary preset's declared
    // cheap aux model (`default_aux_model`, never the `default_model` fallback).
    let explicit = cb
        .summary_model
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let summary_model: String = match explicit {
        Some(model) => model.to_string(),
        None => crate::providers::get_preset(&primary_provider_key.to_lowercase())
            .and_then(|p| p.default_aux_model)?
            .to_string(),
    };

    // No-op when the resolved summary model is the primary's own default model —
    // the rebuilt provider would be byte-identical, so reuse the main LLM.
    if base
        .models
        .first()
        .is_some_and(|m| m.as_str() == summary_model)
    {
        return None;
    }

    let source = if explicit.is_some() {
        "summary_model"
    } else {
        "preset aux_model"
    };
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
    use crate::{
        ContextBudgetToml, FallbackProviderToml, ProviderConfig, StabilityToml, StrategyToml,
    };

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
            model_thresholds: vec![crate::ModelThresholdToml {
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
        assert_eq!(window_aware_fresh_tail(FRESH_TAIL_ANCHOR_BUDGET), FRESH_TAIL_BASE_COUNT);
        assert_eq!(window_aware_fresh_tail(MIN_USABLE_BUDGET), FRESH_TAIL_BASE_COUNT);
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

    // ── cheap-tier summary provider wiring (Reasonix summaryModel parity) ──

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
